// Hardware Ray Tracing (VK_KHR_ray_query) 1-spp GI + RIS emitter NEE for three-tier sparse voxels.
// Traverses per-chunk procedural AABB BLASes (1m L1 cells) via TLAS, then refines inside the
// hit L1 cell using L2 4x4x4 brick occupancy and L3 4x4x4 bitmasks.

struct VoxelFrameUbo
{
    float4x4 invViewProj;
    float4x4 viewProj;
    float4x4 prevViewProj;
    float4   sunDirRadiance;    // xyz = sun dir, w = radiance
    float4   sunColorAmbient;   // rgb = sun color, a = ambient strength
    uint4    counts;            // x = chunkCount, y = width, z = height, w = skyMode
    uint4    counts2;           // x = emitterCount, y = frameIndex, z = hasPrevHistory, w = cameraMoved
    float4   cameraPosTime;     // xyz = camera world pos, w = timeSeconds
};

struct VoxelChunkGpu
{
    int4  originAndL1Offset;    // xyz = origin_l1, w = base L1 cell index
    uint4 dimsAndBrickOffset;   // xyz = dims_l1, w = base brick index
};

struct VoxelEmitterGpu
{
    float4 bminRange;           // xyz = bmin, w = maxRange
    float4 bmaxRadius;          // xyz = bmax, w = softRadius
    float4 colorIntensity;      // rgb = linear radiance, w = powerWeight
};

[[vk::binding(0, 0)]]  ConstantBuffer<VoxelFrameUbo>        ubo;
[[vk::binding(1, 0)]]  StructuredBuffer<VoxelChunkGpu>      chunks;
[[vk::binding(2, 0)]]  StructuredBuffer<uint4>              l1Cells;
[[vk::binding(3, 0)]]  StructuredBuffer<uint2>              brickOcc;
[[vk::binding(4, 0)]]  StructuredBuffer<uint>               brickVoxels;
[[vk::binding(5, 0)]]  StructuredBuffer<VoxelEmitterGpu>    emitters;
[[vk::binding(6, 0)]]  RaytracingAccelerationStructure      voxelTlas;
[[vk::binding(7, 0)]]  Texture2D<float4>                    posNormTex;
[[vk::binding(8, 0)]]  Texture2D<float4>                    prevGiTex;
[[vk::binding(9, 0)]]  [[vk::image_format("rgba16f")]] RWTexture2D<float4> giOutTex;

static const float VOXEL_PI = 3.14159265;
static const uint  L1_UNIFORM_REF = 0xFFFFFFFFu;

float interleaved_gradient_noise(uint2 px, uint frameIdx)
{
    float3 magic = float3(0.06711056, 0.00583715, 52.9829189);
    float2 p = float2(px) + float2(float(frameIdx & 63u) * 5.588238, float(frameIdx & 63u) * 5.588238);
    return frac(magic.z * frac(dot(p, magic.xy)));
}

float3 decode_normal_id(int id)
{
    if (id == 0) return float3( 1.0,  0.0,  0.0);
    if (id == 1) return float3(-1.0,  0.0,  0.0);
    if (id == 2) return float3( 0.0,  1.0,  0.0);
    if (id == 3) return float3( 0.0, -1.0,  0.0);
    if (id == 4) return float3( 0.0,  0.0,  1.0);
    return float3(0.0, 0.0, -1.0);
}

void build_orthonormal_basis(float3 n, out float3 tangent, out float3 bitangent)
{
    float3 up = (abs(n.y) < 0.99) ? float3(0.0, 1.0, 0.0) : float3(1.0, 0.0, 0.0);
    tangent = normalize(cross(up, n));
    bitangent = cross(n, tangent);
}

float3 sample_cosine_hemisphere(float3 normal, float3 tangent, float3 bitangent, float u1, float u2)
{
    float phi = u1 * 6.2831853;
    float cosThetaSq = clamp(1.0 - u2, 0.02, 1.0);
    float cosTheta = sqrt(cosThetaSq);
    float sinTheta = sqrt(max(1.0 - cosThetaSq, 0.0));
    return normalize(tangent * (cos(phi) * sinTheta) + bitangent * (sin(phi) * sinTheta) + normal * cosTheta);
}

float3 unpack_rgb(uint rgba)
{
    return float3(
        float(rgba & 0xFFu) / 255.0,
        float((rgba >> 8) & 0xFFu) / 255.0,
        float((rgba >> 16) & 0xFFu) / 255.0
    );
}

