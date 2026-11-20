// Multi-platform Virtual Geometry compute pipeline (`VirtualGeoPass`).
//
// Coordinates: right-handed world space, column-major `mul(mat, vec)`, Vulkan clip
// depth in `[0, 1]` with optional Reverse-Z conversion (`flags & 1u`).
//
// Entry points (all sharing Set 0):
//   1. vgCullMain     - Cluster-DAG cut evaluation, frustum + normal-cone + HZB
//                       culling, fault queueing, and HW/SW/SGGX queue routing.
//   2. vgSwRasterMain - 32-bit atomic Reverse-Z software rasterizer for sub-pixel
//                       micro-triangle clusters (< 4 px^2) and splat surrogates.
//   3. vgResolveMain  - Analytic perspective-correct barycentric reconstruction,
//                       quotient-rule screen derivatives (`ddx/ddy`), SGGX foliage
//                       BRDF/transmittance, and Geomorphing-compensated velocity.
//   4. vgHzbBuildMain - 2x2 Reverse-Z min-depth hierarchical Z pyramid downsample.

static const uint VG_PRIM_TRIANGLES          = 0u;
static const uint VG_PRIM_DMM                = 1u;
static const uint VG_PRIM_SGGX_SPLATS        = 2u;

static const float VG_MICRO_TRI_THRESHOLD    = 4.0;
static const uint  VG_FLAG_REVERSE_Z_PROJ    = 1u << 0u;
static const uint  VG_FLAG_HAS_PREV_HZB      = 1u << 1u;

public struct VgFrameUbo
{
    public float4x4 viewProj;
    public float4x4 invViewProj;
    public float4x4 prevViewProj;
    public float4   cameraPosNear;     // xyz = camera world pos, w = near plane
    public float4   screenParams;      // x = width, y = height, z = cot_half_fovy, w = error_threshold_px
    public float4   sunDirIntensity;   // xyz = dir to sun, w = sun intensity
    public float4   sunColorAmbient;   // rgb = sun linear color, w = ambient factor
    public uint4    counts;            // x = group_count, y = cluster_count, z = hzb_mip_count, w = flags
};

public struct VgGroupGpu
{
    public float4 boundingSphere;      // xyz = local center, w = radius
    public float4 errorAndRange;       // x = parent_error, y = self_error, z = geomorph_window, w = reserved
    public uint4  topology;            // x = group_id, y = lod | (prim << 8) | (resident << 16), z = first_cluster, w = cluster_count
};

public struct VgClusterGpu
{
    public float4 aabbMin;             // xyz = local min, w = parent_error
    public float4 aabbMax;             // xyz = local max, w = self_error
    public float4 normalCone;          // xyz = cone axis, w = sin(half_angle)
    public uint4  meta;                // x = cluster_id, y = group_id, z = vertex_offset_words, w = tri_offset_words
    public uint4  countsAndMat;        // x = vertex_count, y = tri_count, z = material_idx | (prim << 16), w = geomorph_scale_bits
    public float4 splatCenterOpacity;  // xyz = splat center, w = opacity
    public float4 splatCovDiag;        // xyz = sxx, syy, szz, w = max_scale
    public float4 splatCovOff;         // xyz = sxy, sxz, syz, w = optical_density
};

public struct VgDispatchedClusterGpu
{
    public uint4 item;                 // x = cluster_id, y = instance_id, z = asuint(morph_alpha), w = primitive_kind
};

public struct VgPushConstants
{
    public uint passMode;
    public uint width;
    public uint height;
    public uint mipLevel;
};

[[vk::push_constant]]
VgPushConstants vgPc;

