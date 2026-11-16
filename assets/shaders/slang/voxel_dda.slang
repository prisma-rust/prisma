// Three-tier compute DDA raymarcher (L1 1.0m COW -> L2 0.25m bricks -> L3 0.0625m micro-voxels).
//
// Set 0 bindings:
//   0 - VoxelFrameUbo (matrices, camera, sun, cursor highlight box)
//   1 - StructuredBuffer<VoxelChunkGpu> (chunk headers)
//   2 - StructuredBuffer<uint4> (L1 cells: .x=brickRef, .y=uniformRgba, .z=uniformVisual)
//   3 - StructuredBuffer<uint2> (L2 brick occupancy masks: .x=occLo, .y=occHi)
//   4 - StructuredBuffer<uint4> (L2 cell payload: .x=rgba, .y=visual, .z=l3Lo, .w=l3Hi)
//   5 - Texture2D<float4> (ForwardPass HDR color input)
//   6 - Texture2D<float> (ForwardPass D32 depth input)
//   7 - RWTexture2D<float4> (VOXEL_COLOR_H composited HDR output)
//   8 - RWTexture2D<float> (VOXEL_DEPTH_H composited depth output)

static const uint L1_UNIFORM_REF = 0xFFFFFFFFu;
static const float L1_SIZE_M = 1.0;
static const float L2_SIZE_M = 0.25;
static const float L3_SIZE_M = 0.0625;

static const uint VIS_WET         = 1u << 0u;
static const uint VIS_BURNING     = 1u << 1u;
static const uint VIS_FROZEN      = 1u << 2u;
static const uint VIS_CHARGED     = 1u << 3u;
static const uint VIS_CORRODED    = 1u << 4u;
static const uint VIS_INFECTED    = 1u << 5u;
static const uint VIS_WARNING     = 1u << 6u;
static const uint VIS_STASIS      = 1u << 7u;
static const uint VIS_BROKEN      = 1u << 8u;
static const uint VIS_LIQUID      = 1u << 9u;
static const uint VIS_TRANSLUCENT = 1u << 10u;
static const uint VIS_EMISSIVE    = 1u << 11u;
static const uint VIS_METALLIC    = 1u << 12u;
static const uint VIS_HEAT_GLOW   = 1u << 13u;

public struct VoxelFrameUbo
{
    public float4x4 viewProj;
    public float4x4 invViewProj;
    public float4   cameraPos;       // xyz = eye position, w = elapsed time (seconds)
    public float4   sunDirRadiance;  // xyz = direction TO sun, w = sun radiance
    public float4   sunColorAmbient; // rgb = sun linear color, w = ambient factor
    public uint4    counts;          // x = chunkCount, y = widthPx, z = heightPx, w = cursorActive
    public float4   cursorMin;       // xyz = cursor box min, w = reserved
    public float4   cursorMax;       // xyz = cursor box max, w = exposure
    public float4   cursorColor;     // rgba = wireframe tint
};

public struct VoxelChunkGpu
{
    public int4  originAndL1Offset;  // xyz = origin_l1, w = base L1 cell index
    public uint4 dimsAndBrickOffset; // xyz = dims_l1, w = base brick index
};

[[vk::binding(0, 0)]] ConstantBuffer<VoxelFrameUbo>   ubo;
[[vk::binding(1, 0)]] StructuredBuffer<VoxelChunkGpu> chunks;
[[vk::binding(2, 0)]] StructuredBuffer<uint4>         l1Cells;
[[vk::binding(3, 0)]] StructuredBuffer<uint2>         brickOcc;
[[vk::binding(4, 0)]] StructuredBuffer<uint4>         brickCells;
[[vk::binding(5, 0)]] Texture2D<float4>               forwardColorTex;
[[vk::binding(6, 0)]] Texture2D<float>                forwardDepthTex;
[[vk::binding(7, 0)]] [[vk::image_format("rgba16f")]] RWTexture2D<float4> outColorTex;
[[vk::binding(8, 0)]] [[vk::image_format("r32f")]]    RWTexture2D<float>  outDepthTex;

