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

use bunny_ui::gpu::shaders::PRELUDE_300ES;
use bunny_ui::gpu::walk::{
    build_frame, AtlasGround, FrameBatches, GlassInstance, RectInstance, RunAtlas, RunKind,
    SpriteInstance, GLASS_MAX_LEVEL,
};
use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;

#[link(wasm_import_module = "bunny_gpu")]
unsafe extern "C" {
    /// `kind` 0 is the page's own surface, 1 the islands' backing
    /// canvas. Zero back is a refusal — no WebGL2, a shader that would
    /// not compile, or `?present=cpu`. Non-zero is MAX_TEXTURE_SIZE.
    fn gl_init(kind: u32, width: u32, height: u32) -> u32;
    fn gl_log(pointer: *const u8, len: usize);
    fn gl_island_blit(id: u32, width: u32, height: u32);
    /// The page's own clock, for a line that must say what it timed.
    pub(crate) fn gl_now() -> f64;
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

// MARK: - The pipelines (compiled once, at install)

const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_UNIFORM_BUFFER: u32 = 0x8A11;
const GL_STREAM_DRAW: u32 = 0x88E0;
const GL_FLOAT: u32 = 0x1406;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE0: u32 = 0x84C0;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_NEAREST: i32 = 0x2600;
const GL_CLAMP_TO_EDGE: i32 = 0x812F;
const GL_UNPACK_ROW_LENGTH: u32 = 0x0CF2;
const UBO_FRAME_BINDING: u32 = 0;
const UBO_ROUND_BINDING: u32 = 1;

struct Pipelines {
    rect: u32,
    sprite: u32,
    glass: u32,
    blur: u32,
    blit: u32,
    vao_rect: u32,
    vao_sprite: u32,
    vao_glass: u32,
    /// A vertex array with no attributes at all: the full-screen
    /// triangle builds its own corners from the vertex id.
    vao_full: u32,
    glass_buffer: u32,
    glass_cap: u32,
    blur_step: u32,
    blur_mode: u32,
    rects: u32,
    sprites: u32,
    frame_ubo: u32,
    round_ubo: u32,
    /// What each instance buffer currently holds, in bytes — a smaller
    /// frame reuses the store, a bigger one re-opens it.
    rect_cap: u32,
    sprite_cap: u32,
}

pub(crate) fn say(message: &str) {
    unsafe { gl_log(message.as_ptr(), message.len()) };
}

fn compile(kind: u32, source: &str) -> u32 {
    unsafe { gl_compile_shader(kind, source.as_ptr(), source.len()) }
}

fn program(vertex: &str, fragment: &str, attribs: &[&str]) -> Result<u32, String> {
    let vs = compile(GL_VERTEX_SHADER, vertex);
    if vs == 0 {
        return Err(last_log());
    }
    let fs = compile(GL_FRAGMENT_SHADER, fragment);
    if fs == 0 {
        return Err(last_log());
    }
    // the locations are bound by NAME before the link, exactly as the
    // desktop tier binds them — one mechanism, both tiers
    let handle = unsafe {
        let handle = gl_link_program(vs, fs);
        if handle == 0 {
            return Err(last_log());
        }
        handle
    };
    // ...and again after, because a link is what fixes them
    for (index, name) in attribs.iter().enumerate() {
        unsafe { gl_bind_attrib_location(handle, index as u32, name.as_ptr(), name.len()) };
    }
    let relinked = if attribs.is_empty() {
        handle
    } else {
        let again = unsafe { gl_link_program(vs, fs) };
        if again == 0 { return Err(last_log()) } else { again }
    };
    unsafe {
        let frame = "Frame";
        gl_uniform_block(relinked, frame.as_ptr(), frame.len(), UBO_FRAME_BINDING);
        let round = "Round";
        gl_uniform_block(relinked, round.as_ptr(), round.len(), UBO_ROUND_BINDING);
    }
    Ok(relinked)
}

impl Pipelines {
    fn build() -> Result<Pipelines, String> {
        use bunny_ui::gpu::shaders as src;
        let rect = program(
            &format!("{}{}", PRELUDE_300ES, src::RECT_VERT),
            &format!("{}{}{}", PRELUDE_300ES, src::SHARED_FRAG, src::RECT_FRAG_BODY),
            &["a_rect", "a_clip", "a_params", "a_color", "a_color2", "a_point2", "a_radii"],
        )?;
        let sprite = program(
            &format!("{}{}", PRELUDE_300ES, src::SPRITE_VERT),
            &format!("{}{}{}", PRELUDE_300ES, src::SHARED_FRAG, src::SPRITE_FRAG_BODY),
            &["a_dest", "a_tex", "a_clip"],
        )?;
        let glass = program(
            &format!("{}{}", PRELUDE_300ES, src::GLASS_VERT),
            &format!("{}{}{}", PRELUDE_300ES, src::SHARED_FRAG, src::GLASS_FRAG_BODY),
            &[
                "a_rect", "a_clip", "a_radii", "a_lens", "a_finish", "a_touch", "a_tint",
                "a_highlight", "a_spot",
            ],
        )?;
        let blur = program(
            &format!("{}{}", PRELUDE_300ES, src::FULL_VERT),
            &format!("{}{}", PRELUDE_300ES, src::BLUR_FRAG),
            &[],
        )?;
        let blit = program(
            &format!("{}{}", PRELUDE_300ES, src::FULL_VERT),
            &format!("{}{}", PRELUDE_300ES, src::BLIT_FRAG),
            &[],
        )?;
        let sampler = |handle: u32, name: &str| unsafe {
            gl_use_program(handle);
            let slot = gl_uniform_location(handle, name.as_ptr(), name.len());
            if slot != 0 {
                gl_uniform1i(slot, 0);
            }
        };
        sampler(sprite, "atlas");
        sampler(glass, "pyramid");
        sampler(blur, "source");
        sampler(blit, "source");
        let (blur_step, blur_mode) = unsafe {
            gl_use_program(blur);
            let step = "blur_step";
            let mode = "blur_mode";
            (
                gl_uniform_location(blur, step.as_ptr(), step.len()),
                gl_uniform_location(blur, mode.as_ptr(), mode.len()),
            )
        };
        let pipelines = unsafe {
            Pipelines {
                rect,
                sprite,
                glass,
                blur,
                blit,
                blur_step,
                blur_mode,
                vao_rect: gl_create_vertex_array(),
                vao_sprite: gl_create_vertex_array(),
                vao_glass: gl_create_vertex_array(),
                vao_full: gl_create_vertex_array(),
                glass_buffer: gl_create_buffer(),
                glass_cap: 0,
                rects: gl_create_buffer(),
                sprites: gl_create_buffer(),
                frame_ubo: gl_create_buffer(),
                round_ubo: gl_create_buffer(),
                rect_cap: 0,
                sprite_cap: 0,
            }
        };
        unsafe {
            // std140: Frame is one vec2 in a sixteen-byte register,
            // Round is two vec4s
            gl_bind_buffer(GL_UNIFORM_BUFFER, pipelines.frame_ubo);
            gl_buffer_data_size(GL_UNIFORM_BUFFER, 16, GL_STREAM_DRAW);
            gl_bind_buffer(GL_UNIFORM_BUFFER, pipelines.round_ubo);
            gl_buffer_data_size(GL_UNIFORM_BUFFER, 32, GL_STREAM_DRAW);
            gl_bind_buffer_base(GL_UNIFORM_BUFFER, UBO_FRAME_BINDING, pipelines.frame_ubo);
            gl_bind_buffer_base(GL_UNIFORM_BUFFER, UBO_ROUND_BINDING, pipelines.round_ubo);
        }
        Ok(pipelines)
    }
}

/// The instance attributes, re-pointed per run: the run's BASE rides
/// the byte offset, the way the desktop tier carries it, so no shader
/// ever asks which instance it is.
fn rect_attribs(base: usize) {
    let stride = std::mem::size_of::<RectInstance>() as i32;
    for (index, offset, count, kind, normalized) in [
        (0u32, 0usize, 4, GL_FLOAT, 0u32),
        (1, 16, 4, GL_FLOAT, 0),
        (2, 32, 4, GL_FLOAT, 0),
        (3, 48, 4, GL_UNSIGNED_BYTE, 1),
        (4, 52, 4, GL_UNSIGNED_BYTE, 1),
        (5, 56, 2, GL_FLOAT, 0),
        (6, 64, 4, GL_FLOAT, 0),
    ] {
        unsafe {
            gl_enable_vertex_attrib_array(index);
            gl_vertex_attrib_pointer(index, count, kind, normalized, stride, (base + offset) as i32);
            gl_vertex_attrib_divisor(index, 1);
        }
    }
}

fn sprite_attribs(base: usize) {
    let stride = std::mem::size_of::<SpriteInstance>() as i32;
    for (index, offset) in [(0u32, 0usize), (1, 16), (2, 32)] {
        unsafe {
            gl_enable_vertex_attrib_array(index);
            gl_vertex_attrib_pointer(index, 4, GL_FLOAT, 0, stride, (base + offset) as i32);
            gl_vertex_attrib_divisor(index, 1);
        }
    }
}

/// Bytes of a slice of plain-old-data instances, for the upload.
fn instance_bytes<T>(items: &[T]) -> &[u8] {
    // every wire struct is `#[repr(C)]` and holds only floats and bytes
    unsafe {
        std::slice::from_raw_parts(items.as_ptr().cast::<u8>(), std::mem::size_of_val(items))
    }
}

fn upload(target_buffer: u32, cap: &mut u32, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        gl_bind_buffer(GL_ARRAY_BUFFER, target_buffer);
        if bytes.len() as u32 > *cap {
            *cap = bytes.len() as u32;
            gl_buffer_data_size(GL_ARRAY_BUFFER, *cap, GL_STREAM_DRAW);
        } else {
            // orphan: the driver renames the store instead of stalling
            // on the frame still reading it. A blocking fence is
            // illegal here, so renaming IS the synchronisation.
            gl_buffer_data_size(GL_ARRAY_BUFFER, *cap, GL_STREAM_DRAW);
        }
        gl_buffer_sub_data(GL_ARRAY_BUFFER, 0, bytes.as_ptr(), bytes.len());
    }
}

