//! Worlds on Metal: the pass that draws a world frame into its target,
//! before the 2D pass composites the target into the scene.
//!
//! The frame is the core's (`bunny_ui::world::frame`): which draws, in
//! which order, with which instances and lights. This module only moves
//! its bytes and runs its draws, with the shading of the software
//! rasterizer (`bunny_ui::world::shade`) rewritten below in MSL, operation
//! for operation. The parity tests hold the two pictures together.
//!
//! A target is a resolved RGBA8 texture the 2D pass samples. Its
//! multisample colour and its depth are MEMORYLESS where the GPU allows:
//! they live in tile memory for the length of one pass and never touch
//! memory, which is why a world draws in ONE encoder.
//!
//! The world library is compiled the first time a box paints a world: an
//! app that shows none never pays for it.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::rc::Rc;

use bunny_ui::gpu::walk::FrameBatches;
use bunny_ui::gpu::world::{WorldCaps, WorldGround, WorldJob};
use bunny_ui::world::frame::{Background, Blend, DrawKind, WorldFrame};
use bunny_ui::world::shade::SRGB_DECODE;
use bunny_ui::world::wire::{instance_bytes, material_bytes};

use super::{
    LOAD_ACTION_CLEAR, MTLClearColor, MTLOrigin, MTLRegion, MTLSize, PIXEL_FORMAT_RGBA8, PIXEL_FORMAT_RGBA8_SRGB,
    PRIMITIVE_TRIANGLE, RESOURCE_SHARED_WRITE_COMBINED, SHADER_PRELUDE, STORAGE_MODE_PRIVATE, STORAGE_MODE_SHARED,
    STORE_ACTION_STORE, Sels, TEXTURE_USAGE_RENDER_TARGET, TEXTURE_USAGE_SHADER_READ, msg_id, msg_id_arg,
    msg_id_id_id_ptr, msg_id_id_ptr, msg_id_u64, msg_id_u64_u64, msg_id_u64_u64_u64_bool, msg_void, msg_void_bool,
    msg_void_clear_color, msg_void_f64, msg_void_id, msg_void_id_u64, msg_void_id_u64_u64, msg_void_ptr_u64_u64,
    msg_void_region_u64_ptr_u64, msg_void_u64, msg_void_u64x3, msg_void_u64x5,
};
use crate::ffi::{Id, Sel, class, error_message, ns_string, sel};

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    // `drawIndexedPrimitives:indexCount:indexType:indexBuffer:
    // indexBufferOffset:instanceCount:`
    #[link_name = "objc_msgSend"]
    fn msg_void_draw_indexed(obj: Id, sel: Sel, primitive: u64, count: u64, kind: u64, buffer: Id, offset: u64, instances: u64);
    #[link_name = "objc_msgSend"]
    fn msg_bool_u64(obj: Id, sel: Sel, a: u64) -> i8;
}

// MARK: - Metal vocabulary of the world pass

const PIXEL_FORMAT_DEPTH32_FLOAT: u64 = 252;
const STORAGE_MODE_MEMORYLESS: u64 = 3;
const TEXTURE_TYPE_2D_MULTISAMPLE: u64 = 4;
const STORE_ACTION_DONT_CARE: u64 = 0;
const STORE_ACTION_MULTISAMPLE_RESOLVE: u64 = 2;
const COMPARE_LESS: u64 = 1;
const COMPARE_LESS_EQUAL: u64 = 3;
const COMPARE_GREATER: u64 = 4;
const COMPARE_ALWAYS: u64 = 7;
const CULL_NONE: u64 = 0;
const CULL_BACK: u64 = 2;
const WINDING_CLOCKWISE: u64 = 0;
const WINDING_COUNTER_CLOCKWISE: u64 = 1;
const INDEX_UINT32: u64 = 1;
const BLEND_ZERO: u64 = 0;
const BLEND_ONE: u64 = 1;
const BLEND_ONE_MINUS_SOURCE_ALPHA: u64 = 5;
const BLEND_ONE_MINUS_DESTINATION_ALPHA: u64 = 9;
const FILTER_LINEAR: u64 = 1;
const MIP_FILTER_LINEAR: u64 = 2;
const ADDRESS_REPEAT: u64 = 2;
/// `MTLGPUFamilyApple1`: every Apple GPU, the ones with tile memory.
const GPU_FAMILY_APPLE1: u64 = 1001;

/// The bits of the per-draw block the fragment reads.
const DRAW_MASK: u32 = 1;
const DRAW_TEXTURED: u32 = 2;
const DRAW_REPLACE: u32 = 4;

// MARK: - The shaders

/// The world library: the 2D prelude (the sprite, the clip curve), the
/// sRGB table, then this. Every function below mirrors one of
/// `bunny_ui::world::shade` or `soft`, in the same order of operations.
const WORLD_SOURCE: &str = r#"
// MARK: - The wire (bunny_ui::world::wire, textually)

struct WorldUniforms {
    float4x4 clip_from_eye;
    float4 viewport;
    float4 params;
    float4 section;
    float4 sun_direction;
    float4 sun_color;
    float4 sky_color;
    float4 ground_color;
    float4 sky_up;
    float4 tone;
    float4x4 shadow_from_eye;
    float4 shadow;
    float4 counts;
    float4 point_position[8];
    float4 point_color[8];
    uint4 point_groups[2];
};

struct WorldInstance {
    float4 eye_from_model[3];
    float4 normal[3];
    uchar4 tint;
    uint material_base;
    uint entity;
    uint flags;
};

struct WorldMaterial {
    float4 base;
    float4 back;
    float4 emissive;
    float4 params;
};

struct WorldVertex {
    packed_float3 position;
    short2 normal;
    float2 uv;
    uchar4 color;
    uchar material;
    uchar occlusion;
    ushort part;
};

struct WorldSegment {
    packed_float3 a;
    packed_float3 b;
};

struct LineStyle {
    float4 color;
    float4 shape;
};

// what one draw adds: its first instance (the pass has no base
// instance on every tier), its bits, the planes the lines need, and how
// many pixels of its slope an opaque surface steps back under edges
struct WorldDraw {
    uint first_instance;
    uint flags;
    float near;
    float far;
    float4 bias;
};

constant uint DRAW_MASK = 1;
constant uint DRAW_TEXTURED = 2;
constant uint DRAW_REPLACE = 4;
constant uint MATERIAL_UNLIT = 1;
constant uint MATERIAL_MASK = 4;
constant float LINE_PULL = 2.0e-3;
constant float DEPTH_CEILING = as_type<float>(0x3F7FFFFFu);
constant float DEPTH_TIE = 2.384185791015625e-7;
constant uchar BAYER[16] = {0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5};

// MARK: - Bunny Lit v1 (bunny_ui::world::shade)

static float3 safe_normalize(float3 a) {
    float len = sqrt(dot(a, a));
    return len > 0.0 ? a * (1.0 / len) : float3(0.0, 0.0, 1.0);
}

static float exp2_fixed(float x) {
    x = max(x, -126.0);
    float whole = floor(x);
    float f = x - whole;
    float fraction = 0.99999976 + f * (0.6931568 + f * (0.24013178 + f * (0.05587644 + f * (0.00894063 + f * 0.0018943873))));
    float power = as_type<float>(uint(int(whole) + 127) << 23);
    return power * fraction;
}

static float3 hemisphere(float3 direction, float3 up, float3 sky, float3 ground) {
    return ground + (sky - ground) * (0.5 + 0.5 * dot(direction, up));
}

static float3 environment_brdf(float3 f0, float roughness, float n_dot_v) {
    float4 r = float4(1.0 - roughness, roughness * -0.0275 + 0.0425, roughness * -0.572 + 1.04, roughness * 0.022 - 0.04);
    float a004 = min(r.x * r.x, exp2_fixed(-9.28 * n_dot_v)) * r.x + r.y;
    float a = -1.04 * a004 + r.z;
    float b = 1.04 * a004 + r.w;
    return f0 * a + b;
}

static float3 tone(float3 color, float exposure, float knee) {
    float3 c = color * exposure;
    float m = max(max(c.r, c.g), c.b);
    if (m > knee) {
        float f = knee + (1.0 - knee) * (m - knee) / ((1.0 - knee) + (m - knee));
        c = c * (f / m);
        float t = saturate((m - 1.0) / 3.0);
        float bleach = 0.6 * t * t * (3.0 - 2.0 * t);
        c = c + (float3(f) - c) * bleach;
    }
    return saturate(c);
}

static float bayer(uint x, uint y) {
    return (float(BAYER[(y & 3) * 4 + (x & 3)]) + 0.5) / 16.0 - 0.5;
}

// linear light to a continuous sRGB code; the CPU reads the same curve
// from its table, and the two agree to a hundredth of a code
static float encode_code(float linear) {
    if (!(linear > 0.0)) {
        return 0.0;
    }
    if (linear >= 1.0) {
        return 255.0;
    }
    float s = linear <= 0.0031308 ? linear * 12.92 : 1.055 * pow(linear, 1.0 / 2.4) - 0.055;
    return s * 255.0;
}

