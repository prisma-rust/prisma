// UI overlay — screen-space colored quads + textured glyph quads drawn on
// top of the final scene.
//
// Two quad kinds share one pipeline:
//   - plain colored fills (backgrounds): uv = (0,0) → samples the atlas
//     reserve pixel, which is white, so alpha = 1 and the vertex color
//     passes through;
//   - glyph quads (text): uv spans the glyph's rect in the R8 glyph atlas
//     (fontdue-rasterized); the sampled alpha modulates the vertex color.
//
// A third quad kind samples the filtered backdrop: when a surface's material
// asks for a backdrop, the host blurs the scene into an image and the
// fragment shader composites that instead of a flat fill. The vertex carries
// the blur radius; zero means "do not sample the backdrop at all".
//
// Sets (must match `UiOverlay::create_pipeline`):
//   set 0, binding 0 - font glyph atlas (R8 UNORM, combined image sampler)
//   set 0, binding 1 - filtered backdrop (combined image sampler)
//   set 1, binding 0 - bindless samplers (`globalSamplers[4]`, shared table)
//   set 1, binding 1 - bindless SRVs (`bindlessSrvs[]`, shared table)
//
// Background images (#591) reuse the scene's bindless texture table, so an
// icon costs one vertex attribute instead of a second pipeline.
//
// Compile:
//   slangc ui_overlay.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -stage vertex   -fvk-use-entrypoint-name -o ui_overlay.vert.spv
//   slangc ui_overlay.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o ui_overlay.frag.spv

// ---- set 0, binding 0 - glyph atlas (combined image+sampler). Pixel (0,0)
// is reserved as white so untextured quads (uv = 0) sample 1.0.
[[vk::binding(0, 0)]]
Sampler2D glyphAtlas;

// Filtered scene colour, produced by the backdrop pass. Bound even when no
// surface asks for it; the shader only samples it for backdrop quads.
[[vk::binding(1, 0)]]
Sampler2D backdropTex;

[[vk::binding(0, 1)]]
SamplerState globalSamplers[];

[[vk::binding(1, 1)]]
Texture2D bindlessSrvs[];

// Sampler slot order is fixed by `bindless::SamplerType`: 1 = linear clamp, 2 = nearest.
static const uint SAMPLER_LINEAR_CLAMP = 1u;
static const uint SAMPLER_NEAREST = 2u;
// Vertex-side sentinel: sample the glyph atlas instead of a bindless SRV.
static const uint ATLAS_TEXTURE = 0xFFFFFFFFu;

struct UiVertexIn
{
    float2 position     : POSITION;   // location 0 — NDC position
    float2 uv           : TEXCOORD0;  // location 1 — atlas uv (0,0 for plain quads)
    float4 color        : COLOR0;     // location 2 — RGBA color
    float2 local        : TEXCOORD1;  // location 3 — pixel offset from quad centre
    float2 halfSize     : TEXCOORD2;  // location 4 — quad half size in pixels
    float  radius       : TEXCOORD3;  // location 5 — corner radius (>=0), -1=bitmap, <=-2=SDF+outline
    uint   texture      : TEXCOORD4;  // location 6 — bindless slot (0xFFFFFFFF = atlas)
    float  backdrop     : TEXCOORD5;  // location 7 — backdrop sigma in pixels (0 = none)
    float4 outlineColor : COLOR1;     // location 8 — SDF outline RGBA (unpacked UNORM)
};

struct UiVaryings
{
    float4 clipPos      : SV_Position;
    float2 uv           : TEXCOORD0;
    float4 color        : COLOR0;
    float2 local        : TEXCOORD1;
    float2 halfSize     : TEXCOORD2;
    float  radius       : TEXCOORD3;
    uint   texture      : TEXCOORD4;
    float  backdrop     : TEXCOORD5;
    float2 screenUv     : TEXCOORD6;
    float4 outlineColor : COLOR1;
};

[shader("vertex")]
UiVaryings vertexMain(UiVertexIn in)
{
    UiVaryings out;
    out.clipPos      = float4(in.position, 0.0, 1.0);
    out.uv           = in.uv;
    out.color        = in.color;
    out.local        = in.local;
    out.halfSize     = in.halfSize;
    out.radius       = in.radius;
    out.texture      = in.texture;
    out.backdrop     = in.backdrop;
    out.outlineColor = in.outlineColor;
    // NDC -> screen UV for backdrop sampling. Vulkan NDC y = -1 is the top of
    // the screen, and the blurred image is written top-down, so y is not
    // flipped here.
    out.screenUv = in.position * float2(0.5, 0.5) + float2(0.5, 0.5);
    return out;
}

