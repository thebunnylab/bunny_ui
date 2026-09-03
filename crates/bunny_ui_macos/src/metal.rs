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
//! hand-written `objc_msgSend` border as AppKit, and the shaders are a
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::hash::{Hash, Hasher};
use std::ptr::null_mut;

use bunny_ui::image_engine::{ImageEngine, ImageSource, raster_source};
use bunny_ui::layout::{Color, Corners, DisplayList, DrawCommand, Rect, Size};
use bunny_ui::raster::physical_extent;
use bunny_ui::text_engine::{FontKey, FontSpec, TextEngine};

use crate::ffi::{CGSize, Id, Sel, class, sel};

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
    fn msg_bool(obj: Id, sel: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_arg(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64_u64(obj: Id, sel: Sel, a: u64, b: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_cstr(obj: Id, sel: Sel) -> *const c_char;
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

// The run atlas: text tiles append into one shared texture. Runs wider
// than a chunk split into seamless chunks (texel reads are 1:1, a seam
// cannot show). Overflow drains the in-flight frames, resets the whole
// atlas and re-inserts the current frame — a copying collector, not a
// per-tile free list.
const ATLAS_CHUNK_WIDTH: u32 = 1024;
const ATLAS_INITIAL_SIZE: u32 = 2048;
const ATLAS_MAX_SIZE: u32 = 4096;

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

/// One rect primitive: fill, stroke ring or shadow, selected by
/// `params[2]`. Everything is snapped device pixels resolved on the CPU
/// in f64 — the shader is a pure coverage evaluator.
///
/// The struct crosses to the GPU as raw bytes; the MSL source declares
/// the same layout textually and the asserts below are the ONLY defense
/// against drift.
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)] // written whole, read by the GPU — never field by field
struct RectInstance {
    rect: [f32; 4],   // x0, y0, x1, y1 (the shadow ships its EXPANDED box)
    clip: [f32; 4],   // the snapped clip-stack top
    params: [f32; 4], // aspect (the ellipse only), thickness/reach/first, kind, expansion/second
    color: [u8; 4],   // straight RGBA
    // A gradient's second half rides here: the far color plus one
    // point (centre for the rings, end for the line). The ramp fits
    // the twelve bytes that were padding.
    pad: [u8; 12],
    // The four corners, clockwise from the top left, CLAMPED in device
    // px — the shader only picks the one its quadrant owns.
    radii: [f32; 4],
}

/// One text run (or chunk of one): a rectangle of atlas texels copied
/// 1:1 to the destination — no sampler, no resampling, exact bytes.
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)] // written whole, read by the GPU — never field by field
struct SpriteInstance {
    dest: [f32; 4], // x0, y0, x1, y1 in device px
    tex: [f32; 4],  // atlas texel origin + the same extent
    clip: [f32; 4],
}

/// One pane of liquid glass. Everything is snapped device pixels
/// resolved on the CPU in f64, like every other instance here — the
/// shader only evaluates the material.
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)] // written whole, read by the GPU — never field by field
struct GlassInstance {
    rect: [f32; 4],   // x0, y0, x1, y1
    clip: [f32; 4],   // the snapped clip-stack top
    radii: [f32; 4],  // the four corners, clamped
    lens: [f32; 4],   // blur, refraction band, refraction amount, chromatic
    finish: [f32; 4], // highlight band, highlight intensity, saturation, brightness
    touch: [f32; 4],  // sheen, spot x, spot y, spot radius
    tint: [u8; 4],    // straight RGBA
    highlight: [u8; 4],
    spot_alpha: f32,
    pad: f32,
}

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
    out.position = to_ndc(mix(low, high, corner), uniforms.viewport);
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
                              constant ClipRound& round [[buffer(1)]]) {
    RectInstance rect = rects[in.id];
    float2 p = in.position.xy;
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
    out.position = to_ndc(mix(low, high, corner), uniforms.viewport);
    out.id = iid;
    return out;
}

