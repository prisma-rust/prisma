// Backdrop filter for the UI overlay.
//
// The host copies the composited scene into an offscreen image, then runs two
// fullscreen passes of this shader (horizontal, vertical): a separable Gaussian
// over that copy. The UI shader samples the result wherever a surface's
// material asks for a backdrop, which is how frosted glass works without the
// UI pipeline knowing anything about blur.
//
// One pipeline, two passes: the direction is a push constant, so both passes
// share bytecode and the same target format as the swapchain it was copied
// from. Writing through a color attachment (rather than a storage image) keeps
// the pass independent of storage-image format support.
//
// Sets (must match `UiBackdropPass::write_descriptors`):
//   set 0, binding 0 - source colour (combined image sampler)
//
// Compile:
//   slangc ui_backdrop.slang -profile spirv_1_4 -target spirv \
//          -entry vertexMain   -stage vertex   -fvk-use-entrypoint-name \
//          -o ui_backdrop.vert.spv
//   slangc ui_backdrop.slang -profile spirv_1_4 -target spirv \
//          -entry fragmentMain -stage fragment -fvk-use-entrypoint-name \
//          -o ui_backdrop.frag.spv

[[vk::binding(0, 0)]]
Sampler2D srcTex;

// Tightly packed; mirrors the Rust `UiBackdropPush` field for field.
public struct UiBackdropPush
{
    public float2 texel_step;  // source-texel step for this pass, in UV
    public float  sigma;       // Gaussian sigma in pixels
    public uint   taps;        // samples per side (1..MAX_TAPS)
};

[[vk::push_constant]]
UiBackdropPush pc;

// A 12-tap-per-side kernel covers sigma <= 64 px well enough for UI glass;
// beyond that the weights are so flat that extra taps buy nothing visible.
static const uint MAX_TAPS = 12u;

struct VsOut
{
    float4 clipPos : SV_Position;
    float2 uv      : TEXCOORD0;
};

[shader("vertex")]
VsOut vertexMain(uint vid : SV_VertexID)
{
    // Fullscreen triangle: covers NDC [-1,1] in two corners and overshoots the
    // third, so no vertex buffer is needed.
    float2 ndc = float2(
        (vid == 0u) ? -1.0 : (vid == 1u) ? 3.0 : -1.0,
        (vid == 0u) ? -1.0 : (vid == 1u) ? -1.0 : 3.0
    );
    VsOut out;
    out.clipPos = float4(ndc, 0.0, 1.0);
    // Vulkan NDC y = -1 is the top of the screen and the rendered image is
    // written top-down, so UV.y follows NDC without a flip.
    out.uv = ndc * float2(0.5, 0.5) + float2(0.5, 0.5);
    return out;
}

[shader("fragment")]
float4 fragmentMain(VsOut input) : SV_Target0
{
    float2 halfPixel = pc.texel_step * 0.5;
    if (pc.taps == 0u)
    {
        // Dual-Kawase downsample (5-tap): center weighted 4x + 4 diagonal corners.
        float4 sum = srcTex.SampleLevel(input.uv, 0.0) * 4.0;
        sum += srcTex.SampleLevel(input.uv - halfPixel, 0.0);
        sum += srcTex.SampleLevel(input.uv + halfPixel, 0.0);
        sum += srcTex.SampleLevel(input.uv + float2( halfPixel.x, -halfPixel.y), 0.0);
        sum += srcTex.SampleLevel(input.uv + float2(-halfPixel.x,  halfPixel.y), 0.0);
        return sum / 8.0;
    }
    else
    {
        // Dual-Kawase upsample (8-tap tent): 85% weight on the wider blurred mip
        // eliminates two-step halos on high-contrast edges while keeping Mip 0..3 distinct.
        float4 sum = srcTex.SampleLevel(input.uv + float2(-pc.texel_step.x, 0.0), 0.0);
        sum += srcTex.SampleLevel(input.uv + float2( pc.texel_step.x, 0.0), 0.0);
        sum += srcTex.SampleLevel(input.uv + float2(0.0, -pc.texel_step.y), 0.0);
        sum += srcTex.SampleLevel(input.uv + float2(0.0,  pc.texel_step.y), 0.0);
        sum += srcTex.SampleLevel(input.uv + float2(-halfPixel.x, -halfPixel.y), 0.0) * 2.0;
        sum += srcTex.SampleLevel(input.uv + float2( halfPixel.x, -halfPixel.y), 0.0) * 2.0;
        sum += srcTex.SampleLevel(input.uv + float2(-halfPixel.x,  halfPixel.y), 0.0) * 2.0;
        sum += srcTex.SampleLevel(input.uv + float2( halfPixel.x,  halfPixel.y), 0.0) * 2.0;
        return float4(sum.rgb / 12.0, 0.85);
    }
}
