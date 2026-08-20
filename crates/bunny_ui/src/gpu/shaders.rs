//! The shader sources every GPU tier compiles, and the coverage math
//! they all evaluate.
//!
//! One body, two dialects. A tier brings its own prelude — desktop GL
//! speaks `#version 330 core`, a browser speaks `#version 300 es` and
//! must name its precisions — and the text below is the same string
//! for both. That is the point: the rasterizer next door is the
//! oracle, these are its transliteration, and a second copy of the
//! same ramp is a second thing to drift.
//!
//! The vulkan tier is the counter-example. Its GLSL lives on disk
//! because SPIR-V is baked, and it has diverged into a hand-kept
//! twin — `gl_VertexIndex`, push constants, no flip. Two copies of
//! this math already exist. There must not be a third.

/// Desktop GL: the core profile the linux tier asks for.
pub const PRELUDE_330: &str = r#"#version 330 core
"#;

/// The browser: GLSL ES 3.00. The three precisions are not decoration.
/// A fragment stage in ES has NO default float precision and will not
/// compile without one; `texelFetch` addresses a four-thousand-texel
/// atlas with an int; and `gl_FragCoord` inherits the default float,
/// which is the snapping contract itself.
pub const PRELUDE_300ES: &str = r#"#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;
"#;

// MARK: - Shaders (compiled at runtime; the structs above, as attributes)

// The coverage math is the CPU raster's, rewritten once — the same
// kernels the windows shell ships in HLSL, spoken in GLSL 330:
// `clamp(0.5 - sdf, 0, 1)` IS `clamp(radius - distance + 0.5, 0, 1)`
// for the rounded corner, and the full signed distance (outside +
// inside terms) reproduces the straight spans exactly. The instance
// arrives as divisor-1 attributes; the per-run base is the byte offset
// the attrib pointers carry — no shader-side index at all. Colors are
// normalized ubyte attributes (exactly c/255). `gl_FragCoord` counts
// from the bottom — every fragment flips into the raster's top-left
// space first, keeping the +0.5 pixel center intact.

pub const SHARED_FRAG: &str = r#"
layout(std140) uniform Frame {
    vec2 viewport;
};
layout(std140) uniform Round {
    vec4 round_box;
    vec4 round_radii;
};

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

// the curve that softens the run's clip. radius 0 is the straight
// rectangle the quad clamp already cut — and multiplying by 1.0 is
// exact, so a scene without a rounded clip leaves both shaders
// untouched, bit for bit
float clip_cov(vec2 p) {
    return any(greaterThan(round_radii, vec4(0.0)))
        ? rect_cov(p, round_box, round_radii)
        : 1.0;
}

// gl_FragCoord counts from the bottom-left; the raster counts from the
// top — flip once, the +0.5 pixel center survives the mirror
vec2 raster_p() {
    return vec2(gl_FragCoord.x, viewport.y - gl_FragCoord.y);
}
"#;

pub const RECT_VERT: &str = r#"
layout(std140) uniform Frame {
    vec2 viewport;
};

in vec4 a_rect;
in vec4 a_clip;
in vec4 a_params;
in vec4 a_color;
in vec4 a_color2;
in vec2 a_point2;
in vec4 a_radii;

flat out vec4 v_rect;
flat out vec4 v_params;
flat out vec4 v_color;
flat out vec4 v_color2;
flat out vec2 v_point2;
flat out vec4 v_radii;

const vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    // the clip cuts the QUAD, not the coverage: clips are snapped to
    // integers, so the cut falls between pixel centers — exactly the
    // CPU's integer clip
    vec2 low = max(a_rect.xy, a_clip.xy);
    vec2 high = max(min(a_rect.zw, a_clip.zw), low);
    vec2 corner = unit_corners[gl_VertexID];
    vec2 unit = mix(low, high, corner) / viewport;
    gl_Position = vec4(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
    v_rect = a_rect;
    v_params = a_params;
    v_color = a_color;
    v_color2 = a_color2;
    v_point2 = a_point2;
    v_radii = a_radii;
}
"#;

pub const RECT_FRAG_BODY: &str = r#"
flat in vec4 v_rect;
flat in vec4 v_params;
flat in vec4 v_color;
flat in vec4 v_color2;
flat in vec2 v_point2;
flat in vec4 v_radii;
out vec4 out_color;

