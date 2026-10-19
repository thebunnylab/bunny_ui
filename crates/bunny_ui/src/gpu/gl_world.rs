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
//! The 2D pass's state is put back when the pass ends: depth off, culling
//! off, blending on with the 2D pass's own factors, texture unit 0.

use std::collections::HashMap;
use std::rc::Rc;

use super::world::{WorldCaps, WorldGround, WorldJob};
use super::world_shaders as source;
use crate::image_engine::ImageEngine;
use crate::world::frame::{Background, Blend, DrawKind, ShadowFrame, WorldFrame};
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
    pub const DEPTH_COMPONENT32F: u32 = 0x8CAC;
    pub const DEPTH_COMPONENT16: u32 = 0x81A5;
    pub const DEPTH_COMPONENT: u32 = 0x1902;
    pub const FRAMEBUFFER: u32 = 0x8D40;
    pub const READ_FRAMEBUFFER: u32 = 0x8CA8;
    pub const DRAW_FRAMEBUFFER: u32 = 0x8CA9;
    pub const COLOR_ATTACHMENT0: u32 = 0x8CE0;
    pub const DEPTH_ATTACHMENT: u32 = 0x8D00;
    pub const FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
    pub const DEPTH_TEST: u32 = 0x0B71;
    pub const BLEND: u32 = 0x0BE2;
    pub const CULL_FACE: u32 = 0x0B44;
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
    fn blit_color(&mut self, width: i32, height: i32);
    /// The attachments' contents are no longer needed: a tiled GPU need
    /// not write them back. Up to two; 0 is none.
    fn invalidate_framebuffer(&mut self, target: u32, first: u32, second: u32);
    /// The bound framebuffer draws and reads no colour. Desktop GL 3.3
    /// counts a framebuffer of depth alone complete only once it is said;
    /// ES and WebGL2 need nothing.
    fn draw_no_color(&mut self) {}

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
const MESH_ATTRIBUTES: [&str; 13] = [
    "a_position", "a_normal", "a_uv", "a_color", "a_ids", "i_row0", "i_row1", "i_row2", "i_normal0", "i_normal1", "i_normal2", "i_tint",
    "i_ids",
];

/// The first index of the instance attributes.
const INSTANCE_ATTRIBUTE: u32 = 5;

#[derive(Clone, Copy)]
struct Program {
    handle: u32,
    draw: u32,
    rows: u32,
    style: u32,
    ramp: u32,
}

#[derive(Clone, Copy)]
struct Programs {
    mesh: Program,
    stepped: Program,
    lines: Program,
    backdrop: Program,
    /// The 2D pass's sprite for a world's target.
    composite: u32,
    /// The casters into a shadow map.
    shadow: u32,
    frame: u32,
    instances: u32,
    /// The casters' rows.
    casters: u32,
    materials: u32,
    blank: u32,
    /// A map of one texel whose comparison always passes: a frame without
    /// a map reads full sun.
    lit: u32,
    empty: u32,
}

enum Built {
    Not,
    Ready(Programs),
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
}

struct SegmentObjects {
    array: u32,
    buffer: u32,
    count: u32,
}

#[derive(Clone, Copy)]
struct ShadowObjects {
    texture: u32,
    framebuffer: u32,
    size: u32,
}

