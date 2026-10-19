//! The world pass of the GL family, written once.
//!
//! Desktop GL 3.3 and WebGL2 speak the same verbs; what differs is how a
//! call crosses (a function pointer, a JavaScript import). So the pass is
//! written here, against [`GlFamily`] — the verbs it needs, each one GL
//! call — and a tier only says how each verb crosses. Two tiers that
//! cannot drift, because there is one encoder.
//!
//! What the pass does is `crate::world::soft` in GL: the frame's draws in
//! order into a target the 2D pass composites (`super::walk::RunKind::World`).
//! A target is an RGBA8 texture; with four samples the drawing goes to
//! multisample renderbuffers that a blit resolves into it. The shaders
//! are `super::world_shaders`.
//!
//! A frame whose sun casts draws its casters first, when the box's map
//! does not hold them yet, into a 16-bit depth texture the surfaces read
//! through a comparing sampler (unit 2); a frame without one binds a map
//! of one texel that always answers "lit".
//!
//! A frame with a bloom draws its glow beside its colours, into a second
//! attachment (one byte a pixel), and runs the chain of
//! `crate::world::bloom` after the resolve: steps down and back up into
//! small textures of the target's own, and the spill blended over the
//! target as a screen.
//!
//! The 2D pass's state is put back when the pass ends: depth off, culling
//! off, blending on with the 2D pass's own factors, texture unit 0.

use std::collections::HashMap;
use std::rc::Rc;

use super::world::{Redraw, WorldCaps, WorldGround, WorldJob, WorldTiming, slide_copy};
use super::world_shaders as source;
use crate::image_engine::ImageEngine;
use crate::world::frame::{Background, Blend, DrawKind, ShadowFrame, WorldFrame};
use crate::world::bloom::LEVELS;
use crate::world::wire::{SEGMENT_STRIDE, SHADOW_INSTANCE_STRIDE, instance_bytes, material_bytes, shadow_instance_bytes};

/// The numbers GL answers to, the ones this pass uses.
pub mod gl {
    pub const TRIANGLES: u32 = 0x0004;
    pub const ARRAY_BUFFER: u32 = 0x8892;
    pub const ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
    pub const UNIFORM_BUFFER: u32 = 0x8A11;
    pub const STATIC_DRAW: u32 = 0x88E4;
    pub const STREAM_DRAW: u32 = 0x88E0;
    pub const BYTE: u32 = 0x1400;
    pub const UNSIGNED_BYTE: u32 = 0x1401;
    pub const SHORT: u32 = 0x1402;
    pub const UNSIGNED_SHORT: u32 = 0x1403;
    pub const UNSIGNED_INT: u32 = 0x1405;
    pub const FLOAT: u32 = 0x1406;
    pub const TEXTURE_2D: u32 = 0x0DE1;
    pub const TEXTURE0: u32 = 0x84C0;
    pub const TEXTURE1: u32 = 0x84C1;
    pub const TEXTURE2: u32 = 0x84C2;
    pub const TEXTURE3: u32 = 0x84C3;
    pub const TEXTURE4: u32 = 0x84C4;
    pub const TEXTURE5: u32 = 0x84C5;
    pub const TEXTURE6: u32 = 0x84C6;
    pub const TEXTURE7: u32 = 0x84C7;
    pub const TEXTURE_MIN_FILTER: u32 = 0x2801;
    pub const TEXTURE_MAG_FILTER: u32 = 0x2800;
    pub const TEXTURE_WRAP_S: u32 = 0x2802;
    pub const TEXTURE_WRAP_T: u32 = 0x2803;
    pub const TEXTURE_MAX_LEVEL: u32 = 0x813D;
    pub const TEXTURE_COMPARE_MODE: u32 = 0x884C;
    pub const TEXTURE_COMPARE_FUNC: u32 = 0x884D;
    pub const COMPARE_REF_TO_TEXTURE: i32 = 0x884E;
    pub const NEAREST: i32 = 0x2600;
    pub const LINEAR: i32 = 0x2601;
    pub const LINEAR_MIPMAP_LINEAR: i32 = 0x2703;
    pub const REPEAT: i32 = 0x2901;
    pub const CLAMP_TO_EDGE: i32 = 0x812F;
    pub const RGBA: u32 = 0x1908;
    pub const RGBA8: u32 = 0x8058;
    pub const SRGB8_ALPHA8: u32 = 0x8C43;
    pub const RGBA32F: u32 = 0x8814;
    pub const RGBA8UI: u32 = 0x8D7C;
    pub const RGBA_INTEGER: u32 = 0x8D99;
    pub const R32UI: u32 = 0x8236;
    pub const RED_INTEGER: u32 = 0x8D94;
    pub const DEPTH_COMPONENT32F: u32 = 0x8CAC;
    pub const DEPTH_COMPONENT16: u32 = 0x81A5;
    pub const DEPTH_COMPONENT: u32 = 0x1902;
    pub const FRAMEBUFFER: u32 = 0x8D40;
    pub const READ_FRAMEBUFFER: u32 = 0x8CA8;
    pub const DRAW_FRAMEBUFFER: u32 = 0x8CA9;
    pub const COLOR_ATTACHMENT0: u32 = 0x8CE0;
    pub const COLOR_ATTACHMENT1: u32 = 0x8CE1;
    pub const R8: u32 = 0x8229;
    pub const RED: u32 = 0x1903;
    pub const DEPTH_ATTACHMENT: u32 = 0x8D00;
    pub const FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
    pub const DEPTH_TEST: u32 = 0x0B71;
    pub const SCISSOR_TEST: u32 = 0x0C11;
    pub const BLEND: u32 = 0x0BE2;
    pub const CULL_FACE: u32 = 0x0B44;
    pub const SAMPLE_ALPHA_TO_COVERAGE: u32 = 0x809E;
    pub const BACK: u32 = 0x0405;
    pub const CW: u32 = 0x0900;
    pub const CCW: u32 = 0x0901;
    pub const LESS: u32 = 0x0201;
    pub const LEQUAL: u32 = 0x0203;
    pub const GREATER: u32 = 0x0204;
    pub const ALWAYS: u32 = 0x0207;
    pub const ZERO: u32 = 0;
    pub const ONE: u32 = 1;
    pub const ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
    pub const ONE_MINUS_DST_ALPHA: u32 = 0x0305;
    pub const ONE_MINUS_DST_COLOR: u32 = 0x0307;
}

/// The verbs the pass needs, each one GL call (or two, where a verb says
/// so). A tier implements them over its own border; every handle is the
/// tier's GL name, 0 for none.
pub trait GlFamily {
    /// What every stage opens with: the version, and for ES the precisions.
    fn prelude(&self) -> &'static str;
    /// Compiles and links a program, the attributes bound to their index
    /// in the list by name, and each uniform block to its binding. `Err`
    /// carries the device's complaint.
    fn program(&mut self, vertex: &str, fragment: &str, attributes: &[&str], blocks: &[(&str, u32)]) -> Result<u32, String>;
    fn uniform_location(&mut self, program: u32, name: &str) -> u32;
    fn use_program(&mut self, program: u32);
    fn uniform1i(&mut self, location: u32, value: i32);
    /// A `vec4` uniform, or an array of them from its first element.
    fn uniform4fv(&mut self, location: u32, values: &[[f32; 4]]);

    fn create_buffer(&mut self) -> u32;
    fn delete_buffer(&mut self, buffer: u32);
    fn bind_buffer(&mut self, target: u32, buffer: u32);
    /// A new store holding `bytes`.
    fn buffer_data(&mut self, target: u32, bytes: &[u8], usage: u32);
    fn bind_buffer_base(&mut self, target: u32, index: u32, buffer: u32);

    fn create_vertex_array(&mut self) -> u32;
    fn delete_vertex_array(&mut self, array: u32);
    fn bind_vertex_array(&mut self, array: u32);
    fn enable_vertex_attrib_array(&mut self, index: u32);
    fn vertex_attrib_pointer(&mut self, index: u32, size: i32, kind: u32, normalized: bool, stride: i32, offset: usize);
    /// An integer attribute: the shader reads the values as they are.
    fn vertex_attrib_i_pointer(&mut self, index: u32, size: i32, kind: u32, stride: i32, offset: usize);
    fn vertex_attrib_divisor(&mut self, index: u32, divisor: u32);

    fn create_texture(&mut self) -> u32;
    fn delete_texture(&mut self, texture: u32);
    fn active_texture(&mut self, unit: u32);
    fn bind_texture(&mut self, texture: u32);
    fn tex_parameteri(&mut self, name: u32, value: i32);
    /// One level of the bound 2D texture; empty `bytes` allocates it
    /// unfilled. Float data rides as its bytes.
    #[allow(clippy::too_many_arguments)]
    fn tex_image_2d(&mut self, level: i32, internal: u32, width: i32, height: i32, format: u32, kind: u32, bytes: &[u8]);