static float3 lit(constant WorldUniforms& u, float3 position, float3 n, float3 base,
                  float roughness, float metallic, float3 emissive, float occlusion, uint groups) {
    bool parallel = u.params.w > 0.5;
    float3 v = parallel ? float3(0.0, 0.0, 1.0) : safe_normalize(-position);
    float n_dot_v = clamp(dot(n, v), 1e-4, 1.0);
    float metal = saturate(metallic);
    float3 diffuse = base * (1.0 - metal);
    float3 f0 = float3(0.04) + (base - float3(0.04)) * metal;
    float r = clamp(roughness, 0.045, 1.0);
    float a = r * r;
    float a2 = r * r * r * r;
    float3 up = u.sky_up.xyz;
    float3 sky = u.sky_color.xyz;
    float3 ground = u.ground_color.xyz;
    float ao = 1.0 + (occlusion - 1.0) * 0.8;

    float3 light = diffuse * hemisphere(n, up, sky, ground) * ao;
    float3 reflected = n * (2.0 * dot(n, v)) - v;
    light = light + environment_brdf(f0, r, n_dot_v) * hemisphere(reflected, up, sky, ground) * ao;

    if (u.sun_direction.w > 0.5) {
        float3 l = u.sun_direction.xyz;
        float n_dot_l = saturate(dot(n, l));
        if (n_dot_l > 0.0) {
            float3 h = safe_normalize(l + v);
            float n_dot_h = saturate(dot(n, h));
            float v_dot_h = saturate(dot(v, h));
            float q = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
            float pi_d = a2 / (q * q);
            float visibility = 0.5 / max(2.0 * n_dot_l * n_dot_v + (n_dot_l + n_dot_v - 2.0 * n_dot_l * n_dot_v) * a, 1e-5);
            float x = 1.0 - v_dot_h;
            float x5 = x * x * x * x * x;
            float3 fresnel = f0 + (float3(1.0) - f0) * x5;
            float3 specular = fresnel * (pi_d * visibility);
            light = light + (diffuse + specular) * u.sun_color.xyz * n_dot_l;
        }
    }

    uint points = min(uint(u.counts.x), 8u);
    for (uint i = 0; i < points; i++) {
        if ((u.point_groups[i / 4][i % 4] & groups) == 0) {
            continue;
        }
        float4 p = u.point_position[i];
        float3 to = p.xyz - position;
        float d2 = dot(to, to);
        float range2 = p.w * p.w;
        if (d2 >= range2 || range2 <= 0.0) {
            continue;
        }
        float3 l = to * (1.0 / sqrt(max(d2, 1e-8)));
        float w = saturate(1.0 - d2 / range2);
        float r0 = u.point_color[i].w;
        float attenuation = w * w * (r0 * r0) / max(d2 + r0 * r0, 1e-8);
        light = light + diffuse * u.point_color[i].xyz * (attenuation * saturate(dot(n, l)));
    }

    return light + emissive;
}

// MARK: - Surfaces

static float3 unfold_normal(short2 folded) {
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
    return float3(x / len, y / len, z / len);
}

static float3 to_eye(WorldInstance instance, float3 p) {
    float4 r0 = instance.eye_from_model[0];
    float4 r1 = instance.eye_from_model[1];
    float4 r2 = instance.eye_from_model[2];
    return float3(r0.x * p.x + r0.y * p.y + r0.z * p.z + r0.w,
                  r1.x * p.x + r1.y * p.y + r1.z * p.z + r1.w,
                  r2.x * p.x + r2.y * p.y + r2.z * p.z + r2.w);
}

struct WorldVary {
    float4 position [[position]];
    float3 eye;
    float3 normal;
    float2 uv;
    float4 color;
    float occlusion;
    // the depth again, linear on the screen like the depth itself: what
    // a stepped-back surface takes its slope from
    float depth [[center_no_perspective]];
    uint material [[flat]];
    uint instance [[flat]];
};

vertex WorldVary world_vertex(uint vid [[vertex_id]],
                              uint iid [[instance_id]],
                              device const WorldVertex* vertices [[buffer(0)]],
                              device const WorldInstance* instances [[buffer(1)]],
                              constant WorldUniforms& u [[buffer(2)]],
                              constant WorldDraw& draw [[buffer(3)]]) {
    WorldVertex v = vertices[vid];
    uint index = draw.first_instance + iid;
    WorldInstance instance = instances[index];
    float3 eye = to_eye(instance, float3(v.position));
    float3 n = unfold_normal(v.normal);
    WorldVary out;
    out.position = u.clip_from_eye * float4(eye, 1.0);
    out.depth = out.position.z / out.position.w;
    out.eye = eye;
    out.normal = float3(dot(instance.normal[0].xyz, n), dot(instance.normal[1].xyz, n), dot(instance.normal[2].xyz, n));
    out.uv = v.uv;
    out.color = float4(SRGB_DECODE[v.color.r], SRGB_DECODE[v.color.g], SRGB_DECODE[v.color.b], float(v.color.a) / 255.0);
    out.occlusion = float(v.occlusion) / 255.0;
    out.material = v.material;
    out.instance = index;
    return out;
}

// premultiplied codes over 255: the target's unorm write rounds them to
// the byte the CPU computes
static float4 world_shade(WorldVary in, bool front, device const WorldMaterial* materials,
                          device const WorldInstance* instances, constant WorldUniforms& u,
                          constant WorldDraw& draw, texture2d<float> picture, sampler wrap) {
    float4 section = u.section;
    if (any(section != 0.0) && section.x * in.eye.x + section.y * in.eye.y + section.z * in.eye.z + section.w < 0.0) {
        discard_fragment();
    }
    WorldInstance instance = instances[in.instance];
    WorldMaterial material = materials[instance.material_base + in.material];
    uint flags = uint(material.params.w);
    float4 color = in.color;
    if ((draw.flags & DRAW_TEXTURED) != 0) {
        color = color * picture.sample(wrap, in.uv);
    }
    bool two_sided = material.back.w > 0.5;
    float4 surface = (front || !two_sided) ? material.base : float4(material.back.xyz, material.base.w);
    float3 tint = float3(SRGB_DECODE[instance.tint.r], SRGB_DECODE[instance.tint.g], SRGB_DECODE[instance.tint.b]);
    float3 base = surface.rgb * tint * color.rgb;
    float alpha = surface.a * (float(instance.tint.a) / 255.0) * color.a;
    if ((draw.flags & DRAW_MASK) != 0 || (flags & MATERIAL_MASK) != 0) {
        if (alpha < material.params.z) {
            discard_fragment();
        }
        alpha = 1.0;
    }
    if ((draw.flags & DRAW_REPLACE) != 0) {
        alpha = 1.0;
    }
    float3 linear;
    float dither = 0.0;
    if ((flags & MATERIAL_UNLIT) != 0) {
        linear = base;
    } else {
        float3 normal = safe_normalize(in.normal);
        if (!front) {
            normal = -normal;
        }
        float3 light = lit(u, in.eye, normal, base, material.params.x, material.params.y,
                           material.emissive.xyz, in.occlusion, 0xFFFFFFFFu);
        if (u.tone.z > 0.5) {
            dither = bayer(uint(in.position.x), uint(in.position.y));
        }
        linear = tone(light, u.tone.x, u.tone.y);
    }
    float3 code = float3(encode_code(linear.r), encode_code(linear.g), encode_code(linear.b));
    float3 codes = clamp(floor(code * alpha + 0.5 + dither), 0.0, 255.0);
    return float4(codes, floor(alpha * 255.0 + 0.5)) / 255.0;
}

fragment float4 world_fragment(WorldVary in [[stage_in]],
                               bool front [[front_facing]],
                               device const WorldMaterial* materials [[buffer(0)]],
                               device const WorldInstance* instances [[buffer(1)]],
                               constant WorldUniforms& u [[buffer(2)]],
                               constant WorldDraw& draw [[buffer(3)]],
                               texture2d<float> picture [[texture(0)]],
                               sampler wrap [[sampler(0)]]) {
    return world_shade(in, front, materials, instances, u, draw, picture, wrap);
}

struct SteppedBack {
    float4 color [[color(0)]];
    float depth [[depth(greater)]];
};

// an opaque surface under edges steps back by its own slope, short of
// the far plane: an edge drawn on it wins across its whole width
fragment SteppedBack world_fragment_stepped(WorldVary in [[stage_in]],
                                            bool front [[front_facing]],
                                            device const WorldMaterial* materials [[buffer(0)]],
                                            device const WorldInstance* instances [[buffer(1)]],
                                            constant WorldUniforms& u [[buffer(2)]],
                                            constant WorldDraw& draw [[buffer(3)]],
                                            texture2d<float> picture [[texture(0)]],
                                            sampler wrap [[sampler(0)]]) {
    SteppedBack out;
    out.color = world_shade(in, front, materials, instances, u, draw, picture, wrap);
    float z = in.position.z;
    float slope = max(abs(dfdx(in.depth)), abs(dfdy(in.depth)));
    out.depth = min(z + draw.bias.x * slope + DEPTH_TIE, DEPTH_CEILING);
    return out;
}

// MARK: - Lines (bunny_ui::world::soft::line)

struct LineVary {
    float4 position [[position]];
    // the ends on the target, in pixels, y down
    float2 a [[flat]];
    float2 b [[flat]];
    // (depth at a, depth at b, 1/w at a, 1/w at b)
    float4 depth [[flat]];
    // the ends in the eye's space, pulled toward it
    float3 ea [[flat]];
    float3 eb [[flat]];
};

