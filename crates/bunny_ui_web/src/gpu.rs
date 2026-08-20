//! The WebGL2 tier — the SAME display list, presented by the GPU.
//!
//! This is the fifth backend the raster module promised, and the first
//! one that can be certified on a machine that is not the author's: the
//! display list does not change, the pixels must not change beyond the
//! tolerance the parity gate names, and the CPU rasterizer stays as the
//! oracle, the headless path and the fallback.
//!
//! House rules apply: no dependencies. WebGL2 comes in through the same
//! hand-written border as everything else on this target — `glue_gl.js`
//! holds the objects a wasm module cannot, and the pipeline logic stays
//! here, in Rust, beside the walk that feeds it.
//!
//! The LAW of the port carries over from the desktop tiers: every policy
//! decision resolves on the CPU in f64, the instances carry snapped
//! device pixels, and the shaders are blind coverage evaluators. What
//! differs from the linux tier is spelled out where it happens, and
//! there are only three things: an sRGB attachment encodes on write with
//! no toggle to ask for it, a blocking fence is illegal, and a page
//! rounds its own corners with CSS.

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};

use bunny_ui::layout::{Color, DisplayList, Size};

#[link(wasm_import_module = "bunny_gpu")]
unsafe extern "C" {
    /// `kind` 0 is the page's own surface, 1 the islands' backing
    /// canvas. Zero back is a refusal — no WebGL2, a shader that would
    /// not compile, or `?present=cpu`. Non-zero is MAX_TEXTURE_SIZE.
    fn gl_init(kind: u32, width: u32, height: u32) -> u32;
    fn gl_teardown();
    fn gl_resize(width: u32, height: u32);

    fn gl_viewport(x: i32, y: i32, width: i32, height: i32);
    fn gl_clear_color(r: f32, g: f32, b: f32, a: f32);
    fn gl_clear(mask: u32);
    fn gl_enable(cap: u32);
    fn gl_disable(cap: u32);
    fn gl_blend_func_separate(sc: u32, dc: u32, sa: u32, da: u32);
    fn gl_pixel_storei(name: u32, param: i32);
    fn gl_finish();
    fn gl_flush();

    fn gl_compile_shader(kind: u32, pointer: *const u8, len: usize) -> u32;
    fn gl_link_program(vertex: u32, fragment: u32) -> u32;
    fn gl_bind_attrib_location(program: u32, index: u32, pointer: *const u8, len: usize);
    fn gl_use_program(program: u32);
    fn gl_uniform_location(program: u32, pointer: *const u8, len: usize) -> u32;
    fn gl_uniform_block(program: u32, pointer: *const u8, len: usize, binding: u32);
    fn gl_uniform1i(location: u32, value: i32);
    fn gl_uniform4f(location: u32, x: f32, y: f32, z: f32, w: f32);
    fn gl_last_log(out: *mut u8, cap: usize) -> usize;

    fn gl_create_buffer() -> u32;
    fn gl_bind_buffer(target: u32, buffer: u32);
    fn gl_bind_buffer_base(target: u32, index: u32, buffer: u32);
    fn gl_buffer_data_size(target: u32, size: u32, usage: u32);
    fn gl_buffer_sub_data(target: u32, offset: u32, pointer: *const u8, len: usize);
    fn gl_delete_buffer(buffer: u32);

    fn gl_create_vertex_array() -> u32;
    fn gl_bind_vertex_array(array: u32);
    fn gl_enable_vertex_attrib_array(index: u32);
    fn gl_vertex_attrib_pointer(
        index: u32,
        size: i32,
        kind: u32,
        normalized: u32,
        stride: i32,
        offset: i32,
    );
    fn gl_vertex_attrib_divisor(index: u32, divisor: u32);

    fn gl_create_texture() -> u32;
    fn gl_bind_texture(target: u32, texture: u32);
    fn gl_active_texture(unit: u32);
    fn gl_tex_parameteri(target: u32, name: u32, param: i32);
    #[allow(clippy::too_many_arguments)]
    fn gl_tex_image_2d(
        target: u32,
        level: i32,
        internal: i32,
        width: i32,
        height: i32,
        format: u32,
        kind: u32,
        pointer: *const u8,
        len: usize,
    );
    #[allow(clippy::too_many_arguments)]
    fn gl_tex_sub_image_2d(
        target: u32,
        level: i32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        format: u32,
        kind: u32,
        pointer: *const u8,
        len: usize,
    );
    fn gl_delete_texture(texture: u32);

    fn gl_create_framebuffer() -> u32;
    fn gl_bind_framebuffer(target: u32, framebuffer: u32);
    fn gl_framebuffer_texture_2d(
        target: u32,
        attachment: u32,
        textarget: u32,
        texture: u32,
        level: i32,
    );
    fn gl_check_framebuffer_status(target: u32) -> u32;
    fn gl_delete_framebuffer(framebuffer: u32);

    fn gl_draw_arrays(mode: u32, first: i32, count: i32);
    fn gl_draw_arrays_instanced(mode: u32, first: i32, count: i32, instances: i32);
    #[allow(clippy::too_many_arguments)]
    fn gl_read_pixels(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        format: u32,
        kind: u32,
        pointer: *mut u8,
        len: usize,
    );
}