[[vk::binding(0, 0)]] ConstantBuffer<VgFrameUbo>                    vgFrame;
[[vk::binding(1, 0)]] StructuredBuffer<VgGroupGpu>                  vgGroups;
[[vk::binding(2, 0)]] StructuredBuffer<VgClusterGpu>                vgClusters;
[[vk::binding(3, 0)]] StructuredBuffer<uint>                        vgPackedVertices;
[[vk::binding(4, 0)]] StructuredBuffer<uint>                        vgPackedIndices;
[[vk::binding(5, 0)]] StructuredBuffer<float4x4>                    vgInstanceModels;
[[vk::binding(6, 0)]] RWStructuredBuffer<VgDispatchedClusterGpu>    vgHwQueue;
[[vk::binding(7, 0)]] RWStructuredBuffer<VgDispatchedClusterGpu>    vgSwQueue;
[[vk::binding(8, 0)]] RWStructuredBuffer<VgDispatchedClusterGpu>    vgSplatQueue;
[[vk::binding(9, 0)]] RWStructuredBuffer<uint>                      vgQueueCounters;
[[vk::binding(10, 0)]] RWStructuredBuffer<uint2>                    vgFaultBuffer;
[[vk::binding(11, 0)]] RWStructuredBuffer<uint>                     vgVisAtomicBuffer;
[[vk::binding(12, 0)]] Texture2D<float>                             hzbInputTex;
[[vk::binding(13, 0)]] [[vk::image_format("r32f")]]    RWTexture2D<float>  hzbOutputTex;
[[vk::binding(14, 0)]] [[vk::image_format("rgba16f")]] RWTexture2D<float4> outColorTex;
[[vk::binding(15, 0)]] [[vk::image_format("r32f")]]    RWTexture2D<float>  outDepthTex;
[[vk::binding(16, 0)]] [[vk::image_format("rgba16f")]] RWTexture2D<float4> outNormalRoughnessTex;
[[vk::binding(17, 0)]] [[vk::image_format("rg16f")]]   RWTexture2D<float2> outVelocityTex;

struct DecodedVertex
{
    float3 pos;
    float3 prevPos;
    float3 normal;
    float2 uv;
};

float unpackHalf16(uint bits)
{
    return f16tof32(bits & 0xFFFFu);
}

float3 decodeOctNormal(uint packedOct)
{
    int ix = int(int(packedOct << 24) >> 24);
    int iy = int(int((packedOct >> 8u) << 24) >> 24);
    float2 f = clamp(float2(float(ix), float(iy)) / 127.0, float2(-1.0), float2(1.0));
    float nz = 1.0 - abs(f.x) - abs(f.y);
    float2 xy = f;
    if (nz < 0.0)
    {
        xy = float2((1.0 - abs(f.y)) * (f.x >= 0.0 ? 1.0 : -1.0),
                    (1.0 - abs(f.x)) * (f.y >= 0.0 ? 1.0 : -1.0));
    }
    return normalize(float3(xy.x, xy.y, nz));
}

DecodedVertex loadClusterVertex(VgClusterGpu cluster, uint localVertexIdx, float morphAlpha)
{
    // 20 bytes per vertex packed as 5 uint32 words in `vgPackedVertices`.
    uint baseWord = cluster.meta.z + localVertexIdx * 5u;
    uint w0 = vgPackedVertices[baseWord + 0u];
    uint w1 = vgPackedVertices[baseWord + 1u];
    uint w2 = vgPackedVertices[baseWord + 2u];
    uint w3 = vgPackedVertices[baseWord + 3u];
    uint w4 = vgPackedVertices[baseWord + 4u];

    float qx = float(w0 & 0xFFFFu) / 65535.0;
    float qy = float(w0 >> 16u) / 65535.0;
    float qz = float(w1 & 0xFFFFu) / 65535.0;

    float3 span = max(cluster.aabbMax.xyz - cluster.aabbMin.xyz, float3(1e-6));
    float3 basePos = cluster.aabbMin.xyz + float3(qx, qy, qz) * span;

    int dxq = int(int((w3 >> 16u) << 16) >> 16);
    int dyq = int(int(w4 << 16) >> 16);
    int dzq = int(int(w4) >> 16);
    float geoScale = asfloat(cluster.countsAndMat.w);
    float3 delta = (float3(float(dxq), float(dyq), float(dzq)) / 32767.0) * geoScale;

    DecodedVertex outV;
    outV.pos = basePos + saturate(morphAlpha) * delta;
    outV.prevPos = basePos + delta;
    outV.normal = decodeOctNormal((w1 >> 16u) & 0xFFFFu);
    outV.uv = float2(unpackHalf16(w2 & 0xFFFFu), unpackHalf16(w2 >> 16u));
    return outV;
}

