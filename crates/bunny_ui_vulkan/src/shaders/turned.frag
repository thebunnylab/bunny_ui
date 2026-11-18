// The pixel's centre mapped back onto the picture, and the four texels
// around it summed premultiplied — a texel past the picture's edge is
// clear, so the turned edge fades over one texel (`composite_affine`).
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

layout(set = 0, binding = 0) uniform sampler2D atlas;

layout(location = 0) flat in vec4 v_axes;
layout(location = 1) flat in vec4 v_origin;
layout(location = 2) flat in vec4 v_tex;
layout(location = 3) flat in vec4 v_clip;

layout(location = 0) out vec4 out_color;

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
    if (any(lessThan(p, v_clip.xy)) || any(greaterThanEqual(p, v_clip.zw))) {
        discard;
    }
    vec4 m = v_axes;
    vec2 d = p - v_origin.xy;
    vec2 local = vec2(m.w * d.x - m.z * d.y, m.x * d.y - m.y * d.x) / (m.x * m.w - m.z * m.y);
    vec2 at = v_tex.xy + local - 0.5;
    vec2 low = floor(at);
    vec2 f = at - low;
    vec4 sum = vec4(0.0);
    for (int dy = 0; dy < 2; dy++) {
        for (int dx = 0; dx < 2; dx++) {
            vec2 texel = low + vec2(dx, dy);
            float share = (dx == 0 ? 1.0 - f.x : f.x) * (dy == 0 ? 1.0 - f.y : f.y);
            if (share > 0.0 && all(greaterThanEqual(texel, v_tex.xy)) && all(lessThan(texel, v_tex.zw))) {
                vec4 ink = texelFetch(atlas, ivec2(texel), 0);
                sum += share * vec4(ink.rgb * ink.a, ink.a);
            }
        }
    }
    if (sum.a <= 0.0) {
        discard;
    }
    out_color = vec4(sum.rgb / sum.a, sum.a * v_origin.z * clip_cov(p));
}
