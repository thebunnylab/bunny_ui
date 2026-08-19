//! EGL/OpenGL presentation — the SAME display list, presented by the GPU.
//!
//! This module is the Linux twin of the windows shell's d3d module: the
//! display list does not change, the pixels must not change (within the
//! anti-aliasing tolerance the parity tests pin down). The CPU raster
//! stays as the oracle, the headless path and the fallback — this
//! backend exists because a full-window repaint must cost less than a
//! millisecond at ANY window size.
//!
//! House rules apply: no dependencies. EGL comes in through `dlopen` of
//! the system's own `libEGL.so.1` (the d3dcompiler discipline: resolved
//! at runtime, absence prints one line and the CPU presents), and every
//! GL 3.3 symbol resolves through `eglGetProcAddress` — a documented
//! Mesa premise (it answers core names, not just extensions; any NULL
//! is a refusal). The window's EGL surface wraps the wayland surface
//! through `libwayland-egl.so.1`, dlopened the same way. Zero build
//! steps, zero new link-time dependencies.
//!
//! The GPU is the DEFAULT presentation of a window; `BUNNY_PRESENT=cpu`
//! forces the CPU raster, and any failure to come up falls back to it
//! with one line on stderr. The choice happens ONCE, at window creation.
//! One documented exception to never-switch-mid-flight: a reset context
//! (GPU hang recovery) — the shell recreates the whole stack ONCE in
//! silence, and lost again the window presents by CPU for its life.
//!
//! The LAW of the port: every policy decision — snapping, radius
//! clamps, stroke thickness, shadow reach, the clip stack — is resolved
//! on the CPU in f64, operation by operation the way raster.rs resolves
//! it. The instances carry snapped device pixels in f32 (integers,
//! exact) and the shaders are pure coverage evaluators, blind to DPI.
//!
//! Premises (documented, not checked): a NON-sRGB config forever — the
//! CPU raster blends in gamma space, and an sRGB framebuffer would
//! linearize the blending and break parity. `eglSwapInterval(0)` is
//! LOAD-BEARING: Mesa otherwise blocks inside `eglSwapBuffers` on its
//! own internal frame callback — double-throttled while visible and
//! DEADLOCKED when occluded (callbacks stop entirely); pacing is 100%
//! the shell's own `wl_surface.frame` road. And no bare commit ever
//! follows a present: the swap IS the commit.
//!
//! Two traps of the windows port dissolve here, for the better: the
//! per-run base index rides the vertex-attrib-pointer byte offset (GL's
//! instancing reads wherever the pointer says — no per-draw constant
//! write), and the fractional-DPI blit pass does not exist (the wayland
//! buffer is always exactly the physical raster; the compositor scales
//! by `buffer_scale`). One trap is new: `gl_FragCoord` counts from the
//! BOTTOM-left — the fragment shaders flip it back to the raster's
//! top-left space, and the offscreen readback flips its rows.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_uint, c_void, CString};

use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;

use crate::walk::{
    build_frame, AtlasFull, AtlasGround, DrawRun, FrameBatches, RectInstance, RoundClip,
    RunAtlas, RunKind, SpriteInstance,
};

// MARK: - FFI border (dlopen the door, eglGetProcAddress the hallway)

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;

// EGL vocabulary — values from the Khronos registry, stable ABI.
type EglDisplay = *mut c_void;
type EglConfig = *mut c_void;
type EglContext = *mut c_void;
type EglSurface = *mut c_void;
type EglBool = c_uint;

const EGL_PLATFORM_WAYLAND: u32 = 0x31D8;
const EGL_PLATFORM_XCB_EXT: u32 = 0x31DC;
const EGL_PLATFORM_SURFACELESS_MESA: u32 = 0x31DD;
const EGL_OPENGL_API: u32 = 0x30A2;
const EGL_NO_CONTEXT: EglContext = std::ptr::null_mut();
const EGL_NO_SURFACE: EglSurface = std::ptr::null_mut();
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_WINDOW_BIT: i32 = 0x0004;
const EGL_PBUFFER_BIT: i32 = 0x0001;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_BIT: i32 = 0x0008;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_ALPHA_SIZE: i32 = 0x3021;
const EGL_DEPTH_SIZE: i32 = 0x3025;
const EGL_STENCIL_SIZE: i32 = 0x3026;
const EGL_NONE: i32 = 0x3038;
const EGL_WIDTH: i32 = 0x3057;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_CONTEXT_MAJOR_VERSION: i32 = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: i32 = 0x30FB;
const EGL_CONTEXT_OPENGL_PROFILE_MASK: i32 = 0x30FD;
const EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT: i32 = 0x0001;
const EGL_CONTEXT_LOST: u32 = 0x300E;

struct EglFns {
    get_platform_display:
        unsafe extern "C" fn(u32, *mut c_void, *const isize) -> EglDisplay,
    initialize: unsafe extern "C" fn(EglDisplay, *mut i32, *mut i32) -> EglBool,
    bind_api: unsafe extern "C" fn(u32) -> EglBool,
    choose_config:
        unsafe extern "C" fn(EglDisplay, *const i32, *mut EglConfig, i32, *mut i32) -> EglBool,
    create_context:
        unsafe extern "C" fn(EglDisplay, EglConfig, EglContext, *const i32) -> EglContext,
    create_window_surface:
        unsafe extern "C" fn(EglDisplay, EglConfig, *mut c_void, *const i32) -> EglSurface,
    /// The EGL 1.5 platform road: the native window is a POINTER to
    /// the platform's window type (for xcb, `*mut xcb_window_t`) —
    /// the classic pass-the-xid-by-value trap dissolves here.
    create_platform_window_surface:
        unsafe extern "C" fn(EglDisplay, EglConfig, *mut c_void, *const isize) -> EglSurface,
    create_pbuffer_surface:
        unsafe extern "C" fn(EglDisplay, EglConfig, *const i32) -> EglSurface,
    make_current:
        unsafe extern "C" fn(EglDisplay, EglSurface, EglSurface, EglContext) -> EglBool,
    swap_buffers: unsafe extern "C" fn(EglDisplay, EglSurface) -> EglBool,
    swap_interval: unsafe extern "C" fn(EglDisplay, i32) -> EglBool,
    destroy_surface: unsafe extern "C" fn(EglDisplay, EglSurface) -> EglBool,
    destroy_context: unsafe extern "C" fn(EglDisplay, EglContext) -> EglBool,
    get_error: unsafe extern "C" fn() -> u32,
    get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
}

struct WlEglFns {
    window_create: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *mut c_void,
    window_resize: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int),
    window_destroy: unsafe extern "C" fn(*mut c_void),
}

/// Both doors resolved once — a system without them presents by CPU.
struct Loader {
    egl: EglFns,
    wl_egl: WlEglFns,
}

fn resolve(handle: *mut c_void, name: &CString) -> Option<*mut c_void> {
    let symbol = unsafe { dlsym(handle, name.as_ptr()) };
    (!symbol.is_null()).then_some(symbol)
}

fn loader() -> Option<&'static Loader> {
    static LOADER: std::sync::OnceLock<Option<Loader>> = std::sync::OnceLock::new();
    LOADER
        .get_or_init(|| {
            let egl_lib = unsafe { dlopen(c"libEGL.so.1".as_ptr(), RTLD_NOW) };
            let wl_lib = unsafe { dlopen(c"libwayland-egl.so.1".as_ptr(), RTLD_NOW) };
            if egl_lib.is_null() || wl_lib.is_null() {
                return None;
            }
            // one macro-free table: any missing symbol refuses the road
            let sym = |name: &str| resolve(egl_lib, &CString::new(name).expect("egl name"));
            let wl_sym = |name: &str| resolve(wl_lib, &CString::new(name).expect("wl name"));
            unsafe {
                Some(Loader {
                    egl: EglFns {
                        get_platform_display: std::mem::transmute(sym("eglGetPlatformDisplay")?),
                        initialize: std::mem::transmute(sym("eglInitialize")?),
                        bind_api: std::mem::transmute(sym("eglBindAPI")?),
                        choose_config: std::mem::transmute(sym("eglChooseConfig")?),
                        create_context: std::mem::transmute(sym("eglCreateContext")?),
                        create_window_surface: std::mem::transmute(sym("eglCreateWindowSurface")?),
                        create_platform_window_surface: std::mem::transmute(sym(
                            "eglCreatePlatformWindowSurface",
                        )?),
                        create_pbuffer_surface: std::mem::transmute(sym(
                            "eglCreatePbufferSurface",
                        )?),
                        make_current: std::mem::transmute(sym("eglMakeCurrent")?),
                        swap_buffers: std::mem::transmute(sym("eglSwapBuffers")?),
                        swap_interval: std::mem::transmute(sym("eglSwapInterval")?),
                        destroy_surface: std::mem::transmute(sym("eglDestroySurface")?),
                        destroy_context: std::mem::transmute(sym("eglDestroyContext")?),
                        get_error: std::mem::transmute(sym("eglGetError")?),
                        get_proc_address: std::mem::transmute(sym("eglGetProcAddress")?),
                    },
                    wl_egl: WlEglFns {
                        window_create: std::mem::transmute(wl_sym("wl_egl_window_create")?),
                        window_resize: std::mem::transmute(wl_sym("wl_egl_window_resize")?),
                        window_destroy: std::mem::transmute(wl_sym("wl_egl_window_destroy")?),
                    },
                })
            }
        })
        .as_ref()
}