struct TraceHit
{
    bool   hit;
    float  t;
    float3 worldPos;
    float3 normal;
    uint   rgba;
    uint   visual;
    uint   tier; // 1 = L1, 2 = L2, 3 = L3
    bool   hasGlass;
    float  glassT;
    float3 glassNormal;
    uint   glassRgba;
    uint   glassVisual;
};

bool bit64_test(uint lo, uint hi, uint idx)
{
    uint word = (idx < 32u) ? lo : hi;
    return ((word >> (idx & 31u)) & 1u) != 0u;
}

float4 unpack_rgba_linear(uint packed)
{
    float4 srgb = float4(
        float(packed & 0xFFu),
        float((packed >> 8u) & 0xFFu),
        float((packed >> 16u) & 0xFFu),
        float((packed >> 24u) & 0xFFu)
    ) * (1.0 / 255.0);
    return float4(pow(max(srgb.rgb, float3(0.0, 0.0, 0.0)), float3(2.2, 2.2, 2.2)), srgb.a);
}

bool intersect_aabb(
    float3 origin,
    float3 invDir,
    float3 bmin,
    float3 bmax,
    float  tLo,
    float  tHi,
    out float tEnter,
    out float tExit,
    out float3 enterNormal)
{
    float3 t0s = (bmin - origin) * invDir;
    float3 t1s = (bmax - origin) * invDir;
    float3 tSmall = min(t0s, t1s);
    float3 tLarge = max(t0s, t1s);
    tEnter = max(tLo, max(tSmall.x, max(tSmall.y, tSmall.z)));
    tExit  = min(tHi, min(tLarge.x, min(tLarge.y, tLarge.z)));

    enterNormal = float3(0.0, 1.0, 0.0);
    if (tSmall.x >= tSmall.y && tSmall.x >= tSmall.z)
        enterNormal = float3(invDir.x >= 0.0 ? -1.0 : 1.0, 0.0, 0.0);
    else if (tSmall.y >= tSmall.z)
        enterNormal = float3(0.0, invDir.y >= 0.0 ? -1.0 : 1.0, 0.0);
    else
        enterNormal = float3(0.0, 0.0, invDir.z >= 0.0 ? -1.0 : 1.0);

    return tEnter < tExit;
}

float next_t(float rel, float d, int cell)
{
    if (d > 1e-7)
        return (float(cell + 1) - rel) / d;
    if (d < -1e-7)
        return (float(cell) - rel) / d;
    return 1e30;
}

void record_translucent(
    inout TraceHit res,
    float tHit,
    float3 nHit,
    uint rgba,
    uint visual)
{
    if (!res.hasGlass || tHit < res.glassT)
    {
        res.hasGlass = true;
        res.glassT = tHit;
        res.glassNormal = nHit;
        res.glassRgba = rgba;
        res.glassVisual = visual;
    }
}