    fn create_framebuffer(&mut self) -> u32;
    fn delete_framebuffer(&mut self, framebuffer: u32);
    fn bind_framebuffer(&mut self, target: u32, framebuffer: u32);
    /// Level 0 of a 2D texture at `attachment` of the bound framebuffer.
    fn framebuffer_texture_2d(&mut self, attachment: u32, texture: u32);
    fn framebuffer_renderbuffer(&mut self, attachment: u32, renderbuffer: u32);
    fn check_framebuffer_status(&mut self) -> u32;
    fn create_renderbuffer(&mut self) -> u32;
    fn delete_renderbuffer(&mut self, renderbuffer: u32);
    /// Binds `renderbuffer` and gives it storage.
    fn renderbuffer_storage(&mut self, renderbuffer: u32, samples: i32, internal: u32, width: i32, height: i32);
    /// The colour of the read framebuffer onto the draw framebuffer, the
    /// same rectangle on both: how multisamples resolve.
    fn blit_color(&mut self, from: [i32; 4], to: [i32; 2]);
    /// The box the scissor test keeps, `[x, y, width, height]` in the
    /// framebuffer's pixels.
    fn scissor(&mut self, rect: [i32; 4]);
    /// The attachments' contents are no longer needed: a tiled GPU need
    /// not write them back. Up to two; 0 is none.
    fn invalidate_framebuffer(&mut self, target: u32, first: u32, second: u32);
    /// The bound framebuffer draws and reads no colour. Desktop GL 3.3
    /// counts a framebuffer of depth alone complete only once it is said;
    /// ES and WebGL2 need nothing.
    fn draw_no_color(&mut self) {}
    /// The bound draw framebuffer draws into its first `count` colour
    /// attachments: one, or two when a frame draws its glow.
    fn draw_buffers(&mut self, count: u32);
    /// The colour attachment the bound read framebuffer reads from.
    fn read_buffer(&mut self, attachment: u32);
    /// One colour attachment of the bound draw framebuffer, cleared.
    fn clear_color_buffer(&mut self, index: i32, color: [f32; 4]);
    /// The name the device's graphics answer to, when the tier can read
    /// it: the governor's start.
    fn renderer(&self) -> Option<String> {
        None
    }
    /// Whether the context times its GPU ([`GlFamily::begin_timer`]).
    fn has_timers(&self) -> bool {
        false
    }
    /// A timer of the GPU's time begins over the commands that follow:
    /// its handle, or 0 when the context keeps no timers.
    fn begin_timer(&mut self) -> u32 {
        0
    }
    /// The timer begun last ends after the commands before it.
    fn end_timer(&mut self) {}
    /// What a timer measured, read without waiting; a timer read done or
    /// lost is given back.
    fn read_timer(&mut self, timer: u32) -> TimerRead {
        let _ = timer;
        TimerRead::Lost
    }

    fn viewport(&mut self, width: i32, height: i32);
    fn enable(&mut self, capability: u32);
    fn disable(&mut self, capability: u32);
    fn depth_func(&mut self, function: u32);
    fn depth_mask(&mut self, write: bool);
    fn blend_func_separate(&mut self, source_rgb: u32, destination_rgb: u32, source_alpha: u32, destination_alpha: u32);
    fn cull_face(&mut self, face: u32);
    fn front_face(&mut self, winding: u32);
    /// The bound framebuffer's colour and depth, both cleared.
    fn clear(&mut self, color: [f32; 4], depth: f32);
    fn draw_arrays(&mut self, mode: u32, first: i32, count: i32);
    fn draw_arrays_instanced(&mut self, mode: u32, first: i32, count: i32, instances: i32);
    fn draw_elements_instanced(&mut self, mode: u32, count: i32, kind: u32, offset: usize, instances: i32);
    /// RGBA8 rows of the read framebuffer, from its first row.
    fn read_pixels(&mut self, width: i32, height: i32, out: &mut [u8]);
}

/// The uniform block the world's frame binds to; the 2D pass keeps 0 and 1.
pub const WORLD_BINDING: u32 = 2;

/// The mesh program's attributes, in index order.
const MESH_ATTRIBUTES: [&str; 14] = [
    "a_position", "a_normal", "a_uv", "a_color", "a_ids", "i_row0", "i_row1", "i_row2", "i_normal0", "i_normal1", "i_normal2", "i_tint",
    "i_ids", "i_more",
];

/// How many attributes an instance carries.
const INSTANCE_ATTRIBUTES: u32 = 9;

/// The first index of the instance attributes.
const INSTANCE_ATTRIBUTE: u32 = 5;

#[derive(Clone, Copy)]
struct Program {
    handle: u32,
    draw: u32,
    rows: u32,
    style: u32,
    ramp: u32,
    bloom: u32,
}

#[derive(Clone, Copy)]
struct Programs {
    mesh: Program,
    stepped: Program,
    lines: Program,
    backdrop: Program,
    /// The bloom's steps: down, back up, and the spill over the target.
    bloom_down: Program,
    bloom_up: Program,
    bloom_spill: Program,
    /// The 2D pass's sprite for a world's target.
    composite: u32,
    /// The casters into a shadow map.
    shadow: Program,
    frame: u32,
    instances: u32,
    /// The casters' rows.
    casters: u32,
    materials: u32,
    /// The frame's part table: four unsigned bytes a part.
    parts: u32,
    /// The frame's palettes: three RGBA32F texels a joint.
    joints: u32,
    /// One word: what a mesh without a skin binds for its deform stream.
    no_deform: u32,
    blank: u32,
    /// A map of one texel whose comparison always passes: a frame without
    /// a map reads full sun.
    lit: u32,
    /// One black, opaque texel: a floor map that adds no light and takes
    /// none away, bound while a frame's floor map is not decoded.
    dark: u32,
    empty: u32,
}

enum Built {
    Not,
    Ready(Box<Programs>),
    /// A program the device refused: worlds stay on the CPU.
    Failed,
}

#[derive(Clone, Copy)]
struct Target {
    texture: u32,
    /// The framebuffer of the texture (and, at one sample, the depth).
    resolve: u32,
    /// The multisample framebuffer, or 0 at one sample.
    multisample: u32,
    color: u32,
    depth: u32,
    size: (u32, u32),
}

struct MeshObjects {
    array: u32,
    vertices: u32,
    indices: u32,
    /// The deform stream as an R32UI texture (1024 words a row), or 0.
    deform: u32,
}

struct SegmentObjects {
    array: u32,
    buffer: u32,
    count: u32,
}

/// A target's bloom: its glow, and the chain's textures.
#[derive(Clone, Copy)]
struct BloomObjects {
    /// The glow at one sample, and a framebuffer of it alone: where the
    /// samples resolve.
    glow: u32,
    glow_framebuffer: u32,
    /// The glow's samples, or 0 when the target takes one.
    glow_samples: u32,
    /// The target's texture alone: what the spill draws into.
    spill_framebuffer: u32,
    /// The steps down and back up, each a texture and its framebuffer;
    /// `up[k]` is the size of `down[k]`.
    down: [(u32, u32); LEVELS],
    up: [(u32, u32); LEVELS - 1],
    sizes: [(u32, u32); LEVELS],
}

#[derive(Clone, Copy)]
struct ShadowObjects {
    texture: u32,
    framebuffer: u32,
    size: u32,
}

/// The world's GL objects, and the door the walk opens.
/// What a GPU timer gave ([`GlFamily::read_timer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerRead {
    /// The GPU has not finished the commands it times.
    Pending,
    /// The measure was lost: the clock jumped, or the timer is unknown.
    Lost,
    Nanos(u64),
}

/// Timers a world pass keeps waiting before it gives the oldest up.
const TIMERS_HELD: usize = 32;

pub struct GlWorld<G: GlFamily> {
    pub gl: G,
    built: Built,
    /// Each job's timer, by target, until the GPU has run it.
    timers: Vec<(u64, u32)>,
    /// What the timers measured, for the residency's next ask.
    timings: Vec<WorldTiming>,
    targets: HashMap<u64, Target>,
    meshes: HashMap<u64, MeshObjects>,
    textures: HashMap<u64, u32>,
    segments: HashMap<u64, SegmentObjects>,
    shadow_maps: HashMap<u64, ShadowObjects>,
    /// By target: made on the first frame with a bloom.
    blooms: HashMap<u64, BloomObjects>,
    next: u64,
    /// The 2D pass's blend factors, put back when the world pass ends.
    blend_2d: [u32; 4],
    samples: u8,
}