fragment float4 sprite_fragment(SpriteVary in [[stage_in]],
                                device const SpriteInstance* sprites [[buffer(0)]],
                                constant ClipRound& round [[buffer(1)]],
                                texture2d<float, access::read> atlas [[texture(0)]]) {
    SpriteInstance sprite = sprites[in.id];
    float2 texel = sprite.tex.xy + (floor(in.position.xy) - floor(sprite.dest.xy));
    // straight alpha in, straight alpha out — only the coverage moves,
    // and text under a rounded corner loses its square edge at last
    float4 ink = atlas.read(uint2(texel));
    return float4(ink.rgb, ink.a * clip_cov(in.position.xy, round));
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
    out.position = to_ndc(mix(low, high, corner), uniforms.viewport);
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
    float2 point = in.position.xy;

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
    in_live_resize: Sel,
    set_presents_with_transaction: Sel,
    wait_scheduled: Sel,
    present: Sel,
    status: Sel,
    retain: Sel,
    release: Sel,
    contents: Sel,
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
                in_live_resize: sel("inLiveResize"),
                set_presents_with_transaction: sel("setPresentsWithTransaction:"),
                wait_scheduled: sel("waitUntilScheduled"),
                present: sel("present"),
                status: sel("status"),
                retain: sel("retain"),
                release: sel("release"),
                contents: sel("contents"),
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

unsafe fn ns_string(text: &str) -> Id {
    let text = CString::new(text).expect("string without NUL");
    unsafe {
        msg_id_cstr(
            class("NSString"),
            sel("stringWithUTF8String:"),
            text.as_ptr(),
        )
    }
}

unsafe fn error_message(error: Id) -> String {
    unsafe {
        if error.is_null() {
            return "unknown error".to_string();
        }
        let description = msg_id(error, sel("localizedDescription"));
        if description.is_null() {
            return "unknown error".to_string();
        }
        let chars = msg_cstr(description, sel("UTF8String"));
        if chars.is_null() {
            return "unknown error".to_string();
        }
        CStr::from_ptr(chars).to_string_lossy().into_owned()
    }
}

unsafe fn default_device() -> Option<Id> {
    let device = unsafe { MTLCreateSystemDefaultDevice() };
    (!device.is_null()).then_some(device)
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
                let size = [frame.viewport.0, frame.viewport.1];
                // argument bindings persist across pipeline swaps — the
                // uniforms bind once
                msg_void_ptr_u64_u64(
                    encoder,
                    self.sels.set_vertex_bytes,
                    size.as_ptr() as *const c_void,
                    8,
                    1,
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
                            RunKind::Sprites | RunKind::Texture(_) => {
                                msg_void_id(encoder, self.sels.set_pipeline, self.sprite_pipeline);
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
                                // dedicated texture — same pipeline
                                let texture = match run.kind {
                                    RunKind::Texture(index) => frame.textures[index as usize],
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
            let size = [frame.viewport.0, frame.viewport.1];
            msg_void_ptr_u64_u64(
                encoder,
                self.sels.set_vertex_bytes,
                size.as_ptr() as *const c_void,
                8,
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
                8,
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
    viewport: (f32, f32),
    instances: Id,
    sprite_offset: usize,
    glass_offset: usize,
    runs: &'a [DrawRun],
    rounds: &'a [RoundClip],
    atlas_texture: Id,
    textures: &'a [Id],
    pyramid: Option<&'a GlassTextures>,
}

/// The deepest level of the blur pyramid — four levels in all, mirroring
/// `bunny_ui::glass::MAX_LEVEL`.
const GLASS_MAX_LEVEL: u32 = 3;

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

// MARK: - The walk (display list → instances, all policy in f64)

/// A snapped box in device pixels, `[x0, y0, x1, y1)` — the same tuple
/// the Surface uses for damage and clips.
type Box4 = (i64, i64, i64, i64);

fn box_intersect(a: Box4, b: Box4) -> Option<Box4> {
    let rect = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (rect.0 < rect.2 && rect.1 < rect.3).then_some(rect)
}

/// The mirror of `snap(scale_rect(rect, factor))` — scale origin and
/// size separately, then round each edge on its own. The operation order
/// matters: it is what makes neighbors close without a seam, and parity
/// is byte-level.
fn snap_scaled(rect: Rect, factor: f64) -> Box4 {
    let sx = rect.origin.x * factor;
    let sy = rect.origin.y * factor;
    let sw = rect.size.width * factor;
    let sh = rect.size.height * factor;
    (
        sx.round() as i64,
        sy.round() as i64,
        (sx + sw).round() as i64,
        (sy + sh).round() as i64,
    )
}

/// The CPU's radius clamp, verbatim — the same `Corners::clamped` the
/// raster runs, against the SNAPPED extent.
fn corner_clamp(scaled: Corners, snapped: Box4) -> Corners {
    scaled.clamped((snapped.2 - snapped.0) as f64, (snapped.3 - snapped.1) as f64)
}

/// The curve a run is cut by, as the shaders see it — ONE per draw
/// run, bound as 32 bytes of fragment constants, never per instance:
/// the 64-byte rect wire and the 48-byte sprite wire stay untouched.
/// `radius == 0` is the straight rectangle every clip has been until
/// now — and multiplying coverage by 1.0 is exact, so a frame without
/// a curve leaves both shaders bit for bit as they were.
#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
struct RoundClip {
    /// The rounded clip's OWN snapped box in device px — the cut can
    /// be smaller without the corner moving.
    box4: [f32; 4],
    /// The four corners. They fit the three floats MSL was already
    /// padding this struct out to, so the cut carries four for the
    /// price of one.
    radii: [f32; 4],
}

const _: () = {
    assert!(std::mem::size_of::<RoundClip>() == 32);
    assert!(std::mem::offset_of!(RoundClip, box4) == 0);
    assert!(std::mem::offset_of!(RoundClip, radii) == 16);
};

/// Slot zero of every frame — the cut that never bends.
const NO_ROUND: RoundClip = RoundClip { box4: [0.0; 4], radii: [0.0; 4] };

const KIND_FILL: f32 = 0.0;
const KIND_STROKE: f32 = 1.0;
const KIND_SHADOW: f32 = 2.0;
const KIND_RADIAL: f32 = 3.0;
const KIND_LINEAR: f32 = 4.0;
/// The elliptical rings: the ASPECT rides params.x (the corner slot —
/// an elliptical ramp ignores the box corner; a rounded wash clips
/// through `.clipped()`), start and end radii stay in params.y/.w.
const KIND_ELLIPTIC: f32 = 5.0;

// MARK: - The run atlas (text tiles, append-only shelves)

/// One rectangle of atlas texels.
#[derive(Clone, Copy)]
struct Tile {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

struct Shelf {
    y: u32,
    height: u32,
    cursor: u32,
}

/// Append-only shelf packing: a run lands on the first shelf of exactly
/// its height with room, or opens a new shelf below. There is no
/// per-tile free list — reclamation is the atlas RESET (drain, clear,
/// re-insert the live frame), a copying collector in one move.
struct ShelfPacker {
    width: u32,
    height: u32,
    shelves: Vec<Shelf>,
    next_y: u32,
}

impl ShelfPacker {
    fn new(width: u32, height: u32) -> ShelfPacker {
        ShelfPacker { width, height, shelves: Vec::new(), next_y: 0 }
    }

    fn place(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if width > self.width || height == 0 || width == 0 {
            return None;
        }
        for shelf in &mut self.shelves {
            if shelf.height == height && shelf.cursor + width <= self.width {
                let x = shelf.cursor;
                shelf.cursor += width;
                return Some((x, shelf.y));
            }
        }
        if self.next_y + height <= self.height {
            let y = self.next_y;
            self.next_y += height;
            self.shelves.push(Shelf { y, height, cursor: width });
            return Some((0, y));
        }
        None
    }

    fn reset(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }
}

/// The atlas is full — the caller drains the in-flight frames, resets
/// (growing once to the cap) and walks the frame again.
struct AtlasFull;

/// One cached run: the engine's raster uploaded as chunk tiles. The
/// color sits IN the key — the engine bakes it, which keeps emoji true
/// and byte parity possible; a theme flip mints new tiles and the old
/// ones fall with the next reset.
struct RunEntry {
    font: FontKey,
    color: u32,
    scale: u32,
    content: String,
    tiles: Vec<Tile>,
    width: u32,
    height: u32,
}

fn packed_color(color: Color) -> u32 {
    ((color.r as u32) << 24) | ((color.g as u32) << 16) | ((color.b as u32) << 8) | color.a as u32
}

/// The lookup hash — computed WITHOUT allocating (typing must never pay
/// a String per warm frame); collisions resolve by comparing the entry.
fn run_hash(font: FontKey, color: u32, scale: u32, content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    font.hash(&mut hasher);
    color.hash(&mut hasher);
    scale.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

/// The text side of the GPU frame: one shared RGBA texture of run
/// tiles, keyed by (font, color, scale, content).
///
/// The append-only INVARIANT: tiles are only ever written into virgin
/// space, so a frame still riding the GPU never sees its texels change.
/// The only operation that reuses space is `reset`, and reset requires
/// the caller to DRAIN in-flight frames first.
struct RunAtlas {
    device: Id,
    texture: Id,
    size: u32,
    packer: ShelfPacker,
    entries: HashMap<u64, Vec<RunEntry>>,
    /// Resampled images riding the SHARED texture, keyed by
    /// (source key, physical width, physical height) — icons and
    /// thumbnails, the many and the hot.
    images: HashMap<(u64, u32, u32), ImageEntry>,
    /// Images too big for a shelf get a texture of their own (a shelf
    /// eats its full height across the atlas width, and anything larger
    /// than the atlas would LIVELOCK the reset-retry). Capped; overflow
    /// rides the same reset the atlas already does.
    dedicated: HashMap<(u64, u32, u32), Id>,
}

/// One cached image on the shared atlas: its chunk tiles at one
/// physical size.
struct ImageEntry {
    tiles: Vec<Tile>,
}

/// What `resolve_image` hands the frame walk: shared tiles, or one
/// whole dedicated texture.
enum ResolvedImage<'a> {
    Tiles(&'a ImageEntry),
    Dedicated(Id, u32, u32),
}

/// The shelf ceiling: taller goes dedicated (uniform shelf heights
/// pack well; one tall image would burn a whole shelf band)…
const DEDICATED_HEIGHT: u32 = 256;
/// …and so does anything larger than this area, atlas-budget-wise.
const DEDICATED_AREA: u32 = 512 * 512;
/// Dedicated textures retained before the reset collects them.
const DEDICATED_KEEP: usize = 8;

impl RunAtlas {
    fn new(device: Id) -> RunAtlas {
        RunAtlas {
            device,
            texture: null_mut(),
            size: ATLAS_INITIAL_SIZE,
            packer: ShelfPacker::new(ATLAS_INITIAL_SIZE, ATLAS_INITIAL_SIZE),
            entries: HashMap::new(),
            images: HashMap::new(),
            dedicated: HashMap::new(),
        }
    }

    /// Drops every entry and every shelf. `grow` doubles the texture
    /// once (2048 → 4096); the texture itself is re-made lazily. The
    /// caller MUST have drained in-flight frames — this is the one
    /// moment texel space is reused.
    fn reset(&mut self, grow: bool) {
        if grow && self.size < ATLAS_MAX_SIZE {
            self.size = ATLAS_MAX_SIZE;
            unsafe {
                if !self.texture.is_null() {
                    msg_void(self.texture, sel("release"));
                    self.texture = null_mut();
                }
            }
            self.packer = ShelfPacker::new(self.size, self.size);
        } else {
            self.packer.reset();
        }
        self.entries.clear();
        self.images.clear();
        // the dedicated textures ride the same collector: the caller
        // drained the GPU before any reset, so releasing here is safe
        for (_, texture) in self.dedicated.drain() {
            unsafe { msg_void(texture, sel("release")) };
        }
    }

    unsafe fn ensure_texture(&mut self) -> bool {
        unsafe {
            if !self.texture.is_null() {
                return true;
            }
            let descriptor = msg_id_u64_u64_u64_bool(
                class("MTLTextureDescriptor"),
                sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
                PIXEL_FORMAT_RGBA8,
                self.size as u64,
                self.size as u64,
                0,
            );
            msg_void_u64(descriptor, sel("setUsage:"), TEXTURE_USAGE_SHADER_READ);
            msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_SHARED);
            self.texture = msg_id_arg(self.device, sel("newTextureWithDescriptor:"), descriptor);
            !self.texture.is_null()
        }
    }

    /// The tiles for one run — warm from the map, or rasterized by the
    /// engine, chunked and uploaded. `Ok(None)` means the engine had
    /// nothing to paint (the CPU path skips those too).
    fn resolve(
        &mut self,
        slice: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
        engine: &dyn TextEngine,
    ) -> Result<Option<&RunEntry>, AtlasFull> {
        let key = font.key();
        let packed = packed_color(color);
        let hash = run_hash(key, packed, scale as u32, slice);
        let warm = self.entries.get(&hash).is_some_and(|bucket| {
            bucket.iter().any(|entry| {
                entry.font == key
                    && entry.color == packed
                    && entry.scale == scale as u32
                    && entry.content == slice
            })
        });
        if !warm {
            let Some(raster) = engine.raster_line(slice, font, color, scale) else {
                return Ok(None);
            };
            unsafe {
                if !self.ensure_texture() {
                    return Err(AtlasFull);
                }
            }
            let width = raster.width as u32;
            let height = raster.height as u32;
            let mut tiles = Vec::new();
            let mut chunk_x: u32 = 0;
            while chunk_x < width {
                let chunk_width = (width - chunk_x).min(ATLAS_CHUNK_WIDTH);
                let Some((x, y)) = self.packer.place(chunk_width, height) else {
                    return Err(AtlasFull);
                };
                unsafe {
                    msg_void_region_u64_ptr_u64(
                        self.texture,
                        sel("replaceRegion:mipmapLevel:withBytes:bytesPerRow:"),
                        MTLRegion {
                            origin: MTLOrigin { x: x as u64, y: y as u64, z: 0 },
                            size: MTLSize {
                                width: chunk_width as u64,
                                height: height as u64,
                                depth: 1,
                            },
                        },
                        0,
                        raster.rgba.as_ptr().add(chunk_x as usize * 4) as *const c_void,
                        (raster.width * 4) as u64,
                    );
                }
                tiles.push(Tile { x, y, width: chunk_width, height });
                chunk_x += chunk_width;
            }
            self.entries.entry(hash).or_default().push(RunEntry {
                font: key,
                color: packed,
                scale: scale as u32,
                content: slice.to_string(),
                tiles,
                width,
                height,
            });
        }
        let entry = self
            .entries
            .get(&hash)
            .and_then(|bucket| {
                bucket.iter().find(|entry| {
                    entry.font == key
                        && entry.color == packed
                        && entry.scale == scale as u32
                        && entry.content == slice
                })
            })
            .expect("a run just resolved lives in the atlas");
        Ok(Some(entry))
    }

    /// The texels for one image at one physical size — warm from a map,
    /// or resampled by the engine and uploaded: small rides the shared
    /// atlas in chunk tiles, big claims a dedicated texture. `Ok(None)`
    /// = the engine has nothing yet (async decode, broken bytes).
    fn resolve_image(
        &mut self,
        source: &ImageSource,
        width: u32,
        height: u32,
        engine: &dyn ImageEngine,
    ) -> Result<Option<ResolvedImage<'_>>, AtlasFull> {
        let cache_key = (source.key(), width, height);
        if let Some(texture) = self.dedicated.get(&cache_key) {
            return Ok(Some(ResolvedImage::Dedicated(*texture, width, height)));
        }
        let shared = height <= DEDICATED_HEIGHT && width * height <= DEDICATED_AREA;
        if shared && !self.images.contains_key(&cache_key) {
            let Some(raster) = raster_source(engine, source, width as usize, height as usize) else {
                return Ok(None);
            };
            unsafe {
                if !self.ensure_texture() {
                    return Err(AtlasFull);
                }
            }
            let mut tiles = Vec::new();
            let mut chunk_x: u32 = 0;
            while chunk_x < width {
                let chunk_width = (width - chunk_x).min(ATLAS_CHUNK_WIDTH);
                let Some((x, y)) = self.packer.place(chunk_width, height) else {
                    return Err(AtlasFull);
                };
                unsafe {
                    msg_void_region_u64_ptr_u64(
                        self.texture,
                        sel("replaceRegion:mipmapLevel:withBytes:bytesPerRow:"),
                        MTLRegion {
                            origin: MTLOrigin { x: x as u64, y: y as u64, z: 0 },
                            size: MTLSize {
                                width: chunk_width as u64,
                                height: height as u64,
                                depth: 1,
                            },
                        },
                        0,
                        raster.rgba.as_ptr().add(chunk_x as usize * 4) as *const c_void,
                        (raster.width * 4) as u64,
                    );
                }
                tiles.push(Tile { x, y, width: chunk_width, height });
                chunk_x += chunk_width;
            }
            self.images.insert(cache_key, ImageEntry { tiles });
        }
        if shared {
            return Ok(self
                .images
                .get(&cache_key)
                .map(ResolvedImage::Tiles));
        }

        // dedicated: over the cap, the frame asks for the collector —
        // after the drain+reset the map is empty and the walk re-runs
        if self.dedicated.len() >= DEDICATED_KEEP {
            return Err(AtlasFull);
        }
        let Some(raster) = raster_source(engine, source, width as usize, height as usize) else {
            return Ok(None);
        };
        let texture = unsafe {
            let descriptor = msg_id_u64_u64_u64_bool(
                class("MTLTextureDescriptor"),
                sel("texture2DDescriptorWithPixelFormat:width:height:mipmapped:"),
                PIXEL_FORMAT_RGBA8,
                width as u64,
                height as u64,
                0,
            );
            msg_void_u64(descriptor, sel("setUsage:"), TEXTURE_USAGE_SHADER_READ);
            msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_SHARED);
            let texture = msg_id_arg(self.device, sel("newTextureWithDescriptor:"), descriptor);
            if texture.is_null() {
                return Err(AtlasFull);
            }
            msg_void_region_u64_ptr_u64(
                texture,
                sel("replaceRegion:mipmapLevel:withBytes:bytesPerRow:"),
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize { width: width as u64, height: height as u64, depth: 1 },
                },
                0,
                raster.rgba.as_ptr() as *const c_void,
                (raster.width * 4) as u64,
            );
            texture
        };
        self.dedicated.insert(cache_key, texture);
        Ok(Some(ResolvedImage::Dedicated(texture, width, height)))
    }
}

#[allow(clippy::too_many_arguments)]
fn push_rect(
    out: &mut Vec<RectInstance>,
    quad: Box4,
    clip: Box4,
    color: Color,
    radii: Corners,
    extra: f64,
    kind: f32,
    expansion: f64,
) {
    out.push(RectInstance {
        rect: [quad.0 as f32, quad.1 as f32, quad.2 as f32, quad.3 as f32],
        clip: [clip.0 as f32, clip.1 as f32, clip.2 as f32, clip.3 as f32],
        params: [0.0, extra as f32, kind, expansion as f32],
        color: [color.r, color.g, color.b, color.a],
        pad: [0; 12],
        radii: wire_radii(radii),
    });
}

/// The four corners as the shader reads them, clockwise from the top
/// left — the ONE place the field order is spoken.
fn wire_radii(radii: Corners) -> [f32; 4] {
    [
        radii.top_left as f32,
        radii.top_right as f32,
        radii.bottom_right as f32,
        radii.bottom_left as f32,
    ]
}

/// One gradient instance: the fill's quad and corner, plus the second
/// half of the ramp packed into the bytes the struct already had.
#[allow(clippy::too_many_arguments)]
fn push_gradient(
    out: &mut Vec<RectInstance>,
    quad: Box4,
    clip: Box4,
    near: Color,
    far: Color,
    radii: Corners,
    aspect: f64,
    first: f64,
    second: f64,
    point: (f64, f64),
    kind: f32,
) {
    let mut pad = [0u8; 12];
    pad[0..4].copy_from_slice(&[far.r, far.g, far.b, far.a]);
    pad[4..8].copy_from_slice(&(point.0 as f32).to_ne_bytes());
    pad[8..12].copy_from_slice(&(point.1 as f32).to_ne_bytes());
    out.push(RectInstance {
        rect: [quad.0 as f32, quad.1 as f32, quad.2 as f32, quad.3 as f32],
        clip: [clip.0 as f32, clip.1 as f32, clip.2 as f32, clip.3 as f32],
        params: [aspect as f32, first as f32, kind, second as f32],
        color: [near.r, near.g, near.b, near.a],
        pad,
        radii: wire_radii(radii),
    });
}

/// A maximal run of one instance kind, in paint order — the draw-call
/// unit. Batches break only where rects and text alternate.
#[derive(Clone, Copy, PartialEq)]
enum RunKind {
    Rects,
    /// A batch of liquid-glass panes. It carries its own pass: the
    /// scene has to be blurred into the pyramid BEFORE the panes read
    /// it, and a pass boundary is what orders the two.
    Glass,
    Sprites,
    /// Sprites read from a DEDICATED texture (an image too big for the
    /// shared atlas) — the index points into the frame's texture list.
    Texture(u16),
}

#[derive(Clone, Copy)]
struct DrawRun {
    kind: RunKind,
    base: u32,
    count: u32,
    /// Glass only: how deep the pyramid must go for this batch — the
    /// deepest blur any pane in it asked for.
    levels: u32,
    /// Index into the frame's interned curves — a `u32` compare keeps
    /// run coalescing cheap, and the run only breaks when the SHAPE of
    /// the cut changes, which no scene of today ever does.
    round: u32,
}

fn note_run(runs: &mut Vec<DrawRun>, kind: RunKind, round: u32, index: usize) {
    match runs.last_mut() {
        Some(run) if run.kind == kind && run.round == round => run.count += 1,
        _ => runs.push(DrawRun { kind, base: index as u32, count: 1, round, levels: 0 }),
    }
}

fn box_union(a: Box4, b: Box4) -> Box4 {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// A pane joins the batch in front of it only if it does not TOUCH any
/// pane already in it. One batch reads one capture of the scene, so two
/// panes that overlap must not share it: the upper one would sample a
/// blur taken before the lower one existed, and stacked glass would
/// show nothing of the glass beneath it.
fn note_glass(
    runs: &mut Vec<DrawRun>,
    round: u32,
    index: usize,
    bounds: Box4,
    levels: u32,
    batch: &mut Option<Box4>,
) {
    let joins = matches!(runs.last(), Some(run) if run.kind == RunKind::Glass && run.round == round)
        && batch.is_some_and(|acc| box_intersect(acc, bounds).is_none());
    if joins {
        let run = runs.last_mut().expect("the run the match found");
        run.count += 1;
        run.levels = run.levels.max(levels);
        *batch = batch.map(|acc| box_union(acc, bounds));
    } else {
        runs.push(DrawRun { kind: RunKind::Glass, base: index as u32, count: 1, round, levels });
        *batch = Some(bounds);
    }
}

/// The instance lists of one frame, retained so their capacity survives
/// across frames.
#[derive(Default)]
struct FrameBatches {
    rects: Vec<RectInstance>,
    sprites: Vec<SpriteInstance>,
    glass: Vec<GlassInstance>,
    runs: Vec<DrawRun>,
    /// The frame's interned curves — slot 0 is always [`NO_ROUND`], so
    /// a frame with no rounded clip binds once and moves on.
    rounds: Vec<RoundClip>,
    /// Dedicated textures this frame reads (borrowed from the atlas's
    /// cache — the atlas owns and releases them).
    textures: Vec<Id>,
}

/// Walks the display list in paint order and fills the frame batches.
/// The clip stack mirrors `Surface::walk_clips`: snapped, intersected in
/// integers, an empty intersection degenerating to a zero-area box.
/// `Err(AtlasFull)` asks the caller to drain, reset the atlas and walk
/// again.
fn build_frame(
    display: &DisplayList,
    scale: usize,
    target: (usize, usize),
    engine: &dyn TextEngine,
    images: &dyn ImageEngine,
    atlas: &mut RunAtlas,
    batches: &mut FrameBatches,
) -> Result<(), AtlasFull> {
    batches.rects.clear();
    batches.sprites.clear();
    batches.glass.clear();
    batches.runs.clear();
    batches.textures.clear();
    batches.rounds.clear();
    batches.rounds.push(NO_ROUND);
    let out = &mut batches.rects;
    let factor = scale as f64;
    let whole: Box4 = (0, 0, target.0 as i64, target.1 as i64);
    // each entry: the hard cut, plus the index of the curve it lives
    // under (the CPU's inheritance rule, spoken in indices)
    let mut clips: Vec<(Box4, u32)> = Vec::new();
    // the boxes the open glass batch already holds — a pane that
    // touches one of them starts a batch of its own
    let mut glass_batch: Option<Box4> = None;
    for command in display.iter() {
        match command {
            DrawCommand::FillRect { rect, color, corner_radius } => {
                let Some(clip) = effective_clip(&clips, whole) else { continue };
                let snapped = snap_scaled(*rect, factor);
                if snapped.2 <= snapped.0 || snapped.3 <= snapped.1 {
                    continue;
                }
                if box_intersect(snapped, clip).is_none() {
                    continue;
                }
                let radii = corner_clamp(corner_radius * factor, snapped);
                push_rect(out, snapped, clip, *color, radii, 0.0, KIND_FILL, 0.0);
                note_run(&mut batches.runs, RunKind::Rects, round_of(&clips), out.len() - 1);
            }
            DrawCommand::Backdrop { rect, glass, corner_radius } => {
                let Some(clip) = effective_clip(&clips, whole) else { continue };
                let snapped = snap_scaled(*rect, factor);
                if snapped.2 <= snapped.0 || snapped.3 <= snapped.1 {
                    continue;
                }
                if box_intersect(snapped, clip).is_none() {
                    continue;
                }
                let radii = corner_clamp(corner_radius * factor, snapped);
                let paint = glass.scaled(factor);
                batches.glass.push(GlassInstance {
                    rect: [snapped.0 as f32, snapped.1 as f32, snapped.2 as f32, snapped.3 as f32],
                    clip: [clip.0 as f32, clip.1 as f32, clip.2 as f32, clip.3 as f32],
                    radii: wire_radii(radii),
                    lens: [
                        paint.blur as f32,
                        paint.refraction_band as f32,
                        paint.refraction_amount as f32,
                        paint.chromatic as f32,
                    ],
                    finish: [
                        paint.highlight_band as f32,
                        paint.highlight_intensity as f32,
                        paint.saturation as f32,
                        paint.brightness as f32,
                    ],
                    touch: [
                        paint.sheen as f32,
                        paint.spot_center.x as f32,
                        paint.spot_center.y as f32,
                        paint.spot_radius as f32,
                    ],
                    tint: [paint.tint.r, paint.tint.g, paint.tint.b, paint.tint.a],
                    highlight: [
                        paint.highlight.r,
                        paint.highlight.g,
                        paint.highlight.b,
                        paint.highlight.a,
                    ],
                    spot_alpha: paint.spot_alpha as f32,
                    pad: 0.0,
                });
                note_glass(
                    &mut batches.runs,
                    round_of(&clips),
                    batches.glass.len() - 1,
                    snapped,
                    bunny_ui::glass::levels_for(paint.blur) as u32,
                    &mut glass_batch,
                );
            }
            DrawCommand::Gradient { rect, paint, corner_radius } => {
                let Some(clip) = effective_clip(&clips, whole) else { continue };
                let snapped = snap_scaled(*rect, factor);
                if snapped.2 <= snapped.0 || snapped.3 <= snapped.1 {
                    continue;
                }
                if box_intersect(snapped, clip).is_none() {
                    continue;
                }
                let radii = corner_clamp(corner_radius * factor, snapped);
                match paint.scaled(factor) {
                    bunny_ui::layout::GradientPaint::Radial {
                        center,
                        start,
                        end,
                        aspect,
                        inner,
                        outer,
                    } => {
                        // the circle keeps its kind (and its corners)
                        // byte for byte; the ellipse drops the corners
                        // and takes the aspect slot instead
                        let (kind, corners) = if aspect == 1.0 {
                            (KIND_RADIAL, radii)
                        } else {
                            (KIND_ELLIPTIC, Corners::ZERO)
                        };
                        push_gradient(
                            out,
                            snapped,
                            clip,
                            inner,
                            outer,
                            corners,
                            aspect,
                            start,
                            end,
                            (center.x, center.y),
                            kind,
                        )
                    }
                    // the line's two ends fill the four numbers the
                    // struct still had: its start in the params, its
                    // end in the point — the quad stays the box
                    bunny_ui::layout::GradientPaint::Linear { start, end, from, to } => {
                        push_gradient(
                            out,
                            snapped,
                            clip,
                            from,
                            to,
                            radii,
                            0.0,
                            start.x,
                            start.y,
                            (end.x, end.y),
                            KIND_LINEAR,
                        )
                    }
                }
                note_run(&mut batches.runs, RunKind::Rects, round_of(&clips), out.len() - 1);
            }
            DrawCommand::StrokeRect { rect, color, width, corner_radius } => {
                let Some(clip) = effective_clip(&clips, whole) else { continue };
                let snapped = snap_scaled(*rect, factor);
                if snapped.2 <= snapped.0 || snapped.3 <= snapped.1 {
                    continue;
                }
                if box_intersect(snapped, clip).is_none() {
                    continue;
                }
                // the cpu's integer thickness, resolved here: at least
                // one device pixel, rounded once
                let thickness = (width * factor).max(1.0).round();
                let radii = corner_clamp(corner_radius * factor, snapped);
                push_rect(out, snapped, clip, *color, radii, thickness, KIND_STROKE, 0.0);
                note_run(&mut batches.runs, RunKind::Rects, round_of(&clips), out.len() - 1);
            }
            DrawCommand::Shadow { rect, radius, color, corner_radius } => {
                let Some(clip) = effective_clip(&clips, whole) else { continue };
                let snapped = snap_scaled(*rect, factor);
                // reach stays unrounded for the falloff; its rounding
                // only sizes the quad (the cpu loop bound) — any pixel
                // beyond it computes coverage zero anyway
                let reach = (radius * factor).max(1.0);
                let reach_px = reach.round() as i64;
                let corner = corner_clamp(corner_radius * factor, snapped);
                let expanded = (
                    snapped.0 - reach_px,
                    snapped.1 - reach_px,
                    snapped.2 + reach_px,
                    snapped.3 + reach_px,
                );
                if box_intersect(expanded, clip).is_none() {
                    continue;
                }
                push_rect(out, expanded, clip, *color, corner, reach, KIND_SHADOW, reach_px as f64);
                note_run(&mut batches.runs, RunKind::Rects, round_of(&clips), out.len() - 1);
            }
            DrawCommand::TextLine { origin, content, range, color, font } => {
                let Some(clip) = effective_clip(&clips, whole) else { continue };
                let slice = &content[range.0..range.1];
                let Some(entry) = atlas.resolve(slice, font, *color, scale, engine)? else {
                    continue;
                };
                // the composite_text mirror: one snap of the logical
                // origin, texels copied 1:1 from there
                let base_x = (origin.x * factor).round() as i64;
                let base_y = (origin.y * factor).round() as i64;
                let dest = (base_x, base_y, base_x + entry.width as i64, base_y + entry.height as i64);
                if box_intersect(dest, clip).is_none() {
                    continue;
                }
                let mut chunk_x: i64 = 0;
                for tile in &entry.tiles {
                    let chunk = (
                        base_x + chunk_x,
                        base_y,
                        base_x + chunk_x + tile.width as i64,
                        base_y + tile.height as i64,
                    );
                    chunk_x += tile.width as i64;
                    if box_intersect(chunk, clip).is_none() {
                        continue;
                    }
                    batches.sprites.push(SpriteInstance {
                        dest: [chunk.0 as f32, chunk.1 as f32, chunk.2 as f32, chunk.3 as f32],
                        tex: [
                            tile.x as f32,
                            tile.y as f32,
                            (tile.x + tile.width) as f32,
                            (tile.y + tile.height) as f32,
                        ],
                        clip: [clip.0 as f32, clip.1 as f32, clip.2 as f32, clip.3 as f32],
                    });
                    note_run(&mut batches.runs, RunKind::Sprites, round_of(&clips), batches.sprites.len() - 1);
                }
            }
            DrawCommand::Image { rect, source } => {
                let Some(clip) = effective_clip(&clips, whole) else { continue };
                let width = physical_extent(rect.size.width, scale) as u32;
                let height = physical_extent(rect.size.height, scale) as u32;
                if width == 0 || height == 0 {
                    continue;
                }
                // the composite_rgba mirror: one snap of the logical
                // origin, texels pasted 1:1 from there
                let base_x = (rect.origin.x * factor).round() as i64;
                let base_y = (rect.origin.y * factor).round() as i64;
                let dest =
                    (base_x, base_y, base_x + width as i64, base_y + height as i64);
                if box_intersect(dest, clip).is_none() {
                    continue;
                }
                match atlas.resolve_image(source, width, height, images)? {
                    None => {}
                    Some(ResolvedImage::Tiles(entry)) => {
                        let mut chunk_x: i64 = 0;
                        for tile in &entry.tiles {
                            let chunk = (
                                base_x + chunk_x,
                                base_y,
                                base_x + chunk_x + tile.width as i64,
                                base_y + tile.height as i64,
                            );
                            chunk_x += tile.width as i64;
                            if box_intersect(chunk, clip).is_none() {
                                continue;
                            }
                            batches.sprites.push(SpriteInstance {
                                dest: [
                                    chunk.0 as f32,
                                    chunk.1 as f32,
                                    chunk.2 as f32,
                                    chunk.3 as f32,
                                ],
                                tex: [
                                    tile.x as f32,
                                    tile.y as f32,
                                    (tile.x + tile.width) as f32,
                                    (tile.y + tile.height) as f32,
                                ],
                                clip: [
                                    clip.0 as f32,
                                    clip.1 as f32,
                                    clip.2 as f32,
                                    clip.3 as f32,
                                ],
                            });
                            note_run(
                                &mut batches.runs,
                                RunKind::Sprites,
                                round_of(&clips),
                                batches.sprites.len() - 1,
                            );
                        }
                    }
                    Some(ResolvedImage::Dedicated(texture, tex_w, tex_h)) => {
                        let index = match batches.textures.iter().position(|t| *t == texture)
                        {
                            Some(index) => index,
                            None => {
                                batches.textures.push(texture);
                                batches.textures.len() - 1
                            }
                        };
                        batches.sprites.push(SpriteInstance {
                            dest: [dest.0 as f32, dest.1 as f32, dest.2 as f32, dest.3 as f32],
                            tex: [0.0, 0.0, tex_w as f32, tex_h as f32],
                            clip: [
                                clip.0 as f32,
                                clip.1 as f32,
                                clip.2 as f32,
                                clip.3 as f32,
                            ],
                        });
                        note_run(
                            &mut batches.runs,
                            RunKind::Texture(index as u16),
                            round_of(&clips),
                            batches.sprites.len() - 1,
                        );
                    }
                }
            }
            DrawCommand::PushClip { rect, corner_radius } => {
                let snapped = snap_scaled(*rect, factor);
                let cut = match clips.last().copied() {
                    Some((top, _)) => box_intersect(snapped, top)
                        .unwrap_or((snapped.0, snapped.1, snapped.0, snapped.1)),
                    None => snapped,
                };
                // the same clamp and the same half-pixel door the CPU
                // keeps — below it, the clip INHERITS the open curve
                let radii = corner_clamp(corner_radius * factor, snapped);
                let round = if !radii.is_zero() {
                    let entry = RoundClip {
                        box4: [
                            snapped.0 as f32,
                            snapped.1 as f32,
                            snapped.2 as f32,
                            snapped.3 as f32,
                        ],
                        radii: wire_radii(radii),
                    };
                    match batches.rounds.iter().position(|r| *r == entry) {
                        Some(index) => index as u32,
                        None => {
                            batches.rounds.push(entry);
                            (batches.rounds.len() - 1) as u32
                        }
                    }
                } else {
                    clips.last().map_or(0, |(_, round)| *round)
                };
                clips.push((cut, round));
            }
            DrawCommand::PopClip => {
                clips.pop();
            }
        }
    }
    Ok(())
}

/// The clip a primitive paints under: the stack top intersected with the
/// target — `None` means nothing under it can paint (the CPU's clamped
/// loops collapse to nothing there).
fn effective_clip(clips: &[(Box4, u32)], whole: Box4) -> Option<Box4> {
    match clips.last().copied() {
        Some((top, _)) => box_intersect(top, whole),
        None => Some(whole),
    }
}

/// The curve index the open clip lives under — slot 0 when none.
fn round_of(clips: &[(Box4, u32)]) -> u32 {
    clips.last().map_or(0, |(_, round)| *round)
}

// MARK: - Instance buffers (a fixed ring, recycled by polling)

/// One in-flight frame: its instance buffer and the command buffer that
/// reads it. The command buffer is RETAINED while stored; `status >=
/// Completed` (or Error — Metal completes errored buffers too) frees the
/// slot for reuse.
#[derive(Clone, Copy)]
struct FrameSlot {
    buffer: Id,
    capacity: usize,
    command: Id,
}

impl FrameSlot {
    const fn empty() -> FrameSlot {
        FrameSlot { buffer: null_mut(), capacity: 0, command: null_mut() }
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
                    msg_void(slots[index].command, sels.release);
                    slots[index].command = null_mut();
                }
                *cursor = (index + 1) % slots.len();
                return index;
            }
        }
        let index = *cursor;
        msg_void(slots[index].command, sels.wait_completed);
        msg_void(slots[index].command, sels.release);
        slots[index].command = null_mut();
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

/// The per-window GPU state. Like `BACKING`: the view has no ivars, so
/// the presenter lives in a thread-local next to the run loop.
struct MetalPresenter {
    stack: MetalStack,
    layer: Id,
    view: Id,
    physical: (usize, usize),
    scale: usize,
    slots: [FrameSlot; 3],
    cursor: usize,
    atlas: RunAtlas,
    batches: FrameBatches,
    /// The last presented frame's key — an identical frame skips the
    /// encode entirely.
    retained: Option<(DisplayList, (usize, usize), usize, Color)>,
    /// Whether the layer currently presents inside the CATransaction —
    /// toggled ON only during live resize.
    transactional: bool,
    /// The scene texture and the blur pyramid, made on the first frame
    /// that carries glass and remade whenever the drawable resizes. A
    /// window that never shows glass never allocates them.
    glass: Option<GlassTextures>,
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
    retained: &Option<(DisplayList, (usize, usize), usize, Color)>,
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

thread_local! {
    /// The main window's presenter.
    static PRESENTER: RefCell<Option<MetalPresenter>> = const { RefCell::new(None) };
    /// One presenter per grafted DIALOG view (D127: a dialog is a real
    /// window, and a real window resizes on the GPU road like the main
    /// one — a CPU raster of a whole 1220×820 dialog on every step of a
    /// drag was the difference between a fluid workbench and a dialog
    /// that lagged its own corner).
    static VIEW_PRESENTERS: RefCell<HashMap<usize, MetalPresenter>> =
        RefCell::new(HashMap::new());
}

impl MetalPresenter {
    /// Flips the layer's present contract and remembers it. The flag
    /// has to be set BEFORE the drawable it governs is asked for: a
    /// drawable taken under the asynchronous contract and then
    /// presented inside the transaction is the one frame the layer
    /// stretches from the size it used to have.
    fn set_transactional(&mut self, live: bool) {
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
        for attempt in 0..3 {
            match build_frame(
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
                    if attempt == 2 {
                        // pathological frame: keep the rects, drop the
                        // rest of the text — never a crash
                        eprintln!("bunny_ui metal: atlas overflow survived two resets");
                        return;
                    }
                    crate::trace::mark("X", format_args!("what=atlas-drain"));
                    self.drain_slots();
                    self.atlas.reset(true);
                }
            }
        }
    }

    /// One frame: walk the list, upload, resize the drawable if the
    /// window changed, take the drawable as LATE as possible, encode,
    /// present, commit.
    fn present(
        &mut self,
        display: &DisplayList,
        size: Size,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let physical = (
                (size.width.round().max(0.0) as usize) * scale,
                (size.height.round().max(0.0) as usize) * scale,
            );
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
            if physical != self.physical || scale != self.scale {
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
            self.build_with_retries(display, scale, physical, text, images);
            // the contract of THIS frame's drawable, settled before it
            // is asked for. A window whose delegate armed the drag
            // already agrees and this changes nothing; a size the app
            // set itself has no delegate to speak for it, and lands here
            let live = msg_bool(self.view, self.stack.sels.in_live_resize) != 0;
            self.set_transactional(live);
            let index = acquire_slot(&mut self.slots, &mut self.cursor, &self.stack.sels);
            let (sprite_offset, glass_offset) = upload_frame(
                &mut self.slots[index],
                self.stack.device,
                &self.stack.sels,
                &self.batches,
            );
            let drawable = msg_id(self.layer, self.stack.sels.next_drawable);
            if drawable.is_null() {
                objc_autoreleasePoolPop(pool);
                return;
            }
            let drawable_texture = msg_id(drawable, self.stack.sels.texture);
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
            let command = self.stack.encode_frame(EncodeFrame {
                target,
                present_to,
                canvas,
                viewport: (physical.0 as f32, physical.1 as f32),
                instances: self.slots[index].buffer,
                sprite_offset,
                glass_offset,
                runs: &self.batches.runs,
                rounds: &self.batches.rounds,
                atlas_texture: self.atlas.texture,
                textures: &self.batches.textures,
                pyramid,
            });
            // live resize presents INSIDE the CATransaction: commit,
            // wait for the schedule, present — layer content and window
            // frame land together (the anti-tear toggle). every other
            // frame presents async, no stall.
            if live {
                msg_void(command, self.stack.sels.commit);
                msg_void(command, self.stack.sels.wait_scheduled);
                msg_void(drawable, self.stack.sels.present);
            } else {
                msg_void_id(command, self.stack.sels.present_drawable, drawable);
                msg_void(command, self.stack.sels.commit);
            }
            self.slots[index].command = msg_id(command, self.stack.sels.retain);
            self.retained = Some((display.clone(), physical, scale, canvas));
            objc_autoreleasePoolPop(pool);
        }
    }
}

/// Grafts the CAMetalLayer onto the view — called by `create_window`
/// BEFORE `setWantsLayer:`, so the view becomes layer-HOSTING and
/// `drawRect:` never runs. Returns false (and touches nothing) when the
/// GPU path is refused or cannot come up; the caller proceeds with the
/// CPU path.
///
/// The default is the GPU. `BUNNY_PRESENT=cpu` forces the CPU raster
/// forever; any failure to come up (no device, a shader that does not
/// compile) prints one line and falls back — a window never fails to
/// open because of Metal.
pub(crate) fn try_install(view: Id, scale: f64, width: f64, height: f64) -> bool {
    match graft(view, scale, width, height) {
        Some(presenter) => {
            PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
            true
        }
        None => false,
    }
}

/// The same graft on a DIALOG's view — a window of its own, presented
/// by its own layer and its own presenter, keyed by the view (a dialog
/// is pooled reusable-dead and never re-grafted). False leaves the view
/// on the CPU road, exactly as before.
pub(crate) fn try_install_view(view: Id, scale: f64, width: f64, height: f64) -> bool {
    match graft(view, scale, width, height) {
        Some(presenter) => {
            VIEW_PRESENTERS.with(|slot| {
                slot.borrow_mut().insert(view as usize, presenter);
            });
            true
        }
        None => false,
    }
}

/// Builds a presenter over a fresh CAMetalLayer on `view`, or answers
/// `None` when the GPU road is refused (`BUNNY_PRESENT=cpu`) or cannot
/// come up. Touches nothing on `None`.
fn graft(view: Id, scale: f64, width: f64, height: f64) -> Option<MetalPresenter> {
    if std::env::var("BUNNY_PRESENT").ok().as_deref() == Some("cpu") {
        return None;
    }
    let stack = MetalStack::create(PIXEL_FORMAT_BGRA8)?;
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
        if layer.is_null() {
            objc_autoreleasePoolPop(pool);
            return None;
        }
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
        // the layer is OURS, so AppKit never disables CA's implicit
        // actions on it — and an abrupt resize step would crossfade
        // the old drawable over the new for a quarter second, the
        // whole window double-exposed (no native window does this)
        crate::ffi::kill_layer_actions(layer);
        msg_void_id(view, sel("setLayer:"), layer);

        let device = stack.device;
        let mut presenter = MetalPresenter {
            stack,
            layer,
            view,
            physical: (0, 0),
            scale: 0,
            slots: [FrameSlot::empty(); 3],
            cursor: 0,
            atlas: RunAtlas::new(device),
            batches: FrameBatches::default(),
            retained: None,
            transactional: false,
            glass: None,
        };
        // anti-flash: the first clear happens before the window shows —
        // a virgin CAMetalLayer would flash black on order-front
        presenter.present(
            &DisplayList::default(),
            Size { width, height },
            scale as usize,
            bunny_ui::theme::canvas(),
            &bunny_ui::text_engine::PixelFont,
            &bunny_ui::image_engine::RawImages::default(),
        );
        objc_autoreleasePoolPop(pool);
        Some(presenter)
    }
}

/// True when this window presents by GPU — the shell branches ONCE per
/// frame on this, never mid-flight.
pub(crate) fn active() -> bool {
    PRESENTER.with(|slot| slot.borrow().is_some())
}

/// Presents one frame on a grafted DIALOG view. False when the view was
/// never grafted, and the caller takes the CPU road for it.
pub(crate) fn present_view(
    view: Id,
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) -> bool {
    VIEW_PRESENTERS.with(|slot| {
        let mut presenters = slot.borrow_mut();
        let Some(presenter) = presenters.get_mut(&(view as usize)) else {
            return false;
        };
        presenter.present(display, size, scale, canvas, text, images);
        true
    })
}

/// [`arm_transaction`] for a grafted dialog view — the dialog's own
/// delegate speaks for its own drag. A view on the CPU road is a no-op.
pub(crate) fn arm_transaction_view(view: Id, live: bool) {
    VIEW_PRESENTERS.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().get_mut(&(view as usize)) {
            presenter.set_transactional(live);
        }
    });
}

