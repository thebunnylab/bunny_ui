// dst *= coverage of the window's own rounded box — the premultiplied
// corner fade (the blend factors carry the multiply).
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

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

void main() {
    float coverage =
        clamp(0.5 - rect_sdf(gl_FragCoord.xy, pc.round_box, pc.round_radii), 0.0, 1.0);
    out_color = vec4(0.0, 0.0, 0.0, coverage);
}