bool intersect_aabb(float3 origin, float3 invDir, float3 bmin, float3 bmax, float tMin, float tMax, out float tEnter, out float tExit)
{
    float3 t0 = (bmin - origin) * invDir;
    float3 t1 = (bmax - origin) * invDir;
    float3 tmin3 = min(t0, t1);
    float3 tmax3 = max(t0, t1);
    tEnter = max(max(tmin3.x, tmin3.y), max(tmin3.z, tMin));
    tExit  = min(min(tmax3.x, tmax3.y), min(tmax3.z, tMax));
    return tExit >= tEnter;
}

struct RtVoxelHit
{
    bool   hit;
    float  t;
    float3 worldPos;
    float3 normal;
    float3 albedo;
    float3 emissive;
};

RtVoxelHit trace_rt_voxel(float3 origin, float3 dir, float maxT, bool anyHitOnly)
{
    RtVoxelHit res;
    res.hit = false;
    res.t = maxT;
    res.worldPos = float3(0.0, 0.0, 0.0);
    res.normal = float3(0.0, 1.0, 0.0);
    res.albedo = float3(0.0, 0.0, 0.0);
    res.emissive = float3(0.0, 0.0, 0.0);

    RayDesc ray;
    ray.Origin = origin;
    ray.Direction = dir;
    ray.TMin = 0.001;
    ray.TMax = maxT;

    RayQuery<RAY_FLAG_NONE> q;
    q.TraceRayInline(voxelTlas, RAY_FLAG_NONE, 0xFF, ray);

    float3 invDir = float3(
        abs(dir.x) > 1e-6 ? 1.0 / dir.x : 1e30,
        abs(dir.y) > 1e-6 ? 1.0 / dir.y : 1e30,
        abs(dir.z) > 1e-6 ? 1.0 / dir.z : 1e30
    );

    while (q.Proceed())
    {
        if (q.CandidateType() == CANDIDATE_PROCEDURAL_PRIMITIVE)
        {
            uint chunkIdx = q.CandidateInstanceIndex();
            VoxelChunkGpu chunk = chunks[chunkIdx];
            float3 chunkOrigin = float3(chunk.originAndL1Offset.xyz);
            float3 relOrigin = origin - chunkOrigin;

            // Locate the candidate L1 cell from the ray entry point along the ray.
            float tCand = q.CommittedRayT();
            float3 pLocal = relOrigin + dir * max(0.001, tCand);
            int3 l1Cell = clamp(int3(floor(pLocal)), int3(0, 0, 0), int3(chunk.dimsAndBrickOffset.xyz) - 1);

            uint l1Flat = uint(chunk.originAndL1Offset.w)
                + uint(l1Cell.x)
                + uint(l1Cell.y) * chunk.dimsAndBrickOffset.x
                + uint(l1Cell.z) * chunk.dimsAndBrickOffset.x * chunk.dimsAndBrickOffset.y;
            uint4 l1 = l1Cells[l1Flat];
            if (l1.x == 0u)
                continue;

            float tEnter, tExit;
            if (!intersect_aabb(relOrigin, invDir, float3(l1Cell), float3(l1Cell) + 1.0, 0.001, res.t, tEnter, tExit))
                continue;

            if (l1.x == L1_UNIFORM_REF)
            {
                q.CommitProceduralPrimitiveHit(tEnter);
                if (tEnter < res.t)
                {
                    res.hit = true;
                    res.t = tEnter;
                    res.worldPos = origin + dir * tEnter;
                    res.albedo = unpack_rgb(l1.y);
                    uint emitFlag = (l1.z >> 8) & 0xFFu;
                    res.emissive = (emitFlag > 0u) ? res.albedo * (float(emitFlag) * 0.04) : float3(0.0, 0.0, 0.0);
                }
                if (anyHitOnly)
                    q.Abort();
                continue;
            }

            // Fine 4x4x4 L2 brick check inside the 1m L1 cell.
            uint brickIdx = chunk.dimsAndBrickOffset.w + (l1.x - 1u);
            uint2 occ = brickOcc[brickIdx];
            if ((occ.x | occ.y) == 0u)
                continue;

            q.CommitProceduralPrimitiveHit(tEnter);
            if (tEnter < res.t)
            {
                res.hit = true;
                res.t = tEnter;
                res.worldPos = origin + dir * tEnter;
                uint sampleVoxel = brickVoxels[brickIdx * 64u];
                res.albedo = unpack_rgb(sampleVoxel);
            }
            if (anyHitOnly)
                q.Abort();
        }
    }

    return res;
}