/// Arms (or disarms) the layer's transactional present, from AppKit's
/// own word that a drag is starting. It arrives BEFORE the first
/// resized frame, which is the only moment early enough: by the time a
/// frame observes `inLiveResize` the window has already grown, and a
/// drawable of the old size stretched to the new bounds is what the
/// eye reads as the whole UI drawn twice.
pub(crate) fn arm_transaction(live: bool) {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.set_transactional(live);
        }
    });
}

/// The GPU twin of the Surface + blit path: same display list in, one
/// presented frame out. `text` is the frame's engine — the atlas
/// rasterizes through it, exactly like the CPU compositor.
pub(crate) fn present_window(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.present(display, size, scale, canvas, text, images);
        }
    });
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
            // Shared: the CPU reads the render target directly (the
            // Apple-Silicon premise of the module)
            msg_void_u64(descriptor, sel("setStorageMode:"), STORAGE_MODE_SHARED);
            let target = msg_id_arg(stack.device, sel("newTextureWithDescriptor:"), descriptor);
            objc_autoreleasePoolPop(pool);
            if target.is_null() {
                return None;
            }
            let device = stack.device;
            Some(OffscreenGpu {
                stack,
                target,
                glass: None,
                width,
                height,
                slots: [FrameSlot::empty(); 3],
                cursor: 0,
                atlas: RunAtlas::new(device),
                batches: FrameBatches::default(),
            })
        }
    }

    fn drain(&mut self) {
        unsafe {
            for slot in &mut self.slots {
                if !slot.command.is_null() {
                    msg_void(slot.command, self.stack.sels.wait_completed);
                    msg_void(slot.command, self.stack.sels.release);
                    slot.command = null_mut();
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
            for attempt in 0..3 {
                match build_frame(
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
                        if attempt == 2 {
                            eprintln!("bunny_ui metal: atlas overflow survived two resets");
                            break;
                        }
                        self.drain();
                        self.atlas.reset(true);
                    }
                }
            }
            let index = acquire_slot(&mut self.slots, &mut self.cursor, &self.stack.sels);
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
            let command = self.stack.encode_frame(EncodeFrame {
                target: self.target,
                present_to: null_mut(),
                canvas,
                viewport: (self.width as f32, self.height as f32),
                instances: self.slots[index].buffer,
                sprite_offset,
                glass_offset,
                runs: &self.batches.runs,
                rounds: &self.batches.rounds,
                atlas_texture: self.atlas.texture,
                textures: &self.batches.textures,
                pyramid,
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
        let entries: usize = self.atlas.entries.values().map(Vec::len).sum();
        (
            entries + self.atlas.images.len() + self.atlas.dedicated.len(),
            self.atlas.packer.next_y,
        )
    }

    /// The rendered bytes, R,G,B,A per pixel — the same order as the
    /// Surface mirror, so parity compares are `==` over slices.
    pub fn read_rgba(&self) -> Vec<u8> {
        let mut bytes = vec![0u8; self.width * self.height * 4];
        unsafe {
            msg_void_ptr_u64_region_u64(
                self.target,
                self.stack.sels.get_bytes,
                bytes.as_mut_ptr() as *mut c_void,
                (self.width * 4) as u64,
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize {
                        width: self.width as u64,
                        height: self.height as u64,
                        depth: 1,
                    },
                },
                0,
            );
        }
        bytes
    }
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
        let retained = Some((quiet.clone(), (200usize, 120usize), 2usize, Color::CANVAS));
        assert!(frame_repeats(&retained, &quiet, (200, 120), 2, Color::CANVAS));
        assert!(!frame_repeats(&retained, &changed, (200, 120), 2, Color::CANVAS));
        assert!(!frame_repeats(&retained, &quiet, (210, 120), 2, Color::CANVAS));
        assert!(!frame_repeats(&retained, &quiet, (200, 120), 1, Color::CANVAS));
        assert!(!frame_repeats(&retained, &quiet, (200, 120), 2, Color::hex(0x18181D)));
        assert!(!frame_repeats(&None, &quiet, (200, 120), 2, Color::CANVAS));
    }

    #[test]
    fn shelves_place_reset_and_reuse() {
        // the pure allocator: exact-height reuse, new shelves below,
        // refusal at the brim, a clean slate after reset
        let mut packer = ShelfPacker::new(64, 32);
        assert_eq!(packer.place(40, 10), Some((0, 0)));
        assert_eq!(packer.place(30, 10), Some((0, 10)), "no room on the first shelf");
        assert_eq!(packer.place(10, 10), Some((40, 0)), "exact height reuses shelf one");
        assert_eq!(packer.place(64, 12), Some((0, 20)));
        assert_eq!(packer.place(1, 1), None, "the atlas is full below");
        assert_eq!(packer.place(65, 1), None, "wider than the atlas never fits");
        packer.reset();
        assert_eq!(packer.place(64, 32), Some((0, 0)), "reset reclaims everything");
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
        let engine = crate::text::CoreTextEngine::new();
        let logical = Size { width: 260.0, height: 100.0 };
        let scale = 2usize;
        let physical = (520, 200);
        let runtime = Runtime::new().text_engine(Rc::new(crate::text::CoreTextEngine::new()));
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
    fn overlapping_panes_break_the_batch_and_apart_ones_share_it() {
        // the walk's own rule, without a device: two panes that touch
        // take two batches, two that do not take one
        let batch = |first: Box4, second: Box4| {
            let mut runs: Vec<DrawRun> = Vec::new();
            let mut open: Option<Box4> = None;
            note_glass(&mut runs, 0, 0, first, 1, &mut open);
            note_glass(&mut runs, 0, 1, second, 2, &mut open);
            runs
        };
        let apart = batch((0, 0, 10, 10), (20, 20, 30, 30));
        assert_eq!(apart.len(), 1, "panes that never meet share one capture");
        assert_eq!(apart[0].count, 2);
        assert_eq!(apart[0].levels, 2, "the batch digs as deep as its deepest pane");

        let over = batch((0, 0, 20, 20), (10, 10, 30, 30));
        assert_eq!(over.len(), 2, "glass over glass takes a capture of its own");
        assert_eq!(over[1].levels, 2);
    }
}