// GL vocabulary — values from the Khronos registry, stable ABI.
type GlEnum = c_uint;
type GlSync = *mut c_void;

const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_BLEND: GlEnum = 0x0BE2;
const GL_SCISSOR_TEST: GlEnum = 0x0C11;
const GL_DEPTH_TEST: GlEnum = 0x0B71;
const GL_CULL_FACE: GlEnum = 0x0B44;
const GL_SRC_ALPHA: GlEnum = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: GlEnum = 0x0303;
const GL_ONE: GlEnum = 1;
const GL_ZERO: GlEnum = 0;
const GL_TRIANGLES: GlEnum = 0x0004;
const GL_ARRAY_BUFFER: GlEnum = 0x8892;
const GL_UNIFORM_BUFFER: GlEnum = 0x8A11;
const GL_STREAM_DRAW: GlEnum = 0x88E0;
const GL_FLOAT: GlEnum = 0x1406;
const GL_UNSIGNED_BYTE: GlEnum = 0x1401;
const GL_TEXTURE_2D: GlEnum = 0x0DE1;
const GL_TEXTURE0: GlEnum = 0x84C0;
const GL_RGBA8: GlEnum = 0x8058;
const GL_RGBA: GlEnum = 0x1908;
const GL_TEXTURE_MIN_FILTER: GlEnum = 0x2801;
const GL_TEXTURE_MAG_FILTER: GlEnum = 0x2800;
const GL_TEXTURE_WRAP_S: GlEnum = 0x2802;
const GL_TEXTURE_WRAP_T: GlEnum = 0x2803;
const GL_NEAREST: i32 = 0x2600;
const GL_CLAMP_TO_EDGE: i32 = 0x812F;
const GL_UNPACK_ALIGNMENT: GlEnum = 0x0CF5;
const GL_UNPACK_ROW_LENGTH: GlEnum = 0x0CF2;
const GL_PACK_ALIGNMENT: GlEnum = 0x0D05;
const GL_VERTEX_SHADER: GlEnum = 0x8B31;
const GL_FRAGMENT_SHADER: GlEnum = 0x8B30;
const GL_COMPILE_STATUS: GlEnum = 0x8B81;
const GL_LINK_STATUS: GlEnum = 0x8B82;
const GL_FRAMEBUFFER: GlEnum = 0x8D40;
const GL_COLOR_ATTACHMENT0: GlEnum = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: GlEnum = 0x8CD5;
const GL_MAX_TEXTURE_SIZE: GlEnum = 0x0D33;
const GL_SYNC_GPU_COMMANDS_COMPLETE: GlEnum = 0x9117;
const GL_SYNC_FLUSH_COMMANDS_BIT: u32 = 0x0001;
const GL_ALREADY_SIGNALED: GlEnum = 0x911A;
const GL_CONDITION_SATISFIED: GlEnum = 0x911C;

/// The GL 3.3 core surface this module speaks, resolved through
/// `eglGetProcAddress` after the display initializes. One struct, one
/// resolve, every pointer checked — a missing symbol refuses the road.
struct GlFns {
    get_error: unsafe extern "C" fn() -> GlEnum,
    get_integerv: unsafe extern "C" fn(GlEnum, *mut i32),
    enable: unsafe extern "C" fn(GlEnum),
    disable: unsafe extern "C" fn(GlEnum),
    viewport: unsafe extern "C" fn(i32, i32, i32, i32),
    clear_color: unsafe extern "C" fn(f32, f32, f32, f32),
    clear: unsafe extern "C" fn(u32),
    blend_func_separate: unsafe extern "C" fn(GlEnum, GlEnum, GlEnum, GlEnum),
    gen_buffers: unsafe extern "C" fn(i32, *mut u32),
    bind_buffer: unsafe extern "C" fn(GlEnum, u32),
    buffer_data: unsafe extern "C" fn(GlEnum, isize, *const c_void, GlEnum),
    buffer_sub_data: unsafe extern "C" fn(GlEnum, isize, isize, *const c_void),
    gen_vertex_arrays: unsafe extern "C" fn(i32, *mut u32),
    bind_vertex_array: unsafe extern "C" fn(u32),
    enable_vertex_attrib_array: unsafe extern "C" fn(u32),
    vertex_attrib_pointer:
        unsafe extern "C" fn(u32, i32, GlEnum, u8, i32, *const c_void),
    vertex_attrib_divisor: unsafe extern "C" fn(u32, u32),
    draw_arrays: unsafe extern "C" fn(GlEnum, i32, i32),
    draw_arrays_instanced: unsafe extern "C" fn(GlEnum, i32, i32, i32),
    gen_textures: unsafe extern "C" fn(i32, *mut u32),
    bind_texture: unsafe extern "C" fn(GlEnum, u32),
    active_texture: unsafe extern "C" fn(GlEnum),
    tex_parameteri: unsafe extern "C" fn(GlEnum, GlEnum, i32),
    tex_image_2d: unsafe extern "C" fn(
        GlEnum,
        i32,
        i32,
        i32,
        i32,
        i32,
        GlEnum,
        GlEnum,
        *const c_void,
    ),
    tex_sub_image_2d: unsafe extern "C" fn(
        GlEnum,
        i32,
        i32,
        i32,
        i32,
        i32,
        GlEnum,
        GlEnum,
        *const c_void,
    ),
    delete_textures: unsafe extern "C" fn(i32, *const u32),
    pixel_storei: unsafe extern "C" fn(GlEnum, i32),
    create_shader: unsafe extern "C" fn(GlEnum) -> u32,
    shader_source:
        unsafe extern "C" fn(u32, i32, *const *const c_char, *const i32),
    compile_shader: unsafe extern "C" fn(u32),
    get_shaderiv: unsafe extern "C" fn(u32, GlEnum, *mut i32),
    get_shader_info_log: unsafe extern "C" fn(u32, i32, *mut i32, *mut c_char),
    create_program: unsafe extern "C" fn() -> u32,
    attach_shader: unsafe extern "C" fn(u32, u32),
    bind_attrib_location: unsafe extern "C" fn(u32, u32, *const c_char),
    link_program: unsafe extern "C" fn(u32),
    get_programiv: unsafe extern "C" fn(u32, GlEnum, *mut i32),
    get_program_info_log: unsafe extern "C" fn(u32, i32, *mut i32, *mut c_char),
    use_program: unsafe extern "C" fn(u32),
    delete_shader: unsafe extern "C" fn(u32),
    delete_program: unsafe extern "C" fn(u32),
    get_uniform_location: unsafe extern "C" fn(u32, *const c_char) -> i32,
    uniform1i: unsafe extern "C" fn(i32, i32),
    uniform4f: unsafe extern "C" fn(i32, f32, f32, f32, f32),
    get_uniform_block_index: unsafe extern "C" fn(u32, *const c_char) -> u32,
    uniform_block_binding: unsafe extern "C" fn(u32, u32, u32),
    bind_buffer_base: unsafe extern "C" fn(GlEnum, u32, u32),
    fence_sync: unsafe extern "C" fn(GlEnum, u32) -> GlSync,
    client_wait_sync: unsafe extern "C" fn(GlSync, u32, u64) -> GlEnum,
    delete_sync: unsafe extern "C" fn(GlSync),
    flush: unsafe extern "C" fn(),
    gen_framebuffers: unsafe extern "C" fn(i32, *mut u32),
    bind_framebuffer: unsafe extern "C" fn(GlEnum, u32),
    framebuffer_texture_2d: unsafe extern "C" fn(GlEnum, GlEnum, GlEnum, u32, i32),
    check_framebuffer_status: unsafe extern "C" fn(GlEnum) -> GlEnum,
    delete_framebuffers: unsafe extern "C" fn(i32, *const u32),
    read_pixels:
        unsafe extern "C" fn(i32, i32, i32, i32, GlEnum, GlEnum, *mut c_void),
}