bool trace_l3(
    float3 origin,
    float3 dir,
    float3 relOrigin,
    int3   l2Local,
    uint   l3Lo,
    uint   l3Hi,
    uint   rgba,
    uint   visual,
    float  tEnter,
    float  tExit,
    float3 startNormal,
    bool   stopOnTranslucent,
    inout TraceHit res)
{
    float scale = 16.0;
    float eps = 1e-4;
    float3 base = relOrigin * scale - float3(l2Local * 4);
    float3 pStart = base + dir * scale * (tEnter + eps);
    int3 c = clamp(int3(floor(pStart)), int3(0, 0, 0), int3(3, 3, 3));
    int3 stepDir = int3(dir.x >= 0.0 ? 1 : -1, dir.y >= 0.0 ? 1 : -1, dir.z >= 0.0 ? 1 : -1);
    float3 tDelta = float3(
        abs(dir.x) > 1e-7 ? L3_SIZE_M / abs(dir.x) : 1e30,
        abs(dir.y) > 1e-7 ? L3_SIZE_M / abs(dir.y) : 1e30,
        abs(dir.z) > 1e-7 ? L3_SIZE_M / abs(dir.z) : 1e30
    );
    float3 tNext = float3(
        next_t(base.x, dir.x * scale, c.x),
        next_t(base.y, dir.y * scale, c.y),
        next_t(base.z, dir.z * scale, c.z)
    );
    float tCur = tEnter;
    float3 n = startNormal;

    for (uint iter = 0u; iter < 16u; iter++)
    {
        if (any(c < int3(0, 0, 0)) || any(c > int3(3, 3, 3)))
            break;

        uint bitIdx = uint(c.x) + 4u * (uint(c.y) + 4u * uint(c.z));
        if (bit64_test(l3Lo, l3Hi, bitIdx))
        {
            bool isTrans = ((visual & (VIS_LIQUID | VIS_TRANSLUCENT)) != 0u) && ((rgba >> 24u) < 250u);
            if (!isTrans || stopOnTranslucent)
            {
                res.hit = true;
                res.t = tCur;
                res.worldPos = origin + dir * tCur;
                res.normal = n;
                res.rgba = rgba;
                res.visual = visual;
                res.tier = 3u;
                return true;
            }
            record_translucent(res, tCur, n, rgba, visual);
        }

        float tStepExit = min(tExit, min(tNext.x, min(tNext.y, tNext.z)));
        if (tStepExit >= tExit)
            break;

        if (tNext.x <= tNext.y && tNext.x <= tNext.z)
        {
            tCur = tNext.x;
            tNext.x += tDelta.x;
            c.x += stepDir.x;
            n = float3(-float(stepDir.x), 0.0, 0.0);
        }
        else if (tNext.y <= tNext.z)
        {
            tCur = tNext.y;
            tNext.y += tDelta.y;
            c.y += stepDir.y;
            n = float3(0.0, -float(stepDir.y), 0.0);
        }
        else
        {
            tCur = tNext.z;
            tNext.z += tDelta.z;
            c.z += stepDir.z;
            n = float3(0.0, 0.0, -float(stepDir.z));
        }
        if (tCur >= tExit)
            break;
    }
    return false;
}

