//! Metal presentation — the SAME display list, presented by the GPU.
//!
//! This module is the second presentation backend the raster module
//! promised: the display list does not change, the pixels must not change
//! (within an anti-aliasing tolerance the parity tests pin down). The CPU
//! raster stays as the oracle, the headless path and the fallback — this
//! backend exists because a full-window repaint must cost less than a
//! millisecond at ANY window size.
//!
//! House rules apply: no dependencies. Metal comes in through the same
//! hand-written `objc_msgSend` border as the rest of the shell, and the shaders are a
//! source string compiled at RUNTIME (`newLibraryWithSource:`) — zero
//! build steps. No Objective-C blocks either: command-buffer recycling
//! polls `status`, because the whole shell is one thread and a completion
//! handler would be the only concurrent code in the codebase.
//!
//! The GPU is the DEFAULT presentation of a window; `BUNNY_PRESENT=cpu`
//! forces the CPU raster, and any Metal failure falls back to it with
//! one line on stderr. The choice happens ONCE, at window creation —
//! a window never switches backends mid-flight.
//!
//! The LAW of the port: every policy decision — snapping, radius clamps,
//! stroke thickness, shadow reach, the clip stack — is resolved on the
//! CPU in f64, operation by operation the way raster.rs resolves it. The
//! instances carry snapped device pixels in f32 (integers, exact) and the
//! shaders are pure coverage evaluators, blind to DPI.
//!
//! Premises (documented, not checked): arm64 + Apple Silicon (shared
//! memory makes render targets CPU-readable without a blit), and a
//! NON-sRGB pixel format forever — the compositor blends in gamma space,
//! exactly like the CPU raster. An `_sRGB` format would linearize the
//! blending and break parity.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::rc::Rc;

use bunny_ui::gpu::walk::{
    AtlasFull, AtlasGround, DrawRun, FrameBatches, GLASS_MAX_LEVEL, GlassInstance, RectInstance,
    RoundClip, RunAtlas, RunKind, SpriteInstance, build_frame, ATLAS_KEEP_WALKS,
};
use bunny_ui::image_engine::ImageEngine;
use bunny_ui::image_engine::PixelFormat;
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::raster::{DamageRect, ListDamage, list_damage};
use bunny_ui::text_engine::{MeasureCache, TextEngine};

use crate::ffi::{CFRelease, CFRetain, CGPoint, CGRect, CGSize, Id, Sel, class, error_message, kill_layer_actions, ns_string, sel};

// MARK: - FFI border

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

