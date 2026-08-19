// The 1:1 texel copy — texelFetch, no sampler math, exact bytes.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) flat in vec4 v_dest;
layout(location = 1) flat in vec4 v_tex;

layout(location = 0) out vec4 out_color;

// which of the four a pixel answers to: the box's own midpoint splits
// it in quarters, and a pixel far from every corner reads the same
// coverage whichever radius it picked — a straight edge does not
// depend on it
float corner_at(vec2 p, vec4 rect, vec4 radii) {
    vec2 mid = (rect.xy + rect.zw) * 0.5;
    return p.x < mid.x ? (p.y < mid.y ? radii.x : radii.w)
                       : (p.y < mid.y ? radii.y : radii.z);
}

float rect_sdf(vec2 p, vec4 rect, vec4 radii) {
    float radius = corner_at(p, rect, radii);
    vec2 shifted = max(rect.xy + radius - p, p - (rect.zw - radius));
    float outside = length(max(shifted, vec2(0.0)));
    float inside = min(max(shifted.x, shifted.y), 0.0);
    return outside + inside - radius;
}

float rect_cov(vec2 p, vec4 rect, vec4 radii) {
    return clamp(0.5 - rect_sdf(p, rect, radii), 0.0, 1.0);
}

float clip_cov(vec2 p) {
    return any(greaterThan(pc.round_radii, vec4(0.0)))
        ? rect_cov(p, pc.round_box, pc.round_radii)
        : 1.0;
}

void main() {
    vec2 p = gl_FragCoord.xy;
    vec2 texel = v_tex.xy + (floor(p) - floor(v_dest.xy));
    vec4 ink = texelFetch(atlas, ivec2(texel), 0);
    out_color = vec4(ink.rgb, ink.a * clip_cov(p));
}
