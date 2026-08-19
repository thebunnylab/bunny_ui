// The sprite (text/image tile) quad — Vulkan dialect, y-down NDC.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

layout(location = 0) in vec4 a_dest;
layout(location = 1) in vec4 a_tex;
layout(location = 2) in vec4 a_clip;

layout(location = 0) flat out vec4 v_dest;
layout(location = 1) flat out vec4 v_tex;

vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    vec2 low = max(a_dest.xy, a_clip.xy);
    vec2 high = max(min(a_dest.zw, a_clip.zw), low);
    vec2 corner = unit_corners[gl_VertexIndex];
    vec2 unit = mix(low, high, corner) / pc.viewport;
    gl_Position = vec4(unit * 2.0 - 1.0, 0.0, 1.0);
    v_dest = a_dest;
    v_tex = a_tex;
}