uint3 loadClusterTriangle(VgClusterGpu cluster, uint triIdx)
{
    uint word = vgPackedIndices[cluster.meta.w + triIdx];
    return uint3(word & 0xFFu, (word >> 8u) & 0xFFu, (word >> 16u) & 0xFFu);
}

float toReverseZ(float ndcZ)
{
    bool isRevZ = (vgFrame.counts.w & VG_FLAG_REVERSE_Z_PROJ) != 0u;
    return isRevZ ? saturate(ndcZ) : saturate(1.0 - ndcZ);
}

float projectErrorPx(float worldError, float4 worldSphere)
{
    float dist = length(worldSphere.xyz - vgFrame.cameraPosNear.xyz);
    float denom = max(dist - worldSphere.w, max(vgFrame.cameraPosNear.w, 1e-3));
    float halfH = 0.5 * max(vgFrame.screenParams.y, 1.0);
    return (worldError * vgFrame.screenParams.z * halfH) / denom;
}

bool aabbFrustumVisible(float3 bmin, float3 bmax)
{
    float2 uvMin = float2(1e9, 1e9);
    float2 uvMax = float2(-1e9, -1e9);
    bool anyFront = false;

    for (uint i = 0u; i < 8u; i++)
    {
        float3 c = float3(
            (i & 1u) != 0u ? bmax.x : bmin.x,
            (i & 2u) != 0u ? bmax.y : bmin.y,
            (i & 4u) != 0u ? bmax.z : bmin.z
        );
        float4 clip = mul(vgFrame.viewProj, float4(c, 1.0));
        if (clip.w > 1e-4)
        {
            anyFront = true;
            float2 ndc = clip.xy / clip.w;
            uvMin = min(uvMin, ndc);
            uvMax = max(uvMax, ndc);
        }
    }

    if (!anyFront)
        return false;
    if (uvMax.x < -1.0 || uvMin.x > 1.0 || uvMax.y < -1.0 || uvMin.y > 1.0)
        return false;
    return true;
}

bool aabbHzbOccluded(float3 bmin, float3 bmax)
{
    if ((vgFrame.counts.w & VG_FLAG_HAS_PREV_HZB) == 0u)
        return false;

    uint hzbW = 1u;
    uint hzbH = 1u;
    hzbInputTex.GetDimensions(hzbW, hzbH);
    if (hzbW == 0u || hzbH == 0u)
        return false;

    float2 uvMin = float2(1.0, 1.0);
    float2 uvMax = float2(0.0, 0.0);
    float nearestRevZ = 0.0;

    for (uint i = 0u; i < 8u; i++)
    {
        float3 c = float3(
            (i & 1u) != 0u ? bmax.x : bmin.x,
            (i & 2u) != 0u ? bmax.y : bmin.y,
            (i & 4u) != 0u ? bmax.z : bmin.z
        );
        float4 clip = mul(vgFrame.viewProj, float4(c, 1.0));
        if (clip.w <= 1e-4)
            return false;
        float3 ndc = clip.xyz / clip.w;
        float2 uv = ndc.xy * 0.5 + 0.5;
        if (any(uv < 0.0) || any(uv > 1.0))
            return false;
        uvMin = min(uvMin, uv);
        uvMax = max(uvMax, uv);
        nearestRevZ = max(nearestRevZ, toReverseZ(ndc.z));
    }

    int2 px0 = clamp(int2(floor(uvMin * float2(hzbW, hzbH))), int2(0, 0), int2(int(hzbW) - 1, int(hzbH) - 1));
    int2 px1 = clamp(int2(floor(uvMax * float2(hzbW, hzbH))), int2(0, 0), int2(int(hzbW) - 1, int(hzbH) - 1));
    float minOccluder = 1.0;
    minOccluder = min(minOccluder, hzbInputTex.Load(int3(px0.x, px0.y, 0)));
    minOccluder = min(minOccluder, hzbInputTex.Load(int3(px1.x, px0.y, 0)));
    minOccluder = min(minOccluder, hzbInputTex.Load(int3(px0.x, px1.y, 0)));
    minOccluder = min(minOccluder, hzbInputTex.Load(int3(px1.x, px1.y, 0)));
    return nearestRevZ < (minOccluder - 1e-4);
}