bool trace_brick(
    float3 origin,
    float3 dir,
    float3 relOrigin,
    int3   l1Cell,
    uint   brickIdx,
    float  tEnter,
    float  tExit,
    float3 startNormal,
    bool   stopOnTranslucent,
    bool   ignoreTranslucent,
    inout TraceHit res)
{
    uint2 occ = brickOcc[brickIdx];
    if ((occ.x | occ.y) == 0u)
        return false;

    float scale = 4.0;
    float eps = 1e-4;
    float3 base = (relOrigin - float3(l1Cell)) * scale;
    float3 pStart = base + dir * scale * (tEnter + eps);
    int3 c = clamp(int3(floor(pStart)), int3(0, 0, 0), int3(3, 3, 3));
    int3 stepDir = int3(dir.x >= 0.0 ? 1 : -1, dir.y >= 0.0 ? 1 : -1, dir.z >= 0.0 ? 1 : -1);
    float3 tDelta = float3(
        abs(dir.x) > 1e-7 ? L2_SIZE_M / abs(dir.x) : 1e30,
        abs(dir.y) > 1e-7 ? L2_SIZE_M / abs(dir.y) : 1e30,
        abs(dir.z) > 1e-7 ? L2_SIZE_M / abs(dir.z) : 1e30
    );
    float3 tNext = float3(
        next_t(base.x, dir.x * scale, c.x),
        next_t(base.y, dir.y * scale, c.y),
        next_t(base.z, dir.z * scale, c.z)
    );
    float tCur = tEnter;
    float3 n = startNormal;

    for (uint iter = 0u; iter < 16u; iter++)
    {
        if (any(c < int3(0, 0, 0)) || any(c > int3(3, 3, 3)))
            break;

        float tStepExit = min(tExit, min(tNext.x, min(tNext.y, tNext.z)));
        uint sub = uint(c.x) + 4u * (uint(c.y) + 4u * uint(c.z));
        if (bit64_test(occ.x, occ.y, sub))
        {
            uint4 cell = brickCells[brickIdx * 64u + sub];
            uint rgba = cell.x;
            uint visual = cell.y;
            uint l3Lo = cell.z;
            uint l3Hi = cell.w;
            if ((rgba >> 24u) != 0u && ((l3Lo | l3Hi) != 0u))
            {
                bool isTrans = ((visual & (VIS_LIQUID | VIS_TRANSLUCENT)) != 0u) && ((rgba >> 24u) < 250u);
                if (!ignoreTranslucent || !isTrans)
                {
                    if (l3Lo == 0xFFFFFFFFu && l3Hi == 0xFFFFFFFFu)
                    {
                        if (!isTrans || stopOnTranslucent)
                        {
                            res.hit = true;
                            res.t = tCur;
                            res.worldPos = origin + dir * tCur;
                            res.normal = n;
                            res.rgba = rgba;
                            res.visual = visual;
                            res.tier = 2u;
                            return true;
                        }
                        record_translucent(res, tCur, n, rgba, visual);
                    }
                    else
                    {
                        int3 l2Local = l1Cell * 4 + c;
                        if (trace_l3(
                            origin, dir, relOrigin, l2Local, l3Lo, l3Hi,
                            rgba, visual, tCur, tStepExit, n, stopOnTranslucent, res))
                        {
                            return true;
                        }
                    }
                }
            }
        }

        if (tStepExit >= tExit)
            break;

        if (tNext.x <= tNext.y && tNext.x <= tNext.z)
        {
            tCur = tNext.x;
            tNext.x += tDelta.x;
            c.x += stepDir.x;
            n = float3(-float(stepDir.x), 0.0, 0.0);
        }
        else if (tNext.y <= tNext.z)
        {
            tCur = tNext.y;
            tNext.y += tDelta.y;
            c.y += stepDir.y;
            n = float3(0.0, -float(stepDir.y), 0.0);
        }
        else
        {
            tCur = tNext.z;
            tNext.z += tDelta.z;
            c.z += stepDir.z;
            n = float3(0.0, 0.0, -float(stepDir.z));
        }
        if (tCur >= tExit)
            break;
    }
    return false;
}