void main() {
    vec2 p = raster_p();
    float kind = v_params.z;
    float coverage;
    if (kind == 0.0) {
        // fill: the cpu corner ramp, clamp(radius - d + 0.5, 0, 1)
        coverage = rect_cov(p, v_rect, v_radii);
    } else if (kind == 1.0) {
        // stroke: outer coverage minus the inner rect's — the inset
        // keeps the same corner center as the cpu ring, and integer
        // edges keep the straight bars exact and never double-blended
        float thickness = v_params.y;
        vec4 inner = vec4(v_rect.xy + thickness, v_rect.zw - thickness);
        vec4 inner_radii = max(v_radii - thickness, vec4(0.0));
        coverage = clamp(
            rect_cov(p, v_rect, v_radii) - rect_cov(p, inner, inner_radii),
            0.0, 1.0);
    } else if (kind == 2.0) {
        // shadow: quadratic falloff outside the rounded core — the quad
        // arrives pre-expanded, params.w undoes the expansion
        float expansion = v_params.w;
        vec4 base = vec4(v_rect.xy + expansion, v_rect.zw - expansion);
        float corner = corner_at(p, base, v_radii);
        float reach = v_params.y;
        vec2 delta = p - clamp(p, base.xy + corner, base.zw - corner);
        float dist = length(delta) - corner;
        float strength = 1.0 - dist / reach;
        coverage = (dist > 0.0 && dist < reach) ? strength * strength : 0.0;
    } else {
        // the gradients cover the fill's shape and change color per
        // pixel: rings from point2 (params.y and .w are the radii), or
        // a ramp from params to point2. The cpu resolved every number
        // in f64 — this only mixes.
        coverage = rect_cov(p, v_rect, v_radii);
        float t;
        if (kind == 3.0) {
            float dist = length(p - v_point2);
            t = clamp((dist - v_params.y) / (v_params.w - v_params.y), 0.0, 1.0);
        } else if (kind == 5.0) {
            // the ellipse is a circle in a Y-scaled space; its corner
            // slot carries the aspect, so the cover is the plain box
            coverage = rect_cov(p, v_rect, vec4(0.0));
            vec2 away = p - v_point2;
            float dist = length(vec2(away.x, away.y / v_params.x));
            t = clamp((dist - v_params.y) / (v_params.w - v_params.y), 0.0, 1.0);
        } else {
            vec2 origin = vec2(v_params.y, v_params.w);
            vec2 axis = v_point2 - origin;
            float length2 = dot(axis, axis);
            t = length2 > 0.0 ? clamp(dot(p - origin, axis) / length2, 0.0, 1.0) : 1.0;
        }
        // the cpu rounds the mixed color to bytes before blending;
        // rounding here keeps the two within one step (the attributes
        // are already c/255 — scale up, round, scale back)
        vec4 mixed = floor(mix(v_color, v_color2, t) * 255.0 + 0.5) / 255.0;
        out_color = vec4(mixed.rgb, mixed.a * coverage * clip_cov(p));
        return;
    }
    out_color = vec4(v_color.rgb, v_color.a * coverage * clip_cov(p));
}
"#;

pub const SPRITE_VERT: &str = r#"
layout(std140) uniform Frame {
    vec2 viewport;
};

in vec4 a_dest;
in vec4 a_tex;
in vec4 a_clip;

flat out vec4 v_dest;
flat out vec4 v_tex;

const vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    vec2 low = max(a_dest.xy, a_clip.xy);
    vec2 high = max(min(a_dest.zw, a_clip.zw), low);
    vec2 corner = unit_corners[gl_VertexID];
    vec2 unit = mix(low, high, corner) / viewport;
    gl_Position = vec4(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
    v_dest = a_dest;
    v_tex = a_tex;
}
"#;

pub const SPRITE_FRAG_BODY: &str = r#"
flat in vec4 v_dest;
flat in vec4 v_tex;
out vec4 out_color;
uniform sampler2D atlas;

void main() {
    vec2 p = raster_p();
    vec2 texel = v_tex.xy + (floor(p) - floor(v_dest.xy));
    // straight alpha in, straight alpha out — only the coverage moves,
    // and text under a rounded corner loses its square edge at last
    vec4 ink = texelFetch(atlas, ivec2(texel), 0);
    out_color = vec4(ink.rgb, ink.a * clip_cov(p));
}
"#;

// the scene's corner pass: the CPU road multiplies the corner squares
// of its ARGB backing by the rounded-window coverage (premultiplied);
// the GPU twin draws the same four squares with dst *= coverage —
// blend (ZERO, SRC_ALPHA) — over an alpha-carrying surface. An opaque
// frame times coverage IS the premultiplied corner, no extra pass.
pub const MASK_VERT: &str = r#"
layout(std140) uniform Frame {
    vec2 viewport;
};
uniform vec4 u_quad;

const vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    vec2 corner = unit_corners[gl_VertexID];
    vec2 unit = mix(u_quad.xy, u_quad.zw, corner) / viewport;
    gl_Position = vec4(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
}
"#;

pub const MASK_FRAG_BODY: &str = r#"
out vec4 out_color;

void main() {
    // the window's own rounded box rides the Round block
    out_color = vec4(0.0, 0.0, 0.0, rect_cov(raster_p(), round_box, round_radii));
}
"#;


// MARK: - Liquid glass (the material of `glass.rs`, in GLSL)
//
// A pane READS the scene, and no pass can sample the target it is
// drawing into. So a frame with glass renders into a scene texture of
// its own, blurs it into the pyramid, draws the panes over it, and
// blits the result onto the real target at the end. A frame without
// glass never binds one of these programs.
//
// The pyramid is `SRGB8_ALPHA8` on purpose: sampling decodes and — with
// `GL_FRAMEBUFFER_SRGB` on for those passes alone — writing encodes, so
// the whole chain averages in LINEAR light for free.