/// Resolves the whole GL table through `eglGetProcAddress`. Mesa
/// answers core names (the documented premise); any NULL refuses.
fn resolve_gl(egl: &EglFns) -> Option<GlFns> {
    let sym = |name: &str| -> Option<*mut c_void> {
        let name = CString::new(name).expect("gl name");
        let address = unsafe { (egl.get_proc_address)(name.as_ptr()) };
        (!address.is_null()).then_some(address)
    };
    macro_rules! gl {
        ($name:literal) => {
            unsafe { std::mem::transmute(sym($name)?) }
        };
    }
    Some(GlFns {
        get_error: gl!("glGetError"),
        get_integerv: gl!("glGetIntegerv"),
        enable: gl!("glEnable"),
        disable: gl!("glDisable"),
        viewport: gl!("glViewport"),
        clear_color: gl!("glClearColor"),
        clear: gl!("glClear"),
        blend_func_separate: gl!("glBlendFuncSeparate"),
        gen_buffers: gl!("glGenBuffers"),
        bind_buffer: gl!("glBindBuffer"),
        buffer_data: gl!("glBufferData"),
        buffer_sub_data: gl!("glBufferSubData"),
        gen_vertex_arrays: gl!("glGenVertexArrays"),
        bind_vertex_array: gl!("glBindVertexArray"),
        enable_vertex_attrib_array: gl!("glEnableVertexAttribArray"),
        vertex_attrib_pointer: gl!("glVertexAttribPointer"),
        vertex_attrib_divisor: gl!("glVertexAttribDivisor"),
        draw_arrays: gl!("glDrawArrays"),
        draw_arrays_instanced: gl!("glDrawArraysInstanced"),
        gen_textures: gl!("glGenTextures"),
        bind_texture: gl!("glBindTexture"),
        active_texture: gl!("glActiveTexture"),
        tex_parameteri: gl!("glTexParameteri"),
        tex_image_2d: gl!("glTexImage2D"),
        tex_sub_image_2d: gl!("glTexSubImage2D"),
        delete_textures: gl!("glDeleteTextures"),
        pixel_storei: gl!("glPixelStorei"),
        create_shader: gl!("glCreateShader"),
        shader_source: gl!("glShaderSource"),
        compile_shader: gl!("glCompileShader"),
        get_shaderiv: gl!("glGetShaderiv"),
        get_shader_info_log: gl!("glGetShaderInfoLog"),
        create_program: gl!("glCreateProgram"),
        attach_shader: gl!("glAttachShader"),
        bind_attrib_location: gl!("glBindAttribLocation"),
        link_program: gl!("glLinkProgram"),
        get_programiv: gl!("glGetProgramiv"),
        get_program_info_log: gl!("glGetProgramInfoLog"),
        use_program: gl!("glUseProgram"),
        delete_shader: gl!("glDeleteShader"),
        delete_program: gl!("glDeleteProgram"),
        get_uniform_location: gl!("glGetUniformLocation"),
        uniform1i: gl!("glUniform1i"),
        uniform4f: gl!("glUniform4f"),
        get_uniform_block_index: gl!("glGetUniformBlockIndex"),
        uniform_block_binding: gl!("glUniformBlockBinding"),
        bind_buffer_base: gl!("glBindBufferBase"),
        fence_sync: gl!("glFenceSync"),
        client_wait_sync: gl!("glClientWaitSync"),
        delete_sync: gl!("glDeleteSync"),
        flush: gl!("glFlush"),
        gen_framebuffers: gl!("glGenFramebuffers"),
        bind_framebuffer: gl!("glBindFramebuffer"),
        framebuffer_texture_2d: gl!("glFramebufferTexture2D"),
        check_framebuffer_status: gl!("glCheckFramebufferStatus"),
        delete_framebuffers: gl!("glDeleteFramebuffers"),
        read_pixels: gl!("glReadPixels"),
    })
}

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

const SHADER_PRELUDE: &str = r#"#version 330 core
"#;

const SHARED_FRAG: &str = r#"
layout(std140) uniform Frame {
    vec2 viewport;
};
layout(std140) uniform Round {
    vec4 round_box;
    float round_radius;
};

float rect_sdf(vec2 p, vec4 rect, float radius) {
    vec2 shifted = max(rect.xy + radius - p, p - (rect.zw - radius));
    float outside = length(max(shifted, vec2(0.0)));
    float inside = min(max(shifted.x, shifted.y), 0.0);
    return outside + inside - radius;
}

float rect_cov(vec2 p, vec4 rect, float radius) {
    return clamp(0.5 - rect_sdf(p, rect, radius), 0.0, 1.0);
}

// the curve that softens the run's clip. radius 0 is the straight
// rectangle the quad clamp already cut — and multiplying by 1.0 is
// exact, so a scene without a rounded clip leaves both shaders
// untouched, bit for bit
float clip_cov(vec2 p) {
    return round_radius > 0.0 ? rect_cov(p, round_box, round_radius) : 1.0;
}

// gl_FragCoord counts from the bottom-left; the raster counts from the
// top — flip once, the +0.5 pixel center survives the mirror
vec2 raster_p() {
    return vec2(gl_FragCoord.x, viewport.y - gl_FragCoord.y);
}
"#;

const RECT_VERT: &str = r#"
layout(std140) uniform Frame {
    vec2 viewport;
};

in vec4 a_rect;
in vec4 a_clip;
in vec4 a_params;
in vec4 a_color;
in vec4 a_color2;
in vec2 a_point2;

flat out vec4 v_rect;
flat out vec4 v_params;
flat out vec4 v_color;
flat out vec4 v_color2;
flat out vec2 v_point2;

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
}
"#;

const RECT_FRAG_BODY: &str = r#"
flat in vec4 v_rect;
flat in vec4 v_params;
flat in vec4 v_color;
flat in vec4 v_color2;
flat in vec2 v_point2;
out vec4 out_color;

