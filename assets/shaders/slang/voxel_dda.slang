// Three-tier compute DDA raymarcher + 2-bounce GI + emissive NEE + edge-aware À-Trous denoiser.
//
// Pipelines (all sharing Set 0):
//   1. voxelDdaMain       - Primary L1->L2->L3 DDA raymarch, direct sun/shadow, emissive NEE,
//                           2-bounce cosine hemisphere GI, temporal accumulation.
//   2. voxelGiDenoiseMain - 4-iteration 3x3 À-Trous bilateral filter on demodulated GI+AO using
//                           exact voxel face-normal ID and plane-distance edge stopping.
//   3. voxelCompositeMain - Re-modulates denoised GI by albedo, adds direct/spec/emissive,
//                           aerial haze, translucent glass, and 3D cursor wireframe.

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
    public float4x4 prevViewProj;
    public float4   cameraPos;       // xyz = eye position, w = elapsed time (seconds)
    public float4   sunDirRadiance;  // xyz = direction TO sun, w = sun radiance
    public float4   sunColorAmbient; // rgb = sun linear color, w = ambient factor
    public uint4    counts;          // x = chunkCount, y = widthPx, z = heightPx, w = cursorActive
    public uint4    counts2;         // x = emitterCount, y = frameIndex, z = hasPrevHistory, w = sceneChanged
    public float4   cursorMin;       // xyz = cursor box min, w = atlasFlag (16.0 if loaded else 0.0)
    public float4   cursorMax;       // xyz = cursor box max, w = exposure
    public float4   cursorColor;     // rgba = wireframe tint
};

public struct VoxelChunkGpu
{
    public int4  originAndL1Offset;  // xyz = origin_l1, w = base L1 cell index
    public uint4 dimsAndBrickOffset; // xyz = dims_l1, w = base brick index
};

public struct VoxelEmitterGpu
{
    public float4 bminRange;         // xyz = AABB min, w = effective max range
    public float4 bmaxRadius;        // xyz = AABB max, w = core soft radius
    public float4 colorIntensity;    // rgb = linear HDR emissive radiance, w = area/power weight
};

public struct VoxelDenoisePush
{
    public int  stepSize;
    public uint width;
    public uint height;
    public uint passIndex;
};

[[vk::push_constant]]
VoxelDenoisePush denoisePc;

[[vk::binding(0, 0)]]  ConstantBuffer<VoxelFrameUbo>   ubo;
[[vk::binding(1, 0)]]  StructuredBuffer<VoxelChunkGpu> chunks;
[[vk::binding(2, 0)]]  StructuredBuffer<uint4>         l1Cells;
[[vk::binding(3, 0)]]  StructuredBuffer<uint2>         brickOcc;
[[vk::binding(4, 0)]]  StructuredBuffer<uint4>         brickCells;
[[vk::binding(5, 0)]]  StructuredBuffer<uint>          spbrAtlas;
[[vk::binding(6, 0)]]  StructuredBuffer<VoxelEmitterGpu> emitters;
[[vk::binding(7, 0)]]  Texture2D<float4>               forwardColorTex;
[[vk::binding(8, 0)]]  Texture2D<float>                forwardDepthTex;
[[vk::binding(9, 0)]]  Texture2D<float4>               prevGiTex;
[[vk::binding(10, 0)]] Texture2D<float4>               prevPosNormTex;
[[vk::binding(11, 0)]] Texture2D<float4>               giReadTex;
[[vk::binding(12, 0)]] Texture2D<float4>               posNormReadTex;
[[vk::binding(13, 0)]] Texture2D<float4>               directReadTex;
[[vk::binding(14, 0)]] Texture2D<float4>               albedoReadTex;
[[vk::binding(15, 0)]] Texture2D<float4>               specEnvReadTex;
[[vk::binding(16, 0)]] Texture2D<uint4>                glassReadTex;
[[vk::binding(17, 0)]] [[vk::image_format("rgba16f")]]  RWTexture2D<float4> outColorTex;
[[vk::binding(18, 0)]] [[vk::image_format("r32f")]]     RWTexture2D<float>  outDepthTex;
[[vk::binding(19, 0)]] [[vk::image_format("rgba16f")]]  RWTexture2D<float4> directOutTex;
[[vk::binding(20, 0)]] [[vk::image_format("rgba16f")]]  RWTexture2D<float4> albedoOutTex;
[[vk::binding(21, 0)]] [[vk::image_format("rgba16f")]]  RWTexture2D<float4> specEnvOutTex;
[[vk::binding(22, 0)]] [[vk::image_format("rgba32f")]]  RWTexture2D<float4> posNormOutTex;
[[vk::binding(23, 0)]] [[vk::image_format("rgba16f")]]  RWTexture2D<float4> giOutTex;
[[vk::binding(24, 0)]] [[vk::image_format("rgba32ui")]] RWTexture2D<uint4>  glassOutTex;

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

int encode_normal_id(float3 n)
{
    float3 an = abs(n);
    if (an.x >= an.y && an.x >= an.z)
        return (n.x >= 0.0) ? 0 : 1;
    if (an.y >= an.z)
        return (n.y >= 0.0) ? 2 : 3;
    return (n.z >= 0.0) ? 4 : 5;
}

float3 decode_normal_id(int id)
{
    if (id == 0) return float3( 1.0,  0.0,  0.0);
    if (id == 1) return float3(-1.0,  0.0,  0.0);
    if (id == 2) return float3( 0.0,  1.0,  0.0);
    if (id == 3) return float3( 0.0, -1.0,  0.0);
    if (id == 4) return float3( 0.0,  0.0,  1.0);
    return float3( 0.0,  0.0, -1.0);
}

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

float4 unpack_unorm4(uint packed)
{
    return float4(
        float(packed & 0xFFu),
        float((packed >> 8u) & 0xFFu),
        float((packed >> 16u) & 0xFFu),
        float((packed >> 24u) & 0xFFu)
    ) * (1.0 / 255.0);
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
    float curMaxT = maxT;
    float3 segEnd = origin + dir * curMaxT;
    float3 segMin = min(origin, segEnd) - float3(0.01, 0.01, 0.01);
    float3 segMax = max(origin, segEnd) + float3(0.01, 0.01, 0.01);

    for (uint i = 0u; i < count; i++)
    {
        VoxelChunkGpu chunk = chunks[i];
        float3 bmin = float3(chunk.originAndL1Offset.xyz) * L1_SIZE_M;
        float3 bmax = bmin + float3(chunk.dimsAndBrickOffset.xyz) * L1_SIZE_M;
        if (any(segMax < bmin) || any(segMin > bmax))
            continue;

        trace_chunk(chunk, origin, dir, invDir, curMaxT, shadowRay, best);
        if (shadowRay && best.hit)
            return best;
        if (best.t < curMaxT)
        {
            curMaxT = best.t;
            segEnd = origin + dir * curMaxT;
            segMin = min(origin, segEnd) - float3(0.01, 0.01, 0.01);
            segMax = max(origin, segEnd) + float3(0.01, 0.01, 0.01);
        }
    }
    return best;
}

float hash31(float3 p)
{
    p = frac(p * 0.1031);
    p += dot(p, p.yzx + 33.33);
    return frac((p.x + p.y) * p.z);
}

