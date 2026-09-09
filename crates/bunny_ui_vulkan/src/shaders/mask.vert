// The scene-corner mask quad: one corner square from the push range.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    vec2 corner = unit_corners[gl_VertexIndex];
    vec2 unit = mix(pc.quad.xy, pc.quad.zw, corner) / pc.viewport;
    gl_Position = vec4(unit * 2.0 - 1.0, 0.0, 1.0);
}