// RIS single-emitter NEE: samples 1 emitter proportionally to weight instead of looping all emitters.
float3 eval_ris_emitter(float3 worldPos, float3 normal, float uRand)
{
    uint emitterCount = ubo.counts2.x;
    if (emitterCount == 0u)
        return float3(0.0, 0.0, 0.0);

    uint idx = min(uint(uRand * float(emitterCount)), emitterCount - 1u);
    VoxelEmitterGpu em = emitters[idx];
    float3 center = 0.5 * (em.bminRange.xyz + em.bmaxRadius.xyz);
    float maxRange = em.bminRange.w;
    float softR = max(em.bmaxRadius.w, 0.50);

    float3 toE = center - worldPos;
    float dist = length(toE);
    if (dist > maxRange || dist < 0.05)
        return float3(0.0, 0.0, 0.0);

    float3 dir = toE / dist;
    float ndl = max(dot(normal, dir), 0.0);
    if (ndl <= 0.0)
        return float3(0.0, 0.0, 0.0);

    RtVoxelHit sh = trace_rt_voxel(worldPos + normal * 0.03, dir, dist - 0.05, true);
    if (sh.hit)
        return float3(0.0, 0.0, 0.0);

    float effDist = max(dist, 0.35);
    float atten = 1.0 / (effDist * effDist + softR * softR);
    float fade = saturate(1.0 - effDist / maxRange);
    fade *= fade;
    return em.colorIntensity.rgb * (em.colorIntensity.w * ndl * atten * fade * float(emitterCount) * 0.25);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void voxelRtGiMain(uint3 tid : SV_DispatchThreadID)
{
    uint width = ubo.counts.y;
    uint height = ubo.counts.z;
    if (tid.x >= width || tid.y >= height)
        return;

    float4 pn = posNormTex.Load(int3(tid.xy, 0));
    if (pn.w < -0.5)
    {
        giOutTex[tid.xy] = float4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    float3 worldPos = pn.xyz;
    float3 normal = decode_normal_id(int(round(pn.w)));
    float3 tangent, bitangent;
    build_orthonormal_basis(normal, tangent, bitangent);

    uint frameIdx = ubo.counts2.y;
    float u1 = interleaved_gradient_noise(tid.xy, frameIdx);
    float u2 = interleaved_gradient_noise(tid.xy + uint2(17u, 29u), frameIdx + 5u);

    float3 d1 = sample_cosine_hemisphere(normal, tangent, bitangent, u1, u2 * 0.90);
    RtVoxelHit h1 = trace_rt_voxel(worldPos + normal * 0.03, d1, 16.0, false);

    float3 sunDir = normalize(ubo.sunDirRadiance.xyz);
    float3 sunCol = ubo.sunColorAmbient.rgb * max(0.25, ubo.sunDirRadiance.w * 2.3);
    float ambientStrength = ubo.sunColorAmbient.a;

    float3 gi = float3(0.0, 0.0, 0.0);
    float ao = 1.0;

    if (!h1.hit)
    {
        gi = sunCol * (ambientStrength * 0.25);
    }
    else
    {
        if (h1.t < 2.8)
            ao = clamp(1.0 - (1.0 - h1.t / 2.8) * 0.85, 0.22, 1.0);

        float ndl = max(dot(h1.normal, sunDir), 0.0);
        float vis = 0.0;
        if (ndl > 0.0)
        {
            RtVoxelHit sh = trace_rt_voxel(h1.worldPos + h1.normal * 0.03, sunDir, 28.0, true);
            vis = sh.hit ? 0.0 : 1.0;
        }
        gi = h1.albedo * (sunCol * (ndl * vis / VOXEL_PI)) + h1.emissive;
    }

    gi += eval_ris_emitter(worldPos, normal, u1);
    gi = clamp(gi, float3(0.0, 0.0, 0.0), float3(16.0, 16.0, 16.0));

    float4 outGi = float4(gi, ao);
    if (ubo.counts2.z != 0u)
    {
        float4 prev = prevGiTex.Load(int3(tid.xy, 0));
        if (all(isfinite(prev)))
            outGi = lerp(prev, outGi, 0.20);
    }

    giOutTex[tid.xy] = outGi;
}