float evalSggxProjectedArea(float3 diagCov, float3 offCov, float3 dir)
{
    float3 d = normalize(dir);
    float q = d.x * d.x * diagCov.x + d.y * d.y * diagCov.y + d.z * d.z * diagCov.z
            + 2.0 * (d.x * d.y * offCov.x + d.x * d.z * offCov.y + d.y * d.z * offCov.z);
    return sqrt(max(q, 0.0));
}

float evalSggxNdf(float3 diagCov, float3 offCov, float3 h)
{
    float3 hn = normalize(h);
    float sxx = max(diagCov.x, 1e-4);
    float syy = max(diagCov.y, 1e-4);
    float szz = max(diagCov.z, 1e-4);
    float sxy = offCov.x;
    float sxz = offCov.y;
    float syz = offCov.z;

    float cxx = syy * szz - syz * syz;
    float cyy = sxx * szz - sxz * sxz;
    float czz = sxx * syy - sxy * sxy;
    float cxy = sxz * syz - sxy * szz;
    float cxz = sxy * syz - sxz * syy;
    float cyz = sxy * sxz - sxx * syz;
    float det = sxx * cxx + sxy * cxy + sxz * cxz;
    if (det <= 1e-12)
        return 0.0;

    float quadAdj = max(
        hn.x * hn.x * cxx + hn.y * hn.y * cyy + hn.z * hn.z * czz
        + 2.0 * (hn.x * hn.y * cxy + hn.x * hn.z * cxz + hn.y * hn.z * cyz),
        1e-12
    );
    return pow(det, 1.5) / (3.14159265 * quadAdj * quadAdj);
}

// Pass 1: Cluster-DAG cut selection, frustum/normal/HZB culling, and queue routing.
[shader("compute")]
[numthreads(64, 1, 1)]
void vgCullMain(uint3 tid : SV_DispatchThreadID)
{
    uint clusterIdx = tid.x;
    if (clusterIdx >= vgFrame.counts.y)
        return;

    VgClusterGpu cluster = vgClusters[clusterIdx];
    uint groupIdx = cluster.meta.y;
    if (groupIdx >= vgFrame.counts.x)
        return;

    VgGroupGpu group = vgGroups[groupIdx];
    uint pageResident = (group.topology.y >> 16u) & 1u;
    if (pageResident == 0u)
    {
        uint faultSlot = 0u;
        InterlockedAdd(vgQueueCounters[3], 1u, faultSlot);
        if (faultSlot < 4096u)
            vgFaultBuffer[faultSlot] = uint2(groupIdx, asuint(group.errorAndRange.x));
        return;
    }

    float tau = max(vgFrame.screenParams.w, 0.25);
    float parentErrPx = projectErrorPx(cluster.aabbMin.w, group.boundingSphere);
    float selfErrPx = projectErrorPx(cluster.aabbMax.w, group.boundingSphere);
    if (!(parentErrPx > tau && selfErrPx <= tau))
        return;

    if (!aabbFrustumVisible(cluster.aabbMin.xyz, cluster.aabbMax.xyz))
        return;

    float3 center = 0.5 * (cluster.aabbMin.xyz + cluster.aabbMax.xyz);
    float3 extent = cluster.aabbMax.xyz - cluster.aabbMin.xyz;
    float radius = 0.5 * length(extent);
    float3 toCluster = center - vgFrame.cameraPosNear.xyz;
    float dist = length(toCluster);
    if (cluster.normalCone.w < 0.999 && dist > radius)
    {
        if (dot(normalize(cluster.normalCone.xyz), toCluster / dist) > cluster.normalCone.w)
            return;
    }

    if (aabbHzbOccluded(cluster.aabbMin.xyz, cluster.aabbMax.xyz))
        return;

    float window = max(group.errorAndRange.z, 0.01);
    float morphAlpha = clamp((parentErrPx - tau) / max(tau * window, 1e-4), 0.0, 1.0);
    float triCount = max(float(cluster.countsAndMat.y), 1.0);
    float approxAreaPx2 = max(
        (max(extent.x * extent.y, max(extent.y * extent.z, extent.x * extent.z))
         * vgFrame.screenParams.z * vgFrame.screenParams.y * 0.5)
        / max(dist * dist * triCount, 1e-3),
        0.0
    );

    uint primKind = (cluster.countsAndMat.z >> 16u) & 0xFFu;
    VgDispatchedClusterGpu item;
    item.item = uint4(clusterIdx, 0u, asuint(morphAlpha), primKind);

    if (primKind == VG_PRIM_SGGX_SPLATS)
    {
        uint slot = 0u;
        InterlockedAdd(vgQueueCounters[2], 1u, slot);
        if (slot < 65536u)
            vgSplatQueue[slot] = item;
    }
    else if (approxAreaPx2 < VG_MICRO_TRI_THRESHOLD)
    {
        uint slot = 0u;
        InterlockedAdd(vgQueueCounters[1], 1u, slot);
        if (slot < 65536u)
            vgSwQueue[slot] = item;
    }
    else
    {
        uint slot = 0u;
        InterlockedAdd(vgQueueCounters[0], 1u, slot);
        if (slot < 65536u)
            vgHwQueue[slot] = item;
    }
}