/// The world's GL objects, and the door the walk opens.
pub struct GlWorld<G: GlFamily> {
    pub gl: G,
    built: Built,
    targets: HashMap<u64, Target>,
    meshes: HashMap<u64, MeshObjects>,
    textures: HashMap<u64, u32>,
    segments: HashMap<u64, SegmentObjects>,
    shadow_maps: HashMap<u64, ShadowObjects>,
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
            targets: HashMap::new(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            segments: HashMap::new(),
            shadow_maps: HashMap::new(),
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
                Ok(programs) => Built::Ready(programs),
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
        let mesh_vertex = format!("{prelude}{}{table}{}", source::WORLD_COMMON, source::MESH_VERT);
        let mesh_fragment = |body: &str| format!("{prelude}{}{}{body}", source::WORLD_COMMON, source::MESH_FRAG_COMMON);
        let mesh = gl.program(&mesh_vertex, &mesh_fragment(source::MESH_FRAG), &MESH_ATTRIBUTES, &world)?;
        let stepped = gl.program(&mesh_vertex, &mesh_fragment(source::MESH_FRAG_STEPPED), &MESH_ATTRIBUTES, &world)?;
        let lines = gl.program(
            &format!("{prelude}{}{}", source::WORLD_COMMON, source::LINE_VERT),
            &format!("{prelude}{}{}", source::WORLD_COMMON, source::LINE_FRAG),
            &["a_from", "a_to"],
            &world,
        )?;
        let backdrop = gl.program(&format!("{prelude}{}", source::BACKDROP_VERT), &format!("{prelude}{}", source::BACKDROP_FRAG), &[], &[])?;
        let shadow = gl.program(
            &format!("{prelude}{}{}", source::WORLD_COMMON, source::SHADOW_VERT),
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
            let mut located = Program { handle, draw: 0, rows: 0, style: 0, ramp: 0 };
            located.draw = gl.uniform_location(handle, "u_draw");
            located.rows = gl.uniform_location(handle, "u_rows");
            located.style = gl.uniform_location(handle, "u_style");
            located.ramp = gl.uniform_location(handle, "u_ramp");
            for (name, unit) in [("u_materials", 0), ("u_surface", 1), ("u_shadow", 2), ("atlas", 0)] {
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
        let composite = program(gl, composite).handle;
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
            composite,
            shadow,
            frame: gl.create_buffer(),
            instances: gl.create_buffer(),
            casters: gl.create_buffer(),
            materials,
            blank,
            lit,
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
        let Built::Ready(programs) = self.built else { return };
        if jobs.is_empty() {
            return;
        }
        for job in jobs {
            self.encode_job(&programs, job);
        }
        let gl = &mut self.gl;
        gl.bind_vertex_array(0);
        gl.use_program(0);
        gl.disable(gl::DEPTH_TEST);
        gl.depth_mask(true);
        gl.disable(gl::CULL_FACE);
        gl.front_face(gl::CCW);
        gl.enable(gl::BLEND);
        let [a, b, c, d] = self.blend_2d;
        gl.blend_func_separate(a, b, c, d);
        gl.active_texture(gl::TEXTURE2);
        gl.bind_texture(0);
        gl.active_texture(gl::TEXTURE1);
        gl.bind_texture(0);
        gl.active_texture(gl::TEXTURE0);
        gl.bind_texture(0);
        gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
    }

    /// The casters of the frame's shadow into the box's map, from the sun:
    /// both faces, the nearest depth kept.
    fn draw_shadow(&mut self, programs: &Programs, job: &WorldJob, map: ShadowObjects, shadow: &ShadowFrame) {
        let gl = &mut self.gl;
        // nothing samples the map while it is drawn
        gl.active_texture(gl::TEXTURE2);
        gl.bind_texture(0);
        gl.bind_framebuffer(gl::FRAMEBUFFER, map.framebuffer);
        gl.viewport(map.size as i32, map.size as i32);
        gl.enable(gl::DEPTH_TEST);
        gl.depth_mask(true);
        gl.depth_func(gl::LESS);
        gl.disable(gl::BLEND);
        gl.disable(gl::CULL_FACE);
        gl.clear([0.0; 4], 1.0);
        gl.use_program(programs.shadow);
        gl.bind_buffer(gl::ARRAY_BUFFER, programs.casters);
        gl.buffer_data(gl::ARRAY_BUFFER, &shadow_instance_bytes(&shadow.instances), gl::STREAM_DRAW);
        for draw in &shadow.draws {
            let Some(objects) = job.meshes.get(draw.mesh as usize).and_then(|id| self.meshes.get(id)) else { continue };
            let gl = &mut self.gl;
            gl.bind_vertex_array(objects.array);
            gl.bind_buffer(gl::ARRAY_BUFFER, programs.casters);
            point_casters(gl, draw.first_instance as usize * SHADOW_INSTANCE_STRIDE);
            gl.draw_elements_instanced(gl::TRIANGLES, draw.index_count as i32, gl::UNSIGNED_INT, draw.first_index as usize * 4, draw.instance_count as i32);
        }
    }

    fn encode_job(&mut self, programs: &Programs, job: &WorldJob) {
        let Some(target) = self.targets.get(&job.target).copied() else { return };
        let frame = &job.frame;
        // the frame's block, which every program of the pass declares
        let gl = &mut self.gl;
        gl.bind_buffer(gl::UNIFORM_BUFFER, programs.frame);
        gl.buffer_data(gl::UNIFORM_BUFFER, &frame.uniforms.to_bytes(), gl::STREAM_DRAW);
        gl.bind_buffer_base(gl::UNIFORM_BUFFER, WORLD_BINDING, programs.frame);
        // the sun's map first, when it does not hold this frame's casters
        let map = job.shadow.and_then(|shadow| self.shadow_maps.get(&shadow.map).copied().map(|map| (shadow, map)));
        if let (Some((shadow, map)), Some(casters)) = (map, frame.shadow.as_ref())
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
        gl.active_texture(gl::TEXTURE1);
        gl.bind_texture(programs.blank);

        // the target, cleared to the backdrop
        gl.bind_framebuffer(gl::FRAMEBUFFER, if target.multisample != 0 { target.multisample } else { target.resolve });
        gl.viewport(width, height);
        gl.enable(gl::DEPTH_TEST);
        gl.depth_mask(true);
        gl.disable(gl::BLEND);
        gl.disable(gl::CULL_FACE);
        let clear = match frame.background {
            Background::Color(c) => {
                let premultiply = |channel: u8| ((channel as u32 * c.a as u32 + 127) / 255) as f32 / 255.0;
                [premultiply(c.r), premultiply(c.g), premultiply(c.b), c.a as f32 / 255.0]
            }
            Background::Transparent | Background::Gradient { .. } => [0.0; 4],
        };
        gl.clear(clear, 1.0);
        if let Background::Gradient { top, bottom } = frame.background {
            let codes = |c: crate::layout::Color| [c.r as f32, c.g as f32, c.b as f32, c.a as f32];
            gl.use_program(programs.backdrop.handle);
            gl.uniform4fv(programs.backdrop.ramp, &[codes(top), codes(bottom), [width as f32, height as f32, 0.0, 0.0]]);
            gl.depth_func(gl::ALWAYS);
            gl.depth_mask(false);
            gl.bind_vertex_array(programs.empty);
            gl.draw_arrays(gl::TRIANGLES, 0, 3);
        }

        let (near, far) = (frame.near as f32, frame.far as f32);
        for draw in &frame.draws {
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
                    if surface.is_some() {
                        flags |= 2;
                    }
                    if replace {
                        flags |= 4;
                    }
                    let gl = &mut self.gl;
                    gl.use_program(program.handle);
                    gl.uniform4fv(program.draw, &[[flags as f32, near, far, bias]]);
                    set_blend(gl, blend);
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

        // the samples into the texture, and nothing written back that
        // nobody reads
        let gl = &mut self.gl;
        if target.multisample != 0 {
            gl.bind_framebuffer(gl::READ_FRAMEBUFFER, target.multisample);
            gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, target.resolve);
            gl.blit_color(width, height);
            gl.invalidate_framebuffer(gl::READ_FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::DEPTH_ATTACHMENT);
        } else {
            gl.invalidate_framebuffer(gl::FRAMEBUFFER, gl::DEPTH_ATTACHMENT, 0);
        }
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
    gl.vertex_attrib_i_pointer(INSTANCE_ATTRIBUTE + 7, 3, gl::UNSIGNED_INT, stride, base);
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
}

impl<G: GlFamily> WorldGround for GlWorld<G> {
    fn world_caps(&self) -> WorldCaps {
        WorldCaps { samples: self.samples }
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
        if let Some(target) = self.targets.remove(&id) {
            release_target(&mut self.gl, &target);
        }
    }

    fn make_mesh(&mut self, vertices: &[u8], indices: &[u8]) -> Option<u64> {
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
        for index in 0..INSTANCE_ATTRIBUTE + 8 {
            gl.enable_vertex_attrib_array(index);
        }
        gl.vertex_attrib_pointer(0, 3, gl::FLOAT, false, stride, offset::POSITION);
        gl.vertex_attrib_i_pointer(1, 2, gl::SHORT, stride, offset::NORMAL);
        gl.vertex_attrib_pointer(2, 2, gl::FLOAT, false, stride, offset::UV);
        gl.vertex_attrib_i_pointer(3, 4, gl::UNSIGNED_BYTE, stride, offset::COLOR);
        gl.vertex_attrib_i_pointer(4, 2, gl::UNSIGNED_BYTE, stride, offset::MATERIAL);
        for index in INSTANCE_ATTRIBUTE..INSTANCE_ATTRIBUTE + 8 {
            gl.vertex_attrib_divisor(index, 1);
        }
        let index_buffer = gl.create_buffer();
        gl.bind_buffer(gl::ELEMENT_ARRAY_BUFFER, index_buffer);
        gl.buffer_data(gl::ELEMENT_ARRAY_BUFFER, indices, gl::STATIC_DRAW);
        gl.bind_vertex_array(0);
        let id = self.mint();
        self.meshes.insert(id, MeshObjects { array, vertices: vertex_buffer, indices: index_buffer });
        Some(id)
    }

    fn drop_mesh(&mut self, id: u64) {
        if let Some(mesh) = self.meshes.remove(&id) {
            self.gl.delete_vertex_array(mesh.array);
            self.gl.delete_buffer(mesh.vertices);
            self.gl.delete_buffer(mesh.indices);
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
        fn blit_color(&mut self, width: i32, height: i32) {
            self.note(format!("blit {width}x{height}"));
        }
        fn invalidate_framebuffer(&mut self, _: u32, _: u32, _: u32) {
            self.note("invalidate".into());
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
        assert_eq!(tape.count("program"), 6, "mesh, stepped, lines, backdrop, shadow, composite");
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
        let data = tape.calls.iter().position(|call| *call == format!("data {:#x} 96", gl::ARRAY_BUFFER)).expect("two casters' rows");
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