void main() {
    vec2 p = raster_p();
    float kind = v_params.z;
    float coverage;
    if (kind == 0.0) {
        // fill: the cpu corner ramp, clamp(radius - d + 0.5, 0, 1)
        coverage = rect_cov(p, v_rect, v_params.x);
    } else if (kind == 1.0) {
        // stroke: outer coverage minus the inner rect's — the inset
        // keeps the same corner center as the cpu ring, and integer
        // edges keep the straight bars exact and never double-blended
        float thickness = v_params.y;
        vec4 inner = vec4(v_rect.xy + thickness, v_rect.zw - thickness);
        float inner_radius = max(v_params.x - thickness, 0.0);
        coverage = clamp(
            rect_cov(p, v_rect, v_params.x) - rect_cov(p, inner, inner_radius),
            0.0, 1.0);
    } else if (kind == 2.0) {
        // shadow: quadratic falloff outside the rounded core — the quad
        // arrives pre-expanded, params.w undoes the expansion
        float expansion = v_params.w;
        vec4 base = vec4(v_rect.xy + expansion, v_rect.zw - expansion);
        float corner = v_params.x;
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
        coverage = rect_cov(p, v_rect, v_params.x);
        float t;
        if (kind == 3.0) {
            float dist = length(p - v_point2);
            t = clamp((dist - v_params.y) / (v_params.w - v_params.y), 0.0, 1.0);
        } else if (kind == 5.0) {
            // the ellipse is a circle in a Y-scaled space; its corner
            // slot carries the aspect, so the cover is the plain box
            coverage = rect_cov(p, v_rect, 0.0);
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

const SPRITE_VERT: &str = r#"
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

const SPRITE_FRAG_BODY: &str = r#"
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
const MASK_VERT: &str = r#"
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

const MASK_FRAG_BODY: &str = r#"
out vec4 out_color;

void main() {
    // the window's own rounded box rides the Round block
    out_color = vec4(0.0, 0.0, 0.0, rect_cov(raster_p(), round_box, round_radius));
}
"#;

// MARK: - The stack (display, context, pipelines, fixed state)

/// Everything a render target needs, window or offscreen: the EGL
/// display+context pair and the compiled pipelines. Built once; any
/// failure prints one line and the caller falls back to the CPU.
struct GlStack {
    egl: &'static Loader,
    gl: GlFns,
    display: EglDisplay,
    config: EglConfig,
    context: EglContext,
    rect_program: u32,
    sprite_program: u32,
    mask_program: u32,
    mask_quad: i32,
    vao_rect: u32,
    vao_sprite: u32,
    frame_ubo: u32,
    round_ubo: u32,
    /// The Frame block's cached viewport — written on change only.
    frame_viewport: (f32, f32),
}

const UBO_FRAME_BINDING: u32 = 0;
const UBO_ROUND_BINDING: u32 = 1;

unsafe fn compile_stage(gl: &GlFns, kind: GlEnum, sources: &[&str]) -> Result<u32, String> {
    unsafe {
        let shader = (gl.create_shader)(kind);
        let joined: String = sources.concat();
        let source = CString::new(joined).expect("shader source without NUL");
        let pointer = source.as_ptr();
        (gl.shader_source)(shader, 1, &pointer, std::ptr::null());
        (gl.compile_shader)(shader);
        let mut status = 0;
        (gl.get_shaderiv)(shader, GL_COMPILE_STATUS, &mut status);
        if status == 0 {
            let mut log = vec![0u8; 2048];
            let mut length = 0;
            (gl.get_shader_info_log)(shader, log.len() as i32, &mut length, log.as_mut_ptr().cast());
            (gl.delete_shader)(shader);
            return Err(String::from_utf8_lossy(&log[..length.max(0) as usize]).into_owned());
        }
        Ok(shader)
    }
}

/// Compiles and links one pipeline; attribute names bind to fixed
/// locations BEFORE the link so the layout is deterministic.
unsafe fn build_program(
    gl: &GlFns,
    vert: &[&str],
    frag: &[&str],
    attribs: &[&std::ffi::CStr],
) -> Result<u32, String> {
    unsafe {
        let vs = compile_stage(gl, GL_VERTEX_SHADER, vert)?;
        let fs = match compile_stage(gl, GL_FRAGMENT_SHADER, frag) {
            Ok(fs) => fs,
            Err(log) => {
                (gl.delete_shader)(vs);
                return Err(log);
            }
        };
        let program = (gl.create_program)();
        (gl.attach_shader)(program, vs);
        (gl.attach_shader)(program, fs);
        for (index, name) in attribs.iter().enumerate() {
            (gl.bind_attrib_location)(program, index as u32, name.as_ptr());
        }
        (gl.link_program)(program);
        (gl.delete_shader)(vs);
        (gl.delete_shader)(fs);
        let mut status = 0;
        (gl.get_programiv)(program, GL_LINK_STATUS, &mut status);
        if status == 0 {
            let mut log = vec![0u8; 2048];
            let mut length = 0;
            (gl.get_program_info_log)(
                program,
                log.len() as i32,
                &mut length,
                log.as_mut_ptr().cast(),
            );
            (gl.delete_program)(program);
            return Err(String::from_utf8_lossy(&log[..length.max(0) as usize]).into_owned());
        }
        // uniform blocks bind once; the sampler slot likewise
        let frame = (gl.get_uniform_block_index)(program, c"Frame".as_ptr());
        if frame != u32::MAX {
            (gl.uniform_block_binding)(program, frame, UBO_FRAME_BINDING);
        }
        let round = (gl.get_uniform_block_index)(program, c"Round".as_ptr());
        if round != u32::MAX {
            (gl.uniform_block_binding)(program, round, UBO_ROUND_BINDING);
        }
        (gl.use_program)(program);
        let atlas = (gl.get_uniform_location)(program, c"atlas".as_ptr());
        if atlas >= 0 {
            (gl.uniform1i)(atlas, 0);
        }
        Ok(program)
    }
}

/// What the window road wants from a config vs the offscreen road.
#[derive(Clone, Copy, PartialEq)]
enum TargetKind {
    /// An opaque window buffer (XRGB — the CPU road's twin).
    Window,
    /// A window buffer with alpha: the scene's corners fade through it.
    SceneWindow,
    /// A surfaceless/pbuffer context rendering into an FBO.
    Offscreen,
}

impl GlStack {
    fn create(kind: TargetKind, platform: u32, native_display: *mut c_void) -> Option<GlStack> {
        let result = Self::build(kind, platform, native_display);
        if let Err(reason) = &result {
            eprintln!("bunny_ui gl: {reason} — presenting by cpu");
        }
        result.ok()
    }

    fn build(
        kind: TargetKind,
        platform: u32,
        native_display: *mut c_void,
    ) -> Result<GlStack, String> {
        let loader = loader().ok_or("no libEGL.so.1 on this system")?;
        let egl = &loader.egl;
        unsafe {
            let display = match kind {
                TargetKind::Offscreen => {
                    // surfaceless first (the headless door Mesa keeps
                    // open), the caller's platform as the fallback
                    let surfaceless = (egl.get_platform_display)(
                        EGL_PLATFORM_SURFACELESS_MESA,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    );
                    if surfaceless.is_null() {
                        (egl.get_platform_display)(platform, native_display, std::ptr::null())
                    } else {
                        surfaceless
                    }
                }
                _ => (egl.get_platform_display)(platform, native_display, std::ptr::null()),
            };
            if display.is_null() {
                return Err("no EGL display".to_string());
            }
            let (mut major, mut minor) = (0, 0);
            if (egl.initialize)(display, &mut major, &mut minor) == 0 {
                return Err("EGL refused to initialize".to_string());
            }
            if (egl.bind_api)(EGL_OPENGL_API) == 0 {
                return Err("EGL has no OpenGL API".to_string());
            }
            // NON-sRGB config forever — the parity law. The scene asks
            // for alpha (its corners fade); everything else is opaque.
            let surface_bit = match kind {
                TargetKind::Offscreen => EGL_PBUFFER_BIT,
                _ => EGL_WINDOW_BIT,
            };
            let alpha = if kind == TargetKind::SceneWindow { 8 } else { 0 };
            let attribs = [
                EGL_SURFACE_TYPE,
                surface_bit,
                EGL_RENDERABLE_TYPE,
                EGL_OPENGL_BIT,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_ALPHA_SIZE,
                alpha,
                EGL_DEPTH_SIZE,
                0,
                EGL_STENCIL_SIZE,
                0,
                EGL_NONE,
            ];
            let mut config: EglConfig = std::ptr::null_mut();
            let mut found = 0;
            // surfaceless offers no surface bits at all — retry the
            // choose without the surface-type row before giving up
            let chosen = (egl.choose_config)(display, attribs.as_ptr(), &mut config, 1, &mut found)
                != 0
                && found > 0;
            if !chosen {
                let bare = [
                    EGL_RENDERABLE_TYPE,
                    EGL_OPENGL_BIT,
                    EGL_RED_SIZE,
                    8,
                    EGL_GREEN_SIZE,
                    8,
                    EGL_BLUE_SIZE,
                    8,
                    EGL_ALPHA_SIZE,
                    alpha,
                    EGL_NONE,
                ];
                if kind != TargetKind::Offscreen
                    || (egl.choose_config)(display, bare.as_ptr(), &mut config, 1, &mut found) == 0
                    || found == 0
                {
                    return Err("no EGL config fits".to_string());
                }
            }
            let context_attribs = [
                EGL_CONTEXT_MAJOR_VERSION,
                3,
                EGL_CONTEXT_MINOR_VERSION,
                3,
                EGL_CONTEXT_OPENGL_PROFILE_MASK,
                EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT,
                EGL_NONE,
            ];
            let context =
                (egl.create_context)(display, config, EGL_NO_CONTEXT, context_attribs.as_ptr());
            if context == EGL_NO_CONTEXT {
                return Err("no GL 3.3 core context".to_string());
            }
            // the offscreen road goes current surfaceless right away
            // (KHR_surfaceless_context — Mesa always); the window road
            // waits for its wl_egl_window, current comes later
            if kind == TargetKind::Offscreen
                && (egl.make_current)(display, EGL_NO_SURFACE, EGL_NO_SURFACE, context) == 0
            {
                // fall back to a 1×1 pbuffer as the anchor surface —
                // the FBO is the real target either way
                let tiny = [EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE];
                let anchor = (egl.create_pbuffer_surface)(display, config, tiny.as_ptr());
                if anchor == EGL_NO_SURFACE
                    || (egl.make_current)(display, anchor, anchor, context) == 0
                {
                    (egl.destroy_context)(display, context);
                    return Err("no current context for the offscreen target".to_string());
                }
            }
            let mut stack = GlStack {
                egl: loader,
                gl: resolve_gl(egl).ok_or("a GL 3.3 symbol is missing")?,
                display,
                config,
                context,
                rect_program: 0,
                sprite_program: 0,
                mask_program: 0,
                mask_quad: -1,
                vao_rect: 0,
                vao_sprite: 0,
                frame_ubo: 0,
                round_ubo: 0,
                frame_viewport: (0.0, 0.0),
            };
            if kind == TargetKind::Offscreen {
                stack.finish_pipelines()?;
            }
            Ok(stack)
        }
    }

    /// Compiles the pipelines and allocates the fixed buffers — needs a
    /// CURRENT context, so the window road calls it after make_current.
    fn finish_pipelines(&mut self) -> Result<(), String> {
        let gl = &self.gl;
        unsafe {
            self.rect_program = build_program(
                gl,
                &[SHADER_PRELUDE, RECT_VERT],
                &[SHADER_PRELUDE, SHARED_FRAG, RECT_FRAG_BODY],
                &[c"a_rect", c"a_clip", c"a_params", c"a_color", c"a_color2", c"a_point2"],
            )
            .map_err(|log| format!("rect pipeline refused: {}", log.trim()))?;
            self.sprite_program = build_program(
                gl,
                &[SHADER_PRELUDE, SPRITE_VERT],
                &[SHADER_PRELUDE, SHARED_FRAG, SPRITE_FRAG_BODY],
                &[c"a_dest", c"a_tex", c"a_clip"],
            )
            .map_err(|log| format!("sprite pipeline refused: {}", log.trim()))?;
            self.mask_program = build_program(
                gl,
                &[SHADER_PRELUDE, MASK_VERT],
                &[SHADER_PRELUDE, SHARED_FRAG, MASK_FRAG_BODY],
                &[],
            )
            .map_err(|log| format!("mask pipeline refused: {}", log.trim()))?;
            (gl.use_program)(self.mask_program);
            self.mask_quad = (gl.get_uniform_location)(self.mask_program, c"u_quad".as_ptr());
            let mut vaos = [0u32; 2];
            (gl.gen_vertex_arrays)(2, vaos.as_mut_ptr());
            self.vao_rect = vaos[0];
            self.vao_sprite = vaos[1];
            let mut ubos = [0u32; 2];
            (gl.gen_buffers)(2, ubos.as_mut_ptr());
            self.frame_ubo = ubos[0];
            self.round_ubo = ubos[1];
            // std140: Frame = one vec2 in a 16-byte register; Round =
            // vec4 + float, padded to 32
            (gl.bind_buffer)(GL_UNIFORM_BUFFER, self.frame_ubo);
            (gl.buffer_data)(GL_UNIFORM_BUFFER, 16, std::ptr::null(), GL_STREAM_DRAW);
            (gl.bind_buffer)(GL_UNIFORM_BUFFER, self.round_ubo);
            (gl.buffer_data)(GL_UNIFORM_BUFFER, 32, std::ptr::null(), GL_STREAM_DRAW);
            (gl.bind_buffer_base)(GL_UNIFORM_BUFFER, UBO_FRAME_BINDING, self.frame_ubo);
            (gl.bind_buffer_base)(GL_UNIFORM_BUFFER, UBO_ROUND_BINDING, self.round_ubo);
            // fixed state the frame never changes: gamma-space
            // source-over with straight alpha — the LITERAL blend_px
            // formula: rgb = s·sa + d·(1−sa); a = sa + da·(1−sa)
            (gl.disable)(GL_DEPTH_TEST);
            (gl.disable)(GL_CULL_FACE);
            (gl.disable)(GL_SCISSOR_TEST);
            (gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
            (gl.pixel_storei)(GL_PACK_ALIGNMENT, 1);
            Ok(())
        }
    }

    /// Writes the Frame block when the target size changes — 16 bytes,
    /// orphaned (the driver renames; no stall against in-flight reads).
    unsafe fn set_viewport(&mut self, viewport: (f32, f32)) {
        if self.frame_viewport == viewport {
            return;
        }
        self.frame_viewport = viewport;
        let gl = &self.gl;
        let bytes: [f32; 4] = [viewport.0, viewport.1, 0.0, 0.0];
        unsafe {
            (gl.bind_buffer)(GL_UNIFORM_BUFFER, self.frame_ubo);
            (gl.buffer_data)(
                GL_UNIFORM_BUFFER,
                16,
                bytes.as_ptr().cast(),
                GL_STREAM_DRAW,
            );
        }
    }

    /// One pass over the bound target: clear to `canvas`, then the runs
    /// in paint order — the pipeline swaps only where rects and text
    /// alternate, and the per-run base rides the attrib pointers.
    #[allow(clippy::too_many_arguments)]
    unsafe fn encode_frame(
        &mut self,
        viewport: (f32, f32),
        canvas: Color,
        slot: &FrameSlot,
        runs: &[DrawRun],
        rounds: &[RoundClip],
        atlas_texture: u32,
        textures: &[u64],
        corner_mask: Option<f64>,
    ) {
        unsafe {
            self.set_viewport(viewport);
            let gl = &self.gl;
            (gl.viewport)(0, 0, viewport.0 as i32, viewport.1 as i32);
            (gl.clear_color)(
                canvas.r as f32 / 255.0,
                canvas.g as f32 / 255.0,
                canvas.b as f32 / 255.0,
                canvas.a as f32 / 255.0,
            );
            (gl.clear)(GL_COLOR_BUFFER_BIT);
            (gl.enable)(GL_BLEND);
            (gl.blend_func_separate)(
                GL_SRC_ALPHA,
                GL_ONE_MINUS_SRC_ALPHA,
                GL_ONE,
                GL_ONE_MINUS_SRC_ALPHA,
            );
            let mut bound: Option<RunKind> = None;
            let mut bound_round: Option<u32> = None;
            let mut bound_texture: Option<u32> = None;
            for run in runs {
                // 32 bytes per SHAPE change — a frame with no rounded
                // clip writes slot zero once; bindings persist across
                // the pipeline swaps
                if bound_round != Some(run.round) {
                    let round = &rounds[run.round as usize];
                    (gl.bind_buffer)(GL_UNIFORM_BUFFER, self.round_ubo);
                    (gl.buffer_data)(
                        GL_UNIFORM_BUFFER,
                        32,
                        (round as *const RoundClip).cast(),
                        GL_STREAM_DRAW,
                    );
                    bound_round = Some(run.round);
                }
                let swap_kind = match (bound, run.kind) {
                    (Some(RunKind::Sprites | RunKind::Texture(_)), RunKind::Sprites
                    | RunKind::Texture(_)) => false,
                    (was, now) => was != Some(now),
                };
                if swap_kind {
                    match run.kind {
                        RunKind::Rects => {
                            (gl.use_program)(self.rect_program);
                            (gl.bind_vertex_array)(self.vao_rect);
                        }
                        RunKind::Sprites | RunKind::Texture(_) => {
                            (gl.use_program)(self.sprite_program);
                            (gl.bind_vertex_array)(self.vao_sprite);
                        }
                    }
                }
                match run.kind {
                    RunKind::Rects => {
                        // the run's base = the byte offset every attrib
                        // pointer carries — the SV_InstanceID trap of
                        // the windows port, dissolved
                        (gl.bind_buffer)(GL_ARRAY_BUFFER, slot.rects.buffer);
                        let base = run.base as usize * std::mem::size_of::<RectInstance>();
                        rect_attribs(gl, base);
                    }
                    RunKind::Sprites | RunKind::Texture(_) => {
                        (gl.bind_buffer)(GL_ARRAY_BUFFER, slot.sprites.buffer);
                        let base = run.base as usize * std::mem::size_of::<SpriteInstance>();
                        sprite_attribs(gl, base);
                        // the shared atlas, or the run's own dedicated
                        // texture — same pipeline (the walk's handles
                        // ARE gl texture names on this tier)
                        let texture = match run.kind {
                            RunKind::Texture(index) => textures[index as usize] as u32,
                            _ => atlas_texture,
                        };
                        if bound_texture != Some(texture) {
                            (gl.active_texture)(GL_TEXTURE0);
                            (gl.bind_texture)(GL_TEXTURE_2D, texture);
                            bound_texture = Some(texture);
                        }
                    }
                }
                bound = Some(run.kind);
                (gl.draw_arrays_instanced)(GL_TRIANGLES, 0, 6, run.count as i32);
            }
            if let Some(radius) = corner_mask {
                self.mask_corners(viewport, radius);
            }
            (gl.bind_vertex_array)(0);
        }
    }

    /// The scene's rounded corners: four corner squares multiplied by
    /// the window's own rounded coverage — dst *= src.alpha is the
    /// premultiplied fade over an alpha surface, and everywhere else
    /// the buffer stays exactly the opaque frame.
    unsafe fn mask_corners(&self, viewport: (f32, f32), radius: f64) {
        let gl = &self.gl;
        let r = radius as f32;
        let (w, h) = viewport;
        unsafe {
            let round = RoundClip { box4: [0.0, 0.0, w, h], radius: r, pad: [0.0; 3] };
            (gl.bind_buffer)(GL_UNIFORM_BUFFER, self.round_ubo);
            (gl.buffer_data)(
                GL_UNIFORM_BUFFER,
                32,
                (&round as *const RoundClip).cast(),
                GL_STREAM_DRAW,
            );
            (gl.use_program)(self.mask_program);
            (gl.bind_vertex_array)(self.vao_rect);
            (gl.blend_func_separate)(GL_ZERO, GL_SRC_ALPHA, GL_ZERO, GL_SRC_ALPHA);
            let quads = [
                [0.0, 0.0, r, r],
                [w - r, 0.0, w, r],
                [0.0, h - r, r, h],
                [w - r, h - r, w, h],
            ];
            for quad in quads {
                (gl.uniform4f)(self.mask_quad, quad[0], quad[1], quad[2], quad[3]);
                (gl.draw_arrays)(GL_TRIANGLES, 0, 6);
            }
            (gl.blend_func_separate)(
                GL_SRC_ALPHA,
                GL_ONE_MINUS_SRC_ALPHA,
                GL_ONE,
                GL_ONE_MINUS_SRC_ALPHA,
            );
        }
    }
}

impl Drop for GlStack {
    fn drop(&mut self) {
        // the caller tears the surface first; the context goes last.
        // Objects inside the context die with it — deleting them one
        // by one against a possibly-lost context buys nothing.
        unsafe {
            (self.egl.egl.make_current)(self.display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
            if self.context != EGL_NO_CONTEXT {
                (self.egl.egl.destroy_context)(self.display, self.context);
            }
        }
    }
}

/// The rect layout at `base` bytes: 3×vec4 + 2× normalized ubyte vec4 +
/// vec2, stride 64, divisor 1 — the RectInstance bytes, spoken to GL.
unsafe fn rect_attribs(gl: &GlFns, base: usize) {
    let stride = std::mem::size_of::<RectInstance>() as i32;
    unsafe {
        for (index, offset, count, kind, normalized) in [
            (0u32, 0usize, 4, GL_FLOAT, 0u8),
            (1, 16, 4, GL_FLOAT, 0),
            (2, 32, 4, GL_FLOAT, 0),
            (3, 48, 4, GL_UNSIGNED_BYTE, 1),
            (4, 52, 4, GL_UNSIGNED_BYTE, 1),
            (5, 56, 2, GL_FLOAT, 0),
        ] {
            (gl.enable_vertex_attrib_array)(index);
            (gl.vertex_attrib_pointer)(
                index,
                count,
                kind,
                normalized,
                stride,
                (base + offset) as *const c_void,
            );
            (gl.vertex_attrib_divisor)(index, 1);
        }
    }
}

/// The sprite layout at `base` bytes: 3×vec4, stride 48, divisor 1.
unsafe fn sprite_attribs(gl: &GlFns, base: usize) {
    let stride = std::mem::size_of::<SpriteInstance>() as i32;
    unsafe {
        for (index, offset) in [(0u32, 0usize), (1, 16), (2, 32)] {
            (gl.enable_vertex_attrib_array)(index);
            (gl.vertex_attrib_pointer)(
                index,
                4,
                GL_FLOAT,
                0,
                stride,
                (base + offset) as *const c_void,
            );
            (gl.vertex_attrib_divisor)(index, 1);
        }
    }
}

// MARK: - The GL ground (where the walk's tiles land on this tier)

/// One RGBA texture, shader-read only, NEAREST/CLAMP (texelFetch never
/// samples, but complete state keeps every driver honest).
unsafe fn make_texture(gl: &GlFns, width: u32, height: u32, initial: Option<&[u8]>) -> u32 {
    unsafe {
        let mut texture = 0;
        (gl.gen_textures)(1, &mut texture);
        (gl.bind_texture)(GL_TEXTURE_2D, texture);
        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        (gl.tex_image_2d)(
            GL_TEXTURE_2D,
            0,
            GL_RGBA8 as i32,
            width as i32,
            height as i32,
            0,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            initial.map_or(std::ptr::null(), |bytes| bytes.as_ptr().cast()),
        );
        texture
    }
}

/// The GL tier's [`AtlasGround`]: the shared texture id lives in the
/// presenter (`shared`), dedicated handles ARE the GL texture names.
struct GlGround<'a> {
    gl: &'a GlFns,
    shared: &'a mut Option<u32>,
}

impl AtlasGround for GlGround<'_> {
    fn ensure_shared(&mut self, size: u32) -> bool {
        if self.shared.is_some() {
            return true;
        }
        let texture = unsafe { make_texture(self.gl, size, size, None) };
        *self.shared = (texture != 0).then_some(texture);
        self.shared.is_some()
    }

    fn upload_shared(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: *const u8, pitch_px: u32) {
        let Some(texture) = *self.shared else { return };
        unsafe {
            (self.gl.bind_texture)(GL_TEXTURE_2D, texture);
            (self.gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, pitch_px as i32);
            (self.gl.tex_sub_image_2d)(
                GL_TEXTURE_2D,
                0,
                x as i32,
                y as i32,
                w as i32,
                h as i32,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                bytes.cast(),
            );
            (self.gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, 0);
        }
    }

    fn drop_shared(&mut self) {
        if let Some(texture) = self.shared.take() {
            unsafe { (self.gl.delete_textures)(1, &texture) };
        }
    }

    fn make_dedicated(&mut self, w: u32, h: u32, bytes: &[u8], pitch_px: u32) -> Option<u64> {
        unsafe {
            (self.gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, pitch_px as i32);
            let texture = make_texture(self.gl, w, h, Some(bytes));
            (self.gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, 0);
            (texture != 0).then_some(texture as u64)
        }
    }

    fn drop_dedicated(&mut self, id: u64) {
        let texture = id as u32;
        unsafe { (self.gl.delete_textures)(1, &texture) };
    }
}

// MARK: - Instance buffers (a fixed ring, recycled by polling)

/// One side of a slot: a vertex buffer sized in instances of one
/// stride. `0` = not yet allocated.
struct SlotBuffer {
    buffer: u32,
    capacity: usize,
}

impl SlotBuffer {
    const fn empty() -> SlotBuffer {
        SlotBuffer { buffer: 0, capacity: 0 }
    }

    /// Grows (never shrinks) to hold `count` instances of `stride`
    /// bytes. The ring frees the slot before reuse, so the fresh store
    /// never races an in-flight read.
    fn ensure(&mut self, gl: &GlFns, count: usize, stride: usize) -> bool {
        if count == 0 || self.capacity >= count {
            return true;
        }
        let capacity = count.next_multiple_of(64);
        unsafe {
            if self.buffer == 0 {
                (gl.gen_buffers)(1, &mut self.buffer);
                if self.buffer == 0 {
                    return false;
                }
            }
            (gl.bind_buffer)(GL_ARRAY_BUFFER, self.buffer);
            (gl.buffer_data)(
                GL_ARRAY_BUFFER,
                (capacity * stride) as isize,
                std::ptr::null(),
                GL_STREAM_DRAW,
            );
            self.capacity = capacity;
            ((gl.get_error)() == 0).then_some(()).is_some()
        }
    }

    /// One sub-data write — the whole side of the frame at once, into a
    /// slot the fence already proved free.
    fn upload<T>(&mut self, gl: &GlFns, items: &[T]) -> bool {
        if items.is_empty() {
            return true;
        }
        if self.buffer == 0 {
            return false;
        }
        unsafe {
            (gl.bind_buffer)(GL_ARRAY_BUFFER, self.buffer);
            (gl.buffer_sub_data)(
                GL_ARRAY_BUFFER,
                0,
                std::mem::size_of_val(items) as isize,
                items.as_ptr().cast(),
            );
        }
        true
    }
}

/// One in-flight frame: its instance buffers and the fence that answers
/// when the GPU is done reading them. A signaled fence frees the slot.
struct FrameSlot {
    rects: SlotBuffer,
    sprites: SlotBuffer,
    fence: GlSync,
    in_flight: bool,
}

impl FrameSlot {
    const fn empty() -> FrameSlot {
        FrameSlot {
            rects: SlotBuffer::empty(),
            sprites: SlotBuffer::empty(),
            fence: std::ptr::null_mut(),
            in_flight: false,
        }
    }
}

fn fence_done(gl: &GlFns, fence: GlSync) -> bool {
    if fence.is_null() {
        return true;
    }
    let status = unsafe { (gl.client_wait_sync)(fence, 0, 0) };
    status == GL_ALREADY_SIGNALED || status == GL_CONDITION_SATISFIED
}

/// A free slot from a ring: polled by the fence, oldest-first. When all
/// ride the GPU (a burst above the refresh rate), waits for the oldest
/// — bounded by one sub-millisecond frame.
fn acquire_slot(gl: &GlFns, slots: &mut [FrameSlot; 3], cursor: &mut usize) -> usize {
    for offset in 0..slots.len() {
        let index = (*cursor + offset) % slots.len();
        let free = !slots[index].in_flight || fence_done(gl, slots[index].fence);
        if free {
            slots[index].in_flight = false;
            *cursor = (index + 1) % slots.len();
            return index;
        }
    }
    let index = *cursor;
    if !slots[index].fence.is_null() {
        unsafe {
            (gl.client_wait_sync)(slots[index].fence, GL_SYNC_FLUSH_COMMANDS_BIT, u64::MAX);
        }
    }
    slots[index].in_flight = false;
    *cursor = (index + 1) % slots.len();
    index
}

/// Marks the slot in flight: a fence lands after this frame's commands
/// and the ring polls it (the whole shell is one thread — a completion
/// callback would be the only concurrent code in the crate). The flush
/// pushes the fence to the GPU so a poll can ever see it signal.
fn mark_in_flight(gl: &GlFns, slot: &mut FrameSlot) {
    unsafe {
        if !slot.fence.is_null() {
            (gl.delete_sync)(slot.fence);
        }
        slot.fence = (gl.fence_sync)(GL_SYNC_GPU_COMMANDS_COMPLETE, 0);
        (gl.flush)();
    }
    slot.in_flight = !slot.fence.is_null();
}

fn drain_slots(gl: &GlFns, slots: &mut [FrameSlot; 3]) {
    for slot in slots.iter_mut() {
        if slot.in_flight && !slot.fence.is_null() {
            unsafe {
                (gl.client_wait_sync)(slot.fence, GL_SYNC_FLUSH_COMMANDS_BIT, u64::MAX);
            }
        }
        slot.in_flight = false;
    }
}

/// Uploads the frame's instances into the slot's two buffers (rects and
/// sprites each have their own stride, so each rides its own vertex
/// buffer). The size is EXACT before the API is touched — no
/// speculative encode, no overflow retry.
fn upload_frame(slot: &mut FrameSlot, gl: &GlFns, batches: &FrameBatches) -> bool {
    slot.rects.ensure(gl, batches.rects.len(), std::mem::size_of::<RectInstance>())
        && slot.sprites.ensure(gl, batches.sprites.len(), std::mem::size_of::<SpriteInstance>())
        && slot.rects.upload(gl, &batches.rects)
        && slot.sprites.upload(gl, &batches.sprites)
}

// MARK: - The window presenter

/// The per-window GPU state. Like the CPU backing: one main window, so
/// the presenter lives in a thread-local next to the pump.
struct GlPresenter {
    stack: GlStack,
    egl_surface: EglSurface,
    /// The `wl_egl_window` bridging the wayland surface to EGL.
    native_window: *mut c_void,
    /// The buffer's size in device pixels.
    buffer: (usize, usize),
    /// The scene draws its own rounded corners — the mask pass.
    scene: bool,
    slots: [FrameSlot; 3],
    cursor: usize,
    atlas: RunAtlas,
    /// The shared atlas texture on THIS tier (the walk owns the data,
    /// the ground owns the pixels).
    shared: Option<u32>,
    batches: FrameBatches,
    /// The last presented frame's key — an identical frame skips the
    /// encode entirely.
    retained: Option<(DisplayList, (usize, usize), usize, Color)>,
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
    static PRESENTER: RefCell<Option<GlPresenter>> = const { RefCell::new(None) };
    /// The one silent recreate a lost context is allowed.
    static RECREATE_SPENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What one present attempt concluded.
#[derive(PartialEq)]
enum Presented {
    Ok,
    /// The context is gone (GPU reset) — the caller rebuilds the whole
    /// stack once, or falls back to the CPU.
    DeviceLost,
}

impl GlPresenter {
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
            let mut ground = GlGround { gl: &self.stack.gl, shared: &mut self.shared };
            match build_frame(
                &mut ground,
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
                        eprintln!("bunny_ui gl: atlas overflow survived two resets");
                        return;
                    }
                    drain_slots(&self.stack.gl, &mut self.slots);
                    let mut ground =
                        GlGround { gl: &self.stack.gl, shared: &mut self.shared };
                    self.atlas.reset(&mut ground, true);
                }
            }
        }
    }

    /// One frame: walk the list, upload, resize the EGL window if the
    /// surface changed, encode, swap. The swap IS the wayland commit —
    /// the shell arms the frame callback just before it and notes the
    /// present just after, so the pacing law holds unchanged.
    fn present(
        &mut self,
        display: &DisplayList,
        size: Size,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) -> Presented {
        let physical = (
            (size.width * scale as f64).round().max(0.0) as usize,
            (size.height * scale as f64).round().max(0.0) as usize,
        );
        if physical.0 == 0 || physical.1 == 0 {
            // a zero target is an abort, not a frame
            return Presented::Ok;
        }
        if frame_repeats(&self.retained, display, physical, scale, canvas) {
            // the caret blink and friends land here every half second —
            // nothing changed, nothing encodes, nothing commits
            return Presented::Ok;
        }
        let egl = &self.stack.egl.egl;
        if physical != self.buffer {
            // wayland resizes its egl window in place; the x11 surface
            // tracks the X window by itself — nothing to say
            if !self.native_window.is_null() {
                unsafe {
                    (self.stack.egl.wl_egl.window_resize)(
                        self.native_window,
                        physical.0 as c_int,
                        physical.1 as c_int,
                        0,
                        0,
                    );
                }
            }
            self.buffer = physical;
        }
        self.build_with_retries(display, scale, physical, text, images);
        let index = acquire_slot(&self.stack.gl, &mut self.slots, &mut self.cursor);
        if !upload_frame(&mut self.slots[index], &self.stack.gl, &self.batches) {
            return Presented::DeviceLost;
        }
        // the scene's corners round at the same radius the CPU road
        // masks; a maximized scene fills its edges like the CPU too
        let corner_mask = self.scene.then_some(8.0 * scale as f64);
        unsafe {
            (self.stack.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
            self.stack.encode_frame(
                (physical.0 as f32, physical.1 as f32),
                canvas,
                &self.slots[index],
                &self.batches.runs,
                &self.batches.rounds,
                self.shared.unwrap_or(0),
                &self.batches.textures,
                corner_mask,
            );
        }
        // the map dance holds: buffer scale and the frame callback go
        // on the surface BEFORE the swap commits it, and the present
        // is noted after — the CPU road's exact envelope
        if !crate::ffi::gpu_pre_present(scale) {
            // not configured yet: encoding was free, committing is not
            // legal — the next redraw lands the frame
            mark_in_flight(&self.stack.gl, &mut self.slots[index]);
            return Presented::Ok;
        }
        let swapped = unsafe { (egl.swap_buffers)(self.stack.display, self.egl_surface) };
        mark_in_flight(&self.stack.gl, &mut self.slots[index]);
        if swapped == 0 {
            let error = unsafe { (egl.get_error)() };
            if error == EGL_CONTEXT_LOST {
                return Presented::DeviceLost;
            }
            // any other swap failure: skip the frame, keep the road
            return Presented::Ok;
        }
        crate::ffi::gpu_note_present();
        self.retained = Some((display.clone(), physical, scale, canvas));
        Presented::Ok
    }
}

/// Builds the whole stack for one window: display, context, pipelines
/// and the EGL surface over the door's native window — the wayland
/// surface through `wl_egl_window`, the x11 window through the EGL 1.5
/// platform road. `None` falls back to the CPU raster.
fn install() -> Option<GlPresenter> {
    use crate::ffi::GpuTargets;
    let targets = crate::ffi::gpu_targets()?;
    let (platform, native_display, scene) = match &targets {
        GpuTargets::Wayland { display, scene, .. } => (EGL_PLATFORM_WAYLAND, *display, *scene),
        GpuTargets::X11 { connection, scene, .. } => (EGL_PLATFORM_XCB_EXT, *connection, *scene),
    };
    let kind = if scene { TargetKind::SceneWindow } else { TargetKind::Window };
    let mut stack = GlStack::create(kind, platform, native_display)?;
    let (width, height) = crate::ffi::gpu_buffer_size();
    let loader = stack.egl;
    unsafe {
        // the wayland arm owns a wl_egl_window it must also resize and
        // destroy; the x11 arm wraps the xid and the surface tracks
        // the window by itself
        let (native_window, egl_surface) = match &targets {
            GpuTargets::Wayland { surface, .. } => {
                let native_window = (loader.wl_egl.window_create)(
                    *surface,
                    width.max(1) as c_int,
                    height.max(1) as c_int,
                );
                if native_window.is_null() {
                    eprintln!("bunny_ui gl: no wl_egl_window — presenting by cpu");
                    return None;
                }
                let egl_surface = (loader.egl.create_window_surface)(
                    stack.display,
                    stack.config,
                    native_window,
                    std::ptr::null(),
                );
                (native_window, egl_surface)
            }
            GpuTargets::X11 { window, .. } => {
                let mut xid = *window;
                let egl_surface = (loader.egl.create_platform_window_surface)(
                    stack.display,
                    stack.config,
                    (&raw mut xid).cast(),
                    std::ptr::null(),
                );
                (std::ptr::null_mut(), egl_surface)
            }
        };
        if egl_surface == EGL_NO_SURFACE {
            if !native_window.is_null() {
                (loader.wl_egl.window_destroy)(native_window);
            }
            eprintln!("bunny_ui gl: no EGL surface — presenting by cpu");
            return None;
        }
        if (loader.egl.make_current)(stack.display, egl_surface, egl_surface, stack.context) == 0 {
            (loader.egl.destroy_surface)(stack.display, egl_surface);
            if !native_window.is_null() {
                (loader.wl_egl.window_destroy)(native_window);
            }
            eprintln!("bunny_ui gl: make_current refused — presenting by cpu");
            return None;
        }
        // LOAD-BEARING: interval 0 or Mesa paces us against its own
        // frame callback — double-throttled visible, deadlocked hidden
        (loader.egl.swap_interval)(stack.display, 0);
        if let Err(reason) = stack.finish_pipelines() {
            eprintln!("bunny_ui gl: {reason} — presenting by cpu");
            (loader.egl.destroy_surface)(stack.display, egl_surface);
            if !native_window.is_null() {
                (loader.wl_egl.window_destroy)(native_window);
            }
            return None;
        }
        // the compositor must never resize the window past what the
        // GPU can render — the texture ceiling, spoken as a max size
        let mut max_texture = 0;
        (stack.gl.get_integerv)(GL_MAX_TEXTURE_SIZE, &mut max_texture);
        if max_texture > 0 {
            crate::ffi::gpu_limit_size(max_texture as usize);
        }
        Some(GlPresenter {
            stack,
            egl_surface,
            native_window,
            buffer: (width, height),
            scene,
            slots: [FrameSlot::empty(), FrameSlot::empty(), FrameSlot::empty()],
            cursor: 0,
            atlas: RunAtlas::new(),
            shared: None,
            batches: FrameBatches::default(),
            retained: None,
        })
    }
}

/// Grafts the GPU present onto the window — called by the shell
/// assembler after `create_window` and BEFORE the first frame, so the
/// first presenting commit (the reveal, by protocol design) is already
/// the GPU's. Returns false (and touches nothing) when the GPU path is
/// refused or cannot come up; the caller proceeds with the CPU path.
///
/// The default is the GPU. `BUNNY_PRESENT=cpu` forces the CPU raster
/// forever — checked before any EGL touch; any failure to come up (no
/// libEGL, no config, a shader that does not compile) prints one line
/// and falls back — a window never fails to open because of GL.
pub(crate) fn try_install() -> bool {
    if std::env::var("BUNNY_PRESENT").ok().as_deref() == Some("cpu") {
        return false;
    }
    let Some(presenter) = install() else {
        return false;
    };
    PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
    true
}

/// True when this window presents by GPU — the shell branches per frame
/// on this (a lost context may hand the window back to the CPU).
pub(crate) fn active() -> bool {
    PRESENTER.with(|slot| slot.borrow().is_some())
}

/// Forgets the retained frame so the NEXT present cannot skip — the
/// ack road leans on this when a state-only configure owes a commit
/// and a bare one is off the table.
pub(crate) fn invalidate() {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.retained = None;
        }
    });
}