// MARK: - Liquid glass (the scene texture and the blur pyramid)

const GL_SRGB8_ALPHA8: i32 = 0x8C43;
const GL_LINEAR: i32 = 0x2601;
const GL_LINEAR_MIPMAP_LINEAR: i32 = 0x2703;
const GL_TEXTURE_MAX_LEVEL: u32 = 0x813D;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;

/// A pane READS what is under it, so the scene cannot be drawn straight
/// at the drawable: it goes to a texture, the pyramid is blurred out of
/// that texture, and the whole thing is copied back at the end.
struct GlassTargets {
    scene: u32,
    ping: u32,
    pong: u32,
    /// One framebuffer, re-pointed per pass — a framebuffer is a
    /// pointer to an attachment, and re-pointing costs nothing next to
    /// keeping ten alive.
    fbo: u32,
    size: (u32, u32),
}

/// A pyramid texture: four levels of `SRGB8_ALPHA8`, trilinear,
/// clamped. Every level is allocated by hand — the chain is never
/// GENERATED, it is blurred, one pass per level.
///
/// The format is where a browser differs from the desktop. Desktop GL
/// asks for the encode with `GL_FRAMEBUFFER_SRGB`; GLES has no such
/// toggle, because an sRGB attachment encodes on write and decodes on
/// sample by specification, always. The enable/disable pair simply does
/// not exist here, and the chain still averages in linear light.
fn make_pyramid(width: u32, height: u32) -> u32 {
    unsafe {
        let texture = gl_create_texture();
        gl_bind_texture(GL_TEXTURE_2D, texture);
        gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR);
        gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MAX_LEVEL, GLASS_MAX_LEVEL as i32);
        for level in 0..=GLASS_MAX_LEVEL {
            gl_tex_image_2d(
                GL_TEXTURE_2D,
                level as i32,
                GL_SRGB8_ALPHA8,
                (width >> level).max(1) as i32,
                (height >> level).max(1) as i32,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                std::ptr::null(),
                0,
            );
        }
        texture
    }
}

