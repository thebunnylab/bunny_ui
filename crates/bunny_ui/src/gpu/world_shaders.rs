//! The world's shaders for the GL family: GLSL that both dialects take,
//! behind the same preludes as the 2D sources (`#version 330 core` for
//! desktop GL, `#version 300 es` for a browser).
//!
//! Every function mirrors one of `crate::world::shade` or `soft`, in the
//! same order of operations — the CPU rasterizer is the oracle, and the
//! parity tests hold the two pictures together.
//!
//! Two conventions are this family's own, and both are spelled out where
//! they happen: GL's clip space runs from −w to w in depth, so the frame's
//! `[0, 1]` projection is remapped; and a target is drawn upside down in
//! clip space, so its first row in memory is the picture's TOP row — the
//! order the 2D pass, the CPU and every other tier read.

/// The frame's block and the arithmetic of light, for both stages.
pub const WORLD_COMMON: &str = r#"
layout(std140) uniform World {
    mat4 clip_from_eye;
    vec4 viewport;
    vec4 params;
    vec4 section;
    vec4 sun_direction;
    vec4 sun_color;
    vec4 sky_color;
    vec4 ground_color;
    vec4 sky_up;
    vec4 tone;
    mat4 shadow_from_eye;
    vec4 shadow;
    vec4 counts;
    vec4 point_position[8];
    vec4 point_color[8];
    uvec4 point_groups[2];
};

// (flags, near, far, step back): what one draw adds to the frame
uniform vec4 u_draw;

const uint DRAW_MASK = 1u;
const uint DRAW_TEXTURED = 2u;
const uint DRAW_REPLACE = 4u;
const uint MATERIAL_UNLIT = 1u;
const uint MATERIAL_MASK = 4u;
const float LINE_PULL = 2.0e-3;
// the largest number below the far plane's 1, and the constant step
// that breaks a surface's ties with the edges on it (2^-22)
const float DEPTH_CEILING = 0.99999994;
const float DEPTH_TIE = 2.384185791015625e-7;

// the frame's projection gives a depth in [0, w]; GL's clip space wants
// [-w, w], and the picture upside down so its first row is its top
vec4 to_gl(vec4 clip) {
    return vec4(clip.x, -clip.y, 2.0 * clip.z - clip.w, clip.w);
}

float encode_code(float linear) {
    if (!(linear > 0.0)) {
        return 0.0;
    }
    if (linear >= 1.0) {
        return 255.0;
    }
    float s = linear <= 0.0031308 ? linear * 12.92 : 1.055 * pow(linear, 1.0 / 2.4) - 0.055;
    return s * 255.0;
}
"#;

/// The sRGB table, for the vertex stage: written at runtime from the
/// core's own table, so each value parses back to the same bits.
pub fn srgb_table() -> String {
    let values: Vec<String> = crate::world::shade::SRGB_DECODE.iter().map(|value| format!("{value:?}")).collect();
    format!("const float SRGB_DECODE[256] = float[256]({});\n", values.join(", "))
}

/// A surface's vertex stage: the vertex and its instance arrive as
/// attributes at fixed locations; the instance attributes are re-pointed
/// per draw, so the first instance rides the byte offset.
pub const MESH_VERT: &str = r#"
layout(location = 0) in vec3 a_position;
layout(location = 1) in ivec2 a_normal;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in uvec4 a_color;
layout(location = 4) in uvec2 a_ids;
layout(location = 5) in vec4 i_row0;
layout(location = 6) in vec4 i_row1;
layout(location = 7) in vec4 i_row2;
layout(location = 8) in vec4 i_normal0;
layout(location = 9) in vec4 i_normal1;
layout(location = 10) in vec4 i_normal2;
layout(location = 11) in uvec4 i_tint;
layout(location = 12) in uvec3 i_ids;

out vec3 v_eye;
out vec3 v_normal;
out vec2 v_uv;
out vec4 v_color;
out float v_occlusion;
out vec2 v_zw;
flat out uint v_material;
flat out vec4 v_tint;

