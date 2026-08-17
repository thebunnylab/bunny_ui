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
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

use bunny_ui::layout::{Color, DisplayList, DrawCommand, Rect, Size};

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
    fn msg_void_id_u64_u64(obj: Id, sel: Sel, a: Id, b: u64, c: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_ptr_u64_u64(obj: Id, sel: Sel, a: *const c_void, b: u64, c: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64x5(obj: Id, sel: Sel, a: u64, b: u64, c: u64, d: u64, e: u64);
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
}

// MARK: - Metal vocabulary (constants live in source, like the CG ones)

const PIXEL_FORMAT_RGBA8: u64 = 70; // MTLPixelFormatRGBA8Unorm — the mirror's byte order
const PIXEL_FORMAT_BGRA8: u64 = 80; // MTLPixelFormatBGRA8Unorm — the only format a layer takes
const LOAD_ACTION_CLEAR: u64 = 2;
const STORE_ACTION_STORE: u64 = 1;
const BLEND_ONE: u64 = 1;
const BLEND_SOURCE_ALPHA: u64 = 4;
const BLEND_ONE_MINUS_SOURCE_ALPHA: u64 = 5;
const STORAGE_MODE_SHARED: u64 = 0;
const TEXTURE_USAGE_SHADER_READ: u64 = 1;
const TEXTURE_USAGE_RENDER_TARGET: u64 = 4;
// CPUCacheModeWriteCombined | StorageModeShared — the CPU only writes
// instance bytes, the GPU only reads them.
const RESOURCE_SHARED_WRITE_COMBINED: u64 = 1;
const PRIMITIVE_TRIANGLE: u64 = 3;
const STATUS_COMPLETED: u64 = 4; // MTLCommandBufferStatus: Completed=4, Error=5

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
    params: [f32; 4], // corner_radius, thickness or reach, kind (0/1/2), expansion
    color: [u8; 4],   // straight RGBA
    pad: [u8; 12],
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

const _: () = {
    assert!(std::mem::size_of::<RectInstance>() == 64);
    assert!(std::mem::offset_of!(RectInstance, rect) == 0);
    assert!(std::mem::offset_of!(RectInstance, clip) == 16);
    assert!(std::mem::offset_of!(RectInstance, params) == 32);
    assert!(std::mem::offset_of!(RectInstance, color) == 48);
    assert!(std::mem::offset_of!(RectInstance, pad) == 52);
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
    uchar  pad[12];
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

static float rect_sdf(float2 p, float4 rect, float radius) {
    float2 shifted = max(rect.xy + radius - p, p - (rect.zw - radius));
    float outside = length(max(shifted, 0.0));
    float inside = min(max(shifted.x, shifted.y), 0.0);
    return outside + inside - radius;
}

static float rect_cov(float2 p, float4 rect, float radius) {
    return clamp(0.5 - rect_sdf(p, rect, radius), 0.0, 1.0);
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

fragment float4 rect_fragment(RectVary in [[stage_in]],
                              device const RectInstance* rects [[buffer(0)]]) {
    RectInstance rect = rects[in.id];
    float2 p = in.position.xy;
    // kind 0: fill — the ring and the halo arrive with the sdf phase
    float coverage = rect_cov(p, rect.rect, rect.params.x);
    float4 color = float4(rect.color) / 255.0;
    return float4(color.rgb, color.a * coverage);
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
                                texture2d<float, access::read> atlas [[texture(0)]]) {
    SpriteInstance sprite = sprites[in.id];
    float2 texel = sprite.tex.xy + (floor(in.position.xy) - floor(sprite.dest.xy));
    return atlas.read(uint2(texel));
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
    draw: Sel,
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
                draw: sel("drawPrimitives:vertexStart:vertexCount:instanceCount:baseInstance:"),
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
    #[allow(dead_code)] // the draw calls arrive with the sprite phase
    sprite_pipeline: Id,
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
                build_pipeline(device, library, "rect_vertex", "rect_fragment", format)?;
            let sprite_pipeline =
                build_pipeline(device, library, "sprite_vertex", "sprite_fragment", format)?;
            msg_void(library, sel("release"));
            Ok(MetalStack {
                device,
                queue,
                rect_pipeline,
                sprite_pipeline,
                pass_class: class("MTLRenderPassDescriptor"),
                sels: Sels::new(),
            })
        }
    }

    /// One pass over `target`: clear to `canvas`, then the instanced
    /// draws. Returns the command buffer (autoreleased — the caller
    /// holds the pool and decides how to present it).
    unsafe fn encode_frame(
        &self,
        target: Id,
        canvas: Color,
        viewport: (f32, f32),
        instances: Id,
        rect_count: usize,
    ) -> Id {
        unsafe {
            let pass = msg_id(self.pass_class, self.sels.render_pass_descriptor);
            let attachment = msg_id_u64(
                msg_id(pass, self.sels.color_attachments),
                self.sels.object_at,
                0,
            );
            msg_void_id(attachment, self.sels.set_texture, target);
            msg_void_u64(attachment, self.sels.set_load_action, LOAD_ACTION_CLEAR);
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
            let command = msg_id(self.queue, self.sels.command_buffer);
            let encoder = msg_id_arg(command, self.sels.encoder, pass);
            if rect_count > 0 {
                msg_void_id(encoder, self.sels.set_pipeline, self.rect_pipeline);
                msg_void_id_u64_u64(encoder, self.sels.set_vertex_buffer, instances, 0, 0);
                msg_void_id_u64_u64(encoder, self.sels.set_fragment_buffer, instances, 0, 0);
                let size = [viewport.0, viewport.1];
                msg_void_ptr_u64_u64(
                    encoder,
                    self.sels.set_vertex_bytes,
                    size.as_ptr() as *const c_void,
                    8,
                    1,
                );
                msg_void_u64x5(
                    encoder,
                    self.sels.draw,
                    PRIMITIVE_TRIANGLE,
                    0,
                    6,
                    rect_count as u64,
                    0,
                );
            }
            msg_void(encoder, self.sels.end_encoding);
            command
        }
    }
}

unsafe fn build_pipeline(
    device: Id,
    library: Id,
    vertex: &str,
    fragment: &str,
    format: u64,
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
        msg_void_bool(attachment, sel("setBlendingEnabled:"), 1);
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

/// The CPU's radius clamp, verbatim: never negative via `max`, then
/// halved against the SNAPPED extent (which can drive it negative on a
/// degenerate rect — the raster accepts that, so the port does too).
fn corner_clamp(scaled_radius: f64, snapped: Box4) -> f64 {
    scaled_radius
        .max(0.0)
        .min((snapped.2 - snapped.0) as f64 / 2.0)
        .min((snapped.3 - snapped.1) as f64 / 2.0)
}

const KIND_FILL: f32 = 0.0;

fn push_rect(
    out: &mut Vec<RectInstance>,
    quad: Box4,
    clip: Box4,
    color: Color,
    radius: f64,
    extra: f64,
    kind: f32,
    expansion: f64,
) {
    out.push(RectInstance {
        rect: [quad.0 as f32, quad.1 as f32, quad.2 as f32, quad.3 as f32],
        clip: [clip.0 as f32, clip.1 as f32, clip.2 as f32, clip.3 as f32],
        params: [radius as f32, extra as f32, kind, expansion as f32],
        color: [color.r, color.g, color.b, color.a],
        pad: [0; 12],
    });
}

/// Walks the display list in paint order and emits rect instances. The
/// clip stack mirrors `Surface::walk_clips`: snapped, intersected in
/// integers, an empty intersection degenerating to a zero-area box.
/// Stroke, shadow and text arrive with their phases.
fn build_rects(
    display: &DisplayList,
    scale: usize,
    target: (usize, usize),
    out: &mut Vec<RectInstance>,
) {
    out.clear();
    let factor = scale as f64;
    let whole: Box4 = (0, 0, target.0 as i64, target.1 as i64);
    let mut clips: Vec<Box4> = Vec::new();
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
                let radius = corner_clamp(corner_radius * factor, snapped);
                push_rect(out, snapped, clip, *color, radius, 0.0, KIND_FILL, 0.0);
            }
            DrawCommand::StrokeRect { .. } | DrawCommand::Shadow { .. } => {
                // the ring and the halo arrive with the sdf phase
            }
            DrawCommand::TextLine { .. } => {
                // text arrives with the atlas phase
            }
            DrawCommand::PushClip { rect } => {
                let snapped = snap_scaled(*rect, factor);
                let top = match clips.last().copied() {
                    Some(top) => box_intersect(snapped, top)
                        .unwrap_or((snapped.0, snapped.1, snapped.0, snapped.1)),
                    None => snapped,
                };
                clips.push(top);
            }
            DrawCommand::PopClip => {
                clips.pop();
            }
        }
    }
}