// The same trampoline discipline as ffi.rs: one alias per concrete
// message signature. Re-declaration across modules is the sanctioned
// pattern (the symbol is one).
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);

    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64(obj: Id, sel: Sel, a: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_f64(obj: Id, sel: Sel, a: f64);
    #[link_name = "objc_msgSend"]
    fn msg_u64(obj: Id, sel: Sel) -> u64;
    #[link_name = "objc_msgSend"]
    fn msg_f64(obj: Id, sel: Sel) -> f64;
    #[link_name = "objc_msgSend"]
    fn msg_u64_u64(obj: Id, sel: Sel, a: u64) -> u64;
    #[link_name = "objc_msgSend"]
    fn msg_id_arg(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64_u64(obj: Id, sel: Sel, a: u64, b: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_id_u64(obj: Id, sel: Sel, a: Id, b: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_u64_u64(obj: Id, sel: Sel, a: Id, b: u64, c: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_ptr_u64_u64(obj: Id, sel: Sel, a: *const c_void, b: u64, c: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64x5(obj: Id, sel: Sel, a: u64, b: u64, c: u64, d: u64, e: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64x3(obj: Id, sel: Sel, a: u64, b: u64, c: u64);
    // `CGSize` is a 2-double HFA — it travels in registers.
    #[link_name = "objc_msgSend"]
    fn msg_void_size(obj: Id, sel: Sel, a: CGSize);
    // `CGRect` is a 4-double HFA — registers too.
    #[link_name = "objc_msgSend"]
    fn msg_void_rect(obj: Id, sel: Sel, a: CGRect);
    // `MTLClearColor` is a 4-double HFA — registers as well.
    #[link_name = "objc_msgSend"]
    fn msg_void_clear_color(obj: Id, sel: Sel, a: MTLClearColor);
    // `newLibraryWithSource:options:error:` — the error comes back through
    // the out-pointer when the returned id is nil.
    #[link_name = "objc_msgSend"]
    fn msg_id_id_id_ptr(obj: Id, sel: Sel, a: Id, b: Id, error: *mut Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_ptr(obj: Id, sel: Sel, a: Id, error: *mut Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64_u64_u64_bool(obj: Id, sel: Sel, a: u64, b: u64, c: u64, d: i8) -> Id;
    // `MTLRegion` is 6×u64 (48 bytes, not a float aggregate) — the ABI
    // passes it INDIRECTLY by pointer; `#[repr(C)]` by value spells that
    // convention out for us.
    #[link_name = "objc_msgSend"]
    fn msg_void_ptr_u64_region_u64(
        obj: Id,
        sel: Sel,
        bytes: *mut c_void,
        per_row: u64,
        region: MTLRegion,
        level: u64,
    );
    #[link_name = "objc_msgSend"]
    fn msg_void_region_u64_ptr_u64(
        obj: Id,
        sel: Sel,
        region: MTLRegion,
        level: u64,
        bytes: *const c_void,
        per_row: u64,
    );
    // `copyFromBuffer:sourceOffset:sourceBytesPerRow:sourceBytesPerImage:
    // sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:`
    // — `MTLSize` and `MTLOrigin` are 3×u64 aggregates, passed INDIRECTLY
    // like the region above.
    #[link_name = "objc_msgSend"]
    fn msg_void_blit(
        obj: Id,
        sel: Sel,
        buffer: Id,
        offset: u64,
        per_row: u64,
        per_image: u64,
        size: MTLSize,
        texture: Id,
        slice: u64,
        level: u64,
        origin: MTLOrigin,
    );
}

// MARK: - Metal vocabulary (constants live in source, like the CG ones)

const PIXEL_FORMAT_RGBA8: u64 = 70; // MTLPixelFormatRGBA8Unorm — the mirror's byte order
const PIXEL_FORMAT_BGRA8: u64 = 80; // MTLPixelFormatBGRA8Unorm — the only format a layer takes
// MTLPixelFormatRGBA8Unorm_sRGB — the blur pyramid. An sRGB texture
// decodes on sample and encodes on write, so the whole chain averages
// in LINEAR light for free, which is the difference between glass and
// a grey halo.
const PIXEL_FORMAT_RGBA8_SRGB: u64 = 71;
const LOAD_ACTION_DONT_CARE: u64 = 0;
const LOAD_ACTION_LOAD: u64 = 1;
const LOAD_ACTION_CLEAR: u64 = 2;
const STORE_ACTION_STORE: u64 = 1;
const BLEND_ONE: u64 = 1;
const BLEND_SOURCE_ALPHA: u64 = 4;
const BLEND_ONE_MINUS_SOURCE_ALPHA: u64 = 5;
const STORAGE_MODE_SHARED: u64 = 0;
const STORAGE_MODE_PRIVATE: u64 = 2;
const TEXTURE_USAGE_SHADER_READ: u64 = 1;
const TEXTURE_USAGE_RENDER_TARGET: u64 = 4;
// CPUCacheModeWriteCombined | StorageModeShared — the CPU only writes
// instance bytes, the GPU only reads them.
const RESOURCE_SHARED_WRITE_COMBINED: u64 = 1;
const PRIMITIVE_TRIANGLE: u64 = 3;
const STATUS_COMPLETED: u64 = 4; // MTLCommandBufferStatus: Completed=4, Error=5
// MTLPurgeableState: what a resource may lose while nobody draws with it
const PURGEABLE_NON_VOLATILE: u64 = 2;
const PURGEABLE_VOLATILE: u64 = 3;
const PURGEABLE_EMPTY: u64 = 4;

// The run atlas: text tiles append into one shared texture. Runs wider
// than a chunk split into seamless chunks (texel reads are 1:1, a seam
// cannot show). Overflow drains the in-flight frames, resets the whole
// atlas and re-inserts the current frame — a copying collector, not a
// per-tile free list.

#[repr(C)]
struct MTLClearColor {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

#[repr(C)]
struct MTLOrigin {
    x: u64,
    y: u64,
    z: u64,
}

#[repr(C)]
struct MTLSize {
    width: u64,
    height: u64,
    depth: u64,
}

#[repr(C)]
struct MTLRegion {
    origin: MTLOrigin,
    size: MTLSize,
}

// MARK: - The wire format shared with the shaders

// The instance structs are the walk's (`bunny_ui::gpu::walk`) — every
// tier reads the same bytes. The shaders below spell them textually,
// and the asserts here pin the two spellings to one layout.
const _: () = {
    assert!(std::mem::size_of::<GlassInstance>() == 112);
    assert!(std::mem::offset_of!(GlassInstance, rect) == 0);
    assert!(std::mem::offset_of!(GlassInstance, clip) == 16);
    assert!(std::mem::offset_of!(GlassInstance, radii) == 32);
    assert!(std::mem::offset_of!(GlassInstance, lens) == 48);
    assert!(std::mem::offset_of!(GlassInstance, finish) == 64);
    assert!(std::mem::offset_of!(GlassInstance, touch) == 80);
    assert!(std::mem::offset_of!(GlassInstance, tint) == 96);
    assert!(std::mem::offset_of!(GlassInstance, highlight) == 100);
    assert!(std::mem::offset_of!(GlassInstance, spot_alpha) == 104);
};

/// What one blur pass needs — the twin of `BlurParams` in the shader.
#[repr(C)]
#[derive(Clone, Copy)]
struct BlurParams {
    inv_dst: [f32; 2],
    direction: [f32; 2],
    source_level: f32,
    decode: f32,
    pad: [f32; 2],
}

const _: () = {
    assert!(std::mem::size_of::<BlurParams>() == 32);
    assert!(std::mem::size_of::<RectInstance>() == 80);
    assert!(std::mem::offset_of!(RectInstance, rect) == 0);
    assert!(std::mem::offset_of!(RectInstance, clip) == 16);
    assert!(std::mem::offset_of!(RectInstance, params) == 32);
    assert!(std::mem::offset_of!(RectInstance, color) == 48);
    assert!(std::mem::offset_of!(RectInstance, pad) == 52);
    assert!(std::mem::offset_of!(RectInstance, radii) == 64);
    assert!(std::mem::size_of::<SpriteInstance>() == 48);
    assert!(std::mem::offset_of!(SpriteInstance, dest) == 0);
    assert!(std::mem::offset_of!(SpriteInstance, tex) == 16);
    assert!(std::mem::offset_of!(SpriteInstance, clip) == 32);
};

// MARK: - Shaders (compiled at runtime; the structs above, textually)

// The coverage math is the CPU raster's, rewritten once:
// `clamp(0.5 - sdf, 0, 1)` IS `clamp(radius - distance + 0.5, 0, 1)` for
// the rounded corner, and the full signed distance (outside + inside
// terms) reproduces the straight spans exactly — every interior pixel
// center sits at least 0.5 inside an integer edge, so coverage saturates
// to 1.0 with no `radius < 0.5` branch.
const SHADER_SOURCE: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Uniforms {
    float2 viewport;
    // where the target's first pixel sits in the frame: zero for a
    // whole frame, the patch's corner for a patch
    float2 origin;
};

struct RectInstance {
    float4 rect;
    float4 clip;
    float4 params;
    uchar4 color;
    uchar4 color2;   // a gradient's far color (padding otherwise)
    float2 point2;   // rings: the centre; line: its end (padding otherwise)
    float4 radii;    // top left, top right, bottom right, bottom left
};

struct SpriteInstance {
    float4 dest;
    float4 tex;
    float4 clip;
};

constant float2 unit_corners[6] = {
    float2(0.0, 0.0), float2(1.0, 0.0), float2(0.0, 1.0),
    float2(0.0, 1.0), float2(1.0, 0.0), float2(1.0, 1.0)
};

static float4 to_ndc(float2 position, float2 viewport) {
    float2 unit = position / viewport;
    return float4(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
}

// which of the four a pixel answers to: the box's own midpoint splits
// it in quarters, and a pixel far from every corner reads the same
// coverage whichever radius it picked — a straight edge does not
// depend on it
static float corner_at(float2 p, float4 rect, float4 radii) {
    float2 mid = (rect.xy + rect.zw) * 0.5;
    return p.x < mid.x ? (p.y < mid.y ? radii.x : radii.w)
                       : (p.y < mid.y ? radii.y : radii.z);
}

static float rect_sdf(float2 p, float4 rect, float4 radii) {
    float radius = corner_at(p, rect, radii);
    float2 shifted = max(rect.xy + radius - p, p - (rect.zw - radius));
    float outside = length(max(shifted, 0.0));
    float inside = min(max(shifted.x, shifted.y), 0.0);
    return outside + inside - radius;
}

static float rect_cov(float2 p, float4 rect, float4 radii) {
    return clamp(0.5 - rect_sdf(p, rect, radii), 0.0, 1.0);
}

struct RectVary {
    float4 position [[position]];
    uint id [[flat]];
};

vertex RectVary rect_vertex(uint vid [[vertex_id]],
                            uint iid [[instance_id]],
                            device const RectInstance* rects [[buffer(0)]],
                            constant Uniforms& uniforms [[buffer(1)]]) {
    RectInstance rect = rects[iid];
    // the clip cuts the QUAD, not the coverage: clips are snapped to
    // integers, so the cut falls between pixel centers — exactly the
    // CPU's integer clip
    float2 low = max(rect.rect.xy, rect.clip.xy);
    float2 high = max(min(rect.rect.zw, rect.clip.zw), low);
    float2 corner = unit_corners[vid];
    RectVary out;
    out.position = to_ndc(mix(low, high, corner) - uniforms.origin, uniforms.viewport);
    out.id = iid;
    return out;
}

struct ClipRound {
    float4 box;
    float4 radii;
};

// the curve that softens the run's clip. radius 0 is the straight
// rectangle the quad clamp already cut — and multiplying by 1.0 is
// exact, so a scene without a rounded clip leaves both shaders
// untouched, bit for bit
static float clip_cov(float2 p, constant ClipRound& round) {
    return any(round.radii > 0.0) ? rect_cov(p, round.box, round.radii) : 1.0;
}

fragment float4 rect_fragment(RectVary in [[stage_in]],
                              device const RectInstance* rects [[buffer(0)]],
                              constant ClipRound& round [[buffer(1)]],
                              constant Uniforms& uniforms [[buffer(2)]]) {
    RectInstance rect = rects[in.id];
    // the frame's pixel, wherever the target sits in it
    float2 p = in.position.xy + uniforms.origin;
    float kind = rect.params.z;
    float coverage;
    if (kind == 0.0) {
        // fill: the cpu corner ramp, clamp(radius - d + 0.5, 0, 1)
        coverage = rect_cov(p, rect.rect, rect.radii);
    } else if (kind == 1.0) {
        // stroke: outer coverage minus the inner rect's — the inset
        // keeps the same corner center as the cpu ring, and integer
        // edges keep the straight bars exact and never double-blended
        float thickness = rect.params.y;
        float4 inner = float4(rect.rect.xy + thickness, rect.rect.zw - thickness);
        float4 inner_radii = max(rect.radii - thickness, 0.0);
        coverage = clamp(
            rect_cov(p, rect.rect, rect.radii) - rect_cov(p, inner, inner_radii),
            0.0, 1.0);
    } else if (kind == 2.0) {
        // shadow: quadratic falloff outside the rounded core — the quad
        // arrives pre-expanded, params.w undoes the expansion
        float expansion = rect.params.w;
        float4 base = float4(rect.rect.xy + expansion, rect.rect.zw - expansion);
        float corner = corner_at(p, base, rect.radii);
        float reach = rect.params.y;
        float2 delta = p - clamp(p, base.xy + corner, base.zw - corner);
        float distance = length(delta) - corner;
        float strength = 1.0 - distance / reach;
        coverage = (distance > 0.0 && distance < reach) ? strength * strength : 0.0;
    } else {
        // the gradients cover the fill's shape and change color per
        // pixel: rings from point2 (params.y and .w are the radii), or
        // a ramp from rect.xy to point2. The cpu resolved every number
        // in f64 — this only mixes.
        coverage = rect_cov(p, rect.rect, rect.radii);
        float t;
        if (kind == 3.0) {
            float distance = length(p - rect.point2);
            t = saturate((distance - rect.params.y) / (rect.params.w - rect.params.y));
        } else if (kind == 5.0) {
            // the ellipse is a circle in a Y-scaled space; params.x
            // carries the aspect, so the cover is the plain box
            coverage = rect_cov(p, rect.rect, float4(0.0));
            float2 away = p - rect.point2;
            float distance = length(float2(away.x, away.y / rect.params.x));
            t = saturate((distance - rect.params.y) / (rect.params.w - rect.params.y));
        } else {
            float2 origin = float2(rect.params.y, rect.params.w);
            float2 axis = rect.point2 - origin;
            float length2 = dot(axis, axis);
            t = length2 > 0.0 ? saturate(dot(p - origin, axis) / length2) : 1.0;
        }
        // the cpu rounds the mixed color to bytes before blending;
        // rounding here keeps the two within one step
        float4 near = float4(rect.color);
        float4 far = float4(rect.color2);
        float4 mixed = floor(mix(near, far, t) + 0.5) / 255.0;
        return float4(mixed.rgb, mixed.a * coverage * clip_cov(p, round));
    }
    float4 color = float4(rect.color) / 255.0;
    return float4(color.rgb, color.a * coverage * clip_cov(p, round));
}

struct SpriteVary {
    float4 position [[position]];
    uint id [[flat]];
};

vertex SpriteVary sprite_vertex(uint vid [[vertex_id]],
                                uint iid [[instance_id]],
                                device const SpriteInstance* sprites [[buffer(0)]],
                                constant Uniforms& uniforms [[buffer(1)]]) {
    SpriteInstance sprite = sprites[iid];
    float2 low = max(sprite.dest.xy, sprite.clip.xy);
    float2 high = max(min(sprite.dest.zw, sprite.clip.zw), low);
    float2 corner = unit_corners[vid];
    SpriteVary out;
    out.position = to_ndc(mix(low, high, corner) - uniforms.origin, uniforms.viewport);
    out.id = iid;
    return out;
}

fragment float4 sprite_fragment(SpriteVary in [[stage_in]],
                                device const SpriteInstance* sprites [[buffer(0)]],
                                constant ClipRound& round [[buffer(1)]],
                                constant Uniforms& uniforms [[buffer(2)]],
                                texture2d<float, access::read> atlas [[texture(0)]]) {
    SpriteInstance sprite = sprites[in.id];
    float2 p = in.position.xy + uniforms.origin;
    float2 ratio = (sprite.tex.zw - sprite.tex.xy) / (sprite.dest.zw - sprite.dest.xy);
    float2 texel = sprite.tex.xy + (floor(p) - floor(sprite.dest.xy)) * ratio;
    // straight alpha in, straight alpha out — only the coverage moves,
    // and text under a rounded corner loses its square edge at last
    float4 ink = atlas.read(uint2(texel));
    return float4(ink.rgb, ink.a * clip_cov(p, round));
}

// a feed's sprite: the picture is its own size and the box is another,
// so the sampler scales — linear — from the pixel's centre in the box
// to its place in the picture. Pixel coordinates, no normalisation.
constexpr sampler live_sampler(coord::pixel, filter::linear, address::clamp_to_edge);

fragment float4 live_fragment(SpriteVary in [[stage_in]],
                              device const SpriteInstance* sprites [[buffer(0)]],
                              constant ClipRound& round [[buffer(1)]],
                              constant Uniforms& uniforms [[buffer(2)]],
                              texture2d<float> live [[texture(0)]]) {
    SpriteInstance sprite = sprites[in.id];
    float2 p = in.position.xy + uniforms.origin;
    float2 ratio = (sprite.tex.zw - sprite.tex.xy) / (sprite.dest.zw - sprite.dest.xy);
    float2 texel = sprite.tex.xy + (p - sprite.dest.xy) * ratio;
    float4 ink = live.sample(live_sampler, texel);
    return float4(ink.rgb, ink.a * clip_cov(p, round));
}

// MARK: - Liquid glass
//
// The material of `glass.rs`, textually. Every constant below is that
// module's, and the parity tests hold the two answers together.
//
// A pane READS the scene, and a drawable cannot be read
// (`framebufferOnly` stays YES — turning it off costs lossless
// compression and the direct-to-display path on EVERY frame, glass or
// not). So a frame that carries glass renders into an offscreen colour
// texture and is blitted over at the end. A frame without glass never
// pays a byte of this.

struct BlurParams {
    float2 inv_dst;     // 1 / destination size in px
    float2 direction;   // (1,0) horizontal, (0,1) vertical
    float source_level; // the mip this pass reads
    float decode;       // 1 when the source is raw scene colour
    float2 pad;
};

struct FullVary {
    float4 position [[position]];
};

// One oversized triangle: no shared edge for the rasterizer to seam,
// and three vertices instead of four.
vertex FullVary full_vertex(uint vid [[vertex_id]]) {
    float2 uv = float2(float((vid << 1) & 2), float(vid & 2));
    return FullVary{float4(uv * 2.0 - 1.0, 0.0, 1.0)};
}

// An exact copy: no sampler, no filtering, no uv to get backwards, and
// no colour conversion — the scene texture carries the drawable's own
// format.
fragment float4 blit_fragment(FullVary in [[stage_in]],
                              texture2d<float, access::read> source [[texture(0)]]) {
    return source.read(uint2(in.position.xy));
}

// nine bilinear taps == a seventeen-tap gaussian at sigma 2.6 texels
constant float BLUR_W[5] = {0.153584, 0.256886, 0.125975, 0.034902, 0.005445};
constant float BLUR_O[5] = {0.0, 1.44475, 3.37341, 5.30746, 7.24824};

constant float GLASS_SIGMA_L0 = 5.2;
constant float GLASS_MAX_LEVEL = 3.0;
constant float GLASS_RIM_FLOOR = 0.1;
constant float GLASS_RIM_FALLOFF = 1.7;
constant float2 GLASS_LIGHT_DIR = float2(-0.70710678, -0.70710678);
constant float3 GLASS_LUMA = float3(0.2126, 0.7152, 0.0722);
constant float GLASS_OUTER_AMOUNT_RATIO = 0.25;
constant float GLASS_OUTER_HEIGHT_RATIO = 0.5;
constant float GLASS_VIBRANT_SATURATION = 2.069;
constant float GLASS_VIBRANT_GAIN = 1.45;
constant float GLASS_VIBRANT_BIAS = 0.05;
constant float GLASS_GRAD_RADIUS_FACTOR = 1.5;

static float3 srgb_to_linear3(float3 c) {
    return select(pow((c + 0.055) / 1.055, 2.4), c / 12.92, c <= 0.04045);
}

static float3 linear_to_srgb3(float3 c) {
    return select(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, c <= 0.0031308);
}

static float4 blur_tap(texture2d<float> source, sampler s, float2 uv,
                       constant BlurParams& params) {
    float4 c = source.sample(s, uv, level(params.source_level));
    if (params.decode != 0.0) {
        // colour only: a transfer function never applies to alpha
        return float4(srgb_to_linear3(c.rgb), c.a);
    }
    return c;
}

// The destination is half the resolution of the source, and the offsets
// are in DESTINATION texels — which is what makes the downsample free:
// each bilinear tap already averages a 2x2 neighbourhood.
fragment float4 blur_fragment(FullVary in [[stage_in]],
                              constant BlurParams& params [[buffer(0)]],
                              texture2d<float> source [[texture(0)]]) {
    constexpr sampler s(mag_filter::linear, min_filter::linear,
                        mip_filter::linear, address::clamp_to_edge);
    float2 uv = in.position.xy * params.inv_dst;
    float2 step = params.direction * params.inv_dst;
    float4 acc = blur_tap(source, s, uv, params) * BLUR_W[0];
    for (uint i = 1; i < 5; i++) {
        float2 away = step * BLUR_O[i];
        acc += (blur_tap(source, s, uv + away, params) +
                blur_tap(source, s, uv - away, params)) * BLUR_W[i];
    }
    return acc;
}

struct GlassInstance {
    float4 rect;
    float4 clip;
    float4 radii;
    float4 lens;    // blur, refraction band, refraction amount, chromatic
    float4 finish;  // highlight band, highlight intensity, saturation, brightness
    float4 touch;   // sheen, spot x, spot y, spot radius
    uchar4 tint;
    uchar4 highlight;
    float spot_alpha;
    float pad;
};

struct GlassVary {
    float4 position [[position]];
    uint id [[flat]];
};

// the lens profile: a quarter circle, one at the rim and flat at the
// centre, with an INFINITE slope at the rim
static float glass_circle_map(float x) {
    float c = saturate(x);
    return 1.0 - sqrt(max(1.0 - c * c, 0.0));
}

// the pyramid level a blur reads — `glass::level_for`, word for word
static float glass_level(float sigma) {
    return clamp(log2(max(sigma, GLASS_SIGMA_L0) / GLASS_SIGMA_L0), 0.0, GLASS_MAX_LEVEL);
}

// the analytic gradient of the rounded-rect field. Deliberately not
// dfdx/dfdy: those are quantised to 2x2 fragment quads, which shows as
// a stair-stepped rim
static float2 glass_normal(float2 center_to_point, float2 corner_center) {
    float2 s = float2(center_to_point.x < 0.0 ? -1.0 : 1.0,
                      center_to_point.y < 0.0 ? -1.0 : 1.0);
    float2 m = max(corner_center, 0.0);
    float l = length(m);
    if (l > 1e-5) {
        return s * (m / l);
    }
    return corner_center.x > corner_center.y ? float2(s.x, 0.0) : float2(0.0, s.y);
}

vertex GlassVary glass_vertex(uint vid [[vertex_id]],
                              uint iid [[instance_id]],
                              device const GlassInstance* panes [[buffer(0)]],
                              constant Uniforms& uniforms [[buffer(1)]]) {
    GlassInstance pane = panes[iid];
    float2 low = max(pane.rect.xy, pane.clip.xy);
    float2 high = max(min(pane.rect.zw, pane.clip.zw), low);
    float2 corner = unit_corners[vid];
    GlassVary out;
    out.position = to_ndc(mix(low, high, corner) - uniforms.origin, uniforms.viewport);
    out.id = iid;
    return out;
}

fragment float4 glass_fragment(GlassVary in [[stage_in]],
                               device const GlassInstance* panes [[buffer(0)]],
                               constant ClipRound& round [[buffer(1)]],
                               constant Uniforms& uniforms [[buffer(2)]],
                               texture2d<float> pyramid [[texture(0)]]) {
    // trilinear, so a blur that crosses a level slides instead of
    // snapping
    constexpr sampler s(mag_filter::linear, min_filter::linear,
                        mip_filter::linear, address::clamp_to_edge);
    GlassInstance pane = panes[in.id];
    float2 point = in.position.xy + uniforms.origin;

    float2 half_size = (pane.rect.zw - pane.rect.xy) * 0.5;
    float2 center_to_point = point - pane.rect.xy - half_size;
    float radius = corner_at(point, pane.rect, pane.radii);
    float2 corner_to_point = abs(center_to_point) - half_size;
    float2 corner_center = corner_to_point + radius;
    float sdf = length(max(corner_center, 0.0)) +
                min(max(corner_center.x, corner_center.y), 0.0) - radius;
    float coverage = clamp(0.5 - sdf, 0.0, 1.0);
    if (coverage <= 0.0) {
        return float4(0.0);
    }
    float depth = max(-sdf, 0.0);

    // the direction field, ovalised so a corner sweeps instead of
    // kinking. The true radius already cut the shape above
    float grad_radius = min(radius * GLASS_GRAD_RADIUS_FACTOR, min(half_size.x, half_size.y));
    float2 normal = glass_normal(center_to_point, corner_to_point + grad_radius);

    // two opposed bands on one quarter-circle profile. The main one
    // samples INWARD: the rim magnifies, a convex lens. Outward pinches
    float band = max(pane.lens.y, 1.0);
    float inner = glass_circle_map(1.0 - depth / band);
    float outer = glass_circle_map(1.0 - depth / (band * GLASS_OUTER_HEIGHT_RATIO));
    float profile = inner - outer * GLASS_OUTER_AMOUNT_RATIO;
    float2 displace = normal * (-pane.lens.z * profile);

    // sharper where the lens works, frosted on the face
    float sharpen = 1.0 - saturate(depth / band);
    float mip = max(glass_level(pane.lens.x) - sharpen, 0.0);
    float2 inv_viewport = 1.0 / uniforms.viewport;
    float2 base = point * inv_viewport;
    float4 sampled;
    if (pane.lens.w > 0.0) {
        float spread = pane.lens.w;
        float4 red = pyramid.sample(s, base + displace * (1.0 - spread) * inv_viewport, level(mip));
        float4 green = pyramid.sample(s, base + displace * inv_viewport, level(mip));
        float4 blue = pyramid.sample(s, base + displace * (1.0 + spread) * inv_viewport, level(mip));
        sampled = float4(red.r, green.g, blue.b, green.a);
    } else {
        sampled = pyramid.sample(s, base + displace * inv_viewport, level(mip));
    }

    // back to the engine's colour space FIRST: the saturation this
    // material is tuned against runs on ENCODED values, unlike the
    // blur, which must average in linear light
    float alpha = max(sampled.a, 1e-4);
    float3 rgb = linear_to_srgb3(sampled.rgb / alpha);
    float luma = dot(rgb, GLASS_LUMA);
    rgb = (luma + (rgb - luma) * pane.finish.z) * pane.finish.w;
    float4 color = float4(rgb, sampled.a);

    // the tint, over
    float4 tint = float4(pane.tint) / 255.0;
    color = float4(mix(color.rgb, tint.rgb, tint.a), tint.a + color.a * (1.0 - tint.a));

    // the specular rim: a thin band lit along BOTH diagonals, in the
    // colour of the scene under it, ADDED instead of painted
    float rim = 1.0 - saturate(depth / max(pane.finish.x, 1.0));
    float axis = abs(dot(normal, GLASS_LIGHT_DIR));
    float ring = GLASS_RIM_FLOOR + (1.0 - GLASS_RIM_FLOOR) * pow(axis, GLASS_RIM_FALLOFF);
    float4 highlight = float4(pane.highlight) / 255.0;
    float strength = pane.finish.y * rim * rim * ring * highlight.a;
    if (strength > 0.0) {
        float grey = dot(color.rgb, GLASS_LUMA);
        float3 vibrant = saturate(
            (grey + (color.rgb - grey) * GLASS_VIBRANT_SATURATION) * GLASS_VIBRANT_GAIN
            + GLASS_VIBRANT_BIAS);
        color = float4(saturate(color.rgb + vibrant * highlight.rgb * strength), color.a);
    }

    // the touch: a flat wash plus a pool of light, both additive and
    // both zero unless the pane asked
    float spot = 0.0;
    if (pane.spot_alpha > 0.0 && pane.touch.w > 0.0) {
        float away = distance(point, pane.touch.yz);
        float fall = 1.0 - saturate(away / pane.touch.w);
        spot = pane.spot_alpha * fall * fall;
    }
    float touch = saturate(pane.touch.x + spot);
    if (touch > 0.0) {
        color = float4(saturate(color.rgb + touch), color.a);
    }

    // straight alpha out — the blend state premultiplies, exactly as it
    // does for a rect
    return float4(color.rgb, color.a * coverage * clip_cov(point, round));
}
"#;

// MARK: - Selectors of the hot path

/// Registered ONCE — `sel()` allocates a `CString` per call, a price the
/// per-frame path refuses to pay.
struct Sels {
    render_pass_descriptor: Sel,
    color_attachments: Sel,
    object_at: Sel,
    set_texture: Sel,
    set_load_action: Sel,
    set_store_action: Sel,
    set_clear_color: Sel,
    command_buffer: Sel,
    encoder: Sel,
    end_encoding: Sel,
    present_drawable: Sel,
    commit: Sel,
    wait_completed: Sel,
    next_drawable: Sel,
    texture: Sel,
    set_contents_scale: Sel,
    set_drawable_size: Sel,
    get_bytes: Sel,
    set_pipeline: Sel,
    set_vertex_buffer: Sel,
    set_fragment_buffer: Sel,
    set_vertex_bytes: Sel,
    set_fragment_bytes: Sel,
    draw: Sel,
    /// The plain three-vertex draw the fullscreen passes make.
    draw_plain: Sel,
    /// The mip a blur pass writes into.
    set_level: Sel,
    set_fragment_texture: Sel,
    set_presents_with_transaction: Sel,
    wait_scheduled: Sel,
    present: Sel,
    status: Sel,
    gpu_start: Sel,
    gpu_end: Sel,
    retain: Sel,
    release: Sel,
    contents: Sel,
    /// The blit pass a frame with feeds opens first.
    blit_encoder: Sel,
    copy_to_texture: Sel,
}

impl Sels {
    unsafe fn new() -> Sels {
        unsafe {
            Sels {
                render_pass_descriptor: sel("renderPassDescriptor"),
                color_attachments: sel("colorAttachments"),
                object_at: sel("objectAtIndexedSubscript:"),
                set_texture: sel("setTexture:"),
                set_load_action: sel("setLoadAction:"),
                set_store_action: sel("setStoreAction:"),
                set_clear_color: sel("setClearColor:"),
                command_buffer: sel("commandBuffer"),
                encoder: sel("renderCommandEncoderWithDescriptor:"),
                end_encoding: sel("endEncoding"),
                present_drawable: sel("presentDrawable:"),
                commit: sel("commit"),
                wait_completed: sel("waitUntilCompleted"),
                next_drawable: sel("nextDrawable"),
                texture: sel("texture"),
                set_contents_scale: sel("setContentsScale:"),
                set_drawable_size: sel("setDrawableSize:"),
                get_bytes: sel("getBytes:bytesPerRow:fromRegion:mipmapLevel:"),
                set_pipeline: sel("setRenderPipelineState:"),
                set_vertex_buffer: sel("setVertexBuffer:offset:atIndex:"),
                set_fragment_buffer: sel("setFragmentBuffer:offset:atIndex:"),
                set_vertex_bytes: sel("setVertexBytes:length:atIndex:"),
                set_fragment_bytes: sel("setFragmentBytes:length:atIndex:"),
                draw: sel("drawPrimitives:vertexStart:vertexCount:instanceCount:baseInstance:"),
                draw_plain: sel("drawPrimitives:vertexStart:vertexCount:"),
                set_level: sel("setLevel:"),
                set_fragment_texture: sel("setFragmentTexture:atIndex:"),
                set_presents_with_transaction: sel("setPresentsWithTransaction:"),
                wait_scheduled: sel("waitUntilScheduled"),
                present: sel("present"),
                status: sel("status"),
                gpu_start: sel("GPUStartTime"),
                gpu_end: sel("GPUEndTime"),
                retain: sel("retain"),
                release: sel("release"),
                contents: sel("contents"),
                blit_encoder: sel("blitCommandEncoder"),
                copy_to_texture: sel(
                    "copyFromBuffer:sourceOffset:sourceBytesPerRow:sourceBytesPerImage:\
                     sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:",
                ),
            }
        }
    }
}

// MARK: - The stack (device, queue, pipelines)

/// Everything a render target needs, window or offscreen. Built once;
/// any failure prints one line and the caller falls back to the CPU.
struct MetalStack {
    device: Id,
    queue: Id,
    rect_pipeline: Id,
    sprite_pipeline: Id,
    /// The feeds' pipeline: the sprite vertex over a LINEAR sampler, so
    /// a picture of one size lands in a box of another.
    live_pipeline: Id,
    /// The three pipelines liquid glass adds: the pane itself, one
    /// separable blur pass, and the copy of the offscreen scene onto
    /// the target a frame with glass cannot render into directly.
    glass_pipeline: Id,
    blur_pipeline: Id,
    blit_pipeline: Id,
    /// The render-target format the pipelines bound to — the scene
    /// texture a glass frame renders into must match it.
    format: u64,
    pass_class: Id, // MTLRenderPassDescriptor — the class object is stable
    sels: Sels,
}

pub(crate) unsafe fn default_device() -> Option<Id> {
    let device = unsafe { MTLCreateSystemDefaultDevice() };
    (!device.is_null()).then_some(device)
}

/// A shared-storage, shader-read texture of `pixel_format` the CPU may
/// fill with `upload_texture` — the atlas's own kind, by any format.
pub(crate) unsafe fn shared_texture(device: Id, pixel_format: u64, width: u32, height: u32) -> Id {
    unsafe {
        let descriptor = msg_id_u64_u64_u64_bool(
            class("MTLTextureDescriptor"),
            sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
            pixel_format,
            width as u64,
            height as u64,
            0,
        );
        msg_void_u64(descriptor, sel("setUsage:"), TEXTURE_USAGE_SHADER_READ);
        msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_SHARED);
        msg_id_arg(device, sel("newTextureWithDescriptor:"), descriptor)
    }
}

/// One tile of rows into a shared texture: `bytes` starts at the tile's
/// first texel and the rows are `pitch_px` apart (four bytes a pixel).
pub(crate) unsafe fn upload_texture(texture: Id, x: u32, y: u32, w: u32, h: u32, bytes: &[u8], pitch_px: u32) {
    unsafe {
        msg_void_region_u64_ptr_u64(
            texture,
            sel("replaceRegion:mipmapLevel:withBytes:bytesPerRow:"),
            MTLRegion {
                origin: MTLOrigin { x: x as u64, y: y as u64, z: 0 },
                size: MTLSize { width: w as u64, height: h as u64, depth: 1 },
            },
            0,
            bytes.as_ptr() as *const c_void,
            (pitch_px * 4) as u64,
        );
    }
}

impl MetalStack {
    /// `format` is the render-target pixel format the pipelines bind to:
    /// BGRA for the layer, RGBA for offscreen readback.
    fn create(format: u64) -> Option<MetalStack> {
        unsafe {
            let device = default_device()?;
            let pool = objc_autoreleasePoolPush();
            let result = Self::build(device, format);
            objc_autoreleasePoolPop(pool);
            if let Err(reason) = &result {
                eprintln!("bunny_ui metal: {reason} — presenting by cpu");
            }
            result.ok()
        }
    }

    unsafe fn build(device: Id, format: u64) -> Result<MetalStack, String> {
        unsafe {
            let queue = msg_id(device, sel("newCommandQueue"));
            if queue.is_null() {
                return Err("the device gave no command queue".to_string());
            }
            let mut error: Id = null_mut();
            let library = msg_id_id_id_ptr(
                device,
                sel("newLibraryWithSource:options:error:"),
                ns_string(SHADER_SOURCE),
                null_mut(), // nil options: no fast-math surprises to audit
                &mut error,
            );
            if library.is_null() {
                return Err(format!("shader compile failed: {}", error_message(error)));
            }
            let rect_pipeline =
                build_pipeline(device, library, "rect_vertex", "rect_fragment", format, true)?;
            let sprite_pipeline =
                build_pipeline(device, library, "sprite_vertex", "sprite_fragment", format, true)?;
            let live_pipeline =
                build_pipeline(device, library, "sprite_vertex", "live_fragment", format, true)?;
            // a pane blends over the scene like any other paint; the
            // blur and the blit REPLACE what they write (a pass that
            // covers its whole destination has nothing to keep)
            let glass_pipeline =
                build_pipeline(device, library, "glass_vertex", "glass_fragment", format, true)?;
            let blur_pipeline = build_pipeline(
                device,
                library,
                "full_vertex",
                "blur_fragment",
                PIXEL_FORMAT_RGBA8_SRGB,
                false,
            )?;
            let blit_pipeline =
                build_pipeline(device, library, "full_vertex", "blit_fragment", format, false)?;
            msg_void(library, sel("release"));
            Ok(MetalStack {
                device,
                queue,
                rect_pipeline,
                sprite_pipeline,
                live_pipeline,
                glass_pipeline,
                blur_pipeline,
                blit_pipeline,
                format,
                pass_class: class("MTLRenderPassDescriptor"),
                sels: Sels::new(),
            })
        }
    }

    /// The frame, in as many passes as it takes.
    ///
    /// A frame with no glass is ONE pass, exactly as it always was:
    /// clear to `canvas`, then the runs in paint order, the pipeline
    /// swapping only where rects and text alternate.
    ///
    /// A pane of glass READS the scene, so it cannot share a pass with
    /// the scene it reads. Each glass batch closes the pass before it,
    /// blurs what is there into the pyramid, and opens a pass that
    /// LOADS instead of clearing. `present_to` is the drawable a glass
    /// frame is copied onto at the end — a drawable is
    /// `framebufferOnly` and cannot be read, so the frame rendered into
    /// an offscreen texture instead.
    ///
    /// Returns the command buffer (autoreleased — the caller holds the
    /// pool and decides how to present it).
    #[allow(clippy::too_many_arguments)]
    unsafe fn encode_frame(&self, frame: EncodeFrame) -> Id {
        unsafe {
            let command = msg_id(self.queue, self.sels.command_buffer);
            // the feeds' new bytes land first, from the slot's staging
            // buffer, on the queue — after every earlier frame that still
            // samples the textures, before this one does
            if !frame.live_copies.is_empty() {
                let blit = msg_id(command, self.sels.blit_encoder);
                for copy in frame.live_copies {
                    msg_void_blit(
                        blit,
                        self.sels.copy_to_texture,
                        frame.staging,
                        copy.offset,
                        copy.per_row,
                        copy.per_row * copy.height,
                        MTLSize { width: copy.width, height: copy.height, depth: 1 },
                        copy.texture,
                        0,
                        0,
                        MTLOrigin { x: 0, y: 0, z: 0 },
                    );
                }
                msg_void(blit, self.sels.end_encoding);
            }
            let mut cleared = false;
            let mut index = 0;
            while index < frame.runs.len() {
                let start = index;
                while index < frame.runs.len() && frame.runs[index].kind != RunKind::Glass {
                    index += 1;
                }
                if index > start || !cleared {
                    self.encode_paint(command, &frame, &frame.runs[start..index], !cleared);
                    cleared = true;
                }
                if index < frame.runs.len() {
                    let run = frame.runs[index];
                    if let Some(pyramid) = frame.pyramid {
                        self.build_pyramid(command, pyramid, frame.target, frame.viewport, run.levels);
                        self.encode_glass(command, &frame, run, pyramid);
                    }
                    index += 1;
                }
            }
            if !cleared {
                self.encode_paint(command, &frame, &[], true);
            }
            if !frame.present_to.is_null() {
                self.encode_blit(command, frame.target, frame.present_to);
            }
            command
        }
    }

    /// A render pass over `target`, with the load action the caller
    /// owns: the FIRST pass of a frame clears to the canvas colour and
    /// every pass after it loads what is already there.
    unsafe fn begin_pass(&self, command: Id, target: Id, level: u64, load: u64, canvas: Color) -> Id {
        unsafe {
            let pass = msg_id(self.pass_class, self.sels.render_pass_descriptor);
            let attachment =
                msg_id_u64(msg_id(pass, self.sels.color_attachments), self.sels.object_at, 0);
            msg_void_id(attachment, self.sels.set_texture, target);
            msg_void_u64(attachment, self.sels.set_level, level);
            msg_void_u64(attachment, self.sels.set_load_action, load);
            msg_void_u64(attachment, self.sels.set_store_action, STORE_ACTION_STORE);
            msg_void_clear_color(
                attachment,
                self.sels.set_clear_color,
                MTLClearColor {
                    red: canvas.r as f64 / 255.0,
                    green: canvas.g as f64 / 255.0,
                    blue: canvas.b as f64 / 255.0,
                    alpha: canvas.a as f64 / 255.0,
                },
            );
            msg_id_arg(command, self.sels.encoder, pass)
        }
    }

    /// One pass of rects and text — the pass this backend has always
    /// encoded.
    unsafe fn encode_paint(&self, command: Id, frame: &EncodeFrame, runs: &[DrawRun], clear: bool) {
        unsafe {
            let load = if clear { LOAD_ACTION_CLEAR } else { LOAD_ACTION_LOAD };
            let encoder = self.begin_pass(command, frame.target, 0, load, frame.canvas);
            if !runs.is_empty() {
                let uniforms = [frame.viewport.0, frame.viewport.1, frame.origin.0, frame.origin.1];
                // argument bindings persist across pipeline swaps — the
                // uniforms bind once, to both stages
                msg_void_ptr_u64_u64(
                    encoder,
                    self.sels.set_vertex_bytes,
                    uniforms.as_ptr() as *const c_void,
                    16,
                    1,
                );
                msg_void_ptr_u64_u64(
                    encoder,
                    self.sels.set_fragment_bytes,
                    uniforms.as_ptr() as *const c_void,
                    16,
                    2,
                );
                let mut bound: Option<RunKind> = None;
                let mut bound_round: Option<u32> = None;
                for run in runs {
                    // 32 bytes per SHAPE change — a frame with no
                    // rounded clip binds slot zero once; bindings
                    // persist across the pipeline swaps
                    if bound_round != Some(run.round) {
                        msg_void_ptr_u64_u64(
                            encoder,
                            self.sels.set_fragment_bytes,
                            (&frame.rounds[run.round as usize]) as *const RoundClip as *const c_void,
                            32,
                            1,
                        );
                        bound_round = Some(run.round);
                    }
                    if bound != Some(run.kind) {
                        match run.kind {
                            RunKind::Rects => {
                                msg_void_id(encoder, self.sels.set_pipeline, self.rect_pipeline);
                                msg_void_id_u64_u64(
                                    encoder,
                                    self.sels.set_vertex_buffer,
                                    frame.instances,
                                    0,
                                    0,
                                );
                                msg_void_id_u64_u64(
                                    encoder,
                                    self.sels.set_fragment_buffer,
                                    frame.instances,
                                    0,
                                    0,
                                );
                            }
                            RunKind::Sprites | RunKind::Texture(_) | RunKind::Live(_) => {
                                let pipeline = match run.kind {
                                    RunKind::Live(_) => self.live_pipeline,
                                    _ => self.sprite_pipeline,
                                };
                                msg_void_id(encoder, self.sels.set_pipeline, pipeline);
                                msg_void_id_u64_u64(
                                    encoder,
                                    self.sels.set_vertex_buffer,
                                    frame.instances,
                                    frame.sprite_offset as u64,
                                    0,
                                );
                                msg_void_id_u64_u64(
                                    encoder,
                                    self.sels.set_fragment_buffer,
                                    frame.instances,
                                    frame.sprite_offset as u64,
                                    0,
                                );
                                // the shared atlas, or the run's own
                                // dedicated or live texture
                                let texture = match run.kind {
                                    RunKind::Texture(index) | RunKind::Live(index) => {
                                        frame.textures[index as usize]
                                    }
                                    _ => frame.atlas_texture,
                                };
                                msg_void_id_u64(
                                    encoder,
                                    self.sels.set_fragment_texture,
                                    texture,
                                    0,
                                );
                            }
                            // glass never reaches this pass
                            RunKind::Glass => continue,
                        }
                        bound = Some(run.kind);
                    }
                    msg_void_u64x5(
                        encoder,
                        self.sels.draw,
                        PRIMITIVE_TRIANGLE,
                        0,
                        6,
                        run.count as u64,
                        run.base as u64,
                    );
                }
            }
            msg_void(encoder, self.sels.end_encoding);
        }
    }

    /// One batch of panes, over the scene they just read.
    unsafe fn encode_glass(
        &self,
        command: Id,
        frame: &EncodeFrame,
        run: DrawRun,
        pyramid: &GlassTextures,
    ) {
        unsafe {
            let encoder =
                self.begin_pass(command, frame.target, 0, LOAD_ACTION_LOAD, frame.canvas);
            let size = [frame.viewport.0, frame.viewport.1, frame.origin.0, frame.origin.1];
            msg_void_ptr_u64_u64(
                encoder,
                self.sels.set_vertex_bytes,
                size.as_ptr() as *const c_void,
                16,
                1,
            );
            msg_void_id(encoder, self.sels.set_pipeline, self.glass_pipeline);
            msg_void_id_u64_u64(
                encoder,
                self.sels.set_vertex_buffer,
                frame.instances,
                frame.glass_offset as u64,
                0,
            );
            msg_void_id_u64_u64(
                encoder,
                self.sels.set_fragment_buffer,
                frame.instances,
                frame.glass_offset as u64,
                0,
            );
            msg_void_ptr_u64_u64(
                encoder,
                self.sels.set_fragment_bytes,
                (&frame.rounds[run.round as usize]) as *const RoundClip as *const c_void,
                32,
                1,
            );
            msg_void_ptr_u64_u64(
                encoder,
                self.sels.set_fragment_bytes,
                size.as_ptr() as *const c_void,
                16,
                2,
            );
            msg_void_id_u64(encoder, self.sels.set_fragment_texture, pyramid.ping, 0);
            msg_void_u64x5(
                encoder,
                self.sels.draw,
                PRIMITIVE_TRIANGLE,
                0,
                6,
                run.count as u64,
                run.base as u64,
            );
            msg_void(encoder, self.sels.end_encoding);
        }
    }

    /// The blur pyramid, from the scene as it stands right now.
    ///
    /// Level 0 is the scene at half resolution blurred to sigma 5.2
    /// device px, and each level halves again and composes another. The
    /// downsample is FUSED into the horizontal pass: it writes the
    /// smaller destination while sampling the larger source, and each
    /// bilinear tap averages a 2x2 neighbourhood on the way. Ping and
    /// pong are two textures on purpose — no pass ever reads the
    /// texture it writes.
    unsafe fn build_pyramid(
        &self,
        command: Id,
        pyramid: &GlassTextures,
        scene: Id,
        viewport: (f32, f32),
        max_level: u32,
    ) {
        unsafe {
            let base_width = (viewport.0.max(1.0) as u32).div_ceil(2).max(1);
            let base_height = (viewport.1.max(1.0) as u32).div_ceil(2).max(1);
            for level in 0..=max_level.min(GLASS_MAX_LEVEL) {
                let width = (base_width >> level).max(1);
                let height = (base_height >> level).max(1);
                let inv_dst = [1.0 / width as f32, 1.0 / height as f32];
                // level 0 reads raw scene colour, which no format
                // decodes for us
                let (source, source_level, decode) = match level {
                    0 => (scene, 0.0, 1.0),
                    _ => (pyramid.ping, (level - 1) as f32, 0.0),
                };
                self.blur_pass(
                    command,
                    source,
                    pyramid.pong,
                    level,
                    BlurParams {
                        inv_dst,
                        direction: [1.0, 0.0],
                        source_level,
                        decode,
                        pad: [0.0, 0.0],
                    },
                );
                self.blur_pass(
                    command,
                    pyramid.pong,
                    pyramid.ping,
                    level,
                    BlurParams {
                        inv_dst,
                        direction: [0.0, 1.0],
                        source_level: level as f32,
                        decode: 0.0,
                        pad: [0.0, 0.0],
                    },
                );
            }
        }
    }

    unsafe fn blur_pass(&self, command: Id, source: Id, target: Id, level: u32, params: BlurParams) {
        unsafe {
            // the pass covers its whole destination: there is nothing
            // to load and nothing to clear
            let encoder = self.begin_pass(
                command,
                target,
                level as u64,
                LOAD_ACTION_DONT_CARE,
                Color::BLACK,
            );
            msg_void_id(encoder, self.sels.set_pipeline, self.blur_pipeline);
            msg_void_ptr_u64_u64(
                encoder,
                self.sels.set_fragment_bytes,
                (&params) as *const BlurParams as *const c_void,
                32,
                0,
            );
            msg_void_id_u64(encoder, self.sels.set_fragment_texture, source, 0);
            msg_void_u64x3(encoder, self.sels.draw_plain, PRIMITIVE_TRIANGLE, 0, 3);
            msg_void(encoder, self.sels.end_encoding);
        }
    }

    /// The offscreen scene onto the drawable — an exact copy.
    unsafe fn encode_blit(&self, command: Id, source: Id, target: Id) {
        unsafe {
            let encoder =
                self.begin_pass(command, target, 0, LOAD_ACTION_DONT_CARE, Color::BLACK);
            msg_void_id(encoder, self.sels.set_pipeline, self.blit_pipeline);
            msg_void_id_u64(encoder, self.sels.set_fragment_texture, source, 0);
            msg_void_u64x3(encoder, self.sels.draw_plain, PRIMITIVE_TRIANGLE, 0, 3);
            msg_void(encoder, self.sels.end_encoding);
        }
    }
}

/// Everything one frame's encode needs — a struct because the call is
/// deep and the arguments are many.
struct EncodeFrame<'a> {
    /// What the frame renders into: the drawable, or the offscreen
    /// scene texture when the frame carries glass.
    target: Id,
    /// The drawable to copy onto at the end, or null.
    present_to: Id,
    canvas: Color,
    /// The target's size in pixels.
    viewport: (f32, f32),
    /// Where the target's first pixel sits in the frame: zero for a
    /// whole frame, a patch's corner for a patch.
    origin: (f32, f32),
    instances: Id,
    sprite_offset: usize,
    glass_offset: usize,
    runs: &'a [DrawRun],
    rounds: &'a [RoundClip],
    atlas_texture: Id,
    textures: &'a [Id],
    pyramid: Option<&'a GlassTextures>,
    /// The slot's staging buffer, and the feeds' copies out of it.
    staging: Id,
    live_copies: &'a [LiveCopy],
}

/// The textures liquid glass needs: the ping and pong of the blur
/// pyramid, and the offscreen scene a window frame renders into because
/// its drawable cannot be read.
struct GlassTextures {
    ping: Id,
    pong: Id,
    /// Null when the target is readable already — the offscreen harness
    /// renders into its own texture and needs no copy.
    scene: Id,
    size: (usize, usize),
}

impl GlassTextures {
    /// Half resolution, four mips, private storage. Returns `None` when
    /// any texture fails to come up — a frame then paints without its
    /// panes instead of failing to present.
    unsafe fn new(
        device: Id,
        size: (usize, usize),
        format: u64,
        offscreen_scene: bool,
    ) -> Option<GlassTextures> {
        unsafe {
            if size.0 == 0 || size.1 == 0 {
                return None;
            }
            let half = (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1));
            let ping = make_texture(device, PIXEL_FORMAT_RGBA8_SRGB, half, GLASS_MAX_LEVEL + 1);
            let pong = make_texture(device, PIXEL_FORMAT_RGBA8_SRGB, half, GLASS_MAX_LEVEL + 1);
            let scene = match offscreen_scene {
                true => make_texture(device, format, size, 1),
                false => null_mut(),
            };
            if ping.is_null() || pong.is_null() || (offscreen_scene && scene.is_null()) {
                return None;
            }
            Some(GlassTextures { ping, pong, scene, size })
        }
    }

    unsafe fn release(&self, sels: &Sels) {
        unsafe {
            for texture in [self.ping, self.pong, self.scene] {
                if !texture.is_null() {
                    msg_void(texture, sels.release);
                }
            }
        }
    }
}

unsafe fn make_texture(device: Id, format: u64, size: (usize, usize), levels: u32) -> Id {
    unsafe {
        let descriptor = msg_id_u64_u64_u64_bool(
            class("MTLTextureDescriptor"),
            sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
            format,
            size.0 as u64,
            size.1 as u64,
            (levels > 1) as i8,
        );
        if levels > 1 {
            msg_void_u64(descriptor, sel("setMipmapLevelCount:"), levels as u64);
        }
        msg_void_u64(
            descriptor,
            sel("setUsage:"),
            TEXTURE_USAGE_RENDER_TARGET | TEXTURE_USAGE_SHADER_READ,
        );
        // the GPU alone touches these — the CPU never reads a pyramid
        msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_PRIVATE);
        msg_id_arg(device, sel("newTextureWithDescriptor:"), descriptor)
    }
}

/// `blend` false is a REPLACING pipeline: a pass that covers its whole
/// destination has nothing to blend with, and blending a blur or a blit
/// would only fold the destination back in.
unsafe fn build_pipeline(
    device: Id,
    library: Id,
    vertex: &str,
    fragment: &str,
    format: u64,
    blend: bool,
) -> Result<Id, String> {
    unsafe {
        let vertex_fn = msg_id_arg(library, sel("newFunctionWithName:"), ns_string(vertex));
        if vertex_fn.is_null() {
            return Err(format!("missing shader function {vertex}"));
        }
        let fragment_fn = msg_id_arg(library, sel("newFunctionWithName:"), ns_string(fragment));
        if fragment_fn.is_null() {
            return Err(format!("missing shader function {fragment}"));
        }
        let descriptor = msg_id(
            msg_id(class("MTLRenderPipelineDescriptor"), sel("alloc")),
            sel("init"),
        );
        msg_void_id(descriptor, sel("setVertexFunction:"), vertex_fn);
        msg_void_id(descriptor, sel("setFragmentFunction:"), fragment_fn);
        let attachment = msg_id_u64(
            msg_id(descriptor, sel("colorAttachments")),
            sel("objectAtIndexedSubscript:"),
            0,
        );
        msg_void_u64(attachment, sel("setPixelFormat:"), format);
        // Source-over with straight alpha — the LITERAL blend_px formula:
        // rgb = s·sa + d·(1−sa); a = sa + da·(1−sa).
        msg_void_bool(attachment, sel("setBlendingEnabled:"), blend as i8);
        msg_void_u64(
            attachment,
            sel("setSourceRGBBlendFactor:"),
            BLEND_SOURCE_ALPHA,
        );
        msg_void_u64(
            attachment,
            sel("setDestinationRGBBlendFactor:"),
            BLEND_ONE_MINUS_SOURCE_ALPHA,
        );
        msg_void_u64(attachment, sel("setSourceAlphaBlendFactor:"), BLEND_ONE);
        msg_void_u64(
            attachment,
            sel("setDestinationAlphaBlendFactor:"),
            BLEND_ONE_MINUS_SOURCE_ALPHA,
        );
        let mut error: Id = null_mut();
        let pipeline = msg_id_id_ptr(
            device,
            sel("newRenderPipelineStateWithDescriptor:error:"),
            descriptor,
            &mut error,
        );
        msg_void(descriptor, sel("release"));
        msg_void(vertex_fn, sel("release"));
        msg_void(fragment_fn, sel("release"));
        if pipeline.is_null() {
            return Err(format!(
                "pipeline {vertex}/{fragment} failed: {}",
                error_message(error)
            ));
        }
        Ok(pipeline)
    }
}

// MARK: - The ground (where the walk's tiles land)

/// The Metal side of the atlas seam. The walk (`bunny_ui::gpu::walk`)
/// keeps every allocation decision — shelves, chunks, the dedicated cap,
/// the collector; this only mints textures and moves bytes into them,
/// and answers a handle the batches carry until the encode binds it.
struct MetalGround {
    device: Id,
    /// The shared atlas texture, or null until the first tile asks.
    shared: Id,
    /// The dedicated AND live textures by the handle the walk was given.
    textures: HashMap<u64, Id>,
    next: u64,
    native: HashMap<u64, std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    /// One staging buffer per ring slot: a feed's new bytes are copied
    /// here while the walk runs, and the frame's command buffer copies
    /// them into the texture — after every earlier frame that still
    /// samples it, which `replaceRegion` could not promise.
    staging: [Staging; 3],
    /// The slot the frame in progress stages into.
    slot: usize,
    /// The copies the frame's blit pass makes, in order.
    pending: Vec<LiveCopy>,
    /// The atlas was offered back to the system while the window rested.
    volatile: bool,
}

/// The staging buffer of one ring slot. Grows when a frame's feeds
/// outgrow it; the buffer it grew out of waits in `retired` until the
/// slot comes round again, because the command buffer in flight still
/// reads it.
struct Staging {
    buffer: Id,
    capacity: usize,
    cursor: usize,
    retired: Vec<Id>,
}

/// One feed's bytes, staged: where they lie in the slot's buffer and
/// the texture they go to. The blit pass at the head of the frame
/// moves them.
#[derive(Clone, Copy)]
struct LiveCopy {
    texture: Id,
    offset: u64,
    per_row: u64,
    width: u64,
    height: u64,
}

/// A blit's source offset rides a 256-byte boundary — the alignment
/// every Metal buffer copy accepts.
const STAGING_ALIGN: usize = 256;

impl MetalGround {
    /// The window rests: the atlas is a cache, and the system may take
    /// it back under pressure while nobody draws with it.
    unsafe fn rest(&mut self) {
        if !self.shared.is_null() && !self.volatile {
            unsafe { msg_u64_u64(self.shared, sel("setPurgeableState:"), PURGEABLE_VOLATILE) };
            self.volatile = true;
        }
    }

    /// A frame is about to draw: the atlas is asked back. False when the
    /// system took it — every tile it held is gone.
    unsafe fn wake(&mut self) -> bool {
        if !self.volatile {
            return true;
        }
        self.volatile = false;
        if self.shared.is_null() {
            return true;
        }
        let before = unsafe { msg_u64_u64(self.shared, sel("setPurgeableState:"), PURGEABLE_NON_VOLATILE) };
        before != PURGEABLE_EMPTY
    }

    fn new(device: Id) -> MetalGround {
        MetalGround {
            device,
            shared: null_mut(),
            volatile: false,
            textures: HashMap::new(),
            next: 1,
            native: HashMap::new(),
            staging: std::array::from_fn(|_| Staging {
                buffer: null_mut(),
                capacity: 0,
                cursor: 0,
                retired: Vec::new(),
            }),
            slot: 0,
            pending: Vec::new(),
        }
    }

    /// The frame about to walk stages into ring slot `index` — whose
    /// previous command buffer has completed, so its buffer is free and
    /// the buffers it retired can go.
    fn begin_slot(&mut self, index: usize) {
        self.slot = index;
        let staging = &mut self.staging[index];
        staging.cursor = 0;
        for buffer in staging.retired.drain(..) {
            unsafe { msg_void(buffer, sel("release")) };
        }
        self.pending.clear();
    }

    /// Room for `bytes` in the slot's staging buffer, 256-aligned —
    /// grown when short, the old buffer kept until the slot comes round.
    unsafe fn stage(&mut self, bytes: &[u8]) -> u64 {
        unsafe {
            let staging = &mut self.staging[self.slot];
            let offset = staging.cursor.next_multiple_of(STAGING_ALIGN);
            let needed = offset + bytes.len();
            if staging.buffer.is_null() || staging.capacity < needed {
                let capacity = (needed * 2).next_multiple_of(4096);
                crate::trace::mark("X", format_args!("what=staging-grow bytes={capacity}"));
                let grown = msg_id_u64_u64(
                    self.device,
                    sel("newBufferWithLength:options:"),
                    capacity as u64,
                    RESOURCE_SHARED_WRITE_COMBINED,
                );
                if !staging.buffer.is_null() {
                    // the copies already recorded keep their offsets: the
                    // bytes move with them, the old buffer waits its turn
                    let from = msg_id(staging.buffer, sel("contents")) as *const u8;
                    let to = msg_id(grown, sel("contents")) as *mut u8;
                    std::ptr::copy_nonoverlapping(from, to, staging.cursor);
                    staging.retired.push(staging.buffer);
                }
                staging.buffer = grown;
                staging.capacity = capacity;
            }
            let contents = msg_id(staging.buffer, sel("contents")) as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), contents.add(offset), bytes.len());
            staging.cursor = needed;
            offset as u64
        }
    }

    /// The staging buffer the frame's copies read from.
    fn staging_buffer(&self) -> Id {
        self.staging[self.slot].buffer
    }

    /// Release this walk's native imports. In-flight slots retain their leases.
    fn clear_native(&mut self) {
        for id in self.native.keys() {
            if let Some(texture) = self.textures.remove(id) {
                unsafe { msg_void(texture, sel("release")) };
            }
        }
        self.native.clear();
    }

    /// The texture behind a handle the walk handed out — null when the
    /// collector already took it (a frame never binds one of those).
    fn texture_of(&self, id: u64) -> Id {
        self.textures.get(&id).copied().unwrap_or(null_mut())
    }

    /// The frame's dedicated textures in the order the runs index them.
    fn bound(&self, handles: &[u64]) -> Vec<Id> {
        handles.iter().map(|id| self.texture_of(*id)).collect()
    }

    /// A shared-storage RGBA texture the CPU writes into directly (the
    /// Apple-Silicon premise of the module), read by the sprite pass.
    unsafe fn make_texture(&self, width: u32, height: u32) -> Id {
        unsafe { shared_texture(self.device, PIXEL_FORMAT_RGBA8, width, height) }
    }

    /// One tile of straight-RGBA rows into a texture: `bytes` starts at
    /// the tile's first texel and the rows are `pitch_px` apart.
    unsafe fn upload(texture: Id, x: u32, y: u32, w: u32, h: u32, bytes: &[u8], pitch_px: u32) {
        unsafe { upload_texture(texture, x, y, w, h, bytes, pitch_px) }
    }
}

impl AtlasGround for MetalGround {
    fn import_native(&mut self, source: &ImageSource) -> Option<u64> {
        let ImageSource::Native { payload, .. } = source else { return None };
        let frame = payload.downcast_ref::<crate::surface::MetalFrame>()?;
        let texture = frame.import(self.device)?;
        let id = self.next;
        self.next += 1;
        self.textures.insert(id, texture);
        self.native.insert(id, payload.clone());
        Some(id)
    }

    fn ensure_shared(&mut self, size: u32) -> bool {
        if !self.shared.is_null() {
            return true;
        }
        self.shared = unsafe { self.make_texture(size, size) };
        !self.shared.is_null()
    }

    fn upload_shared(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: &[u8], pitch_px: u32) {
        if !self.shared.is_null() {
            unsafe { MetalGround::upload(self.shared, x, y, w, h, bytes, pitch_px) };
        }
    }

    fn drop_shared(&mut self) {
        self.volatile = false;
        if !self.shared.is_null() {
            unsafe { msg_void(self.shared, sel("release")) };
            self.shared = null_mut();
        }
    }

    fn make_dedicated(&mut self, w: u32, h: u32, bytes: &[u8], pitch_px: u32) -> Option<u64> {
        let texture = unsafe { self.make_texture(w, h) };
        if texture.is_null() {
            return None;
        }
        unsafe { MetalGround::upload(texture, 0, 0, w, h, bytes, pitch_px) };
        let id = self.next;
        self.next += 1;
        self.textures.insert(id, texture);
        Some(id)
    }

    fn drop_dedicated(&mut self, id: u64) {
        self.native.remove(&id);
        if let Some(texture) = self.textures.remove(&id) {
            unsafe { msg_void(texture, sel("release")) };
        }
    }

    fn make_live(&mut self, w: u32, h: u32, format: PixelFormat) -> Option<u64> {
        let PixelFormat::Rgba8 = format else { return None };
        let texture = unsafe {
            let descriptor = msg_id_u64_u64_u64_bool(
                class("MTLTextureDescriptor"),
                sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
                PIXEL_FORMAT_RGBA8,
                w as u64,
                h as u64,
                0,
            );
            msg_void_u64(descriptor, sel("setUsage:"), TEXTURE_USAGE_SHADER_READ);
            // the GPU alone writes it, from the staging buffer, in order
            msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_PRIVATE);
            msg_id_arg(self.device, sel("newTextureWithDescriptor:"), descriptor)
        };
        if texture.is_null() {
            return None;
        }
        let id = self.next;
        self.next += 1;
        self.textures.insert(id, texture);
        Some(id)
    }

    fn update_live(&mut self, id: u64, w: u32, h: u32, bytes: &[u8], pitch_px: u32) -> bool {
        let Some(&texture) = self.textures.get(&id) else { return false };
        let per_row = pitch_px as usize * 4;
        let needed = (h as usize - 1) * per_row + w as usize * 4;
        if bytes.len() < needed {
            return false;
        }
        let offset = unsafe { self.stage(&bytes[..needed]) };
        // one copy per texture per frame — the newest bytes win
        self.pending.retain(|copy| copy.texture != texture);
        self.pending.push(LiveCopy {
            texture,
            offset,
            per_row: per_row as u64,
            width: w as u64,
            height: h as u64,
        });
        true
    }

    fn drop_live(&mut self, id: u64) {
        if let Some(texture) = self.textures.remove(&id) {
            self.pending.retain(|copy| copy.texture != texture);
            unsafe { msg_void(texture, sel("release")) };
        }
    }
}

// MARK: - Instance buffers (a fixed ring, recycled by polling)

/// One in-flight frame: its instance buffer and the command buffer that
/// reads it. The command buffer is RETAINED while stored; `status >=
/// Completed` (or Error — Metal completes errored buffers too) frees the
/// slot for reuse.
struct FrameSlot {
    buffer: Id,
    capacity: usize,
    command: Id,
    native: Vec<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl FrameSlot {
    const fn empty() -> FrameSlot {
        FrameSlot { buffer: null_mut(), capacity: 0, command: null_mut(), native: Vec::new() }
    }
}

impl Drop for FrameSlot {
    fn drop(&mut self) {
        // Window/offscreen teardown only: never release an owned frame while
        // the compositor may still sample it. Normal reuse polls completion.
        unsafe {
            if !self.command.is_null() {
                msg_void(self.command, sel("waitUntilCompleted"));
                msg_void(self.command, sel("release"));
            }
            if !self.buffer.is_null() { msg_void(self.buffer, sel("release")); }
        }
    }
}
impl Drop for MetalGround {
    fn drop(&mut self) {
        self.clear_native();
        for staging in &mut self.staging {
            unsafe {
                if !staging.buffer.is_null() {
                    msg_void(staging.buffer, sel("release"));
                }
                for buffer in staging.retired.drain(..) {
                    msg_void(buffer, sel("release"));
                }
            }
        }
    }
}

/// A completed frame's time on the GPU, on the tape: `G gpu=<ms>`. Read
/// when its slot is taken for a later frame — the command buffer is done
/// by then, and the two timestamps are the GPU's own clock.
unsafe fn mark_gpu_time(command: Id, sels: &Sels) {
    if !crate::trace::enabled() {
        return;
    }
    let (start, end) = unsafe { (msg_f64(command, sels.gpu_start), msg_f64(command, sels.gpu_end)) };
    if end > start {
        crate::trace::mark("G", format_args!("gpu={:.2}", (end - start) * 1000.0));
    }
}

/// A free slot from a ring: polled by `status`, oldest-first. When all
/// ride the GPU (a burst above the refresh rate), waits for the oldest
/// — bounded by one sub-millisecond frame.
fn acquire_slot(slots: &mut [FrameSlot; 3], cursor: &mut usize, sels: &Sels) -> usize {
    unsafe {
        for offset in 0..slots.len() {
            let index = (*cursor + offset) % slots.len();
            let free = slots[index].command.is_null()
                || msg_u64(slots[index].command, sels.status) >= STATUS_COMPLETED;
            if free {
                if !slots[index].command.is_null() {
                    mark_gpu_time(slots[index].command, sels);
                    msg_void(slots[index].command, sels.release);
                    slots[index].command = null_mut();
                    slots[index].native.clear();
                }
                *cursor = (index + 1) % slots.len();
                return index;
            }
        }
        let index = *cursor;
        msg_void(slots[index].command, sels.wait_completed);
        mark_gpu_time(slots[index].command, sels);
        msg_void(slots[index].command, sels.release);
        slots[index].command = null_mut();
        slots[index].native.clear();
        *cursor = (index + 1) % slots.len();
        index
    }
}

fn as_bytes<T>(items: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(items.as_ptr() as *const u8, std::mem::size_of_val(items))
    }
}

/// Copies the frame's instances into the slot's buffer — rects at zero,
/// sprites 256-aligned after them — growing it when the frame outgrows
/// the capacity. The size is EXACT before Metal is touched, so there is
/// no speculative encode and no overflow retry. Returns the sprite
/// byte offset.
/// Uploads the frame's instances into one buffer, three regions each
/// aligned to 256 bytes, and answers where the sprites and the panes
/// start.
unsafe fn upload_frame(
    slot: &mut FrameSlot,
    device: Id,
    sels: &Sels,
    batches: &FrameBatches,
) -> (usize, usize) {
    unsafe {
        let rect_bytes = as_bytes(&batches.rects);
        let sprite_bytes = as_bytes(&batches.sprites);
        let glass_bytes = as_bytes(&batches.glass);
        let sprite_offset = rect_bytes.len().next_multiple_of(256);
        let glass_offset = (sprite_offset + sprite_bytes.len()).next_multiple_of(256);
        let total = glass_offset + glass_bytes.len();
        if total == 0 {
            return (0, 0);
        }
        if slot.buffer.is_null() || slot.capacity < total {
            if !slot.buffer.is_null() {
                msg_void(slot.buffer, sels.release);
            }
            let capacity = total.next_multiple_of(4096);
            crate::trace::mark("X", format_args!("what=buffer-grow bytes={capacity}"));
            slot.buffer = msg_id_u64_u64(
                device,
                sel("newBufferWithLength:options:"),
                capacity as u64,
                RESOURCE_SHARED_WRITE_COMBINED,
            );
            slot.capacity = capacity;
        }
        let contents = msg_id(slot.buffer, sels.contents) as *mut u8;
        std::ptr::copy_nonoverlapping(rect_bytes.as_ptr(), contents, rect_bytes.len());
        if !sprite_bytes.is_empty() {
            std::ptr::copy_nonoverlapping(
                sprite_bytes.as_ptr(),
                contents.add(sprite_offset),
                sprite_bytes.len(),
            );
        }
        if !glass_bytes.is_empty() {
            std::ptr::copy_nonoverlapping(
                glass_bytes.as_ptr(),
                contents.add(glass_offset),
                glass_bytes.len(),
            );
        }
        (sprite_offset, glass_offset)
    }
}

// MARK: - The window presenter

/// The per-layer GPU state. The shell owns it: a view has no ivars, so
/// the presenter lives in a thread-local of the shell, next to the run
/// loop, keyed however that platform names its surfaces.
/// What a presenter's atlas holds, counted ([`MetalPresenter::atlas_counts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AtlasCounts {
    /// Text run tiles, over every font and colour.
    pub runs: usize,
    /// Image tiles on the shelves.
    pub images: usize,
    /// Images too big for a shelf, each a texture of its own.
    pub dedicated: usize,
    /// Live feeds, one texture each.
    pub live: usize,
    /// The atlas texture's side, in texels.
    pub size: u32,
}

impl std::fmt::Display for AtlasCounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "runs {} images {} dedicated {} live {} size {}",
            self.runs, self.images, self.dedicated, self.live, self.size
        )
    }
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceSetPurgeable(buffer: Id, new_state: u32, old_state: *mut u32) -> i32;
}

