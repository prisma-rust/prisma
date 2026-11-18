// GPU compute shaders for runtime IBL baking from an equirectangular HDR texture.
//
// Used when loose `.hdr` assets or procedural skies are loaded at runtime (editor,
// `PRISMA_LOOSE_ASSETS=1`, or tests), replacing slow CPU Monte Carlo loops with
// sub-millisecond/millisecond GPU dispatches. Offline `.pak` builds still use
// `prism-gfx` CPU baking when no GPU is present.
//
// All four entry points share one descriptor set (set 0) so a single pipeline
// layout covers every dispatch:
//   binding 0 - source equirectangular HDR texture (COMBINED_IMAGE_SAMPLER)
//   binding 1 - output cubemap array layer view (STORAGE_IMAGE, rgba16f, 6 layers)
//   binding 2 - output 2D BRDF LUT (STORAGE_IMAGE, rg16f)

static const float PI = 3.14159265359;

struct IblBakePush
{
    uint face_size;
    float roughness;
    uint sample_count;
    uint _pad;
};

[[vk::push_constant]]
IblBakePush pc;

[[vk::binding(0, 0)]]
Sampler2D inEquirect;

[[vk::binding(1, 0)]]
[vk::image_format("rgba16f")]
RWTexture2DArray<float4> outCube;

[[vk::binding(2, 0)]]
[vk::image_format("rg16f")]
RWTexture2D<float2> outBrdf;

float3 cubeDirection(uint face, float u, float v)
{
    switch (face)
    {
        case 0u: return float3( 1.0, -v, -u); // +X
        case 1u: return float3(-1.0, -v,  u); // -X
        case 2u: return float3(   u,  1.0,  v); // +Y
        case 3u: return float3(   u, -1.0, -v); // -Y
        case 4u: return float3(   u, -v,  1.0); // +Z
        default: return float3(  -u, -v, -1.0); // -Z
    }
}

float2 dirToEquirectUV(float3 d)
{
    float u = atan2(d.z, d.x) / (2.0 * PI) + 0.5;
    float v = acos(clamp(d.y, -1.0, 1.0)) / PI;
    return float2(u, v);
}

void buildTangentFrame(float3 n, out float3 t, out float3 b)
{
    float3 up = (abs(n.y) < 0.9999) ? float3(0.0, 1.0, 0.0) : float3(0.0, 0.0, 1.0);
    t = normalize(cross(up, n));
    b = cross(n, t);
}

float radicalInverseVdc(uint bits)
{
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return float(bits) * 2.3283064365386963e-10;
}

float2 hammersley(uint i, uint n)
{
    return float2(float(i) / float(n), radicalInverseVdc(i));
}