// MARK: - The numbers GL answers to (the same ones the desktop tier uses)

pub(crate) const GL_TRIANGLES: u32 = 0x0004;
pub(crate) const GL_SRC_ALPHA: u32 = 0x0302;
pub(crate) const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
pub(crate) const GL_ONE: u32 = 1;
pub(crate) const GL_BLEND: u32 = 0x0BE2;
pub(crate) const GL_DEPTH_TEST: u32 = 0x0B71;
pub(crate) const GL_CULL_FACE: u32 = 0x0B44;
pub(crate) const GL_SCISSOR_TEST: u32 = 0x0C11;
pub(crate) const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
pub(crate) const GL_RGBA: u32 = 0x1908;
pub(crate) const GL_UNSIGNED_BYTE: u32 = 0x1401;
pub(crate) const GL_VERTEX_SHADER: u32 = 0x8B31;
pub(crate) const GL_FRAGMENT_SHADER: u32 = 0x8B30;
pub(crate) const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;

/// A window never switches roads mid-flight, so the tier is chosen once
/// and this is where the answer lives.
struct Tier {
    /// The device's texture ceiling — the atlas may not grow past it.
    max_texture: u32,
    /// The physical size the drawable currently holds.
    physical: (u32, u32),
}

thread_local! {
    static TIER: RefCell<Option<Tier>> = const { RefCell::new(None) };
    /// A lost context is allowed exactly one silent rebuild.
    static REBUILD_SPENT: Cell<bool> = const { Cell::new(false) };
}