/// `kIOSurfacePurgeableNonVolatile`, `kIOSurfacePurgeableVolatile`.
const SURFACE_NON_VOLATILE: u32 = 0;
const SURFACE_VOLATILE: u32 = 1;

/// A layer's drawables as the presenter met them. A resting window shows
/// one drawable and needs none of the others until its next frame — at a
/// window's size each is several megabytes — so the ones off screen are
/// offered back to the system while it rests, as the atlas is, and a
/// frame takes its drawable back whole before it paints it.
#[derive(Default)]
struct Drawables {
    /// Their surfaces, retained, and whether each is offered back.
    surfaces: Vec<(Id, bool)>,
    /// The drawable presented last, retained, and when: the one on screen
    /// once its present has landed.
    shown: Option<(Id, std::time::Instant)>,
    /// The system declined an offer once: no more asks.
    declined: bool,
}

/// How many surfaces a layer keeps at most — three drawables, and a
/// spare for the one a resize leaves behind.
const DRAWABLES_KEPT: usize = 4;

/// A present is on screen within a refresh or two: past this it has
/// landed, whether or not the layer said so. A window's first frame is
/// never reported presented at all.
const PRESENT_LANDS_IN: std::time::Duration = std::time::Duration::from_millis(100);

impl Drawables {
    /// A drawable taken for a frame: its surface is remembered, and taken
    /// back from the system when it was offered while the window rested.
    unsafe fn take(&mut self, texture: Id) {
        unsafe {
            let surface = msg_id(texture, sel("iosurface"));
            if surface.is_null() {
                return;
            }
            match self.surfaces.iter_mut().find(|(kept, _)| *kept == surface) {
                Some((kept, offered)) => {
                    if *offered {
                        // the frame paints it whole: what the system took
                        // of it, if anything, is not missed
                        let mut was = 0;
                        IOSurfaceSetPurgeable(*kept, SURFACE_NON_VOLATILE, &mut was);
                        *offered = false;
                    }
                }
                None => {
                    CFRetain(surface as *const c_void);
                    self.surfaces.push((surface, false));
                    if self.surfaces.len() > DRAWABLES_KEPT {
                        let (old, offered) = self.surfaces.remove(0);
                        if offered {
                            let mut was = 0;
                            IOSurfaceSetPurgeable(old, SURFACE_NON_VOLATILE, &mut was);
                        }
                        CFRelease(old as *const c_void);
                    }
                }
            }
        }
    }