/// The fullscreen triangle both the blur and the blit ride: three
/// vertices, no attributes, no shared edge for the rasterizer to seam.
pub const FULL_VERT: &str = r#"
void main() {
    vec2 uv = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}
"#;

pub const BLIT_FRAG: &str = r#"
uniform sampler2D source;
out vec4 out_color;

void main() {
    // an exact copy: one texel fetch, no filter, no conversion
    out_color = texelFetch(source, ivec2(gl_FragCoord.xy), 0);
}
"#;

pub const BLUR_FRAG: &str = r#"
uniform sampler2D source;
// x, y = 1 / destination size; z, w = the direction, (1,0) or (0,1)
uniform vec4 blur_step;
// x = the mip this pass reads; y = 1 when the source is raw scene
// colour, which no format decodes for us
uniform vec4 blur_mode;
out vec4 out_color;

const float BLUR_W[5] = float[5](0.153584, 0.256886, 0.125975, 0.034902, 0.005445);
const float BLUR_O[5] = float[5](0.0, 1.44475, 3.37341, 5.30746, 7.24824);

vec3 srgb_to_linear3(vec3 c) {
    return mix(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92,
               lessThanEqual(c, vec3(0.04045)));
}

vec4 blur_tap(vec2 uv) {
    vec4 c = textureLod(source, uv, blur_mode.x);
    // colour only: a transfer function never applies to alpha
    return blur_mode.y != 0.0 ? vec4(srgb_to_linear3(c.rgb), c.a) : c;
}

void main() {
    // the destination is half the resolution of the source and the
    // offsets are in DESTINATION texels — which is what makes the
    // downsample free: each bilinear tap already averages a 2x2
    vec2 inv = blur_step.xy;
    vec2 uv = gl_FragCoord.xy * inv;
    vec2 stride = blur_step.zw * inv;
    vec4 acc = blur_tap(uv) * BLUR_W[0];
    for (int i = 1; i < 5; i++) {
        vec2 away = stride * BLUR_O[i];
        acc += (blur_tap(uv + away) + blur_tap(uv - away)) * BLUR_W[i];
    }
    out_color = acc;
}
"#;

pub const GLASS_VERT: &str = r#"
layout(std140) uniform Frame {
    vec2 viewport;
};

in vec4 a_rect;
in vec4 a_clip;
in vec4 a_radii;
in vec4 a_lens;
in vec4 a_finish;
in vec4 a_touch;
in vec4 a_tint;
in vec4 a_highlight;
in vec4 a_spot;

flat out vec4 v_rect;
flat out vec4 v_radii;
flat out vec4 v_lens;
flat out vec4 v_finish;
flat out vec4 v_touch;
flat out vec4 v_tint;
flat out vec4 v_highlight;
flat out float v_spot_alpha;

const vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

void main() {
    vec2 low = max(a_rect.xy, a_clip.xy);
    vec2 high = max(min(a_rect.zw, a_clip.zw), low);
    vec2 corner = unit_corners[gl_VertexID];
    vec2 unit = mix(low, high, corner) / viewport;
    gl_Position = vec4(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
    v_rect = a_rect;
    v_radii = a_radii;
    v_lens = a_lens;
    v_finish = a_finish;
    v_touch = a_touch;
    v_tint = a_tint;
    v_highlight = a_highlight;
    v_spot_alpha = a_spot.x;
}
"#;

pub const GLASS_FRAG_BODY: &str = r#"
uniform sampler2D pyramid;

flat in vec4 v_rect;
flat in vec4 v_radii;
flat in vec4 v_lens;
flat in vec4 v_finish;
flat in vec4 v_touch;
flat in vec4 v_tint;
flat in vec4 v_highlight;
flat in float v_spot_alpha;
out vec4 out_color;

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

// the analytic gradient of the rounded-rect field — never a screen-space
// derivative, which is quantised to 2x2 quads and shows as a
// stair-stepped rim
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

// the scene texture and its pyramid are GL-oriented (row zero at the
// bottom); the material thinks in raster space, so the sample flips on
// the way out
vec2 to_uv(vec2 raster) {
    return vec2(raster.x / viewport.x, 1.0 - raster.y / viewport.y);
}

void main() {
    vec2 p = raster_p();
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
    vec4 sampled;
    if (v_lens.w > 0.0) {
        float spread = v_lens.w;
        vec4 red = textureLod(pyramid, to_uv(p + displace * (1.0 - spread)), mip);
        vec4 green = textureLod(pyramid, to_uv(p + displace), mip);
        vec4 blue = textureLod(pyramid, to_uv(p + displace * (1.0 + spread)), mip);
        sampled = vec4(red.r, green.g, blue.b, green.a);
    } else {
        sampled = textureLod(pyramid, to_uv(p + displace), mip);
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
"#;