impl<G: GlFamily> GlWorld<G> {
    /// `blend_2d` is the 2D pass's `(source, destination)` factors for
    /// colour and alpha; `samples` the most samples a target may take.
    pub fn new(gl: G, blend_2d: [u32; 4], samples: u8) -> GlWorld<G> {
        GlWorld {
            gl,
            built: Built::Not,
            timers: Vec::new(),
            timings: Vec::new(),
            targets: HashMap::new(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            segments: HashMap::new(),
            shadow_maps: HashMap::new(),
            blooms: HashMap::new(),
            next: 1,
            blend_2d,
            samples: samples.clamp(1, 4),
        }
    }

    /// The door, once the programs stand: built on the first ask, and
    /// never asked again when the device refused one.
    pub fn door(&mut self) -> Option<&mut dyn WorldGround> {
        if let Built::Not = self.built {
            self.built = match self.build() {
                Ok(programs) => Built::Ready(Box::new(programs)),
                Err(_) => Built::Failed,
            };
        }
        match self.built {
            Built::Ready(_) => Some(self),
            _ => None,
        }
    }

    /// The device's complaint, when the programs did not build.
    pub fn failed(&self) -> bool {
        matches!(self.built, Built::Failed)
    }

    /// The context is gone, and every name with it: nothing is deleted,
    /// everything is made again.
    pub fn forget_all(&mut self) {
        self.built = Built::Not;
        self.targets.clear();
        self.meshes.clear();
        self.textures.clear();
        self.segments.clear();
        self.shadow_maps.clear();
        self.blooms.clear();
        self.timers.clear();
        self.timings.clear();
    }

    fn mint(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn build(&mut self) -> Result<Programs, String> {
        let gl = &mut self.gl;
        let prelude = gl.prelude();
        let table = source::srgb_table();
        let world = [("World", WORLD_BINDING)];
        let mesh_vertex = format!("{prelude}{}{table}{}{}{}", source::WORLD_COMMON, source::PARTS, source::SKIN, source::MESH_VERT);
        let mesh_fragment = |body: &str| format!("{prelude}{}{}{}{body}", source::WORLD_COMMON, source::PARTS, source::MESH_FRAG_COMMON);
        let mesh = gl.program(&mesh_vertex, &mesh_fragment(source::MESH_FRAG), &MESH_ATTRIBUTES, &world)?;
        let stepped = gl.program(&mesh_vertex, &mesh_fragment(source::MESH_FRAG_STEPPED), &MESH_ATTRIBUTES, &world)?;
        let lines = gl.program(
            &format!("{prelude}{}{}", source::WORLD_COMMON, source::LINE_VERT),
            &format!("{prelude}{}{}", source::WORLD_COMMON, source::LINE_FRAG),
            &["a_from", "a_to"],
            &world,
        )?;
        let backdrop = gl.program(&format!("{prelude}{}", source::BACKDROP_VERT), &format!("{prelude}{}", source::BACKDROP_FRAG), &[], &[])?;
        let step = |gl: &mut G, body: &str| {
            gl.program(&format!("{prelude}{}", source::BACKDROP_VERT), &format!("{prelude}{}{body}", source::BLOOM_COMMON), &[], &[])
        };
        let bloom_down = step(gl, source::BLOOM_DOWN_FRAG)?;
        let bloom_up = step(gl, source::BLOOM_UP_FRAG)?;
        let bloom_spill = step(gl, source::BLOOM_SPILL_FRAG)?;
        let shadow = gl.program(
            &format!("{prelude}{}{}{}{}", source::WORLD_COMMON, source::PARTS, source::SKIN, source::SHADOW_VERT),
            &format!("{prelude}{}", source::SHADOW_FRAG),
            &MESH_ATTRIBUTES,
            &world,
        )?;
        let composite = gl.program(
            &format!("{prelude}{}", super::shaders::SPRITE_VERT),
            &format!("{prelude}{}{}", super::shaders::SHARED_FRAG, source::COMPOSITE_FRAG_BODY),
            &["a_dest", "a_tex", "a_clip"],
            &[("Frame", 0), ("Round", 1)],
        )?;
        let program = |gl: &mut G, handle: u32| {
            gl.use_program(handle);
            let mut located = Program { handle, draw: 0, rows: 0, style: 0, ramp: 0, bloom: 0 };
            located.draw = gl.uniform_location(handle, "u_draw");
            located.rows = gl.uniform_location(handle, "u_rows");
            located.style = gl.uniform_location(handle, "u_style");
            located.ramp = gl.uniform_location(handle, "u_ramp");
            located.bloom = gl.uniform_location(handle, "u_bloom");
            let units = [
                ("u_materials", 0),
                ("u_surface", 1),
                ("u_shadow", 2),
                ("u_floor", 3),
                ("u_parts", 4),
                ("u_joints", 5),
                ("u_deform", 6),
                ("u_moving", 7),
                ("atlas", 0),
                ("u_above", 0),
                ("u_second", 1),
            ];
            for (name, unit) in units {
                let at = gl.uniform_location(handle, name);
                if at != 0 {
                    gl.uniform1i(at, unit);
                }
            }
            located
        };
        let mesh = program(gl, mesh);
        let stepped = program(gl, stepped);
        let lines = program(gl, lines);
        let backdrop = program(gl, backdrop);
        let bloom_down = program(gl, bloom_down);
        let bloom_up = program(gl, bloom_up);
        let bloom_spill = program(gl, bloom_spill);
        let composite = program(gl, composite).handle;
        let shadow = program(gl, shadow);
        gl.use_program(0);
        let blank = gl.create_texture();
        gl.active_texture(gl::TEXTURE0);
        gl.bind_texture(blank);
        gl.tex_image_2d(0, gl::SRGB8_ALPHA8, 1, 1, gl::RGBA, gl::UNSIGNED_BYTE, &[255, 255, 255, 255]);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
        let materials = gl.create_texture();
        gl.bind_texture(materials);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
        let parts = gl.create_texture();
        gl.bind_texture(parts);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
        let joints = gl.create_texture();
        gl.bind_texture(joints);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
        let no_deform = gl.create_texture();
        gl.bind_texture(no_deform);
        gl.tex_image_2d(0, gl::R32UI, 1, 1, gl::RED_INTEGER, gl::UNSIGNED_INT, &[0; 4]);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
        let dark = gl.create_texture();
        gl.bind_texture(dark);
        gl.tex_image_2d(0, gl::SRGB8_ALPHA8, 1, 1, gl::RGBA, gl::UNSIGNED_BYTE, &[0, 0, 0, 255]);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
        // its texel is never written: a comparison that always passes
        let lit = gl.create_texture();
        gl.bind_texture(lit);
        gl.tex_image_2d(0, gl::DEPTH_COMPONENT16, 1, 1, gl::DEPTH_COMPONENT, gl::UNSIGNED_SHORT, &[]);
        compare_with(gl, gl::ALWAYS, gl::NEAREST);
        gl.bind_texture(0);
        Ok(Programs {
            mesh,
            stepped,
            lines,
            backdrop,
            bloom_down,
            bloom_up,
            bloom_spill,
            composite,
            shadow,
            frame: gl.create_buffer(),
            instances: gl.create_buffer(),
            casters: gl.create_buffer(),
            materials,
            parts,
            joints,
            no_deform,
            blank,
            lit,
            dark,
            empty: gl.create_vertex_array(),
        })
    }

    /// The program the 2D pass composites a world's target with, and the
    /// target's texture for the world run `handle` names. `None` while
    /// there is nothing to composite.
    pub fn composite(&self, handle: u64) -> Option<(u32, u32)> {
        let Built::Ready(programs) = &self.built else { return None };
        let target = self.targets.get(&handle)?;
        Some((programs.composite, target.texture))
    }

    /// Draws every job into its target. Call it after the walk and before
    /// the 2D pass; the 2D pass's state is put back at the end.
    pub fn encode(&mut self, jobs: &[WorldJob]) {
        let Built::Ready(programs) = &self.built else { return };
        let programs = **programs;
        if jobs.is_empty() {
            return;
        }
        self.read_timers();
        for job in jobs {
            // the GPU's time over the job: its shadows, its pass, its bloom
            let timer = self.gl.begin_timer();
            self.encode_job(&programs, job);
            if timer != 0 {
                self.gl.end_timer();
                self.timers.push((job.target, timer));
            }
        }
        // a context that never finishes its timers keeps no more of them
        while self.timers.len() > TIMERS_HELD {
            let (_, timer) = self.timers.remove(0);
            let _ = self.gl.read_timer(timer);
        }
        let gl = &mut self.gl;
        gl.bind_vertex_array(0);
        gl.use_program(0);
        gl.disable(gl::DEPTH_TEST);
        gl.depth_mask(true);
        gl.disable(gl::CULL_FACE);
        gl.disable(gl::SAMPLE_ALPHA_TO_COVERAGE);
        gl.front_face(gl::CCW);
        gl.enable(gl::BLEND);
        let [a, b, c, d] = self.blend_2d;
        gl.blend_func_separate(a, b, c, d);
        for unit in [gl::TEXTURE7, gl::TEXTURE6, gl::TEXTURE5, gl::TEXTURE4, gl::TEXTURE3, gl::TEXTURE2, gl::TEXTURE1, gl::TEXTURE0] {
            gl.active_texture(unit);
            gl.bind_texture(0);
        }
        gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
    }

    /// The timers the GPU has run, read without waiting, into the timings
    /// the residency asks for.
    fn read_timers(&mut self) {
        let GlWorld { gl, timers, timings, .. } = self;
        timers.retain(|&(target, timer)| match gl.read_timer(timer) {
            TimerRead::Pending => true,
            TimerRead::Lost => false,
            TimerRead::Nanos(nanos) => {
                timings.push(WorldTiming { target, seconds: nanos as f64 * 1e-9 });
                false
            }
        });
    }

    /// The casters of one of the frame's shadows into the box's map, from
    /// the sun: both faces, the nearest depth kept.
    fn draw_shadow(&mut self, programs: &Programs, job: &WorldJob, map: ShadowObjects, shadow: &ShadowFrame) {
        let gl = &mut self.gl;
        // nothing samples a map while it is drawn
        gl.active_texture(gl::TEXTURE2);
        gl.bind_texture(0);
        gl.active_texture(gl::TEXTURE7);
        gl.bind_texture(0);
        gl.bind_framebuffer(gl::FRAMEBUFFER, map.framebuffer);
        gl.viewport(map.size as i32, map.size as i32);
        gl.enable(gl::DEPTH_TEST);
        gl.depth_mask(true);
        gl.depth_func(gl::LESS);
        gl.disable(gl::BLEND);
        gl.disable(gl::CULL_FACE);
        gl.clear([0.0; 4], 1.0);
        gl.use_program(programs.shadow.handle);
        gl.bind_buffer(gl::ARRAY_BUFFER, programs.casters);
        gl.buffer_data(gl::ARRAY_BUFFER, &shadow_instance_bytes(&shadow.instances), gl::STREAM_DRAW);
        for draw in &shadow.draws {
            let Some(objects) = job.meshes.get(draw.mesh as usize).and_then(|id| self.meshes.get(id)) else { continue };
            // how the caster's mesh lays out its morphs and its skin
            let deform = job.frame.meshes.get(draw.mesh as usize).map_or(0, |mesh| mesh.deform_flags());
            let gl = &mut self.gl;
            gl.uniform4fv(programs.shadow.draw, &[[deform as f32, 0.0, 0.0, 0.0]]);
            gl.active_texture(gl::TEXTURE6);
            gl.bind_texture(if objects.deform != 0 { objects.deform } else { programs.no_deform });
            gl.bind_vertex_array(objects.array);
            gl.bind_buffer(gl::ARRAY_BUFFER, programs.casters);
            point_casters(gl, draw.first_instance as usize * SHADOW_INSTANCE_STRIDE);
            gl.draw_elements_instanced(gl::TRIANGLES, draw.index_count as i32, gl::UNSIGNED_INT, draw.first_index as usize * 4, draw.instance_count as i32);
        }
    }

    fn encode_job(&mut self, programs: &Programs, job: &WorldJob) {
        let Some(target) = self.targets.get(&job.target).copied() else { return };
        let frame = &job.frame;
        let bloom = if frame.bloom > 0 { self.bloom_of(job.target, &target) } else { None };
        // the frame's block, which every program of the pass declares
        let gl = &mut self.gl;
        gl.bind_buffer(gl::UNIFORM_BUFFER, programs.frame);
        gl.buffer_data(gl::UNIFORM_BUFFER, &frame.uniforms.to_bytes(), gl::STREAM_DRAW);
        gl.bind_buffer_base(gl::UNIFORM_BUFFER, WORLD_BINDING, programs.frame);
        // the part table, which the casters read too: 1024 parts a row,
        // and one row at least, so the unit always holds a texture
        let count = frame.parts.len().max(1);
        let (columns, rows) = (count.min(1024), count.div_ceil(1024));
        let mut table = crate::world::wire::part_bytes(&frame.parts);
        table.resize(columns * rows * 4, 255);
        gl.active_texture(gl::TEXTURE4);
        gl.bind_texture(programs.parts);
        gl.tex_image_2d(0, gl::RGBA8UI, columns as i32, rows as i32, gl::RGBA_INTEGER, gl::UNSIGNED_BYTE, &table);
        // the palettes, which the casters read too: 1024 rows a line
        let count = frame.joints.len().max(1);
        let (columns, rows) = (count.min(1024), count.div_ceil(1024));
        let mut palette: Vec<u8> = frame.joints.iter().flatten().flat_map(|value| value.to_le_bytes()).collect();
        palette.resize(columns * rows * 16, 0);
        gl.active_texture(gl::TEXTURE5);
        gl.bind_texture(programs.joints);
        gl.tex_image_2d(0, gl::RGBA32F, columns as i32, rows as i32, gl::RGBA, gl::FLOAT, &palette);
        // the sun's maps first, when they do not hold this frame's casters
        let map = job.shadow.and_then(|shadow| self.shadow_maps.get(&shadow.map).copied().map(|map| (shadow, map)));
        if let (Some((shadow, map)), Some(casters)) = (map, frame.shadow.as_ref())
            && shadow.draw
        {
            self.draw_shadow(programs, job, map, casters);
        }
        let moving = job.moving_shadow.and_then(|shadow| self.shadow_maps.get(&shadow.map).copied().map(|map| (shadow, map)));
        if let (Some((shadow, map)), Some(casters)) = (moving, frame.moving_shadow.as_ref())
            && shadow.draw
        {
            self.draw_shadow(programs, job, map, casters);
        }
        let gl = &mut self.gl;
        let (width, height) = (target.size.0 as i32, target.size.1 as i32);

        // what every draw of the frame reads
        gl.bind_buffer(gl::ARRAY_BUFFER, programs.instances);
        gl.buffer_data(gl::ARRAY_BUFFER, &instance_bytes(&frame.instances), gl::STREAM_DRAW);
        // the material table: four RGBA32F texels a material, 1024 a row
        let count = frame.materials.len().max(1);
        let (columns, rows) = (count.min(1024), count.div_ceil(1024));
        let mut table = material_bytes(&frame.materials);
        table.resize(columns * rows * 64, 0);
        gl.active_texture(gl::TEXTURE0);
        gl.bind_texture(programs.materials);
        gl.tex_image_2d(0, gl::RGBA32F, (columns * 4) as i32, rows as i32, gl::RGBA, gl::FLOAT, &table);
        gl.active_texture(gl::TEXTURE2);
        gl.bind_texture(map.map_or(programs.lit, |(_, map)| map.texture));
        gl.active_texture(gl::TEXTURE7);
        gl.bind_texture(moving.map_or(programs.lit, |(_, map)| map.texture));
        let floor = frame.floor.and_then(|index| job.textures.get(index as usize).copied().flatten()).and_then(|id| self.textures.get(&id).copied());
        let gl = &mut self.gl;
        gl.active_texture(gl::TEXTURE3);
        gl.bind_texture(floor.unwrap_or(programs.dark));
        gl.active_texture(gl::TEXTURE1);
        gl.bind_texture(programs.blank);

        // the target, cleared to the backdrop, and its glow to none: all of
        // it, or each rect the frame asks for, the rest kept — the scissor
        // holds the clear and the draws inside the rect
        gl.bind_framebuffer(gl::FRAMEBUFFER, if target.multisample != 0 { target.multisample } else { target.resolve });
        gl.draw_buffers(if bloom.is_some() { 2 } else { 1 });
        gl.viewport(width, height);
        gl.enable(gl::DEPTH_TEST);
        gl.disable(gl::BLEND);
        gl.disable(gl::CULL_FACE);
        let clear = match frame.background {
            Background::Color(c) => {
                let premultiply = |channel: u8| ((channel as u32 * c.a as u32 + 127) / 255) as f32 / 255.0;
                [premultiply(c.r), premultiply(c.g), premultiply(c.b), c.a as f32 / 255.0]
            }
            Background::Transparent | Background::Gradient { .. } | Background::Sky { .. } => [0.0; 4],
        };
        // a slide first: the picture the box showed, moved into this
        // target by the pan's step, which the rects then draw over
        if let Redraw::Slide { from, shift, .. } = &job.redraw
            && let Some(from) = self.targets.get(from).copied()
            && let Some((source, to)) = slide_copy(*shift, target.size)
        {
            let gl = &mut self.gl;
            gl.bind_framebuffer(gl::READ_FRAMEBUFFER, from.resolve);
            gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, target.resolve);
            gl.blit_color(source.map(|value| value as i32), to.map(|value| value as i32));
            gl.bind_framebuffer(gl::FRAMEBUFFER, if target.multisample != 0 { target.multisample } else { target.resolve });
        }
        let gl = &mut self.gl;
        let (rects, partial) = match job.redraw.rects() {
            None => (vec![[0, 0, width as u32, height as u32]], false),
            Some(rects) => (rects.to_vec(), true),
        };
        if partial {
            gl.enable(gl::SCISSOR_TEST);
        }
        let (near, far) = (frame.near as f32, frame.far as f32);
        for &rect in &rects {
            let gl = &mut self.gl;
            if partial {
                gl.scissor(rect.map(|value| value as i32));
            }
            gl.depth_mask(true);
            gl.clear(clear, 1.0);
            if bloom.is_some() {
                gl.clear_color_buffer(1, [0.0; 4]);
            }
            if let Some(ramp) = frame.backdrop() {
                gl.use_program(programs.backdrop.handle);
                gl.uniform4fv(programs.backdrop.ramp, &ramp);
                gl.depth_func(gl::ALWAYS);
                gl.depth_mask(false);
                gl.bind_vertex_array(programs.empty);
                gl.draw_arrays(gl::TRIANGLES, 0, 3);
            }

            for draw in &frame.draws {
                if partial && !frame.draw_reaches(draw, rect) {
                    continue;
                }
                match draw.kind {
                    DrawKind::Mesh { mesh, first_index, index_count, texture, cull_back, mirrored, blend, mask } => {
                        let Some(objects) = job.meshes.get(mesh as usize).and_then(|id| self.meshes.get(id)) else { continue };
                        let surface = texture
                            .and_then(|index| job.textures.get(index as usize).copied().flatten())
                            .and_then(|id| self.textures.get(&id).copied());
                        let replace = blend == Blend::Replace;
                        let bias = if replace { frame.face_bias } else { 0.0 };
                        let program = if bias > 0.0 { programs.stepped } else { programs.mesh };
                        let mut flags = 0u32;
                        if mask {
                            flags |= 1;
                        }
                        // with four samples a cut-out's edge becomes coverage
                        let coverage = mask && target.multisample != 0;
                        if coverage {
                            flags |= 8;
                        }
                        if surface.is_some() {
                            flags |= 2;
                        }
                        if replace {
                            flags |= 4;
                        }
                        // how the mesh lays out its morphs and its skin
                        flags |= frame.meshes.get(mesh as usize).map_or(0, |mesh| mesh.deform_flags());
                        let gl = &mut self.gl;
                        gl.use_program(program.handle);
                        gl.uniform4fv(program.draw, &[[flags as f32, near, far, bias]]);
                        set_blend(gl, blend);
                        if coverage {
                            gl.enable(gl::SAMPLE_ALPHA_TO_COVERAGE);
                        } else {
                            gl.disable(gl::SAMPLE_ALPHA_TO_COVERAGE);
                        }
                        gl.depth_func(gl::LESS);
                        gl.depth_mask(replace);
                        if cull_back {
                            gl.enable(gl::CULL_FACE);
                            gl.cull_face(gl::BACK);
                        } else {
                            gl.disable(gl::CULL_FACE);
                        }
                        // the picture is upside down in clip space: a front that
                        // turns counter-clockwise turns clockwise on the target
                        gl.front_face(if mirrored { gl::CCW } else { gl::CW });
                        gl.active_texture(gl::TEXTURE1);
                        gl.bind_texture(surface.unwrap_or(programs.blank));
                        gl.active_texture(gl::TEXTURE6);
                        gl.bind_texture(if objects.deform != 0 { objects.deform } else { programs.no_deform });
                        gl.bind_vertex_array(objects.array);
                        gl.bind_buffer(gl::ARRAY_BUFFER, programs.instances);
                        point_instances(gl, draw.first_instance as usize * crate::world::wire::instance_offset::SIZE);
                        gl.draw_elements_instanced(gl::TRIANGLES, index_count as i32, gl::UNSIGNED_INT, first_index as usize * 4, draw.instance_count as i32);
                    }
                    DrawKind::Lines { lines, style } => {
                        let Some(objects) = job.segments.get(lines as usize).and_then(|id| self.segments.get(id)) else { continue };
                        let (Some(style), Some(instance)) = (frame.line_styles.get(style as usize), frame.instances.get(draw.first_instance as usize)) else {
                            continue;
                        };
                        if objects.count == 0 {
                            continue;
                        }
                        let gl = &mut self.gl;
                        gl.use_program(programs.lines.handle);
                        gl.uniform4fv(programs.lines.draw, &[[0.0, near, far, 0.0]]);
                        gl.uniform4fv(programs.lines.rows, &instance.eye_from_model);
                        gl.uniform4fv(programs.lines.style, &[style.color, style.shape]);
                        set_blend(gl, Blend::Alpha);
                        gl.depth_func(match style.shape[3] as u32 {
                            0 => gl::LEQUAL,
                            1 => gl::GREATER,
                            _ => gl::ALWAYS,
                        });
                        gl.depth_mask(false);
                        gl.disable(gl::CULL_FACE);
                        gl.bind_vertex_array(objects.array);
                        gl.draw_arrays_instanced(gl::TRIANGLES, 0, 6, objects.count as i32);
                    }
                }
            }
        }

        // the samples into the texture — the rects alone, with the
        // scissor off, which a blit would heed — and nothing written back
        // that nobody reads
        let gl = &mut self.gl;
        if partial {
            gl.disable(gl::SCISSOR_TEST);
        }
        if target.multisample != 0 {
            gl.bind_framebuffer(gl::READ_FRAMEBUFFER, target.multisample);
            gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, target.resolve);
            for &rect in &rects {
                gl.blit_color(rect.map(|value| value as i32), [rect[0] as i32, rect[1] as i32]);
            }
            if let Some(bloom) = bloom {
                gl.read_buffer(gl::COLOR_ATTACHMENT1);
                gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, bloom.glow_framebuffer);
                gl.blit_color([0, 0, width, height], [0, 0]);
                gl.read_buffer(gl::COLOR_ATTACHMENT0);
                gl.invalidate_framebuffer(gl::READ_FRAMEBUFFER, gl::COLOR_ATTACHMENT1, 0);
            }
            gl.invalidate_framebuffer(gl::READ_FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::DEPTH_ATTACHMENT);
        } else {
            gl.invalidate_framebuffer(gl::FRAMEBUFFER, gl::DEPTH_ATTACHMENT, 0);
        }
        if let Some(bloom) = bloom {
            self.draw_bloom(programs, &target, &bloom, frame.bloom);
        }
    }

    /// The target's bloom objects, made on its first frame with a bloom:
    /// the glow beside the colour (its samples on the target's samples),
    /// and the chain's textures. `None` when a framebuffer will not stand.
    fn bloom_of(&mut self, id: u64, target: &Target) -> Option<BloomObjects> {
        if let Some(bloom) = self.blooms.get(&id) {
            return Some(*bloom);
        }
        let gl = &mut self.gl;
        let (w, h) = (target.size.0 as i32, target.size.1 as i32);
        let texture = |gl: &mut G, internal: u32, format: u32, (w, h): (i32, i32)| {
            let texture = gl.create_texture();
            gl.active_texture(gl::TEXTURE0);
            gl.bind_texture(texture);
            gl.tex_image_2d(0, internal, w, h, format, gl::UNSIGNED_BYTE, &[]);
            gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
            gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
            gl.tex_parameteri(gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
            gl.tex_parameteri(gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
            gl.bind_texture(0);
            texture
        };
        let alone = |gl: &mut G, texture: u32| {
            let framebuffer = gl.create_framebuffer();
            gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer);
            gl.framebuffer_texture_2d(gl::COLOR_ATTACHMENT0, texture);
            let complete = gl.check_framebuffer_status() == gl::FRAMEBUFFER_COMPLETE;
            (framebuffer, complete)
        };
        let glow = texture(gl, gl::R8, gl::RED, (w, h));
        let (glow_framebuffer, mut complete) = alone(gl, glow);
        let (spill_framebuffer, spill) = alone(gl, target.texture);
        complete &= spill;
        let sizes = crate::world::bloom::sizes(target.size);
        let mut down = [(0, 0); LEVELS];
        let mut up = [(0, 0); LEVELS - 1];
        for (level, size) in sizes.iter().enumerate() {
            let size = (size.0 as i32, size.1 as i32);
            let step = texture(gl, gl::RGBA8, gl::RGBA, size);
            let (framebuffer, ok) = alone(gl, step);
            down[level] = (step, framebuffer);
            complete &= ok;
            if level < LEVELS - 1 {
                let step = texture(gl, gl::RGBA8, gl::RGBA, size);
                let (framebuffer, ok) = alone(gl, step);
                up[level] = (step, framebuffer);
                complete &= ok;
            }
        }
        // the glow beside the colour: samples of its own, or the texture
        let mut glow_samples = 0;
        if target.multisample != 0 {
            glow_samples = gl.create_renderbuffer();
            gl.renderbuffer_storage(glow_samples, self.samples as i32, gl::R8, w, h);
            gl.bind_framebuffer(gl::FRAMEBUFFER, target.multisample);
            gl.framebuffer_renderbuffer(gl::COLOR_ATTACHMENT1, glow_samples);
        } else {
            gl.bind_framebuffer(gl::FRAMEBUFFER, target.resolve);
            gl.framebuffer_texture_2d(gl::COLOR_ATTACHMENT1, glow);
        }
        gl.draw_buffers(2);
        complete &= gl.check_framebuffer_status() == gl::FRAMEBUFFER_COMPLETE;
        gl.draw_buffers(1);
        gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        let bloom = BloomObjects { glow, glow_framebuffer, glow_samples, spill_framebuffer, down, up, sizes };
        if !complete {
            release_bloom(gl, &bloom);
            return None;
        }
        self.blooms.insert(id, bloom);
        Some(bloom)
    }

    /// The chain over a resolved target: four steps down from its colours
    /// and glow, three back up, and the spill blended over the target as a
    /// screen — `c + s·(1 − c)`, alpha by the brightest channel.
    fn draw_bloom(&mut self, programs: &Programs, target: &Target, bloom: &BloomObjects, strength: u16) {
        let gl = &mut self.gl;
        gl.disable(gl::DEPTH_TEST);
        gl.disable(gl::BLEND);
        gl.disable(gl::CULL_FACE);
        gl.disable(gl::SAMPLE_ALPHA_TO_COVERAGE);
        gl.bind_vertex_array(programs.empty);
        let step = |gl: &mut G, program: &Program, framebuffer: u32, size: (u32, u32), above: u32, second: u32, values: [f32; 4]| {
            gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer);
            gl.viewport(size.0 as i32, size.1 as i32);
            gl.use_program(program.handle);
            gl.uniform4fv(program.bloom, &[values]);
            gl.active_texture(gl::TEXTURE0);
            gl.bind_texture(above);
            gl.active_texture(gl::TEXTURE1);
            gl.bind_texture(second);
            gl.draw_arrays(gl::TRIANGLES, 0, 3);
        };
        // down: the glowing colours first, then each step from the one above
        step(gl, &programs.bloom_down, bloom.down[0].1, bloom.sizes[0], target.texture, bloom.glow, [1.0, 0.0, 0.0, 0.0]);
        for level in 1..LEVELS {
            let above = bloom.down[level - 1].0;
            step(gl, &programs.bloom_down, bloom.down[level].1, bloom.sizes[level], above, above, [0.0; 4]);
        }
        // up: each step its own step down and the step below read up
        let mut lower = bloom.down[LEVELS - 1].0;
        for (below, level) in (0..LEVELS - 1).rev().enumerate() {
            let carried = (below + 1) as f32;
            step(gl, &programs.bloom_up, bloom.up[level].1, bloom.sizes[level], bloom.down[level].0, lower, [0.0, 0.0, carried, 0.0]);
            lower = bloom.up[level].0;
        }
        // the spill, a screen over the target
        gl.enable(gl::BLEND);
        gl.blend_func_separate(gl::ONE_MINUS_DST_COLOR, gl::ONE, gl::ONE_MINUS_DST_ALPHA, gl::ONE);
        step(gl, &programs.bloom_spill, bloom.spill_framebuffer, target.size, bloom.glow, lower, [0.0, strength as f32, 0.0, 0.0]);
        gl.disable(gl::BLEND);
    }

    /// Draws one frame by itself into a target of its own and reads it
    /// back: premultiplied, sRGB-encoded RGBA8, the top row first — what
    /// the CPU's `soft::draw` gives for the same frame. For the parity
    /// harness; `None` when the pass cannot stand.
    pub fn draw_single(&mut self, frame: &Rc<WorldFrame>, images: &dyn ImageEngine) -> Option<Vec<u8>> {
        let job = WorldJob::single(self.door()?, frame, images)?;
        self.encode(std::slice::from_ref(&job));
        let resolve = self.targets.get(&job.target)?.resolve;
        let mut out = vec![0u8; frame.size.0 as usize * frame.size.1 as usize * 4];
        self.gl.bind_framebuffer(gl::FRAMEBUFFER, resolve);
        self.gl.read_pixels(frame.size.0 as i32, frame.size.1 as i32, &mut out);
        self.gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        job.give_back(self);
        Some(out)
    }

    /// `before` drawn whole into a target of its own, then `after` over it
    /// as a box draws its next frame — slid by `shift`, then `rects` drawn
    /// again ([`WorldJob::over`]) — read back as [`GlWorld::draw_single`]
    /// reads: what `after` drawn whole gives, when the redraw is right. For
    /// the parity harness.
    pub fn draw_over(
        &mut self,
        before: &Rc<WorldFrame>,
        after: &Rc<WorldFrame>,
        shift: [i32; 2],
        rects: &[[u32; 4]],
        images: &dyn ImageEngine,
    ) -> Option<Vec<u8>> {
        let under = WorldJob::single(self.door()?, before, images)?;
        self.encode(std::slice::from_ref(&under));
        let over = WorldJob::over(self.door()?, &under, after, images, shift, rects.to_vec())?;
        self.encode(std::slice::from_ref(&over));
        let resolve = self.targets.get(&over.target)?.resolve;
        let mut out = vec![0u8; after.size.0 as usize * after.size.1 as usize * 4];
        self.gl.bind_framebuffer(gl::FRAMEBUFFER, resolve);
        self.gl.read_pixels(after.size.0 as i32, after.size.1 as i32, &mut out);
        self.gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        over.give_back_over(self);
        under.give_back(self);
        Some(out)
    }
}