    /// The drawable just presented.
    unsafe fn presented(&mut self, drawable: Id) {
        unsafe {
            let now = std::time::Instant::now();
            if let Some((old, _)) = self.shown.replace((msg_id(drawable, sel("retain")), now)) {
                msg_void(old, sel("release"));
            }
        }
    }

    /// Offers every surface but the one on screen back to the system —
    /// all of them when `hidden` (nothing of the layer shows). False when
    /// the last present has not landed yet: the drawable before it may
    /// still be the one on screen, and the offer waits.
    unsafe fn offer(&mut self, hidden: bool) -> bool {
        unsafe {
            if self.declined {
                return true;
            }
            let on_screen = match (hidden, self.shown) {
                (true, _) => null_mut(),
                (false, None) => return true,
                (false, Some((shown, at))) => {
                    let landed = at.elapsed() >= PRESENT_LANDS_IN
                        || msg_f64(shown, sel("presentedTime")) > 0.0;
                    if !landed {
                        return false;
                    }
                    msg_id(msg_id(shown, sel("texture")), sel("iosurface"))
                }
            };
            for (surface, offered) in &mut self.surfaces {
                if *surface == on_screen || *offered {
                    continue;
                }
                let mut was = 0;
                if IOSurfaceSetPurgeable(*surface, SURFACE_VOLATILE, &mut was) != 0 {
                    self.declined = true;
                    return true;
                }
                *offered = true;
            }
            true
        }
    }

    /// The layer remade its drawables (a new size): the surfaces held are
    /// let go, whole.
    unsafe fn forget(&mut self) {
        unsafe {
            for (surface, offered) in self.surfaces.drain(..) {
                if offered {
                    let mut was = 0;
                    IOSurfaceSetPurgeable(surface, SURFACE_NON_VOLATILE, &mut was);
                }
                CFRelease(surface as *const c_void);
            }
            if let Some((shown, _)) = self.shown.take() {
                msg_void(shown, sel("release"));
            }
        }
    }

    /// How many surfaces are offered back right now.
    fn offered(&self) -> usize {
        self.surfaces.iter().filter(|(_, offered)| *offered).count()
    }
}

impl Drop for Drawables {
    fn drop(&mut self) {
        unsafe { self.forget() }
    }
}

/// A frame a layer shows: the list, its physical size, its scale and its
/// clear colour — the staleness quadruple, the list shared.
type KeptFrame = (Rc<DisplayList>, (usize, usize), usize, Color);

/// What a frame needs from the window, measured against the frame the
/// window's own layer shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// The layer already shows this frame: a patch over it steps aside.
    Same,
    /// The change fits a patch: this box (physical, top-left) repaints.
    Patch(DamageRect),
    /// Repaint the window.
    Whole,
}

/// A patch box snaps outward to this grid of pixels, so the strokes of
/// one word keep one drawable size instead of asking for a new one per
/// glyph.
const PATCH_GRID: i64 = 64;

/// How many changed commands the diff boxes before it calls the frame
/// whole — a scroll moves them all and never pays a measure.
const PATCH_COMMANDS: usize = 64;

/// The share of the window a patch may cover, as a fraction. Past it the
/// window repaints whole: a patch pays a pass and a composite of its own,
/// and a big one buys little.
const PATCH_SHARE: (i64, i64) = (1, 4);