vertex LineVary line_vertex(uint vid [[vertex_id]],
                            uint iid [[instance_id]],
                            device const WorldSegment* segments [[buffer(0)]],
                            device const WorldInstance* instances [[buffer(1)]],
                            constant WorldUniforms& u [[buffer(2)]],
                            constant WorldDraw& draw [[buffer(3)]],
                            constant LineStyle& style [[buffer(4)]]) {
    WorldSegment segment = segments[iid];
    WorldInstance instance = instances[draw.first_instance];
    float3 a = to_eye(instance, float3(segment.a));
    float3 b = to_eye(instance, float3(segment.b));
    bool parallel = u.params.w > 0.5;
    LineVary out;
    if (!parallel) {
        // keep the part in front of the near plane
        float near = -draw.near;
        float da = near - a.z;
        float db = near - b.z;
        if (da < 0.0 && db < 0.0) {
            out.position = float4(0.0, 0.0, -1.0, 1.0);
            return out;
        }
        if (da < 0.0) {
            a = a + (b - a) * (da / (da - db));
        } else if (db < 0.0) {
            b = b + (a - b) * (db / (db - da));
        }
    }
    float3 ea = parallel ? float3(a.xy, a.z + LINE_PULL * (draw.far - draw.near)) : a * (1.0 - LINE_PULL);
    float3 eb = parallel ? float3(b.xy, b.z + LINE_PULL * (draw.far - draw.near)) : b * (1.0 - LINE_PULL);
    float w = u.viewport.x;
    float h = u.viewport.y;
    float4 ca = u.clip_from_eye * float4(ea, 1.0);
    float4 cb = u.clip_from_eye * float4(eb, 1.0);
    float ia = 1.0 / ca.w;
    float ib = 1.0 / cb.w;
    float2 pa = float2((ca.x * ia + 1.0) * 0.5 * w, (1.0 - ca.y * ia) * 0.5 * h);
    float2 pb = float2((cb.x * ib + 1.0) * 0.5 * w, (1.0 - cb.y * ib) * 0.5 * h);
    // a quad along the segment that holds every pixel it covers
    float reach = style.shape.x * 0.5 + 1.0;
    float2 d = pb - pa;
    float len = sqrt(dot(d, d));
    float2 along = len > 0.0 ? d / len : float2(1.0, 0.0);
    float2 across = float2(-along.y, along.x);
    float2 corner = unit_corners[vid];
    float2 end = corner.x < 0.5 ? pa - along * reach : pb + along * reach;
    float2 pixel = end + across * (corner.y < 0.5 ? -reach : reach);
    out.position = float4(pixel.x / w * 2.0 - 1.0, 1.0 - pixel.y / h * 2.0, 0.5, 1.0);
    out.a = pa;
    out.b = pb;
    out.depth = float4(ca.z * ia, cb.z * ib, ia, ib);
    out.ea = ea;
    out.eb = eb;
    return out;
}

struct LineOut {
    float4 color [[color(0)]];
    float depth [[depth(any)]];
};

fragment LineOut line_fragment(LineVary in [[stage_in]],
                               constant WorldUniforms& u [[buffer(2)]],
                               constant LineStyle& style [[buffer(4)]]) {
    float2 c = in.position.xy;
    float2 d = in.b - in.a;
    float length2 = dot(d, d);
    float len = sqrt(length2);
    float t = length2 > 0.0 ? clamp(((c.x - in.a.x) * d.x + (c.y - in.a.y) * d.y) / length2, 0.0, 1.0) : 0.0;
    float2 off = in.a + d * t - c;
    float coverage = saturate(style.shape.x * 0.5 + 0.5 - sqrt(off.x * off.x + off.y * off.y));
    if (coverage <= 0.0) {
        discard_fragment();
    }
    float dash = style.shape.y;
    if (dash > 0.0) {
        float along = t * len;
        float period = dash + style.shape.z;
        if (along - floor(along / period) * period >= dash) {
            discard_fragment();
        }
    }
    float z = in.depth.x + (in.depth.y - in.depth.x) * t;
    if (z < 0.0 || z > 1.0) {
        discard_fragment();
    }
    float4 section = u.section;
    if (any(section != 0.0)) {
        float wa = (1.0 - t) * in.depth.z;
        float wb = t * in.depth.w;
        float3 p = (in.ea * wa + in.eb * wb) / (wa + wb);
        if (section.x * p.x + section.y * p.y + section.z * p.z + section.w < 0.0) {
            discard_fragment();
        }
    }
    float3 codes = float3(encode_code(style.color.r), encode_code(style.color.g), encode_code(style.color.b));
    float alpha = coverage * style.color.a;
    LineOut out;
    out.color = float4(codes * alpha, alpha * 255.0) / 255.0;
    out.depth = z;
    return out;
}

// MARK: - The backdrop (a vertical ramp)

struct WorldFullVary {
    float4 position [[position]];
};

vertex WorldFullVary backdrop_vertex(uint vid [[vertex_id]]) {
    float2 uv = float2(float((vid << 1) & 2), float(vid & 2));
    return WorldFullVary{float4(uv * 2.0 - 1.0, 0.5, 1.0)};
}

struct Backdrop {
    float4 top;
    float4 bottom;
    float4 size;
};

fragment float4 backdrop_fragment(WorldFullVary in [[stage_in]],
                                  constant Backdrop& ramp [[buffer(0)]]) {
    float t = in.position.y / ramp.size.y;
    float4 c = clamp(floor(ramp.top + (ramp.bottom - ramp.top) * t + 0.5), 0.0, 255.0);
    float3 rgb = floor((c.rgb * c.a + 127.0) / 255.0);
    return float4(rgb, c.a) / 255.0;
}

// MARK: - The composite (the 2D pass)

// a world's target into the box: premultiplied in, straight out, the
// box's veil and the clip's curve on the alpha
fragment float4 world_composite_fragment(SpriteVary in [[stage_in]],
                                         device const SpriteInstance* sprites [[buffer(0)]],
                                         constant ClipRound& round [[buffer(1)]],
                                         constant Uniforms& uniforms [[buffer(2)]],
                                         texture2d<float> picture [[texture(0)]]) {
    SpriteInstance sprite = sprites[in.id];
    float2 p = in.position.xy + uniforms.origin;
    float2 texel = (p - sprite.dest.xy) * (sprite.tex.zw / (sprite.dest.zw - sprite.dest.xy));
    float4 ink = picture.sample(live_sampler, texel);
    float3 rgb = ink.a > 0.0 ? ink.rgb / ink.a : float3(0.0);
    return float4(rgb, ink.a * sprite.tex.x * clip_cov(p, round));
}
"#;

/// The sRGB table of the core as an MSL constant, written so that each
/// value parses back to the same bits.
fn srgb_table() -> String {
    let values: Vec<String> = SRGB_DECODE.iter().map(|value| format!("{value:?}")).collect();
    format!("constant float SRGB_DECODE[256] = {{{}}};\n", values.join(", "))
}

// MARK: - Byte blocks of the pass

/// The twin of `WorldDraw` in the shader.
#[repr(C)]
#[derive(Clone, Copy)]
struct DrawBlock {
    first_instance: u32,
    flags: u32,
    near: f32,
    far: f32,
    /// `x`: how many pixels of its slope the surface steps back.
    bias: [f32; 4],
}

const DRAW_BLOCK: u64 = std::mem::size_of::<DrawBlock>() as u64;
const _: () = assert!(DRAW_BLOCK == 32);

/// The twin of `Backdrop` in the shader: codes, then the target's size.
#[repr(C)]
#[derive(Clone, Copy)]
struct BackdropBlock {
    top: [f32; 4],
    bottom: [f32; 4],
    size: [f32; 4],
}

// MARK: - The stack (built on the first world)

/// How a pipeline meets the target.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pipe {
    /// A surface; `true` when it steps back under the frame's edges.
    Mesh(Blend, bool),
    Lines,
    Backdrop,
}

#[derive(Clone, Copy)]
struct Selectors {
    depth_attachment: Sel,
    set_resolve_texture: Sel,
    set_clear_depth: Sel,
    set_depth_state: Sel,
    set_cull_mode: Sel,
    set_winding: Sel,
    set_fragment_sampler: Sel,
    draw_indexed: Sel,
}

struct WorldStack {
    library: Id,
    /// The 2D pass's pipeline for a world's target, bound to the 2D
    /// target's format.
    composite: Id,
    /// By the way they meet the target and the samples a pixel.
    pipelines: HashMap<(Pipe, u8), Id>,
    /// Opaque, see-through, lines tested, lines hidden, always.
    depth_states: [Id; 5],
    sampler: Id,
    /// One white texel: what a draw with no texture binds.
    blank: Id,
    /// Whether the GPU keeps attachments in tile memory.
    memoryless: bool,
    sels: Selectors,
}

const DEPTH_OPAQUE: usize = 0;
const DEPTH_SEE_THROUGH: usize = 1;
const DEPTH_LINES_TESTED: usize = 2;
const DEPTH_LINES_HIDDEN: usize = 3;
const DEPTH_ALWAYS: usize = 4;

enum Built {
    Not,
    Ready(Box<WorldStack>),
    /// The library did not compile: worlds stay on the CPU.
    Failed,
}

/// One world target: the resolved picture, and the storage beside it.
struct Target {
    /// What the 2D pass samples.
    resolve: Id,
    /// The multisample colour, or null at one sample.
    color: Id,
    depth: Id,
    size: (u32, u32),
    samples: u8,
}

struct MeshBuffers {
    vertices: Id,
    indices: Id,
}

