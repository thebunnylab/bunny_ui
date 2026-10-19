//! Worlds on D3D11: the pass that draws a world frame into its target,
//! before the 2D pass composites the target into the scene.
//!
//! The frame is the core's (`bunny_ui::world::frame`): which draws, in
//! which order, with which instances and lights. This module only moves
//! its bytes and runs its draws, with the shading of the software
//! rasterizer (`bunny_ui::world::shade`) rewritten below in HLSL,
//! operation for operation. The parity tests hold the two pictures
//! together.
//!
//! A target is an RGBA8 texture the 2D pass samples. With four samples
//! the drawing goes to a multisample texture that `ResolveSubresource`
//! averages into it; the depth is a 32-bit float texture beside it.
//! Vertices, instances, materials and segments are structured buffers
//! the shaders read by index — the same road the 2D pass takes, with no
//! input layout.
//!
//! A frame whose sun casts draws its casters first, when the box's map
//! does not hold them yet: a pass of depth alone, no pixel stage, into a
//! 16-bit map the surfaces read through a comparing sampler (`t5`, `s2`).
//! A frame without a map binds a texel and a sampler whose comparison
//! always passes.
//!
//! The world shaders are compiled the first time a box paints a world:
//! an app that shows none never pays for them.
//!
//! The pass leaves the 2D pass's ground as it found it: the 2D pass
//! binds its own targets, shaders, buffers and states, and its sampler
//! slot is not touched — the world's sampler lives at `s1`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::rc::Rc;

use bunny_ui::gpu::world::{WorldCaps, WorldGround, WorldJob};
use bunny_ui::image_engine::ImageEngine;
use bunny_ui::world::frame::{Background, Blend, DrawKind, WorldFrame};
use bunny_ui::world::shade::SRGB_DECODE;
use bunny_ui::world::wire::{
    SEGMENT_STRIDE, SHADOW_INSTANCE_STRIDE, instance_bytes, instance_offset, material_bytes, part_bytes, shadow_instance_bytes,
};

use super::{
    ADDRESS_CLAMP, BIND_RENDER_TARGET, BIND_SHADER_RESOURCE, BLEND_INV_SRC_ALPHA, BLEND_ONE, BLEND_OP_ADD, BlendDesc, BlendState, Blob,
    Buffer, BufferDesc, COMPARISON_NEVER, CPU_ACCESS_READ, CULL_NONE, Context, DepthStencilDesc,
    DepthStencilOpDesc, DepthStencilState, Device, Dsv, FILL_SOLID, FILTER_MIN_MAG_MIP_LINEAR, FORMAT_RGBA8,
    FORMAT_RGBA8_SRGB, FORMAT_UNKNOWN, MAP_READ, MAP_WRITE_DISCARD, MISC_BUFFER_STRUCTURED, MappedSubresource,
    PixelShader, RasterizerDesc, RasterizerState, RenderTargetBlendDesc, Rtv, SHADER_SOURCE, SRV_DIMENSION_BUFFER,
    SamplerDesc, SamplerState, SlotBuffer, Srv, SrvDesc, SubresourceData, TOPOLOGY_TRIANGLELIST, Texture2d,
    Texture2dDesc, USAGE_DEFAULT, USAGE_STAGING, VertexShader, Viewport, WorldRuns, blob_bytes, compile_source,
    compiler, make_constant_buffer, make_rtv, make_srv,
};
use crate::ffi::{Com, com_ok};

// MARK: - D3D vocabulary of the world pass (values from the SDK headers)

const FORMAT_D32_FLOAT: u32 = 40;
const FORMAT_R32_UINT: u32 = 42;
const FORMAT_R16_TYPELESS: u32 = 53;
const FORMAT_D16_UNORM: u32 = 55;
const FORMAT_R16_UNORM: u32 = 56;
const SRV_DIMENSION_TEXTURE2D: u32 = 4;
const DSV_DIMENSION_TEXTURE2D: u32 = 3;
const FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT: u32 = 0x94;
const USAGE_IMMUTABLE: u32 = 1;
const BIND_INDEX_BUFFER: u32 = 0x2;
const BIND_DEPTH_STENCIL: u32 = 0x40;
const CLEAR_DEPTH: u32 = 0x1;
const DEPTH_WRITE_ZERO: u32 = 0;
const DEPTH_WRITE_ALL: u32 = 1;
const COMPARISON_LESS: u32 = 2;
const COMPARISON_LESS_EQUAL: u32 = 4;
const COMPARISON_GREATER: u32 = 5;
const COMPARISON_ALWAYS: u32 = 8;
const STENCIL_OP_KEEP: u32 = 1;
const CULL_BACK: u32 = 3;
const BLEND_INV_DEST_ALPHA: u32 = 8;
const ADDRESS_WRAP: u32 = 1;
/// `D3DCOMPILE_IEEE_STRICTNESS`: the shading is held against the CPU's
/// to a few codes — no reassociation, divisions and roots as written.
const COMPILE_IEEE_STRICTNESS: u32 = 1 << 13;

/// The bits of the per-draw block the pixel stage reads.
const DRAW_MASK: u32 = 1;
const DRAW_TEXTURED: u32 = 2;
const DRAW_REPLACE: u32 = 4;

// MARK: - The shaders

/// The world's library: the sRGB table, then this. Every function below
/// mirrors one of `bunny_ui::world::shade` or `soft`, in the same order
/// of operations, and the Metal pass's MSL line for line.
const WORLD_SOURCE: &str = r#"
// MARK: - The wire (bunny_ui::world::wire, textually)

struct WorldUniforms {
    column_major float4x4 clip_from_eye;
    float4 viewport;
    float4 params;
    float4 section;
    float4 sun_direction;
    float4 sun_color;
    float4 sky_color;
    float4 ground_color;
    float4 sky_up;
    float4 tone;
    column_major float4x4 shadow_from_eye;
    float4 shadow;
    float4 counts;
    float4 point_position[8];
    float4 point_color[8];
    uint4 point_groups[2];
    float4 floor_u;
    float4 floor_v;
};

struct WorldInstance {
    float4 eye_from_model[3];
    float4 normal[3];
    uint tint;
    uint material_base;
    uint entity;
    uint flags;
    uint ambient;
    uint part_base;
    uint reserved0;
    uint reserved1;
};

struct WorldMaterial {
    float4 base;
    float4 back;
    float4 emissive;
    float4 params;
};

// 32 bytes: the position, the folded normal (two int16, x low), the uv,
// the colour (four bytes, r low) and the ids (material, occlusion, part)
struct WorldVertex {
    float3 position;
    uint folded;
    float2 uv;
    uint color;
    uint ids;
};

struct WorldSegment {
    float3 a;
    float3 b;
};

// what one draw adds: its first instance (SV_InstanceID restarts at zero
// on every draw), its bits, the planes the lines need, how many pixels
// of its slope an opaque surface steps back under edges, and the style
// of a set of lines
struct WorldDraw {
    uint first_instance;
    uint flags;
    float near_z;
    float far_z;
    float4 bias;
    float4 line_color;
    float4 line_shape;
};

struct Backdrop {
    float4 top;
    float4 bottom;
    float4 size;
};

// every cbuffer owns its register: a stricter compiler refuses two on one
cbuffer WorldBlock : register(b0) {
    WorldUniforms u;
};

cbuffer DrawBlock : register(b1) {
    WorldDraw draw;
};

cbuffer BackdropBlock : register(b2) {
    Backdrop ramp;
};

StructuredBuffer<WorldVertex> vertices : register(t0);
StructuredBuffer<WorldInstance> instances : register(t1);
StructuredBuffer<WorldMaterial> materials : register(t2);
Texture2D<float4> picture : register(t3);
StructuredBuffer<WorldSegment> segments : register(t4);
Texture2D<float> shadow_map : register(t5);
Texture2D<float4> floor_picture : register(t7);
// the frame's part table: a part's tint and, in the top byte, 0 when it hides
StructuredBuffer<uint> parts : register(t8);
SamplerState surface_sampler : register(s1);
SamplerComparisonState shadow_sampler : register(s2);

static const uint DRAW_MASK = 1;
static const uint DRAW_TEXTURED = 2;
static const uint DRAW_REPLACE = 4;
static const uint MATERIAL_UNLIT = 1;
static const uint MATERIAL_MASK = 4;
static const float LINE_PULL = 2.0e-3;
// the largest number below the far plane's 1, and the constant step
// that breaks a surface's ties with the edges on it (2^-22)
static const float DEPTH_CEILING = 0.99999994;
static const float DEPTH_TIE = 2.384185791015625e-7;
static const uint BAYER[16] = {0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5};
static const float2 QUAD_CORNERS[6] = {
    float2(0.0, 0.0), float2(1.0, 0.0), float2(0.0, 1.0),
    float2(0.0, 1.0), float2(1.0, 0.0), float2(1.0, 1.0)
};

// MARK: - Bunny Lit v1 (bunny_ui::world::shade)

float3 safe_normalize(float3 a) {
    float len = sqrt(dot(a, a));
    return len > 0.0 ? a * (1.0 / len) : float3(0.0, 0.0, 1.0);
}

float exp2_fixed(float x) {
    x = max(x, -126.0);
    float whole = floor(x);
    float f = x - whole;
    float fraction = 0.99999976 + f * (0.6931568 + f * (0.24013178 + f * (0.05587644 + f * (0.00894063 + f * 0.0018943873))));
    float power = asfloat(((uint)((int)whole + 127)) << 23);
    return power * fraction;
}

float3 hemisphere(float3 direction, float3 up, float3 sky, float3 ground) {
    return ground + (sky - ground) * (0.5 + 0.5 * dot(direction, up));
}

float3 environment_brdf(float3 f0, float roughness, float n_dot_v) {
    float4 r = float4(1.0 - roughness, roughness * -0.0275 + 0.0425, roughness * -0.572 + 1.04, roughness * 0.022 - 0.04);
    float a004 = min(r.x * r.x, exp2_fixed(-9.28 * n_dot_v)) * r.x + r.y;
    float a = -1.04 * a004 + r.z;
    float b = 1.04 * a004 + r.w;
    return f0 * a + b;
}

