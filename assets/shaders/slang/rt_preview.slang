// rt_preview.slang — consumer example: fullscreen sampling of RenderTexture (bindless).
//
// Demonstrates engine-style RT consumption: RenderTexture is registered into the bindless SRV table
// at creation time, obtaining a TextureHandle; this shader receives the handle via push constant,
// and samples directly from bindlessSrvs[] — no different from sampling any ordinary texture.

// ---- set 1, binding 0 - bindless SAMPLER array (4 modes) ------------
SamplerState globalSamplers[];

// ---- set 1, binding 1 - bindless SAMPLED_IMAGE array -----------------
Texture2D bindlessSrvs[];

struct RtPreviewPush
{
    uint textureHandle; // RenderTexture's bindless slot
};
[[vk::push_constant]] RtPreviewPush pc;

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

[shader("vertex")]
VsOut vertexMain(uint vid : SV_VertexID)
{
    VsOut o;
    float2 uv = float2(float((vid << 1) & 2), float(vid & 2));
    o.pos = float4(uv * 2.0 - 1.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

[shader("fragment")]
float4 fragmentMain(VsOut i) : SV_Target
{
    // Linear sampling + wrap (SamplerType::LinearWrap = slot 0), RT content scaled to fullscreen.
    return bindlessSrvs[pc.textureHandle].SampleLevel(globalSamplers[0], i.uv, 0.0);
}