vec3 unfold_normal(ivec2 folded) {
    float u = max(float(folded.x) / 32767.0, -1.0);
    float v = max(float(folded.y) / 32767.0, -1.0);
    float z = 1.0 - abs(u) - abs(v);
    float x = u;
    float y = v;
    if (z < 0.0) {
        x = (1.0 - abs(v)) * (u >= 0.0 ? 1.0 : -1.0);
        y = (1.0 - abs(u)) * (v >= 0.0 ? 1.0 : -1.0);
    }
    float len = sqrt(x * x + y * y + z * z);
    return vec3(x / len, y / len, z / len);
}

void main() {
    vec3 p = a_position;
    vec3 eye = vec3(i_row0.x * p.x + i_row0.y * p.y + i_row0.z * p.z + i_row0.w,
                    i_row1.x * p.x + i_row1.y * p.y + i_row1.z * p.z + i_row1.w,
                    i_row2.x * p.x + i_row2.y * p.y + i_row2.z * p.z + i_row2.w);
    vec3 n = unfold_normal(a_normal);
    vec4 clip = clip_from_eye * vec4(eye, 1.0);
    gl_Position = to_gl(clip);
    v_zw = vec2(clip.z, clip.w);
    v_eye = eye;
    v_normal = vec3(dot(i_normal0.xyz, n), dot(i_normal1.xyz, n), dot(i_normal2.xyz, n));
    v_uv = a_uv;
    v_color = vec4(SRGB_DECODE[a_color.r], SRGB_DECODE[a_color.g], SRGB_DECODE[a_color.b], float(a_color.a) / 255.0);
    v_occlusion = float(a_ids.y) / 255.0;
    v_material = i_ids.x + a_ids.x;
    v_tint = vec4(SRGB_DECODE[i_tint.r], SRGB_DECODE[i_tint.g], SRGB_DECODE[i_tint.b], float(i_tint.a) / 255.0);
}
"#;

/// A surface's fragment stage, up to the colour: Bunny Lit v1.
pub const MESH_FRAG_COMMON: &str = r#"
in vec3 v_eye;
in vec3 v_normal;
in vec2 v_uv;
in vec4 v_color;
in float v_occlusion;
in vec2 v_zw;
flat in uint v_material;
flat in vec4 v_tint;

uniform sampler2D u_materials;
uniform sampler2D u_surface;
uniform highp sampler2DShadow u_shadow;
out vec4 out_color;

const int BAYER[16] = int[16](0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5);

vec3 safe_normalize(vec3 a) {
    float len = sqrt(dot(a, a));
    return len > 0.0 ? a * (1.0 / len) : vec3(0.0, 0.0, 1.0);
}

float exp2_fixed(float x) {
    x = max(x, -126.0);
    float whole = floor(x);
    float f = x - whole;
    float fraction = 0.99999976 + f * (0.6931568 + f * (0.24013178 + f * (0.05587644 + f * (0.00894063 + f * 0.0018943873))));
    float power = uintBitsToFloat(uint(int(whole) + 127) << 23);
    return power * fraction;
}

vec3 hemisphere(vec3 direction, vec3 up, vec3 sky, vec3 ground) {
    return ground + (sky - ground) * (0.5 + 0.5 * dot(direction, up));
}

vec3 environment_brdf(vec3 f0, float roughness, float n_dot_v) {
    vec4 r = vec4(1.0 - roughness, roughness * -0.0275 + 0.0425, roughness * -0.572 + 1.04, roughness * 0.022 - 0.04);
    float a004 = min(r.x * r.x, exp2_fixed(-9.28 * n_dot_v)) * r.x + r.y;
    float a = -1.04 * a004 + r.z;
    float b = 1.04 * a004 + r.w;
    return f0 * a + b;
}

