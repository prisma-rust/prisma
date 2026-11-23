// Custom material fragment shader for Sponza (loaded at runtime via ShaderAsset).
// Follows ForwardPass's set 0..1 + PbrBindlessPush + SceneOutput contract.
import common;

[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

struct GpuMaterial
{
    float4 base_color;
    float4 metallic_roughness_emissive;
    uint   albedo_idx;
    uint   normal_idx;
    uint   metallic_roughness_idx;
    uint   emissive_idx;
    float4 transmission_factor;
    float4 clearcoat;
    uint   transmission_tex_idx;
    uint   occlusion_idx;
    float  normal_scale;
    float  occlusion_strength;
    float  alpha_cutoff;
    uint   alpha_mode;
    float2 alpha_pad;
};
[[vk::binding(1, 0)]]
StructuredBuffer<GpuMaterial> materials;

[[vk::binding(0, 1)]]
SamplerState globalSamplers[];

[[vk::binding(1, 1)]]
Texture2D bindlessSrvs[];

struct PbrBindlessPush
{
    float4x4 model;
    uint     material_slot;
    uint     env_handle;
    uint     debug_flags;
    uint     bone_offset;
    uint     skin_mode;
    uint     instance_offset;
    uint     instance_count;
    uint     prev_draw_index;
    uint     reactive_mask;
    uint     splat_slot0;
    uint     splat_slot1;
    uint     splat_slot2;
    uint     splat_slot3;
    float    splat_tiling;
    uint     gi_mode;
    float    prt_leak_bias;
};
[[vk::push_constant]]
PbrBindlessPush pc;

struct SceneOutput
{
    float4 color           : SV_Target0;
    float4 viewNormal      : SV_Target1;
    float2 velocity        : SV_Target2;
    float4 albedoRoughness : SV_Target3;
};

[shader("fragment")]
SceneOutput fragmentMain(MeshVaryings in)
{
    GpuMaterial mat = materials[pc.material_slot];

    float4 tex_albedo = float4(1.0, 1.0, 1.0, 1.0);
    if (mat.albedo_idx != 0u)
    {
        tex_albedo = bindlessSrvs[NonUniformResourceIndex(mat.albedo_idx)]
            .Sample(globalSamplers[0], in.uv);
    }

    float4 base = mat.base_color * tex_albedo;
    if (mat.alpha_mode == 1u && base.a < mat.alpha_cutoff)
    {
        discard;
    }

    float3 N = normalize(in.worldNormal);
    if (mat.normal_idx != 0u && dot(in.tangent.xyz, in.tangent.xyz) > 1e-6)
    {
        float3 T = normalize(in.tangent.xyz - N * dot(N, in.tangent.xyz));
        float3 B = cross(N, T) * in.tangent.w;
        float3 n_ts = bindlessSrvs[NonUniformResourceIndex(mat.normal_idx)]
            .Sample(globalSamplers[0], in.uv).xyz * 2.0 - 1.0;
        n_ts.xy *= mat.normal_scale;
        N = normalize(T * n_ts.x + B * n_ts.y + N * n_ts.z);
    }

    float3 V = normalize(frame.cameraPosition.xyz - in.worldPos);
    float3 L = normalize(frame.lightDirection.xyz);
    float sun_rad = max(frame.lightDirection.w, 0.0);

    // Stylized 2-band architectural shading + cool hemisphere fill + warm rim.
    float ndotl = dot(N, L);
    float band = smoothstep(0.02, 0.08, ndotl);
    float3 sun_lit = frame.lightColor.rgb * sun_rad * (0.25 + 0.75 * band);
    float3 sky_fill = lerp(float3(0.09, 0.11, 0.16), float3(0.22, 0.28, 0.38), N.y * 0.5 + 0.5);
    float rim = pow(1.0 - saturate(dot(N, V)), 3.0) * saturate(ndotl * 0.5 + 0.5);
    float3 rim_col = frame.lightColor.rgb * sun_rad * 0.35 * rim;

    float3 lit = base.rgb * (sun_lit + sky_fill) + rim_col;

    float3 view_n = normalize(mul((float3x3)frame.view, N));

    SceneOutput out;
    out.color = float4(lit, base.a);
    out.viewNormal = float4(view_n, float(pc.reactive_mask));
    out.velocity = in.velocity;
    out.albedoRoughness = float4(base.rgb, mat.metallic_roughness_emissive.y);
    return out;
}
