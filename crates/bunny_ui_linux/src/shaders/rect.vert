// The rect instance quad — Vulkan dialect of the certified GL vertex.
// NDC y points DOWN here, the raster's own direction: no flip anywhere.
// The committed .spv beside this file is its baked twin; a gated test
// recompiles and byte-compares when the compiler is on the box.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec2 viewport;
    float round_radius;
    float pad;
} pc;

layout(location = 0) in vec4 a_rect;
layout(location = 1) in vec4 a_clip;
layout(location = 2) in vec4 a_params;
layout(location = 3) in vec4 a_color;
layout(location = 4) in vec4 a_color2;
layout(location = 5) in vec2 a_point2;

layout(location = 0) flat out vec4 v_rect;
layout(location = 1) flat out vec4 v_params;
layout(location = 2) flat out vec4 v_color;
layout(location = 3) flat out vec4 v_color2;
layout(location = 4) flat out vec2 v_point2;

vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    // the clip cuts the QUAD, not the coverage — the CPU's integer clip
    vec2 low = max(a_rect.xy, a_clip.xy);
    vec2 high = max(min(a_rect.zw, a_clip.zw), low);
    vec2 corner = unit_corners[gl_VertexIndex];
    vec2 unit = mix(low, high, corner) / pc.viewport;
    gl_Position = vec4(unit * 2.0 - 1.0, 0.0, 1.0);
    v_rect = a_rect;
    v_params = a_params;
    v_color = a_color;
    v_color2 = a_color2;
    v_point2 = a_point2;
}