/// The GPU twin of the Surface + blit path: same display list in, one
/// presented frame out. `text` is the frame's engine — the atlas
/// rasterizes through it, exactly like the CPU compositor.
///
/// A lost context (GPU reset mid-run) rebuilds the whole stack once in
/// silence and re-presents; lost again, the window presents by CPU for
/// the rest of its life with one line on stderr.
pub(crate) fn present_window(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    let outcome = PRESENTER.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|presenter| presenter.present(display, size, scale, canvas, text, images))
    });
    if outcome != Some(Presented::DeviceLost) {
        return;
    }
    teardown();
    if !RECREATE_SPENT.with(|spent| spent.replace(true)) {
        if let Some(mut presenter) = install() {
            presenter.present(display, size, scale, canvas, text, images);
            PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
            return;
        }
    }
    eprintln!("bunny_ui gl: the context is lost — presenting by cpu");
}

/// Releases the presenter before the wayland surface dies. The order is
/// law: EGL surface first, then the wl_egl_window, and the context goes
/// with the stack — all before `wl_surface.destroy`.
pub(crate) fn teardown() {
    PRESENTER.with(|slot| {
        let Some(presenter) = slot.borrow_mut().take() else { return };
        let loader = presenter.stack.egl;
        unsafe {
            (loader.egl.make_current)(
                presenter.stack.display,
                EGL_NO_SURFACE,
                EGL_NO_SURFACE,
                EGL_NO_CONTEXT,
            );
            if presenter.egl_surface != EGL_NO_SURFACE {
                (loader.egl.destroy_surface)(presenter.stack.display, presenter.egl_surface);
            }
            if !presenter.native_window.is_null() {
                (loader.wl_egl.window_destroy)(presenter.native_window);
            }
        }
        // the stack's Drop releases the context
    });
}