// Jimenez Interleaved Gradient Noise — blue-noise-like screen spectrum with zero clumping.
float interleaved_gradient_noise(uint2 px, uint frameIdx)
{
    float2 fpx = float2(px) + 5.588238 * float(frameIdx & 15u);
    return frac(52.9829189 * frac(dot(fpx, float2(0.06711056, 0.00583715))));
}

static const float VOXEL_PI = 3.14159265;

float d_ggx(float ndh, float roughness)
{
    float a = max(roughness * roughness, 0.002);
    float a2 = a * a;
    float denom = ndh * ndh * (a2 - 1.0) + 1.0;
    return a2 / max(VOXEL_PI * denom * denom, 1e-5);
}

float g_schlick_ggx(float ndv, float roughness)
{
    float r = roughness + 1.0;
    float k = (r * r) * 0.125;
    return ndv / max(ndv * (1.0 - k) + k, 1e-5);
}

float g_smith(float ndl, float ndv, float roughness)
{
    return g_schlick_ggx(ndl, roughness) * g_schlick_ggx(ndv, roughness);
}

float3 f_schlick(float cosTheta, float3 f0)
{
    return f0 + (1.0 - f0) * pow(clamp(1.0 - cosTheta, 0.0, 1.0), 5.0);
}

float sky_noise2(float2 p)
{
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    float a = hash31(float3(i, 0.0));
    float b = hash31(float3(i + float2(1.0, 0.0), 0.0));
    float c = hash31(float3(i + float2(0.0, 1.0), 0.0));
    float d = hash31(float3(i + float2(1.0, 1.0), 0.0));
    return lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y);
}

float sky_fbm(float2 p)
{
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++)
    {
        v += a * sky_noise2(p);
        p = p * 2.04 + float2(17.3, 9.2);
        a *= 0.5;
    }
    return v;
}

void compute_tod_weights(out float isNight, out float dayFactor, out float twilightFactor, out float overcast)
{
    float3 lightCol = ubo.sunColorAmbient.rgb;
    float ambient = ubo.sunColorAmbient.w;
    float sunRad = ubo.sunDirRadiance.w;
    float3 sunDir = normalize(ubo.sunDirRadiance.xyz);

    isNight = smoothstep(0.27, 0.16, ambient) * smoothstep(-0.02, 0.10, lightCol.b - lightCol.r + 0.04);
    dayFactor = smoothstep(0.16, 0.50, ambient) * (1.0 - isNight);
    twilightFactor = clamp(1.0 - sunDir.y * 3.0, 0.0, 1.0) * (1.0 - isNight);
    float expectedClearRad = lerp(0.25, 2.2, dayFactor);
    overcast = clamp(1.0 - sunRad / max(expectedClearRad, 0.2), 0.0, 0.85);
}

float3 sample_atmosphere(float3 dir, float3 sunDir, float3 sunCol)
{
    float isNight, dayFactor, twilightFactor, overcast;
    compute_tod_weights(isNight, dayFactor, twilightFactor, overcast);

    float up = clamp(dir.y, -0.2, 1.0);
    float horizonPow = pow(1.0 - max(up, 0.0), 4.0);

    float3 dayZenith = float3(0.13, 0.36, 0.82);
    float3 dayHorizon = float3(0.56, 0.76, 0.95);
    float3 duskZenith = float3(0.10, 0.14, 0.36);
    float3 duskHorizon = float3(0.94, 0.44, 0.20);
    float3 nightZenith = float3(0.008, 0.014, 0.036);
    float3 nightHorizon = float3(0.024, 0.042, 0.088);

    float3 zenith = lerp(dayZenith, duskZenith, twilightFactor);
    float3 horizon = lerp(dayHorizon, duskHorizon, twilightFactor);
    zenith = lerp(zenith, nightZenith, isNight);
    horizon = lerp(horizon, nightHorizon, isNight);

    float3 stormCol = lerp(float3(0.22, 0.26, 0.32), float3(0.025, 0.032, 0.045), isNight);
    zenith = lerp(zenith, stormCol * 0.75, overcast * 0.80);
    horizon = lerp(horizon, stormCol, overcast * 0.85);

    float3 sky = lerp(horizon, zenith, pow(max(up, 0.0), 0.52));

    float sunCos = max(dot(dir, sunDir), 0.0);
    float glowScale = (1.0 - overcast * 0.75) * lerp(1.0, 0.22, isNight);
    sky += sunCol * (0.28 * pow(sunCos, 8.0) + 0.80 * pow(sunCos, 96.0)) * glowScale;
    sky += float3(0.98, 0.42, 0.16) * (twilightFactor * 0.35 * horizonPow * pow(sunCos, 3.0));
    return sky;
}

float3 render_procedural_sky(float3 dir, float3 sunDir, float3 sunCol)
{
    float isNight, dayFactor, twilightFactor, overcast;
    compute_tod_weights(isNight, dayFactor, twilightFactor, overcast);

    float3 sky = sample_atmosphere(dir, sunDir, sunCol);
    float time = ubo.cameraPos.w;
    float sunCos = max(dot(dir, sunDir), 0.0);

    if (isNight < 0.5)
    {
        float disc = smoothstep(0.99935, 0.99972, sunCos) * (1.0 - overcast * 0.85);
        sky += sunCol * (disc * 6.5);
    }
    else
    {
        float moonDisc = smoothstep(0.99945, 0.99975, sunCos) * (1.0 - overcast * 0.85);
        float crater = 0.82 + 0.18 * sky_noise2(dir.xz * 95.0);
        sky += float3(0.88, 0.92, 1.0) * (moonDisc * 2.4 * crater);
    }

    if (isNight > 0.02 && dir.y > 0.01)
    {
        float starMask = isNight * smoothstep(0.01, 0.26, dir.y) * (1.0 - overcast * 0.92);
        float3 starCell = floor(dir * 260.0);
        float h = hash31(starCell);
        if (h > 0.991)
        {
            float3 cellCenter = normalize(starCell + 0.5);
            float dist = length(dir - cellCenter) * 260.0;
            float sparkle = smoothstep(0.45, 0.05, dist);
            float twinkle = 0.65 + 0.35 * sin(time * 2.4 + h * 180.0);
            float3 starTint = lerp(float3(0.72, 0.84, 1.0), float3(1.0, 0.90, 0.72), frac(h * 73.1));
            sky += starTint * (sparkle * twinkle * starMask * 1.45);
        }
        float mw = sky_fbm(dir.xz * 3.5 + float2(1.2, 4.7));
        sky += float3(0.035, 0.048, 0.085) * (smoothstep(0.48, 0.78, mw) * starMask);
    }

    if (dir.y > 0.015)
    {
        float cloudPlaneDist = 120.0 / max(dir.y, 0.035);
        float2 cloudUv = dir.xz * (cloudPlaneDist * 0.0085) + float2(time * 0.014, time * 0.006);
        float n = sky_fbm(cloudUv);
        float detail = sky_fbm(cloudUv * 2.7 - float2(time * 0.008, 0.0));
        float density = smoothstep(0.48 - overcast * 0.22, 0.76 - overcast * 0.14, n * 0.72 + detail * 0.28);
        float horizonFade = smoothstep(0.015, 0.18, dir.y);
        float cloudAlpha = density * horizonFade * lerp(0.72, 0.92, overcast);

        float3 cloudLit = lerp(float3(0.95, 0.97, 1.0), float3(1.0, 0.62, 0.38), twilightFactor);
        cloudLit = lerp(cloudLit, float3(0.09, 0.12, 0.18), isNight);
        cloudLit = lerp(cloudLit, float3(0.28, 0.31, 0.36) * (1.0 - isNight * 0.75), overcast * 0.7);
        float3 cloudDark = cloudLit * lerp(0.58, 0.42, isNight);

        float silverLining = pow(sunCos, 6.0) * (1.0 - overcast * 0.65) * lerp(0.7, 0.2, isNight);
        float3 cloudCol = lerp(cloudDark, cloudLit, smoothstep(0.2, 0.85, n)) + sunCol * silverLining * 0.45;
        sky = lerp(sky, cloudCol, cloudAlpha);
    }

    return sky;
}