/// How a draw's colour meets the target — `soft`'s `Target::blend`, in
/// factors: over is `s + d·(1−sa)`, add is `s + d` with the alpha of over.
fn set_blend<G: GlFamily>(gl: &mut G, blend: Blend) {
    match blend {
        Blend::Replace => gl.disable(gl::BLEND),
        Blend::Alpha => {
            gl.enable(gl::BLEND);
            gl.blend_func_separate(gl::ONE, gl::ONE_MINUS_SRC_ALPHA, gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
        }
        Blend::Add => {
            gl.enable(gl::BLEND);
            gl.blend_func_separate(gl::ONE, gl::ONE, gl::ONE_MINUS_DST_ALPHA, gl::ONE);
        }
    }
}

/// The comparison a depth texture answers a sampler with, and its filter.
fn compare_with<G: GlFamily>(gl: &mut G, function: u32, filter: i32) {
    gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, filter);
    gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, filter);
    gl.tex_parameteri(gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
    gl.tex_parameteri(gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
    gl.tex_parameteri(gl::TEXTURE_COMPARE_MODE, gl::COMPARE_REF_TO_TEXTURE);
    gl.tex_parameteri(gl::TEXTURE_COMPARE_FUNC, function as i32);
}

/// The casters' rows at a draw's first caster. The surface attributes
/// the casters' stage does not read point into the same buffer, so no
/// attribute the array enables reads past an end.
fn point_casters<G: GlFamily>(gl: &mut G, base: usize) {
    let stride = SHADOW_INSTANCE_STRIDE as i32;
    for row in 0..3 {
        gl.vertex_attrib_pointer(INSTANCE_ATTRIBUTE + row as u32, 4, gl::FLOAT, false, stride, base + row * 16);
    }
    for row in 3..6 {
        gl.vertex_attrib_pointer(INSTANCE_ATTRIBUTE + row, 4, gl::FLOAT, false, stride, base);
    }
    gl.vertex_attrib_i_pointer(INSTANCE_ATTRIBUTE + 6, 4, gl::UNSIGNED_BYTE, stride, base);
    // the part base, read as the first of the casters' ids
    gl.vertex_attrib_i_pointer(INSTANCE_ATTRIBUTE + 7, 3, gl::UNSIGNED_INT, stride, base + 48);
    gl.vertex_attrib_i_pointer(INSTANCE_ATTRIBUTE + 8, 4, gl::UNSIGNED_INT, stride, base);
}

/// The instance attributes, at the draw's first instance: GL has no base
/// instance on every tier, so the first instance rides the byte offset.
fn point_instances<G: GlFamily>(gl: &mut G, base: usize) {
    use crate::world::wire::instance_offset as at;
    let stride = at::SIZE as i32;
    for row in 0..6 {
        gl.vertex_attrib_pointer(INSTANCE_ATTRIBUTE + row as u32, 4, gl::FLOAT, false, stride, base + row * 16);
    }
    gl.vertex_attrib_i_pointer(INSTANCE_ATTRIBUTE + 6, 4, gl::UNSIGNED_BYTE, stride, base + at::TINT);
    gl.vertex_attrib_i_pointer(INSTANCE_ATTRIBUTE + 7, 3, gl::UNSIGNED_INT, stride, base + at::MATERIAL_BASE);
    gl.vertex_attrib_i_pointer(INSTANCE_ATTRIBUTE + 8, 4, gl::UNSIGNED_INT, stride, base + at::AMBIENT);
}

impl<G: GlFamily> WorldGround for GlWorld<G> {
    fn world_caps(&self) -> WorldCaps {
        let start = self.gl.renderer().map_or(crate::world::frame::Quality::High, |name| crate::world::governor::device_quality(&name));
        // a rect alone resolves by a blit of that rect, and a picture
        // slides by a blit from one target into another
        WorldCaps { samples: self.samples, start, redraw_samples: 4, slides: true, timers: self.gl.has_timers() }
    }

    fn make_target(&mut self, width: u32, height: u32, samples: u8) -> Option<u64> {
        let gl = &mut self.gl;
        let (w, h) = (width as i32, height as i32);
        let texture = gl.create_texture();
        gl.active_texture(gl::TEXTURE0);
        gl.bind_texture(texture);
        gl.tex_image_2d(0, gl::RGBA8, w, h, gl::RGBA, gl::UNSIGNED_BYTE, &[]);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::LINEAR);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::LINEAR);
        gl.tex_parameteri(gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
        gl.tex_parameteri(gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
        gl.bind_texture(0);
        let resolve = gl.create_framebuffer();
        gl.bind_framebuffer(gl::FRAMEBUFFER, resolve);
        gl.framebuffer_texture_2d(gl::COLOR_ATTACHMENT0, texture);
        let mut target = Target { texture, resolve, multisample: 0, color: 0, depth: 0, size: (width, height) };
        let depth = gl.create_renderbuffer();
        gl.renderbuffer_storage(depth, if samples > 1 { samples as i32 } else { 0 }, gl::DEPTH_COMPONENT32F, w, h);
        target.depth = depth;
        let complete = if samples > 1 {
            let complete = gl.check_framebuffer_status() == gl::FRAMEBUFFER_COMPLETE;
            let color = gl.create_renderbuffer();
            gl.renderbuffer_storage(color, samples as i32, gl::RGBA8, w, h);
            let multisample = gl.create_framebuffer();
            gl.bind_framebuffer(gl::FRAMEBUFFER, multisample);
            gl.framebuffer_renderbuffer(gl::COLOR_ATTACHMENT0, color);
            gl.framebuffer_renderbuffer(gl::DEPTH_ATTACHMENT, depth);
            target.color = color;
            target.multisample = multisample;
            complete && gl.check_framebuffer_status() == gl::FRAMEBUFFER_COMPLETE
        } else {
            gl.framebuffer_renderbuffer(gl::DEPTH_ATTACHMENT, depth);
            gl.check_framebuffer_status() == gl::FRAMEBUFFER_COMPLETE
        };
        gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        if !complete {
            release_target(gl, &target);
            return None;
        }
        let id = self.mint();
        self.targets.insert(id, target);
        Some(id)
    }

    fn drop_target(&mut self, id: u64) {
        if let Some(bloom) = self.blooms.remove(&id) {
            release_bloom(&mut self.gl, &bloom);
        }
        if let Some(target) = self.targets.remove(&id) {
            release_target(&mut self.gl, &target);
        }
    }

    fn make_mesh(&mut self, vertices: &[u8], indices: &[u8], deform: &[u8]) -> Option<u64> {
        use crate::world::mesh::{VERTEX_STRIDE, offset};
        if vertices.is_empty() || indices.is_empty() {
            return None;
        }
        let gl = &mut self.gl;
        let array = gl.create_vertex_array();
        gl.bind_vertex_array(array);
        let vertex_buffer = gl.create_buffer();
        gl.bind_buffer(gl::ARRAY_BUFFER, vertex_buffer);
        gl.buffer_data(gl::ARRAY_BUFFER, vertices, gl::STATIC_DRAW);
        let stride = VERTEX_STRIDE as i32;
        for index in 0..INSTANCE_ATTRIBUTE + INSTANCE_ATTRIBUTES {
            gl.enable_vertex_attrib_array(index);
        }
        gl.vertex_attrib_pointer(0, 3, gl::FLOAT, false, stride, offset::POSITION);
        gl.vertex_attrib_i_pointer(1, 2, gl::SHORT, stride, offset::NORMAL);
        gl.vertex_attrib_pointer(2, 2, gl::FLOAT, false, stride, offset::UV);
        gl.vertex_attrib_i_pointer(3, 4, gl::UNSIGNED_BYTE, stride, offset::COLOR);
        // material, occlusion, and the part's two bytes
        gl.vertex_attrib_i_pointer(4, 4, gl::UNSIGNED_BYTE, stride, offset::MATERIAL);
        for index in INSTANCE_ATTRIBUTE..INSTANCE_ATTRIBUTE + INSTANCE_ATTRIBUTES {
            gl.vertex_attrib_divisor(index, 1);
        }
        let index_buffer = gl.create_buffer();
        gl.bind_buffer(gl::ELEMENT_ARRAY_BUFFER, index_buffer);
        gl.buffer_data(gl::ELEMENT_ARRAY_BUFFER, indices, gl::STATIC_DRAW);
        gl.bind_vertex_array(0);
        // the deform stream: one word a texel, 1024 a row
        let mut deform_texture = 0;
        if !deform.is_empty() {
            let words = deform.len().div_ceil(4);
            let (columns, rows) = (words.min(1024), words.div_ceil(1024));
            let mut bytes = deform.to_vec();
            bytes.resize(columns * rows * 4, 0);
            deform_texture = gl.create_texture();
            gl.active_texture(gl::TEXTURE0);
            gl.bind_texture(deform_texture);
            gl.tex_image_2d(0, gl::R32UI, columns as i32, rows as i32, gl::RED_INTEGER, gl::UNSIGNED_INT, &bytes);
            gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
            gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::NEAREST);
            gl.bind_texture(0);
        }
        let id = self.mint();
        self.meshes.insert(id, MeshObjects { array, vertices: vertex_buffer, indices: index_buffer, deform: deform_texture });
        Some(id)
    }

    fn drop_mesh(&mut self, id: u64) {
        if let Some(mesh) = self.meshes.remove(&id) {
            self.gl.delete_vertex_array(mesh.array);
            self.gl.delete_buffer(mesh.vertices);
            self.gl.delete_buffer(mesh.indices);
            if mesh.deform != 0 {
                self.gl.delete_texture(mesh.deform);
            }
        }
    }

    fn make_texture(&mut self, width: u32, height: u32, levels: &[&[u8]]) -> Option<u64> {
        if levels.is_empty() {
            return None;
        }
        let gl = &mut self.gl;
        let texture = gl.create_texture();
        gl.active_texture(gl::TEXTURE0);
        gl.bind_texture(texture);
        let (mut w, mut h) = (width, height);
        for (level, bytes) in levels.iter().enumerate() {
            if bytes.len() < (w * h * 4) as usize {
                gl.bind_texture(0);
                gl.delete_texture(texture);
                return None;
            }
            gl.tex_image_2d(level as i32, gl::SRGB8_ALPHA8, w as i32, h as i32, gl::RGBA, gl::UNSIGNED_BYTE, &bytes[..(w * h * 4) as usize]);
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        gl.tex_parameteri(gl::TEXTURE_MAX_LEVEL, levels.len() as i32 - 1);
        gl.tex_parameteri(gl::TEXTURE_MIN_FILTER, gl::LINEAR_MIPMAP_LINEAR);
        gl.tex_parameteri(gl::TEXTURE_MAG_FILTER, gl::LINEAR);
        gl.tex_parameteri(gl::TEXTURE_WRAP_S, gl::REPEAT);
        gl.tex_parameteri(gl::TEXTURE_WRAP_T, gl::REPEAT);
        gl.bind_texture(0);
        let id = self.mint();
        self.textures.insert(id, texture);
        Some(id)
    }

    fn drop_texture(&mut self, id: u64) {
        if let Some(texture) = self.textures.remove(&id) {
            self.gl.delete_texture(texture);
        }
    }

    fn make_segments(&mut self, segments: &[u8]) -> Option<u64> {
        let count = (segments.len() / SEGMENT_STRIDE) as u32;
        let gl = &mut self.gl;
        let array = gl.create_vertex_array();
        let buffer = gl.create_buffer();
        gl.bind_vertex_array(array);
        gl.bind_buffer(gl::ARRAY_BUFFER, buffer);
        if count > 0 {
            gl.buffer_data(gl::ARRAY_BUFFER, segments, gl::STATIC_DRAW);
        }
        for (index, at) in [(0u32, 0usize), (1, 12)] {
            gl.enable_vertex_attrib_array(index);
            gl.vertex_attrib_pointer(index, 3, gl::FLOAT, false, SEGMENT_STRIDE as i32, at);
            gl.vertex_attrib_divisor(index, 1);
        }
        gl.bind_vertex_array(0);
        let id = self.mint();
        self.segments.insert(id, SegmentObjects { array, buffer, count });
        Some(id)
    }

    fn drop_segments(&mut self, id: u64) {
        if let Some(segments) = self.segments.remove(&id) {
            self.gl.delete_vertex_array(segments.array);
            self.gl.delete_buffer(segments.buffer);
        }
    }

    fn make_shadow_map(&mut self, size: u32) -> Option<u64> {
        let gl = &mut self.gl;
        let texture = gl.create_texture();
        gl.active_texture(gl::TEXTURE0);
        gl.bind_texture(texture);
        gl.tex_image_2d(0, gl::DEPTH_COMPONENT16, size as i32, size as i32, gl::DEPTH_COMPONENT, gl::UNSIGNED_SHORT, &[]);
        compare_with(gl, gl::LEQUAL, gl::LINEAR);
        gl.bind_texture(0);
        let framebuffer = gl.create_framebuffer();
        gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer);
        gl.framebuffer_texture_2d(gl::DEPTH_ATTACHMENT, texture);
        gl.draw_no_color();
        let complete = gl.check_framebuffer_status() == gl::FRAMEBUFFER_COMPLETE;
        gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        if !complete {
            gl.delete_framebuffer(framebuffer);
            gl.delete_texture(texture);
            return None;
        }
        let id = self.mint();
        self.shadow_maps.insert(id, ShadowObjects { texture, framebuffer, size });
        Some(id)
    }

    fn drop_shadow_map(&mut self, id: u64) {
        if let Some(map) = self.shadow_maps.remove(&id) {
            self.gl.delete_framebuffer(map.framebuffer);
            self.gl.delete_texture(map.texture);
        }
    }

    fn take_timings(&mut self) -> Vec<WorldTiming> {
        self.read_timers();
        std::mem::take(&mut self.timings)
    }
}