void trace_chunk(
    VoxelChunkGpu chunk,
    float3 origin,
    float3 dir,
    float3 invDir,
    float  maxT,
    bool   shadowRay,
    inout TraceHit best)
{
    float3 bmin = float3(chunk.originAndL1Offset.xyz) * L1_SIZE_M;
    float3 bmax = bmin + float3(chunk.dimsAndBrickOffset.xyz) * L1_SIZE_M;

    float tEnter, tExit;
    float3 enterNormal;
    if (!intersect_aabb(origin, invDir, bmin, bmax, 0.0, min(maxT, best.t), tEnter, tExit, enterNormal))
        return;
    tEnter = max(tEnter, 0.0);

    float3 relOrigin = origin - bmin;
    float3 pStart = relOrigin + dir * (tEnter + 1e-4);
    int3 dims = int3(chunk.dimsAndBrickOffset.xyz);
    int3 cell = clamp(int3(floor(pStart)), int3(0, 0, 0), dims - int3(1, 1, 1));
    int3 stepDir = int3(dir.x >= 0.0 ? 1 : -1, dir.y >= 0.0 ? 1 : -1, dir.z >= 0.0 ? 1 : -1);
    float3 tDelta = float3(
        abs(dir.x) > 1e-7 ? 1.0 / abs(dir.x) : 1e30,
        abs(dir.y) > 1e-7 ? 1.0 / abs(dir.y) : 1e30,
        abs(dir.z) > 1e-7 ? 1.0 / abs(dir.z) : 1e30
    );
    float3 tNext = float3(
        next_t(relOrigin.x, dir.x, cell.x),
        next_t(relOrigin.y, dir.y, cell.y),
        next_t(relOrigin.z, dir.z, cell.z)
    );
    float tCur = tEnter;
    float3 n = enterNormal;
    uint l1Base = uint(chunk.originAndL1Offset.w);
    uint brickBase = chunk.dimsAndBrickOffset.w;

    uint maxSteps = uint(dims.x + dims.y + dims.z + 8);
    for (uint iter = 0u; iter < maxSteps; iter++)
    {
        if (any(cell < int3(0, 0, 0)) || any(cell >= dims))
            break;
        if (tCur >= best.t)
            break;

        float tStepExit = min(tExit, min(tNext.x, min(tNext.y, tNext.z)));
        uint l1Idx = l1Base + uint(cell.x) + chunk.dimsAndBrickOffset.x * (uint(cell.y) + chunk.dimsAndBrickOffset.y * uint(cell.z));
        uint4 l1Val = l1Cells[l1Idx];
        uint l1Ref = l1Val.x;

        if (l1Ref == L1_UNIFORM_REF)
        {
            uint rgba = l1Val.y;
            uint vis = l1Val.z;
            if ((rgba >> 24u) != 0u)
            {
                bool isTrans = ((vis & (VIS_LIQUID | VIS_TRANSLUCENT)) != 0u) && ((rgba >> 24u) < 250u);
                if (!shadowRay || !isTrans)
                {
                    if (!isTrans)
                    {
                        best.hit = true;
                        best.t = tCur;
                        best.worldPos = origin + dir * tCur;
                        best.normal = n;
                        best.rgba = rgba;
                        best.visual = vis;
                        best.tier = 1u;
                        return;
                    }
                    record_translucent(best, tCur, n, rgba, vis);
                }
            }
        }
        else if (l1Ref != 0u)
        {
            TraceHit candidate = best;
            candidate.hit = false;
            candidate.hasGlass = false;
            if (trace_brick(
                origin, dir, relOrigin, cell, brickBase + (l1Ref - 1u),
                tCur, tStepExit, n, false, shadowRay, candidate))
            {
                best.hit = true;
                best.t = candidate.t;
                best.worldPos = candidate.worldPos;
                best.normal = candidate.normal;
                best.rgba = candidate.rgba;
                best.visual = candidate.visual;
                best.tier = candidate.tier;
                if (candidate.hasGlass)
                    record_translucent(best, candidate.glassT, candidate.glassNormal, candidate.glassRgba, candidate.glassVisual);
                return;
            }
            if (candidate.hasGlass)
                record_translucent(best, candidate.glassT, candidate.glassNormal, candidate.glassRgba, candidate.glassVisual);
        }

        if (tStepExit >= tExit)
            break;

        if (tNext.x <= tNext.y && tNext.x <= tNext.z)
        {
            tCur = tNext.x;
            tNext.x += tDelta.x;
            cell.x += stepDir.x;
            n = float3(-float(stepDir.x), 0.0, 0.0);
        }
        else if (tNext.y <= tNext.z)
        {
            tCur = tNext.y;
            tNext.y += tDelta.y;
            cell.y += stepDir.y;
            n = float3(0.0, -float(stepDir.y), 0.0);
        }
        else
        {
            tCur = tNext.z;
            tNext.z += tDelta.z;
            cell.z += stepDir.z;
            n = float3(0.0, 0.0, -float(stepDir.z));
        }
        if (tCur >= tExit)
            break;
    }
}

