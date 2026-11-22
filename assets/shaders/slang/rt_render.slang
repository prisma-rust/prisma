// rt_render.slang — offscreen shader outputting to RenderTexture (engine-style RenderTexture demo).
//
// Render target and shader decoupled: this shader only writes color to color attachment 0;
// it doesn't know whether the final target is a RenderTexture or swapchain — the target is decided
// by RenderToTexturePass (binding the RT's image view as framebuffer attachment).
//
// Content: expands a 16-bit pattern bit by bit into a 4x4 grid (bitmap pattern).
//   - UV ∈ [0,1] split into 4x4 cells, cell = int(uv * 4)
//   - bit index = row * 4 + col (bit0 = top-left)
//   - bit = 1 → white, bit = 0 → black
// 16-bit number randomly generated per frame on CPU side (inside RenderToTexturePass via xorshift),
// passed in via push constant.

struct RtRenderPush
{
    uint pattern; // low 16 bits valid: 4x4 bitmap (bit i = cell at row i/4, col i%4)
};
[[vk::push_constant]] RtRenderPush pc;

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

// Fullscreen triangle (3 vertices, no vertex buffer), UV from (0,0) to (2,2) covering the viewport.
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
    int2 cell = int2(i.uv * 4.0);
    int idx = cell.y * 4 + cell.x;
    uint bit = (pc.pattern >> uint(idx)) & 1u;
    return float4(float(bit), float(bit), float(bit), 1.0);
}
