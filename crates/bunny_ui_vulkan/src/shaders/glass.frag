// The liquid-glass material — `bunny_ui::glass`, evaluated. Every
// constant below is that module's, and the parity tests hold the two
// answers together. gl_FragCoord is already the raster's top-left
// space, and so is the pyramid's uv: no flip anywhere.
#version 450

layout(push_constant) uniform Push {
    vec4 round_box;
    vec4 quad;
    vec4 round_radii;
    vec2 viewport;
} pc;

layout(set = 0, binding = 0) uniform sampler2D pyramid;

layout(location = 0) flat in vec4 v_rect;
layout(location = 1) flat in vec4 v_radii;
layout(location = 2) flat in vec4 v_lens;
layout(location = 3) flat in vec4 v_finish;
layout(location = 4) flat in vec4 v_touch;
layout(location = 5) flat in vec4 v_tint;
layout(location = 6) flat in vec4 v_highlight;
layout(location = 7) flat in float v_spot_alpha;

layout(location = 0) out vec4 out_color;

const float GLASS_SIGMA_L0 = 5.2;
const float GLASS_MAX_LEVEL = 3.0;
const float GLASS_RIM_FLOOR = 0.1;
const float GLASS_RIM_FALLOFF = 1.7;
const vec2 GLASS_LIGHT_DIR = vec2(-0.70710678, -0.70710678);
const vec3 GLASS_LUMA = vec3(0.2126, 0.7152, 0.0722);
const float GLASS_OUTER_AMOUNT_RATIO = 0.25;
const float GLASS_OUTER_HEIGHT_RATIO = 0.5;
const float GLASS_VIBRANT_SATURATION = 2.069;
const float GLASS_VIBRANT_GAIN = 1.45;
const float GLASS_VIBRANT_BIAS = 0.05;
const float GLASS_GRAD_RADIUS_FACTOR = 1.5;

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

float clip_cov(vec2 p) {
    return any(greaterThan(pc.round_radii, vec4(0.0)))
        ? clamp(0.5 - rect_sdf(p, pc.round_box, pc.round_radii), 0.0, 1.0)
        : 1.0;
}

vec3 linear_to_srgb3(vec3 c) {
    return mix(1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, c * 12.92,
               lessThanEqual(c, vec3(0.0031308)));
}

// the lens profile: a quarter circle, one at the rim and flat at the
// centre, with an INFINITE slope at the rim
float glass_circle_map(float x) {
    float c = clamp(x, 0.0, 1.0);
    return 1.0 - sqrt(max(1.0 - c * c, 0.0));
}

float glass_level(float sigma) {
    return clamp(log2(max(sigma, GLASS_SIGMA_L0) / GLASS_SIGMA_L0), 0.0, GLASS_MAX_LEVEL);
}

// the analytic gradient of the rounded-rect field — never a
// screen-space derivative, which is quantised to 2x2 quads and shows as
// a stair-stepped rim
vec2 glass_normal(vec2 center_to_point, vec2 corner_center) {
    vec2 s = vec2(center_to_point.x < 0.0 ? -1.0 : 1.0,
                  center_to_point.y < 0.0 ? -1.0 : 1.0);
    vec2 m = max(corner_center, vec2(0.0));
    float l = length(m);
    if (l > 1e-5) {
        return s * (m / l);
    }
    return corner_center.x > corner_center.y ? vec2(s.x, 0.0) : vec2(0.0, s.y);
}