TraceHit trace_world(float3 origin, float3 dir, float maxT, bool shadowRay)
{
    TraceHit best;
    best.hit = false;
    best.t = maxT;
    best.worldPos = float3(0.0, 0.0, 0.0);
    best.normal = float3(0.0, 1.0, 0.0);
    best.rgba = 0u;
    best.visual = 0u;
    best.tier = 0u;
    best.hasGlass = false;
    best.glassT = maxT;
    best.glassNormal = float3(0.0, 1.0, 0.0);
    best.glassRgba = 0u;
    best.glassVisual = 0u;

    float3 invDir = float3(
        abs(dir.x) > 1e-7 ? 1.0 / dir.x : 1e30,
        abs(dir.y) > 1e-7 ? 1.0 / dir.y : 1e30,
        abs(dir.z) > 1e-7 ? 1.0 / dir.z : 1e30
    );

    uint count = ubo.counts.x;
    for (uint i = 0u; i < count; i++)
    {
        trace_chunk(chunks[i], origin, dir, invDir, maxT, shadowRay, best);
        if (shadowRay && best.hit)
            return best;
    }
    return best;
}

float hash31(float3 p)
{
    p = frac(p * 0.1031);
    p += dot(p, p.yzx + 33.33);
    return frac((p.x + p.y) * p.z);
}