impl GlassTargets {
    /// `None` when anything refuses — a frame then paints without its
    /// panes rather than failing to present at all.
    fn new(size: (u32, u32)) -> Option<GlassTargets> {
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        let half = (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1));
        unsafe {
            let scene = gl_create_texture();
            gl_bind_texture(GL_TEXTURE_2D, scene);
            gl_tex_image_2d(
                GL_TEXTURE_2D, 0, GL_RGBA as i32, size.0 as i32, size.1 as i32,
                GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null(), 0,
            );
            // the scene is SAMPLED by the first blur pass, so it filters
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            let ping = make_pyramid(half.0, half.1);
            let pong = make_pyramid(half.0, half.1);
            let fbo = gl_create_framebuffer();
            gl_bind_framebuffer(GL_FRAMEBUFFER, fbo);
            gl_framebuffer_texture_2d(
                GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, scene, 0,
            );
            if gl_check_framebuffer_status(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
                gl_bind_framebuffer(GL_FRAMEBUFFER, 0);
                return None;
            }
            gl_bind_framebuffer(GL_FRAMEBUFFER, 0);
            Some(GlassTargets { scene, ping, pong, fbo, size })
        }
    }

    fn release(&self) {
        unsafe {
            gl_delete_texture(self.scene);
            gl_delete_texture(self.ping);
            gl_delete_texture(self.pong);
            gl_delete_framebuffer(self.fbo);
        }
    }
}