/// The damage snapped outward to the patch grid and clamped to the
/// window — `None` when nothing of it lands on the window.
fn snap_patch(damage: DamageRect, physical: (usize, usize)) -> Option<DamageRect> {
    let (width, height) = (physical.0 as i64, physical.1 as i64);
    let down = |v: i64| v.div_euclid(PATCH_GRID) * PATCH_GRID;
    let up = |v: i64| (v + PATCH_GRID - 1).div_euclid(PATCH_GRID) * PATCH_GRID;
    let rect = (down(damage.0).max(0), down(damage.1).max(0), up(damage.2).min(width), up(damage.3).min(height));
    (rect.0 < rect.2 && rect.1 < rect.3).then_some(rect)
}

/// The patch for a damage box, or `None` when it would cover more of the
/// window than [`PATCH_SHARE`].
fn patch_box(damage: DamageRect, physical: (usize, usize)) -> Option<DamageRect> {
    let rect = snap_patch(damage, physical)?;
    let area = (rect.2 - rect.0) * (rect.3 - rect.1);
    let window = physical.0 as i64 * physical.1 as i64;
    (area * PATCH_SHARE.1 <= window * PATCH_SHARE.0).then_some(rect)
}

/// The small layer over the window's own: a `CAMetalLayer` hung above
/// the drawable and under every live layer, showing the new frame's
/// pixels inside its box while the window's layer keeps the last whole
/// frame. A keystroke or a scrollbar that moves then costs the GPU, and
/// the window server compositing it, the box — not the window.
struct Patch {
    layer: Id,
    /// The drawable size the layer was last given, in pixels.
    size: (usize, usize),
    /// The scale the layer was last given.
    scale: usize,
    /// The box it covers on screen (physical, top-left), or `None`
    /// while it hides.
    shown: Option<DamageRect>,
    /// Its drawables, offered back while the window rests.
    drawables: Drawables,
}

impl Patch {
    /// A hidden patch layer over `root`'s drawable, configured as the
    /// window's own layer is (opaque: the box is painted whole).
    unsafe fn new(stack: &MetalStack, root: Id, scale: usize) -> Option<Patch> {
        unsafe {
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            if layer.is_null() {
                return None;
            }
            msg_void_id(layer, sel("setDevice:"), stack.device);
            msg_void_u64(layer, sel("setPixelFormat:"), stack.format);
            msg_void_bool(layer, sel("setOpaque:"), 1);
            msg_void_bool(layer, sel("setFramebufferOnly:"), 1);
            // a patch presents a stroke at a time: two drawables keep one
            // on screen and one to paint
            msg_void_u64(layer, sel("setMaximumDrawableCount:"), 2);
            msg_void_bool(layer, sel("setAllowsNextDrawableTimeout:"), 0);
            // every patch presents inside the transaction, its box and
            // its pixels together. A layer that flips between that and
            // the asynchronous present loses frames: a drawable once
            // presented inside a transaction and later presented on its
            // own is not shown (seen on screen, a stroke behind)
            msg_void_bool(layer, sel("setPresentsWithTransaction:"), 1);
            msg_void_f64(layer, sel("setContentsScale:"), scale as f64);
            msg_void_bool(layer, sel("setHidden:"), 1);
            kill_layer_actions(layer);
            // first among the sublayers: over the drawable, under the
            // live layers the shell hangs after it
            msg_void_id_u64(root, sel("insertSublayer:atIndex:"), layer, 0);
            Some(Patch { layer, size: (0, 0), scale, shown: None, drawables: Drawables::default() })
        }
    }

    /// The box in the root layer's points: the Mac counts from the
    /// bottom-left, UIKit from the top-left.
    fn frame(rect: DamageRect, physical: (usize, usize), scale: usize) -> CGRect {
        let scale = scale as f64;
        let y = if cfg!(target_os = "macos") { physical.1 as i64 - rect.3 } else { rect.1 };
        CGRect {
            origin: CGPoint { x: rect.0 as f64 / scale, y: y as f64 / scale },
            size: CGSize {
                width: (rect.2 - rect.0) as f64 / scale,
                height: (rect.3 - rect.1) as f64 / scale,
            },
        }
    }

    /// Hides the layer — inside whatever transaction is open.
    unsafe fn hide(&mut self) {
        if self.shown.take().is_some() {
            unsafe { msg_void_bool(self.layer, sel("setHidden:"), 1) };
            crate::trace::mark("Q", format_args!("hide"));
        }
    }
}

impl Drop for Patch {
    fn drop(&mut self) {
        unsafe {
            self.drawables.forget();
            msg_void(self.layer, sel("removeFromSuperlayer"));
            msg_void(self.layer, sel("release"));
        }
    }
}

pub struct MetalPresenter {
    stack: MetalStack,
    layer: Id,
    physical: (usize, usize),
    scale: usize,
    slots: [FrameSlot; 3],
    cursor: usize,
    ground: MetalGround,
    atlas: RunAtlas,
    batches: FrameBatches,
    /// The last presented frame's key — an identical frame skips the
    /// encode entirely.
    retained: Option<KeptFrame>,
    /// The last WHOLE frame: what the window's own layer shows. A patch
    /// is measured against it, never against the patch before it — so a
    /// patch always covers every pixel the layer under it has wrong.
    base: Option<KeptFrame>,
    /// The layer that carries a change too small to repaint the window
    /// for, made on the first such change.
    patch: Option<Patch>,
    /// Text boxes for the diff against the base, warm across strokes.
    boxes: MeasureCache,
    /// The window's own drawables, offered back while it rests.
    drawables: Drawables,
    /// The window rests: set by [`MetalPresenter::rest`], cleared by the
    /// next frame that paints.
    resting: bool,
    /// Whether the layer currently presents inside the CATransaction —
    /// toggled ON only during live resize.
    transactional: bool,
    /// The scene texture and the blur pyramid, made on the first frame
    /// that carries glass and remade whenever the drawable resizes. A
    /// window that never shows glass never allocates them.
    glass: Option<GlassTextures>,
    /// How long the last present waited for a drawable, in milliseconds.
    drawable_wait_ms: f64,
}

impl MetalPresenter {
    /// Makes the glass textures if this frame needs them and the ones
    /// in hand are the wrong size.
    fn ensure_glass(&mut self, physical: (usize, usize)) {
        if self.batches.glass.is_empty() {
            return;
        }
        unsafe {
            if let Some(textures) = &self.glass {
                if textures.size == physical {
                    return;
                }
                textures.release(&self.stack.sels);
            }
            self.glass =
                GlassTextures::new(self.stack.device, physical, self.stack.format, true);
            if self.glass.is_none() {
                eprintln!("bunny_ui metal: no scene texture — the frame paints without its panes");
            }
        }
    }
}

/// The skip key: the SAME list on the SAME target needs no new frame.
/// The list alone is not enough — a resize or a theme flip with an
/// unchanged list must still re-present, so the physical size, the
/// scale and the clear color all sit in the key (the CPU staleness
/// quadruple, verbatim).
fn frame_repeats(
    retained: &Option<KeptFrame>,
    display: &DisplayList,
    physical: (usize, usize),
    scale: usize,
    canvas: Color,
) -> bool {
    matches!(retained, Some((list, kept_physical, kept_scale, kept_canvas))
        if *kept_physical == physical
            && *kept_scale == scale
            && *kept_canvas == canvas
            && list.as_slice() == display.as_slice())
}

impl MetalPresenter {
    /// The window went to rest. The frames in flight are waited out (the
    /// last one was committed moments ago) and let go, and the atlas — a
    /// cache, rebuilt from the scene whenever it is lost — is offered
    /// back to the system: a resting window keeps its pixels on screen
    /// and needs none of its tiles until the next frame, which asks for
    /// the atlas again and re-rasterizes only if the system took it. So
    /// are the drawables off screen ([`MetalPresenter::offer_drawables`]);
    /// false when that has to wait for the last present to land.
    pub fn rest(&mut self) -> bool {
        unsafe {
            for slot in &mut self.slots {
                if slot.command.is_null() {
                    continue;
                }
                if msg_u64(slot.command, self.stack.sels.status) < STATUS_COMPLETED {
                    msg_void(slot.command, self.stack.sels.wait_completed);
                }
                mark_gpu_time(slot.command, &self.stack.sels);
                msg_void(slot.command, self.stack.sels.release);
                slot.command = null_mut();
                slot.native.clear();
            }
            self.ground.rest();
        }
        self.resting = true;
        self.offer_drawables()
    }

    /// Offers the drawables off screen — the window's and the patch's —
    /// back to the system while the window rests. False when the last
    /// present has not landed: the drawable before it may still be the
    /// one on screen, and the shell asks again a moment later. A window
    /// that painted since it rested answers true and offers nothing.
    pub fn offer_drawables(&mut self) -> bool {
        if !self.resting {
            return true;
        }
        unsafe {
            let window = self.drawables.offer(false);
            let patch = match self.patch.as_mut() {
                Some(patch) => {
                    let hidden = patch.shown.is_none();
                    patch.drawables.offer(hidden)
                }
                None => true,
            };
            let offered = self.drawables.offered()
                + self.patch.as_ref().map_or(0, |patch| patch.drawables.offered());
            if window && patch && offered > 0 {
                crate::trace::mark("X", format_args!("what=drawables-offered n={offered}"));
            }
            window && patch
        }
    }

    /// Flips the layer's present contract and remembers it. The flag
    /// has to be set BEFORE the drawable it governs is asked for: a
    /// drawable taken under the asynchronous contract and then
    /// presented inside the transaction is the one frame the layer
    /// stretches from the size it used to have.
    pub fn set_transactional(&mut self, live: bool) {
        if live == self.transactional {
            return;
        }
        crate::trace::mark(
            "X",
            format_args!("what=sync-{}", if live { "on" } else { "off" }),
        );
        unsafe {
            msg_void_bool(
                self.layer,
                self.stack.sels.set_presents_with_transaction,
                live as i8,
            );
        }
        self.transactional = live;
    }

    /// Waits out every in-flight frame — the precondition of an atlas
    /// reset (the one moment texel space is reused).
    fn drain_slots(&mut self) {
        unsafe {
            for slot in &mut self.slots {
                if !slot.command.is_null() {
                    msg_void(slot.command, self.stack.sels.wait_completed);
                    msg_void(slot.command, self.stack.sels.release);
                    slot.command = null_mut();
                    slot.native.clear();
                }
            }
        }
    }

    /// Walks the frame; on atlas overflow drains the GPU, resets the
    /// atlas (growing once) and walks again — the copying collector.
    fn build_with_retries(
        &mut self,
        display: &DisplayList,
        scale: usize,
        physical: (usize, usize),
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) {
        self.ground.clear_native();
        for attempt in 0..4 {
            match build_frame(
                &mut self.ground,
                display,
                scale,
                physical,
                text,
                images,
                &mut self.atlas,
                &mut self.batches,
            ) {
                Ok(()) => return,
                Err(AtlasFull) => {
                    if attempt == 3 {
                        // pathological frame: keep the rects, drop the
                        // rest of the text — never a crash
                        eprintln!("bunny_ui metal: atlas overflow survived three resets");
                        return;
                    }
                    // the cheap road first: shelves nobody read for a while
                    // are given back with no drain and no re-raster of what
                    // still shows — a scrolled file or a terminal's log
                    // leave rows of text behind that only a reset took back
                    if attempt == 0 {
                        let freed = self.atlas.evict_stale(ATLAS_KEEP_WALKS);
                        if freed > 0 {
                            crate::trace::mark("X", format_args!("what=atlas-evict shelves={freed}"));
                            continue;
                        }
                    }
                    crate::trace::mark("X", format_args!("what=atlas-drain"));
                    self.drain_slots();
                    self.atlas.reset(&mut self.ground, true);
                }
            }
        }
    }

    /// One frame: walk the list, upload, resize the drawable if the
    /// window changed, take the drawable as LATE as possible, encode,
    /// present, commit.
    ///
    /// `live` is the shell's word on whether the surface is being
    /// resized by the hand right now (a live resize on the Mac) — the
    /// frame then presents inside the transaction. A platform whose
    /// surface never resizes under a drag passes `false`.
    pub fn present(
        &mut self,
        display: &DisplayList,
        size: Size,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        live: bool,
    ) {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let physical = (
                (size.width.round().max(0.0) as usize) * scale,
                (size.height.round().max(0.0) as usize) * scale,
            );
            self.drawable_wait_ms = 0.0;
            if physical.0 == 0 || physical.1 == 0 {
                // a zero drawable is an abort, not a frame
                objc_autoreleasePoolPop(pool);
                return;
            }
            if frame_repeats(&self.retained, display, physical, scale, canvas) {
                // the caret blink and friends land here every half
                // second — nothing changed, nothing encodes
                objc_autoreleasePoolPop(pool);
                return;
            }
            // what the frame changes against the window's own layer — a
            // live resize is always whole
            let plan = if live { Plan::Whole } else { self.plan(display, physical, scale, canvas, text) };
            if plan == Plan::Same {
                // the window's layer already shows this frame (a stroke
                // undone, a hover gone): the patch over it steps aside
                // and nothing encodes
                if let Some(patch) = self.patch.as_mut() {
                    let transaction = class("CATransaction");
                    msg_void(transaction, sel("begin"));
                    patch.hide();
                    msg_void(transaction, sel("commit"));
                }
                self.retained = self.base.clone();
                objc_autoreleasePoolPop(pool);
                return;
            }
            if !self.ground.wake() {
                // the system took the atlas while the window rested: its
                // tiles are gone, and the walk below rasterizes anew
                crate::trace::mark("X", format_args!("what=atlas-purged"));
                self.atlas.reset(&mut self.ground, false);
            }
            if physical != self.physical || scale != self.scale {
                // the layer makes new drawables at the new size: the old
                // ones are let go
                self.drawables.forget();
                // the drawable must resize BEFORE nextDrawable, or the
                // frame comes back at the old size
                msg_void_f64(self.layer, self.stack.sels.set_contents_scale, scale as f64);
                msg_void_size(
                    self.layer,
                    self.stack.sels.set_drawable_size,
                    CGSize {
                        width: physical.0 as f64,
                        height: physical.1 as f64,
                    },
                );
                self.physical = physical;
                self.scale = scale;
            }
            // the slot FIRST: the walk stages the feeds' bytes into its
            // buffer, which is free only once its last frame completed
            let index = acquire_slot(&mut self.slots, &mut self.cursor, &self.stack.sels);
            self.ground.begin_slot(index);
            self.build_with_retries(display, scale, physical, text, images);
            if let Plan::Patch(rect) = plan {
                // glass reads the whole scene, so a frame that carries a
                // pane repaints whole (the diff refuses one already)
                if self.batches.glass.is_empty() && self.present_patch(index, rect, canvas, physical, scale) {
                    self.retained = Some((Rc::new(display.clone()), physical, scale, canvas));
                    objc_autoreleasePoolPop(pool);
                    return;
                }
            }
            // a whole frame over a patch takes the patch down in the same
            // transaction — apart, the box would show the old frame for
            // one refresh
            let hiding = self.patch.as_ref().is_some_and(|patch| patch.shown.is_some());
            // the contract of THIS frame's drawable, settled before it
            // is asked for. A window whose delegate armed the drag
            // already agrees and this changes nothing; a size the app
            // set itself has no delegate to speak for it, and lands here
            self.set_transactional(live || hiding);
            let (sprite_offset, glass_offset) = upload_frame(
                &mut self.slots[index],
                self.stack.device,
                &self.stack.sels,
                &self.batches,
            );
            let asked = crate::trace::clock_ms();
            let drawable = msg_id(self.layer, self.stack.sels.next_drawable);
            self.drawable_wait_ms = crate::trace::clock_ms() - asked;
            if drawable.is_null() {
                objc_autoreleasePoolPop(pool);
                return;
            }
            let drawable_texture = msg_id(drawable, self.stack.sels.texture);
            self.resting = false;
            self.drawables.take(drawable_texture);
            // a drawable cannot be READ, so a frame that carries glass
            // renders into a scene texture of its own and is copied
            // over at the end. A frame without glass never asks for
            // one, and never pays for one
            self.ensure_glass(physical);
            let pyramid = (!self.batches.glass.is_empty()).then_some(()).and(self.glass.as_ref());
            let (target, present_to) = match pyramid {
                Some(textures) => (textures.scene, drawable_texture),
                None => (drawable_texture, null_mut()),
            };
            self.slots[index].native = self.ground.native.values().cloned().collect();
            let textures = self.ground.bound(&self.batches.textures);
            let command = self.stack.encode_frame(EncodeFrame {
                target,
                present_to,
                canvas,
                viewport: (physical.0 as f32, physical.1 as f32),
                origin: (0.0, 0.0),
                instances: self.slots[index].buffer,
                sprite_offset,
                glass_offset,
                runs: &self.batches.runs,
                rounds: &self.batches.rounds,
                atlas_texture: self.ground.shared,
                textures: &textures,
                pyramid,
                staging: self.ground.staging_buffer(),
                live_copies: &self.ground.pending,
            });
            // live resize presents INSIDE the CATransaction: commit,
            // wait for the schedule, present — layer content and window
            // frame land together (the anti-tear toggle). A frame that
            // takes a patch down does the same with the patch. Every
            // other frame presents async, no stall.
            if live || hiding {
                msg_void(command, self.stack.sels.commit);
                msg_void(command, self.stack.sels.wait_scheduled);
                let transaction = class("CATransaction");
                msg_void(transaction, sel("begin"));
                msg_void(drawable, self.stack.sels.present);
                if let Some(patch) = self.patch.as_mut() {
                    patch.hide();
                }
                msg_void(transaction, sel("commit"));
            } else {
                msg_void_id(command, self.stack.sels.present_drawable, drawable);
                msg_void(command, self.stack.sels.commit);
            }
            self.slots[index].command = msg_id(command, self.stack.sels.retain);
            self.drawables.presented(drawable);
            let kept = (Rc::new(display.clone()), physical, scale, canvas);
            self.base = Some(kept.clone());
            self.retained = Some(kept);
            objc_autoreleasePoolPop(pool);
        }
    }

    /// The plan for `display` against the frame the window's layer
    /// shows: the same frame, a box small enough for a patch, or the
    /// whole window. No base yet, or a base of another size, scale or
    /// colour, is the whole window.
    fn plan(
        &mut self,
        display: &DisplayList,
        physical: (usize, usize),
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
    ) -> Plan {
        let Some((base, base_physical, base_scale, base_canvas)) = &self.base else {
            return Plan::Whole;
        };
        if *base_physical != physical || *base_scale != scale || *base_canvas != canvas {
            return Plan::Whole;
        }
        self.boxes.begin_frame();
        match list_damage(
            base.as_slice(),
            display.as_slice(),
            scale,
            physical,
            PATCH_COMMANDS,
            &self.boxes,
            text,
        ) {
            ListDamage::Same => Plan::Same,
            ListDamage::Whole => Plan::Whole,
            ListDamage::Rect(damage) => patch_box(damage, physical).map_or(Plan::Whole, Plan::Patch),
        }
    }

    /// Presents `rect` of the frame just walked into slot `index`
    /// through the patch layer, which the window's own layer keeps
    /// showing the base under. The patch presents inside the
    /// transaction, with its new geometry when the box moved, resized or
    /// shows from hiding. False when no patch layer can be had — the
    /// caller paints whole.
    unsafe fn present_patch(
        &mut self,
        index: usize,
        rect: DamageRect,
        canvas: Color,
        physical: (usize, usize),
        scale: usize,
    ) -> bool {
        unsafe {
            if self.patch.is_none() {
                self.patch = Patch::new(&self.stack, self.layer, scale);
            }
            let Some(patch) = self.patch.as_mut() else {
                return false;
            };
            let size = ((rect.2 - rect.0) as usize, (rect.3 - rect.1) as usize);
            let moves = patch.shown != Some(rect);
            if patch.scale != scale {
                msg_void_f64(patch.layer, self.stack.sels.set_contents_scale, scale as f64);
                patch.scale = scale;
            }
            if patch.size != size {
                patch.drawables.forget();
                msg_void_size(
                    patch.layer,
                    self.stack.sels.set_drawable_size,
                    CGSize { width: size.0 as f64, height: size.1 as f64 },
                );
                patch.size = size;
            }
            let (sprite_offset, glass_offset) = upload_frame(
                &mut self.slots[index],
                self.stack.device,
                &self.stack.sels,
                &self.batches,
            );
            let drawable = msg_id(patch.layer, self.stack.sels.next_drawable);
            if drawable.is_null() {
                return false;
            }
            let texture = msg_id(drawable, self.stack.sels.texture);
            self.resting = false;
            patch.drawables.take(texture);
            self.slots[index].native = self.ground.native.values().cloned().collect();
            let textures = self.ground.bound(&self.batches.textures);
            let command = self.stack.encode_frame(EncodeFrame {
                target: texture,
                present_to: null_mut(),
                canvas,
                viewport: (size.0 as f32, size.1 as f32),
                origin: (rect.0 as f32, rect.1 as f32),
                instances: self.slots[index].buffer,
                sprite_offset,
                glass_offset,
                runs: &self.batches.runs,
                rounds: &self.batches.rounds,
                atlas_texture: self.ground.shared,
                textures: &textures,
                pyramid: None,
                staging: self.ground.staging_buffer(),
                live_copies: &self.ground.pending,
            });
            msg_void(command, self.stack.sels.commit);
            msg_void(command, self.stack.sels.wait_scheduled);
            let transaction = class("CATransaction");
            msg_void(transaction, sel("begin"));
            if moves {
                msg_void_rect(patch.layer, sel("setFrame:"), Patch::frame(rect, physical, scale));
                msg_void_bool(patch.layer, sel("setHidden:"), 0);
                patch.shown = Some(rect);
            }
            msg_void(drawable, self.stack.sels.present);
            msg_void(transaction, sel("commit"));
            patch.drawables.presented(drawable);
            self.slots[index].command = msg_id(command, self.stack.sels.retain);
            crate::trace::mark(
                "Q",
                format_args!(
                    "box={},{},{},{}{}",
                    rect.0,
                    rect.1,
                    rect.2,
                    rect.3,
                    if moves { " moved" } else { "" }
                ),
            );
            true
        }
    }
}