void main() {
    vec2 p = gl_FragCoord.xy;
    vec2 half_size = (v_rect.zw - v_rect.xy) * 0.5;
    vec2 center_to_point = p - v_rect.xy - half_size;
    float radius = corner_at(p, v_rect, v_radii);
    vec2 corner_to_point = abs(center_to_point) - half_size;
    vec2 corner_center = corner_to_point + radius;
    float sdf = length(max(corner_center, vec2(0.0)))
        + min(max(corner_center.x, corner_center.y), 0.0) - radius;
    float coverage = clamp(0.5 - sdf, 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }
    float depth = max(-sdf, 0.0);

    // the direction field, ovalised so a corner sweeps instead of
    // kinking. The true radius already cut the shape above
    float grad_radius = min(radius * GLASS_GRAD_RADIUS_FACTOR, min(half_size.x, half_size.y));
    vec2 normal = glass_normal(center_to_point, corner_to_point + grad_radius);

    // two opposed bands on one quarter-circle profile. The main one
    // samples INWARD: the rim magnifies, a convex lens. Outward pinches
    float band = max(v_lens.y, 1.0);
    float inner = glass_circle_map(1.0 - depth / band);
    float outer = glass_circle_map(1.0 - depth / (band * GLASS_OUTER_HEIGHT_RATIO));
    float profile = inner - outer * GLASS_OUTER_AMOUNT_RATIO;
    vec2 displace = normal * (-v_lens.z * profile);

    // sharper where the lens works, frosted on the face
    float sharpen = 1.0 - clamp(depth / band, 0.0, 1.0);
    float mip = max(glass_level(v_lens.x) - sharpen, 0.0);
    vec2 inv_viewport = 1.0 / pc.viewport;
    vec2 base = p * inv_viewport;
    vec4 sampled;
    if (v_lens.w > 0.0) {
        float spread = v_lens.w;
        vec4 red = textureLod(pyramid, base + displace * (1.0 - spread) * inv_viewport, mip);
        vec4 green = textureLod(pyramid, base + displace * inv_viewport, mip);
        vec4 blue = textureLod(pyramid, base + displace * (1.0 + spread) * inv_viewport, mip);
        sampled = vec4(red.r, green.g, blue.b, green.a);
    } else {
        sampled = textureLod(pyramid, base + displace * inv_viewport, mip);
    }

    // back to the engine's colour space FIRST: the saturation this
    // material is tuned against runs on ENCODED values, unlike the
    // blur, which must average in linear light
    float alpha = max(sampled.a, 1e-4);
    vec3 rgb = linear_to_srgb3(sampled.rgb / alpha);
    float luma = dot(rgb, GLASS_LUMA);
    rgb = (luma + (rgb - luma) * v_finish.z) * v_finish.w;
    vec4 color = vec4(rgb, sampled.a);

    // the tint, over
    color = vec4(mix(color.rgb, v_tint.rgb, v_tint.a),
                 v_tint.a + color.a * (1.0 - v_tint.a));

    // the specular rim: a thin band lit along BOTH diagonals, in the
    // colour of the scene under it, ADDED instead of painted
    float rim = 1.0 - clamp(depth / max(v_finish.x, 1.0), 0.0, 1.0);
    float axis = abs(dot(normal, GLASS_LIGHT_DIR));
    float ring = GLASS_RIM_FLOOR + (1.0 - GLASS_RIM_FLOOR) * pow(axis, GLASS_RIM_FALLOFF);
    float strength = v_finish.y * rim * rim * ring * v_highlight.a;
    if (strength > 0.0) {
        float grey = dot(color.rgb, GLASS_LUMA);
        vec3 vibrant = clamp(
            (grey + (color.rgb - grey) * GLASS_VIBRANT_SATURATION) * GLASS_VIBRANT_GAIN
            + GLASS_VIBRANT_BIAS, 0.0, 1.0);
        color = vec4(clamp(color.rgb + vibrant * v_highlight.rgb * strength, 0.0, 1.0), color.a);
    }

    // the touch: a flat wash plus a pool of light, both additive and
    // both zero unless the pane asked
    float spot = 0.0;
    if (v_spot_alpha > 0.0 && v_touch.w > 0.0) {
        float away = distance(p, v_touch.yz);
        float fall = 1.0 - clamp(away / v_touch.w, 0.0, 1.0);
        spot = v_spot_alpha * fall * fall;
    }
    float touch = clamp(v_touch.x + spot, 0.0, 1.0);
    if (touch > 0.0) {
        color = vec4(clamp(color.rgb + touch, 0.0, 1.0), color.a);
    }

    // straight alpha out — the blend state premultiplies, exactly as it
    // does for a rect
    out_color = vec4(color.rgb, color.a * coverage * clip_cov(p));
}