/// The Metal side of the world seam: it mints targets, buffers and
/// textures, and draws the walk's jobs.
pub(super) struct MetalWorld {
    device: Id,
    /// The 2D target's pixel format, for the composite pipeline.
    format: u64,
    built: Built,
    targets: HashMap<u64, Target>,
    meshes: HashMap<u64, MeshBuffers>,
    textures: HashMap<u64, Id>,
    /// Each set's buffer and its segment count.
    segments: HashMap<u64, (Id, u32)>,
    next: u64,
    /// Targets the CPU can read: the harness's, for the parity tests.
    pub(super) readable: bool,
}

impl MetalWorld {
    pub(super) fn new(device: Id, format: u64) -> MetalWorld {
        MetalWorld {
            device,
            format,
            built: Built::Not,
            targets: HashMap::new(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            segments: HashMap::new(),
            next: 1,
            readable: false,
        }
    }

    /// The door, once the stack stands: built on the first ask, and never
    /// asked again when it failed.
    pub(super) fn door(&mut self) -> Option<&mut dyn WorldGround> {
        if let Built::Not = self.built {
            self.built = match unsafe { WorldStack::build(self.device, self.format) } {
                Ok(stack) => Built::Ready(Box::new(stack)),
                Err(reason) => {
                    eprintln!("bunny_ui metal: {reason} — worlds draw on the cpu");
                    Built::Failed
                }
            };
        }
        match self.built {
            Built::Ready(_) => Some(self),
            _ => None,
        }
    }

    fn stack(&self) -> Option<&WorldStack> {
        match &self.built {
            Built::Ready(stack) => Some(stack),
            _ => None,
        }
    }

    fn mint(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Every handle a frame's jobs name, resolved, and every pipeline they
    /// need, built: what the encode reads, with nothing left to borrow.
    /// `staged` holds where each job's instances and materials lie in the
    /// slot's staging buffer.
    pub(super) fn prepare(&mut self, batches: &FrameBatches, staged: &[(u64, u64)]) -> FramePass {
        let Built::Ready(stack) = &mut self.built else { return FramePass::default() };
        let composite = stack.composite;
        let mut jobs = Vec::with_capacity(batches.worlds.len());
        for (job, &(instances, materials)) in batches.worlds.iter().zip(staged) {
            let Some(target) = self.targets.get(&job.target) else { continue };
            let samples = target.samples;
            let mut draws = Vec::with_capacity(job.frame.draws.len());
            for draw in &job.frame.draws {
                match draw.kind {
                    DrawKind::Mesh { mesh, first_index, index_count, texture, cull_back, mirrored, blend, mask } => {
                        let Some(buffers) = job.meshes.get(mesh as usize).and_then(|id| self.meshes.get(id)) else { continue };
                        let texture = texture
                            .and_then(|index| job.textures.get(index as usize).copied().flatten())
                            .and_then(|id| self.textures.get(&id).copied());
                        let bias = if blend == Blend::Replace { job.frame.face_bias } else { 0.0 };
                        let pipeline = unsafe { stack.pipeline(self.device, Pipe::Mesh(blend, bias > 0.0), samples) };
                        if pipeline.is_null() {
                            continue;
                        }
                        let mut flags = 0;
                        if mask {
                            flags |= DRAW_MASK;
                        }
                        if texture.is_some() {
                            flags |= DRAW_TEXTURED;
                        }
                        if blend == Blend::Replace {
                            flags |= DRAW_REPLACE;
                        }
                        draws.push(PreparedDraw::Mesh {
                            pipeline,
                            depth: stack.depth_states[if blend == Blend::Replace { DEPTH_OPAQUE } else { DEPTH_SEE_THROUGH }],
                            cull: if cull_back { CULL_BACK } else { CULL_NONE },
                            // the instances mirror the mesh: its front turns the other way
                            winding: if mirrored { WINDING_CLOCKWISE } else { WINDING_COUNTER_CLOCKWISE },
                            vertices: buffers.vertices,
                            indices: buffers.indices,
                            first_index,
                            index_count,
                            instance_count: draw.instance_count,
                            texture: texture.unwrap_or(stack.blank),
                            block: DrawBlock {
                                first_instance: draw.first_instance,
                                flags,
                                near: job.frame.near as f32,
                                far: job.frame.far as f32,
                                bias: [bias, 0.0, 0.0, 0.0],
                            },
                        });
                    }
                    DrawKind::Lines { lines, style } => {
                        let Some(&(buffer, count)) = job.segments.get(lines as usize).and_then(|id| self.segments.get(id)) else { continue };
                        let Some(style) = job.frame.line_styles.get(style as usize) else { continue };
                        if count == 0 {
                            continue;
                        }
                        let pipeline = unsafe { stack.pipeline(self.device, Pipe::Lines, samples) };
                        if pipeline.is_null() {
                            continue;
                        }
                        let depth = match style.shape[3] as u32 {
                            0 => DEPTH_LINES_TESTED,
                            1 => DEPTH_LINES_HIDDEN,
                            _ => DEPTH_ALWAYS,
                        };
                        let mut bytes = [0.0f32; 8];
                        bytes[..4].copy_from_slice(&style.color);
                        bytes[4..].copy_from_slice(&style.shape);
                        draws.push(PreparedDraw::Lines {
                            pipeline,
                            depth: stack.depth_states[depth],
                            segments: buffer,
                            count,
                            style: bytes,
                            block: DrawBlock {
                                first_instance: draw.first_instance,
                                flags: 0,
                                near: job.frame.near as f32,
                                far: job.frame.far as f32,
                                bias: [0.0; 4],
                            },
                        });
                    }
                }
            }
            let (clear, backdrop) = match job.frame.background {
                Background::Transparent => (MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 }, None),
                Background::Color(c) => {
                    let premultiply = |channel: u8| ((channel as u32 * c.a as u32 + 127) / 255) as f64 / 255.0;
                    (MTLClearColor { red: premultiply(c.r), green: premultiply(c.g), blue: premultiply(c.b), alpha: c.a as f64 / 255.0 }, None)
                }
                Background::Gradient { top, bottom } => {
                    let pipeline = unsafe { stack.pipeline(self.device, Pipe::Backdrop, samples) };
                    let codes = |c: bunny_ui::layout::Color| [c.r as f32, c.g as f32, c.b as f32, c.a as f32];
                    let ramp = BackdropBlock { top: codes(top), bottom: codes(bottom), size: [job.size.0 as f32, job.size.1 as f32, 0.0, 0.0] };
                    (MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 }, (!pipeline.is_null()).then_some((pipeline, ramp)))
                }
            };
            jobs.push(PreparedJob {
                resolve: target.resolve,
                color: target.color,
                depth: target.depth,
                clear,
                backdrop,
                backdrop_depth: stack.depth_states[DEPTH_ALWAYS],
                uniforms: job.frame.uniforms.to_bytes(),
                instances,
                materials,
                draws,
                sampler: stack.sampler,
            });
        }
        let targets = batches
            .world_targets
            .iter()
            .map(|id| self.targets.get(id).map_or(null_mut(), |target| target.resolve))
            .collect();
        FramePass { composite, targets, jobs, sels: Some(stack.sels) }
    }
}

impl WorldStack {
    unsafe fn build(device: Id, format: u64) -> Result<WorldStack, String> {
        unsafe {
            let pool = super::objc_autoreleasePoolPush();
            let built = Self::build_inner(device, format);
            super::objc_autoreleasePoolPop(pool);
            built
        }
    }

    unsafe fn build_inner(device: Id, format: u64) -> Result<WorldStack, String> {
        unsafe {
            let source = format!("{SHADER_PRELUDE}{}{WORLD_SOURCE}", srgb_table());
            // the shading is held against the CPU's to a few codes: no
            // reassociation, divisions and roots rounded as written
            let options = msg_id(msg_id(class("MTLCompileOptions"), sel("alloc")), sel("init"));
            msg_void_bool(options, sel("setFastMathEnabled:"), 0);
            let mut error: Id = null_mut();
            let library = msg_id_id_id_ptr(device, sel("newLibraryWithSource:options:error:"), ns_string(&source), options, &mut error);
            msg_void(options, sel("release"));
            if library.is_null() {
                return Err(format!("world shader compile failed: {}", error_message(error)));
            }
            let composite = super::build_pipeline(device, library, "sprite_vertex", "world_composite_fragment", format, true)?;
            let depth_state = |compare: u64, write: bool| {
                let descriptor = msg_id(msg_id(class("MTLDepthStencilDescriptor"), sel("alloc")), sel("init"));
                msg_void_u64(descriptor, sel("setDepthCompareFunction:"), compare);
                msg_void_bool(descriptor, sel("setDepthWriteEnabled:"), write as i8);
                let state = msg_id_arg(device, sel("newDepthStencilStateWithDescriptor:"), descriptor);
                msg_void(descriptor, sel("release"));
                state
            };
            let depth_states = [
                depth_state(COMPARE_LESS, true),
                depth_state(COMPARE_LESS, false),
                depth_state(COMPARE_LESS_EQUAL, false),
                depth_state(COMPARE_GREATER, false),
                depth_state(COMPARE_ALWAYS, false),
            ];
            if depth_states.iter().any(|state| state.is_null()) {
                return Err("no depth state".to_string());
            }
            let sampler_descriptor = msg_id(msg_id(class("MTLSamplerDescriptor"), sel("alloc")), sel("init"));
            msg_void_u64(sampler_descriptor, sel("setMinFilter:"), FILTER_LINEAR);
            msg_void_u64(sampler_descriptor, sel("setMagFilter:"), FILTER_LINEAR);
            msg_void_u64(sampler_descriptor, sel("setMipFilter:"), MIP_FILTER_LINEAR);
            msg_void_u64(sampler_descriptor, sel("setSAddressMode:"), ADDRESS_REPEAT);
            msg_void_u64(sampler_descriptor, sel("setTAddressMode:"), ADDRESS_REPEAT);
            let sampler = msg_id_arg(device, sel("newSamplerStateWithDescriptor:"), sampler_descriptor);
            msg_void(sampler_descriptor, sel("release"));
            let blank = super::shared_texture(device, PIXEL_FORMAT_RGBA8_SRGB, 1, 1);
            if sampler.is_null() || blank.is_null() {
                return Err("no sampler or blank texture".to_string());
            }
            super::upload_texture(blank, 0, 0, 1, 1, &[255, 255, 255, 255], 1);
            // tile memory on an Apple GPU — but not under the simulator,
            // whose Metal has no memoryless storage
            let memoryless = cfg!(not(target_abi = "sim")) && msg_bool_u64(device, sel("supportsFamily:"), GPU_FAMILY_APPLE1) != 0;
            Ok(WorldStack {
                library,
                composite,
                pipelines: HashMap::new(),
                depth_states,
                sampler,
                blank,
                memoryless,
                sels: Selectors {
                    depth_attachment: sel("depthAttachment"),
                    set_resolve_texture: sel("setResolveTexture:"),
                    set_clear_depth: sel("setClearDepth:"),
                    set_depth_state: sel("setDepthStencilState:"),
                    set_cull_mode: sel("setCullMode:"),
                    set_winding: sel("setFrontFacingWinding:"),
                    set_fragment_sampler: sel("setFragmentSamplerState:atIndex:"),
                    draw_indexed: sel("drawIndexedPrimitives:indexCount:indexType:indexBuffer:indexBufferOffset:instanceCount:"),
                },
            })
        }
    }

