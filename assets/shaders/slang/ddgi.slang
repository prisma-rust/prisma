// RTXGI v1.3 DDGI compute passes (VK_KHR_ray_query + Texture2DArray atlases).
//
// Five compute entry points matching crates/prism-render/src/ddgi.rs:
//   1. ddgiClearMain            - clear full volume or newly-scrolled slices
//   2. ddgiUpdateIrradianceMain - trace rays per probe -> rayBuffer + 6x6 irradiance atlas
//   3. ddgiUpdateDistanceMain   - update 14x14 Chebyshev distance atlas (r, r^2)
//   4. ddgiBlendMain            - octahedral 1-px border copy, relocation, classification, variability
//   5. ddgiResolveMain          - project 6x6 octahedral irradiance atlas -> 9-coeff SH3D probeVolumeOut

#include "path_integrator.slang"

static const uint DDGI_IRR_INTERIOR = 6u;
static const uint DDGI_IRR_TILE     = 8u;
static const uint DDGI_DIST_INTERIOR = 14u;
static const uint DDGI_DIST_TILE     = 16u;

static const uint DDGI_FLAG_RELOCATION     = 1u << 0;
static const uint DDGI_FLAG_CLASSIFICATION = 1u << 1;
static const uint DDGI_FLAG_SCROLLING      = 1u << 2;
static const uint DDGI_FLAG_VARIABILITY    = 1u << 3;
static const uint DDGI_FLAG_CLEAR_ALL      = 1u << 4;

// Order-2 real SH basis constants (must match gi.slang / gi.rs).
static const float DDGI_SH_C0 = 0.282095;
static const float DDGI_SH_C1 = 0.488603;
static const float DDGI_SH_C2 = 1.092548;
static const float DDGI_SH_C3 = 0.315392;
static const float DDGI_SH_C4 = 0.546274;

public struct DdgiVolumeUbo
{
    public float4 origin;          // xyz = snapped world origin, w = max_ray_distance
    public float4 rotation_xyzw;   // probe volume orientation quaternion
    public float4 spacing;         // xyz = per-axis probe spacing, w = hysteresis
    public uint4  counts;          // xyz = probe counts [nx, ny, nz], w = rays_per_probe (multiple of 64)
    public int4   scroll_offsets;  // xyz = toroidal scroll offsets, w = frame_index
    public int4   scroll_delta;    // xyz = signed scroll step this frame, w = flags (DDGI_FLAG_*)
    public float4 ray_rot_xyzw;    // per-frame random rotation quaternion for Fibonacci rays
    public float4 light_dir;       // xyz = normalized direction TO sun, w = normal_bias
    public float4 light_color;     // rgb = sun radiance, w = view_bias
    public float4 thresholds;      // x = irradiance_threshold, y = brightness_threshold, z = num_lights, w = num_emissive
};

[[vk::binding(0, 0)]]
ConstantBuffer<DdgiVolumeUbo> ddgiUbo;

[[vk::binding(1, 0)]]
[format("rgba16f")]
RWTexture2DArray<float4> irradianceAtlas;

[[vk::binding(2, 0)]]
RaytracingAccelerationStructure tlas;

[[vk::binding(3, 0)]]
ByteAddressBuffer vertexData;

[[vk::binding(4, 0)]]
StructuredBuffer<uint> indices;

[[vk::binding(5, 0)]]
[format("rg16f")]
RWTexture2DArray<float2> distanceAtlas;

// b6..b9 are declared in path_integrator.slang (instance_meta, materials, ptLights, ptEmissive).

[[vk::binding(10, 0)]]
[format("rgba16f")]
RWTexture2DArray<float4> probeMetadata; // xyz = relocation offset, w = classification state (>0 active)

[[vk::binding(11, 0)]]
RWStructuredBuffer<float4> rayBuffer; // rgb = radiance, w = signed distance (<0 = backface)

[[vk::binding(12, 0)]]
[format("rgba32f")]
RWTexture3D<float4> probeVolumeOut; // (dims.x, dims.y, dims.z * 9)