/// Blurs the pyramid down to `levels`. The downsample is FUSED into the
/// horizontal pass — the destination is half the source, so each of the
/// nine bilinear taps already averages a two-by-two and the reduction
/// rides along free. That is why a heavy blur costs what a light one
/// costs.
fn build_pyramid(pipes: &Pipelines, targets: &GlassTargets, levels: u32) {
    let base = (targets.size.0.div_ceil(2).max(1), targets.size.1.div_ceil(2).max(1));
    unsafe {
        gl_disable(GL_BLEND);
        gl_bind_vertex_array(pipes.vao_full);
        gl_use_program(pipes.blur);
        gl_active_texture(GL_TEXTURE0);
        gl_bind_framebuffer(GL_FRAMEBUFFER, targets.fbo);
        for level in 0..=levels.min(GLASS_MAX_LEVEL) {
            let width = (base.0 >> level).max(1);
            let height = (base.1 >> level).max(1);
            let inv = (1.0 / width as f32, 1.0 / height as f32);
            // level zero reads RAW scene colour, which no format
            // decoded for us — every level above reads an sRGB texture
            // and the sampler has already decoded it
            let (source, source_level, decode) = match level {
                0 => (targets.scene, 0.0f32, 1.0f32),
                _ => (targets.ping, (level - 1) as f32, 0.0),
            };
            for (pass, direction) in
                [(targets.pong, (1.0f32, 0.0f32)), (targets.ping, (0.0f32, 1.0f32))]
            {
                let (from, from_level, decoding) = if pass == targets.pong {
                    (source, source_level, decode)
                } else {
                    (targets.pong, level as f32, 0.0)
                };
                gl_framebuffer_texture_2d(
                    GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, pass, level as i32,
                );
                gl_viewport(0, 0, width as i32, height as i32);
                gl_bind_texture(GL_TEXTURE_2D, from);
                gl_uniform4f(pipes.blur_step, inv.0, inv.1, direction.0, direction.1);
                gl_uniform4f(pipes.blur_mode, from_level, decoding, 0.0, 0.0);
                gl_draw_arrays(GL_TRIANGLES, 0, 3);
            }
        }
        gl_enable(GL_BLEND);
    }
}

/// Copies the scene texture onto the real target, texel for texel. The
/// drawable is written, never read, so the frame lives in a texture
/// until this last step.
fn blit_scene(pipes: &Pipelines, targets: &GlassTargets, physical: (u32, u32)) {
    unsafe {
        gl_bind_framebuffer(GL_FRAMEBUFFER, 0);
        gl_viewport(0, 0, physical.0 as i32, physical.1 as i32);
        gl_disable(GL_BLEND);
        gl_use_program(pipes.blit);
        gl_bind_vertex_array(pipes.vao_full);
        gl_active_texture(GL_TEXTURE0);
        gl_bind_texture(GL_TEXTURE_2D, targets.scene);
        gl_draw_arrays(GL_TRIANGLES, 0, 3);
        // the scene must not still be BOUND when the next frame
        // attaches it as a target: desktop GL shrugs at that, a browser
        // calls it a feedback loop and drops the draw
        gl_bind_texture(GL_TEXTURE_2D, 0);
        gl_enable(GL_BLEND);
    }
}