float3 importanceSampleGGX(float2 xi, float roughness)
{
    float a = roughness * roughness;
    float a2 = a * a;
    float phi = 2.0 * PI * xi.x;
    float cosTheta = sqrt((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y));
    float sinTheta = sqrt(max(0.0, 1.0 - cosTheta * cosTheta));
    return float3(sinTheta * cos(phi), sinTheta * sin(phi), cosTheta);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void equirectToCubeMain(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= pc.face_size || tid.y >= pc.face_size || tid.z >= 6u)
        return;

    float u = 2.0 * (float(tid.x) + 0.5) / float(pc.face_size) - 1.0;
    float v = 2.0 * (float(tid.y) + 0.5) / float(pc.face_size) - 1.0;
    float3 d = normalize(cubeDirection(tid.z, u, v));
    float4 c = inEquirect.SampleLevel(dirToEquirectUV(d), 0.0);
    outCube[tid] = float4(c.rgb, 1.0);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void irradianceMain(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= pc.face_size || tid.y >= pc.face_size || tid.z >= 6u)
        return;

    float u = 2.0 * (float(tid.x) + 0.5) / float(pc.face_size) - 1.0;
    float v = 2.0 * (float(tid.y) + 0.5) / float(pc.face_size) - 1.0;
    float3 n = normalize(cubeDirection(tid.z, u, v));
    float3 t, b;
    buildTangentFrame(n, t, b);

    uint steps = max(1u, uint(sqrt(float(pc.sample_count))));
    float invSteps = 1.0 / float(steps);
    float3 sum = float3(0.0, 0.0, 0.0);
    float totalWeight = 0.0;

    for (uint ti = 0u; ti < steps; ++ti)
    {
        float theta = (float(ti) + 0.5) * invSteps * (PI * 0.5);
        float sinTheta = sin(theta);
        float cosTheta = cos(theta);
        for (uint pj = 0u; pj < steps; ++pj)
        {
            float phi = (float(pj) + 0.5) * invSteps * (2.0 * PI);
            float3 lLocal = float3(sinTheta * cos(phi), sinTheta * sin(phi), cosTheta);
            float3 l = normalize(t * lLocal.x + b * lLocal.y + n * lLocal.z);
            float weight = max(dot(n, l), 0.0);
            if (weight > 0.0)
            {
                float3 c = inEquirect.SampleLevel(dirToEquirectUV(l), 0.0).rgb;
                sum += c * weight;
                totalWeight += weight;
            }
        }
    }

    float3 result = (totalWeight > 0.0) ? (sum / totalWeight) : float3(0.0, 0.0, 0.0);
    outCube[tid] = float4(result, 1.0);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void prefilterMain(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= pc.face_size || tid.y >= pc.face_size || tid.z >= 6u)
        return;

    float u = 2.0 * (float(tid.x) + 0.5) / float(pc.face_size) - 1.0;
    float v = 2.0 * (float(tid.y) + 0.5) / float(pc.face_size) - 1.0;
    float3 n = normalize(cubeDirection(tid.z, u, v));
    float3 t, b;
    buildTangentFrame(n, t, b);

    float3 sum = float3(0.0, 0.0, 0.0);
    float count = 0.0;

    for (uint i = 0u; i < pc.sample_count; ++i)
    {
        float2 xi = hammersley(i, pc.sample_count);
        float3 hLocal = importanceSampleGGX(xi, pc.roughness);
        float vDotH = hLocal.z;
        float3 lLocal = float3(
            2.0 * vDotH * hLocal.x,
            2.0 * vDotH * hLocal.y,
            2.0 * vDotH * hLocal.z - 1.0
        );
        if (lLocal.z <= 0.0)
            continue;

        float3 l = normalize(t * lLocal.x + b * lLocal.y + n * lLocal.z);
        sum += inEquirect.SampleLevel(dirToEquirectUV(l), 0.0).rgb;
        count += 1.0;
    }

    float3 result = (count > 0.0) ? (sum / count) : float3(0.0, 0.0, 0.0);
    outCube[tid] = float4(result, 1.0);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void brdfLutMain(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= pc.face_size || tid.y >= pc.face_size)
        return;

    float invSize = 1.0 / float(pc.face_size);
    float nDotV = (float(tid.x) + 0.5) * invSize;
    float roughness = (float(tid.y) + 0.5) * invSize;
    float3 v = float3(sqrt(max(0.0, 1.0 - nDotV * nDotV)), 0.0, nDotV);

    float scale = 0.0;
    float bias = 0.0;

    for (uint i = 0u; i < pc.sample_count; ++i)
    {
        float2 xi = hammersley(i, pc.sample_count);
        float3 h = importanceSampleGGX(xi, roughness);
        float vDotH = dot(v, h);
        float3 l = 2.0 * vDotH * h - v;

        float nDotL = max(l.z, 0.0);
        float nDotH = max(h.z, 0.0);
        float vDotHPos = max(vDotH, 0.0);

        if (nDotL > 0.0)
        {
            float k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
            float gV = nDotV / (nDotV * (1.0 - k) + k);
            float gL = nDotL / (nDotL * (1.0 - k) + k);
            float g = gV * gL;
            float gVis = g * vDotHPos / max(nDotH * nDotV, 1e-6);
            float fc = pow(1.0 - vDotHPos, 5.0);
            scale += (1.0 - fc) * gVis;
            bias += fc * gVis;
        }
    }

    float invN = 1.0 / float(pc.sample_count);
    outBrdf[tid.xy] = float2(scale * invN, bias * invN);
}