vec3 tone_curve(vec3 color, float exposure, float knee) {
    vec3 c = color * exposure;
    float m = max(max(c.r, c.g), c.b);
    if (m > knee) {
        float f = knee + (1.0 - knee) * (m - knee) / ((1.0 - knee) + (m - knee));
        c = c * (f / m);
        float t = clamp((m - 1.0) / 3.0, 0.0, 1.0);
        float bleach = 0.6 * t * t * (3.0 - 2.0 * t);
        c = c + (vec3(f) - c) * bleach;
    }
    return clamp(c, 0.0, 1.0);
}

float bayer(uint x, uint y) {
    return (float(BAYER[int((y & 3u) * 4u + (x & 3u))]) + 0.5) / 16.0 - 0.5;
}

// how much of the sun reaches a point: its depth from the sun against the
// map's, each lookup four texels compared and filtered by the sampler
float sun_visibility(vec3 position, vec3 n) {
    float taps = shadow.w;
    if (taps < 0.5 || sun_direction.w <= 0.5) {
        return 1.0;
    }
    float n_dot_l = clamp(dot(n, sun_direction.xyz), 0.0, 1.0);
    if (!(n_dot_l > 0.0)) {
        return 1.0;
    }
    float sine = sqrt(max(1.0 - n_dot_l * n_dot_l, 0.0));
    vec3 p = position + n * (shadow.y * sine);
    vec4 s = shadow_from_eye * vec4(p, 1.0);
    if (!(s.x > 0.0 && s.x < 1.0 && s.y > 0.0 && s.y < 1.0)) {
        return 1.0;
    }
    float reference = clamp(s.z - shadow.z, 0.0, 1.0);
    float texel = shadow.x;
    if (taps < 1.5) {
        return textureLod(u_shadow, vec3(s.x, s.y, reference), 0.0);
    }
    if (taps < 4.5) {
        float sum = 0.0;
        sum += textureLod(u_shadow, vec3(s.x - 0.5 * texel, s.y - 0.5 * texel, reference), 0.0);
        sum += textureLod(u_shadow, vec3(s.x + 0.5 * texel, s.y - 0.5 * texel, reference), 0.0);
        sum += textureLod(u_shadow, vec3(s.x - 0.5 * texel, s.y + 0.5 * texel, reference), 0.0);
        sum += textureLod(u_shadow, vec3(s.x + 0.5 * texel, s.y + 0.5 * texel, reference), 0.0);
        return sum * 0.25;
    }
    float sum = 0.0;
    for (int j = -1; j <= 1; j++) {
        for (int i = -1; i <= 1; i++) {
            sum += textureLod(u_shadow, vec3(s.x + float(i) * texel, s.y + float(j) * texel, reference), 0.0);
        }
    }
    return sum / 9.0;
}