/// The clip a primitive paints under: the stack top intersected with the
/// target — `None` means nothing under it can paint (the CPU's clamped
/// loops collapse to nothing there).
fn effective_clip(clips: &[Box4], whole: Box4) -> Option<Box4> {
    match clips.last().copied() {
        Some(top) => box_intersect(top, whole),
        None => Some(whole),
    }
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

/// Copies the instance bytes into the slot's buffer, growing it when the
/// frame outgrows the capacity — the size is EXACT before Metal is
/// touched, so there is no speculative encode and no overflow retry.
unsafe fn upload(slot: &mut FrameSlot, device: Id, sels: &Sels, bytes: &[u8]) {
    unsafe {
        if bytes.is_empty() {
            return;
        }
        if slot.buffer.is_null() || slot.capacity < bytes.len() {
            if !slot.buffer.is_null() {
                msg_void(slot.buffer, sels.release);
            }
            let capacity = bytes.len().next_multiple_of(4096);
            slot.buffer = msg_id_u64_u64(
                device,
                sel("newBufferWithLength:options:"),
                capacity as u64,
                RESOURCE_SHARED_WRITE_COMBINED,
            );
            slot.capacity = capacity;
        }
        let contents = msg_id(slot.buffer, sels.contents);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), contents as *mut u8, bytes.len());
    }
}