    /// The pipeline for `pipe` at `samples`, built on its first ask; null
    /// when it cannot be built (the draw is skipped, and says so once).
    unsafe fn pipeline(&mut self, device: Id, pipe: Pipe, samples: u8) -> Id {
        if let Some(&pipeline) = self.pipelines.get(&(pipe, samples)) {
            return pipeline;
        }
        let (vertex, fragment) = match pipe {
            Pipe::Mesh(_, false) => ("world_vertex", "world_fragment"),
            Pipe::Mesh(_, true) => ("world_vertex", "world_fragment_stepped"),
            Pipe::Lines => ("line_vertex", "line_fragment"),
            Pipe::Backdrop => ("backdrop_vertex", "backdrop_fragment"),
        };
        let blend = match pipe {
            Pipe::Mesh(blend, _) => blend,
            Pipe::Lines => Blend::Alpha,
            Pipe::Backdrop => Blend::Replace,
        };
        let pipeline = unsafe { world_pipeline(device, self.library, vertex, fragment, blend, samples) };
        let pipeline = pipeline.unwrap_or_else(|reason| {
            eprintln!("bunny_ui metal: {reason}");
            null_mut()
        });
        self.pipelines.insert((pipe, samples), pipeline);
        pipeline
    }
}

impl Drop for WorldStack {
    fn drop(&mut self) {
        unsafe {
            for id in [self.library, self.composite, self.sampler, self.blank].into_iter().chain(self.depth_states) {
                msg_void(id, sel("release"));
            }
            for pipeline in self.pipelines.values() {
                if !pipeline.is_null() {
                    msg_void(*pipeline, sel("release"));
                }
            }
        }
    }
}

/// A pipeline of the world pass: RGBA8 colour and 32-bit depth at
/// `samples`, blended the way the CPU blends that kind of draw. Colour
/// is premultiplied throughout.
unsafe fn world_pipeline(device: Id, library: Id, vertex: &str, fragment: &str, blend: Blend, samples: u8) -> Result<Id, String> {
    unsafe {
        let vertex_fn = msg_id_arg(library, sel("newFunctionWithName:"), ns_string(vertex));
        let fragment_fn = msg_id_arg(library, sel("newFunctionWithName:"), ns_string(fragment));
        if vertex_fn.is_null() || fragment_fn.is_null() {
            return Err(format!("missing world shader {vertex}/{fragment}"));
        }
        let descriptor = msg_id(msg_id(class("MTLRenderPipelineDescriptor"), sel("alloc")), sel("init"));
        msg_void_id(descriptor, sel("setVertexFunction:"), vertex_fn);
        msg_void_id(descriptor, sel("setFragmentFunction:"), fragment_fn);
        msg_void_u64(descriptor, sel("setDepthAttachmentPixelFormat:"), PIXEL_FORMAT_DEPTH32_FLOAT);
        msg_void_u64(descriptor, sel("setRasterSampleCount:"), samples as u64);
        let attachment = msg_id_u64(msg_id(descriptor, sel("colorAttachments")), sel("objectAtIndexedSubscript:"), 0);
        msg_void_u64(attachment, sel("setPixelFormat:"), PIXEL_FORMAT_RGBA8);
        // over: rgb = s + d·(1−sa), a = sa + da·(1−sa); add: rgb = s + d,
        // a = da + sa·(1−da) — `Target::blend` of the CPU, word for word
        let factors = match blend {
            Blend::Replace => None,
            Blend::Alpha => Some((BLEND_ONE, BLEND_ONE_MINUS_SOURCE_ALPHA, BLEND_ONE, BLEND_ONE_MINUS_SOURCE_ALPHA)),
            Blend::Add => Some((BLEND_ONE, BLEND_ONE, BLEND_ONE_MINUS_DESTINATION_ALPHA, BLEND_ONE)),
        };
        msg_void_bool(attachment, sel("setBlendingEnabled:"), factors.is_some() as i8);
        let (rgb_source, rgb_destination, alpha_source, alpha_destination) = factors.unwrap_or((BLEND_ONE, BLEND_ZERO, BLEND_ONE, BLEND_ZERO));
        msg_void_u64(attachment, sel("setSourceRGBBlendFactor:"), rgb_source);
        msg_void_u64(attachment, sel("setDestinationRGBBlendFactor:"), rgb_destination);
        msg_void_u64(attachment, sel("setSourceAlphaBlendFactor:"), alpha_source);
        msg_void_u64(attachment, sel("setDestinationAlphaBlendFactor:"), alpha_destination);
        let mut error: Id = null_mut();
        let pipeline = msg_id_id_ptr(device, sel("newRenderPipelineStateWithDescriptor:error:"), descriptor, &mut error);
        msg_void(descriptor, sel("release"));
        msg_void(vertex_fn, sel("release"));
        msg_void(fragment_fn, sel("release"));
        if pipeline.is_null() {
            return Err(format!("world pipeline {vertex}/{fragment} failed: {}", error_message(error)));
        }
        Ok(pipeline)
    }
}

/// A texture for one attachment of a target.
unsafe fn attachment(device: Id, format: u64, size: (u32, u32), samples: u8, storage: u64, usage: u64) -> Id {
    unsafe {
        let descriptor = msg_id_u64_u64_u64_bool(
            class("MTLTextureDescriptor"),
            sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
            format,
            size.0 as u64,
            size.1 as u64,
            0,
        );
        if samples > 1 {
            msg_void_u64(descriptor, sel("setTextureType:"), TEXTURE_TYPE_2D_MULTISAMPLE);
            msg_void_u64(descriptor, sel("setSampleCount:"), samples as u64);
        }
        msg_void_u64(descriptor, sel("setStorageMode:"), storage);
        msg_void_u64(descriptor, sel("setUsage:"), usage);
        msg_id_arg(device, sel("newTextureWithDescriptor:"), descriptor)
    }
}

impl WorldGround for MetalWorld {
    fn world_caps(&self) -> WorldCaps {
        WorldCaps { samples: 4 }
    }

    fn make_target(&mut self, width: u32, height: u32, samples: u8) -> Option<u64> {
        let memoryless = self.stack()?.memoryless;
        let size = (width, height);
        let transient = if memoryless { STORAGE_MODE_MEMORYLESS } else { STORAGE_MODE_PRIVATE };
        let resolve_storage = if self.readable { STORAGE_MODE_SHARED } else { STORAGE_MODE_PRIVATE };
        let target = unsafe {
            let pool = super::objc_autoreleasePoolPush();
            let resolve = attachment(self.device, PIXEL_FORMAT_RGBA8, size, 1, resolve_storage, TEXTURE_USAGE_RENDER_TARGET | TEXTURE_USAGE_SHADER_READ);
            // tile memory where the GPU has it; a device that refuses it
            // gets ordinary private storage
            let transient_of = |format: u64| {
                let texture = attachment(self.device, format, size, samples, transient, TEXTURE_USAGE_RENDER_TARGET);
                if texture.is_null() && transient != STORAGE_MODE_PRIVATE {
                    attachment(self.device, format, size, samples, STORAGE_MODE_PRIVATE, TEXTURE_USAGE_RENDER_TARGET)
                } else {
                    texture
                }
            };
            let color = if samples > 1 { transient_of(PIXEL_FORMAT_RGBA8) } else { null_mut() };
            let depth = transient_of(PIXEL_FORMAT_DEPTH32_FLOAT);
            super::objc_autoreleasePoolPop(pool);
            Target { resolve, color, depth, size, samples }
        };
        if target.resolve.is_null() || target.depth.is_null() || (samples > 1 && target.color.is_null()) {
            release_target(&target);
            return None;
        }
        let id = self.mint();
        self.targets.insert(id, target);
        Some(id)
    }