vec3 lit(vec3 position, vec3 n, vec3 base, float roughness, float metallic, vec3 emissive, float occlusion, uint groups, float shadow) {
    bool parallel = params.w > 0.5;
    vec3 v = parallel ? vec3(0.0, 0.0, 1.0) : safe_normalize(-position);
    float n_dot_v = clamp(dot(n, v), 1e-4, 1.0);
    float metal = clamp(metallic, 0.0, 1.0);
    vec3 diffuse = base * (1.0 - metal);
    vec3 f0 = vec3(0.04) + (base - vec3(0.04)) * metal;
    float r = clamp(roughness, 0.045, 1.0);
    float a = r * r;
    float a2 = r * r * r * r;
    vec3 up = sky_up.xyz;
    vec3 sky = sky_color.xyz;
    vec3 ground = ground_color.xyz;
    float ao = 1.0 + (occlusion - 1.0) * 0.8;

    vec3 light = diffuse * hemisphere(n, up, sky, ground) * ao;
    vec3 reflected = n * (2.0 * dot(n, v)) - v;
    light = light + environment_brdf(f0, r, n_dot_v) * hemisphere(reflected, up, sky, ground) * ao;

    if (sun_direction.w > 0.5) {
        vec3 l = sun_direction.xyz;
        float n_dot_l = clamp(dot(n, l), 0.0, 1.0);
        if (n_dot_l > 0.0) {
            vec3 h = safe_normalize(l + v);
            float n_dot_h = clamp(dot(n, h), 0.0, 1.0);
            float v_dot_h = clamp(dot(v, h), 0.0, 1.0);
            float q = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
            float pi_d = a2 / (q * q);
            float visibility = 0.5 / max(2.0 * n_dot_l * n_dot_v + (n_dot_l + n_dot_v - 2.0 * n_dot_l * n_dot_v) * a, 1e-5);
            float x = 1.0 - v_dot_h;
            float x5 = x * x * x * x * x;
            vec3 fresnel = f0 + (vec3(1.0) - f0) * x5;
            vec3 specular = fresnel * (pi_d * visibility);
            light = light + (diffuse + specular) * sun_color.xyz * (n_dot_l * shadow);
        }
    }

    int points = min(int(counts.x), 8);
    for (int i = 0; i < 8; i++) {
        if (i >= points) {
            break;
        }
        if ((point_groups[i / 4][i % 4] & groups) == 0u) {
            continue;
        }
        vec4 p = point_position[i];
        vec3 to = p.xyz - position;
        float d2 = dot(to, to);
        float range2 = p.w * p.w;
        if (d2 >= range2 || range2 <= 0.0) {
            continue;
        }
        vec3 l = to * (1.0 / sqrt(max(d2, 1e-8)));
        float w = clamp(1.0 - d2 / range2, 0.0, 1.0);
        float r0 = point_color[i].w;
        float attenuation = w * w * (r0 * r0) / max(d2 + r0 * r0, 1e-8);
        light = light + diffuse * point_color[i].xyz * (attenuation * clamp(dot(n, l), 0.0, 1.0));
    }

    return light + emissive;
}

vec4 material_row(int row) {
    int index = int(v_material);
    return texelFetch(u_materials, ivec2((index % 1024) * 4 + row, index / 1024), 0);
}

// premultiplied codes over 255: the target's unorm write rounds them to
// the byte the CPU computes
vec4 shade() {
    if (any(notEqual(section, vec4(0.0))) && section.x * v_eye.x + section.y * v_eye.y + section.z * v_eye.z + section.w < 0.0) {
        discard;
    }
    vec4 base_row = material_row(0);
    vec4 back_row = material_row(1);
    vec4 emissive_row = material_row(2);
    vec4 params_row = material_row(3);
    uint flags = uint(params_row.w);
    uint draw = uint(u_draw.x);
    vec4 color = v_color;
    if ((draw & DRAW_TEXTURED) != 0u) {
        color = color * texture(u_surface, v_uv);
    }
    bool front = gl_FrontFacing;
    bool two_sided = back_row.w > 0.5;
    vec4 surface = (front || !two_sided) ? base_row : vec4(back_row.xyz, base_row.w);
    vec3 base = surface.rgb * v_tint.rgb * color.rgb;
    float alpha = surface.a * v_tint.a * color.a;
    if ((draw & DRAW_MASK) != 0u || (flags & MATERIAL_MASK) != 0u) {
        if (alpha < params_row.z) {
            discard;
        }
        alpha = 1.0;
    }
    if ((draw & DRAW_REPLACE) != 0u) {
        alpha = 1.0;
    }
    vec3 linear;
    float dither = 0.0;
    if ((flags & MATERIAL_UNLIT) != 0u) {
        linear = base;
    } else {
        vec3 normal = safe_normalize(v_normal);
        if (!front) {
            normal = -normal;
        }
        vec3 light = lit(v_eye, normal, base, params_row.x, params_row.y, emissive_row.xyz, v_occlusion, 0xFFFFFFFFu, sun_visibility(v_eye, normal));
        if (tone.z > 0.5) {
            dither = bayer(uint(gl_FragCoord.x), uint(gl_FragCoord.y));
        }
        linear = tone_curve(light, tone.x, tone.y);
    }
    vec3 code = vec3(encode_code(linear.r), encode_code(linear.g), encode_code(linear.b));
    vec3 codes = clamp(floor(code * alpha + 0.5 + dither), 0.0, 255.0);
    return vec4(codes, floor(alpha * 255.0 + 0.5)) / 255.0;
}
"#;

