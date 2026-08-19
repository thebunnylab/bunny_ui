// The rect coverage evaluator — the CPU raster's kernels, spoken in
// Vulkan GLSL. gl_FragCoord is already the raster's top-left space.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec2 viewport;
    float round_radius;
    float pad;
} pc;

layout(location = 0) flat in vec4 v_rect;
layout(location = 1) flat in vec4 v_params;
layout(location = 2) flat in vec4 v_color;
layout(location = 3) flat in vec4 v_color2;
layout(location = 4) flat in vec2 v_point2;

layout(location = 0) out vec4 out_color;

float rect_sdf(vec2 p, vec4 rect, float radius) {
    vec2 shifted = max(rect.xy + radius - p, p - (rect.zw - radius));
    float outside = length(max(shifted, vec2(0.0)));
    float inside = min(max(shifted.x, shifted.y), 0.0);
    return outside + inside - radius;
}

float rect_cov(vec2 p, vec4 rect, float radius) {
    return clamp(0.5 - rect_sdf(p, rect, radius), 0.0, 1.0);
}

float clip_cov(vec2 p) {
    return pc.round_radius > 0.0 ? rect_cov(p, pc.round_box, pc.round_radius) : 1.0;
}

void main() {
    vec2 p = gl_FragCoord.xy;
    float kind = v_params.z;
    float coverage;
    if (kind == 0.0) {
        coverage = rect_cov(p, v_rect, v_params.x);
    } else if (kind == 1.0) {
        float thickness = v_params.y;
        vec4 inner = vec4(v_rect.xy + thickness, v_rect.zw - thickness);
        float inner_radius = max(v_params.x - thickness, 0.0);
        coverage = clamp(
            rect_cov(p, v_rect, v_params.x) - rect_cov(p, inner, inner_radius),
            0.0, 1.0);
    } else if (kind == 2.0) {
        float expansion = v_params.w;
        vec4 base = vec4(v_rect.xy + expansion, v_rect.zw - expansion);
        float corner = v_params.x;
        float reach = v_params.y;
        vec2 delta = p - clamp(p, base.xy + corner, base.zw - corner);
        float dist = length(delta) - corner;
        float strength = 1.0 - dist / reach;
        coverage = (dist > 0.0 && dist < reach) ? strength * strength : 0.0;
    } else {
        coverage = rect_cov(p, v_rect, v_params.x);
        float t;
        if (kind == 3.0) {
            float dist = length(p - v_point2);
            t = clamp((dist - v_params.y) / (v_params.w - v_params.y), 0.0, 1.0);
        } else if (kind == 5.0) {
            coverage = rect_cov(p, v_rect, 0.0);
            vec2 away = p - v_point2;
            float dist = length(vec2(away.x, away.y / v_params.x));
            t = clamp((dist - v_params.y) / (v_params.w - v_params.y), 0.0, 1.0);
        } else {
            vec2 origin = vec2(v_params.y, v_params.w);
            vec2 axis = v_point2 - origin;
            float length2 = dot(axis, axis);
            t = length2 > 0.0 ? clamp(dot(p - origin, axis) / length2, 0.0, 1.0) : 1.0;
        }
        vec4 mixed = floor(mix(v_color, v_color2, t) * 255.0 + 0.5) / 255.0;
        out_color = vec4(mixed.rgb, mixed.a * coverage * clip_cov(p));
        return;
    }
    out_color = vec4(v_color.rgb, v_color.a * coverage * clip_cov(p));
}