// MARK: - Offscreen target (parity tests and the bench)

/// A windowless render target: same stack, same shaders, an FBO whose
/// readback lines up with the CPU mirror byte for byte (rows flipped —
/// `glReadPixels` counts from the bottom). This is the harness surface
/// — the parity tests and the benchmark present here. The context comes
/// up surfaceless (llvmpipe is the WARP twin), so parity runs headless
/// on any machine.
pub struct OffscreenGl {
    stack: GlStack,
    framebuffer: u32,
    target: u32,
    width: usize,
    height: usize,
    slots: [FrameSlot; 3],
    cursor: usize,
    atlas: RunAtlas,
    shared: Option<u32>,
    batches: FrameBatches,
}

impl OffscreenGl {
    /// Makes a target of `width`×`height` device pixels. `None` when
    /// there is no GL at all or the shaders do not compile.
    pub fn new(width: usize, height: usize) -> Option<OffscreenGl> {
        if width == 0 || height == 0 {
            return None;
        }
        let stack =
            GlStack::create(TargetKind::Offscreen, EGL_PLATFORM_WAYLAND, std::ptr::null_mut())?;
        let gl = &stack.gl;
        let (framebuffer, target) = unsafe {
            let target = make_texture(gl, width as u32, height as u32, None);
            if target == 0 {
                return None;
            }
            let mut framebuffer = 0;
            (gl.gen_framebuffers)(1, &mut framebuffer);
            (gl.bind_framebuffer)(GL_FRAMEBUFFER, framebuffer);
            (gl.framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                target,
                0,
            );
            if (gl.check_framebuffer_status)(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
                eprintln!("bunny_ui gl: the offscreen framebuffer is incomplete");
                return None;
            }
            (framebuffer, target)
        };
        Some(OffscreenGl {
            stack,
            framebuffer,
            target,
            width,
            height,
            slots: [FrameSlot::empty(), FrameSlot::empty(), FrameSlot::empty()],
            cursor: 0,
            atlas: RunAtlas::new(),
            shared: None,
            batches: FrameBatches::default(),
        })
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
        for attempt in 0..3 {
            let mut ground = GlGround { gl: &self.stack.gl, shared: &mut self.shared };
            match build_frame(
                &mut ground,
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
                        eprintln!("bunny_ui gl: atlas overflow survived two resets");
                        break;
                    }
                    drain_slots(&self.stack.gl, &mut self.slots);
                    let mut ground =
                        GlGround { gl: &self.stack.gl, shared: &mut self.shared };
                    self.atlas.reset(&mut ground, true);
                }
            }
        }
        let index = acquire_slot(&self.stack.gl, &mut self.slots, &mut self.cursor);
        if !upload_frame(&mut self.slots[index], &self.stack.gl, &self.batches) {
            return;
        }
        unsafe {
            (self.stack.gl.bind_framebuffer)(GL_FRAMEBUFFER, self.framebuffer);
            self.stack.encode_frame(
                (self.width as f32, self.height as f32),
                canvas,
                &self.slots[index],
                &self.batches.runs,
                &self.batches.rounds,
                self.shared.unwrap_or(0),
                &self.batches.textures,
                None,
            );
        }
        mark_in_flight(&self.stack.gl, &mut self.slots[index]);
        if wait {
            if !self.slots[index].fence.is_null() {
                unsafe {
                    (self.stack.gl.client_wait_sync)(
                        self.slots[index].fence,
                        GL_SYNC_FLUSH_COMMANDS_BIT,
                        u64::MAX,
                    );
                }
            }
            self.slots[index].in_flight = false;
        }
    }

    /// Renders and WAITS — determinism for tests and honest numbers for
    /// the bench (walk + upload + encode + GPU time, nothing hidden).
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

    /// Renders and RETURNS after the submit — the CPU-side cost of a
    /// production present (a window submits and moves on; the ring
    /// keeps the in-flight frames safe).
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
    /// Surface mirror, so parity compares are `==` over slices. The
    /// read blocks until the GPU is done; the rows flip on the way out
    /// (`glReadPixels` counts from the bottom, the raster from the top).
    pub fn read_rgba(&self) -> Vec<u8> {
        let gl = &self.stack.gl;
        let mut upside_down = vec![0u8; self.width * self.height * 4];
        unsafe {
            (gl.bind_framebuffer)(GL_FRAMEBUFFER, self.framebuffer);
            (gl.read_pixels)(
                0,
                0,
                self.width as i32,
                self.height as i32,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                upside_down.as_mut_ptr().cast(),
            );
        }
        let row = self.width * 4;
        let mut bytes = vec![0u8; upside_down.len()];
        for y in 0..self.height {
            let from = &upside_down[(self.height - 1 - y) * row..(self.height - y) * row];
            bytes[y * row..(y + 1) * row].copy_from_slice(from);
        }
        bytes
    }
}