/// A surface, as it shades.
pub const MESH_FRAG: &str = r#"
void main() {
    out_color = shade();
}
"#;

/// A surface under edges: it steps back by its own depth slope, short of
/// the far plane. The slope is the derivative of a screen-linear copy of
/// the depth; `gl_FragCoord.z` is the depth GL interpolated.
pub const MESH_FRAG_STEPPED: &str = r#"
void main() {
    out_color = shade();
    float depth = v_zw.x / v_zw.y;
    float slope = max(abs(dFdx(depth)), abs(dFdy(depth)));
    gl_FragDepth = min(gl_FragCoord.z + u_draw.w * slope + DEPTH_TIE, DEPTH_CEILING);
}
"#;

/// A caster into the sun's shadow map: the vertex placed by the caster's
/// rows, the map upside down in clip space like every target, so its
/// first row is its top. It follows `WORLD_COMMON`.
pub const SHADOW_VERT: &str = r#"
layout(location = 0) in vec3 a_position;
layout(location = 5) in vec4 i_row0;
layout(location = 6) in vec4 i_row1;
layout(location = 7) in vec4 i_row2;

void main() {
    vec3 p = a_position;
    vec3 m = vec3(i_row0.x * p.x + i_row0.y * p.y + i_row0.z * p.z + i_row0.w,
                  i_row1.x * p.x + i_row1.y * p.y + i_row1.z * p.z + i_row1.w,
                  i_row2.x * p.x + i_row2.y * p.y + i_row2.z * p.z + i_row2.w);
    gl_Position = to_gl(vec4(m, 1.0));
}
"#;

/// A caster's fragment: the depth alone, written by the fixed stages.
pub const SHADOW_FRAG: &str = r#"
void main() {
}
"#;

/// A set of lines: one instance per segment, a quad along it that holds
/// every pixel it covers; the coverage is analytic in the fragment.
pub const LINE_VERT: &str = r#"
layout(location = 0) in vec3 a_from;
layout(location = 1) in vec3 a_to;

// the instance's rows, then the style: (colour) and (width, dash, gap, depth mode)
uniform vec4 u_rows[3];
uniform vec4 u_style[2];

flat out vec2 v_a;
flat out vec2 v_b;
flat out vec4 v_depth;
flat out vec3 v_ea;
flat out vec3 v_eb;

const vec2 unit_corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);

vec3 to_eye(vec3 p) {
    return vec3(u_rows[0].x * p.x + u_rows[0].y * p.y + u_rows[0].z * p.z + u_rows[0].w,
                u_rows[1].x * p.x + u_rows[1].y * p.y + u_rows[1].z * p.z + u_rows[1].w,
                u_rows[2].x * p.x + u_rows[2].y * p.y + u_rows[2].z * p.z + u_rows[2].w);
}

