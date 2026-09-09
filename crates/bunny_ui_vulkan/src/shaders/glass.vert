// The pane quad — the rect vertex with the material's instance instead
// of the rect's. NDC y points DOWN here, the raster's own direction: no
// flip anywhere.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

layout(location = 0) in vec4 a_rect;
layout(location = 1) in vec4 a_clip;
layout(location = 2) in vec4 a_radii;
layout(location = 3) in vec4 a_lens;
layout(location = 4) in vec4 a_finish;
layout(location = 5) in vec4 a_touch;
layout(location = 6) in vec4 a_tint;
layout(location = 7) in vec4 a_highlight;
layout(location = 8) in vec2 a_spot;

layout(location = 0) flat out vec4 v_rect;
layout(location = 1) flat out vec4 v_radii;
layout(location = 2) flat out vec4 v_lens;
layout(location = 3) flat out vec4 v_finish;
layout(location = 4) flat out vec4 v_touch;
layout(location = 5) flat out vec4 v_tint;
layout(location = 6) flat out vec4 v_highlight;
layout(location = 7) flat out float v_spot_alpha;

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
    v_radii = a_radii;
    v_lens = a_lens;
    v_finish = a_finish;
    v_touch = a_touch;
    v_tint = a_tint;
    v_highlight = a_highlight;
    v_spot_alpha = a_spot.x;
}