float3 tone_curve(float3 color, float exposure, float knee) {
    float3 c = color * exposure;
    float m = max(max(c.r, c.g), c.b);
    if (m > knee) {
        float f = knee + (1.0 - knee) * (m - knee) / ((1.0 - knee) + (m - knee));
        c = c * (f / m);
        float t = saturate((m - 1.0) / 3.0);
        float bleach = 0.6 * t * t * (3.0 - 2.0 * t);
        c = c + (float3(f, f, f) - c) * bleach;
    }
    return saturate(c);
}

float bayer(uint x, uint y) {
    return ((float)BAYER[(y & 3u) * 4u + (x & 3u)] + 0.5) / 16.0 - 0.5;
}

// linear light to a continuous sRGB code; the CPU reads the same curve
// from its table, and the two agree to a hundredth of a code
float encode_code(float shade) {
    if (!(shade > 0.0)) {
        return 0.0;
    }
    if (shade >= 1.0) {
        return 255.0;
    }
    float s = shade <= 0.0031308 ? shade * 12.92 : 1.055 * pow(abs(shade), 1.0 / 2.4) - 0.055;
    return s * 255.0;
}

// how much of the sun reaches a point: its depth from the sun against the
// map's, each lookup four texels compared and filtered by the sampler
float sun_visibility(float3 position, float3 n) {
    float taps = u.shadow.w;
    if (taps < 0.5 || u.sun_direction.w <= 0.5) {
        return 1.0;
    }
    float n_dot_l = saturate(dot(n, u.sun_direction.xyz));
    if (!(n_dot_l > 0.0)) {
        return 1.0;
    }
    float sine = sqrt(max(1.0 - n_dot_l * n_dot_l, 0.0));
    float3 p = position + n * (u.shadow.y * sine);
    float4 s = mul(u.shadow_from_eye, float4(p, 1.0));
    if (!(s.x > 0.0 && s.x < 1.0 && s.y > 0.0 && s.y < 1.0)) {
        return 1.0;
    }
    float reference = clamp(s.z - u.shadow.z, 0.0, 1.0);
    float texel = u.shadow.x;
    if (taps < 1.5) {
        return shadow_map.SampleCmpLevelZero(shadow_sampler, s.xy, reference);
    }
    if (taps < 4.5) {
        float four = 0.0;
        four += shadow_map.SampleCmpLevelZero(shadow_sampler, float2(s.x - 0.5 * texel, s.y - 0.5 * texel), reference);
        four += shadow_map.SampleCmpLevelZero(shadow_sampler, float2(s.x + 0.5 * texel, s.y - 0.5 * texel), reference);
        four += shadow_map.SampleCmpLevelZero(shadow_sampler, float2(s.x - 0.5 * texel, s.y + 0.5 * texel), reference);
        four += shadow_map.SampleCmpLevelZero(shadow_sampler, float2(s.x + 0.5 * texel, s.y + 0.5 * texel), reference);
        return four * 0.25;
    }
    float sum = 0.0;
    [unroll]
    for (int j = -1; j <= 1; j++) {
        [unroll]
        for (int i = -1; i <= 1; i++) {
            sum += shadow_map.SampleCmpLevelZero(shadow_sampler, float2(s.x + (float)i * texel, s.y + (float)j * texel), reference);
        }
    }
    return sum / 9.0;
}

// what the floor map gives a point: its texel from the base level, never
// past the picture's edge; nothing off the map or for a surface that
// reads none
float4 floor_light(uint flags, float3 position) {
    if (u.counts.y < 0.5 || (flags & 2u) == 0u) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float s = u.floor_u.x * position.x + u.floor_u.y * position.y + u.floor_u.z * position.z + u.floor_u.w;
    float t = u.floor_v.x * position.x + u.floor_v.y * position.y + u.floor_v.z * position.z + u.floor_v.w;
    if (!(s >= 0.0 && s <= 1.0 && t >= 0.0 && t <= 1.0)) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    uint width;
    uint height;
    floor_picture.GetDimensions(width, height);
    float2 half_texel = 0.5 / float2((float)width, (float)height);
    return floor_picture.SampleLevel(surface_sampler, clamp(float2(s, t), half_texel, float2(1.0, 1.0) - half_texel), 0.0);
}

float3 lit(float3 position, float3 n, float3 base, float roughness, float metallic, float3 emissive,
           float occlusion, uint groups, float shadow, float4 floor_texel, float3 ambient) {
    bool parallel = u.params.w > 0.5;
    float3 v = parallel ? float3(0.0, 0.0, 1.0) : safe_normalize(-position);
    float n_dot_v = clamp(dot(n, v), 1e-4, 1.0);
    float metal = saturate(metallic);
    float3 diffuse = base * (1.0 - metal);
    float3 f0 = float3(0.04, 0.04, 0.04) + (base - float3(0.04, 0.04, 0.04)) * metal;
    float r = clamp(roughness, 0.045, 1.0);
    float a = r * r;
    float a2 = r * r * r * r;
    float3 up = u.sky_up.xyz;
    float3 sky = u.sky_color.xyz * ambient;
    float3 ground = u.ground_color.xyz * ambient;
    float ao = (1.0 + (occlusion - 1.0) * 0.8) * floor_texel.a;

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
            float3 fresnel = f0 + (float3(1.0, 1.0, 1.0) - f0) * x5;
            float3 specular = fresnel * (pi_d * visibility);
            light = light + (diffuse + specular) * u.sun_color.xyz * (n_dot_l * shadow);
        }
    }

    uint lights = min((uint)u.counts.x, 8u);
    [loop]
    for (uint i = 0; i < lights; i++) {
        if ((u.point_groups[i / 4u][i % 4u] & groups) == 0u) {
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

    light = light + diffuse * floor_texel.rgb;
    return light + emissive;
}

// MARK: - Surfaces

float3 unfold_normal(int2 folded) {
    float fu = max((float)folded.x / 32767.0, -1.0);
    float fv = max((float)folded.y / 32767.0, -1.0);
    float z = 1.0 - abs(fu) - abs(fv);
    float x = fu;
    float y = fv;
    if (z < 0.0) {
        x = (1.0 - abs(fv)) * (fu >= 0.0 ? 1.0 : -1.0);
        y = (1.0 - abs(fu)) * (fv >= 0.0 ? 1.0 : -1.0);
    }
    float len = sqrt(x * x + y * y + z * z);
    return float3(x / len, y / len, z / len);
}

float3 to_eye(WorldInstance instance, float3 p) {
    float4 r0 = instance.eye_from_model[0];
    float4 r1 = instance.eye_from_model[1];
    float4 r2 = instance.eye_from_model[2];
    return float3(r0.x * p.x + r0.y * p.y + r0.z * p.z + r0.w,
                  r1.x * p.x + r1.y * p.y + r1.z * p.z + r1.w,
                  r2.x * p.x + r2.y * p.y + r2.z * p.z + r2.w);
}

struct WorldVary {
    float4 position : SV_Position;
    float3 eye : W_EYE;
    float3 normal : W_NORMAL;
    float2 uv : W_UV;
    float4 color : W_COLOR;
    float occlusion : W_OCCLUSION;
    // the depth again, linear on the screen like the depth itself: what
    // a stepped-back surface takes its slope from
    noperspective float depth : W_DEPTH;
    nointerpolation uint material : W_MATERIAL;
    nointerpolation uint instance : W_INSTANCE;
    // the tint of the vertex's part over the instance's (linear)
    nointerpolation float3 part_tint : W_PART;
};

WorldVary world_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    WorldVertex vx = vertices[vid];
    uint index = draw.first_instance + iid;
    WorldInstance instance = instances[index];
    WorldVary o;
    o.part_tint = float3(1.0, 1.0, 1.0);
    // a part that hides leaves the clip volume with every corner
    if (instance.part_base != 0xFFFFFFFFu) {
        uint part = parts[instance.part_base + (vx.ids >> 16)];
        if ((part >> 24) == 0u) {
            o = (WorldVary)0;
            o.position = float4(0.0, 0.0, 2.0, 1.0);
            return o;
        }
        o.part_tint = float3(SRGB_DECODE[part & 0xFFu], SRGB_DECODE[(part >> 8) & 0xFFu], SRGB_DECODE[(part >> 16) & 0xFFu]);
    }
    float3 eye = to_eye(instance, vx.position);
    int2 folded = int2(asint(vx.folded << 16) >> 16, asint(vx.folded) >> 16);
    float3 n = unfold_normal(folded);
    o.position = mul(u.clip_from_eye, float4(eye, 1.0));
    o.depth = o.position.z / o.position.w;
    o.eye = eye;
    o.normal = float3(dot(instance.normal[0].xyz, n), dot(instance.normal[1].xyz, n), dot(instance.normal[2].xyz, n));
    o.uv = vx.uv;
    o.color = float4(SRGB_DECODE[vx.color & 0xFFu], SRGB_DECODE[(vx.color >> 8) & 0xFFu],
                     SRGB_DECODE[(vx.color >> 16) & 0xFFu], (float)(vx.color >> 24) / 255.0);
    o.occlusion = (float)((vx.ids >> 8) & 0xFFu) / 255.0;
    o.material = vx.ids & 0xFFu;
    o.instance = index;
    return o;
}