impl MetalPresenter {
    /// Builds a presenter over a `CAMetalLayer` the shell already holds
    /// — the layer the Mac grafts with `setLayer:`, the view's own
    /// `+layerClass` layer on iOS. Configures the layer for the road
    /// (device, format, opaque, framebuffer-only, three drawables, a
    /// blocking `nextDrawable`, the scale) and kills its implicit
    /// actions. Answers `None` when the GPU road is refused
    /// (`BUNNY_PRESENT=cpu`) or cannot come up, and touches the layer
    /// only on `Some`.
    ///
    /// The default is the GPU. `BUNNY_PRESENT=cpu` forces the CPU raster
    /// forever; any failure to come up (no device, a shader that does
    /// not compile) prints one line and the shell falls back — a window
    /// never fails to open because of Metal.
    pub fn attach(layer: Id, scale: f64) -> Option<MetalPresenter> {
        if std::env::var("BUNNY_PRESENT").ok().as_deref() == Some("cpu") {
            return None;
        }
        if layer.is_null() {
            return None;
        }
        let stack = MetalStack::create(PIXEL_FORMAT_BGRA8)?;
        unsafe {
            let scale = scale.round().max(1.0);
            msg_void_id(layer, sel("setDevice:"), stack.device);
            msg_void_u64(layer, sel("setPixelFormat:"), PIXEL_FORMAT_BGRA8);
            msg_void_bool(layer, sel("setOpaque:"), 1);
            msg_void_bool(layer, sel("setFramebufferOnly:"), 1);
            msg_void_u64(layer, sel("setMaximumDrawableCount:"), 3);
            // nextDrawable BLOCKS instead of returning nil — the natural
            // frame pacing for an event-driven present
            msg_void_bool(layer, sel("setAllowsNextDrawableTimeout:"), 0);
            msg_void_f64(layer, sel("setContentsScale:"), scale);
            // the layer is OURS, so the platform never disables CA's
            // implicit actions on it — and an abrupt resize step would
            // crossfade the old drawable over the new for a quarter
            // second, the whole window double-exposed (no native window
            // does this)
            kill_layer_actions(layer);
        }
        let device = stack.device;
        Some(MetalPresenter {
            stack,
            layer,
            physical: (0, 0),
            scale: 0,
            slots: std::array::from_fn(|_| FrameSlot::empty()),
            cursor: 0,
            ground: MetalGround::new(device),
            atlas: RunAtlas::new(),
            batches: FrameBatches::default(),
            retained: None,
            base: None,
            patch: None,
            boxes: MeasureCache::default(),
            drawables: Drawables::default(),
            resting: false,
            transactional: false,
            glass: None,
            drawable_wait_ms: 0.0,
        })
    }

    /// How long the last present waited for a drawable, in milliseconds —
    /// zero for a frame that was skipped.
    ///
    /// A layer holds three drawables: one on the glass, two that wait for
    /// their refresh. A frame that misses its refresh takes the next one,
    /// which the frame after it wanted; from there every present finds all
    /// three taken and waits for the display to free one — most of a beat,
    /// inside the handler, and every frame shown is a refresh older than it
    /// had to be. One frame a beat never drains that line. A shell reads
    /// this after a present and holds ONE beat when the wait says the line
    /// is full (`FramePacer::congested`): the line drains and stays short.
    /// Diagnostics: what the text atlas holds — the run tiles, the
    /// image tiles, the dedicated textures, the live feeds, and the
    /// atlas's side in texels. A tape prints it beside the engine's own
    /// counts, so a tile population that only grows is named.
    pub fn atlas_counts(&self) -> AtlasCounts {
        AtlasCounts {
            runs: self.atlas.entries.values().map(Vec::len).sum(),
            images: self.atlas.images.len(),
            dedicated: self.atlas.dedicated.len(),
            live: self.atlas.live.len(),
            size: self.atlas.size,
        }
    }

    pub fn drawable_wait_ms(&self) -> f64 {
        self.drawable_wait_ms
    }

    /// The anti-flash frame: one clear at the size the surface will
    /// show, BEFORE it shows. A virgin `CAMetalLayer` flashes black on
    /// its first appearance otherwise.
    pub fn prime(&mut self, width: f64, height: f64, scale: usize) {
        self.present(
            &DisplayList::default(),
            Size { width, height },
            scale,
            bunny_ui::theme::canvas(),
            &bunny_ui::text_engine::PixelFont,
            &bunny_ui::image_engine::RawImages::default(),
            false,
        );
    }
}

// MARK: - Offscreen target (parity tests and the bench)

/// A windowless render target: same stack, same shaders, RGBA byte order
/// so `read_rgba` lines up with the CPU mirror byte for byte. This is the
/// harness surface — the parity tests and the benchmark present here.
pub struct OffscreenGpu {
    stack: MetalStack,
    target: Id,
    /// The blur pyramid, made on the first frame that carries glass.
    glass: Option<GlassTextures>,
    width: usize,
    height: usize,
    slots: [FrameSlot; 3],
    cursor: usize,
    ground: MetalGround,
    atlas: RunAtlas,
    batches: FrameBatches,
}

impl OffscreenGpu {
    /// Makes a target of `width`×`height` device pixels. `None` when
    /// there is no Metal device or the shaders do not compile.
    pub fn new(width: usize, height: usize) -> Option<OffscreenGpu> {
        if width == 0 || height == 0 {
            return None;
        }
        let stack = MetalStack::create(PIXEL_FORMAT_RGBA8)?;
        unsafe {
            let target = readable_target(stack.device, width, height)?;
            let device = stack.device;
            Some(OffscreenGpu {
                stack,
                target,
                glass: None,
                width,
                height,
                slots: std::array::from_fn(|_| FrameSlot::empty()),
                cursor: 0,
                ground: MetalGround::new(device),
                atlas: RunAtlas::new(),
                batches: FrameBatches::default(),
            })
        }
    }

    /// Walks the frame into the batches, resetting the atlas on overflow
    /// as the window does.
    fn walk(&mut self, display: &DisplayList, scale: usize, text: &dyn TextEngine, images: &dyn ImageEngine) {
        self.ground.clear_native();
        for attempt in 0..4 {
            match build_frame(
                &mut self.ground,
                display,
                scale,
                (self.width, self.height),
                text,
                images,
                &mut self.atlas,
                &mut self.batches,
            ) {
                Ok(()) => break,
                Err(AtlasFull) => {
                    if attempt == 3 {
                        eprintln!("bunny_ui metal: atlas overflow survived three resets");
                        break;
                    }
                    self.drain();
                    self.atlas.reset(&mut self.ground, true);
                }
            }
        }
    }

    fn drain(&mut self) {
        unsafe {
            for slot in &mut self.slots {
                if !slot.command.is_null() {
                    msg_void(slot.command, self.stack.sels.wait_completed);
                    msg_void(slot.command, self.stack.sels.release);
                    slot.command = null_mut();
                    slot.native.clear();
                }
            }
        }
    }

    fn present_inner(
        &mut self,
        display: &DisplayList,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        wait: bool,
    ) {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            // the slot first, as the window does: the feeds stage into it
            let index = acquire_slot(&mut self.slots, &mut self.cursor, &self.stack.sels);
            self.ground.begin_slot(index);
            self.walk(display, scale, text, images);
            let (sprite_offset, glass_offset) = upload_frame(
                &mut self.slots[index],
                self.stack.device,
                &self.stack.sels,
                &self.batches,
            );
            // the harness target is READABLE, so the panes read it
            // where it lies: no scene texture, no copy at the end
            if !self.batches.glass.is_empty() && self.glass.is_none() {
                self.glass = GlassTextures::new(
                    self.stack.device,
                    (self.width, self.height),
                    self.stack.format,
                    false,
                );
            }
            let pyramid = (!self.batches.glass.is_empty()).then_some(()).and(self.glass.as_ref());
            self.slots[index].native = self.ground.native.values().cloned().collect();
            let textures = self.ground.bound(&self.batches.textures);
            let command = self.stack.encode_frame(EncodeFrame {
                target: self.target,
                present_to: null_mut(),
                canvas,
                viewport: (self.width as f32, self.height as f32),
                origin: (0.0, 0.0),
                instances: self.slots[index].buffer,
                sprite_offset,
                glass_offset,
                runs: &self.batches.runs,
                rounds: &self.batches.rounds,
                atlas_texture: self.ground.shared,
                textures: &textures,
                pyramid,
                staging: self.ground.staging_buffer(),
                live_copies: &self.ground.pending,
            });
            msg_void(command, self.stack.sels.commit);
            self.slots[index].command = msg_id(command, self.stack.sels.retain);
            if wait {
                msg_void(command, self.stack.sels.wait_completed);
            }
            objc_autoreleasePoolPop(pool);
        }
    }

    /// Renders and WAITS — determinism for tests and honest numbers for
    /// the bench (walk + upload + encode + commit + GPU time, nothing
    /// hidden).
    pub fn present_wait(
        &mut self,
        display: &DisplayList,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) {
        self.present_inner(display, scale, canvas, text, images, true);
    }

    /// Renders and RETURNS after the commit — the CPU-side cost of a
    /// production present (a window commits and moves on; the ring keeps
    /// the in-flight frames safe).
    pub fn present_nowait(
        &mut self,
        display: &DisplayList,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) {
        self.present_inner(display, scale, canvas, text, images, false);
    }

    /// The atlas footprint — how many cached runs, images and dedicated
    /// textures, and how deep the shelves go. The warm-frame tests pin
    /// upload reuse with it.
    #[cfg(test)]
    fn atlas_footprint(&self) -> (usize, u32) {
        self.atlas.footprint()
    }

    /// The rendered bytes, R,G,B,A per pixel — the same order as the
    /// Surface mirror, so parity compares are `==` over slices.
    pub fn read_rgba(&self) -> Vec<u8> {
        unsafe { read_target(self.target, self.width, self.height, &self.stack.sels) }
    }

    /// Renders the box `rect` (physical, top-left: x0, y0, x1, y1) of
    /// `display` into a target of the box's own size — the patch a
    /// window presents over its last whole frame — and returns its RGBA
    /// bytes, row by row.
    pub fn patch_rgba(
        &mut self,
        display: &DisplayList,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        rect: (usize, usize, usize, usize),
    ) -> Vec<u8> {
        let (width, height) = (rect.2 - rect.0, rect.3 - rect.1);
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let index = acquire_slot(&mut self.slots, &mut self.cursor, &self.stack.sels);
            self.ground.begin_slot(index);
            self.walk(display, scale, text, images);
            let (sprite_offset, glass_offset) = upload_frame(
                &mut self.slots[index],
                self.stack.device,
                &self.stack.sels,
                &self.batches,
            );
            let target = readable_target(self.stack.device, width, height).expect("a patch target");
            self.slots[index].native = self.ground.native.values().cloned().collect();
            let textures = self.ground.bound(&self.batches.textures);
            let command = self.stack.encode_frame(EncodeFrame {
                target,
                present_to: null_mut(),
                canvas,
                viewport: (width as f32, height as f32),
                origin: (rect.0 as f32, rect.1 as f32),
                instances: self.slots[index].buffer,
                sprite_offset,
                glass_offset,
                runs: &self.batches.runs,
                rounds: &self.batches.rounds,
                atlas_texture: self.ground.shared,
                textures: &textures,
                pyramid: None,
                staging: self.ground.staging_buffer(),
                live_copies: &self.ground.pending,
            });
            msg_void(command, self.stack.sels.commit);
            msg_void(command, self.stack.sels.wait_completed);
            self.slots[index].command = msg_id(command, self.stack.sels.retain);
            let bytes = read_target(target, width, height, &self.stack.sels);
            msg_void(target, self.stack.sels.release);
            objc_autoreleasePoolPop(pool);
            bytes
        }
    }
}

/// A render target the CPU can read: RGBA8, shared storage (the
/// Apple-Silicon premise of the module).
unsafe fn readable_target(device: Id, width: usize, height: usize) -> Option<Id> {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let descriptor = msg_id_u64_u64_u64_bool(
            class("MTLTextureDescriptor"),
            sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
            PIXEL_FORMAT_RGBA8,
            width as u64,
            height as u64,
            0,
        );
        msg_void_u64(
            descriptor,
            sel("setUsage:"),
            TEXTURE_USAGE_RENDER_TARGET | TEXTURE_USAGE_SHADER_READ,
        );
        msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_SHARED);
        let target = msg_id_arg(device, sel("newTextureWithDescriptor:"), descriptor);
        objc_autoreleasePoolPop(pool);
        (!target.is_null()).then_some(target)
    }
}