impl Drop for OffscreenGl {
    fn drop(&mut self) {
        let gl = &self.stack.gl;
        unsafe {
            (gl.delete_framebuffers)(1, &self.framebuffer);
            (gl.delete_textures)(1, &self.target);
        }
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::prelude::*;
    use bunny_ui::raster::rasterize_with;

    /// One probe, cached: llvmpipe makes a context near-universal, but
    /// a machine without libEGL skips honestly.
    fn device_present() -> bool {
        static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let present = *PRESENT.get_or_init(|| OffscreenGl::new(4, 4).is_some());
        if !present {
            eprintln!("no gl context — skipping");
        }
        present
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
        let cpu = rasterize_with(
            &display,
            physical.0,
            physical.1,
            scale,
            canvas,
            &PixelFont,
            &RawImages::default(),
        )
        .to_rgba_bytes();
        let mut gpu = OffscreenGl::new(physical.0, physical.1).expect("offscreen gpu");
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
    fn the_context_compiles_every_pipeline() {
        if !device_present() {
            return;
        }
        let stack =
            GlStack::create(TargetKind::Offscreen, EGL_PLATFORM_WAYLAND, std::ptr::null_mut());
        assert!(stack.is_some(), "the runtime shader compile must succeed");
    }

    #[test]
    fn a_clear_frame_reads_back_the_canvas_color_exactly() {
        if !device_present() {
            return;
        }
        // this test is the ABI smoke: every resolved symbol in the
        // present path runs once — a wrong signature corrupts the
        // readback loudly (the lesson the text engine taught)
        let canvas = Color::hex(0x18181D);
        let mut gpu = OffscreenGl::new(16, 16).expect("offscreen gpu");
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
        // the corner-bug configuration, judged by the oracle
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
        use bunny_ui::layout::{Gradient, UnitPoint};
        // an elliptical wash across a wide bar — the aspect rides the
        // corner slot, so the kinds diverge from the circle and the
        // two rasterizers must agree
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
    fn freetype_runs_match_within_tolerance() {
        if !device_present() {
            return;
        }
        // the real engine, SAME instance on both sides: identical run
        // rasters in, so only blend rounding may differ
        use std::rc::Rc;
        let engine = crate::text::FreeTypeEngine::new();
        let logical = Size { width: 260.0, height: 100.0 };
        let scale = 2usize;
        let physical = (520, 200);
        let runtime = Runtime::new().text_engine(Rc::new(crate::text::FreeTypeEngine::new()));
        let root = vstack((
            text("Fjord glyphs vex quick waltz"),
            text("bunny_ui presents by egl").foreground_color(Color::hex(0x3B82F6)),
        ))
        .padding_length(10.0)
        .background_color(Color::hex(0xFFFFFF))
        .corner_radius(9.0);
        let display = runtime.display_frame(&root, logical);
        let cpu = rasterize_with(
            &display,
            physical.0,
            physical.1,
            scale,
            Color::CANVAS,
            &engine,
            &RawImages::default(),
        )
        .to_rgba_bytes();
        let mut gpu = OffscreenGl::new(physical.0, physical.1).expect("offscreen gpu");
        gpu.present_wait(&display, scale, Color::CANVAS, &engine, &RawImages::default());
        assert_close(&gpu.read_rgba(), &cpu, 2, "freetype runs");
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
        let mut gpu = OffscreenGl::new(240, 120).expect("offscreen gpu");
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
        let mut gpu = OffscreenGl::new(640, 800).expect("offscreen gpu");
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
            path: MARK_PATH,
            tint: None,
        }],
    };
    const DISC_GLYPH: bunny_ui::icon::Glyph = bunny_ui::icon::Glyph {
        draws: &[bunny_ui::icon::Draw {
            paint: bunny_ui::icon::Paint::Fill(bunny_ui::icon::Rule::NonZero),
            path: DISC_PATH,
            tint: None,
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

    #[test]
    fn icons_match_the_cpu_within_one_blend_step() {
        if !device_present() {
            return;
        }
        // the house rasterizes the glyph once and both pipelines blit
        // those same bytes — every interior texel (alpha 255) replaces
        // the destination and is EXACT. Only the anti-aliased edge
        // blends a partial alpha, and there the raster unit's rounding
        // may sit one step from the CPU's integer div255 — the same
        // documented deviation the windows port answers within one.
        let (gpu, cpu) =
            scene_bytes(&icon_scene(), Size { width: 120.0, height: 160.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "icon edges drifted by {delta} (allowed 1)");
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
        let mut gpu = OffscreenGl::new(240, 320).expect("offscreen gpu");
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
        // reset-retry (the livelock shape)
        let root = image(gradient_source(3)).resizable().frame(2100.0, 60.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 400.0, height: 100.0 }, 2, Color::CANVAS);
        assert!(gpu == cpu, "ultra-wide image diverged");
        assert!(
            gpu.chunks_exact(4).any(|pixel| pixel[..3] != [0xF2, 0xF3, 0xF7]),
            "the image painted through the dedicated texture"
        );
    }
}