void main() {
    vec3 a = to_eye(a_from);
    vec3 b = to_eye(a_to);
    bool parallel = params.w > 0.5;
    v_a = vec2(0.0);
    v_b = vec2(0.0);
    v_depth = vec4(0.0);
    v_ea = vec3(0.0);
    v_eb = vec3(0.0);
    if (!parallel) {
        // keep the part in front of the near plane
        float near_z = -u_draw.y;
        float da = near_z - a.z;
        float db = near_z - b.z;
        if (da < 0.0 && db < 0.0) {
            gl_Position = vec4(0.0, 0.0, -2.0, 1.0);
            return;
        }
        if (da < 0.0) {
            a = a + (b - a) * (da / (da - db));
        } else if (db < 0.0) {
            b = b + (a - b) * (db / (db - da));
        }
    }
    vec3 ea = parallel ? vec3(a.xy, a.z + LINE_PULL * (u_draw.z - u_draw.y)) : a * (1.0 - LINE_PULL);
    vec3 eb = parallel ? vec3(b.xy, b.z + LINE_PULL * (u_draw.z - u_draw.y)) : b * (1.0 - LINE_PULL);
    float w = viewport.x;
    float h = viewport.y;
    vec4 ca = clip_from_eye * vec4(ea, 1.0);
    vec4 cb = clip_from_eye * vec4(eb, 1.0);
    float ia = 1.0 / ca.w;
    float ib = 1.0 / cb.w;
    vec2 pa = vec2((ca.x * ia + 1.0) * 0.5 * w, (1.0 - ca.y * ia) * 0.5 * h);
    vec2 pb = vec2((cb.x * ib + 1.0) * 0.5 * w, (1.0 - cb.y * ib) * 0.5 * h);
    float reach = u_style[1].x * 0.5 + 1.0;
    vec2 d = pb - pa;
    float len = sqrt(dot(d, d));
    vec2 along = len > 0.0 ? d / len : vec2(1.0, 0.0);
    vec2 across = vec2(-along.y, along.x);
    vec2 corner = unit_corners[gl_VertexID];
    vec2 end = corner.x < 0.5 ? pa - along * reach : pb + along * reach;
    vec2 pixel = end + across * (corner.y < 0.5 ? -reach : reach);
    // pixels counted from the top, as the target's rows are
    gl_Position = vec4(pixel.x / w * 2.0 - 1.0, pixel.y / h * 2.0 - 1.0, 0.0, 1.0);
    v_a = pa;
    v_b = pb;
    v_depth = vec4(ca.z * ia, cb.z * ib, ia, ib);
    v_ea = ea;
    v_eb = eb;
}
"#;

pub const LINE_FRAG: &str = r#"
uniform vec4 u_style[2];

flat in vec2 v_a;
flat in vec2 v_b;
flat in vec4 v_depth;
flat in vec3 v_ea;
flat in vec3 v_eb;
out vec4 out_color;

void main() {
    vec2 c = gl_FragCoord.xy;
    vec2 d = v_b - v_a;
    float length2 = dot(d, d);
    float len = sqrt(length2);
    float t = length2 > 0.0 ? clamp(((c.x - v_a.x) * d.x + (c.y - v_a.y) * d.y) / length2, 0.0, 1.0) : 0.0;
    vec2 off = v_a + d * t - c;
    float coverage = clamp(u_style[1].x * 0.5 + 0.5 - sqrt(off.x * off.x + off.y * off.y), 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }
    float dash = u_style[1].y;
    if (dash > 0.0) {
        float along = t * len;
        float period = dash + u_style[1].z;
        if (along - floor(along / period) * period >= dash) {
            discard;
        }
    }
    float z = v_depth.x + (v_depth.y - v_depth.x) * t;
    if (z < 0.0 || z > 1.0) {
        discard;
    }
    if (any(notEqual(section, vec4(0.0)))) {
        float wa = (1.0 - t) * v_depth.z;
        float wb = t * v_depth.w;
        vec3 p = (v_ea * wa + v_eb * wb) / (wa + wb);
        if (section.x * p.x + section.y * p.y + section.z * p.z + section.w < 0.0) {
            discard;
        }
    }
    vec3 codes = vec3(encode_code(u_style[0].r), encode_code(u_style[0].g), encode_code(u_style[0].b));
    float alpha = coverage * u_style[0].a;
    out_color = vec4(codes * alpha, alpha * 255.0) / 255.0;
    gl_FragDepth = z;
}
"#;

/// The backdrop: one oversized triangle, a vertical ramp in codes.
pub const BACKDROP_VERT: &str = r#"
void main() {
    vec2 uv = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}
"#;

pub const BACKDROP_FRAG: &str = r#"
// the ramp's two ends in codes, and the target's height
uniform vec4 u_ramp[3];
out vec4 out_color;