/// A readable target's bytes, R,G,B,A per pixel, row by row.
unsafe fn read_target(target: Id, width: usize, height: usize, sels: &Sels) -> Vec<u8> {
    let mut bytes = vec![0u8; width * height * 4];
    unsafe {
        msg_void_ptr_u64_region_u64(
            target,
            sels.get_bytes,
            bytes.as_mut_ptr() as *mut c_void,
            (width * 4) as u64,
            MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize { width: width as u64, height: height as u64, depth: 1 },
            },
            0,
        );
    }
    bytes
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::prelude::*;
    use bunny_ui::raster::rasterize_with;

    fn device_present() -> bool {
        unsafe {
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                eprintln!("no metal device — skipping");
                return false;
            }
            msg_void(device, sel("release"));
            true
        }
    }

    /// Renders the same scene by both backends: the GPU offscreen target
    /// and the CPU raster oracle, byte-comparable RGBA out of each.
    fn scene_bytes(
        root: &impl View,
        logical: Size,
        scale: usize,
        canvas: Color,
    ) -> (Vec<u8>, Vec<u8>) {
        let physical = (
            (logical.width.round() as usize) * scale,
            (logical.height.round() as usize) * scale,
        );
        let runtime = Runtime::new();
        let display = runtime.display_frame(root, logical);
        let cpu = rasterize_with(&display, physical.0, physical.1, scale, canvas, &PixelFont, &RawImages::default())
            .to_rgba_bytes();
        let mut gpu = OffscreenGpu::new(physical.0, physical.1).expect("offscreen gpu");
        gpu.present_wait(&display, scale, canvas, &PixelFont, &RawImages::default());
        (gpu.read_rgba(), cpu)
    }

    fn max_channel_delta(a: &[u8], b: &[u8]) -> u8 {
        a.iter().zip(b.iter()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
    }

    /// The parity gate for anti-aliased scenes: every channel within
    /// `max_delta`, and at most 1% of channels beyond one step (float
    /// coverage vs the cpu's two integer roundings).
    fn assert_close(gpu: &[u8], cpu: &[u8], max_delta: u8, label: &str) {
        assert_eq!(gpu.len(), cpu.len(), "{label}: byte lengths differ");
        let mut worst = 0u8;
        let mut beyond_one = 0usize;
        for (a, b) in gpu.iter().zip(cpu.iter()) {
            let delta = a.abs_diff(*b);
            worst = worst.max(delta);
            if delta > 1 {
                beyond_one += 1;
            }
        }
        assert!(worst <= max_delta, "{label}: worst channel delta {worst} (allowed {max_delta})");
        let share = beyond_one as f64 / gpu.len() as f64;
        assert!(
            share <= 0.01,
            "{label}: {beyond_one} channels beyond one step ({:.3}% > 1%)",
            share * 100.0
        );
    }

    /// A scene for the patch tests: a panel, a rounded clip over rows
    /// and text, a pill that moves by fractions of a point, a stroke and
    /// a shadow — every pipeline that reads its pixel's position.
    fn patch_scene(word: &str, hot: bool, shift: f64) -> DisplayList {
        use bunny_ui::layout::{Corners, DrawCommand, Point, Rect};
        let rect = |x: f64, y: f64, w: f64, h: f64| Rect {
            origin: Point { x, y },
            size: Size { width: w, height: h },
        };
        let line = |x: f64, y: f64, text: &str| DrawCommand::TextLine {
            origin: Point { x, y },
            content: std::sync::Arc::from(text),
            range: (0, text.len()),
            color: Color::BLACK,
            font: bunny_ui::text_engine::FontSpec::DEFAULT,
        };
        DisplayList::from(vec![
            DrawCommand::FillRect { rect: rect(0.0, 0.0, 160.0, 100.0), color: Color::WHITE, corner_radius: Corners::ZERO },
            DrawCommand::PushClip { rect: rect(6.0, 6.0, 148.0, 80.0), corner_radius: Corners::all(10.0) },
            DrawCommand::FillRect {
                rect: rect(8.0, 10.0, 140.0, 18.0),
                color: if hot { Color::hex(0xD8DCE6) } else { Color::hex(0xEEEEF2) },
                corner_radius: Corners::all(4.0),
            },
            line(12.0, 12.0, word),
            DrawCommand::FillRect { rect: rect(8.0, 32.0, 140.0, 18.0), color: Color::hex(0xEEEEF2), corner_radius: Corners::ZERO },
            line(12.0, 34.0, "beta"),
            DrawCommand::Shadow { rect: rect(20.0, 56.0 + shift, 40.0, 12.0), radius: 4.0, color: Color::BLACK, corner_radius: Corners::all(6.0) },
            DrawCommand::FillRect { rect: rect(20.0, 56.0 + shift, 40.0, 12.0), color: Color::hex(0x3366CC), corner_radius: Corners::all(6.0) },
            DrawCommand::StrokeRect { rect: rect(70.0 + shift, 56.0, 40.0, 12.0), color: Color::BLACK, width: 1.5, corner_radius: Corners::all(3.0) },
            DrawCommand::PopClip,
            line(10.0, 88.0, "footer"),
        ])
    }

    #[test]
    fn a_patch_over_the_last_whole_frame_is_the_new_frame_byte_for_byte() {
        // the window keeps its last whole frame and lays the patch over
        // it: the two together must be the new frame, pixel for pixel —
        // for the exact damage box and for the box snapped to the grid
        if !device_present() {
            return;
        }
        let scale = 2;
        let physical = (320usize, 200usize);
        let cache = MeasureCache::default();
        let mut gpu = OffscreenGpu::new(physical.0, physical.1).expect("offscreen gpu");
        let pairs = [
            (("alpha", false, 0.0), ("alphax", false, 0.0)),
            (("alpha", false, 0.0), ("alpha", true, 0.0)),
            (("alpha", false, 0.0), ("alpha", false, 1.25)),
            (("beta", true, 0.75), ("b", false, 0.0)),
        ];
        for (from, to) in pairs {
            let old = patch_scene(from.0, from.1, from.2);
            let new = patch_scene(to.0, to.1, to.2);
            gpu.present_wait(&new, scale, Color::CANVAS, &PixelFont, &RawImages::default());
            let whole = gpu.read_rgba();
            gpu.present_wait(&old, scale, Color::CANVAS, &PixelFont, &RawImages::default());
            let base = gpu.read_rgba();
            let ListDamage::Rect(damage) =
                list_damage(old.as_slice(), new.as_slice(), scale, physical, 64, &cache, &PixelFont)
            else {
                panic!("{from:?} -> {to:?} boxes");
            };
            for rect in [damage, snap_patch(damage, physical).expect("on the window")] {
                let rect = (rect.0 as usize, rect.1 as usize, rect.2 as usize, rect.3 as usize);
                let patch = gpu.patch_rgba(&new, scale, Color::CANVAS, &PixelFont, &RawImages::default(), rect);
                let row = (rect.2 - rect.0) * 4;
                let mut composed = base.clone();
                for y in rect.1..rect.3 {
                    let at = (y * physical.0 + rect.0) * 4;
                    composed[at..at + row].copy_from_slice(&patch[(y - rect.1) * row..(y - rect.1 + 1) * row]);
                }
                if let Some(first) = composed.iter().zip(&whole).position(|(a, b)| a != b) {
                    let pixel = first / 4;
                    panic!(
                        "{from:?} -> {to:?} under {rect:?}: pixel ({}, {}) differs",
                        pixel % physical.0,
                        pixel / physical.0
                    );
                }
            }
        }
    }

    #[test]
    fn a_patch_box_snaps_to_the_grid_and_a_big_change_is_whole() {
        let window = (2560, 1600);
        assert_eq!(snap_patch((70, 10, 130, 20), window), Some((64, 0, 192, 64)));
        assert_eq!(snap_patch((2500, 1590, 2600, 1700), window), Some((2496, 1536, 2560, 1600)));
        assert_eq!(snap_patch((-40, -40, -1, -1), window), None);
        // a quarter of the window is the most a patch covers
        assert_eq!(patch_box((0, 0, 2560, 380), window), Some((0, 0, 2560, 384)));
        assert_eq!(patch_box((0, 0, 2560, 400), window), None);
    }

    #[test]
    fn the_wire_structs_hold_their_layout() {
        // the const asserts already gate the build; this pins the numbers
        // in a place a failing CI can point at
        // 80 since the four corners: the sixteen bytes buy every
        // pipeline a band that rounds only the corners that end it
        assert_eq!(std::mem::size_of::<RectInstance>(), 80);
        assert_eq!(std::mem::align_of::<RectInstance>(), 4);
        assert_eq!(std::mem::size_of::<SpriteInstance>(), 48);
        assert_eq!(std::mem::size_of::<RoundClip>(), 32);
    }

    #[test]
    fn the_device_compiles_the_library_and_both_pipelines() {
        if !device_present() {
            return;
        }
        let stack = MetalStack::create(PIXEL_FORMAT_RGBA8);
        assert!(stack.is_some(), "the runtime shader compile must succeed");
    }

    #[test]
    fn a_clear_frame_reads_back_the_canvas_color_exactly() {
        if !device_present() {
            return;
        }
        // this test is the ABI smoke: MTLClearColor rides registers (HFA)
        // and MTLRegion rides memory (indirect) — a wrong convention in
        // either alias corrupts the readback loudly
        let canvas = Color::hex(0x18181D);
        let mut gpu = OffscreenGpu::new(16, 16).expect("offscreen gpu");
        gpu.present_wait(&DisplayList::default(), 2, canvas, &PixelFont, &RawImages::default());
        let bytes = gpu.read_rgba();
        assert_eq!(bytes.len(), 16 * 16 * 4);
        for pixel in bytes.chunks_exact(4) {
            assert_eq!(pixel, [0x18, 0x18, 0x1D, 0xFF]);
        }
    }

    #[test]
    fn flat_opaque_rects_match_byte_for_byte() {
        if !device_present() {
            return;
        }
        let root = vstack((
            empty().frame(120.0, 40.0).background_color(Color::hex(0x3B82F6)),
            empty()
                .frame(80.0, 24.0)
                .background_color(Color::hex(0x18181D))
                .padding_length(10.0)
                .background_color(Color::hex(0xDDE1E9)),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 200.0, height: 120.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "flat opaque scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn a_band_with_four_different_corners_matches_the_raster() {
        if !device_present() {
            return;
        }
        // the figure the four corners exist for: a selection over three
        // lines. The first band rounds its top, the middle is square,
        // the last rounds its bottom — and the sides that MEET carry a
        // square corner beside a rounded one, which no single radius
        // can ask for
        use bunny_ui::layout::Corners;
        let tint = Color::rgba(59, 130, 246, 120);
        let root = vstack((
            empty().frame(90.0, 20.0).background_color(tint).corner_radius(Corners::top(6.0)),
            empty().frame(120.0, 20.0).background_color(tint),
            empty().frame(70.0, 20.0).background_color(tint).corner_radius(Corners::bottom(6.0)),
            // and four radii that share nothing, to pin the order
            empty().frame(80.0, 40.0).background_color(Color::hex(0x18181D)).corner_radius(
                Corners { top_left: 2.0, top_right: 10.0, bottom_right: 4.0, bottom_left: 16.0 },
            ),
        ))
        .padding_length(8.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 140.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "the four corners drifted by {delta} (allowed 1)");
    }

    #[test]
    fn a_cut_with_four_corners_matches_the_raster() {
        if !device_present() {
            return;
        }
        // the same four on a CLIP: the curve rides the per-run uniform,
        // which had room for them all along
        use bunny_ui::layout::Corners;
        let root = vstack((
            empty().frame(200.0, 30.0).background_color(Color::hex(0x3B82F6)),
            empty().frame(200.0, 30.0).background_color(Color::hex(0xF59E0B)),
        ))
        .frame(110.0, 50.0)
        .corner_radius(Corners { top_left: 14.0, top_right: 0.0, bottom_right: 14.0, bottom_left: 0.0 })
        .clipped()
        .padding_length(10.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 140.0, height: 80.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "the four-cornered cut drifted by {delta} (allowed 1)");
    }

    #[test]
    fn translucent_veils_match_within_one() {
        if !device_present() {
            return;
        }
        // stacked veils: float source-over vs the CPU's rounded div255 —
        // at most one step apart, per channel
        let veil = Color::rgba(0, 0, 0, 90);
        let root = zstack((
            empty().frame(140.0, 100.0).background_color(Color::hex(0xE7EAF1)),
            empty().frame(100.0, 70.0).background_color(veil),
            empty().frame(60.0, 40.0).background_color(veil),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 120.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "veils drifted by {delta} (allowed 1)");
    }

    #[test]
    fn nested_clips_cut_identically() {
        if !device_present() {
            return;
        }
        // a list taller than its frame: the scroll clip cuts rows and
        // the translucent scrollbar rides on top
        let rows: Vec<usize> = (0..12).collect();
        let root = list(rows, |row| row.to_string(), |row| {
            let tint = if row % 2 == 0 { Color::hex(0x3B82F6) } else { Color::hex(0xDDE1E9) };
            empty().frame(160.0, 24.0).background_color(tint)
        })
        .frame(180.0, 100.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 200.0, height: 120.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "clipped scene drifted by {delta} (allowed 1)");
    }

    /// The whole front on one screen: a bordered rounded island with
    /// .clipped(), text crossing its corner, a child panel with its
    /// own background, and a SCROLL nested inside — rects, sprites and
    /// the inheritance rule all under the AA gate at once.
    #[test]
    fn a_rounded_clip_cuts_the_same_corner_on_both_backends() {
        if !device_present() {
            return;
        }
        let rows: Vec<usize> = (0..8).collect();
        let root = vstack((
            text("corner text").foreground_color(Color::hex(0x202531)),
            empty().frame(150.0, 18.0).background_color(Color::hex(0xAA3322)),
            list(rows, |row| row.to_string(), |row| {
                let tint =
                    if row % 2 == 0 { Color::hex(0x3B82F6) } else { Color::hex(0xDDE1E9) };
                empty().frame(150.0, 16.0).background_color(tint)
            })
            .frame(160.0, 60.0),
        ))
        .background_color(Color::hex(0xF0F2F6))
        .border(Color::hex(0x202531), 1.0)
        .corner_radius(10.0)
        .clipped()
        .frame(170.0, 120.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 190.0, height: 140.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "rounded clip");
    }

    /// Radius zero through the whole new plumbing: the strict tier
    /// must not move — the curve is exactly nothing when absent.
    #[test]
    fn a_clipped_box_without_a_radius_stays_strict() {
        if !device_present() {
            return;
        }
        let root = vstack((
            text("square").foreground_color(Color::hex(0x202531)),
            empty().frame(120.0, 20.0).background_color(Color::hex(0x3B82F6)),
        ))
        .background_color(Color::hex(0xF0F2F6))
        .clipped()
        .frame(140.0, 34.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 60.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "the straight cut drifted by {delta} (allowed 1)");
    }

    #[test]
    fn rounded_fill_within_tolerance() {
        if !device_present() {
            return;
        }
        // the finder radius and an exaggerated one, on a dark canvas —
        // the corner-bug configuration, now judged by the oracle
        let root = vstack((
            empty().frame(140.0, 60.0).background_color(Color::hex(0xF2F3F7)).corner_radius(9.0),
            empty().frame(140.0, 90.0).background_color(Color::hex(0x3B82F6)).corner_radius(40.0),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 180.0, height: 200.0 }, 2, Color::hex(0x18181D));
        assert_close(&gpu, &cpu, 2, "rounded fills");
    }

    #[test]
    fn stroke_ring_never_double_blends() {
        if !device_present() {
            return;
        }
        // a TRANSLUCENT border is the double-blend trap: straight bars
        // must meet without overlap, the rounded ring must follow the
        // curve — one blend per pixel on both backends
        let veil = Color::rgba(0, 0, 0, 90);
        let root = vstack((
            empty().frame(120.0, 40.0).border(veil, 1.0),
            empty().frame(120.0, 40.0).border(veil, 3.0).corner_radius(12.0),
            empty()
                .frame(120.0, 40.0)
                .background_color(Color::hex(0xDDE1E9))
                .border(Color::hex(0x3B82F6), 2.0)
                .corner_radius(9.0),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 160.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "stroke rings");
    }

    #[test]
    fn the_gradients_ramp_the_same_on_both_backends() {
        if !device_present() {
            return;
        }
        use bunny_ui::layout::{Gradient, UnitPoint};
        // rings off-centre with a rounded box, a ramp across a wide
        // one, and a glow that fades to its own color with no alpha —
        // the three shapes a product paints
        let violet = Color::hex(0x8B5CF6);
        let root = vstack((
            empty()
                .frame(120.0, 60.0)
                .background_gradient(
                    Gradient::radial(Color::hex(0xE879F9), Color::hex(0x1E1B4B))
                        .center(UnitPoint::TOP_LEADING)
                        .radius(4.0, 90.0),
                )
                .corner_radius(12.0),
            empty().frame(120.0, 40.0).background_gradient(Gradient::linear(
                Color::hex(0x0EA5E9),
                Color::hex(0x14532D),
            )),
            empty().frame(120.0, 60.0).background_gradient(
                Gradient::radial(violet, violet.fade()).center(UnitPoint::BOTTOM),
            ),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 180.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "gradients");
    }

    #[test]
    fn the_elliptical_wash_ramps_the_same_on_both_backends() {
        if !device_present() {
            return;
        }
        // the D89 numbers, scaled down: an elliptical wash across a
        // wide bar — the aspect rides the corner slot, so the kinds
        // diverge from the circle and the two rasterizers must agree
        let root = empty().frame(280.0, 90.0).background_gradient(
            Gradient::radial(Color::hex(0x7C5CFF), Color::hex(0x7C5CFF).fade())
                .center(UnitPoint::TOP_LEADING)
                .radius(98.0, 140.0)
                .aspect(65.0 / 140.0),
        );
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 300.0, height: 110.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "elliptical wash");
    }

    #[test]
    fn shadow_quadratic_falloff_matches() {
        if !device_present() {
            return;
        }
        // the halo and the notch behind the rounded corner — quadratic
        // falloff, strictly outside the shape
        let root = empty()
            .frame(120.0, 80.0)
            .background_color(Color::hex(0xFFFFFF))
            .corner_radius(9.0)
            .shadow(24.0)
            .padding_length(40.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 200.0, height: 160.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "shadow halo");
    }

    #[test]
    fn degenerate_thin_rects_survive() {
        if !device_present() {
            return;
        }
        // hairlines and borders thicker than the box: the clamps must
        // agree on both backends, no panic, no stray ink
        let root = vstack((
            empty().frame(100.0, 1.0).background_color(Color::hex(0x18181D)),
            empty().frame(1.0, 40.0).background_color(Color::hex(0x18181D)),
            empty().frame(60.0, 10.0).border(Color::hex(0x3B82F6), 20.0),
            empty().frame(40.0, 12.0).background_color(Color::hex(0xDDE1E9)).corner_radius(30.0),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 140.0, height: 120.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "degenerate rects");
    }

    #[test]
    fn the_skip_key_watches_list_size_scale_and_canvas() {
        // the whole quadruple guards the skip: a repeated frame skips,
        // and ANY leg changing — list, physical, scale or clear color —
        // must present again
        let runtime = Runtime::new();
        let logical = Size { width: 100.0, height: 60.0 };
        let quiet = runtime.display_frame(&text("still"), logical);
        let changed = runtime.display_frame(&text("moved"), logical);
        let retained = Some((Rc::new(quiet.clone()), (200usize, 120usize), 2usize, Color::CANVAS));
        assert!(frame_repeats(&retained, &quiet, (200, 120), 2, Color::CANVAS));
        assert!(!frame_repeats(&retained, &changed, (200, 120), 2, Color::CANVAS));
        assert!(!frame_repeats(&retained, &quiet, (210, 120), 2, Color::CANVAS));
        assert!(!frame_repeats(&retained, &quiet, (200, 120), 1, Color::CANVAS));
        assert!(!frame_repeats(&retained, &quiet, (200, 120), 2, Color::hex(0x18181D)));
        assert!(!frame_repeats(&None, &quiet, (200, 120), 2, Color::CANVAS));
    }

    #[test]
    fn text_runs_match_byte_for_byte_with_the_pixel_font() {
        if !device_present() {
            return;
        }
        // the pixel font has no anti-aliasing: alpha is 0 or 255, so the
        // sprite path must be EXACT — any drift is a texel-address bug
        let root = vstack((
            text("the quick brown bunny"),
            text("jumps over the lazy dog").foreground_color(Color::hex(0x3B82F6)),
        ))
        .padding_length(8.0)
        .background_color(Color::hex(0xFFFFFF));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 240.0, height: 80.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "pixel-font text diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn core_text_runs_match_within_tolerance() {
        if !device_present() {
            return;
        }
        // the real engine, SAME instance on both sides: identical run
        // rasters in, so only blend rounding may differ
        let engine = crate::CoreTextEngine::new();
        let logical = Size { width: 260.0, height: 100.0 };
        let scale = 2usize;
        let physical = (520, 200);
        let runtime = Runtime::new().text_engine(Rc::new(crate::CoreTextEngine::new()));
        let root = vstack((
            text("Fjord glyphs vex quick waltz"),
            text("bunny_ui presents by metal").foreground_color(Color::hex(0x3B82F6)),
        ))
        .padding_length(10.0)
        .background_color(Color::hex(0xFFFFFF))
        .corner_radius(9.0);
        let display = runtime.display_frame(&root, logical);
        let cpu = rasterize_with(&display, physical.0, physical.1, scale, Color::CANVAS, &engine, &RawImages::default())
            .to_rgba_bytes();
        let mut gpu = OffscreenGpu::new(physical.0, physical.1).expect("offscreen gpu");
        gpu.present_wait(&display, scale, Color::CANVAS, &engine, &RawImages::default());
        assert_close(&gpu.read_rgba(), &cpu, 2, "core-text runs");
    }

    #[test]
    fn wide_run_chunks_are_seamless() {
        if !device_present() {
            return;
        }
        // 80 chars × 8 px × scale 2 = 1280 device px — wider than one
        // chunk, so the run splits; texel copies are 1:1 and a seam
        // would be a byte difference, not a smudge
        let long = "abcdefghij".repeat(8);
        let root = text(long).padding_length(4.0).background_color(Color::hex(0xFFFFFF));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 700.0, height: 40.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "chunked run diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn a_warm_atlas_reuses_its_tiles() {
        if !device_present() {
            return;
        }
        let root = vstack((text("warm"), text("frame")));
        let logical = Size { width: 120.0, height: 60.0 };
        let runtime = Runtime::new();
        let display = runtime.display_frame(&root, logical);
        let mut gpu = OffscreenGpu::new(240, 120).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &RawImages::default());
        let first = gpu.atlas_footprint();
        assert!(first.0 > 0, "the frame rasterized runs into the atlas");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &RawImages::default());
        let second = gpu.atlas_footprint();
        assert_eq!(first, second, "an identical frame must not mint new tiles");
    }

    #[test]
    fn empty_clip_kills_the_quad() {
        if !device_present() {
            return;
        }
        // a zero-height frame degenerates the clip: nothing under it may
        // paint, on either backend
        let rows: Vec<usize> = vec![1, 2, 3];
        let root = list(rows, |row| row.to_string(), |_| {
            empty().frame(100.0, 20.0).background_color(Color::hex(0xFF0000))
        })
        .frame(120.0, 0.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 140.0, height: 60.0 }, 2, Color::CANVAS);
        assert!(gpu == cpu, "empty-clip scene diverged");
        for pixel in gpu.chunks_exact(4) {
            assert_eq!(pixel, [0xF2, 0xF3, 0xF7, 0xFF], "a clipped-out row leaked ink");
        }
    }

    // MARK: - Images

    /// A 32×32 deterministic gradient in the house raw format.
    fn gradient_source(key: u64) -> ImageSource {
        let mut rgba = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32u32 {
            for x in 0..32u32 {
                rgba.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, 128, 255]);
            }
        }
        ImageSource::bytes_keyed(key, RawImages::encode(32, 32, &rgba))
    }

    fn image_scene() -> impl View {
        let icon = gradient_source(1);
        // small rides the atlas; 300pt at scale 2 is 600×600 physical —
        // over the area threshold, a dedicated texture
        vstack((
            image(icon.clone()).resizable().frame(24.0, 24.0),
            image(gradient_source(2)).resizable().frame(300.0, 300.0),
            image(ImageSource::from_bytes(&b"junk"[..])).resizable().frame(40.0, 40.0),
            image(icon).resizable().aspect_ratio(ContentMode::Fill).frame(60.0, 20.0),
        ))
    }

    #[test]
    fn images_match_byte_for_byte() {
        if !device_present() {
            return;
        }
        // 1:1 texel blits on both sides, no anti-aliasing anywhere —
        // the engines hand both pipelines the SAME resampled bytes
        let (gpu, cpu) =
            scene_bytes(&image_scene(), Size { width: 320.0, height: 400.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "image scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn a_warm_image_frame_reuses_every_upload() {
        if !device_present() {
            return;
        }
        let runtime = Runtime::new();
        let root = image_scene();
        let display = runtime.display_frame(&root, Size { width: 320.0, height: 400.0 });
        let engine = RawImages::default();
        let mut gpu = OffscreenGpu::new(640, 800).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        let first = gpu.atlas_footprint();
        assert!(first.0 >= 3, "atlas icon + cover + dedicated photo: {first:?}");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(first, gpu.atlas_footprint(), "a warm frame re-uploads nothing");
    }

    /// Twelve photos of 40×130pt — 80×260 physical at scale 2, each
    /// taller than a shelf: twelve dedicated textures on ONE frame,
    /// past the retention cap.
    fn crowd_scene(first_key: u64) -> impl View {
        let photos: Vec<_> = (0..12)
            .map(|i| image(gradient_source(first_key + i)).resizable().frame(40.0, 130.0))
            .collect();
        hstack(photos)
    }

    #[test]
    fn a_frame_of_more_big_images_than_the_cap_keeps_every_one() {
        if !device_present() {
            return;
        }
        // the cap is a retention budget between frames, not a limit on
        // a frame: the ninth image asked for the collector, the reset
        // emptied the map, the walk ran into the same wall twice and
        // the frame went out without its last four images (the Atrium
        // floor at scale 2 — dozens of painted paths taller than a shelf)
        let (gpu, cpu) =
            scene_bytes(&crowd_scene(10), Size { width: 640.0, height: 140.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "crowd scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn the_collector_takes_only_textures_no_walk_reads() {
        if !device_present() {
            return;
        }
        let logical = Size { width: 640.0, height: 140.0 };
        let engine = RawImages::default();
        let mut gpu = OffscreenGpu::new(1280, 280).expect("offscreen gpu");
        let first = Runtime::new().display_frame(&crowd_scene(10), logical);
        gpu.present_wait(&first, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(gpu.atlas.dedicated.len(), 12, "every photo of the frame keeps its texture");
        gpu.present_wait(&first, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(gpu.atlas.dedicated.len(), 12, "a warm frame past the cap asks no collector");
        // twelve OTHER photos: the first twelve are stale, the cap asks
        // for the collector, and the map holds only what this frame reads
        let second = Runtime::new().display_frame(&crowd_scene(50), logical);
        gpu.present_wait(&second, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(gpu.atlas.dedicated.len(), 12, "the collector took the stale textures");
    }

    #[test]
    fn a_hundred_heights_ride_the_shelves() {
        if !device_present() {
            return;
        }
        // the Atrium floor at scale 2: hundreds of painted paths of nearly
        // as many heights, none tall enough for a texture of its own. One
        // shelf per exact height wanted more rows than the atlas has, and
        // the frame went out without its last fifty paths
        let paths: Vec<_> = (0..100u64)
            .map(|i| image(gradient_source(200 + i)).resizable().frame(40.0, 10.0 + i as f64))
            .collect();
        let root = zstack(paths);
        let logical = Size { width: 60.0, height: 120.0 };
        let (gpu, cpu) = scene_bytes(&root, logical, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "hundred-heights scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
        let display = Runtime::new().display_frame(&root, logical);
        let mut gpu = OffscreenGpu::new(120, 240).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &RawImages::default());
        assert_eq!(gpu.atlas.images.len(), 100, "every path rode the shelves");
        assert!(gpu.atlas.dedicated.is_empty(), "none needed a texture of its own");
    }

    #[test]
    fn a_full_shelf_on_a_fresh_walk_never_cuts_the_frame() {
        if !device_present() {
            return;
        }
        // seventy shelf-sized photos, more than the shelves hold: the
        // overflow takes textures of its own and the frame goes out whole
        let photos: Vec<_> = (0..70u64)
            .map(|i| image(gradient_source(300 + i)).resizable().frame(500.0, 125.0))
            .collect();
        let root = zstack(photos);
        let logical = Size { width: 500.0, height: 125.0 };
        let (gpu, cpu) = scene_bytes(&root, logical, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "full-shelf scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
        let display = Runtime::new().display_frame(&root, logical);
        let mut gpu = OffscreenGpu::new(1000, 250).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &RawImages::default());
        assert_eq!(gpu.atlas.images.len() + gpu.atlas.dedicated.len(), 70, "every photo is somewhere");
        assert!(!gpu.atlas.dedicated.is_empty(), "the overflow took textures of its own");
    }

    // MARK: - Feeds

    /// A `w`×`h` gradient as the next frame of `feed`, `phase` in blue.
    fn feed_frame(feed: &ImageFeed, (w, h): (u32, u32), phase: u8) {
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[
                    ((x * 255) / w.max(1)) as u8,
                    ((y * 255) / h.max(1)) as u8,
                    phase,
                    255,
                ]);
            }
        }
        feed.push((w, h), rgba);
    }

    /// Every channel within `max_delta` — the gate for a picture the two
    /// roads SCALE. Both filter bilinear in their own arithmetic, so
    /// nearly every pixel may sit a step apart and `assert_close`'s one
    /// percent rule is not the question here; byte equality at 1:1 is
    /// asserted on its own.
    fn assert_filtered_close(gpu: &[u8], cpu: &[u8], max_delta: u8, label: &str) {
        assert_eq!(gpu.len(), cpu.len(), "{label}: byte lengths differ");
        let worst = max_channel_delta(gpu, cpu);
        assert!(worst <= max_delta, "{label}: worst channel delta {worst} (allowed {max_delta})");
    }

    #[test]
    fn a_feed_at_one_to_one_matches_the_raster_byte_for_byte() {
        if !device_present() {
            return;
        }
        let feed = ImageFeed::new();
        feed_frame(&feed, (64, 48), 7);
        // 32×24 pt at scale 2 is 64×48 px: the picture's own size, and
        // a linear sampler on a texel centre reads the texel
        let root = image(&feed).resizable().frame(32.0, 24.0);
        let (gpu, cpu) = scene_bytes(&root, Size { width: 40.0, height: 30.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "a 1:1 feed diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn a_feed_scaled_up_and_down_matches_within_tolerance() {
        if !device_present() {
            return;
        }
        let feed = ImageFeed::new();
        feed_frame(&feed, (32, 24), 7);
        // 4× up, covering a box of another shape (the cover's own clip
        // cuts it), and ½× down beside it
        let root = hstack((
            image(&feed).resizable().aspect_ratio(ContentMode::Fill).frame(64.0, 30.0),
            image(&feed).resizable().frame(8.0, 6.0),
        ));
        let (gpu, cpu) = scene_bytes(&root, Size { width: 100.0, height: 60.0 }, 2, Color::CANVAS);
        assert_filtered_close(&gpu, &cpu, 3, "scaled feed");
    }

    #[test]
    fn a_feed_keeps_one_texture_across_many_generations() {
        if !device_present() {
            return;
        }
        let feed = ImageFeed::new();
        let engine = RawImages::default();
        let mut gpu = OffscreenGpu::new(640, 480).expect("offscreen gpu");
        for generation in 0..100u8 {
            feed_frame(&feed, (64, 48), generation);
            let runtime = Runtime::new();
            let display = runtime.display_frame(
                &image(&feed).resizable().frame(300.0, 200.0),
                Size { width: 320.0, height: 240.0 },
            );
            gpu.present_nowait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        }
        gpu.drain();
        assert_eq!(gpu.atlas.live.len(), 1, "one live texture for one feed");
        assert!(gpu.atlas.dedicated.is_empty(), "a feed never mints a dedicated texture");
        assert_eq!(gpu.ground.textures.len(), 1, "and the ground holds exactly it");
        assert_eq!(gpu.atlas.reset_walk, 0, "no walk ever reset the atlas");
    }

    #[test]
    fn a_feed_beside_a_crowd_never_drains() {
        if !device_present() {
            return;
        }
        // twelve photos over the dedicated cap, warm after the first
        // frame, and a feed changing under them: the collector is asked
        // only when something new wants a dedicated texture — a feed is
        // not that, however many frames it brings
        let feed = ImageFeed::new();
        let engine = RawImages::default();
        let mut gpu = OffscreenGpu::new(1200, 600).expect("offscreen gpu");
        for generation in 0..20u8 {
            feed_frame(&feed, (64, 48), generation);
            let runtime = Runtime::new();
            let display = runtime.display_frame(
                &vstack((crowd_scene(10), image(&feed).resizable().frame(200.0, 150.0))),
                Size { width: 600.0, height: 300.0 },
            );
            gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        }
        assert_eq!(gpu.atlas.dedicated.len(), 12, "the crowd keeps its textures");
        assert_eq!(gpu.atlas.live.len(), 1);
        assert_eq!(gpu.atlas.reset_walk, 0, "twenty frames of feed never asked the collector");
    }

    #[test]
    fn an_idle_feed_retires() {
        if !device_present() {
            return;
        }
        let feed = ImageFeed::new();
        feed_frame(&feed, (16, 16), 1);
        let engine = RawImages::default();
        let mut gpu = OffscreenGpu::new(200, 200).expect("offscreen gpu");
        let shown = Runtime::new().display_frame(
            &image(&feed).resizable().frame(50.0, 50.0),
            Size { width: 100.0, height: 100.0 },
        );
        gpu.present_wait(&shown, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(gpu.atlas.live.len(), 1);
        let empty = Runtime::new()
            .display_frame(&spacer().frame(100.0, 100.0), Size { width: 100.0, height: 100.0 });
        for _ in 0..=bunny_ui::gpu::walk::LIVE_IDLE_WALKS {
            gpu.present_wait(&empty, 2, Color::CANVAS, &PixelFont, &engine);
        }
        assert!(gpu.atlas.live.is_empty(), "the texture was given back");
        assert!(gpu.ground.textures.is_empty(), "and the ground released it");
    }

    // MARK: - Icons

    const MARK_PATH: &[bunny_ui::icon::Verb] = &[
        bunny_ui::icon::Verb::Move(4.0, 12.0),
        bunny_ui::icon::Verb::Line(10.0, 18.0),
        bunny_ui::icon::Verb::Line(20.0, 6.0),
    ];
    const DISC_PATH: &[bunny_ui::icon::Verb] = &[
        bunny_ui::icon::Verb::Move(12.0, 2.0),
        bunny_ui::icon::Verb::Cubic(17.5, 2.0, 22.0, 6.5, 22.0, 12.0),
        bunny_ui::icon::Verb::Cubic(22.0, 17.5, 17.5, 22.0, 12.0, 22.0),
        bunny_ui::icon::Verb::Cubic(6.5, 22.0, 2.0, 17.5, 2.0, 12.0),
        bunny_ui::icon::Verb::Cubic(2.0, 6.5, 6.5, 2.0, 12.0, 2.0),
        bunny_ui::icon::Verb::Close,
    ];
    const MARK_GLYPH: bunny_ui::icon::Glyph = bunny_ui::icon::Glyph {
        draws: &[bunny_ui::icon::Draw {
            paint: bunny_ui::icon::Paint::Stroke { width: 2.0 },
            path: MARK_PATH, tint: None,
        }],
    };
    const DISC_GLYPH: bunny_ui::icon::Glyph = bunny_ui::icon::Glyph {
        draws: &[bunny_ui::icon::Draw {
            paint: bunny_ui::icon::Paint::Fill(bunny_ui::icon::Rule::NonZero),
            path: DISC_PATH, tint: None,
        }],
    };
    const MARK: bunny_ui::icon::Symbol = bunny_ui::icon::Symbol::new("test.mark", &MARK_GLYPH);
    const DISC: bunny_ui::icon::Symbol = bunny_ui::icon::Symbol::new("test.disc", &DISC_GLYPH);

    fn icon_scene() -> impl View {
        // two glyphs, three tints, two sizes — natural beside text,
        // exact through the frame idiom
        vstack((
            icon(MARK),
            icon(MARK).foreground_color(Color::hex(0x3366AA)),
            icon(DISC).foreground_color(Color::hex(0xAA3322)),
            icon(DISC).resizable().frame(32.0, 32.0),
            text("beside").foreground_color(Color::hex(0x222222)),
        ))
        .spacing(4.0)
    }

    /// A box that draws ONE ramped path — the escape hatch's road to
    /// the sprite atlas, with the ink read per pixel instead of once.
    struct RampedMark;

    impl bunny_ui::prelude::CustomElement for RampedMark {
        fn paint(
            &self,
            ctx: &bunny_ui::prelude::PaintCtx,
            painter: &mut bunny_ui::prelude::Painter,
        ) {
            use bunny_ui::icon::{Paint, Rule, Verb};
            let (w, h) = (ctx.size().width as f32, ctx.size().height as f32);
            let verbs = [
                Verb::Move(2.0, 2.0),
                Verb::Line(w - 2.0, 2.0),
                Verb::Line(w - 2.0, h - 2.0),
                Verb::Line(2.0, h - 2.0),
                Verb::Close,
            ];
            painter.path(
                &verbs,
                Paint::Fill(Rule::NonZero),
                bunny_ui::layout::Gradient::linear(
                    Color::hex(0xDD2233),
                    Color::hex(0x2233DD),
                )
                .direction(
                    bunny_ui::layout::UnitPoint::TOP_LEADING,
                    bunny_ui::layout::UnitPoint::BOTTOM_TRAILING,
                ),
            );
        }
    }

    #[test]
    fn a_ramped_path_matches_the_cpu_byte_for_byte() {
        if !device_present() {
            return;
        }
        // the ramp is resolved and sampled ONCE, by the house, into the
        // tile both pipelines then blit — so a gradient inside a traced
        // path needs no shader on either side, and parity stays exact
        let root = bunny_ui::prelude::custom(RampedMark).frame(80.0, 48.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 100.0, height: 60.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "ramped path diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn icons_match_the_cpu_byte_for_byte() {
        if !device_present() {
            return;
        }
        // the FIRST primitive with exact parity: the house rasterizes
        // the glyph once and both pipelines blit those same bytes — a
        // 1:1 texel read on the GPU, a straight blit on the CPU. Not
        // assert_close: assert_eq.
        let (gpu, cpu) =
            scene_bytes(&icon_scene(), Size { width: 120.0, height: 160.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "icon scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn a_warm_icon_frame_reuses_every_upload() {
        if !device_present() {
            return;
        }
        let runtime = Runtime::new();
        let root = icon_scene();
        let display = runtime.display_frame(&root, Size { width: 120.0, height: 160.0 });
        let engine = RawImages::default();
        let mut gpu = OffscreenGpu::new(240, 320).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        let first = gpu.atlas_footprint();
        assert!(first.0 >= 4, "four tinted tiles at least: {first:?}");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(first, gpu.atlas_footprint(), "the tinted keys cache, never thrash");
    }

    #[test]
    fn an_ultra_wide_image_never_touches_the_shelf() {
        if !device_present() {
            return;
        }
        // 2100pt at scale 2 = 4200 physical — wider than the atlas can
        // ever grow; the dedicated path must carry it without a single
        // reset-retry (the old livelock shape)
        let root = image(gradient_source(3)).resizable().frame(2100.0, 60.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 400.0, height: 100.0 }, 2, Color::CANVAS);
        assert!(gpu == cpu, "ultra-wide image diverged");
        assert!(
            gpu.chunks_exact(4).any(|pixel| pixel[..3] != [0xF2, 0xF3, 0xF7]),
            "the image painted through the dedicated texture"
        );
    }

    // MARK: - Liquid glass

    fn glass_scene(glass: bunny_ui::layout::Glass, radius: f64) -> impl View {
        // stripes make the blur legible and the lens unmistakable — a
        // pane over a flat colour proves nothing
        let bars = for_each(
            (0..20).collect::<Vec<i32>>(),
            |index: &i32| index.to_string(),
            |index| {
                empty()
                    .frame_width(240.0)
                    .frame_height(8.0)
                    .background_color(if index % 2 == 0 {
                        Color::hex(0x102A64)
                    } else {
                        Color::hex(0xE8D14A)
                    })
            },
        )
        .vertical();
        zstack!((
            bars,
            empty().frame(150.0, 90.0).corner_radius(radius).glass(glass),
        ))
    }

    /// The parity gate for glass: the material is a blur, a bilinear
    /// sample and a saturate, resolved in f64 on one side and f32 on the
    /// other, so it answers CLOSE, never equal. What it must not do is
    /// answer differently in SHAPE — the tolerance is per channel and
    /// the share of channels beyond one step is what would catch a lens
    /// that bends the wrong way.
    fn assert_glass_close(gpu: &[u8], cpu: &[u8], max_delta: u8, share_beyond: f64, label: &str) {
        assert_eq!(gpu.len(), cpu.len(), "{label}: byte lengths differ");
        let mut worst = 0u8;
        let mut beyond = 0usize;
        for (a, b) in gpu.iter().zip(cpu.iter()) {
            let delta = a.abs_diff(*b);
            worst = worst.max(delta);
            if delta > 1 {
                beyond += 1;
            }
        }
        let share = beyond as f64 / gpu.len() as f64;
        assert!(
            worst <= max_delta,
            "{label}: worst channel delta {worst} (allowed {max_delta}), {:.3}% beyond one",
            share * 100.0
        );
        assert!(
            share <= share_beyond,
            "{label}: {:.3}% of channels beyond one step (allowed {:.3}%)",
            share * 100.0,
            share_beyond * 100.0
        );
    }

    #[test]
    fn the_material_matches_the_raster() {
        if !device_present() {
            return;
        }
        use bunny_ui::layout::Glass;
        for (label, glass, radius) in [
            ("regular", Glass::regular(), 24.0),
            // the pure lens: the pyramid's own floor for a blur and a
            // violent bend — this is the one that catches a rim that
            // pinches instead of magnifying
            (
                "lens",
                Glass::regular().blur(0.0).refraction(20.0, 32.0).tint(Color::rgba(255, 255, 255, 12)),
                30.0,
            ),
            // the fringe: three samples per pixel instead of one
            ("fringe", Glass::regular().chromatic(0.35), 18.0),
            // a flat pane, and the deepest level of the pyramid
            ("frosted", Glass::frosted(), 12.0),
            // the rim alone, on a square pane: no corner to hide behind
            (
                "rim",
                Glass::regular().refraction(0.0, 0.0).highlight(Color::WHITE, 5.0, 1.0),
                0.0,
            ),
            // the touch lights
            (
                "touch",
                Glass::regular().sheen(0.1).spot(bunny_ui::layout::UnitPoint::CENTER, 0.6, 0.4),
                20.0,
            ),
        ] {
            let root = glass_scene(glass, radius);
            let (gpu, cpu) =
                scene_bytes(&root, Size { width: 240.0, height: 160.0 }, 2, Color::CANVAS);
            // measured: the flat materials answer within TWO and the
            // bending ones within three — a lens multiplies the f32/f64
            // gap by how steep the scene is where it samples
            assert_glass_close(&gpu, &cpu, 3, 0.005, label);
        }
    }

    #[test]
    fn stacked_panes_each_read_the_one_below() {
        if !device_present() {
            return;
        }
        // two panes that OVERLAP must not share one capture of the
        // scene: the upper one would sample a blur taken before the
        // lower one existed, and the glass under it would vanish
        use bunny_ui::layout::Glass;
        let root = zstack!((
            empty()
                .frame_width(240.0)
                .frame_height(160.0)
                .background_gradient(bunny_ui::layout::Gradient::linear(
                    Color::hex(0x102A64),
                    Color::hex(0xE8D14A),
                )),
            empty().frame(180.0, 110.0).corner_radius(28.0).glass(Glass::regular()),
            empty().frame(90.0, 60.0).corner_radius(18.0).glass(Glass::clear()),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 240.0, height: 160.0 }, 2, Color::CANVAS);
        // a pane over a pane compounds: the upper one samples a scene
        // that already carries the lower one's own difference
        assert_glass_close(&gpu, &cpu, 6, 0.015, "stacked panes");
    }

    #[test]
    fn a_resting_atlas_is_offered_back_and_a_purged_one_says_so() {
        if !device_present() {
            return;
        }
        unsafe {
            let device = MTLCreateSystemDefaultDevice();
            let mut ground = MetalGround::new(device);
            // no atlas yet: resting offers nothing, waking finds nothing lost
            ground.rest();
            assert!(!ground.volatile, "no texture, nothing to offer");
            assert!(ground.wake());
            assert!(ground.ensure_shared(256));
            // at rest the texture is volatile; asked back untouched, it kept its tiles
            ground.rest();
            assert!(ground.volatile);
            assert!(ground.wake(), "nothing took the atlas: its tiles stand");
            assert!(!ground.volatile);
            // the system takes it (Empty is what a purge leaves): the wake says so
            ground.rest();
            msg_u64_u64(ground.shared, sel("setPurgeableState:"), PURGEABLE_EMPTY);
            assert!(!ground.wake(), "a purged atlas is reported lost");
            // a dropped texture leaves no flag behind
            ground.rest();
            ground.drop_shared();
            assert!(!ground.volatile);
            drop(ground);
            msg_void(device, sel("release"));
        }
    }
}