    fn drop_target(&mut self, id: u64) {
        if let Some(target) = self.targets.remove(&id) {
            release_target(&target);
        }
    }

    fn make_mesh(&mut self, vertices: &[u8], indices: &[u8]) -> Option<u64> {
        if vertices.is_empty() || indices.is_empty() {
            return None;
        }
        let (vertices, indices) = unsafe { (filled_buffer(self.device, vertices), filled_buffer(self.device, indices)) };
        if vertices.is_null() || indices.is_null() {
            unsafe {
                for buffer in [vertices, indices] {
                    if !buffer.is_null() {
                        msg_void(buffer, sel("release"));
                    }
                }
            }
            return None;
        }
        let id = self.mint();
        self.meshes.insert(id, MeshBuffers { vertices, indices });
        Some(id)
    }

    fn drop_mesh(&mut self, id: u64) {
        if let Some(mesh) = self.meshes.remove(&id) {
            // a frame in flight keeps what it reads: the command buffer
            // holds its own references
            unsafe {
                msg_void(mesh.vertices, sel("release"));
                msg_void(mesh.indices, sel("release"));
            }
        }
    }

    fn make_texture(&mut self, width: u32, height: u32, levels: &[&[u8]]) -> Option<u64> {
        if levels.is_empty() {
            return None;
        }
        let texture = unsafe {
            let descriptor = msg_id_u64_u64_u64_bool(
                class("MTLTextureDescriptor"),
                sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
                PIXEL_FORMAT_RGBA8_SRGB,
                width as u64,
                height as u64,
                (levels.len() > 1) as i8,
            );
            msg_void_u64(descriptor, sel("setMipmapLevelCount:"), levels.len() as u64);
            msg_void_u64(descriptor, sel("setUsage:"), TEXTURE_USAGE_SHADER_READ);
            msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_SHARED);
            msg_id_arg(self.device, sel("newTextureWithDescriptor:"), descriptor)
        };
        if texture.is_null() {
            return None;
        }
        let (mut w, mut h) = (width, height);
        for (level, bytes) in levels.iter().enumerate() {
            if bytes.len() < (w * h * 4) as usize {
                unsafe { msg_void(texture, sel("release")) };
                return None;
            }
            unsafe {
                msg_void_region_u64_ptr_u64(
                    texture,
                    sel("replaceRegion:mipmapLevel:withBytes:bytesPerRow:"),
                    MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: w as u64, height: h as u64, depth: 1 } },
                    level as u64,
                    bytes.as_ptr() as *const c_void,
                    (w * 4) as u64,
                );
            }
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let id = self.mint();
        self.textures.insert(id, texture);
        Some(id)
    }

    fn drop_texture(&mut self, id: u64) {
        if let Some(texture) = self.textures.remove(&id) {
            unsafe { msg_void(texture, sel("release")) };
        }
    }

    fn make_segments(&mut self, segments: &[u8]) -> Option<u64> {
        let count = (segments.len() / bunny_ui::world::wire::SEGMENT_STRIDE) as u32;
        let buffer = if segments.is_empty() { null_mut() } else { unsafe { filled_buffer(self.device, segments) } };
        if count > 0 && buffer.is_null() {
            return None;
        }
        let id = self.mint();
        self.segments.insert(id, (buffer, count));
        Some(id)
    }

    fn drop_segments(&mut self, id: u64) {
        if let Some((buffer, _)) = self.segments.remove(&id)
            && !buffer.is_null()
        {
            unsafe { msg_void(buffer, sel("release")) };
        }
    }
}

fn release_target(target: &Target) {
    unsafe {
        for texture in [target.resolve, target.color, target.depth] {
            if !texture.is_null() {
                msg_void(texture, sel("release"));
            }
        }
    }
}

/// A shared buffer holding `bytes`, written once: the GPU only reads it.
unsafe fn filled_buffer(device: Id, bytes: &[u8]) -> Id {
    unsafe {
        let buffer = msg_id_u64_u64(device, sel("newBufferWithLength:options:"), bytes.len() as u64, RESOURCE_SHARED_WRITE_COMBINED);
        if !buffer.is_null() {
            let contents = msg_id(buffer, sel("contents")) as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), contents, bytes.len());
        }
        buffer
    }
}

impl Drop for MetalWorld {
    fn drop(&mut self) {
        let ids: Vec<u64> = self.targets.keys().copied().collect();
        for id in ids {
            self.drop_target(id);
        }
        let ids: Vec<u64> = self.meshes.keys().copied().collect();
        for id in ids {
            self.drop_mesh(id);
        }
        let ids: Vec<u64> = self.textures.keys().copied().collect();
        for id in ids {
            self.drop_texture(id);
        }
        let ids: Vec<u64> = self.segments.keys().copied().collect();
        for id in ids {
            self.drop_segments(id);
        }
    }
}

// MARK: - The frame's world pass

/// What one frame's world pass reads at encode time: plain handles and
/// offsets, resolved by [`MetalWorld::prepare`].
pub(super) struct FramePass {
    composite: Id,
    /// The resolved target of each world the 2D pass composites, by the
    /// index its run carries.
    targets: Vec<Id>,
    jobs: Vec<PreparedJob>,
    sels: Option<Selectors>,
}

struct PreparedJob {
    resolve: Id,
    color: Id,
    depth: Id,
    clear: MTLClearColor,
    backdrop: Option<(Id, BackdropBlock)>,
    backdrop_depth: Id,
    uniforms: Vec<u8>,
    /// Offsets in the slot's staging buffer.
    instances: u64,
    materials: u64,
    draws: Vec<PreparedDraw>,
    sampler: Id,
}

enum PreparedDraw {
    Mesh {
        pipeline: Id,
        depth: Id,
        cull: u64,
        winding: u64,
        vertices: Id,
        indices: Id,
        first_index: u32,
        index_count: u32,
        instance_count: u32,
        texture: Id,
        block: DrawBlock,
    },
    Lines {
        pipeline: Id,
        depth: Id,
        segments: Id,
        count: u32,
        style: [f32; 8],
        block: DrawBlock,
    },
}

impl Default for FramePass {
    fn default() -> FramePass {
        FramePass { composite: null_mut(), targets: Vec::new(), jobs: Vec::new(), sels: None }
    }
}

impl FramePass {
    /// The composite pipeline and the target of the world run `index`;
    /// `None` when there is nothing to composite.
    pub(super) fn composite(&self, index: u16) -> Option<(Id, Id)> {
        let target = *self.targets.get(index as usize)?;
        (!self.composite.is_null() && !target.is_null()).then_some((self.composite, target))
    }

    /// Draws every job into its target, one pass each, before the 2D pass
    /// reads them. `staging` holds the instances and the materials.
    pub(super) unsafe fn encode(&self, command: Id, sels: &Sels, pass_class: Id, staging: Id) {
        let Some(world) = self.sels else { return };
        for job in &self.jobs {
            unsafe {
                let pass = msg_id(pass_class, sels.render_pass_descriptor);
                let attachment = msg_id_u64(msg_id(pass, sels.color_attachments), sels.object_at, 0);
                if job.color.is_null() {
                    msg_void_id(attachment, sels.set_texture, job.resolve);
                    msg_void_u64(attachment, sels.set_store_action, STORE_ACTION_STORE);
                } else {
                    msg_void_id(attachment, sels.set_texture, job.color);
                    msg_void_id(attachment, world.set_resolve_texture, job.resolve);
                    msg_void_u64(attachment, sels.set_store_action, STORE_ACTION_MULTISAMPLE_RESOLVE);
                }
                msg_void_u64(attachment, sels.set_load_action, LOAD_ACTION_CLEAR);
                msg_void_clear_color(
                    attachment,
                    sels.set_clear_color,
                    MTLClearColor { red: job.clear.red, green: job.clear.green, blue: job.clear.blue, alpha: job.clear.alpha },
                );
                let depth = msg_id(pass, world.depth_attachment);
                msg_void_id(depth, sels.set_texture, job.depth);
                msg_void_u64(depth, sels.set_load_action, LOAD_ACTION_CLEAR);
                msg_void_f64(depth, world.set_clear_depth, 1.0);
                msg_void_u64(depth, sels.set_store_action, STORE_ACTION_DONT_CARE);
                let encoder = msg_id_arg(command, sels.encoder, pass);
                let uniforms = job.uniforms.as_ptr() as *const c_void;
                let length = job.uniforms.len() as u64;
                msg_void_ptr_u64_u64(encoder, sels.set_vertex_bytes, uniforms, length, 2);
                msg_void_ptr_u64_u64(encoder, sels.set_fragment_bytes, uniforms, length, 2);
                if let Some((pipeline, ramp)) = &job.backdrop {
                    msg_void_id(encoder, sels.set_pipeline, *pipeline);
                    msg_void_id(encoder, world.set_depth_state, job.backdrop_depth);
                    msg_void_ptr_u64_u64(encoder, sels.set_fragment_bytes, (ramp as *const BackdropBlock).cast(), std::mem::size_of::<BackdropBlock>() as u64, 0);
                    msg_void_u64x3(encoder, sels.draw_plain, PRIMITIVE_TRIANGLE, 0, 3);
                }
                if !staging.is_null() {
                    msg_void_id_u64_u64(encoder, sels.set_vertex_buffer, staging, job.instances, 1);
                    msg_void_id_u64_u64(encoder, sels.set_fragment_buffer, staging, job.instances, 1);
                    msg_void_id_u64_u64(encoder, sels.set_fragment_buffer, staging, job.materials, 0);
                }
                msg_void_id_u64(encoder, world.set_fragment_sampler, job.sampler, 0);
                for draw in &job.draws {
                    match draw {
                        PreparedDraw::Mesh { pipeline, depth, cull, winding, vertices, indices, first_index, index_count, instance_count, texture, block } => {
                            msg_void_id(encoder, sels.set_pipeline, *pipeline);
                            msg_void_id(encoder, world.set_depth_state, *depth);
                            msg_void_u64(encoder, world.set_cull_mode, *cull);
                            msg_void_u64(encoder, world.set_winding, *winding);
                            msg_void_id_u64_u64(encoder, sels.set_vertex_buffer, *vertices, 0, 0);
                            let block = (block as *const DrawBlock).cast();
                            msg_void_ptr_u64_u64(encoder, sels.set_vertex_bytes, block, DRAW_BLOCK, 3);
                            msg_void_ptr_u64_u64(encoder, sels.set_fragment_bytes, block, DRAW_BLOCK, 3);
                            msg_void_id_u64(encoder, sels.set_fragment_texture, *texture, 0);
                            msg_void_draw_indexed(
                                encoder,
                                world.draw_indexed,
                                PRIMITIVE_TRIANGLE,
                                *index_count as u64,
                                INDEX_UINT32,
                                *indices,
                                *first_index as u64 * 4,
                                *instance_count as u64,
                            );
                        }
                        PreparedDraw::Lines { pipeline, depth, segments, count, style, block } => {
                            msg_void_id(encoder, sels.set_pipeline, *pipeline);
                            msg_void_id(encoder, world.set_depth_state, *depth);
                            msg_void_u64(encoder, world.set_cull_mode, CULL_NONE);
                            msg_void_id_u64_u64(encoder, sels.set_vertex_buffer, *segments, 0, 0);
                            let block = (block as *const DrawBlock).cast();
                            msg_void_ptr_u64_u64(encoder, sels.set_vertex_bytes, block, DRAW_BLOCK, 3);
                            msg_void_ptr_u64_u64(encoder, sels.set_fragment_bytes, block, DRAW_BLOCK, 3);
                            let style = style.as_ptr() as *const c_void;
                            msg_void_ptr_u64_u64(encoder, sels.set_vertex_bytes, style, 32, 4);
                            msg_void_ptr_u64_u64(encoder, sels.set_fragment_bytes, style, 32, 4);
                            msg_void_u64x5(encoder, sels.draw, PRIMITIVE_TRIANGLE, 0, 6, *count as u64, 0);
                        }
                    }
                }
                msg_void(encoder, sels.end_encoding);
            }
        }
    }
}