float3 shade_voxel(float3 worldPos, float3 normal, uint rgba, uint visual, uint tier, float3 viewDir, float shadowVis)
{
    float4 baseCol = unpack_rgba_linear(rgba);
    float3 albedo = baseCol.rgb;
    float roughness = float((visual >> 16u) & 0xFFu) * (1.0 / 255.0);
    float emissiveAmt = float((visual >> 24u) & 0xFFu) * (1.0 / 255.0);
    float time = ubo.cameraPos.w;

    // Fine spatial variation + crisp tier border bevel so 1m/0.25m/0.0625m detail reads clearly.
    float cellStep = (tier == 3u) ? L3_SIZE_M : ((tier == 2u) ? L2_SIZE_M : L2_SIZE_M);
    float3 gridP = worldPos / cellStep;
    float3 f = abs(frac(gridP) - 0.5);
    float3 maskN = 1.0 - abs(normal);
    float edgeMax = max(f.x * maskN.x, max(f.y * maskN.y, f.z * maskN.z));
    float bevel = smoothstep(0.42, 0.50, edgeMax);
    albedo *= lerp(1.0, (tier == 3u) ? 0.80 : 0.88, bevel);

    float microNoise = hash31(floor(worldPos / L2_SIZE_M)) - 0.5;
    albedo *= (1.0 + microNoise * 0.06);

    float3 emissive = float3(0.0, 0.0, 0.0);
    if ((visual & VIS_EMISSIVE) != 0u)
        emissive += albedo * (emissiveAmt * 3.5);
    if ((visual & VIS_HEAT_GLOW) != 0u)
        emissive += float3(1.4, 0.38, 0.06) * max(0.3, emissiveAmt * 2.5);

    if ((visual & VIS_WET) != 0u)
    {
        albedo *= 0.68;
        roughness = min(roughness, 0.14);
    }
    if ((visual & VIS_FROZEN) != 0u)
    {
        albedo = lerp(albedo, float3(0.55, 0.85, 1.0), 0.48);
        roughness = min(roughness, 0.18);
    }
    if ((visual & VIS_CORRODED) != 0u)
    {
        float acidMask = step(0.55, hash31(floor(worldPos * 16.0)));
        albedo = lerp(albedo, float3(0.35, 0.72, 0.14), 0.35 + 0.30 * acidMask);
    }
    if ((visual & VIS_INFECTED) != 0u)
    {
        float pulse = 0.5 + 0.5 * sin(time * 3.0 + dot(worldPos, float3(4.0, 3.0, 5.0)));
        float3 spore = float3(0.45, 0.12, 0.65);
        albedo = lerp(albedo, spore, 0.35);
        emissive += spore * (0.25 + 0.45 * pulse);
    }
    if ((visual & VIS_BURNING) != 0u)
    {
        float flicker = 0.55 + 0.45 * sin(time * 14.0 + hash31(floor(worldPos * 8.0)) * 6.2831);
        albedo *= 0.5;
        emissive += float3(1.8, 0.48, 0.08) * flicker;
    }
    if ((visual & VIS_CHARGED) != 0u)
    {
        float spark = pow(0.5 + 0.5 * sin(time * 22.0 + dot(worldPos, float3(12.0, 9.0, 14.0))), 6.0);
        emissive += float3(0.25, 0.80, 1.8) * (0.15 + 1.2 * spark);
    }
    if ((visual & VIS_WARNING) != 0u)
    {
        float pulse = 0.5 + 0.5 * sin(time * 6.0);
        emissive += float3(1.2, 0.25, 0.05) * (0.2 + 0.5 * pulse);
    }
    if ((visual & VIS_STASIS) != 0u)
    {
        float shimmer = 0.5 + 0.5 * sin(time * 4.0 + worldPos.y * 10.0);
        emissive += float3(0.2, 0.55, 1.3) * (0.3 + 0.4 * shimmer);
    }
    if ((visual & VIS_BROKEN) != 0u)
    {
        float crack = step(0.86, abs(sin(dot(floor(worldPos * 16.0), float3(3.1, 5.7, 2.3)))));
        albedo *= (1.0 - crack * 0.55);
    }

    float3 sunDir = normalize(ubo.sunDirRadiance.xyz);
    float3 sunCol = ubo.sunColorAmbient.rgb * max(0.6, ubo.sunDirRadiance.w * 2.2);
    float ambientStrength = max(0.18, ubo.sunColorAmbient.w * 1.35);

    float ndl = max(dot(normal, sunDir), 0.0);
    float3 hemiSky = lerp(float3(0.26, 0.30, 0.36), float3(0.75, 0.88, 1.05), normal.y * 0.5 + 0.5);
    float directionalFacet = 0.88 + 0.12 * dot(normal, normalize(float3(0.3, 1.0, 0.2)));

    float3 H = normalize(sunDir + viewDir);
    float ndh = max(dot(normal, H), 0.0);
    float shininess = lerp(128.0, 8.0, clamp(roughness, 0.05, 1.0));
    float specScale = ((visual & VIS_METALLIC) != 0u) ? 0.55 : (((visual & VIS_WET) != 0u) ? 0.45 : 0.18);
    float spec = pow(ndh, shininess) * specScale * ndl;

    float3 lit = albedo * (sunCol * (ndl * shadowVis) + hemiSky * ambientStrength * directionalFacet);
    lit += sunCol * (spec * shadowVis);
    lit += emissive;
    return lit;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void voxelDdaMain(uint3 tid : SV_DispatchThreadID)
{
    uint width = ubo.counts.y;
    uint height = ubo.counts.z;
    if (tid.x >= width || tid.y >= height)
        return;

    int3 px = int3(int(tid.x), int(tid.y), 0);
    float4 forwardColor = forwardColorTex.Load(px);
    float  forwardDepth = forwardDepthTex.Load(px);

    float2 uv = (float2(tid.xy) + 0.5) / float2(width, height);
    float2 ndc = uv * 2.0 - 1.0;

    float4 pNear = mul(ubo.invViewProj, float4(ndc, 0.0, 1.0));
    float4 pFar  = mul(ubo.invViewProj, float4(ndc, 1.0, 1.0));
    pNear /= pNear.w;
    pFar  /= pFar.w;

    float3 rayOrigin = pNear.xyz;
    float3 rayDelta = pFar.xyz - pNear.xyz;
    float  farDist = length(rayDelta);
    float3 rayDir = (farDist > 1e-6) ? (rayDelta / farDist) : float3(0.0, 0.0, -1.0);

    TraceHit res = trace_world(rayOrigin, rayDir, min(farDist, 256.0), false);

    float3 outColor = forwardColor.rgb;
    float  outDepth = forwardDepth;

    if (res.hit)
    {
        float4 clipPos = mul(ubo.viewProj, float4(res.worldPos, 1.0));
        float voxelDepth = clipPos.z / clipPos.w;
        if (voxelDepth <= forwardDepth)
        {
            float3 sunDir = normalize(ubo.sunDirRadiance.xyz);
            TraceHit sh = trace_world(res.worldPos + res.normal * 0.025, sunDir, 64.0, true);
            float shadowVis = sh.hit ? 0.22 : 1.0;
            outColor = shade_voxel(
                res.worldPos, res.normal, res.rgba, res.visual, res.tier, -rayDir, shadowVis
            );
            // Distance aerial haze so distant terrain bounds feel continuous.
            float fog = clamp((res.t - 45.0) / 110.0, 0.0, 0.55);
            outColor = lerp(outColor, float3(0.42, 0.54, 0.72), fog);
            outDepth = voxelDepth;
        }
    }

    if (res.hasGlass && (!res.hit || res.glassT < res.t))
    {
        float3 glassPos = rayOrigin + rayDir * res.glassT;
        float4 glassClip = mul(ubo.viewProj, float4(glassPos, 1.0));
        float glassDepth = glassClip.z / glassClip.w;
        if (glassDepth <= outDepth)
        {
            float4 gc = unpack_rgba_linear(res.glassRgba);
            float3 litGlass = shade_voxel(
                glassPos, res.glassNormal, res.glassRgba, res.glassVisual, 2u, -rayDir, 1.0
            );
            float fresnel = pow(1.0 - max(dot(res.glassNormal, -rayDir), 0.0), 3.0);
            float alpha = clamp(gc.a + fresnel * 0.25, 0.15, 0.88);
            outColor = lerp(outColor, litGlass + float3(0.18, 0.26, 0.34) * fresnel, alpha);
            outDepth = glassDepth;
        }
    }

    // Analytic wireframe highlight for the 3D aimed L1/L2/L3 voxel bounds.
    if (ubo.counts.w != 0u)
    {
        float3 invDir = float3(
            abs(rayDir.x) > 1e-7 ? 1.0 / rayDir.x : 1e30,
            abs(rayDir.y) > 1e-7 ? 1.0 / rayDir.y : 1e30,
            abs(rayDir.z) > 1e-7 ? 1.0 / rayDir.z : 1e30
        );
        float bEnter, bExit;
        float3 bNormal;
        float boxEps = 0.002;
        float3 boxMin = ubo.cursorMin.xyz - float3(boxEps, boxEps, boxEps);
        float3 boxMax = ubo.cursorMax.xyz + float3(boxEps, boxEps, boxEps);
        if (intersect_aabb(rayOrigin, invDir, boxMin, boxMax, 0.0, 64.0, bEnter, bExit, bNormal))
        {
            float3 pHit = rayOrigin + rayDir * max(bEnter, 0.0);
            float4 bClip = mul(ubo.viewProj, float4(pHit, 1.0));
            float boxDepth = bClip.z / bClip.w;
            if (boxDepth <= outDepth + 1e-4)
            {
                float3 extents = max(boxMax - boxMin, float3(0.01, 0.01, 0.01));
                float3 local = (pHit - boxMin) / extents;
                float3 dEdge = min(local, 1.0 - local);
                float edgeTol = clamp(0.015 / min(extents.x, min(extents.y, extents.z)), 0.04, 0.18);
                int nearEdges = int(dEdge.x < edgeTol) + int(dEdge.y < edgeTol) + int(dEdge.z < edgeTol);
                if (nearEdges >= 2)
                {
                    outColor = lerp(outColor, ubo.cursorColor.rgb * 1.4, ubo.cursorColor.a);
                }
            }
        }
    }

    outColorTex[tid.xy] = float4(outColor, 1.0);
    outDepthTex[tid.xy] = outDepth;
}