// premultiplied codes over 255: the target's unorm write rounds them to
// the byte the CPU computes
float4 world_shade(WorldVary v, bool front) {
    float4 cut = u.section;
    if (any(cut != 0.0) && cut.x * v.eye.x + cut.y * v.eye.y + cut.z * v.eye.z + cut.w < 0.0) {
        discard;
    }
    WorldInstance instance = instances[v.instance];
    WorldMaterial material = materials[instance.material_base + v.material];
    uint flags = (uint)material.params.w;
    // read whether or not the draw is textured: a sample needs every
    // pixel of its quad, and a draw without a picture binds a white one
    float4 texel = picture.Sample(surface_sampler, v.uv);
    float4 color = v.color;
    if ((draw.flags & DRAW_TEXTURED) != 0u) {
        color = color * texel;
    }
    bool two_sided = material.back.w > 0.5;
    float4 surface = (front || !two_sided) ? material.base : float4(material.back.xyz, material.base.w);
    uint tint = instance.tint;
    float3 tinted = float3(SRGB_DECODE[tint & 0xFFu], SRGB_DECODE[(tint >> 8) & 0xFFu], SRGB_DECODE[(tint >> 16) & 0xFFu]) * v.part_tint;
    float3 base = surface.rgb * tinted * color.rgb;
    float alpha = surface.a * ((float)(tint >> 24) / 255.0) * color.a;
    if ((draw.flags & DRAW_MASK) != 0u || (flags & MATERIAL_MASK) != 0u) {
        if (alpha < material.params.z) {
            discard;
        }
        alpha = 1.0;
    }
    if ((draw.flags & DRAW_REPLACE) != 0u) {
        alpha = 1.0;
    }
    float3 shaded;
    float dither = 0.0;
    if ((flags & MATERIAL_UNLIT) != 0u) {
        shaded = base;
    } else {
        float3 normal = safe_normalize(v.normal);
        if (!front) {
            normal = -normal;
        }
        float3 light = lit(v.eye, normal, base, material.params.x, material.params.y,
                           material.emissive.xyz, v.occlusion, 1u << ((instance.flags >> 8) & 31u), sun_visibility(v.eye, normal),
                           floor_light(instance.flags, v.eye),
                           float3(SRGB_DECODE[instance.ambient & 0xFFu], SRGB_DECODE[(instance.ambient >> 8) & 0xFFu],
                                  SRGB_DECODE[(instance.ambient >> 16) & 0xFFu]));
        if (u.tone.z > 0.5) {
            dither = bayer((uint)v.position.x, (uint)v.position.y);
        }
        shaded = tone_curve(light, u.tone.x, u.tone.y);
    }
    float3 code = float3(encode_code(shaded.r), encode_code(shaded.g), encode_code(shaded.b));
    float3 codes = clamp(floor(code * alpha + 0.5 + dither), 0.0, 255.0);
    return float4(codes, floor(alpha * 255.0 + 0.5)) / 255.0;
}

float4 world_fragment(WorldVary v, bool front : SV_IsFrontFace) : SV_Target {
    return world_shade(v, front);
}

// a plain depth, as the GL family writes it: D3D takes a conservative
// one (SV_DepthGreaterEqual) only from a pixel whose position is
// interpolated at its centroid, and the step is measured at the centre
struct SteppedBack {
    float4 color : SV_Target;
    float depth : SV_Depth;
};

// an opaque surface under edges steps back by its own slope, short of
// the far plane: an edge drawn on it wins across its whole width
SteppedBack world_fragment_stepped(WorldVary v, bool front : SV_IsFrontFace) {
    float slope = max(abs(ddx(v.depth)), abs(ddy(v.depth)));
    SteppedBack o;
    o.color = world_shade(v, front);
    o.depth = min(v.position.z + draw.bias.x * slope + DEPTH_TIE, DEPTH_CEILING);
    return o;
}

// MARK: - The sun's shadow map (bunny_ui::world::shadow)

struct ShadowCaster {
    float4 rows[3];
    uint part_base;
    uint pad0;
    uint pad1;
    uint pad2;
};

StructuredBuffer<ShadowCaster> casters : register(t6);

// a caster into the map, placed by its rows: D3D's clip space already
// puts the map's top at its first row, and its depth in [0, 1]
float4 shadow_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) : SV_Position {
    float3 p = vertices[vid].position;
    ShadowCaster caster = casters[draw.first_instance + iid];
    // a part that hides casts nothing
    if (caster.part_base != 0xFFFFFFFFu && (parts[caster.part_base + (vertices[vid].ids >> 16)] >> 24) == 0u) {
        return float4(0.0, 0.0, 2.0, 1.0);
    }
    float4 r0 = caster.rows[0];
    float4 r1 = caster.rows[1];
    float4 r2 = caster.rows[2];
    return float4(r0.x * p.x + r0.y * p.y + r0.z * p.z + r0.w,
                  r1.x * p.x + r1.y * p.y + r1.z * p.z + r1.w,
                  r2.x * p.x + r2.y * p.y + r2.z * p.z + r2.w, 1.0);
}

// MARK: - Lines (bunny_ui::world::soft::line)

struct LineVary {
    float4 position : SV_Position;
    // the ends on the target, in pixels, y down
    nointerpolation float2 a : L_A;
    nointerpolation float2 b : L_B;
    // (depth at a, depth at b, 1/w at a, 1/w at b)
    nointerpolation float4 depth : L_DEPTH;
    // the ends in the eye's space, pulled toward it
    nointerpolation float3 ea : L_EA;
    nointerpolation float3 eb : L_EB;
};

LineVary line_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    WorldSegment segment = segments[iid];
    WorldInstance instance = instances[draw.first_instance];
    float3 a = to_eye(instance, segment.a);
    float3 b = to_eye(instance, segment.b);
    bool parallel = u.params.w > 0.5;
    LineVary o = (LineVary)0;
    if (!parallel) {
        // keep the part in front of the near plane
        float near_plane = -draw.near_z;
        float da = near_plane - a.z;
        float db = near_plane - b.z;
        if (da < 0.0 && db < 0.0) {
            o.position = float4(0.0, 0.0, -1.0, 1.0);
            return o;
        }
        if (da < 0.0) {
            a = a + (b - a) * (da / (da - db));
        } else if (db < 0.0) {
            b = b + (a - b) * (db / (db - da));
        }
    }
    float3 ea = parallel ? float3(a.xy, a.z + LINE_PULL * (draw.far_z - draw.near_z)) : a * (1.0 - LINE_PULL);
    float3 eb = parallel ? float3(b.xy, b.z + LINE_PULL * (draw.far_z - draw.near_z)) : b * (1.0 - LINE_PULL);
    float w = u.viewport.x;
    float h = u.viewport.y;
    float4 ca = mul(u.clip_from_eye, float4(ea, 1.0));
    float4 cb = mul(u.clip_from_eye, float4(eb, 1.0));
    float ia = 1.0 / ca.w;
    float ib = 1.0 / cb.w;
    float2 pa = float2((ca.x * ia + 1.0) * 0.5 * w, (1.0 - ca.y * ia) * 0.5 * h);
    float2 pb = float2((cb.x * ib + 1.0) * 0.5 * w, (1.0 - cb.y * ib) * 0.5 * h);
    // a quad along the segment that holds every pixel it covers
    float reach = draw.line_shape.x * 0.5 + 1.0;
    float2 d = pb - pa;
    float len = sqrt(dot(d, d));
    float2 along = len > 0.0 ? d / len : float2(1.0, 0.0);
    float2 across = float2(-along.y, along.x);
    float2 corner = QUAD_CORNERS[vid];
    float2 end = corner.x < 0.5 ? pa - along * reach : pb + along * reach;
    float2 pixel = end + across * (corner.y < 0.5 ? -reach : reach);
    o.position = float4(pixel.x / w * 2.0 - 1.0, 1.0 - pixel.y / h * 2.0, 0.5, 1.0);
    o.a = pa;
    o.b = pb;
    o.depth = float4(ca.z * ia, cb.z * ib, ia, ib);
    o.ea = ea;
    o.eb = eb;
    return o;
}

struct LineOut {
    float4 color : SV_Target;
    float depth : SV_Depth;
};

LineOut line_fragment(LineVary v) {
    float2 c = v.position.xy;
    float2 d = v.b - v.a;
    float length2 = dot(d, d);
    float len = sqrt(length2);
    float t = length2 > 0.0 ? clamp(((c.x - v.a.x) * d.x + (c.y - v.a.y) * d.y) / length2, 0.0, 1.0) : 0.0;
    float2 off = v.a + d * t - c;
    float coverage = saturate(draw.line_shape.x * 0.5 + 0.5 - sqrt(off.x * off.x + off.y * off.y));
    if (coverage <= 0.0) {
        discard;
    }
    float dash = draw.line_shape.y;
    if (dash > 0.0) {
        float along = t * len;
        float period = dash + draw.line_shape.z;
        if (along - floor(along / period) * period >= dash) {
            discard;
        }
    }
    float z = v.depth.x + (v.depth.y - v.depth.x) * t;
    if (z < 0.0 || z > 1.0) {
        discard;
    }
    float4 cut = u.section;
    if (any(cut != 0.0)) {
        float wa = (1.0 - t) * v.depth.z;
        float wb = t * v.depth.w;
        float3 p = (v.ea * wa + v.eb * wb) / (wa + wb);
        if (cut.x * p.x + cut.y * p.y + cut.z * p.z + cut.w < 0.0) {
            discard;
        }
    }
    float3 codes = float3(encode_code(draw.line_color.r), encode_code(draw.line_color.g), encode_code(draw.line_color.b));
    float alpha = coverage * draw.line_color.a;
    LineOut o;
    o.color = float4(codes * alpha, alpha * 255.0) / 255.0;
    o.depth = z;
    return o;
}

// MARK: - The backdrop (a vertical ramp)

float4 backdrop_vertex(uint vid : SV_VertexID) : SV_Position {
    float2 corner = float2((vid << 1) & 2, vid & 2);
    return float4(corner * 2.0 - 1.0, 0.5, 1.0);
}

float4 backdrop_fragment(float4 position : SV_Position) : SV_Target {
    float t = position.y / ramp.size.y;
    float4 c = clamp(floor(ramp.top + (ramp.bottom - ramp.top) * t + 0.5), 0.0, 255.0);
    float3 rgb = floor((c.rgb * c.a + 127.0) / 255.0);
    return float4(rgb, c.a) / 255.0;
}
"#;

/// The 2D pass's side of a world: its target into the box, premultiplied
/// in, straight out (the 2D blend multiplies by alpha), with the box's
/// veil and the clip's curve on the alpha. Compiled with the 2D library,
/// whose sprite, atlas slot and clip curve it reads.
const COMPOSITE_SOURCE: &str = r#"
float4 world_composite_fragment(SpriteVary vary) : SV_Target {
    SpriteInstance sprite = sprites[vary.id];
    float2 p = vary.position.xy;
    float2 texel = (p - sprite.dest.xy) * (sprite.tex.zw / (sprite.dest.zw - sprite.dest.xy));
    float4 ink = atlas.Sample(source_sampler, texel / sprite.tex.zw);
    float3 rgb = ink.a > 0.0 ? ink.rgb / ink.a : float3(0.0, 0.0, 0.0);
    return float4(rgb, ink.a * sprite.tex.x * clip_cov(p));
}
"#;