/// The bytes each job stages: its instances and its materials.
pub(super) fn staged_bytes(job: &WorldJob) -> (Vec<u8>, Vec<u8>) {
    (instance_bytes(&job.frame.instances), material_bytes(&job.frame.materials))
}

/// A frame of its own, for the harness: what the parity tests compare
/// against the CPU's picture of the same frame. Textures decode through
/// `images`, as the walk's do.
pub(super) fn single_job(world: &mut MetalWorld, frame: &Rc<WorldFrame>, images: &dyn bunny_ui::image_engine::ImageEngine) -> Option<WorldJob> {
    let door = world.door()?;
    let target = door.make_target(frame.size.0, frame.size.1, frame.samples.clamp(1, 4))?;
    let mut meshes = Vec::new();
    for mesh in &frame.meshes {
        meshes.push(door.make_mesh(&mesh.vertex_bytes(), &mesh.index_bytes())?);
    }
    let mut segments = Vec::new();
    for lines in &frame.lines {
        segments.push(door.make_segments(&bunny_ui::world::wire::segment_bytes(lines.segments()))?);
    }
    let mut textures = Vec::new();
    for source in &frame.textures {
        textures.push(bunny_ui::world::texture::texture_of(images, source).and_then(|texture| {
            let levels: Vec<&[u8]> = texture.levels.iter().map(|level| level.rgba.as_slice()).collect();
            door.make_texture(texture.levels[0].width, texture.levels[0].height, &levels)
        }));
    }
    Some(WorldJob { target, size: frame.size, samples: frame.samples.clamp(1, 4), frame: Rc::clone(frame), meshes, textures, segments })
}