fn instance_bytes(rects: &[RectInstance]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            rects.as_ptr() as *const u8,
            std::mem::size_of_val(rects),
        )
    }
}

// MARK: - The window presenter

/// The per-window GPU state. Like `BACKING`: the view has no ivars, so
/// the presenter lives in a thread-local next to the run loop.
struct MetalPresenter {
    stack: MetalStack,
    layer: Id,
    physical: (usize, usize),
    scale: usize,
    slots: [FrameSlot; 3],
    cursor: usize,
    rects: Vec<RectInstance>,
}

thread_local! {
    static PRESENTER: RefCell<Option<MetalPresenter>> = const { RefCell::new(None) };
}

impl MetalPresenter {
    /// A free slot from the ring: polled by `status`, oldest-first. When
    /// all three ride the GPU (a burst above the refresh rate, which
    /// AppKit's event coalescing already prevents), waits for the oldest
    /// — bounded by one sub-millisecond frame.
    fn acquire_slot(&mut self) -> usize {
        unsafe {
            for offset in 0..self.slots.len() {
                let index = (self.cursor + offset) % self.slots.len();
                let free = self.slots[index].command.is_null()
                    || msg_u64(self.slots[index].command, self.stack.sels.status)
                        >= STATUS_COMPLETED;
                if free {
                    if !self.slots[index].command.is_null() {
                        msg_void(self.slots[index].command, self.stack.sels.release);
                        self.slots[index].command = null_mut();
                    }
                    self.cursor = (index + 1) % self.slots.len();
                    return index;
                }
            }
            let index = self.cursor;
            msg_void(self.slots[index].command, self.stack.sels.wait_completed);
            msg_void(self.slots[index].command, self.stack.sels.release);
            self.slots[index].command = null_mut();
            self.cursor = (index + 1) % self.slots.len();
            index
        }
    }

    /// One frame: walk the list, upload, resize the drawable if the
    /// window changed, take the drawable as LATE as possible, encode,
    /// present, commit.
    fn present(&mut self, display: &DisplayList, size: Size, scale: usize, canvas: Color) {
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
            let mut rects = std::mem::take(&mut self.rects);
            build_rects(display, scale, physical, &mut rects);
            let index = self.acquire_slot();
            upload(
                &mut self.slots[index],
                self.stack.device,
                &self.stack.sels,
                instance_bytes(&rects),
            );
            let drawable = msg_id(self.layer, self.stack.sels.next_drawable);
            if drawable.is_null() {
                self.rects = rects;
                objc_autoreleasePoolPop(pool);
                return;
            }
            let target = msg_id(drawable, self.stack.sels.texture);
            let command = self.stack.encode_frame(
                target,
                canvas,
                (physical.0 as f32, physical.1 as f32),
                self.slots[index].buffer,
                rects.len(),
            );
            msg_void_id(command, self.stack.sels.present_drawable, drawable);
            msg_void(command, self.stack.sels.commit);
            self.slots[index].command = msg_id(command, self.stack.sels.retain);
            self.rects = rects;
            objc_autoreleasePoolPop(pool);
        }
    }
}