uint resolve_spbr_tile(uint rgba, uint visual, float3 normal)
{
    if (ubo.cursorMin.w < 0.5)
        return 0xFFFFFFFFu;

    uint r = rgba & 0xFFu;
    uint g = (rgba >> 8u) & 0xFFu;
    uint b = (rgba >> 16u) & 0xFFu;

    if ((visual & VIS_LIQUID) != 0u)
    {
        if ((visual & VIS_FROZEN) != 0u) return 9u;  // ice
        if ((visual & VIS_CORRODED) != 0u) return 8u; // acid
        return 7u;                                    // water
    }
    if ((visual & VIS_TRANSLUCENT) != 0u) return 5u;  // glass
    if ((visual & VIS_INFECTED) != 0u) return 10u;    // amethyst/biomass
    if ((visual & VIS_EMISSIVE) != 0u && g > r && g > 140u) return 14u; // emerald/isotope green
    if ((visual & VIS_METALLIC) != 0u)
    {
        if (r > 180u && g > 100u && b < 80u) return 11u; // copper/wire
        if (r > 180u && g < 110u) return 13u;            // valve/redstone
        return 4u;                                       // iron/alloy
    }
    if (g > r + 25u && g > b + 25u)
    {
        if (normal.y > 0.5) return 2u;
        if (normal.y < -0.5) return 1u;
        return 15u;
    }
    if (r < 90u && g < 90u && b < 100u) return 0u;   // bedrock
    if (r > 145u && g > 140u && b > 130u) return 3u; // stone/limestone
    if (r > 135u && g > 85u && b < 80u) return 6u;   // oak_planks/timber
    return 1u;                                       // dirt
}

int2 voxel_face_texel(float3 worldPos, float3 normal)
{
    float2 uv;
    if (abs(normal.y) > 0.5)
        uv = frac(worldPos.xz);
    else if (abs(normal.x) > 0.5)
        uv = frac(float2(worldPos.z, -worldPos.y));
    else
        uv = frac(float2(worldPos.x, -worldPos.y));
    int2 px = int2(floor(uv * 16.0));
    return clamp(px, int2(0, 0), int2(15, 15));
}

float3 sample_spbr_normal(uint tile, int2 px, float3 normal)
{
    uint packed = spbrAtlas[(px.y + 16) * 256 + int(tile) * 16 + px.x];
    if (packed == 0u)
        return normal;
    float4 nTex = unpack_unorm4(packed);
    float2 xy = nTex.xy * 2.0 - 1.0;
    float nz = sqrt(max(1.0 - dot(xy, xy), 0.0));
    float3 tN = normalize(float3(xy.x, xy.y, nz));

    float3 T, B;
    if (abs(normal.y) > 0.5)
    {
        T = float3(1.0, 0.0, 0.0);
        B = float3(0.0, 0.0, normal.y > 0.0 ? 1.0 : -1.0);
    }
    else if (abs(normal.x) > 0.5)
    {
        T = float3(0.0, 0.0, 1.0);
        B = float3(0.0, 1.0, 0.0);
    }
    else
    {
        T = float3(1.0, 0.0, 0.0);
        B = float3(0.0, 1.0, 0.0);
    }
    return normalize(T * tN.x + B * tN.y + normal * tN.z);
}

float3 perturb_voxel_normal(float3 worldPos, float3 normal, uint visual, float time, float hitDist, uint tile)
{
    float3 n = normal;
    if ((visual & VIS_LIQUID) != 0u)
    {
        float2 p = worldPos.xz * 4.5;
        float w1 = sin(p.x * 1.7 + time * 2.4 + cos(p.y * 1.3 - time * 1.8));
        float w2 = cos(p.y * 2.3 - time * 2.1 + sin(p.x * 1.9 + time * 1.5));
        float3 ripple = float3(w1 * 0.14, 0.0, w2 * 0.14);
        n = normalize(normal + ripple * (1.0 - abs(normal.x) * 0.5));
        return n;
    }

    if (hitDist > 36.0)
        return normal;

    if (tile != 0xFFFFFFFFu)
    {
        int2 px = voxel_face_texel(worldPos, normal);
        return sample_spbr_normal(tile, px, normal);
    }

    float3 texel = floor(worldPos * 16.0);
    float gx = hash31(texel + float3(1.0, 0.0, 0.0)) - hash31(texel - float3(1.0, 0.0, 0.0));
    float gy = hash31(texel + float3(0.0, 1.0, 0.0)) - hash31(texel - float3(0.0, 1.0, 0.0));
    float gz = hash31(texel + float3(0.0, 0.0, 1.0)) - hash31(texel - float3(0.0, 0.0, 1.0));
    float3 grad = float3(gx, gy, gz);
    grad -= normal * dot(grad, normal);
    float bumpStrength = ((visual & VIS_METALLIC) != 0u) ? 0.10 : 0.18;
    return normalize(normal + grad * bumpStrength);
}

// Evaluate unified voxel emissive HDR radiance across static SPBR, material flags, and transient states.
float3 eval_voxel_emissive(
    float3 worldPos,
    uint   rgba,
    uint   visual,
    float3 albedo,
    float  time,
    uint   tile,
    int2   px
)
{
    float3 emissive = float3(0.0, 0.0, 0.0);
    float emissiveAmt = float((visual >> 24u) & 0xFFu) * (1.0 / 255.0);

    if (tile != 0xFFFFFFFFu)
    {
        uint specPacked = spbrAtlas[(px.y + 32) * 256 + int(tile) * 16 + px.x];
        if (specPacked != 0u)
        {
            float4 specTex = unpack_unorm4(specPacked);
            if (specTex.a > 0.0 && specTex.a < 0.999)
                emissive += albedo * (specTex.a * 2.5);
        }
    }

    if ((visual & VIS_EMISSIVE) != 0u || emissiveAmt > 0.0)
        emissive += albedo * (max(emissiveAmt, 0.35) * 3.8);
    if ((visual & VIS_HEAT_GLOW) != 0u)
        emissive += float3(2.2, 0.55, 0.08) * max(0.55, emissiveAmt * 3.8);
    if ((visual & VIS_INFECTED) != 0u)
    {
        float pulse = 0.5 + 0.5 * sin(time * 3.0 + dot(worldPos, float3(4.0, 3.0, 5.0)));
        emissive += float3(0.75, 0.18, 1.15) * (0.45 + 0.65 * pulse);
    }
    if ((visual & VIS_BURNING) != 0u)
    {
        float flicker = 0.65 + 0.35 * sin(time * 14.0 + hash31(floor(worldPos * 8.0)) * 6.2831);
        emissive += float3(3.6, 1.05, 0.16) * flicker;
    }
    if ((visual & VIS_CHARGED) != 0u)
    {
        float spark = pow(0.5 + 0.5 * sin(time * 22.0 + dot(worldPos, float3(12.0, 9.0, 14.0))), 6.0);
        emissive += float3(0.45, 1.45, 3.6) * (0.35 + 1.65 * spark);
    }
    if ((visual & VIS_WARNING) != 0u)
    {
        float pulse = 0.5 + 0.5 * sin(time * 6.0);
        emissive += float3(2.2, 0.45, 0.08) * (0.35 + 0.75 * pulse);
    }
    if ((visual & VIS_STASIS) != 0u)
    {
        float shimmer = 0.5 + 0.5 * sin(time * 4.0 + worldPos.y * 10.0);
        emissive += float3(0.35, 0.95, 2.4) * (0.50 + 0.60 * shimmer);
    }
    return emissive;
}