/// The resolved picture of a harness target, row by row.
pub(super) fn read_target(world: &MetalWorld, id: u64, sels: &Sels) -> Option<Vec<u8>> {
    let target = world.targets.get(&id)?;
    Some(unsafe { super::read_target(target.resolve, target.size.0 as usize, target.size.1 as usize, sels) })
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use bunny_ui::layout::Color;
    use bunny_ui::world::soft::draw;
    use bunny_ui::world::{Background, Camera, Light, LineDepth, LineStyle, Material, Mesh, Quality, Vec3, World, WorldFrame, WorldStyle, vec3};

    use super::super::OffscreenGpu;

    /// The harness, or `None` on a machine with no Metal device.
    fn harness() -> Option<OffscreenGpu> {
        let gpu = OffscreenGpu::new(8, 8);
        if gpu.is_none() {
            eprintln!("no metal device — skipping");
        }
        gpu
    }

    fn style(quality: Quality) -> WorldStyle {
        WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality, ..WorldStyle::default() }
    }

    /// A box, a ball and a floor under a sun: every term of the shading.
    fn still_life() -> World {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60)));
        world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
        world
            .add(Mesh::sphere(0.45), Material::color(Color::rgb(70, 110, 200)).roughness(0.3).metallic(0.2))
            .set_position(vec3(1.25, -0.05, 0.5));
        world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)));
        world
    }

    fn frame_of(world: &World, camera: &Camera, quality: Quality, size: (u32, u32)) -> Rc<WorldFrame> {
        Rc::new(world.frame(camera, &style(quality), size, 1.0))
    }

    /// How two pictures differ: the worst channel, and how many pixels
    /// sit more than one, three and sixteen codes apart.
    struct Spread {
        worst: u8,
        beyond: [usize; 3],
        pixels: usize,
    }

    fn spread(a: &[u8], b: &[u8]) -> Spread {
        assert_eq!(a.len(), b.len());
        let mut out = Spread { worst: 0, beyond: [0; 3], pixels: a.len() / 4 };
        for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
            let delta = (0..4).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0);
            out.worst = out.worst.max(delta);
            for (slot, limit) in [1u8, 3, 16].iter().enumerate() {
                if delta > *limit {
                    out.beyond[slot] += 1;
                }
            }
        }
        out
    }

    fn share(count: usize, of: usize) -> f64 {
        count as f64 / of.max(1) as f64
    }

    /// Writes both pictures as raw RGBA when `BUNNY_WORLD_DUMP` names a
    /// folder — for a person to look at, never for the test to read.
    fn dump(name: &str, size: (u32, u32), ours: &[u8], oracle: &[u8]) {
        if let Ok(folder) = std::env::var("BUNNY_WORLD_DUMP") {
            let folder = std::path::Path::new(&folder);
            let _ = std::fs::write(folder.join(format!("{name}-metal-{}x{}.rgba", size.0, size.1)), ours);
            let _ = std::fs::write(folder.join(format!("{name}-oracle-{}x{}.rgba", size.0, size.1)), oracle);
        }
    }

    #[test]
    fn a_lit_scene_matches_the_oracle() {
        let Some(mut gpu) = harness() else { return };
        let world = still_life();
        let camera = Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0);
        let frame = frame_of(&world, &camera, Quality::Low, (160, 120));
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        dump("lit", frame.size, &ours, &oracle);
        let s = spread(&ours, &oracle);
        eprintln!("lit scene: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        // the interior agrees to the dither's step; the pixels a triangle
        // edge crosses may fall to either side
        assert!(share(s.beyond[1], s.pixels) <= 0.02, "more than 2% of pixels beyond three codes");
        assert!(share(s.beyond[2], s.pixels) <= 0.01, "more than 1% of pixels beyond sixteen codes");
    }

    #[test]
    fn a_parallel_lens_matches_the_oracle() {
        let Some(mut gpu) = harness() else { return };
        let world = still_life();
        let camera = Camera::orbiting(Vec3::ZERO, -45.0, 35.264, 8.0).parallel();
        let frame = frame_of(&world, &camera, Quality::Low, (128, 96));
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        dump("parallel", frame.size, &ours, &oracle);
        let s = spread(&ours, &oracle);
        eprintln!("parallel: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(share(s.beyond[1], s.pixels) <= 0.02);
        assert!(share(s.beyond[2], s.pixels) <= 0.01);
    }

    #[test]
    fn edges_hidden_lines_and_overlays_match_the_oracle() {
        let Some(mut gpu) = harness() else { return };
        let world = World::new();
        let cube = Mesh::cuboid(1.0, 1.0, 1.0);
        world.add(cube.clone(), Material::color(Color::rgb(245, 244, 240)));
        world.add_lines(cube.edges(30.0), LineStyle::solid(Color::rgb(30, 30, 34), 1.5));
        world.add_lines(cube.edges(30.0), LineStyle::solid(Color::rgb(30, 30, 34), 1.0).dashed(4.0, 3.0).depth(LineDepth::Hidden));
        world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)));
        let camera = Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 4.0);
        let frame = frame_of(&world, &camera, Quality::Low, (128, 128));
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        dump("lines", frame.size, &ours, &oracle);
        let s = spread(&ours, &oracle);
        eprintln!("lines: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(share(s.beyond[1], s.pixels) <= 0.03);
        assert!(share(s.beyond[2], s.pixels) <= 0.015);
    }

    #[test]
    fn edges_on_turning_surfaces_stay_whole_and_hidden_ones_stay_hidden() {
        use bunny_ui::layout::Size;
        use bunny_ui::world::vec2;
        let Some(mut gpu) = harness() else { return };
        let world = World::new();
        world.add(Mesh::plane(16.0, 16.0), Material::color(Color::rgb(228, 224, 216)));
        let outline = [vec2(0.0, 0.0), vec2(2.4, 0.0), vec2(2.4, 2.4), vec2(0.0, 2.4)];
        let hole = [vec2(0.6, 0.6), vec2(1.8, 0.6), vec2(1.8, 1.8), vec2(0.6, 1.8)];
        let ring = Mesh::extrude(&outline, &[&hole], 0.4);
        let at = vec3(1.2, 0.0, -2.8);
        world.add(ring.clone(), Material::color(Color::rgb(246, 244, 238))).set_position(at);
        world.add_lines(ring.edges(30.0), LineStyle::solid(Color::rgb(20, 20, 24), 1.2)).set_position(at);
        let camera = Camera::orbiting(vec3(0.0, 0.6, 0.0), 32.0, 24.0, 9.0);
        let frame = frame_of(&world, &camera, Quality::Low, (480, 280));
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        dump("ring", frame.size, &ours, &oracle);
        let s = spread(&ours, &oracle);
        eprintln!("ring: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(share(s.beyond[2], s.pixels) <= 0.001, "the edges agree with the oracle's");
        let size = Size { width: 480.0, height: 280.0 };
        let ink = |x: f64, y: f64| {
            let i = ((y.floor() as usize) * 480 + x.floor() as usize) * 4;
            ours[i] < 120
        };
        // the near edge of the hole, against a wall that faces away
        let (from, to) = (camera.project(at + vec3(0.6, 0.4, 1.8), size).unwrap(), camera.project(at + vec3(1.8, 0.4, 1.8), size).unwrap());
        for step in 2..39 {
            let t = step as f64 / 40.0;
            assert!(ink(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t), "a gap in the edge at {t}");
        }
        // the edge at the hole's far right corner, behind the ring
        for y in [0.05, 0.15, 0.25] {
            let p = camera.project(at + vec3(1.8, y, 0.6), size).unwrap();
            assert!(!ink(p.x, p.y), "a hidden edge shows at height {y}");
        }
    }

    #[test]
    fn see_through_and_glowing_surfaces_match_the_oracle() {
        let Some(mut gpu) = harness() else { return };
        let world = still_life();
        world.add(Mesh::cuboid(2.4, 0.8, 0.1), Material::color(Color::rgba(90, 160, 220, 110)).blend()).set_position(vec3(0.0, 0.0, 1.2));
        world.add(Mesh::sphere(0.2), Material::unlit(Color::rgb(255, 200, 80)).emissive(Color::rgb(255, 200, 80), 2.0)).set_position(vec3(-1.2, 0.0, 0.6));
        let camera = Camera::orbiting(Vec3::ZERO, 10.0, 20.0, 5.0);
        let frame = frame_of(&world, &camera, Quality::Low, (160, 120));
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        dump("see-through", frame.size, &ours, &oracle);
        let s = spread(&ours, &oracle);
        eprintln!("see-through: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(share(s.beyond[1], s.pixels) <= 0.02);
        assert!(share(s.beyond[2], s.pixels) <= 0.01);
    }

    /// A world in a box on a page: the 2D around it, the clip and the
    /// veil, through the walk.
    fn page(world: World, camera: bunny_ui::prelude::State<Camera>, veil: f64) -> impl bunny_ui::prelude::View {
        use bunny_ui::prelude::*;
        use bunny_ui::world::scene_view;
        vstack((
            text("above"),
            scene_view(world, camera).style(style(Quality::Low)).frame(80.0, 60.0).corner_radius(8.0).clipped().opacity(veil),
        ))
        .padding_length(6.0)
    }

    /// The same page drawn by the window's road on the GPU and by the
    /// CPU, the GPU's own world pass held to the CPU's picture.
    fn both(runtime: &bunny_ui::runtime::Runtime, root: &impl bunny_ui::prelude::View, gpu: &mut OffscreenGpu) -> (Vec<u8>, Vec<u8>) {
        use bunny_ui::image_engine::RawImages;
        use bunny_ui::layout::Size;
        use bunny_ui::prelude::PixelFont;
        let logical = Size { width: 100.0, height: 100.0 };
        let display = runtime.display_frame(root, logical);
        let canvas = Color::rgb(250, 250, 250);
        let cpu = bunny_ui::raster::rasterize_with(&display, 200, 200, 2, canvas, &PixelFont, &RawImages::default()).to_rgba_bytes();
        gpu.present_wait(&display, 2, canvas, &PixelFont, &RawImages::default());
        (gpu.read_rgba(), cpu)
    }

    #[test]
    fn a_world_in_a_page_matches_the_cpu_and_a_held_one_draws_nothing() {
        use bunny_ui::prelude::State;
        if harness().is_none() {
            return;
        }
        let mut gpu = OffscreenGpu::new(200, 200).expect("offscreen gpu");
        let camera = State::new(Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0));
        let world = still_life();
        let runtime = bunny_ui::runtime::Runtime::new();
        let (ours, cpu) = both(&runtime, &page(world, camera, 1.0), &mut gpu);
        assert_eq!(gpu.world_jobs(), 1, "the first frame draws the world");
        dump("page", (200, 200), &ours, &cpu);
        let s = spread(&ours, &cpu);
        eprintln!("page: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(s.worst <= 2, "the composite holds the CPU's picture");
        // the same page again: the target holds the picture
        let (again, _) = both(&runtime, &page(world, camera, 1.0), &mut gpu);
        assert_eq!(gpu.world_jobs(), 0, "a held picture is not drawn again");
        assert_eq!(again, ours);
        // a veil rides the identity: composited thinner, never drawn again
        let (veiled, cpu) = both(&runtime, &page(world, camera, 0.5), &mut gpu);
        assert_eq!(gpu.world_jobs(), 0, "a fade draws no world");
        let s = spread(&veiled, &cpu);
        eprintln!("veiled page: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(s.worst <= 2, "the veil matches the CPU's");
        assert_ne!(veiled, ours);
    }

    /// A checkerboard of `cells` squares a side, in two colours, over a
    /// clear band: every level of its chain differs from the one above.
    fn checker(cells: u32, side: u32) -> bunny_ui::image_engine::ImageSource {
        let mut rgba = Vec::with_capacity((side * side * 4) as usize);
        for y in 0..side {
            for x in 0..side {
                let on = ((x * cells / side) + (y * cells / side)).is_multiple_of(2);
                let clear = y < side / 8;
                let pixel = match (clear, on) {
                    (true, _) => [0, 0, 0, 0],
                    (false, true) => [230, 70, 40, 255],
                    (false, false) => [250, 240, 220, 255],
                };
                rgba.extend_from_slice(&pixel);
            }
        }
        bunny_ui::image_engine::ImageSource::rgba(0xC4EC, (side, side), rgba)
    }

    #[test]
    fn a_textured_floor_at_a_grazing_angle_matches_the_oracle() {
        use bunny_ui::image_engine::RawImages;
        let Some(mut gpu) = harness() else { return };
        let images = RawImages::default();
        let world = World::new();
        // a floor that runs far away: every level of the chain is read
        let floor = Mesh::new(bunny_ui::world::MeshData {
            positions: vec![[-4.0, 0.0, 4.0], [4.0, 0.0, 4.0], [4.0, 0.0, -40.0], [-4.0, 0.0, -40.0]],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            uvs: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 22.0], [0.0, 22.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..bunny_ui::world::MeshData::default()
        });
        world.add(floor, Material::color(Color::WHITE).texture(checker(8, 64)).mask(0.5));
        let camera = Camera::looking_at(vec3(0.0, 1.2, 5.0), vec3(0.0, 0.0, -6.0));
        let frame = frame_of(&world, &camera, Quality::Low, (160, 120));
        let ours = gpu.world_rgba_with(&frame, &images).expect("the world pass");
        let oracle = bunny_ui::world::soft::draw_with(&frame, &|index| bunny_ui::world::texture::texture_of(&images, &frame.textures[index])).rgba;
        dump("textured", frame.size, &ours, &oracle);
        let s = spread(&ours, &oracle);
        eprintln!("textured: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        // the hardware picks its level from 2×2 quads, the CPU from each
        // pixel's neighbours: the far rows may sit a few codes apart, and
        // a cut-out's rim may fall on either side
        assert!(share(s.beyond[1], s.pixels) <= 0.25);
        assert!(share(s.beyond[2], s.pixels) <= 0.03);
    }

    #[test]
    fn four_samples_soften_the_edges_and_keep_the_faces() {
        let Some(mut gpu) = harness() else { return };
        let world = still_life();
        let camera = Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0);
        let frame = frame_of(&world, &camera, Quality::Medium, (160, 120));
        assert_eq!(frame.samples, 4);
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        dump("msaa", frame.size, &ours, &oracle);
        let s = spread(&ours, &oracle);
        eprintln!("4×: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        // the oracle takes one sample: only the edges may move, and they do
        assert!(s.beyond[2] > 0, "multisampling changed no edge");
        assert!(share(s.beyond[2], s.pixels) <= 0.06);
    }
}