/// The sRGB table of the core as an HLSL constant, written so that each
/// value parses back to the same bits.
fn srgb_table() -> String {
    let values: Vec<String> = SRGB_DECODE.iter().map(|value| format!("{value:?}")).collect();
    format!("static const float SRGB_DECODE[256] = {{{}}};\n", values.join(", "))
}

// MARK: - Byte blocks of the pass

/// The twin of `WorldDraw` in the shaders.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DrawBlock {
    first_instance: u32,
    flags: u32,
    near: f32,
    far: f32,
    /// `x`: how many pixels of its slope the surface steps back.
    bias: [f32; 4],
    line_color: [f32; 4],
    line_shape: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<DrawBlock>() == 64);

/// The twin of `Backdrop` in the shaders: codes, then the target's size.
#[repr(C)]
#[derive(Clone, Copy)]
struct BackdropBlock {
    top: [f32; 4],
    bottom: [f32; 4],
    size: [f32; 4],
}

/// `D3D11_DEPTH_STENCIL_VIEW_DESC` of one level of a 2D texture.
#[repr(C)]
struct DsvDesc {
    format: u32,
    view_dimension: u32,
    flags: u32,
    mip_slice: u32,
    _union_pad: [u32; 2],
}

/// The bytes of a plain block.
fn bytes_of<T: Copy>(block: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts((block as *const T).cast::<u8>(), std::mem::size_of::<T>()) }
}

// MARK: - The stack (built on the first world)

const DEPTH_OPAQUE: usize = 0;
const DEPTH_SEE_THROUGH: usize = 1;
const DEPTH_LINES_TESTED: usize = 2;
const DEPTH_LINES_HIDDEN: usize = 3;
const DEPTH_ALWAYS: usize = 4;

struct WorldStack {
    mesh_vs: Com<VertexShader>,
    /// The casters into a shadow map; they draw with no pixel stage.
    shadow_vs: Com<VertexShader>,
    line_vs: Com<VertexShader>,
    backdrop_vs: Com<VertexShader>,
    mesh_ps: Com<PixelShader>,
    stepped_ps: Com<PixelShader>,
    line_ps: Com<PixelShader>,
    backdrop_ps: Com<PixelShader>,
    /// The 2D pass's pixel shader for a world's target.
    composite_ps: Com<PixelShader>,
    /// Opaque, see-through, lines tested, lines hidden, always.
    depth_states: [Com<DepthStencilState>; 5],
    /// By culling (none, back), then by the front's winding
    /// (counter-clockwise, clockwise).
    rasterizers: [Com<RasterizerState>; 4],
    /// Over and add; a replacing draw binds no blend at all.
    over: Com<BlendState>,
    add: Com<BlendState>,
    sampler: Com<SamplerState>,
    /// The shadow map's sampler: four texels compared and filtered.
    compare: Com<SamplerState>,
    /// A texel and a sampler that always answer "lit": what a frame
    /// without a map binds.
    always: Com<SamplerState>,
    lit: (Com<Texture2d>, Com<Srv>),
    /// One white texel: what a draw with no texture binds.
    blank: (Com<Texture2d>, Com<Srv>),
    /// One black, opaque texel: a floor map that adds and takes nothing,
    /// bound while a frame's floor map is not decoded.
    dark: (Com<Texture2d>, Com<Srv>),
    uniforms: Com<Buffer>,
    draw: Com<Buffer>,
    backdrop: Com<Buffer>,
}

enum Built {
    Not,
    Ready(Box<WorldStack>),
    /// A shader did not compile or a state was refused: worlds stay on
    /// the CPU.
    Failed,
}

/// One world target: the resolved picture, and the storage beside it.
struct Target {
    /// What the 2D pass samples, and what one sample draws into.
    resolve: Com<Texture2d>,
    resolve_srv: Com<Srv>,
    resolve_rtv: Com<Rtv>,
    /// The multisample colour and its view, or none at one sample.
    color: Option<(Com<Texture2d>, Com<Rtv>)>,
    _depth: Com<Texture2d>,
    dsv: Com<Dsv>,
    size: (u32, u32),
}

struct MeshBuffers {
    _vertices: Com<Buffer>,
    vertex_srv: Com<Srv>,
    indices: Com<Buffer>,
}

struct Segments {
    /// The buffer and its view, or none for an empty set.
    view: Option<(Com<Buffer>, Com<Srv>)>,
    count: u32,
}

/// A box's shadow map: one 16-bit texture, a depth view to draw the
/// casters through and a view the surfaces read.
struct ShadowMap {
    _texture: Com<Texture2d>,
    dsv: Com<Dsv>,
    srv: Com<Srv>,
    size: u32,
}

/// The D3D11 side of the world seam: it mints targets, buffers and
/// textures, and draws the walk's jobs.
pub(super) struct D3dWorld {
    device: *mut Device,
    context: *mut Context,
    built: Built,
    targets: HashMap<u64, Target>,
    meshes: HashMap<u64, MeshBuffers>,
    textures: HashMap<u64, (Com<Texture2d>, Com<Srv>)>,
    segments: HashMap<u64, Segments>,
    shadow_maps: HashMap<u64, ShadowMap>,
    next: u64,
    /// The frame's instances and materials, written again for each job:
    /// a discard map gives every job a buffer of its own.
    instances: SlotBuffer,
    materials: SlotBuffer,
    /// The rows of a shadow's casters, the same way.
    casters: SlotBuffer,
    /// The frame's part table, the same way.
    parts: SlotBuffer,
}