// Pass 2: Atomic 32-bit Reverse-Z software rasterizer for sub-pixel micro-triangles.
[shader("compute")]
[numthreads(64, 1, 1)]
void vgSwRasterMain(uint3 tid : SV_DispatchThreadID)
{
    uint swCount = min(vgQueueCounters[1], 512u);
    uint queueIdx = tid.x >> 7u;
    uint triIdx = tid.x & 127u;
    if (queueIdx >= swCount)
        return;

    VgDispatchedClusterGpu item = vgSwQueue[queueIdx];
    VgClusterGpu cluster = vgClusters[item.item.x];
    if (triIdx >= cluster.countsAndMat.y)
        return;

    float morphAlpha = asfloat(item.item.z);
    uint3 tri = loadClusterTriangle(cluster, triIdx);
    DecodedVertex v0 = loadClusterVertex(cluster, tri.x, morphAlpha);
    DecodedVertex v1 = loadClusterVertex(cluster, tri.y, morphAlpha);
    DecodedVertex v2 = loadClusterVertex(cluster, tri.z, morphAlpha);

    float4 c0 = mul(vgFrame.viewProj, float4(v0.pos, 1.0));
    float4 c1 = mul(vgFrame.viewProj, float4(v1.pos, 1.0));
    float4 c2 = mul(vgFrame.viewProj, float4(v2.pos, 1.0));
    if (c0.w <= 1e-4 || c1.w <= 1e-4 || c2.w <= 1e-4)
        return;

    float2 scrSize = float2(max(vgPc.width, 1u), max(vgPc.height, 1u));
    float2 p0 = (c0.xy / c0.w * 0.5 + 0.5) * scrSize;
    float2 p1 = (c1.xy / c1.w * 0.5 + 0.5) * scrSize;
    float2 p2 = (c2.xy / c2.w * 0.5 + 0.5) * scrSize;

    float z0 = toReverseZ(c0.z / c0.w);
    float z1 = toReverseZ(c1.z / c1.w);
    float z2 = toReverseZ(c2.z / c2.w);

    float area = (p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x);
    if (area <= 1e-6)
        return;
    float invArea = 1.0 / area;

    int2 bmin = max(int2(floor(min(p0, min(p1, p2)))), int2(0, 0));
    int2 bmax = min(int2(ceil(max(p0, max(p1, p2)))), int2(int(vgPc.width) - 1, int(vgPc.height) - 1));
    if (bmax.x < bmin.x || bmax.y < bmin.y)
        return;

    uint visPayload = ((queueIdx & 0x1FFu) << 7u) | (triIdx & 0x7Fu);
    for (int y = bmin.y; y <= bmax.y; y++)
    {
        for (int x = bmin.x; x <= bmax.x; x++)
        {
            float2 px = float2(float(x) + 0.5, float(y) + 0.5);
            float w0 = ((p2.x - p1.x) * (px.y - p1.y) - (p2.y - p1.y) * (px.x - p1.x)) * invArea;
            float w1 = ((p0.x - p2.x) * (px.y - p2.y) - (p0.y - p2.y) * (px.x - p2.x)) * invArea;
            float w2 = 1.0 - w0 - w1;
            if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0)
            {
                float rz = saturate(w0 * z0 + w1 * z1 + w2 * z2);
                uint rzBits = uint(clamp(rz * 65535.0, 0.0, 65535.0));
                uint packed = (rzBits << 16u) | visPayload;
                uint oldVal = 0u;
                uint pxIndex = uint(y) * max(vgPc.width, 1u) + uint(x);
                InterlockedMax(vgVisAtomicBuffer[pxIndex], packed, oldVal);
            }
        }
    }
}