/// Grafts the CAMetalLayer onto the view — called by `create_window`
/// BEFORE `setWantsLayer:`, so the view becomes layer-HOSTING and
/// `drawRect:` never runs. Returns false (and touches nothing) when the
/// GPU path is not requested or cannot come up; the caller proceeds with
/// today's CPU path.
pub(crate) fn try_install(view: Id, scale: f64, width: f64, height: f64) -> bool {
    if std::env::var("BUNNY_PRESENT").ok().as_deref() != Some("gpu") {
        return false;
    }
    let Some(stack) = MetalStack::create(PIXEL_FORMAT_BGRA8) else {
        return false;
    };
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
        if layer.is_null() {
            objc_autoreleasePoolPop(pool);
            return false;
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
        msg_void_id(view, sel("setLayer:"), layer);

        let mut presenter = MetalPresenter {
            stack,
            layer,
            physical: (0, 0),
            scale: 0,
            slots: [FrameSlot::empty(); 3],
            cursor: 0,
            rects: Vec::new(),
        };
        // anti-flash: the first clear happens before the window shows —
        // a virgin CAMetalLayer would flash black on order-front
        presenter.present(
            &DisplayList::default(),
            Size { width, height },
            scale as usize,
            bunny_ui::theme::canvas(),
        );
        PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
        objc_autoreleasePoolPop(pool);
        true
    }
}

/// True when this window presents by GPU — the shell branches ONCE per
/// frame on this, never mid-flight.
pub(crate) fn active() -> bool {
    PRESENTER.with(|slot| slot.borrow().is_some())
}

/// The GPU twin of the Surface + blit path: same display list in, one
/// presented frame out.
pub(crate) fn present_window(display: &DisplayList, size: Size, scale: usize, canvas: Color) {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.present(display, size, scale, canvas);
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
    width: usize,
    height: usize,
    slot: FrameSlot,
    rects: Vec<RectInstance>,
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
            Some(OffscreenGpu {
                stack,
                target,
                width,
                height,
                slot: FrameSlot::empty(),
                rects: Vec::new(),
            })
        }
    }

    /// Renders and WAITS — determinism for tests and honest numbers for
    /// the bench (walk + upload + encode + commit + GPU time, nothing
    /// hidden). The wait also makes single-buffering safe.
    pub fn present_wait(&mut self, display: &DisplayList, scale: usize, canvas: Color) {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let mut rects = std::mem::take(&mut self.rects);
            build_rects(display, scale, (self.width, self.height), &mut rects);
            upload(
                &mut self.slot,
                self.stack.device,
                &self.stack.sels,
                instance_bytes(&rects),
            );
            let command = self.stack.encode_frame(
                self.target,
                canvas,
                (self.width as f32, self.height as f32),
                self.slot.buffer,
                rects.len(),
            );
            msg_void(command, self.stack.sels.commit);
            msg_void(command, self.stack.sels.wait_completed);
            self.rects = rects;
            objc_autoreleasePoolPop(pool);
        }
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
        let cpu = rasterize_with(&display, physical.0, physical.1, scale, canvas, &PixelFont)
            .to_rgba_bytes();
        let mut gpu = OffscreenGpu::new(physical.0, physical.1).expect("offscreen gpu");
        gpu.present_wait(&display, scale, canvas);
        (gpu.read_rgba(), cpu)
    }

    fn max_channel_delta(a: &[u8], b: &[u8]) -> u8 {
        a.iter().zip(b.iter()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
    }

    #[test]
    fn the_wire_structs_hold_their_layout() {
        // the const asserts already gate the build; this pins the numbers
        // in a place a failing CI can point at
        assert_eq!(std::mem::size_of::<RectInstance>(), 64);
        assert_eq!(std::mem::align_of::<RectInstance>(), 4);
        assert_eq!(std::mem::size_of::<SpriteInstance>(), 48);
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
        gpu.present_wait(&DisplayList::default(), 2, canvas);
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
}
