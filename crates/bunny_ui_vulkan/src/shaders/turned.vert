// A picture under an affine map: the texel box one texel wider all
// round, mapped — the fade past the picture's edge lands inside the quad.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

layout(location = 0) in vec4 a_axes;
layout(location = 1) in vec4 a_origin;
layout(location = 2) in vec4 a_tex;
layout(location = 3) in vec4 a_clip;

layout(location = 0) flat out vec4 v_axes;
layout(location = 1) flat out vec4 v_origin;
layout(location = 2) flat out vec4 v_tex;
layout(location = 3) flat out vec4 v_clip;

vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    vec2 texel = mix(a_tex.xy - 1.0, a_tex.zw + 1.0, unit_corners[gl_VertexIndex]) - a_tex.xy;
    vec2 p = a_origin.xy + a_axes.xy * texel.x + a_axes.zw * texel.y;
    vec2 unit = p / pc.viewport;
    gl_Position = vec4(unit * 2.0 - 1.0, 0.0, 1.0);
    v_axes = a_axes;
    v_origin = a_origin;
    v_tex = a_tex;
    v_clip = a_clip;
}
