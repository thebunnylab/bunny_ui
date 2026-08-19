// dst *= coverage of the window's own rounded box — the premultiplied
// corner fade (the blend factors carry the multiply).
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec2 viewport;
    float round_radius;
    float pad;
} pc;

layout(location = 0) out vec4 out_color;

float rect_sdf(vec2 p, vec4 rect, float radius) {
    vec2 shifted = max(rect.xy + radius - p, p - (rect.zw - radius));
    float outside = length(max(shifted, vec2(0.0)));
    float inside = min(max(shifted.x, shifted.y), 0.0);
    return outside + inside - radius;
}

void main() {
    float coverage =
        clamp(0.5 - rect_sdf(gl_FragCoord.xy, pc.round_box, pc.round_radius), 0.0, 1.0);
    out_color = vec4(0.0, 0.0, 0.0, coverage);
}