fn glass_attribs(base: usize) {
    let stride = std::mem::size_of::<GlassInstance>() as i32;
    for (index, offset, count, kind, normalized) in [
        (0u32, 0usize, 4, GL_FLOAT, 0u32),
        (1, 16, 4, GL_FLOAT, 0),
        (2, 32, 4, GL_FLOAT, 0),
        (3, 48, 4, GL_FLOAT, 0),
        (4, 64, 4, GL_FLOAT, 0),
        (5, 80, 4, GL_FLOAT, 0),
        (6, 96, 4, GL_UNSIGNED_BYTE, 1),
        (7, 100, 4, GL_UNSIGNED_BYTE, 1),
        // the spot's alpha and the pad, read as a vec2 of floats
        (8, 104, 2, GL_FLOAT, 0),
    ] {
        unsafe {
            gl_enable_vertex_attrib_array(index);
            gl_vertex_attrib_pointer(index, count, kind, normalized, stride, (base + offset) as i32);
            gl_vertex_attrib_divisor(index, 1);
        }
    }
}

// MARK: - The ground (the one seam the walk asks a tier to fill)

#[derive(Default)]
struct WebGround {
    shared: Option<u32>,
    dedicated: std::collections::HashMap<u64, u32>,
    next: u64,
}

impl AtlasGround for WebGround {
    fn ensure_shared(&mut self, size: u32) -> bool {
        if self.shared.is_some() {
            return true;
        }
        let texture = unsafe {
            let texture = gl_create_texture();
            gl_bind_texture(GL_TEXTURE_2D, texture);
            gl_tex_image_2d(
                GL_TEXTURE_2D, 0, GL_RGBA as i32, size as i32, size as i32,
                GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null(), 0,
            );
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            texture
        };
        self.shared = Some(texture);
        true
    }

    fn upload_shared(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: &[u8], pitch_px: u32) {
        let Some(texture) = self.shared else { return };
        unsafe {
            gl_bind_texture(GL_TEXTURE_2D, texture);
            gl_pixel_storei(GL_UNPACK_ROW_LENGTH, pitch_px as i32);
            gl_tex_sub_image_2d(
                GL_TEXTURE_2D, 0, x as i32, y as i32, w as i32, h as i32,
                GL_RGBA, GL_UNSIGNED_BYTE, bytes.as_ptr(), bytes.len(),
            );
            gl_pixel_storei(GL_UNPACK_ROW_LENGTH, 0);
        }
    }

    fn drop_shared(&mut self) {
        if let Some(texture) = self.shared.take() {
            unsafe { gl_delete_texture(texture) };
        }
    }

    fn make_dedicated(&mut self, w: u32, h: u32, bytes: &[u8], pitch_px: u32) -> Option<u64> {
        let texture = unsafe {
            let texture = gl_create_texture();
            gl_bind_texture(GL_TEXTURE_2D, texture);
            gl_pixel_storei(GL_UNPACK_ROW_LENGTH, pitch_px as i32);
            gl_tex_image_2d(
                GL_TEXTURE_2D, 0, GL_RGBA as i32, w as i32, h as i32,
                GL_RGBA, GL_UNSIGNED_BYTE, bytes.as_ptr(), bytes.len(),
            );
            gl_pixel_storei(GL_UNPACK_ROW_LENGTH, 0);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            gl_tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            texture
        };
        self.next += 1;
        self.dedicated.insert(self.next, texture);
        Some(self.next)
    }

    fn drop_dedicated(&mut self, id: u64) {
        if let Some(texture) = self.dedicated.remove(&id) {
            unsafe { gl_delete_texture(texture) };
        }
    }
}