void apply_voxel_surface_detail(
    float3 worldPos,
    float3 normal,
    uint visual,
    uint tier,
    uint tile,
    inout float3 albedo,
    inout float roughness,
    inout float metallic
)
{
    if (tile != 0xFFFFFFFFu)
    {
        int2 px = voxel_face_texel(worldPos, normal);
        uint albPacked = spbrAtlas[px.y * 256 + int(tile) * 16 + px.x];
        float4 albTex = unpack_rgba_linear(albPacked);
        if (albTex.a > 0.05)
        {
            // Minecraft grass_block_top.png is a grayscale biome-tintable mask.
            if (tile == 2u)
                albedo = saturate(albedo * albTex.rgb * 2.8);
            else
                albedo = albTex.rgb;
        }

        uint specPacked = spbrAtlas[(px.y + 32) * 256 + int(tile) * 16 + px.x];
        if (specPacked != 0u)
        {
            float4 specTex = unpack_unorm4(specPacked);
            float smoothness = specTex.r;
            roughness = clamp((1.0 - smoothness) * (1.0 - smoothness), 0.04, 1.0);
            if (specTex.g >= (229.5 / 255.0))
                metallic = 1.0;
        }
        return;
    }

    float cellStep = (tier == 3u) ? L3_SIZE_M : ((tier == 2u) ? L2_SIZE_M : L1_SIZE_M);
    float3 gridP = worldPos / cellStep;
    float3 f = abs(frac(gridP) - 0.5);
    float3 maskN = 1.0 - abs(normal);
    float edgeMax = max(f.x * maskN.x, max(f.y * maskN.y, f.z * maskN.z));
    float bevel = smoothstep(0.43, 0.50, edgeMax);
    albedo *= lerp(1.0, (tier == 3u) ? 0.78 : 0.86, bevel);

    float3 texel = floor(worldPos * 16.0);
    float h0 = hash31(texel);
    float hStrata = hash31(float3(floor(worldPos.x * 4.0), texel.y, floor(worldPos.z * 4.0)));

    if ((visual & VIS_METALLIC) != 0u)
    {
        float panelEdge = step(0.44, edgeMax);
        albedo *= lerp(0.92 + 0.16 * h0, 0.68, panelEdge);
        roughness = clamp(roughness + (h0 - 0.5) * 0.18 - panelEdge * 0.10, 0.06, 0.95);
    }
    else if ((visual & VIS_LIQUID) == 0u)
    {
        float grain = (h0 - 0.5) * 0.16 + (hStrata - 0.5) * 0.08;
        albedo *= clamp(1.0 + grain, 0.76, 1.24);
        roughness = clamp(roughness + (h0 - 0.5) * 0.18, 0.08, 0.98);
    }
}

void resolve_hit_surface(
    float3 worldPos,
    float3 normal,
    uint   rgba,
    uint   visual,
    uint   tier,
    float  time,
    out float3 albedoOut,
    out float3 emissiveOut,
    out float  roughnessOut,
    out float  metallicOut,
    out uint   tileOut
)
{
    float4 baseCol = unpack_rgba_linear(rgba);
    float3 albedo = baseCol.rgb;
    float roughness = clamp(float((visual >> 16u) & 0xFFu) * (1.0 / 255.0), 0.04, 1.0);
    float metallic = ((visual & VIS_METALLIC) != 0u) ? 0.90 : 0.0;

    uint tile = resolve_spbr_tile(rgba, visual, normal);
    apply_voxel_surface_detail(worldPos, normal, visual, tier, tile, albedo, roughness, metallic);
    int2 texelPx = (tile != 0xFFFFFFFFu) ? voxel_face_texel(worldPos, normal) : int2(0, 0);
    float3 emissive = eval_voxel_emissive(worldPos, rgba, visual, albedo, time, tile, texelPx);

    if ((visual & VIS_WET) != 0u)
    {
        albedo *= 0.68;
        roughness = min(roughness, 0.12);
    }
    if ((visual & VIS_FROZEN) != 0u)
    {
        albedo = lerp(albedo, float3(0.58, 0.86, 1.0), 0.45);
        roughness = min(roughness, 0.16);
    }
    if ((visual & VIS_CORRODED) != 0u)
    {
        float acidMask = step(0.55, hash31(floor(worldPos * 16.0)));
        albedo = lerp(albedo, float3(0.35, 0.72, 0.14), 0.35 + 0.30 * acidMask);
    }
    if ((visual & VIS_INFECTED) != 0u)
    {
        albedo = lerp(albedo, float3(0.45, 0.12, 0.65), 0.35);
    }
    if ((visual & VIS_BURNING) != 0u)
    {
        albedo *= 0.5;
    }
    if ((visual & VIS_BROKEN) != 0u)
    {
        float crack = step(0.86, abs(sin(dot(floor(worldPos * 16.0), float3(3.1, 5.7, 2.3)))));
        albedo *= (1.0 - crack * 0.55);
    }

    albedoOut = albedo;
    emissiveOut = emissive;
    roughnessOut = roughness;
    metallicOut = metallic;
    tileOut = tile;
}