// Pass 3: Analytic barycentric derivatives, SGGX lighting, and Geomorphing-compensated velocity.
[shader("compute")]
[numthreads(8, 8, 1)]
void vgResolveMain(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= vgPc.width || tid.y >= vgPc.height)
        return;

    uint pxIndex = tid.y * vgPc.width + tid.x;
    uint visVal = vgVisAtomicBuffer[pxIndex];
    if (visVal == 0u)
    {
        outColorTex[int2(tid.xy)] = float4(0.0, 0.0, 0.0, 0.0);
        outDepthTex[int2(tid.xy)] = 1.0;
        outNormalRoughnessTex[int2(tid.xy)] = float4(0.0, 1.0, 0.0, 1.0);
        outVelocityTex[int2(tid.xy)] = float2(0.0, 0.0);
        return;
    }

    uint queueIdx = (visVal >> 7u) & 0x1FFu;
    uint triIdx = visVal & 0x7Fu;
    VgDispatchedClusterGpu item = vgSwQueue[queueIdx];
    VgClusterGpu cluster = vgClusters[item.item.x];
    float morphAlpha = asfloat(item.item.z);
    uint primKind = item.item.w;

    uint3 tri = loadClusterTriangle(cluster, triIdx);
    DecodedVertex v0 = loadClusterVertex(cluster, tri.x, morphAlpha);
    DecodedVertex v1 = loadClusterVertex(cluster, tri.y, morphAlpha);
    DecodedVertex v2 = loadClusterVertex(cluster, tri.z, morphAlpha);

    float4 c0 = mul(vgFrame.viewProj, float4(v0.pos, 1.0));
    float4 c1 = mul(vgFrame.viewProj, float4(v1.pos, 1.0));
    float4 c2 = mul(vgFrame.viewProj, float4(v2.pos, 1.0));

    float2 ndcPx = (float2(tid.xy) + 0.5) / float2(max(vgPc.width, 1u), max(vgPc.height, 1u)) * 2.0 - 1.0;
    float2 n0 = c0.xy / max(c0.w, 1e-5);
    float2 n1 = c1.xy / max(c1.w, 1e-5);
    float2 n2 = c2.xy / max(c2.w, 1e-5);
    float det = (n1.x - n0.x) * (n2.y - n0.y) - (n1.y - n0.y) * (n2.x - n0.x);
    float invDet = (abs(det) > 1e-9) ? (1.0 / det) : 0.0;

    float l0 = ((n2.x - n1.x) * (ndcPx.y - n1.y) - (n2.y - n1.y) * (ndcPx.x - n1.x)) * invDet;
    float l1 = ((n0.x - n2.x) * (ndcPx.y - n2.y) - (n0.y - n2.y) * (ndcPx.x - n2.x)) * invDet;
    float l2 = 1.0 - l0 - l1;

    float invW0 = 1.0 / max(c0.w, 1e-5);
    float invW1 = 1.0 / max(c1.w, 1e-5);
    float invW2 = 1.0 / max(c2.w, 1e-5);
    float oneOverW = l0 * invW0 + l1 * invW1 + l2 * invW2;
    float invSum = (abs(oneOverW) > 1e-9) ? (1.0 / oneOverW) : 0.0;
    float3 b = float3(l0 * invW0, l1 * invW1, l2 * invW2) * invSum;

    float3 worldPos = b.x * v0.pos + b.y * v1.pos + b.z * v2.pos;
    float3 prevMorphPos = b.x * v0.prevPos + b.y * v1.prevPos + b.z * v2.prevPos;
    float3 worldNormal = normalize(b.x * v0.normal + b.y * v1.normal + b.z * v2.normal);

    float4 currClip = mul(vgFrame.viewProj, float4(worldPos, 1.0));
    float4 prevClip = mul(vgFrame.prevViewProj, float4(prevMorphPos, 1.0));
    float4 currNoMorph = mul(vgFrame.viewProj, float4(prevMorphPos, 1.0));
    float2 uvCurr = currClip.xy / max(currClip.w, 1e-5) * 0.5 + 0.5;
    float2 uvPrev = prevClip.xy / max(prevClip.w, 1e-5) * 0.5 + 0.5;
    float2 uvNoMorph = currNoMorph.xy / max(currNoMorph.w, 1e-5) * 0.5 + 0.5;
    float2 velocity = (uvCurr - uvPrev) - (uvCurr - uvNoMorph);

    float3 lightDir = normalize(vgFrame.sunDirIntensity.xyz);
    float3 viewDir = normalize(vgFrame.cameraPosNear.xyz - worldPos);
    float3 halfVec = normalize(lightDir + viewDir);
    float ndotl = max(dot(worldNormal, lightDir), 0.0);

    float3 litColor = float3(0.0);
    if (primKind == VG_PRIM_SGGX_SPLATS)
    {
        float ndf = evalSggxNdf(cluster.splatCovDiag.xyz, cluster.splatCovOff.xyz, halfVec);
        float sigma = evalSggxProjectedArea(cluster.splatCovDiag.xyz, cluster.splatCovOff.xyz, lightDir);
        float transmittance = exp(-sigma * max(cluster.splatCovOff.w, 0.25));
        litColor = (float3(0.22, 0.48, 0.18) * (0.25 + 0.75 * transmittance) + float3(0.04) * ndf)
                 * vgFrame.sunColorAmbient.rgb * vgFrame.sunDirIntensity.w;
    }
    else
    {
        litColor = (vgFrame.sunColorAmbient.w * float3(0.55) + ndotl * float3(0.75))
                 * vgFrame.sunColorAmbient.rgb * vgFrame.sunDirIntensity.w;
    }

    outColorTex[int2(tid.xy)] = float4(litColor, 1.0);
    outDepthTex[int2(tid.xy)] = clamp(currClip.z / max(currClip.w, 1e-5), 0.0, 1.0);
    outNormalRoughnessTex[int2(tid.xy)] = float4(worldNormal, 0.5);
    outVelocityTex[int2(tid.xy)] = velocity;
}

// Pass 4: 2x2 Reverse-Z min-depth hierarchical Z pyramid downsample.
[shader("compute")]
[numthreads(8, 8, 1)]
void vgHzbBuildMain(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= vgPc.width || tid.y >= vgPc.height)
        return;

    uint inW = 1u;
    uint inH = 1u;
    hzbInputTex.GetDimensions(inW, inH);
    int2 base = int2(tid.xy) * 2;
    int2 maxPx = int2(max(int(inW) - 1, 0), max(int(inH) - 1, 0));

    float d00 = hzbInputTex.Load(int3(clamp(base + int2(0, 0), int2(0, 0), maxPx), 0));
    float d10 = hzbInputTex.Load(int3(clamp(base + int2(1, 0), int2(0, 0), maxPx), 0));
    float d01 = hzbInputTex.Load(int3(clamp(base + int2(0, 1), int2(0, 0), maxPx), 0));
    float d11 = hzbInputTex.Load(int3(clamp(base + int2(1, 1), int2(0, 0), maxPx), 0));

    hzbOutputTex[int2(tid.xy)] = min(min(d00, d10), min(d01, d11));
}