// Signed distance to a rounded box centred at the origin (pixel space).
float sdRoundBox(float2 p, float2 b, float r)
{
    float2 q = abs(p) - b + r;
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

[shader("fragment")]
float4 fragmentMain(UiVaryings in) : SV_Target0
{
    // Two texture paths share one pipeline:
    //   - glyph / plain quads sample the atlas (plain quads hit the reserved
    //     white pixel, so coverage = 1);
    //   - background images sample the bindless table and tint with the
    //     vertex colour (#591).
    if (in.texture == ATLAS_TEXTURE && in.radius <= -2.0)
    {
        // TextMeshPro-style SDF glyph rendering (0.5 = glyph contour).
        float sdf = glyphAtlas.Sample(in.uv).r;
        float grad = length(float2(ddx(sdf), ddy(sdf)));
        float aa = clamp(grad * 0.75, 0.02, 0.35);
        float outlineW = clamp(-(in.radius + 2.0), 0.0, 0.42);
        float faceAlpha = smoothstep(0.5 - aa, 0.5 + aa, sdf);
        if (outlineW > 0.001)
        {
            float outerAlpha = smoothstep(0.5 - outlineW - aa, 0.5 - outlineW + aa, sdf);
            float3 rgb = lerp(in.outlineColor.rgb, in.color.rgb, faceAlpha);
            float alpha = max(faceAlpha * in.color.a, outerAlpha * in.outlineColor.a);
            return float4(rgb, alpha);
        }
        return float4(in.color.rgb, in.color.a * faceAlpha);
    }

    float4 sampled = float4(1.0, 1.0, 1.0, 1.0);
    if (in.texture == ATLAS_TEXTURE)
        sampled = float4(1.0, 1.0, 1.0, glyphAtlas.Sample(in.uv).r);
    else
    {
        // Slot 0 is the 8x8 fallback checkerboard; sample nearest so its squares stay crisp.
        uint samplerIdx = (in.texture == 0u) ? SAMPLER_NEAREST : SAMPLER_LINEAR_CLAMP;
        sampled = bindlessSrvs[NonUniformResourceIndex(in.texture)]
            .SampleLevel(globalSamplers[NonUniformResourceIndex(samplerIdx)], in.uv, 0.0);
    }

    float coverage = sampled.a;

    // Rounded rectangle (#634): glyph quads pass radius < 0 and keep the
    // plain glyph-atlas coverage. Plain quads get an SDF edge in pixel space,
    // antialiased by one pixel.
    if (in.radius >= 0.0)
    {
        float d = sdRoundBox(in.local, in.halfSize, in.radius);
        coverage *= 1.0 - smoothstep(-1.0, 1.0, d);
    }

    // Backdrop surfaces replace the destination rather than tint it: the
    // destination still holds the *unfiltered* scene, so blending the tint over
    // it would just be a translucent rectangle. Mixing the filtered colour with
    // the tint and writing opaque rgb (coverage on alpha) is what makes the
    // surface read as glass.
    if (in.backdrop > 0.0)
    {
        // Map blur sigma in pixels to mip level in the 1/2..1/16 blur pyramid.
        float lod = clamp(log2(max(in.backdrop, 1.0) * 0.125), 0.0, 3.0);
        float3 blurred = backdropTex.SampleLevel(in.screenUv, lod).rgb;
        // Subtle vibrancy boost so dark/light glass keeps colour richness.
        float luma = dot(blurred, float3(0.2126, 0.7152, 0.0722));
        blurred = max(lerp(float3(luma, luma, luma), blurred, 1.25), 0.0);
        float3 glass = lerp(blurred, in.color.rgb * sampled.rgb, in.color.a);
        return float4(glass, coverage);
    }

    // Straight alpha: the pipeline blends with SRC_ALPHA/ONE_MINUS_SRC_ALPHA,
    // so coverage belongs on alpha only. Multiplying rgb here as well would
    // apply it twice — the classic dark fringe around antialiased text.
    return float4(in.color.rgb * sampled.rgb, in.color.a * coverage);
}