// Explicit Next-Event Estimation from active emissive voxel clusters with occlusion ray.
float3 eval_emitters_at_surface(
    float3 worldPos,
    float3 normal,
    float  jitterA,
    float  jitterB,
    bool   traceVisibility
)
{
    uint emitterCount = ubo.counts2.x;
    if (emitterCount == 0u)
        return float3(0.0, 0.0, 0.0);

    float3 rayOrigin = worldPos + normal * 0.03;

    // Stochastic RIS: evaluate cheap analytic weights for all emitters, then trace at most 1 visibility ray.
    float wSum = 0.0;
    float bestW = 0.0;
    float3 bestContrib = float3(0.0, 0.0, 0.0);
    float3 bestDir = float3(0.0, 1.0, 0.0);
    float bestMaxVisT = 0.0;
    float3 bestBmin = float3(0.0, 0.0, 0.0);
    float3 bestBmax = float3(0.0, 0.0, 0.0);
    float bestDist = 0.0;
    float rng = frac(jitterA * 1.73205 + jitterB * 2.23606);

    for (uint e = 0u; e < emitterCount; e++)
    {
        VoxelEmitterGpu em = emitters[e];
        float3 bmin = em.bminRange.xyz;
        float3 bmax = em.bmaxRadius.xyz;
        float maxRange = em.bminRange.w;
        float softR = max(em.bmaxRadius.w, 0.65);

        float3 closest = clamp(worldPos, bmin, bmax);
        float3 center = 0.5 * (bmin + bmax);
        float3 halfExt = max(0.5 * (bmax - bmin), float3(0.05, 0.05, 0.05));
        float3 anchor = lerp(closest, center, 0.65);
        float anchorDist = length(anchor - worldPos);
        if (anchorDist > maxRange || anchorDist < 0.03)
            continue;

        float3 jitterVec = float3(jitterA - 0.5, jitterB - 0.5, frac(jitterA + jitterB) - 0.5) * (halfExt * 1.35);
        float3 samplePt = clamp(anchor + jitterVec, bmin + halfExt * 0.04, bmax - halfExt * 0.04);
        float3 toE = samplePt - worldPos;
        float dist = length(toE);
        if (dist < 0.03 || dist > maxRange)
            continue;

        float3 dir = toE / dist;
        float ndl = max(dot(normal, dir), 0.0);
        if (ndl <= 0.0)
            continue;

        float effDist = max(anchorDist, 0.55);
        float atten = 1.0 / (effDist * effDist + softR * softR);
        float fade = saturate(1.0 - effDist / maxRange);
        fade *= fade;
        float3 contrib = em.colorIntensity.rgb * (em.colorIntensity.w * ndl * atten * fade);
        float w = max(contrib.r, max(contrib.g, contrib.b));
        if (w <= 1e-6)
            continue;

        wSum += w;
        rng = frac(rng * 1.6180339 + 0.381966);
        if (rng * wSum < w)
        {
            bestW = w;
            bestContrib = contrib;
            bestDir = dir;
            bestDist = dist;
            bestBmin = bmin;
            bestBmax = bmax;
        }
    }

    if (wSum <= 1e-6 || bestW <= 1e-6)
        return float3(0.0, 0.0, 0.0);

    if (traceVisibility && bestDist > 0.06)
    {
        float3 invD = float3(
            abs(bestDir.x) > 1e-6 ? 1.0 / bestDir.x : 1e30,
            abs(bestDir.y) > 1e-6 ? 1.0 / bestDir.y : 1e30,
            abs(bestDir.z) > 1e-6 ? 1.0 / bestDir.z : 1e30
        );
        float bEnter, bExit;
        float3 bN;
        bestMaxVisT = bestDist - 0.03;
        if (intersect_aabb(rayOrigin, invD, bestBmin, bestBmax, 0.0, bestDist + 0.1, bEnter, bExit, bN))
            bestMaxVisT = bEnter - 0.03;
        if (bestMaxVisT > 0.04)
        {
            TraceHit sh = trace_world(rayOrigin, bestDir, bestMaxVisT, true);
            if (sh.hit)
                return float3(0.0, 0.0, 0.0);
        }
    }

    float3 result = bestContrib * (wSum / bestW);
    return result / (1.0 + result * 0.18);
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

// Distance-tiered 2-bounce voxel GI + sky visibility + short-range AO + emissive voxel contribution.
void trace_voxel_ao_and_gi(
    float3 worldPos,
    float3 normal,
    float3 sunDir,
    float3 sunCol,
    float  ambientStrength,
    uint2  px,
    float  time,
    float  hitDist,
    bool   highRes,
    out float  aoOut,
    out float  skyVisOut,
    out float3 bounceGiOut
)
{
    float3 tangent, bitangent;
    build_orthonormal_basis(normal, tangent, bitangent);

    uint frameIdx = ubo.counts2.y;
    float ignA = interleaved_gradient_noise(px, frameIdx);
    float ignB = interleaved_gradient_noise(px + uint2(17u, 29u), frameIdx + 5u);

    // Balanced 2-ray / 1-bounce near-field GI + temporal accumulation + À-Trous denoiser.
    // At high resolution (>1.5M px), cap to 1 ray and tighter 2nd-bounce radius to prevent WDDM TDR.
    uint giRays = (!highRes && hitDist < 12.0) ? 2u : 1u;
    uint b2Cap  = (hitDist < (highRes ? 8.0 : 12.0)) ? 1u : 0u;
    float b1MaxT = (hitDist < 24.0) ? 12.0 : 8.0;

    float occ = 0.0;
    float skyVis = 0.0;
    float3 gi = float3(0.0, 0.0, 0.0);
    float3 rayOrigin = worldPos + normal * 0.03;

    for (uint i = 0u; i < giRays; i++)
    {
        float u1 = (float(i) + ignA) * (1.0 / float(giRays));
        float u2 = frac(ignB + float(i) * 0.61803398875) * 0.90;
        float3 d1 = sample_cosine_hemisphere(normal, tangent, bitangent, u1, u2);

        TraceHit h1 = trace_world(rayOrigin, d1, b1MaxT, false);

        // Translucent emissive fluids/glass (AcidPool, CryoSlush, glowing glass) skipped by opaque DDA.
        if (h1.hasGlass && (!h1.hit || h1.glassT < h1.t))
        {
            float3 gAlb = unpack_rgba_linear(h1.glassRgba).rgb;
            uint gTile = resolve_spbr_tile(h1.glassRgba, h1.glassVisual, h1.glassNormal);
            int2 gPx = (gTile != 0xFFFFFFFFu) ? voxel_face_texel(rayOrigin + d1 * h1.glassT, h1.glassNormal) : int2(0, 0);
            float3 gEmit = eval_voxel_emissive(rayOrigin + d1 * h1.glassT, h1.glassRgba, h1.glassVisual, gAlb, time, gTile, gPx);
            gi += gEmit * 0.65;
        }

        if (!h1.hit)
        {
            skyVis += 1.0;
            gi += sample_atmosphere(d1, sunDir, sunCol) * (ambientStrength * 0.85);
            continue;
        }

        if (h1.t < 2.8)
        {
            float distWeight = 1.0 - clamp(h1.t / 2.8, 0.0, 1.0);
            occ += distWeight * 1.10;
        }

        float3 hit1Alb, hit1Emit;
        float hit1Rough, hit1Met;
        uint hit1Tile;
        resolve_hit_surface(
            h1.worldPos, h1.normal, h1.rgba, h1.visual, h1.tier, time,
            hit1Alb, hit1Emit, hit1Rough, hit1Met, hit1Tile
        );

        // Direct sunlight on Bounce-1 surface (with shadow check so sunlit roofs never leak indoors).
        // Direct sunlight on Bounce-1 surface (with shadow check so sunlit roofs never leak indoors).
        float hit1Ndl = max(dot(h1.normal, sunDir), 0.0);
        float hit1Vis = 0.0;
        if (hit1Ndl > 0.0)
        {
            TraceHit s1 = trace_world(h1.worldPos + h1.normal * 0.03, sunDir, 28.0, true);
            hit1Vis = s1.hit ? 0.0 : 1.0;
        }
        float3 hit1Direct = sunCol * (hit1Ndl * hit1Vis / VOXEL_PI);

        // Emissive NEE at Bounce-1 hit without extra visibility rays.
        float3 hit1EmitterBounce = eval_emitters_at_surface(h1.worldPos, h1.normal, ignB, ignA, false) * 0.45;

        float3 bounce2 = sample_atmosphere(h1.normal, sunDir, sunCol) * (ambientStrength * 0.25);
        if (i < b2Cap)
        {
            // Bounce 2 from h1 -> h2 (brightens sheltered interiors, overhangs, and colored multi-bounce).
            float3 t2, b2;
            build_orthonormal_basis(h1.normal, t2, b2);
            float u2a = frac(ignA * 3.17 + float(i) * 0.381966);
            float u2b = frac(ignB * 2.71 + float(i) * 0.754877) * 0.88;
            float3 d2 = sample_cosine_hemisphere(h1.normal, t2, b2, u2a, u2b);
            TraceHit h2 = trace_world(h1.worldPos + h1.normal * 0.03, d2, 8.0, false);

            if (!h2.hit)
            {
                bounce2 = sample_atmosphere(d2, sunDir, sunCol) * (ambientStrength * 0.75);
            }
            else
            {
                float3 hit2Alb = unpack_rgba_linear(h2.rgba).rgb;
                uint hit2Tile = resolve_spbr_tile(h2.rgba, h2.visual, h2.normal);
                int2 hit2Px = (hit2Tile != 0xFFFFFFFFu) ? voxel_face_texel(h2.worldPos, h2.normal) : int2(0, 0);
                if (hit2Tile != 0xFFFFFFFFu)
                {
                    float4 albTex2 = unpack_rgba_linear(spbrAtlas[hit2Px.y * 256 + int(hit2Tile) * 16 + hit2Px.x]);
                    if (albTex2.a > 0.05)
                        hit2Alb = (hit2Tile == 2u) ? saturate(hit2Alb * albTex2.rgb * 2.8) : albTex2.rgb;
                }
                float3 hit2Emit = eval_voxel_emissive(h2.worldPos, h2.rgba, h2.visual, hit2Alb, time, hit2Tile, hit2Px);
                float hit2Ndl = max(dot(h2.normal, sunDir), 0.0);
                float hit2Vis = 0.0;
                if (hit2Ndl > 0.0)
                {
                    TraceHit s2 = trace_world(h2.worldPos + h2.normal * 0.03, sunDir, 20.0, true);
                    hit2Vis = s2.hit ? 0.0 : 1.0;
                }
                bounce2 = hit2Alb * (sunCol * (hit2Ndl * hit2Vis / VOXEL_PI)) + hit2Emit * 0.95;
            }
        }

        // Physical Lambertian bounce energy without over-saturating NEE emitters.
        float3 hit1Irradiance = (hit1Direct + hit1EmitterBounce + bounce2) * 1.35;
        gi += hit1Alb * hit1Irradiance + hit1Emit * 0.35;
    }

    // Explicit direct emissive NEE at primary hit P (soft shadows cleaned by À-Trous denoiser).
    float3 directEmitterGi = eval_emitters_at_surface(worldPos, normal, ignA, ignB, hitDist < 28.0) * 0.85;

    float invRays = 1.0 / float(giRays);
    aoOut = clamp(1.0 - (occ * invRays) * 0.90, 0.22, 1.0);
    skyVisOut = skyVis * invRays;
    bounceGiOut = gi * invRays + directEmitterGi;
}

float3 shade_translucent_glass(
    float3 worldPos,
    float3 normal,
    uint   rgba,
    uint   visual,
    float3 viewDir,
    float  hitDist,
    float3 baseComposite
)
{
    float time = ubo.cameraPos.w;
    float3 albedo, emissive;
    float roughness, metallic;
    uint tile;
    resolve_hit_surface(worldPos, normal, rgba, visual, 2u, time, albedo, emissive, roughness, metallic, tile);

    float3 N = perturb_voxel_normal(worldPos, normal, visual, time, hitDist, tile);
    float3 V = normalize(viewDir);
    float3 L = normalize(ubo.sunDirRadiance.xyz);
    float3 H = normalize(L + V);
    float3 sunCol = ubo.sunColorAmbient.rgb * max(0.65, ubo.sunDirRadiance.w * 2.3);
    float ambientStrength = max(0.20, ubo.sunColorAmbient.w * 1.35);

    float ndl = max(dot(N, L), 0.0);
    float ndv = max(dot(N, V), 1e-4);
    float ndh = max(dot(N, H), 0.0);
    float vdh = max(dot(V, H), 0.0);

    float3 f0 = lerp(float3(0.04, 0.04, 0.04), albedo, metallic);
    float D = d_ggx(ndh, roughness);
    float G = g_smith(ndl, ndv, roughness);
    float3 F = f_schlick(vdh, f0);
    float3 specBrdf = (D * G * F) / max(4.0 * ndl * ndv, 1e-4);
    float3 kd = (1.0 - F) * (1.0 - metallic);

    float3 skyZenithGi = sample_atmosphere(float3(0.0, 1.0, 0.0), L, sunCol);
    float3 skyHorizonGi = sample_atmosphere(normalize(float3(N.x, 0.08, N.z)), L, sunCol);
    float3 hemiSky = lerp(skyHorizonGi * 0.52, skyZenithGi * 1.15, N.y * 0.5 + 0.5);
    float3 envRefl = sample_atmosphere(reflect(-V, N), L, sunCol);
    float3 fresnelEnv = f_schlick(ndv, f0) * (1.0 - roughness * 0.75);

    float3 litGlass = (kd * albedo / VOXEL_PI + specBrdf) * sunCol * ndl
                    + kd * albedo * (hemiSky * ambientStrength) * 0.55
                    + envRefl * fresnelEnv
                    + emissive;

    float4 gc = unpack_rgba_linear(rgba);
    float fresnel = pow(1.0 - max(dot(N, V), 0.0), 4.0);
    float baseAlpha = ((visual & VIS_LIQUID) != 0u) ? 0.34 : gc.a;
    float alpha = clamp(baseAlpha + fresnel * 0.38, 0.14, 0.85);
    float3 transmitted = baseComposite * lerp(float3(1.0, 1.0, 1.0), gc.rgb, 0.52);
    return lerp(transmitted, litGlass, alpha);
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

    float3 sunDir = normalize(ubo.sunDirRadiance.xyz);
    float3 sunCol = ubo.sunColorAmbient.rgb * max(0.65, ubo.sunDirRadiance.w * 2.3);
    float3 skyCol = ubo.sunColorAmbient.rgb * max(0.25, ubo.sunDirRadiance.w * 2.3);
    float ambientStrength = max(0.20, ubo.sunColorAmbient.w * 1.35);

    float3 bgColor = (forwardDepth >= 0.9995)
        ? render_procedural_sky(rayDir, sunDir, skyCol)
        : forwardColor.rgb;

    if (res.hasGlass && (!res.hit || res.glassT < res.t))
    {
        float4 glassClip = mul(ubo.viewProj, float4(rayOrigin + rayDir * res.glassT, 1.0));
        float glassDepth = glassClip.z / glassClip.w;
        if (glassDepth <= forwardDepth)
        {
            uint nId = uint(encode_normal_id(res.glassNormal));
            glassOutTex[tid.xy] = uint4(res.glassRgba, res.glassVisual, asuint(res.glassT), (1u << 8u) | nId);
        }
        else
        {
            glassOutTex[tid.xy] = uint4(0u, 0u, 0u, 0u);
        }
    }
    else
    {
        glassOutTex[tid.xy] = uint4(0u, 0u, 0u, 0u);
    }

    if (res.hit)
    {
        float4 clipPos = mul(ubo.viewProj, float4(res.worldPos, 1.0));
        float voxelDepth = clipPos.z / clipPos.w;
        if (voxelDepth <= forwardDepth)
        {
            float shadowVis = 0.04;
            if (dot(res.normal, sunDir) > 0.0)
            {
                TraceHit sh = trace_world(res.worldPos + res.normal * 0.025, sunDir, 36.0, true);
                shadowVis = sh.hit ? 0.04 : 1.0;
            }

            float time = ubo.cameraPos.w;
            float3 albedo, emissive;
            float roughness, metallic;
            uint tile;
            resolve_hit_surface(
                res.worldPos, res.normal, res.rgba, res.visual, res.tier, time,
                albedo, emissive, roughness, metallic, tile
            );

            float3 N = perturb_voxel_normal(res.worldPos, res.normal, res.visual, time, res.t, tile);
            float3 V = normalize(-rayDir);
            float3 L = sunDir;
            float3 H = normalize(L + V);

            float ndl = max(dot(N, L), 0.0);
            float ndv = max(dot(N, V), 1e-4);
            float ndh = max(dot(N, H), 0.0);
            float vdh = max(dot(V, H), 0.0);

            float3 f0 = lerp(float3(0.04, 0.04, 0.04), albedo, metallic);
            float D = d_ggx(ndh, roughness);
            float G = g_smith(ndl, ndv, roughness);
            float3 F = f_schlick(vdh, f0);
            float3 specBrdf = (D * G * F) / max(4.0 * ndl * ndv, 1e-4);
            float3 kd = (1.0 - F) * (1.0 - metallic);

            float3 sss = float3(0.0, 0.0, 0.0);
            if ((res.visual & (VIS_TRANSLUCENT | VIS_INFECTED)) != 0u)
            {
                float backScatter = pow(clamp(dot(V, -L), 0.0, 1.0), 4.0);
                sss = albedo * sunCol * (0.45 * backScatter);
            }

            float3 direct = (kd * albedo / VOXEL_PI + specBrdf) * sunCol * (ndl * shadowVis);

            float3 R = reflect(-V, N);
            float3 envRefl = sample_atmosphere(R, L, sunCol);
            float3 fresnelEnv = f_schlick(ndv, f0) * (1.0 - roughness * 0.75);
            float3 specEnv = envRefl * fresnelEnv * (0.35 + 0.65 * shadowVis);

            int curNormalId = encode_normal_id(res.normal);
            directOutTex[tid.xy]  = float4(direct + sss + emissive, voxelDepth);
            albedoOutTex[tid.xy]  = float4(kd * albedo, 1.0);
            specEnvOutTex[tid.xy] = float4(specEnv, res.t);
            posNormOutTex[tid.xy] = float4(res.worldPos, float(curNormalId));
            return;
        }
    }

    directOutTex[tid.xy]  = float4(bgColor, forwardDepth);
    albedoOutTex[tid.xy]  = float4(0.0, 0.0, 0.0, 0.0);
    specEnvOutTex[tid.xy] = float4(0.0, 0.0, 0.0, 0.0);
    posNormOutTex[tid.xy] = float4(0.0, 0.0, 0.0, -1.0);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void voxelGiMain(uint3 tid : SV_DispatchThreadID)
{
    uint width = ubo.counts.y;
    uint height = ubo.counts.z;
    if (tid.x >= width || tid.y >= height)
        return;

    int3 px = int3(int(tid.x), int(tid.y), 0);
    float4 posNorm = posNormReadTex.Load(px);
    if (posNorm.w < -0.5)
    {
        giOutTex[tid.xy] = float4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    float3 worldPos = posNorm.xyz;
    int curNormalId = int(round(posNorm.w));
    float3 normal = decode_normal_id(curNormalId);
    float hitDist = specEnvReadTex.Load(px).w;

    float3 sunDir = normalize(ubo.sunDirRadiance.xyz);
    float3 sunCol = ubo.sunColorAmbient.rgb * max(0.65, ubo.sunDirRadiance.w * 2.3);
    float ambientStrength = max(0.20, ubo.sunColorAmbient.w * 1.35);
    float time = ubo.cameraPos.w;
    float3 L = sunDir;
    float3 N = normal;

    bool hasHistory = false;
    float4 prevGi = float4(0.0, 0.0, 0.0, 1.0);
    if (ubo.counts2.z != 0u)
    {
        float4 prevClip = mul(ubo.prevViewProj, float4(worldPos, 1.0));
        if (prevClip.w > 1e-4)
        {
            float2 prevUv = (prevClip.xy / prevClip.w) * 0.5 + 0.5;
            if (all(prevUv >= float2(0.0, 0.0)) && all(prevUv < float2(1.0, 1.0)))
            {
                int2 prevPx = clamp(int2(prevUv * float2(width, height)), int2(0, 0), int2(int(width) - 1, int(height) - 1));
                float4 prevPN = prevPosNormTex.Load(int3(prevPx, 0));
                if (prevPN.w >= -0.5
                    && abs(prevPN.w - float(curNormalId)) < 0.25
                    && length(prevPN.xyz - worldPos) < 0.22)
                {
                    float4 candidate = prevGiTex.Load(int3(prevPx, 0));
                    if (all(isfinite(candidate)))
                    {
                        prevGi = candidate;
                        hasHistory = true;
                    }
                }
            }
        }
    }

    bool highRes = (width * height) > 1500000u;
    bool checkerboardSkip = highRes
        ? (((tid.x & 1u) | ((tid.y & 1u) << 1u)) != (ubo.counts2.y & 3u))
        : (((tid.x ^ tid.y ^ ubo.counts2.y) & 1u) != 0u);
    bool skipGiTrace = (hitDist < 48.0)
        && checkerboardSkip
        && (ubo.counts2.w == 0u || highRes || !hasHistory);

    float4 rawGiAo = prevGi;
    if (!skipGiTrace || !hasHistory)
    {
        float3 skyZenithGi = sample_atmosphere(float3(0.0, 1.0, 0.0), L, sunCol);
        float3 skyHorizonGi = sample_atmosphere(normalize(float3(N.x, 0.08, N.z)), L, sunCol);
        float3 hemiSky = lerp(skyHorizonGi * 0.52, skyZenithGi * 1.15, N.y * 0.5 + 0.5);
        float3 groundBounce = sunCol * (0.14 * max(L.y, 0.05) * max(-N.y * 0.5 + 0.5, 0.0));
        float directionalFacet = 0.90 + 0.10 * dot(normal, normalize(float3(0.3, 1.0, 0.2)));
        float3 analyticSky = hemiSky * (ambientStrength * directionalFacet) + groundBounce;

        if (skipGiTrace)
        {
            rawGiAo = float4(clamp(analyticSky, float3(0.0, 0.0, 0.0), float3(32.0, 32.0, 32.0)), 1.0);
        }
        else
        {
            float ao = 1.0;
            float skyVis = 1.0;
            float3 tracedGi = analyticSky;
            if (hitDist < 48.0)
            {
                trace_voxel_ao_and_gi(
                    worldPos, normal, sunDir, sunCol, ambientStrength,
                    tid.xy, time, hitDist, highRes, ao, skyVis, tracedGi
                );
                float farBlend = smoothstep(30.0, 48.0, hitDist);
                float3 outdoorFill = analyticSky * lerp(0.18 * ao, 0.45, skyVis);
                float3 fullTraced = tracedGi + outdoorFill;
                tracedGi = lerp(fullTraced, analyticSky, farBlend);
                ao = lerp(ao, 1.0, farBlend);
            }

            tracedGi = clamp(tracedGi, float3(0.0, 0.0, 0.0), float3(32.0, 32.0, 32.0));
            rawGiAo = float4(tracedGi, ao);
            if (hasHistory)
            {
                float alpha = (ubo.counts2.w != 0u) ? 0.38 : 0.22;
                rawGiAo = lerp(prevGi, rawGiAo, alpha);
            }
        }
    }

    giOutTex[tid.xy] = rawGiAo;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void voxelGiDenoiseMain(uint3 tid : SV_DispatchThreadID)
{
    uint width = denoisePc.width;
    uint height = denoisePc.height;
    if (tid.x >= width || tid.y >= height)
        return;

    int2 px = int2(tid.xy);
    float4 centerGi = giReadTex.Load(int3(px, 0));
    float4 centerPN = posNormReadTex.Load(int3(px, 0));
    if (centerPN.w < -0.5)
    {
        giOutTex[tid.xy] = centerGi;
        return;
    }

    float3 centerN = decode_normal_id(int(round(centerPN.w)));
    int step = denoisePc.stepSize;
    static const float weights[3] = { 1.0, 0.5, 0.25 };

    float4 sum = centerGi;
    float wSum = 1.0;

    for (int dy = -1; dy <= 1; dy++)
    {
        for (int dx = -1; dx <= 1; dx++)
        {
            if (dx == 0 && dy == 0)
                continue;

            int2 spx = clamp(px + int2(dx, dy) * step, int2(0, 0), int2(int(width) - 1, int(height) - 1));
            float4 samplePN = posNormReadTex.Load(int3(spx, 0));
            if (samplePN.w < -0.5 || abs(samplePN.w - centerPN.w) > 0.25)
                continue;

            float planeDist = abs(dot(samplePN.xyz - centerPN.xyz, centerN));
            float wPlane = exp(-planeDist * planeDist * 64.0);
            float wKernel = weights[abs(dx)] * weights[abs(dy)];
            float w = wKernel * wPlane;

            float4 sampleGi = giReadTex.Load(int3(spx, 0));
            if (all(isfinite(sampleGi)))
            {
                sum += sampleGi * w;
                wSum += w;
            }
        }
    }

    giOutTex[tid.xy] = sum / max(wSum, 1e-5);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void voxelCompositeMain(uint3 tid : SV_DispatchThreadID)
{
    uint width = ubo.counts.y;
    uint height = ubo.counts.z;
    if (tid.x >= width || tid.y >= height)
        return;

    int3 px = int3(int(tid.x), int(tid.y), 0);
    float4 directData = directReadTex.Load(px);
    float4 albedoData = albedoReadTex.Load(px);
    float4 specEnvData = specEnvReadTex.Load(px);
    float4 giAo = giReadTex.Load(px);
    float4 posNorm = posNormReadTex.Load(px);

    float2 uv = (float2(tid.xy) + 0.5) / float2(width, height);
    float2 ndc = uv * 2.0 - 1.0;
    float4 pNear = mul(ubo.invViewProj, float4(ndc, 0.0, 1.0));
    float4 pFar  = mul(ubo.invViewProj, float4(ndc, 1.0, 1.0));
    pNear /= pNear.w;
    pFar  /= pFar.w;
    float3 rayOrigin = pNear.xyz;
    float3 rayDelta = pFar.xyz - pNear.xyz;
    float farDist = length(rayDelta);
    float3 rayDir = (farDist > 1e-6) ? (rayDelta / farDist) : float3(0.0, 0.0, -1.0);

    float3 outColor = directData.rgb;
    float  outDepth;
    if (posNorm.w >= -0.5)
    {
        float4 vClip = mul(ubo.viewProj, float4(posNorm.xyz, 1.0));
        outDepth = vClip.z / vClip.w;
    }
    else
    {
        outDepth = forwardDepthTex.Load(px);
    }

    if (albedoData.a > 0.5)
    {
        float3 indirect = albedoData.rgb * giAo.rgb + specEnvData.rgb * giAo.a;
        outColor += indirect;

        float3 sunDir = normalize(ubo.sunDirRadiance.xyz);
        float3 sunCol = ubo.sunColorAmbient.rgb * max(0.25, ubo.sunDirRadiance.w * 2.3);
        float hitDist = specEnvData.a;
        float fog = clamp((hitDist - 45.0) / 110.0, 0.0, 0.60);
        float3 fogCol = sample_atmosphere(rayDir, sunDir, sunCol);
        outColor = lerp(outColor, fogCol, fog);
    }

    uint4 glassData = glassReadTex.Load(px);
    if ((glassData.w & 0xFF00u) != 0u)
    {
        float glassT = asfloat(glassData.z);
        float3 glassPos = rayOrigin + rayDir * glassT;
        float4 glassClip = mul(ubo.viewProj, float4(glassPos, 1.0));
        float glassDepth = glassClip.z / glassClip.w;
        if (glassDepth <= outDepth)
        {
            float3 glassNormal = decode_normal_id(int(glassData.w & 0xFFu));
            outColor = shade_translucent_glass(
                glassPos, glassNormal, glassData.x, glassData.y, -rayDir, glassT, outColor
            );
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
            float hitT = max(bEnter, 0.0);
            float3 pHit = rayOrigin + rayDir * hitT;
            float depthBias = max(0.005, hitT * 0.002);
            float3 pDepth = rayOrigin + rayDir * max(hitT - depthBias, 0.0);
            float4 bClip = mul(ubo.viewProj, float4(pDepth, 1.0));
            float boxDepth = bClip.z / bClip.w;
            if (boxDepth <= outDepth)
            {
                float3 extents = max(boxMax - boxMin, float3(0.01, 0.01, 0.01));
                float minExt = min(extents.x, min(extents.y, extents.z));
                float3 dWorld = min(pHit - boxMin, boxMax - pHit);
                float dMin = min(dWorld.x, min(dWorld.y, dWorld.z));
                float dMax = max(dWorld.x, max(dWorld.y, dWorld.z));
                float d2 = (dWorld.x + dWorld.y + dWorld.z) - dMin - dMax;

                // Screen-adaptive hairline width with smoothstep anti-aliasing.
                float pxWorld = max(hitT, 0.1) * (2.0 / max(float(height), 1.0));
                float halfW = clamp(pxWorld * 1.35, 0.0025, minExt * 0.10);
                float aa = max(pxWorld * 0.75, 0.001);
                float edgeCov = 1.0 - smoothstep(halfW - aa, halfW + aa, d2);
                if (edgeCov > 0.001)
                {
                    outColor = lerp(outColor, ubo.cursorColor.rgb * 1.4, ubo.cursorColor.a * edgeCov);
                }
            }
        }
    }

    outColorTex[tid.xy] = float4(outColor, 1.0);
    outDepthTex[tid.xy] = outDepth;
}