fn release_bloom<G: GlFamily>(gl: &mut G, bloom: &BloomObjects) {
    let steps = bloom.down.iter().chain(bloom.up.iter());
    for &(texture, framebuffer) in steps.chain([(bloom.glow, bloom.glow_framebuffer), (0, bloom.spill_framebuffer)].iter()) {
        if framebuffer != 0 {
            gl.delete_framebuffer(framebuffer);
        }
        if texture != 0 {
            gl.delete_texture(texture);
        }
    }
    if bloom.glow_samples != 0 {
        gl.delete_renderbuffer(bloom.glow_samples);
    }
}

fn release_target<G: GlFamily>(gl: &mut G, target: &Target) {
    for framebuffer in [target.resolve, target.multisample] {
        if framebuffer != 0 {
            gl.delete_framebuffer(framebuffer);
        }
    }
    for renderbuffer in [target.color, target.depth] {
        if renderbuffer != 0 {
            gl.delete_renderbuffer(renderbuffer);
        }
    }
    gl.delete_texture(target.texture);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Color;
    use crate::world::camera::Camera;
    use crate::world::frame::{Quality, WorldStyle};
    use crate::world::material::{LineStyle, Material};
    use crate::world::math::{Vec3, vec3};
    use crate::world::mesh::Mesh;
    use crate::world::store::World;

    /// A family that writes down every verb: the encoder's order is the
    /// test's subject, and no device is needed to read it.
    #[derive(Default)]
    struct Tape {
        calls: Vec<String>,
        next: u32,
        refuse: bool,
    }

    impl Tape {
        fn name(&mut self) -> u32 {
            self.next += 1;
            self.next
        }
        fn note(&mut self, call: String) {
            self.calls.push(call);
        }
        fn count(&self, prefix: &str) -> usize {
            self.calls.iter().filter(|call| call.starts_with(prefix)).count()
        }
    }

    impl GlFamily for Tape {
        fn prelude(&self) -> &'static str {
            crate::gpu::shaders::PRELUDE_330
        }
        fn program(&mut self, vertex: &str, fragment: &str, attributes: &[&str], blocks: &[(&str, u32)]) -> Result<u32, String> {
            if self.refuse {
                return Err("refused".into());
            }
            assert!(vertex.starts_with("#version") && fragment.starts_with("#version"));
            self.note(format!("program {} {}", attributes.len(), blocks.len()));
            Ok(self.name())
        }
        fn uniform_location(&mut self, _: u32, name: &str) -> u32 {
            name.len() as u32
        }
        fn use_program(&mut self, program: u32) {
            self.note(format!("use {program}"));
        }
        fn uniform1i(&mut self, _: u32, _: i32) {}
        fn uniform4fv(&mut self, location: u32, values: &[[f32; 4]]) {
            self.note(format!("uniform {location} {}", values.len()));
        }
        fn create_buffer(&mut self) -> u32 {
            self.name()
        }
        fn delete_buffer(&mut self, buffer: u32) {
            self.note(format!("delete buffer {buffer}"));
        }
        fn bind_buffer(&mut self, _: u32, _: u32) {}
        fn buffer_data(&mut self, target: u32, bytes: &[u8], _: u32) {
            self.note(format!("data {target:#x} {}", bytes.len()));
        }
        fn bind_buffer_base(&mut self, _: u32, _: u32, _: u32) {}
        fn create_vertex_array(&mut self) -> u32 {
            self.name()
        }
        fn delete_vertex_array(&mut self, _: u32) {}
        fn bind_vertex_array(&mut self, _: u32) {}
        fn enable_vertex_attrib_array(&mut self, _: u32) {}
        fn vertex_attrib_pointer(&mut self, index: u32, _: i32, _: u32, _: bool, _: i32, offset: usize) {
            self.note(format!("attrib {index} {offset}"));
        }
        fn vertex_attrib_i_pointer(&mut self, index: u32, _: i32, _: u32, _: i32, offset: usize) {
            self.note(format!("attrib {index} {offset}"));
        }
        fn vertex_attrib_divisor(&mut self, _: u32, _: u32) {}
        fn create_texture(&mut self) -> u32 {
            self.name()
        }
        fn delete_texture(&mut self, texture: u32) {
            self.note(format!("delete texture {texture}"));
        }
        fn active_texture(&mut self, _: u32) {}
        fn bind_texture(&mut self, _: u32) {}
        fn tex_parameteri(&mut self, _: u32, _: i32) {}
        fn tex_image_2d(&mut self, level: i32, internal: u32, width: i32, height: i32, _: u32, _: u32, bytes: &[u8]) {
            self.note(format!("image {level} {internal:#x} {width}x{height} {}", bytes.len()));
        }
        fn create_framebuffer(&mut self) -> u32 {
            self.name()
        }
        fn delete_framebuffer(&mut self, _: u32) {}
        fn bind_framebuffer(&mut self, target: u32, framebuffer: u32) {
            self.note(format!("framebuffer {target:#x} {framebuffer}"));
        }
        fn framebuffer_texture_2d(&mut self, _: u32, _: u32) {}
        fn framebuffer_renderbuffer(&mut self, _: u32, _: u32) {}
        fn check_framebuffer_status(&mut self) -> u32 {
            gl::FRAMEBUFFER_COMPLETE
        }
        fn create_renderbuffer(&mut self) -> u32 {
            self.name()
        }
        fn delete_renderbuffer(&mut self, _: u32) {}
        fn renderbuffer_storage(&mut self, _: u32, samples: i32, internal: u32, _: i32, _: i32) {
            self.note(format!("storage {samples} {internal:#x}"));
        }
        fn blit_color(&mut self, from: [i32; 4], _: [i32; 2]) {
            self.note(format!("blit {}x{}", from[2], from[3]));
        }
        fn scissor(&mut self, rect: [i32; 4]) {
            self.note(format!("scissor {rect:?}"));
        }
        fn invalidate_framebuffer(&mut self, _: u32, _: u32, _: u32) {
            self.note("invalidate".into());
        }
        fn draw_buffers(&mut self, count: u32) {
            self.note(format!("draw_buffers {count}"));
        }
        fn read_buffer(&mut self, attachment: u32) {
            self.note(format!("read_buffer {attachment:#x}"));
        }
        fn clear_color_buffer(&mut self, index: i32, _: [f32; 4]) {
            self.note(format!("clear_color_buffer {index}"));
        }
        fn viewport(&mut self, _: i32, _: i32) {}
        fn enable(&mut self, capability: u32) {
            self.note(format!("enable {capability:#x}"));
        }
        fn disable(&mut self, capability: u32) {
            self.note(format!("disable {capability:#x}"));
        }
        fn depth_func(&mut self, function: u32) {
            self.note(format!("depth {function:#x}"));
        }
        fn depth_mask(&mut self, write: bool) {
            self.note(format!("depth write {write}"));
        }
        fn blend_func_separate(&mut self, a: u32, b: u32, c: u32, d: u32) {
            self.note(format!("blend {a:#x} {b:#x} {c:#x} {d:#x}"));
        }
        fn cull_face(&mut self, _: u32) {}
        fn front_face(&mut self, winding: u32) {
            self.note(format!("front {winding:#x}"));
        }
        fn clear(&mut self, color: [f32; 4], _: f32) {
            self.note(format!("clear {:?}", color));
        }
        fn draw_arrays(&mut self, _: u32, _: i32, count: i32) {
            self.note(format!("draw {count}"));
        }
        fn draw_arrays_instanced(&mut self, _: u32, _: i32, count: i32, instances: i32) {
            self.note(format!("draw lines {count} x{instances}"));
        }
        fn draw_elements_instanced(&mut self, _: u32, count: i32, _: u32, offset: usize, instances: i32) {
            self.note(format!("draw mesh {count} at {offset} x{instances}"));
        }
        fn read_pixels(&mut self, _: i32, _: i32, out: &mut [u8]) {
            out.fill(7);
        }
    }

    fn frame(quality: Quality) -> Rc<WorldFrame> {
        let world = World::new();
        let cube = Mesh::cuboid(1.0, 1.0, 1.0);
        // one model placed twice: one instanced draw
        let model = crate::world::material::Model::single(cube.clone(), Material::color(Color::rgb(200, 90, 50)));
        world.add_model(model.clone());
        world.add_model(model).set_position(vec3(2.0, 0.0, 0.0));
        world.add_lines(cube.edges(30.0), LineStyle::solid(Color::rgb(0, 0, 0), 1.5));
        let style = WorldStyle { quality, ..WorldStyle::default() };
        Rc::new(world.frame(&Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0), &style, (64, 48), 1.0))
    }

    #[test]
    fn a_frame_draws_its_surfaces_instanced_then_its_edges_then_resolves() {
        let mut world = GlWorld::new(Tape::default(), [gl::ONE; 4], 4);
        let frame = frame(Quality::Medium);
        let out = world.draw_single(&frame, &crate::image_engine::RawImages::default()).expect("the pass stands");
        assert_eq!(out.len(), 64 * 48 * 4);
        let tape = &world.gl;
        assert_eq!(tape.count("program"), 9, "mesh, stepped, lines, backdrop, three bloom steps, shadow, composite");
        assert_eq!(tape.count("storage 4"), 2, "four samples of colour and of depth");
        assert_eq!(tape.count("draw mesh 36 at 0 x2"), 1, "two cubes in one draw");
        assert_eq!(tape.count("draw lines 6 x12"), 1, "twelve edges, one quad each");
        assert_eq!(tape.count("blit 64x48"), 1, "the samples resolve once");
        let mesh = tape.calls.iter().position(|call| call.starts_with("draw mesh")).unwrap();
        let lines = tape.calls.iter().position(|call| call.starts_with("draw lines")).unwrap();
        let blit = tape.calls.iter().position(|call| call.starts_with("blit")).unwrap();
        assert!(mesh < lines && lines < blit);
        // the surfaces step back under the edges: the stepped program draws them
        assert!(frame.face_bias > 0.0);
        // and everything made for the single frame went away
        assert!(world.targets.is_empty() && world.meshes.is_empty() && world.segments.is_empty());
    }

    #[test]
    fn a_frame_with_a_bloom_draws_its_glow_and_runs_the_chain() {
        let world_of = || {
            let world = World::new();
            world.add(Mesh::sphere(0.4), Material::unlit(Color::rgb(255, 200, 120)).emissive(Color::rgb(255, 200, 120), 3.0));
            world
        };
        let style = WorldStyle { quality: Quality::High, bloom: 0.6, ..WorldStyle::default() };
        let frame = Rc::new(world_of().frame(&Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0), &style, (64, 48), 1.0));
        let mut world = GlWorld::new(Tape::default(), [gl::ONE; 4], 4);
        world.draw_single(&frame, &crate::image_engine::RawImages::default()).expect("the pass stands");
        let tape = &world.gl;
        assert_eq!(tape.count("storage 4"), 3, "four samples of colour, depth and glow");
        assert_eq!(tape.count("clear_color_buffer 1"), 1, "the glow starts at none");
        assert_eq!(tape.count("blit 64x48"), 2, "the colour and the glow resolve");
        assert_eq!(tape.count("draw 3"), 8, "four steps down, three up, one spill");
        assert_eq!(tape.count("blend 0x307 0x1 0x305 0x1"), 1, "the spill is a screen");
        let spill = tape.calls.iter().position(|call| call == "blend 0x307 0x1 0x305 0x1").unwrap();
        let resolve = tape.calls.iter().rposition(|call| call.starts_with("blit")).unwrap();
        assert!(resolve < spill, "the chain reads the resolved target");
        assert!(world.blooms.is_empty(), "the target's bloom went with it");
        // without a bloom, one attachment and no chain
        let plain = Rc::new(world_of().frame(&Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0), &WorldStyle { bloom: 0.0, ..style }, (64, 48), 1.0));
        let mut world = GlWorld::new(Tape::default(), [gl::ONE; 4], 4);
        world.draw_single(&plain, &crate::image_engine::RawImages::default()).expect("the pass stands");
        assert_eq!(world.gl.count("draw 3"), 0);
        assert_eq!(world.gl.count("draw_buffers 2"), 0);
    }

    #[test]
    fn one_sample_draws_straight_into_the_texture() {
        let mut world = GlWorld::new(Tape::default(), [gl::ONE; 4], 4);
        world.draw_single(&frame(Quality::Low), &crate::image_engine::RawImages::default()).expect("the pass stands");
        assert_eq!(world.gl.count("blit"), 0);
        assert_eq!(world.gl.count("storage 0"), 1, "a depth of one sample");
    }

    #[test]
    fn the_2d_state_comes_back_after_the_pass() {
        let mut world = GlWorld::new(Tape::default(), [gl::ONE, gl::ONE_MINUS_SRC_ALPHA, gl::ONE, gl::ONE_MINUS_SRC_ALPHA], 4);
        world.draw_single(&frame(Quality::Low), &crate::image_engine::RawImages::default()).expect("the pass stands");
        let tail: Vec<&String> = world.gl.calls.iter().rev().take(20).collect();
        assert!(tail.iter().any(|call| *call == "disable 0xb71"), "depth off");
        assert!(tail.iter().any(|call| *call == "enable 0xbe2"), "blending on");
        assert!(tail.iter().any(|call| *call == "blend 0x1 0x303 0x1 0x303"), "the 2D factors");
    }

    #[test]
    fn the_casters_go_into_the_map_before_the_frame_and_only_when_they_moved() {
        use crate::world::material::Light;
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(200, 90, 50)));
        world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
        world.add_light(Light::sun().toward(vec3(0.5, -1.0, 0.7)).with_shadows());
        let style = WorldStyle { quality: Quality::Low, ..WorldStyle::default() };
        let frame = Rc::new(world.frame(&Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0), &style, (64, 48), 1.0));
        assert!(frame.shadow.is_some());
        let mut gl_world = GlWorld::new(Tape::default(), [gl::ONE; 4], 4);
        gl_world.draw_single(&frame, &crate::image_engine::RawImages::default()).expect("the pass stands");
        let tape = &gl_world.gl;
        assert_eq!(tape.count("image 0 0x81a5 1024x1024"), 1, "a map of the quality's side");
        let draws: Vec<&String> = tape.calls.iter().filter(|call| call.starts_with("draw mesh")).collect();
        // the cube and the floor into the map, then the same two into the target
        assert_eq!(draws.len(), 4, "{draws:?}");
        let data = tape.calls.iter().position(|call| *call == format!("data {:#x} 128", gl::ARRAY_BUFFER)).expect("two casters' rows");
        let first_draw = tape.calls.iter().position(|call| call.starts_with("draw mesh")).unwrap();
        assert!(data < first_draw, "the rows go up before the casters draw");
        assert!(gl_world.shadow_maps.is_empty(), "the single frame gave its map back");

        // a job whose map holds the casters draws only the frame
        let mut gl_world = GlWorld::new(Tape::default(), [gl::ONE; 4], 4);
        let door = gl_world.door().unwrap();
        let mut job = WorldJob::single(door, &frame, &crate::image_engine::RawImages::default()).unwrap();
        job.shadow.as_mut().unwrap().draw = false;
        gl_world.gl.calls.clear();
        gl_world.encode(std::slice::from_ref(&job));
        assert_eq!(gl_world.gl.count("draw mesh"), 2, "the map is read, not drawn");
    }

    #[test]
    fn a_device_that_refuses_a_program_leaves_worlds_to_the_cpu() {
        let mut world = GlWorld::new(Tape { refuse: true, ..Tape::default() }, [gl::ONE; 4], 4);
        assert!(world.door().is_none());
        assert!(world.failed());
        assert!(world.door().is_none(), "never asked again");
    }
}