impl D3dWorld {
    pub(super) fn new(device: *mut Device, context: *mut Context) -> D3dWorld {
        D3dWorld {
            device,
            context,
            built: Built::Not,
            targets: HashMap::new(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            segments: HashMap::new(),
            shadow_maps: HashMap::new(),
            next: 1,
            instances: SlotBuffer::empty(),
            materials: SlotBuffer::empty(),
            casters: SlotBuffer::empty(),
            parts: SlotBuffer::empty(),
        }
    }

    /// The door, once the stack stands: built on the first ask, and never
    /// asked again when it failed.
    pub(super) fn door(&mut self) -> Option<&mut dyn WorldGround> {
        if let Built::Not = self.built {
            self.built = match unsafe { WorldStack::build(self.device) } {
                Ok(stack) => Built::Ready(Box::new(stack)),
                Err(reason) => {
                    eprintln!("bunny_ui d3d: {reason} — worlds draw on the cpu");
                    Built::Failed
                }
            };
        }
        match self.built {
            Built::Ready(_) => Some(self),
            _ => None,
        }
    }

    fn mint(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// What the 2D pass composites the world runs with: the shader, and
    /// the view of each target by the index its run carries.
    pub(super) fn runs(&self, handles: &[u64]) -> WorldRuns {
        let Built::Ready(stack) = &self.built else { return WorldRuns::none() };
        WorldRuns {
            composite: stack.composite_ps.as_ptr(),
            targets: handles.iter().map(|id| self.targets.get(id).map_or(null_mut(), |target| target.resolve_srv.as_ptr())).collect(),
        }
    }

    /// Draws every job into its target. Call it after the walk and before
    /// the 2D pass.
    pub(super) fn encode(&mut self, jobs: &[WorldJob]) {
        let Built::Ready(stack) = &self.built else { return };
        if jobs.is_empty() {
            return;
        }
        let context = self.context;
        unsafe {
            let vtbl = &*(*context).vtbl;
            // a target the 2D pass sampled last frame is drawn into now:
            // its view leaves the slot it was read from first
            let none: *mut Srv = null_mut();
            (vtbl.ps_set_shader_resources)(context, 1, 1, &none);
            (vtbl.ia_set_primitive_topology)(context, TOPOLOGY_TRIANGLELIST);
            let blocks = [stack.uniforms.as_ptr(), stack.draw.as_ptr(), stack.backdrop.as_ptr()];
            (vtbl.vs_set_constant_buffers)(context, 0, 3, blocks.as_ptr());
            (vtbl.ps_set_constant_buffers)(context, 0, 3, blocks.as_ptr());
            let sampler = stack.sampler.as_ptr();
            (vtbl.ps_set_samplers)(context, 1, 1, &sampler);
        }
        for job in jobs {
            self.encode_job(job);
        }
        unsafe {
            let vtbl = &*(*context).vtbl;
            (vtbl.om_set_depth_stencil_state)(context, null_mut(), 0);
            (vtbl.ia_set_index_buffer)(context, null_mut(), FORMAT_R32_UINT, 0);
            let none: *mut Rtv = null_mut();
            (vtbl.om_set_render_targets)(context, 1, &none, null_mut());
        }
    }

    fn encode_job(&mut self, job: &WorldJob) {
        let D3dWorld { device, context, built, targets, meshes, textures, segments, shadow_maps, instances, materials, casters, parts, .. } = self;
        let (device, context) = (*device, *context);
        let Built::Ready(stack) = built else { return };
        let Some(target) = targets.get(&job.target) else { return };
        let frame = &job.frame;
        let map = job.shadow.and_then(|shadow| shadow_maps.get(&shadow.map).map(|map| (shadow, map)));
        // what every draw of the frame reads: the instances and the
        // material table, a discard map each, so a job never waits on the
        // one before it
        let instance_data = instance_bytes(&frame.instances);
        let material_data = material_bytes(&frame.materials);
        // one part at least: the stage's view is never left empty
        let mut part_data = part_bytes(&frame.parts);
        if part_data.is_empty() {
            part_data = vec![255; 4];
        }
        let staged = instances.ensure(device, frame.instances.len(), instance_offset::SIZE)
            && materials.ensure(device, frame.materials.len(), 64)
            && parts.ensure(device, part_data.len() / 4, 4)
            && instances.upload(context, &instance_data)
            && materials.upload(context, &material_data)
            && parts.upload(context, &part_data);
        if !staged {
            return;
        }
        let (width, height) = target.size;
        unsafe {
            let vtbl = &*(*context).vtbl;
            write_constants(context, stack.uniforms.as_ptr(), &frame.uniforms.to_bytes());
            let part_view = parts.srv_ptr();
            (vtbl.vs_set_shader_resources)(context, 8, 1, &part_view);

            // the sun's map first, when it does not hold this frame's casters
            if let (Some((shadow, map)), Some(frame_shadow)) = (map, frame.shadow.as_ref())
                && shadow.draw
            {
                let rows = shadow_instance_bytes(&frame_shadow.instances);
                if casters.ensure(device, frame_shadow.instances.len(), SHADOW_INSTANCE_STRIDE) && casters.upload(context, &rows) {
                    // nothing reads the map while it is drawn
                    let none: *mut Srv = null_mut();
                    (vtbl.ps_set_shader_resources)(context, 5, 1, &none);
                    let no_color: *mut Rtv = null_mut();
                    (vtbl.om_set_render_targets)(context, 1, &no_color, map.dsv.as_ptr().cast());
                    let side = map.size as f32;
                    let port = Viewport { top_left_x: 0.0, top_left_y: 0.0, width: side, height: side, min_depth: 0.0, max_depth: 1.0 };
                    (vtbl.rs_set_viewports)(context, 1, &port);
                    (vtbl.clear_depth_stencil_view)(context, map.dsv.as_ptr(), CLEAR_DEPTH, 1.0, 0);
                    (vtbl.vs_set_shader)(context, stack.shadow_vs.as_ptr(), null(), 0);
                    (vtbl.ps_set_shader)(context, null_mut(), null(), 0);
                    (vtbl.om_set_depth_stencil_state)(context, stack.depth_states[DEPTH_OPAQUE].as_ptr(), 0);
                    (vtbl.rs_set_state)(context, stack.rasterizers[0].as_ptr());
                    (vtbl.om_set_blend_state)(context, null_mut(), null(), 0xFFFF_FFFF);
                    let caster_view = casters.srv_ptr();
                    (vtbl.vs_set_shader_resources)(context, 6, 1, &caster_view);
                    for draw in &frame_shadow.draws {
                        let Some(buffers) = job.meshes.get(draw.mesh as usize).and_then(|id| meshes.get(id)) else { continue };
                        let block = DrawBlock { first_instance: draw.first_instance, ..DrawBlock::default() };
                        write_constants(context, stack.draw.as_ptr(), bytes_of(&block));
                        let vertex_view = buffers.vertex_srv.as_ptr();
                        (vtbl.vs_set_shader_resources)(context, 0, 1, &vertex_view);
                        (vtbl.ia_set_index_buffer)(context, buffers.indices.as_ptr(), FORMAT_R32_UINT, 0);
                        (vtbl.draw_indexed_instanced)(context, draw.index_count, draw.instance_count, draw.first_index, 0, 0);
                    }
                }
            }

            let instance_view = instances.srv_ptr();
            let material_view = materials.srv_ptr();
            (vtbl.vs_set_shader_resources)(context, 1, 1, &instance_view);
            (vtbl.ps_set_shader_resources)(context, 1, 1, &instance_view);
            (vtbl.ps_set_shader_resources)(context, 2, 1, &material_view);

            // the target, cleared to the backdrop
            let rtv = target.color.as_ref().map_or(target.resolve_rtv.as_ptr(), |(_, rtv)| rtv.as_ptr());
            (vtbl.om_set_render_targets)(context, 1, &rtv, target.dsv.as_ptr().cast());
            // the map is read once it is no longer drawn into
            let (shadow_view, shadow_sampler) =
                map.map_or((stack.lit.1.as_ptr(), stack.always.as_ptr()), |(_, map)| (map.srv.as_ptr(), stack.compare.as_ptr()));
            (vtbl.ps_set_shader_resources)(context, 5, 1, &shadow_view);
            (vtbl.ps_set_samplers)(context, 2, 1, &shadow_sampler);
            let floor_view = frame
                .floor
                .and_then(|index| job.textures.get(index as usize).copied().flatten())
                .and_then(|id| textures.get(&id))
                .map_or(stack.dark.1.as_ptr(), |(_, srv)| srv.as_ptr());
            (vtbl.ps_set_shader_resources)(context, 7, 1, &floor_view);
            let port = Viewport {
                top_left_x: 0.0,
                top_left_y: 0.0,
                width: width as f32,
                height: height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            (vtbl.rs_set_viewports)(context, 1, &port);
            let clear = match frame.background {
                Background::Color(c) => {
                    let premultiply = |channel: u8| ((channel as u32 * c.a as u32 + 127) / 255) as f32 / 255.0;
                    [premultiply(c.r), premultiply(c.g), premultiply(c.b), c.a as f32 / 255.0]
                }
                Background::Transparent | Background::Gradient { .. } => [0.0; 4],
            };
            (vtbl.clear_render_target_view)(context, rtv, clear.as_ptr());
            (vtbl.clear_depth_stencil_view)(context, target.dsv.as_ptr(), CLEAR_DEPTH, 1.0, 0);
            if let Background::Gradient { top, bottom } = frame.background {
                let codes = |c: bunny_ui::layout::Color| [c.r as f32, c.g as f32, c.b as f32, c.a as f32];
                let ramp = BackdropBlock { top: codes(top), bottom: codes(bottom), size: [width as f32, height as f32, 0.0, 0.0] };
                write_constants(context, stack.backdrop.as_ptr(), bytes_of(&ramp));
                (vtbl.vs_set_shader)(context, stack.backdrop_vs.as_ptr(), null(), 0);
                (vtbl.ps_set_shader)(context, stack.backdrop_ps.as_ptr(), null(), 0);
                (vtbl.om_set_depth_stencil_state)(context, stack.depth_states[DEPTH_ALWAYS].as_ptr(), 0);
                (vtbl.rs_set_state)(context, stack.rasterizers[0].as_ptr());
                (vtbl.om_set_blend_state)(context, null_mut(), null(), 0xFFFF_FFFF);
                (vtbl.draw_instanced)(context, 3, 1, 0, 0);
            }

            let (near, far) = (frame.near as f32, frame.far as f32);
            for draw in &frame.draws {
                match draw.kind {
                    DrawKind::Mesh { mesh, first_index, index_count, texture, cull_back, mirrored, blend, mask } => {
                        let Some(buffers) = job.meshes.get(mesh as usize).and_then(|id| meshes.get(id)) else { continue };
                        let surface = texture
                            .and_then(|index| job.textures.get(index as usize).copied().flatten())
                            .and_then(|id| textures.get(&id))
                            .map(|(_, srv)| srv.as_ptr());
                        let replace = blend == Blend::Replace;
                        let bias = if replace { frame.face_bias } else { 0.0 };
                        let mut flags = 0;
                        if mask {
                            flags |= DRAW_MASK;
                        }
                        if surface.is_some() {
                            flags |= DRAW_TEXTURED;
                        }
                        if replace {
                            flags |= DRAW_REPLACE;
                        }
                        let block = DrawBlock { first_instance: draw.first_instance, flags, near, far, bias: [bias, 0.0, 0.0, 0.0], ..DrawBlock::default() };
                        write_constants(context, stack.draw.as_ptr(), bytes_of(&block));
                        (vtbl.vs_set_shader)(context, stack.mesh_vs.as_ptr(), null(), 0);
                        let pixel = if bias > 0.0 { &stack.stepped_ps } else { &stack.mesh_ps };
                        (vtbl.ps_set_shader)(context, pixel.as_ptr(), null(), 0);
                        let depth = if replace { DEPTH_OPAQUE } else { DEPTH_SEE_THROUGH };
                        (vtbl.om_set_depth_stencil_state)(context, stack.depth_states[depth].as_ptr(), 0);
                        // the instances mirror the mesh: its front turns the other way
                        (vtbl.rs_set_state)(context, stack.rasterizers[cull_back as usize * 2 + mirrored as usize].as_ptr());
                        let blend_state = match blend {
                            Blend::Replace => null_mut(),
                            Blend::Alpha => stack.over.as_ptr(),
                            Blend::Add => stack.add.as_ptr(),
                        };
                        (vtbl.om_set_blend_state)(context, blend_state, null(), 0xFFFF_FFFF);
                        let vertex_view = buffers.vertex_srv.as_ptr();
                        (vtbl.vs_set_shader_resources)(context, 0, 1, &vertex_view);
                        let picture = surface.unwrap_or(stack.blank.1.as_ptr());
                        (vtbl.ps_set_shader_resources)(context, 3, 1, &picture);
                        (vtbl.ia_set_index_buffer)(context, buffers.indices.as_ptr(), FORMAT_R32_UINT, 0);
                        (vtbl.draw_indexed_instanced)(context, index_count, draw.instance_count, first_index, 0, 0);
                    }
                    DrawKind::Lines { lines, style } => {
                        let Some(set) = job.segments.get(lines as usize).and_then(|id| segments.get(id)) else { continue };
                        let (Some(style), Some((_, view))) = (frame.line_styles.get(style as usize), set.view.as_ref()) else { continue };
                        if set.count == 0 {
                            continue;
                        }
                        let depth = match style.shape[3] as u32 {
                            0 => DEPTH_LINES_TESTED,
                            1 => DEPTH_LINES_HIDDEN,
                            _ => DEPTH_ALWAYS,
                        };
                        let block = DrawBlock {
                            first_instance: draw.first_instance,
                            flags: 0,
                            near,
                            far,
                            bias: [0.0; 4],
                            line_color: style.color,
                            line_shape: style.shape,
                        };
                        write_constants(context, stack.draw.as_ptr(), bytes_of(&block));
                        (vtbl.vs_set_shader)(context, stack.line_vs.as_ptr(), null(), 0);
                        (vtbl.ps_set_shader)(context, stack.line_ps.as_ptr(), null(), 0);
                        (vtbl.om_set_depth_stencil_state)(context, stack.depth_states[depth].as_ptr(), 0);
                        (vtbl.rs_set_state)(context, stack.rasterizers[0].as_ptr());
                        (vtbl.om_set_blend_state)(context, stack.over.as_ptr(), null(), 0xFFFF_FFFF);
                        let segment_view = view.as_ptr();
                        (vtbl.vs_set_shader_resources)(context, 4, 1, &segment_view);
                        (vtbl.draw_instanced)(context, 6, set.count, 0, 0);
                    }
                }
            }

            // the samples into the picture the 2D pass reads
            if let Some((color, _)) = &target.color {
                (vtbl.resolve_subresource)(
                    context,
                    target.resolve.as_ptr().cast(),
                    0,
                    color.as_ptr().cast(),
                    0,
                    FORMAT_RGBA8,
                );
            }
            // the map leaves the slot it was read from: the next shadow
            // pass draws into it
            let none: *mut Srv = null_mut();
            (vtbl.ps_set_shader_resources)(context, 5, 1, &none);
        }
    }

    /// Draws one frame by itself into a target of its own and reads it
    /// back: premultiplied, sRGB-encoded RGBA8, the top row first — what
    /// the CPU's `soft::draw` gives for the same frame. For the parity
    /// harness; `None` when the pass cannot stand.
    pub(super) fn draw_single(&mut self, frame: &Rc<WorldFrame>, images: &dyn ImageEngine) -> Option<Vec<u8>> {
        let job = WorldJob::single(self.door()?, frame, images)?;
        self.encode(std::slice::from_ref(&job));
        let out = self.read_target(job.target);
        job.give_back(self);
        out
    }

    /// The resolved picture of a target, row by row, through a staging
    /// copy the CPU maps.
    fn read_target(&self, id: u64) -> Option<Vec<u8>> {
        let target = self.targets.get(&id)?;
        let (width, height) = (target.size.0 as usize, target.size.1 as usize);
        let desc = Texture2dDesc {
            width: target.size.0,
            height: target.size.1,
            mip_levels: 1,
            array_size: 1,
            format: FORMAT_RGBA8,
            sample_count: 1,
            sample_quality: 0,
            usage: USAGE_STAGING,
            bind_flags: 0,
            cpu_access_flags: CPU_ACCESS_READ,
            misc_flags: 0,
        };
        let staging = unsafe { texture_2d(self.device, &desc, None)? };
        let context = self.context;
        let mut bytes = vec![0u8; width * height * 4];
        unsafe {
            let vtbl = &*(*context).vtbl;
            (vtbl.copy_resource)(context, staging.as_ptr().cast(), target.resolve.as_ptr().cast());
            let mut mapped = MappedSubresource { data: null_mut(), row_pitch: 0, depth_pitch: 0 };
            if !com_ok((vtbl.map)(context, staging.as_ptr().cast(), 0, MAP_READ, 0, &mut mapped)) {
                return None;
            }
            for y in 0..height {
                let from = (mapped.data as *const u8).add(y * mapped.row_pitch as usize);
                std::ptr::copy_nonoverlapping(from, bytes[y * width * 4..].as_mut_ptr(), width * 4);
            }
            (vtbl.unmap)(context, staging.as_ptr().cast(), 0);
        }
        Some(bytes)
    }
}

/// Writes a block of constants through a discard map — the designed
/// per-draw road for D3D11 (renamed buffers, no stall).
unsafe fn write_constants(context: *mut Context, buffer: *mut Buffer, bytes: &[u8]) {
    let mut mapped = MappedSubresource { data: null_mut(), row_pitch: 0, depth_pitch: 0 };
    unsafe {
        let vtbl = &*(*context).vtbl;
        if !com_ok((vtbl.map)(context, buffer.cast(), 0, MAP_WRITE_DISCARD, 0, &mut mapped)) {
            return;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.data as *mut u8, bytes.len());
        (vtbl.unmap)(context, buffer.cast(), 0);
    }
}

/// A 2D texture of `desc`, filled with `levels` when given (one per mip).
unsafe fn texture_2d(device: *mut Device, desc: &Texture2dDesc, levels: Option<&[SubresourceData]>) -> Option<Com<Texture2d>> {
    unsafe {
        let mut texture: *mut Texture2d = null_mut();
        let initial = levels.map_or(null(), <[SubresourceData]>::as_ptr);
        com_ok(((*(*device).vtbl).create_texture_2d)(device, desc, initial, &mut texture))
            .then(|| Com::from_raw(texture))
            .flatten()
    }
}

/// A buffer the GPU only reads, holding `bytes`; `stride` > 0 makes it a
/// structured buffer of that stride.
unsafe fn filled_buffer(device: *mut Device, bytes: &[u8], bind_flags: u32, stride: u32) -> Option<Com<Buffer>> {
    let desc = BufferDesc {
        byte_width: bytes.len() as u32,
        usage: USAGE_IMMUTABLE,
        bind_flags,
        cpu_access_flags: 0,
        misc_flags: if stride > 0 { MISC_BUFFER_STRUCTURED } else { 0 },
        structure_byte_stride: stride,
    };
    let data = SubresourceData { memory: bytes.as_ptr() as *const c_void, pitch: 0, slice_pitch: 0 };
    unsafe {
        let mut buffer: *mut Buffer = null_mut();
        com_ok(((*(*device).vtbl).create_buffer)(device, &desc, &data, &mut buffer))
            .then(|| Com::from_raw(buffer))
            .flatten()
    }
}

/// The view a shader reads `count` elements of a structured buffer by.
unsafe fn buffer_view(device: *mut Device, buffer: &Com<Buffer>, count: u32) -> Option<Com<Srv>> {
    let desc = SrvDesc { format: FORMAT_UNKNOWN, view_dimension: SRV_DIMENSION_BUFFER, first_element: 0, num_elements: count, _union_pad: [0; 2] };
    unsafe {
        let mut srv: *mut Srv = null_mut();
        com_ok(((*(*device).vtbl).create_shader_resource_view)(device, buffer.as_ptr().cast(), &desc, &mut srv))
            .then(|| Com::from_raw(srv))
            .flatten()
    }
}

impl WorldStack {
    unsafe fn build(device: *mut Device) -> Result<WorldStack, String> {
        let compile = compiler().ok_or("no d3dcompiler_47.dll on this system")?;
        let source = format!("{}{WORLD_SOURCE}", srgb_table());
        let composite_source = format!("{SHADER_SOURCE}{COMPOSITE_SOURCE}");
        unsafe {
            let shader = |source: &str, entry: &str, target: &str, flags: u32| -> Result<Com<Blob>, String> {
                compile_source(compile, source, entry, target, null(), flags).ok_or_else(|| format!("world shader {entry} did not compile"))
            };
            let vertex = |entry: &str| -> Result<Com<VertexShader>, String> {
                let blob = shader(&source, entry, "vs_5_0", COMPILE_IEEE_STRICTNESS)?;
                let (bytes, length) = blob_bytes(&blob);
                let mut out: *mut VertexShader = null_mut();
                if !com_ok(((*(*device).vtbl).create_vertex_shader)(device, bytes, length, null_mut(), &mut out)) {
                    return Err(format!("world vertex shader {entry} refused"));
                }
                Com::from_raw(out).ok_or_else(|| format!("null world vertex shader {entry}"))
            };
            let pixel = |source: &str, entry: &str, flags: u32| -> Result<Com<PixelShader>, String> {
                let blob = shader(source, entry, "ps_5_0", flags)?;
                let (bytes, length) = blob_bytes(&blob);
                let mut out: *mut PixelShader = null_mut();
                if !com_ok(((*(*device).vtbl).create_pixel_shader)(device, bytes, length, null_mut(), &mut out)) {
                    return Err(format!("world pixel shader {entry} refused"));
                }
                Com::from_raw(out).ok_or_else(|| format!("null world pixel shader {entry}"))
            };
            let mesh_vs = vertex("world_vertex")?;
            let shadow_vs = vertex("shadow_vertex")?;
            let line_vs = vertex("line_vertex")?;
            let backdrop_vs = vertex("backdrop_vertex")?;
            let mesh_ps = pixel(&source, "world_fragment", COMPILE_IEEE_STRICTNESS)?;
            let stepped_ps = pixel(&source, "world_fragment_stepped", COMPILE_IEEE_STRICTNESS)?;
            let line_ps = pixel(&source, "line_fragment", COMPILE_IEEE_STRICTNESS)?;
            let backdrop_ps = pixel(&source, "backdrop_fragment", COMPILE_IEEE_STRICTNESS)?;
            let composite_ps = pixel(&composite_source, "world_composite_fragment", 0)?;

            let depth_state = |compare: u32, write: bool| -> Result<Com<DepthStencilState>, String> {
                let keep = DepthStencilOpDesc {
                    stencil_fail_op: STENCIL_OP_KEEP,
                    stencil_depth_fail_op: STENCIL_OP_KEEP,
                    stencil_pass_op: STENCIL_OP_KEEP,
                    stencil_func: COMPARISON_ALWAYS,
                };
                let desc = DepthStencilDesc {
                    depth_enable: 1,
                    depth_write_mask: if write { DEPTH_WRITE_ALL } else { DEPTH_WRITE_ZERO },
                    depth_func: compare,
                    stencil_enable: 0,
                    stencil_read_mask: 0xFF,
                    stencil_write_mask: 0xFF,
                    front_face: keep,
                    back_face: keep,
                };
                let mut state: *mut DepthStencilState = null_mut();
                if !com_ok(((*(*device).vtbl).create_depth_stencil_state)(device, &desc, &mut state)) {
                    return Err("depth state refused".to_string());
                }
                Com::from_raw(state).ok_or_else(|| "null depth state".to_string())
            };
            let depth_states = [
                depth_state(COMPARISON_LESS, true)?,
                depth_state(COMPARISON_LESS, false)?,
                depth_state(COMPARISON_LESS_EQUAL, false)?,
                depth_state(COMPARISON_GREATER, false)?,
                depth_state(COMPARISON_ALWAYS, false)?,
            ];

            let rasterizer = |cull: u32, counter_clockwise: bool| -> Result<Com<RasterizerState>, String> {
                let desc = RasterizerDesc {
                    fill_mode: FILL_SOLID,
                    cull_mode: cull,
                    front_counter_clockwise: counter_clockwise as i32,
                    depth_bias: 0,
                    depth_bias_clamp: 0.0,
                    slope_scaled_depth_bias: 0.0,
                    depth_clip_enable: 1,
                    scissor_enable: 0,
                    multisample_enable: 1,
                    antialiased_line_enable: 0,
                };
                let mut state: *mut RasterizerState = null_mut();
                if !com_ok(((*(*device).vtbl).create_rasterizer_state)(device, &desc, &mut state)) {
                    return Err("world rasterizer state refused".to_string());
                }
                Com::from_raw(state).ok_or_else(|| "null world rasterizer state".to_string())
            };
            // the clip space is the frame's, upside up: a front that turns
            // counter-clockwise there is a front here
            let rasterizers = [
                rasterizer(CULL_NONE, true)?,
                rasterizer(CULL_NONE, false)?,
                rasterizer(CULL_BACK, true)?,
                rasterizer(CULL_BACK, false)?,
            ];

            // over: rgb = s + d·(1−sa), a = sa + da·(1−sa); add: rgb = s + d,
            // a = da + sa·(1−da) — `Target::blend` of the CPU, word for word
            let blend_state = |source_rgb: u32, destination_rgb: u32, source_alpha: u32, destination_alpha: u32| -> Result<Com<BlendState>, String> {
                let target = RenderTargetBlendDesc {
                    blend_enable: 1,
                    src_blend: source_rgb,
                    dest_blend: destination_rgb,
                    blend_op: BLEND_OP_ADD,
                    src_blend_alpha: source_alpha,
                    dest_blend_alpha: destination_alpha,
                    blend_op_alpha: BLEND_OP_ADD,
                    write_mask: 0x0F,
                };
                let desc = BlendDesc { alpha_to_coverage: 0, independent_blend: 0, render_target: [target; 8] };
                let mut state: *mut BlendState = null_mut();
                if !com_ok(((*(*device).vtbl).create_blend_state)(device, &desc, &mut state)) {
                    return Err("world blend state refused".to_string());
                }
                Com::from_raw(state).ok_or_else(|| "null world blend state".to_string())
            };
            let over = blend_state(BLEND_ONE, BLEND_INV_SRC_ALPHA, BLEND_ONE, BLEND_INV_SRC_ALPHA)?;
            let add = blend_state(BLEND_ONE, BLEND_ONE, BLEND_INV_DEST_ALPHA, BLEND_ONE)?;

            let sampler_desc = SamplerDesc {
                filter: FILTER_MIN_MAG_MIP_LINEAR,
                address_u: ADDRESS_WRAP,
                address_v: ADDRESS_WRAP,
                address_w: ADDRESS_WRAP,
                mip_lod_bias: 0.0,
                max_anisotropy: 1,
                comparison_func: COMPARISON_NEVER,
                border_color: [0.0; 4],
                min_lod: 0.0,
                max_lod: f32::MAX,
            };
            let mut sampler: *mut SamplerState = null_mut();
            if !com_ok(((*(*device).vtbl).create_sampler_state)(device, &sampler_desc, &mut sampler)) {
                return Err("world sampler refused".to_string());
            }
            let sampler = Com::from_raw(sampler).ok_or("null world sampler")?;
            let comparing = |function: u32| -> Result<Com<SamplerState>, String> {
                let desc = SamplerDesc {
                    filter: FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT,
                    address_u: ADDRESS_CLAMP,
                    address_v: ADDRESS_CLAMP,
                    address_w: ADDRESS_CLAMP,
                    mip_lod_bias: 0.0,
                    max_anisotropy: 1,
                    comparison_func: function,
                    border_color: [0.0; 4],
                    min_lod: 0.0,
                    max_lod: 0.0,
                };
                let mut state: *mut SamplerState = null_mut();
                if !com_ok(((*(*device).vtbl).create_sampler_state)(device, &desc, &mut state)) {
                    return Err("the shadow sampler refused".to_string());
                }
                Com::from_raw(state).ok_or_else(|| "null shadow sampler".to_string())
            };
            let compare = comparing(COMPARISON_LESS_EQUAL)?;
            let always = comparing(COMPARISON_ALWAYS)?;
            let lit_desc = Texture2dDesc {
                width: 1,
                height: 1,
                mip_levels: 1,
                array_size: 1,
                format: FORMAT_R16_UNORM,
                sample_count: 1,
                sample_quality: 0,
                usage: USAGE_IMMUTABLE,
                bind_flags: BIND_SHADER_RESOURCE,
                cpu_access_flags: 0,
                misc_flags: 0,
            };
            let far = [0xFFu8; 2];
            let level = [SubresourceData { memory: far.as_ptr().cast(), pitch: 2, slice_pitch: 0 }];
            let lit = texture_2d(device, &lit_desc, Some(&level)).ok_or("no lit texel")?;
            let lit_srv = make_srv(device, &lit).ok_or("no lit texel view")?;

            let blank_desc = Texture2dDesc {
                width: 1,
                height: 1,
                mip_levels: 1,
                array_size: 1,
                format: FORMAT_RGBA8_SRGB,
                sample_count: 1,
                sample_quality: 0,
                usage: USAGE_IMMUTABLE,
                bind_flags: BIND_SHADER_RESOURCE,
                cpu_access_flags: 0,
                misc_flags: 0,
            };
            let white = [255u8; 4];
            let level = [SubresourceData { memory: white.as_ptr().cast(), pitch: 4, slice_pitch: 0 }];
            let blank = texture_2d(device, &blank_desc, Some(&level)).ok_or("no blank texture")?;
            let blank_srv = make_srv(device, &blank).ok_or("no blank texture view")?;
            let black = [0u8, 0, 0, 255];
            let level = [SubresourceData { memory: black.as_ptr().cast(), pitch: 4, slice_pitch: 0 }];
            let dark = texture_2d(device, &blank_desc, Some(&level)).ok_or("no dark texel")?;
            let dark_srv = make_srv(device, &dark).ok_or("no dark texel view")?;

            Ok(WorldStack {
                mesh_vs,
                shadow_vs,
                line_vs,
                backdrop_vs,
                mesh_ps,
                stepped_ps,
                line_ps,
                backdrop_ps,
                composite_ps,
                depth_states,
                rasterizers,
                over,
                add,
                sampler,
                compare,
                always,
                lit: (lit, lit_srv),
                blank: (blank, blank_srv),
                dark: (dark, dark_srv),
                uniforms: make_constant_buffer(device, bunny_ui::world::wire::frame_offset::SIZE as u32).ok_or("world constants refused")?,
                draw: make_constant_buffer(device, std::mem::size_of::<DrawBlock>() as u32).ok_or("draw constants refused")?,
                backdrop: make_constant_buffer(device, std::mem::size_of::<BackdropBlock>() as u32).ok_or("backdrop constants refused")?,
            })
        }
    }
}

impl WorldGround for D3dWorld {
    fn world_caps(&self) -> WorldCaps {
        WorldCaps { samples: 4 }
    }

    fn make_target(&mut self, width: u32, height: u32, samples: u8) -> Option<u64> {
        if width == 0 || height == 0 {
            return None;
        }
        let device = self.device;
        let target = unsafe {
            let resolve = super::make_target_texture(device, width, height, FORMAT_RGBA8, 1)?;
            let resolve_rtv = make_rtv(device, &resolve, None)?;
            let resolve_srv = make_srv(device, &resolve)?;
            let attachment = |format: u32, bind_flags: u32| Texture2dDesc {
                width,
                height,
                mip_levels: 1,
                array_size: 1,
                format,
                sample_count: samples.max(1) as u32,
                sample_quality: 0,
                usage: USAGE_DEFAULT,
                bind_flags,
                cpu_access_flags: 0,
                misc_flags: 0,
            };
            let color = if samples > 1 {
                let texture = texture_2d(device, &attachment(FORMAT_RGBA8, BIND_RENDER_TARGET), None)?;
                let rtv = make_rtv(device, &texture, None)?;
                Some((texture, rtv))
            } else {
                None
            };
            let depth = texture_2d(device, &attachment(FORMAT_D32_FLOAT, BIND_DEPTH_STENCIL), None)?;
            let mut dsv: *mut Dsv = null_mut();
            if !com_ok(((*(*device).vtbl).create_depth_stencil_view)(device, depth.as_ptr().cast(), null(), &mut dsv)) {
                return None;
            }
            let dsv = Com::from_raw(dsv)?;
            Target { resolve, resolve_srv, resolve_rtv, color, _depth: depth, dsv, size: (width, height) }
        };
        let id = self.mint();
        self.targets.insert(id, target);
        Some(id)
    }

    fn drop_target(&mut self, id: u64) {
        // a frame in flight keeps what it reads: the context holds its
        // own references to what is bound
        self.targets.remove(&id);
    }

    fn make_mesh(&mut self, vertices: &[u8], indices: &[u8]) -> Option<u64> {
        use bunny_ui::world::mesh::VERTEX_STRIDE;
        if vertices.len() < VERTEX_STRIDE || indices.len() < 4 {
            return None;
        }
        let device = self.device;
        let buffers = unsafe {
            let vertex_buffer = filled_buffer(device, vertices, BIND_SHADER_RESOURCE, VERTEX_STRIDE as u32)?;
            let vertex_srv = buffer_view(device, &vertex_buffer, (vertices.len() / VERTEX_STRIDE) as u32)?;
            let indices = filled_buffer(device, indices, BIND_INDEX_BUFFER, 0)?;
            MeshBuffers { _vertices: vertex_buffer, vertex_srv, indices }
        };
        let id = self.mint();
        self.meshes.insert(id, buffers);
        Some(id)
    }

    fn drop_mesh(&mut self, id: u64) {
        self.meshes.remove(&id);
    }

    fn make_texture(&mut self, width: u32, height: u32, levels: &[&[u8]]) -> Option<u64> {
        if levels.is_empty() || width == 0 || height == 0 {
            return None;
        }
        let mut data = Vec::with_capacity(levels.len());
        let (mut w, mut h) = (width, height);
        for bytes in levels {
            if bytes.len() < (w * h * 4) as usize {
                return None;
            }
            data.push(SubresourceData { memory: bytes.as_ptr().cast(), pitch: w * 4, slice_pitch: 0 });
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let desc = Texture2dDesc {
            width,
            height,
            mip_levels: levels.len() as u32,
            array_size: 1,
            format: FORMAT_RGBA8_SRGB,
            sample_count: 1,
            sample_quality: 0,
            usage: USAGE_IMMUTABLE,
            bind_flags: BIND_SHADER_RESOURCE,
            cpu_access_flags: 0,
            misc_flags: 0,
        };
        let texture = unsafe {
            let texture = texture_2d(self.device, &desc, Some(&data))?;
            let srv = make_srv(self.device, &texture)?;
            (texture, srv)
        };
        let id = self.mint();
        self.textures.insert(id, texture);
        Some(id)
    }

    fn drop_texture(&mut self, id: u64) {
        self.textures.remove(&id);
    }

    fn make_segments(&mut self, segments: &[u8]) -> Option<u64> {
        let count = (segments.len() / SEGMENT_STRIDE) as u32;
        let view = if count == 0 {
            None
        } else {
            unsafe {
                let buffer = filled_buffer(self.device, &segments[..count as usize * SEGMENT_STRIDE], BIND_SHADER_RESOURCE, SEGMENT_STRIDE as u32)?;
                let srv = buffer_view(self.device, &buffer, count)?;
                Some((buffer, srv))
            }
        };
        let id = self.mint();
        self.segments.insert(id, Segments { view, count });
        Some(id)
    }

    fn drop_segments(&mut self, id: u64) {
        self.segments.remove(&id);
    }

    fn make_shadow_map(&mut self, size: u32) -> Option<u64> {
        let device = self.device;
        let map = unsafe {
            // one texture behind two views: a 16-bit depth to draw into,
            // the same bits as a value to compare against
            let desc = Texture2dDesc {
                width: size,
                height: size,
                mip_levels: 1,
                array_size: 1,
                format: FORMAT_R16_TYPELESS,
                sample_count: 1,
                sample_quality: 0,
                usage: USAGE_DEFAULT,
                bind_flags: BIND_DEPTH_STENCIL | BIND_SHADER_RESOURCE,
                cpu_access_flags: 0,
                misc_flags: 0,
            };
            let texture = texture_2d(device, &desc, None)?;
            let dsv_desc = DsvDesc { format: FORMAT_D16_UNORM, view_dimension: DSV_DIMENSION_TEXTURE2D, flags: 0, mip_slice: 0, _union_pad: [0; 2] };
            let mut dsv: *mut Dsv = null_mut();
            if !com_ok(((*(*device).vtbl).create_depth_stencil_view)(device, texture.as_ptr().cast(), (&dsv_desc as *const DsvDesc).cast(), &mut dsv)) {
                return None;
            }
            let dsv = Com::from_raw(dsv)?;
            // the view of a 2D texture: its first level, one level
            let srv_desc = SrvDesc { format: FORMAT_R16_UNORM, view_dimension: SRV_DIMENSION_TEXTURE2D, first_element: 0, num_elements: 1, _union_pad: [0; 2] };
            let mut srv: *mut Srv = null_mut();
            if !com_ok(((*(*device).vtbl).create_shader_resource_view)(device, texture.as_ptr().cast(), &srv_desc, &mut srv)) {
                return None;
            }
            let srv = Com::from_raw(srv)?;
            ShadowMap { _texture: texture, dsv, srv, size }
        };
        let id = self.mint();
        self.shadow_maps.insert(id, map);
        Some(id)
    }

    fn drop_shadow_map(&mut self, id: u64) {
        self.shadow_maps.remove(&id);
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use bunny_ui::gpu::world_scenes::{COUNT, Spread, world_scene};
    use bunny_ui::image_engine::RawImages;
    use bunny_ui::layout::Color;
    use bunny_ui::world::soft::{draw, draw_with};
    use bunny_ui::world::texture::texture_of;
    use bunny_ui::world::{Background, Camera, Material, Mesh, Quality, Vec3, World, WorldFrame, WorldStyle, vec3};

    use super::super::OffscreenD3d;

    /// The harness, or `None` on a machine with no D3D11 device at all
    /// (WARP makes one almost everywhere).
    fn harness() -> Option<OffscreenD3d> {
        let gpu = OffscreenD3d::new(8, 8);
        if gpu.is_none() {
            eprintln!("no d3d11 device — skipping");
        }
        gpu
    }

    /// Writes both pictures as raw RGBA when `BUNNY_WORLD_DUMP` names a
    /// folder — for a person to look at, never for the test to read.
    fn dump(name: &str, size: (u32, u32), ours: &[u8], oracle: &[u8]) {
        if let Ok(folder) = std::env::var("BUNNY_WORLD_DUMP") {
            let folder = std::path::Path::new(&folder);
            let _ = std::fs::write(folder.join(format!("{name}-d3d-{}x{}.rgba", size.0, size.1)), ours);
            let _ = std::fs::write(folder.join(format!("{name}-oracle-{}x{}.rgba", size.0, size.1)), oracle);
        }
    }

    #[test]
    fn every_world_of_the_catalog_matches_the_oracle() {
        let Some(mut gpu) = harness() else { return };
        let images = RawImages::default();
        for index in 0..COUNT {
            let scene = world_scene(index);
            let frame = &scene.frame;
            let ours = gpu.world_rgba_with(frame, &images).expect("the world pass");
            let oracle = draw_with(frame, &|at| texture_of(&images, &frame.textures[at])).rgba;
            dump(&format!("catalog-{index}"), frame.size, &ours, &oracle);
            let s = Spread::of(&ours, &oracle);
            eprintln!("{}: worst {} · >1 {} · >3 {} · >16 {} of {}", scene.name, s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
            assert!(s.within(scene.tolerance), "{} is beyond its tolerance", scene.name);
        }
    }

    #[test]
    fn four_samples_soften_the_edges_and_keep_the_faces() {
        let Some(mut gpu) = harness() else { return };
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60)));
        let style = WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality: Quality::Medium, ..WorldStyle::default() };
        let frame = Rc::new(world.frame(&Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0), &style, (160, 120), 1.0));
        assert_eq!(frame.samples, 4);
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        let s = Spread::of(&ours, &oracle);
        eprintln!("4×: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        // the oracle takes one sample: only the edges may move, and they do
        assert!(s.beyond[2] > 0, "multisampling changed no edge");
        assert!(s.beyond[2] as f64 / s.pixels as f64 <= 0.06);
    }

    #[test]
    fn edges_on_turning_surfaces_stay_whole_and_hidden_ones_stay_hidden() {
        use bunny_ui::layout::Size;
        use bunny_ui::world::{LineStyle, vec2};
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
        let style = WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality: Quality::Low, ..WorldStyle::default() };
        let frame: Rc<WorldFrame> = Rc::new(world.frame(&camera, &style, (480, 280), 1.0));
        let ours = gpu.world_rgba(&frame).expect("the world pass");
        let oracle = draw(&frame).rgba;
        dump("ring", frame.size, &ours, &oracle);
        let s = Spread::of(&ours, &oracle);
        eprintln!("ring: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(s.beyond[2] as f64 / s.pixels as f64 <= 0.001, "the edges agree with the oracle's");
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

    /// A world in a box on a page: the 2D around it, the clip and the
    /// veil, through the walk.
    fn page(world: World, camera: bunny_ui::prelude::State<Camera>, veil: f64) -> impl bunny_ui::prelude::View {
        use bunny_ui::prelude::*;
        use bunny_ui::world::scene_view;
        let style = WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality: Quality::Low, ..WorldStyle::default() };
        vstack((text("above"), scene_view(world, camera).style(style).frame(80.0, 60.0).corner_radius(8.0).clipped().opacity(veil)))
            .padding_length(6.0)
    }

    /// The same page drawn by the window's road on the GPU and by the
    /// CPU, the GPU's own world pass held to the CPU's picture.
    fn both(runtime: &bunny_ui::runtime::Runtime, root: &impl bunny_ui::prelude::View, gpu: &mut OffscreenD3d) -> (Vec<u8>, Vec<u8>) {
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
        let mut gpu = OffscreenD3d::new(200, 200).expect("offscreen gpu");
        let camera = State::new(Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0));
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60)));
        world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
        let runtime = bunny_ui::runtime::Runtime::new();
        let (ours, cpu) = both(&runtime, &page(world, camera, 1.0), &mut gpu);
        assert_eq!(gpu.world_jobs(), 1, "the first frame draws the world");
        dump("page", (200, 200), &ours, &cpu);
        let s = Spread::of(&ours, &cpu);
        eprintln!("page: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(s.worst <= 2, "the composite holds the CPU's picture");
        // the same page again: the target holds the picture
        let (again, _) = both(&runtime, &page(world, camera, 1.0), &mut gpu);
        assert_eq!(gpu.world_jobs(), 0, "a held picture is not drawn again");
        assert_eq!(again, ours);
        // a veil rides the identity: composited thinner, never drawn again
        let (veiled, cpu) = both(&runtime, &page(world, camera, 0.5), &mut gpu);
        assert_eq!(gpu.world_jobs(), 0, "a fade draws no world");
        let s = Spread::of(&veiled, &cpu);
        eprintln!("veiled page: worst {} · >1 {} · >3 {} · >16 {} of {}", s.worst, s.beyond[0], s.beyond[1], s.beyond[2], s.pixels);
        assert!(s.worst <= 2, "the veil matches the CPU's");
        assert_ne!(veiled, ours);
    }

    #[test]
    fn the_wire_blocks_hold_their_layout() {
        assert_eq!(std::mem::size_of::<super::DrawBlock>(), 64);
        assert_eq!(std::mem::size_of::<super::BackdropBlock>(), 48);
        assert_eq!(bunny_ui::world::mesh::VERTEX_STRIDE, 32, "WorldVertex reads 32 bytes");
        assert_eq!(bunny_ui::world::wire::instance_offset::SIZE, 112, "WorldInstance reads 112 bytes");
        assert_eq!(bunny_ui::world::wire::SEGMENT_STRIDE, 24, "WorldSegment reads 24 bytes");
    }
}
