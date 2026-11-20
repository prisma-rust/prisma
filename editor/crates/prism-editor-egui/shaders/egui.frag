#version 450

layout(set = 0, binding = 0) uniform sampler2D u_sampler;

layout(location = 0) in vec4 v_rgba;
layout(location = 1) in vec2 v_uv;

layout(location = 0) out vec4 f_color;

void main() {
    vec4 texel = texture(u_sampler, v_uv);
    texel.rgb *= texel.a;
    f_color = v_rgba * texel;
}
