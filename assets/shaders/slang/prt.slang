// PRT64 + 64-tree runtime GI compute passes (Pass 1-3 of EvaluatePRT64).
//
// Contract: `docs/prt-gi-64tree-design.md` §4-5.
//   * Pass 1 (`clusterEvalMain`): evaluates all 64-coeff clusters against
//     the current sky/sun SH64 vector (1 workgroup = 1 cluster, 64 threads).
//   * Pass 2 (`propagateMain`): accumulates dynamic lights into per-leaf
//     Q8.12 uint32 atomics (`InterlockedAdd`, no float32 atomics). Aborts and
//     flags `overflowFlag[0] = 1` if `light_count > 64`.
//   * Pass 3 (`resolveVolumeMain`): traverses the 64-tree at each voxel of the
//     64^3 `giVolume` texture, applies validity gating + Q8.12 decode, and
//     writes order-2 SH (9 layers) for `ForwardPass` consumption.

static const uint SH64_COUNT = 64u;
static const uint SH9_COUNT  = 9u;
static const uint NO_CHILD   = 0xFFFFFFFFu;
static const uint NO_LEAF    = 0xFFFFFFFFu;
static const float Q8_12_SCALE = 4096.0;
static const float SH_C0 = 0.282095;
static const float SH_C1 = 0.488603;

public struct PrtNodeGpu
{
    public uint  first_child;
    public uint  leaf_index;
    public uint2 child_mask;
};

public struct PrtDynamicLightGpu
{
    public float4 position_radius; // xyz = world pos, w = radius
    public float4 color_intensity; // rgb = linear color * intensity, w = unused
};

public struct PrtFrameUbo
{
    public float4 world_origin_ext;     // xyz = world_origin, w = world_extent
    public uint4  counts;               // x = leaf_count, y = node_count, z = leaf_grid, w = light_count
    public float4 grid_origin;          // xyz = gi_volume origin, w = unused
    public float4 grid_spacing;         // xyz = gi_volume spacing, w = unused
    public uint4  grid_dims;            // xyz = gi_volume dims (64,64,64), w = unused
};

[[vk::binding(0, 0)]] StructuredBuffer<PrtNodeGpu>         prtNodes;
[[vk::binding(1, 0)]] StructuredBuffer<uint>               prtClusters;
[[vk::binding(2, 0)]] StructuredBuffer<uint>               prtClusterValid;
[[vk::binding(3, 0)]] StructuredBuffer<float4>             prtLeafBounds;
[[vk::binding(4, 0)]] RWStructuredBuffer<uint2>            clusterRadiance;
[[vk::binding(5, 0)]] RWStructuredBuffer<uint>             dynAccum;
[[vk::binding(6, 0)]] RWStructuredBuffer<uint>             overflowFlag;
[[vk::binding(7, 0)]] StructuredBuffer<PrtDynamicLightGpu> dynLights;
[[vk::binding(8, 0)]] ConstantBuffer<PrtFrameUbo>          prtFrame;
[[vk::binding(9, 0)]] RWTexture3D<float4>                  giVolume;
[[vk::binding(10, 0)]] StructuredBuffer<float4>            skySh64;

float3 load_cluster_coeff(uint cluster_idx, uint coeff_idx)
{
    uint base_half = cluster_idx * 192u + coeff_idx * 3u;
    float rgb[3];
    for (uint ch = 0u; ch < 3u; ch++)
    {
        uint h = base_half + ch;
        uint w = prtClusters[h >> 1u];
        uint bits = ((h & 1u) != 0u) ? (w >> 16u) : (w & 0xFFFFu);
        rgb[ch] = f16tof32(bits);
    }
    return float3(rgb[0], rgb[1], rgb[2]);
}

uint2 pack_f16x4(float4 v)
{
    uint r = f32tof16(v.x) & 0xFFFFu;
    uint g = f32tof16(v.y) & 0xFFFFu;
    uint b = f32tof16(v.z) & 0xFFFFu;
    uint a = f32tof16(v.w) & 0xFFFFu;
    return uint2(r | (g << 16u), b | (a << 16u));
}

float4 unpack_f16x4(uint2 packed)
{
    return float4(
        f16tof32(packed.x & 0xFFFFu),
        f16tof32(packed.x >> 16u),
        f16tof32(packed.y & 0xFFFFu),
        f16tof32(packed.y >> 16u)
    );
}