[[vk::binding(13, 0)]]
RWStructuredBuffer<uint> variabilityAccum; // [0] = max fixed-point variability

// ---- Helpers --------------------------------------------------------------

float3 rotate_by_quat(float3 v, float4 q)
{
    float3 t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

int3 wrap_probe_coords(int3 logical, int3 counts, int3 scroll)
{
    int3 s = logical + scroll;
    return int3(
        ((s.x % counts.x) + counts.x) % counts.x,
        ((s.y % counts.y) + counts.y) % counts.y,
        ((s.z % counts.z) + counts.z) % counts.z
    );
}

uint probe_linear_index(uint3 logical, uint3 counts)
{
    return logical.x + counts.x * (logical.y + counts.y * logical.z);
}

// Map [-1, 1]^2 octahedral UV to unit 3D direction.
float3 oct_decode(float2 uv)
{
    float3 n = float3(uv.x, uv.y, 1.0 - abs(uv.x) - abs(uv.y));
    if (n.z < 0.0)
    {
        float2 sign_not_zero = float2(n.x >= 0.0 ? 1.0 : -1.0, n.y >= 0.0 ? 1.0 : -1.0);
        n.xy = (1.0 - abs(n.yx)) * sign_not_zero;
    }
    return normalize(n);
}

// Map unit 3D direction to [-1, 1]^2 octahedral UV.
float2 oct_encode(float3 dir)
{
    float l1 = abs(dir.x) + abs(dir.y) + abs(dir.z);
    float2 uv = dir.xy / max(l1, 1e-6);
    if (dir.z < 0.0)
    {
        float2 sign_not_zero = float2(uv.x >= 0.0 ? 1.0 : -1.0, uv.y >= 0.0 ? 1.0 : -1.0);
        uv = (1.0 - abs(uv.yx)) * sign_not_zero;
    }
    return uv;
}

float3 fibonacci_sphere_dir(uint rayIdx, uint rayCount, float4 rayRot)
{
    float y = 1.0 - 2.0 * (float(rayIdx) + 0.5) / float(rayCount);
    float r = sqrt(max(0.0, 1.0 - y * y));
    float phi = float(rayIdx) * 2.39996323; // golden angle
    float3 d = float3(r * cos(phi), y, r * sin(phi));
    return normalize(rotate_by_quat(d, rayRot));
}

float3 probe_world_position(uint3 logical, uint3 storage)
{
    float3 local = float3(logical) * ddgiUbo.spacing.xyz;
    float3 rotated = rotate_by_quat(local, ddgiUbo.rotation_xyzw);
    float3 offset = probeMetadata[uint3(storage.x, storage.z, storage.y)].xyz;
    return ddgiUbo.origin.xyz + rotated + offset;
}

// ---- 1. ddgiClearMain -----------------------------------------------------

[shader("compute")]
[numthreads(4, 4, 1)]
void ddgiClearMain(uint3 tid : SV_DispatchThreadID)
{
    uint3 counts = ddgiUbo.counts.xyz;
    if (tid.x >= counts.x || tid.y >= counts.y || tid.z >= counts.z)
        return;

    bool clearAll = (uint(ddgiUbo.scroll_delta.w) & DDGI_FLAG_CLEAR_ALL) != 0u;
    if (!clearAll)
    {
        // Only clear newly-scrolled boundary slices.
        int3 d = ddgiUbo.scroll_delta.xyz;
        bool onBoundary =
            (d.x > 0 && int(tid.x) >= int(counts.x) - d.x) || (d.x < 0 && int(tid.x) < -d.x) ||
            (d.y > 0 && int(tid.y) >= int(counts.y) - d.y) || (d.y < 0 && int(tid.y) < -d.y) ||
            (d.z > 0 && int(tid.z) >= int(counts.z) - d.z) || (d.z < 0 && int(tid.z) < -d.z);
        if (!onBoundary)
            return;
    }

    int3 storage = wrap_probe_coords(int3(tid), int3(counts), ddgiUbo.scroll_offsets.xyz);
    probeMetadata[uint3(storage.x, storage.z, storage.y)] = float4(0.0, 0.0, 0.0, 1.0);

    uint2 irrBase = uint2(storage.x, storage.z) * DDGI_IRR_TILE;
    for (uint ty = 0u; ty < DDGI_IRR_TILE; ty++)
    {
        for (uint tx = 0u; tx < DDGI_IRR_TILE; tx++)
        {
            irradianceAtlas[uint3(irrBase.x + tx, irrBase.y + ty, storage.y)] = float4(0.0, 0.0, 0.0, 0.0);
        }
    }

    float maxDist = ddgiUbo.origin.w;
    uint2 distBase = uint2(storage.x, storage.z) * DDGI_DIST_TILE;
    for (uint ty = 0u; ty < DDGI_DIST_TILE; ty++)
    {
        for (uint tx = 0u; tx < DDGI_DIST_TILE; tx++)
        {
            distanceAtlas[uint3(distBase.x + tx, distBase.y + ty, storage.y)] = float2(maxDist, maxDist * maxDist);
        }
    }

    if (tid.x == 0u && tid.y == 0u && tid.z == 0u)
    {
        variabilityAccum[0] = 0u;
    }
}

// ---- 2. ddgiUpdateIrradianceMain ------------------------------------------
// Dispatch: (nx * ny * nz, 1, 1) workgroups of 64 threads (1 workgroup per probe).

[shader("compute")]
[numthreads(64, 1, 1)]
void ddgiUpdateIrradianceMain(uint3 gid : SV_GroupID, uint gtid : SV_GroupIndex)
{
    uint3 counts = ddgiUbo.counts.xyz;
    uint totalProbes = counts.x * counts.y * counts.z;
    uint probeIdx = gid.x;
    if (probeIdx >= totalProbes)
        return;

    uint px = probeIdx % counts.x;
    uint py = (probeIdx / counts.x) % counts.y;
    uint pz = probeIdx / (counts.x * counts.y);
    uint3 logical = uint3(px, py, pz);
    int3 storage = wrap_probe_coords(int3(logical), int3(counts), ddgiUbo.scroll_offsets.xyz);

    float state = probeMetadata[uint3(storage.x, storage.z, storage.y)].w;
    if ((uint(ddgiUbo.scroll_delta.w) & DDGI_FLAG_CLASSIFICATION) != 0u && state <= 0.0)
        return;

    float3 worldPos = probe_world_position(logical, uint3(storage));
    uint raysPerProbe = max(ddgiUbo.counts.w, 64u);
    float maxDist = ddgiUbo.origin.w;
    uint numLights = uint(ddgiUbo.thresholds.z);
    uint numEmissive = uint(ddgiUbo.thresholds.w);
    uint frameIdx = uint(ddgiUbo.scroll_offsets.w);

    for (uint r = gtid; r < raysPerProbe; r += 64u)
    {
        float3 dir = fibonacci_sphere_dir(r, raysPerProbe, ddgiUbo.ray_rot_xyzw);
        uint seed = probeIdx * 73856093u + r * 19349663u + frameIdx * 83492791u;

        RayDesc ray;
        ray.Origin = worldPos;
        ray.Direction = dir;
        ray.TMin = 0.01;
        ray.TMax = maxDist;

        RayQuery<RAY_FLAG_FORCE_OPAQUE> q;
        q.TraceRayInline(tlas, RAY_FLAG_FORCE_OPAQUE, 0xFF, ray);
        while (q.Proceed()) { }

        float hitDist = maxDist;
        float3 rad = float3(0.0, 0.0, 0.0);

        if (q.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
        {
            hitDist = q.CommittedRayT();
            uint instId = q.CommittedInstanceID();
            uint primIdx = q.CommittedPrimitiveIndex();
            float3 v0, v1, v2;
            float2 uv0, uv1, uv2;
            float3 n0, n1, n2;
            read_hit_triangle(instId, primIdx, vertexData, indices,
                v0, v1, v2, uv0, uv1, uv2, n0, n1, n2);
            float3 geomN = normalize(cross(v1 - v0, v2 - v0));
            bool backface = dot(geomN, dir) > 0.0;
            if (backface)
            {
                // Encode back-face hit as negative distance for relocation & classification.
                hitDist = -hitDist;
            }
            else
            {
                rad = TraceRadiance(
                    worldPos, dir, tlas, vertexData, indices,
                    ddgiUbo.light_dir.xyz, ddgiUbo.light_color.rgb,
                    numLights, numEmissive, 2u, maxDist, seed
                );
            }
        }
        else
        {
            rad = TraceRadiance(
                worldPos, dir, tlas, vertexData, indices,
                ddgiUbo.light_dir.xyz, ddgiUbo.light_color.rgb,
                numLights, numEmissive, 1u, maxDist, seed
            );
        }

        rayBuffer[probeIdx * raysPerProbe + r] = float4(rad, hitDist);
    }
}

// ---- 3. ddgiUpdateDistanceMain --------------------------------------------
// Updates both 6x6 irradiance interior and 14x14 distance interior from rayBuffer.
// Dispatch: (nx * DDGI_DIST_INTERIOR / 8, nz * DDGI_DIST_INTERIOR / 8, ny)

[shader("compute")]
[numthreads(8, 8, 1)]
void ddgiUpdateDistanceMain(uint3 tid : SV_DispatchThreadID)
{
    uint3 counts = ddgiUbo.counts.xyz;
    uint totalW = counts.x * DDGI_DIST_INTERIOR;
    uint totalH = counts.z * DDGI_DIST_INTERIOR;
    if (tid.x >= totalW || tid.y >= totalH || tid.z >= counts.y)
        return;

    uint px = tid.x / DDGI_DIST_INTERIOR;
    uint pz = tid.y / DDGI_DIST_INTERIOR;
    uint py = tid.z;
    uint tx = tid.x % DDGI_DIST_INTERIOR;
    uint ty = tid.y % DDGI_DIST_INTERIOR;

    int3 storage = wrap_probe_coords(int3(px, py, pz), int3(counts), ddgiUbo.scroll_offsets.xyz);
    uint probeIdx = probe_linear_index(uint3(px, py, pz), counts);
    uint raysPerProbe = max(ddgiUbo.counts.w, 64u);
    float hysteresis = clamp(ddgiUbo.spacing.w, 0.0, 0.995);
    float maxDist = ddgiUbo.origin.w;

    // Distance texel direction.
    float2 distOctUv = (float2(tx, ty) + 0.5) / float(DDGI_DIST_INTERIOR) * 2.0 - 1.0;
    float3 distDir = oct_decode(distOctUv);

    float distSum = 0.0;
    float distSqSum = 0.0;
    float distW = 0.0;

    for (uint r = 0u; r < raysPerProbe; r++)
    {
        float4 rayData = rayBuffer[probeIdx * raysPerProbe + r];
        float3 rayDir = fibonacci_sphere_dir(r, raysPerProbe, ddgiUbo.ray_rot_xyzw);
        float w = pow(max(dot(distDir, rayDir), 0.0), 50.0);
        if (w > 1e-5)
        {
            float d = min(abs(rayData.w), maxDist);
            distSum += d * w;
            distSqSum += (d * d) * w;
            distW += w;
        }
    }

    uint2 distCoord = uint2(storage.x, storage.z) * DDGI_DIST_TILE + uint2(tx + 1u, ty + 1u);
    if (distW > 1e-5)
    {
        float2 newDist = float2(distSum / distW, distSqSum / distW);
        float2 prevDist = distanceAtlas[uint3(distCoord, storage.y)];
        distanceAtlas[uint3(distCoord, storage.y)] = lerp(newDist, prevDist, hysteresis);
    }

    // Also update 6x6 irradiance interior for the first 6x6 threads of each probe tile.
    if (tx < DDGI_IRR_INTERIOR && ty < DDGI_IRR_INTERIOR)
    {
        float2 irrOctUv = (float2(tx, ty) + 0.5) / float(DDGI_IRR_INTERIOR) * 2.0 - 1.0;
        float3 irrDir = oct_decode(irrOctUv);

        float3 irrSum = float3(0.0, 0.0, 0.0);
        float irrW = 0.0;
        for (uint r = 0u; r < raysPerProbe; r++)
        {
            float4 rayData = rayBuffer[probeIdx * raysPerProbe + r];
            if (rayData.w < 0.0)
                continue; // skip back-face hits
            float3 rayDir = fibonacci_sphere_dir(r, raysPerProbe, ddgiUbo.ray_rot_xyzw);
            float w = max(dot(irrDir, rayDir), 0.0);
            if (w > 1e-5)
            {
                irrSum += rayData.rgb * w;
                irrW += w;
            }
        }

        uint2 irrCoord = uint2(storage.x, storage.z) * DDGI_IRR_TILE + uint2(tx + 1u, ty + 1u);
        float4 prevIrr = irradianceAtlas[uint3(irrCoord, storage.y)];
        if (irrW > 1e-5)
        {
            float3 newIrr = irrSum / irrW;
            float diff = max(abs(newIrr.r - prevIrr.r), max(abs(newIrr.g - prevIrr.g), abs(newIrr.b - prevIrr.b)));
            float effHyst = (diff > ddgiUbo.thresholds.x) ? hysteresis * 0.75 : hysteresis;
            float3 blended = lerp(newIrr, prevIrr.rgb, effHyst);
            irradianceAtlas[uint3(irrCoord, storage.y)] = float4(blended, 1.0);

            if ((uint(ddgiUbo.scroll_delta.w) & DDGI_FLAG_VARIABILITY) != 0u)
            {
                uint fixedVar = uint(clamp(diff * 65535.0, 0.0, 4294967295.0));
                InterlockedMax(variabilityAccum[0], fixedVar);
            }
        }
    }
}

// ---- 4. ddgiBlendMain -----------------------------------------------------
// Copies 1-px octahedral borders and updates probe relocation + classification.
// Dispatch: (nx, nz, ny) with numthreads(1, 1, 1) -> 1 thread per probe.

[shader("compute")]
[numthreads(1, 1, 1)]
void ddgiBlendMain(uint3 tid : SV_DispatchThreadID)
{
    uint3 counts = ddgiUbo.counts.xyz;
    if (tid.x >= counts.x || tid.y >= counts.z || tid.z >= counts.y)
        return;

    uint3 logical = uint3(tid.x, tid.z, tid.y);
    int3 storage = wrap_probe_coords(int3(logical), int3(counts), ddgiUbo.scroll_offsets.xyz);
    uint probeIdx = probe_linear_index(logical, counts);
    uint flags = uint(ddgiUbo.scroll_delta.w);

    // 1-px octahedral border copy for 8x8 irradiance tile.
    uint2 irrBase = uint2(storage.x, storage.z) * DDGI_IRR_TILE;
    for (uint i = 0u; i < DDGI_IRR_INTERIOR; i++)
    {
        uint mirror = DDGI_IRR_INTERIOR - 1u - i;
        irradianceAtlas[uint3(irrBase.x + 1u + i, irrBase.y, storage.y)] =
            irradianceAtlas[uint3(irrBase.x + 1u + mirror, irrBase.y + 1u, storage.y)];
        irradianceAtlas[uint3(irrBase.x + 1u + i, irrBase.y + DDGI_IRR_TILE - 1u, storage.y)] =
            irradianceAtlas[uint3(irrBase.x + 1u + mirror, irrBase.y + DDGI_IRR_INTERIOR, storage.y)];
        irradianceAtlas[uint3(irrBase.x, irrBase.y + 1u + i, storage.y)] =
            irradianceAtlas[uint3(irrBase.x + 1u, irrBase.y + 1u + mirror, storage.y)];
        irradianceAtlas[uint3(irrBase.x + DDGI_IRR_TILE - 1u, irrBase.y + 1u + i, storage.y)] =
            irradianceAtlas[uint3(irrBase.x + DDGI_IRR_INTERIOR, irrBase.y + 1u + mirror, storage.y)];
    }
    // Corners.
    irradianceAtlas[uint3(irrBase.x, irrBase.y, storage.y)] =
        irradianceAtlas[uint3(irrBase.x + DDGI_IRR_INTERIOR, irrBase.y + DDGI_IRR_INTERIOR, storage.y)];
    irradianceAtlas[uint3(irrBase.x + DDGI_IRR_TILE - 1u, irrBase.y, storage.y)] =
        irradianceAtlas[uint3(irrBase.x + 1u, irrBase.y + DDGI_IRR_INTERIOR, storage.y)];
    irradianceAtlas[uint3(irrBase.x, irrBase.y + DDGI_IRR_TILE - 1u, storage.y)] =
        irradianceAtlas[uint3(irrBase.x + DDGI_IRR_INTERIOR, irrBase.y + 1u, storage.y)];
    irradianceAtlas[uint3(irrBase.x + DDGI_IRR_TILE - 1u, irrBase.y + DDGI_IRR_TILE - 1u, storage.y)] =
        irradianceAtlas[uint3(irrBase.x + 1u, irrBase.y + 1u, storage.y)];

    // 1-px octahedral border copy for 16x16 distance tile.
    uint2 distBase = uint2(storage.x, storage.z) * DDGI_DIST_TILE;
    for (uint i = 0u; i < DDGI_DIST_INTERIOR; i++)
    {
        uint mirror = DDGI_DIST_INTERIOR - 1u - i;
        distanceAtlas[uint3(distBase.x + 1u + i, distBase.y, storage.y)] =
            distanceAtlas[uint3(distBase.x + 1u + mirror, distBase.y + 1u, storage.y)];
        distanceAtlas[uint3(distBase.x + 1u + i, distBase.y + DDGI_DIST_TILE - 1u, storage.y)] =
            distanceAtlas[uint3(distBase.x + 1u + mirror, distBase.y + DDGI_DIST_INTERIOR, storage.y)];
        distanceAtlas[uint3(distBase.x, distBase.y + 1u + i, storage.y)] =
            distanceAtlas[uint3(distBase.x + 1u, distBase.y + 1u + mirror, storage.y)];
        distanceAtlas[uint3(distBase.x + DDGI_DIST_TILE - 1u, distBase.y + 1u + i, storage.y)] =
            distanceAtlas[uint3(distBase.x + DDGI_DIST_INTERIOR, distBase.y + 1u + mirror, storage.y)];
    }

    // Relocation + classification from rayBuffer.
    if ((flags & (DDGI_FLAG_RELOCATION | DDGI_FLAG_CLASSIFICATION)) != 0u)
    {
        uint raysPerProbe = max(ddgiUbo.counts.w, 64u);
        uint backfaceCount = 0u;
        float closestBackDist = 1e6;
        float3 closestBackDir = float3(0.0, 0.0, 0.0);
        float closestFrontDist = 1e6;
        float3 farthestFrontDir = float3(0.0, 0.0, 0.0);
        float farthestFrontDist = 0.0;

        for (uint r = 0u; r < raysPerProbe; r++)
        {
            float d = rayBuffer[probeIdx * raysPerProbe + r].w;
            float3 dir = fibonacci_sphere_dir(r, raysPerProbe, ddgiUbo.ray_rot_xyzw);
            if (d < 0.0)
            {
                backfaceCount++;
                float ad = -d;
                if (ad < closestBackDist)
                {
                    closestBackDist = ad;
                    closestBackDir = dir;
                }
            }
            else
            {
                if (d < closestFrontDist)
                    closestFrontDist = d;
                if (d > farthestFrontDist)
                {
                    farthestFrontDist = d;
                    farthestFrontDir = dir;
                }
            }
        }

        float4 meta = probeMetadata[uint3(storage.x, storage.z, storage.y)];
        if ((flags & DDGI_FLAG_RELOCATION) != 0u)
        {
            float3 fullOffset = meta.xyz;
            float minFront = 0.25 * min(ddgiUbo.spacing.x, min(ddgiUbo.spacing.y, ddgiUbo.spacing.z));
            if (float(backfaceCount) / float(raysPerProbe) > 0.25)
            {
                fullOffset += closestBackDir * (closestBackDist + minFront * 0.5);
            }
            else if (closestFrontDist < minFront)
            {
                fullOffset += farthestFrontDir * min(farthestFrontDist - minFront, minFront * 0.25);
            }
            float3 maxOffset = ddgiUbo.spacing.xyz * 0.45;
            meta.xyz = clamp(fullOffset, -maxOffset, maxOffset);
        }

        if ((flags & DDGI_FLAG_CLASSIFICATION) != 0u)
        {
            meta.w = (float(backfaceCount) / float(raysPerProbe) > 0.5) ? -1.0 : 1.0;
        }
        probeMetadata[uint3(storage.x, storage.z, storage.y)] = meta;
    }
}

// ---- 5. ddgiResolveMain ---------------------------------------------------
// Projects 6x6 octahedral irradiance atlas -> 9 order-2 SH coefficients in
// RWTexture3D<float4> probeVolumeOut (dims.x, dims.y, dims.z * 9) for scene_frag.slang.

[shader("compute")]
[numthreads(4, 4, 1)]
void ddgiResolveMain(uint3 tid : SV_DispatchThreadID)
{
    uint3 counts = ddgiUbo.counts.xyz;
    if (tid.x >= counts.x || tid.y >= counts.y || tid.z >= counts.z)
        return;

    int3 storage = wrap_probe_coords(int3(tid), int3(counts), ddgiUbo.scroll_offsets.xyz);
    uint2 irrBase = uint2(storage.x, storage.z) * DDGI_IRR_TILE;

    float3 sh[9];
    for (uint c = 0u; c < 9u; c++)
        sh[c] = float3(0.0, 0.0, 0.0);

    float dOmega = (4.0 * 3.14159265) / float(DDGI_IRR_INTERIOR * DDGI_IRR_INTERIOR);
    for (uint ty = 0u; ty < DDGI_IRR_INTERIOR; ty++)
    {
        for (uint tx = 0u; tx < DDGI_IRR_INTERIOR; tx++)
        {
            float2 octUv = (float2(tx, ty) + 0.5) / float(DDGI_IRR_INTERIOR) * 2.0 - 1.0;
            float3 n = oct_decode(octUv);
            float3 irr = irradianceAtlas[uint3(irrBase.x + 1u + tx, irrBase.y + 1u + ty, storage.y)].rgb;

            float w = dOmega;
            sh[0] += irr * (DDGI_SH_C0 * w);
            sh[1] += irr * (DDGI_SH_C1 * n.y * w);
            sh[2] += irr * (DDGI_SH_C1 * n.z * w);
            sh[3] += irr * (DDGI_SH_C1 * n.x * w);
            sh[4] += irr * (DDGI_SH_C2 * n.x * n.y * w);
            sh[5] += irr * (DDGI_SH_C2 * n.y * n.z * w);
            sh[6] += irr * (DDGI_SH_C3 * (3.0 * n.z * n.z - 1.0) * w);
            sh[7] += irr * (DDGI_SH_C2 * n.x * n.z * w);
            sh[8] += irr * (DDGI_SH_C4 * (n.x * n.x - n.y * n.y) * w);
        }
    }

    for (uint c = 0u; c < 9u; c++)
    {
        probeVolumeOut[uint3(tid.x, tid.y, c * counts.z + tid.z)] = float4(sh[c], 0.0);
    }
}