/// A window never switches roads mid-flight, so the tier is chosen once
/// and this is where the answer lives.
struct Tier {
    /// The device's texture ceiling — the atlas may not grow past it.
    max_texture: u32,
    /// The physical size the drawable currently holds.
    physical: (u32, u32),
    pipelines: Pipelines,
    ground: WebGround,
    atlas: RunAtlas,
    batches: FrameBatches,
    /// Built the first time a pane asks, and rebuilt when the window
    /// changes size.
    glass: Option<GlassTargets>,
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
    let pipelines = match Pipelines::build() {
        Ok(pipelines) => pipelines,
        Err(log) => {
            // a shader the device refuses is not a crash: the page has
            // a rasterizer, and it says so once on the way down
            say(&format!("bunny gl: {}", log.trim()));
            unsafe { gl_teardown() };
            return false;
        }
    };
    TIER.with(|slot| {
        *slot.borrow_mut() = Some(Tier {
            max_texture,
            physical,
            pipelines,
            ground: WebGround::default(),
            atlas: RunAtlas::new(),
            batches: FrameBatches::default(),
            glass: None,
        })
    });
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
pub(crate) fn present_window(
    island: Option<(u32, (u32, u32))>,
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    let physical = island.map(|(_, box4)| box4).unwrap_or((
        ((size.width.round() as usize) * scale).max(1) as u32,
        ((size.height.round() as usize) * scale).max(1) as u32,
    ));
    TIER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(tier) = slot.as_mut() else { return };
        if tier.physical != physical {
            tier.physical = physical;
            unsafe { gl_resize(physical.0, physical.1) };
        }
        // the walk, and the copying collector behind it: a full atlas
        // drains, resets (growing once to the cap the device allows)
        // and walks the frame again
        let target = (physical.0 as usize, physical.1 as usize);
        let mut walked = false;
        for attempt in 0..3 {
            let Tier { ground, atlas, batches, .. } = tier;
            match build_frame(ground, display, scale, target, text, images, atlas, batches) {
                Ok(()) => {
                    walked = true;
                    break;
                }
                Err(_) => {
                    let grow = attempt == 0 && tier.max_texture >= 4096;
                    let Tier { ground, atlas, .. } = tier;
                    atlas.reset(ground, grow);
                }
            }
        }
        if !walked {
            say("bunny gl: the atlas overflowed twice - the frame is short");
            return;
        }

        // a pane READS what is under it, so a frame that holds one
        // cannot draw at the drawable: the drawable is write-only. It
        // goes to a texture and comes back at the end.
        let wants_glass = tier.batches.runs.iter().any(|run| run.kind == RunKind::Glass);
        if wants_glass {
            let stale = tier.glass.as_ref().is_none_or(|targets| targets.size != physical);
            if stale {
                if let Some(old) = tier.glass.take() {
                    old.release();
                }
                tier.glass = GlassTargets::new(physical);
            }
        }
        let scene_fbo = match (wants_glass, tier.glass.as_ref()) {
            (true, Some(targets)) => targets.fbo,
            // no pane, or the targets refused: straight at the drawable,
            // and a refused pane paints without its blur rather than
            // failing to present
            _ => 0,
        };
        unsafe {
            gl_bind_framebuffer(GL_FRAMEBUFFER, scene_fbo);
            gl_viewport(0, 0, physical.0 as i32, physical.1 as i32);
            // the canvas colour goes on STRAIGHT, the way the
            // rasterizer fills its bitmap with it — nothing has
            // blended yet
            gl_clear_color(
                canvas.r as f32 / 255.0,
                canvas.g as f32 / 255.0,
                canvas.b as f32 / 255.0,
                canvas.a as f32 / 255.0,
            );
            gl_clear(GL_COLOR_BUFFER_BIT);
        }

        let viewport = [physical.0 as f32, physical.1 as f32, 0.0, 0.0];
        unsafe {
            gl_bind_buffer(GL_UNIFORM_BUFFER, tier.pipelines.frame_ubo);
            gl_buffer_sub_data(
                GL_UNIFORM_BUFFER,
                0,
                viewport.as_ptr().cast::<u8>(),
                std::mem::size_of_val(&viewport),
            );
        }
        upload(
            tier.pipelines.rects,
            &mut tier.pipelines.rect_cap,
            instance_bytes(&tier.batches.rects),
        );
        upload(
            tier.pipelines.sprites,
            &mut tier.pipelines.sprite_cap,
            instance_bytes(&tier.batches.sprites),
        );
        upload(
            tier.pipelines.glass_buffer,
            &mut tier.pipelines.glass_cap,
            instance_bytes(&tier.batches.glass),
        );

        // the runs, in paint order — the display list IS the order, and
        // nothing here sorts or batches across it
        let mut round = u32::MAX;
        let mut program = 0u32;
        for run in &tier.batches.runs {
            if run.round != round {
                round = run.round;
                let curve = tier.batches.rounds[round as usize];
                let block = [
                    curve.box4[0], curve.box4[1], curve.box4[2], curve.box4[3],
                    curve.radii[0], curve.radii[1], curve.radii[2], curve.radii[3],
                ];
                unsafe {
                    gl_bind_buffer(GL_UNIFORM_BUFFER, tier.pipelines.round_ubo);
                    gl_buffer_sub_data(
                        GL_UNIFORM_BUFFER,
                        0,
                        block.as_ptr().cast::<u8>(),
                        std::mem::size_of_val(&block),
                    );
                }
            }
            match run.kind {
                RunKind::Rects => {
                    if program != tier.pipelines.rect {
                        program = tier.pipelines.rect;
                        unsafe {
                            gl_use_program(program);
                            gl_bind_vertex_array(tier.pipelines.vao_rect);
                            gl_bind_buffer(GL_ARRAY_BUFFER, tier.pipelines.rects);
                        }
                    }
                    rect_attribs(run.base as usize * std::mem::size_of::<RectInstance>());
                }
                RunKind::Sprites | RunKind::Texture(_) => {
                    if program != tier.pipelines.sprite {
                        program = tier.pipelines.sprite;
                        unsafe {
                            gl_use_program(program);
                            gl_bind_vertex_array(tier.pipelines.vao_sprite);
                            gl_bind_buffer(GL_ARRAY_BUFFER, tier.pipelines.sprites);
                        }
                    }
                    let texture = match run.kind {
                        RunKind::Texture(index) => tier
                            .batches
                            .textures
                            .get(index as usize)
                            .and_then(|handle| tier.ground.dedicated.get(handle).copied())
                            .unwrap_or(0),
                        _ => tier.ground.shared.unwrap_or(0),
                    };
                    unsafe {
                        gl_active_texture(GL_TEXTURE0);
                        gl_bind_texture(GL_TEXTURE_2D, texture);
                    }
                    sprite_attribs(run.base as usize * std::mem::size_of::<SpriteInstance>());
                }
                RunKind::Glass => {
                    let Some(targets) = tier.glass.as_ref() else { continue };
                    // the pyramid is blurred out of the scene AS DRAWN
                    // SO FAR — everything below this pane, and nothing
                    // above it
                    build_pyramid(&tier.pipelines, targets, run.levels);
                    program = tier.pipelines.glass;
                    unsafe {
                        // the pyramid re-pointed the shared framebuffer
                        // at its own levels; the scene has to be put
                        // back before the pane draws into it
                        gl_bind_framebuffer(GL_FRAMEBUFFER, targets.fbo);
                        gl_framebuffer_texture_2d(
                            GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, targets.scene, 0,
                        );
                        gl_viewport(0, 0, physical.0 as i32, physical.1 as i32);
                        gl_use_program(program);
                        gl_bind_vertex_array(tier.pipelines.vao_glass);
                        gl_bind_buffer(GL_ARRAY_BUFFER, tier.pipelines.glass_buffer);
                        gl_active_texture(GL_TEXTURE0);
                        gl_bind_texture(GL_TEXTURE_2D, targets.ping);
                    }
                    glass_attribs(run.base as usize * std::mem::size_of::<GlassInstance>());
                }
            }
            unsafe { gl_draw_arrays_instanced(GL_TRIANGLES, 0, 6, run.count as i32) };
        }
        if let (true, Some(targets)) = (wants_glass, tier.glass.as_ref()) {
            blit_scene(&tier.pipelines, targets, physical);
        }
        if let Some((id, _)) = island {
            // the shared surface holds this island's pixels: hand them
            // to its own element before the next island overwrites them
            unsafe { gl_island_blit(id, physical.0, physical.1) };
        }
        unsafe { gl_flush() };
    });
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