uint traverse_64tree(float3 world_pos)
{
    uint node_count = prtFrame.counts.y;
    float extent = prtFrame.world_origin_ext.w;
    if (node_count == 0u || extent <= 0.0)
        return NO_LEAF;

    float3 local = (world_pos - prtFrame.world_origin_ext.xyz) / extent;
    if (any(local < 0.0) || any(local >= 1.0))
        return NO_LEAF;

    uint node_idx = 0u;
    for (uint depth = 0u; depth < 8u; depth++)
    {
        if (node_idx >= node_count)
            return NO_LEAF;
        PrtNodeGpu node = prtNodes[node_idx];
        if (node.first_child == NO_CHILD)
            return node.leaf_index;

        uint cx = min(uint(floor(local.x * 4.0)), 3u);
        uint cy = min(uint(floor(local.y * 4.0)), 3u);
        uint cz = min(uint(floor(local.z * 4.0)), 3u);
        uint slot = cx + cy * 4u + cz * 16u;

        uint word = (slot < 32u) ? node.child_mask.x : node.child_mask.y;
        uint bit = slot & 31u;
        if (((word >> bit) & 1u) == 0u)
            return NO_LEAF;

        uint lower_mask = (bit == 0u) ? 0u : ((1u << bit) - 1u);
        uint child_offset = countbits(word & lower_mask);
        if (slot >= 32u)
            child_offset += countbits(node.child_mask.x);

        node_idx = node.first_child + child_offset;
        local = local * 4.0 - float3(cx, cy, cz);
    }
    return NO_LEAF;
}

groupshared float3 s_eval_rgb[SH64_COUNT];

// Pass 1: ClusterEval (dispatch (leaf_count, 1, 1))
[shader("compute")]
[numthreads(64, 1, 1)]
void clusterEvalMain(uint3 group_id : SV_GroupID, uint3 local_id : SV_GroupThreadID)
{
    uint cluster_idx = group_id.x;
    uint c = local_id.x;
    if (cluster_idx >= prtFrame.counts.x)
        return;

    float3 coeff = load_cluster_coeff(cluster_idx, c);
    s_eval_rgb[c] = coeff * skySh64[c].rgb;
    GroupMemoryBarrierWithGroupSync();

    for (uint stride = 32u; stride > 0u; stride >>= 1u)
    {
        if (c < stride)
            s_eval_rgb[c] += s_eval_rgb[c + stride];
        GroupMemoryBarrierWithGroupSync();
    }

    if (c == 0u)
    {
        float valid = (prtClusterValid[cluster_idx] != 0u) ? 1.0 : 0.0;
        float3 rad = max(s_eval_rgb[0], float3(0.0)) * valid;
        clusterRadiance[cluster_idx] = pack_f16x4(float4(rad, valid));

        // Reset Q8.12 accumulators for Pass 2.
        uint base = cluster_idx * 3u;
        dynAccum[base + 0u] = 0u;
        dynAccum[base + 1u] = 0u;
        dynAccum[base + 2u] = 0u;
        if (cluster_idx == 0u)
            overflowFlag[0] = 0u;
    }
}

// Pass 2: Propagate dynamic lights (dispatch (leaf_count, 1, 1))
[shader("compute")]
[numthreads(64, 1, 1)]
void propagateMain(uint3 group_id : SV_GroupID, uint3 local_id : SV_GroupThreadID)
{
    uint cluster_idx = group_id.x;
    uint light_idx = local_id.x;
    if (cluster_idx >= prtFrame.counts.x)
        return;

    uint light_count = prtFrame.counts.w;
    if (light_count > 64u)
    {
        if (cluster_idx == 0u && light_idx == 0u)
            InterlockedMax(overflowFlag[0], 1u);
        return;
    }

    if (light_idx >= light_count || prtClusterValid[cluster_idx] == 0u)
        return;

    PrtDynamicLightGpu light = dynLights[light_idx];
    float4 bounds = prtLeafBounds[cluster_idx];
    float3 delta = light.position_radius.xyz - bounds.xyz;
    float dist_sq = dot(delta, delta);
    float max_dist = light.position_radius.w + bounds.w * 1.7320508;
    if (dist_sq > max_dist * max_dist)
        return;

    float3 dc = load_cluster_coeff(cluster_idx, 0u);
    float transfer_gain = clamp((dc.x + dc.y + dc.z) / 3.0, 0.0, 2.0);
    float r_sq = max(light.position_radius.w * light.position_radius.w, 1e-4);
    float atten = 1.0 / (1.0 + dist_sq / r_sq);
    float3 contrib = max(light.color_intensity.rgb * atten * transfer_gain, float3(0.0));

    uint base = cluster_idx * 3u;
    uint qr = uint(clamp(round(contrib.x * Q8_12_SCALE), 0.0, 65535.0));
    uint qg = uint(clamp(round(contrib.y * Q8_12_SCALE), 0.0, 65535.0));
    uint qb = uint(clamp(round(contrib.z * Q8_12_SCALE), 0.0, 65535.0));
    if (qr > 0u) InterlockedAdd(dynAccum[base + 0u], qr);
    if (qg > 0u) InterlockedAdd(dynAccum[base + 1u], qg);
    if (qb > 0u) InterlockedAdd(dynAccum[base + 2u], qb);
}