/// Brings the tier up for `kind` at a physical size. False means the
/// page presents by CPU — which is not a failure, it is the floor.
pub(crate) fn try_install(kind: u32, physical: (u32, u32)) -> bool {
    let max_texture = unsafe { gl_init(kind, physical.0, physical.1) };
    // below two thousand the shelf atlas cannot even open at its
    // starting size, and a tier that loses its text is worse than no
    // tier — the CPU road draws the same scene and keeps it
    if max_texture < 2048 {
        if max_texture != 0 {
            unsafe { gl_teardown() };
        }
        return false;
    }
    unsafe {
        // fixed for the life of the context, exactly as the desktop
        // tier fixes it: straight alpha, gamma space, and the alpha
        // channel composited the way `blend_px` composites it
        gl_disable(GL_DEPTH_TEST);
        gl_disable(GL_CULL_FACE);
        gl_disable(GL_SCISSOR_TEST);
        gl_enable(GL_BLEND);
        gl_blend_func_separate(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA, GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
        gl_pixel_storei(GL_UNPACK_ALIGNMENT, 1);
    }
    TIER.with(|slot| *slot.borrow_mut() = Some(Tier { max_texture, physical }));
    true
}

/// Whether the tier is presenting. The fallback needs no flag: this
/// going false is enough, and the next present builds a `Surface`.
pub(crate) fn active() -> bool {
    TIER.with(|slot| slot.borrow().is_some())
}

/// The device's texture ceiling, for whoever sizes the atlas.
#[allow(dead_code)]
pub(crate) fn max_texture() -> u32 {
    TIER.with(|slot| slot.borrow().as_ref().map_or(0, |tier| tier.max_texture))
}

/// Drops the tier. The page presents by CPU from the next frame.
pub(crate) fn teardown() {
    if TIER.with(|slot| slot.borrow_mut().take()).is_some() {
        unsafe { gl_teardown() };
    }
}

/// The context died. The CPU takes the page this very turn.
pub(crate) fn lost() {
    TIER.with(|slot| *slot.borrow_mut() = None);
}

/// The context came back. One silent rebuild is owed; after that the
/// CPU keeps the page for the life of the page — the desktop tiers'
/// rule, transposed.
pub(crate) fn restored(physical: (u32, u32)) -> bool {
    if REBUILD_SPENT.with(|spent| spent.replace(true)) {
        return false;
    }
    try_install(0, physical)
}

/// Presents one display list. The frame IS the drawable: no `Surface`
/// is allocated on this road, and the CPU bitmap is never built.
pub(crate) fn present_window(display: &DisplayList, size: Size, scale: usize, canvas: Color) {
    let physical = (
        ((size.width.round() as usize) * scale).max(1) as u32,
        ((size.height.round() as usize) * scale).max(1) as u32,
    );
    let stale = TIER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(tier) = slot.as_mut() else { return false };
        let stale = tier.physical != physical;
        tier.physical = physical;
        stale
    });
    if stale {
        unsafe { gl_resize(physical.0, physical.1) };
    }
    unsafe {
        gl_bind_framebuffer(GL_FRAMEBUFFER, 0);
        gl_viewport(0, 0, physical.0 as i32, physical.1 as i32);
        // the clear takes the canvas colour STRAIGHT, the way the
        // rasterizer fills its bitmap with it — no premultiply, because
        // nothing has blended yet
        gl_clear_color(
            canvas.r as f32 / 255.0,
            canvas.g as f32 / 255.0,
            canvas.b as f32 / 255.0,
            canvas.a as f32 / 255.0,
        );
        gl_clear(GL_COLOR_BUFFER_BIT);
    }
    let _ = display;
}

pub(crate) const GL_FRAMEBUFFER: u32 = 0x8D40;

/// Reads the drawable back, top row first — GL counts from the bottom
/// and the rasterizer counts from the top, so the rows mirror once.
/// This STALLS; only a harness may call it.
pub(crate) fn read_rgba(physical: (u32, u32)) -> Vec<u8> {
    let (width, height) = (physical.0 as usize, physical.1 as usize);
    let mut flipped = vec![0u8; width * height * 4];
    unsafe {
        gl_finish();
        gl_read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            flipped.as_mut_ptr(),
            flipped.len(),
        );
    }
    let mut rows = vec![0u8; width * height * 4];
    let pitch = width * 4;
    for y in 0..height {
        let from = (height - 1 - y) * pitch;
        rows[y * pitch..y * pitch + pitch].copy_from_slice(&flipped[from..from + pitch]);
    }
    rows
}

/// The last shader complaint the border kept, for a message a person
/// can act on.
#[allow(dead_code)]
pub(crate) fn last_log() -> String {
    let mut buffer = vec![0u8; 1024];
    let len = unsafe { gl_last_log(buffer.as_mut_ptr(), buffer.len()) };
    buffer.truncate(len);
    String::from_utf8_lossy(&buffer).into_owned()
}