void main() {
    float t = gl_FragCoord.y / u_ramp[2].y;
    vec4 c = clamp(floor(u_ramp[0] + (u_ramp[1] - u_ramp[0]) * t + 0.5), 0.0, 255.0);
    vec3 rgb = floor((c.rgb * c.a + 127.0) / 255.0);
    out_color = vec4(rgb, c.a) / 255.0;
}
"#;

/// The 2D pass's sprite for a world: the target into the box,
/// premultiplied in, straight out, the box's veil (`v_tex.x`) and the
/// clip's curve on the alpha. It follows `SHARED_FRAG` and the sprite
/// vertex stage of `super::shaders`.
pub const COMPOSITE_FRAG_BODY: &str = r#"
flat in vec4 v_dest;
flat in vec4 v_tex;
out vec4 out_color;
uniform sampler2D atlas;

void main() {
    vec2 p = raster_p();
    vec2 texel = (p - v_dest.xy) * (v_tex.zw / (v_dest.zw - v_dest.xy));
    vec4 ink = texture(atlas, texel / vec2(textureSize(atlas, 0)));
    vec3 rgb = ink.a > 0.0 ? ink.rgb / ink.a : vec3(0.0);
    out_color = vec4(rgb, ink.a * v_tex.x * clip_cov(p));
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::shaders::{PRELUDE_300ES, PRELUDE_330};

    const BODIES: [(&str, &str); 11] = [
        ("WORLD_COMMON", WORLD_COMMON),
        ("MESH_VERT", MESH_VERT),
        ("MESH_FRAG_COMMON", MESH_FRAG_COMMON),
        ("MESH_FRAG", MESH_FRAG),
        ("MESH_FRAG_STEPPED", MESH_FRAG_STEPPED),
        ("SHADOW_VERT", SHADOW_VERT),
        ("SHADOW_FRAG", SHADOW_FRAG),
        ("LINE_VERT", LINE_VERT),
        ("LINE_FRAG", LINE_FRAG),
        ("BACKDROP_FRAG", BACKDROP_FRAG),
        ("COMPOSITE_FRAG_BODY", COMPOSITE_FRAG_BODY),
    ];

    #[test]
    fn no_rust_leaks_into_the_world_shaders() {
        const RUST_ONLY: [&str; 6] = ["pub ", "fn ", "let ", "->", "&str", "r#\""];
        for (name, body) in BODIES.iter().chain([("BACKDROP_VERT", BACKDROP_VERT)].iter()) {
            for (number, line) in body.lines().enumerate() {
                for word in RUST_ONLY {
                    assert!(!line.contains(word), "{name} line {} carries `{word}`: {line}", number + 1);
                }
            }
            assert!(!body.contains("#version"), "{name} names a version of its own");
        }
        for prelude in [PRELUDE_330, PRELUDE_300ES] {
            assert!(prelude.starts_with("#version "));
        }
    }

    #[test]
    fn the_srgb_table_parses_back_to_the_same_bits() {
        let table = srgb_table();
        let values: Vec<f32> = table
            .split_once('(')
            .and_then(|(_, rest)| rest.rsplit_once(')'))
            .map(|(inner, _)| inner.split(", ").map(|v| v.parse::<f32>().expect("a float")).collect())
            .expect("an array");
        assert_eq!(values.len(), 256);
        for (ours, theirs) in values.iter().zip(crate::world::shade::SRGB_DECODE.iter()) {
            assert_eq!(ours.to_bits(), theirs.to_bits());
        }
        // GLSL takes every value as a float literal: a whole number needs its point
        assert!(table.contains("0.0") && table.contains("1.0"));
    }

    #[test]
    fn the_depth_constants_are_the_oracles() {
        assert_eq!("0.99999994".parse::<f32>().unwrap().to_bits(), crate::world::soft::DEPTH_CEILING.to_bits());
        assert_eq!("2.384185791015625e-7".parse::<f32>().unwrap().to_bits(), crate::world::soft::DEPTH_TIE.to_bits());
    }
}