float3 load_leaf_radiance(uint leaf_idx, out float valid_out)
{
    if (leaf_idx == NO_LEAF || leaf_idx >= prtFrame.counts.x)
    {
        valid_out = 0.0;
        return float3(0.0);
    }
    float4 rad_v = unpack_f16x4(clusterRadiance[leaf_idx]);
    valid_out = rad_v.w;
    if (rad_v.w <= 0.0)
        return float3(0.0);

    uint base = leaf_idx * 3u;
    float3 dyn = float3(
        float(min(dynAccum[base + 0u], 0xFFFFu)),
        float(min(dynAccum[base + 1u], 0xFFFFu)),
        float(min(dynAccum[base + 2u], 0xFFFFu))
    ) / Q8_12_SCALE;
    return max(rad_v.rgb + dyn, float3(0.0));
}

groupshared float4 s_prt_tile[4][4][4];

// Pass 3: ResolveVolume into 64^3 order-2 SH texture (dispatch (16, 16, 16))
[shader("compute")]
[numthreads(4, 4, 4)]
void resolveVolumeMain(uint3 tid : SV_DispatchThreadID, uint3 lid : SV_GroupThreadID)
{
    uint3 dims = prtFrame.grid_dims.xyz;
    bool in_bounds = all(tid < dims);

    float3 world_pos = prtFrame.grid_origin.xyz + float3(tid) * prtFrame.grid_spacing.xyz;
    float center_valid = 0.0;
    float3 center_rad = float3(0.0);
    if (in_bounds)
    {
        uint center_leaf = traverse_64tree(world_pos);
        center_rad = load_leaf_radiance(center_leaf, center_valid);
    }

    // Cache center voxel radiance/validity across the 4x4x4 workgroup so interior
    // neighbor lookups avoid redundant 64-tree traversals.
    s_prt_tile[lid.x][lid.y][lid.z] = float4(center_rad, center_valid);
    GroupMemoryBarrierWithGroupSync();

    if (!in_bounds)
        return;

    // 6-neighbor directional SH1 gradient from adjacent leaves (anti-leakage gated).
    static const int3 idirs[6] = {
        int3( 1,  0,  0),
        int3(-1,  0,  0),
        int3( 0,  1,  0),
        int3( 0, -1,  0),
        int3( 0,  0,  1),
        int3( 0,  0, -1)
    };
    static const float3 dirs[6] = {
        float3( 1.0,  0.0,  0.0),
        float3(-1.0,  0.0,  0.0),
        float3( 0.0,  1.0,  0.0),
        float3( 0.0, -1.0,  0.0),
        float3( 0.0,  0.0,  1.0),
        float3( 0.0,  0.0, -1.0)
    };

    float3 sh[SH9_COUNT];
    for (uint c = 0u; c < SH9_COUNT; c++)
        sh[c] = float3(0.0);

    if (center_valid > 0.0)
    {
        float3 accum_dc = center_rad;
        float w_sum = 1.0;
        float3 dir_accum = float3(0.0);

        for (uint i = 0u; i < 6u; i++)
        {
            int3 n_lid = int3(lid) + idirs[i];
            float nvalid = 0.0;
            float3 nrad = float3(0.0);
            if (all(n_lid >= 0) && all(n_lid < 4))
            {
                float4 cached = s_prt_tile[n_lid.x][n_lid.y][n_lid.z];
                nrad = cached.rgb;
                nvalid = cached.w;
            }
            else
            {
                float3 npos = world_pos + dirs[i] * prtFrame.grid_spacing.xyz;
                uint nleaf = traverse_64tree(npos);
                nrad = load_leaf_radiance(nleaf, nvalid);
            }
            if (nvalid > 0.0)
            {
                accum_dc += nrad * 0.25;
                w_sum += 0.25;
                dir_accum += nrad * dirs[i];
            }
        }
        float3 avg_rad = accum_dc / w_sum;
        sh[0] = avg_rad / SH_C0;
        // Band 1: (y, z, x) order matching gi.rs / gi.slang.
        sh[1] = (dir_accum.y * 0.25) / SH_C1;
        sh[2] = (dir_accum.z * 0.25) / SH_C1;
        sh[3] = (dir_accum.x * 0.25) / SH_C1;
    }

    int dz = int(dims.z);
    for (uint c = 0u; c < SH9_COUNT; c++)
    {
        int z = int(c) * dz + int(tid.z);
        giVolume[int3(int(tid.x), int(tid.y), z)] = float4(sh[c], 1.0);
    }
}
