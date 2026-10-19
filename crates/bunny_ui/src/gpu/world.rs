//! The seam a GPU tier fills to draw worlds itself.
//!
//! The walk meets a world picture ([`ImageSource::World`]) and asks the
//! ground for its world door ([`super::walk::AtlasGround::world`]). A
//! tier that has one gets a [`WorldJob`] when the picture moved: a target
//! it keeps for the box, the frame the core decided, and the tier's own
//! handles for every mesh, texture and set of segments the frame names —
//! uploaded once per identity and generation by the [`WorldResidency`]
//! here — and the box's shadow map, with whether the casters must be
//! drawn into it again. The tier draws the jobs into their targets before
//! its 2D pass, and the 2D pass composites each target like a feed, with
//! the box's veil ([`super::walk::RunKind::World`]).
//!
//! The LAW is the walk's: every decision about what to keep and when to
//! let it go is made here, on the CPU. A tier only moves bytes and mints
//! handles. A tier with no world door — or one that could not make what
//! a picture needs — shows the oracle's picture through the live road,
//! so every tier shows every world.
//!
//! A target holds premultiplied, sRGB-encoded RGBA8, its first row at
//! the top: what the CPU rasterizer writes.
//!
//! A target remembers the frame it holds. When the next frame of its box
//! differs only in what some entities show — a figure that walks, a part
//! that turns red — the job asks for those rects alone
//! ([`Redraw::Rects`], from [`crate::world::damage`]), and the tier
//! clears and draws them over the picture the target keeps. When a
//! parallel eye at Low pans, the box keeps a second target: the tier
//! slides the picture from one into the other and draws the strips the
//! slide uncovers ([`Redraw::Slide`]).

use std::collections::HashMap;
use std::rc::Rc;

use crate::image_engine::{ImageEngine, ImageSource};
use crate::world::frame::{Quality, ShadowFrame, WorldFrame};
use crate::world::governor::Governor;
use crate::world::mesh::{Lines, Mesh};
use crate::world::view::Shot;
use crate::world::wire::segment_bytes;

/// What a tier can do for worlds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldCaps {
    /// The most samples a pixel a target may take: 1 or 4.
    pub samples: u8,
    /// The quality the device starts its worlds at
    /// ([`crate::world::governor::device_quality`] of its name); `High`
    /// when the tier cannot name it, which leaves the ask alone.
    pub start: Quality,
    /// The most samples a pixel at which the tier draws only some rects
    /// of a frame over what its target holds ([`Redraw::Rects`]): 0 when
    /// it draws every frame whole.
    pub redraw_samples: u8,
    /// Whether the tier slides a picture from one of its targets into
    /// another of the same size ([`Redraw::Slide`]).
    pub slides: bool,
    /// Whether the tier times its GPU over each job
    /// ([`WorldGround::take_timings`]).
    pub timers: bool,
}

/// How long the GPU took over one job, measured by the tier and read some
/// frames after the job ran.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTiming {
    /// The job's target.
    pub target: u64,
    /// From the job's first command to its last, on the GPU.
    pub seconds: f64,
}

/// The longest a job may take on the GPU, in seconds. A clock that reads
/// more — or nothing — is wrong, and its time is dropped.
pub const TIMING_MOST: f64 = 1.0;

impl WorldTiming {
    /// Whether a clock could have read `seconds` over one job.
    pub fn plausible(seconds: f64) -> bool {
        seconds > 0.0 && seconds < TIMING_MOST
    }
}

/// What of its target a job draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Redraw {
    /// All of it, from a cleared target.
    #[default]
    Whole,
    /// Only these rects, in pixels `[x, y, width, height]` from the top
    /// left, each cleared to the backdrop and drawn again with every draw
    /// that reaches it ([`crate::world::frame::WorldFrame::draw_reaches`]);
    /// the rest of the target keeps the picture it holds. Never empty.
    Rects(Vec<[u32; 4]>),
    /// The picture the target `from` holds, slid by `shift` pixels (x
    /// right, y down) into this job's target ([`slide_copy`]), then
    /// `rects` drawn over it as [`Redraw::Rects`] draws them: what the
    /// slide uncovers and what changed. At one sample a pixel.
    Slide { from: u64, shift: [i32; 2], rects: Vec<[u32; 4]> },
}

impl Redraw {
    /// The rects to draw over what the target holds; `None` for a frame
    /// drawn whole.
    pub fn rects(&self) -> Option<&[[u32; 4]]> {
        match self {
            Redraw::Whole => None,
            Redraw::Rects(rects) | Redraw::Slide { rects, .. } => Some(rects),
        }
    }
}

/// What a slide by `shift` copies on a target of `size`: the rect
/// `[x, y, width, height]` of the picture that stays in view, and the
/// pixel it lands on. `None` when nothing stays.
pub fn slide_copy(shift: [i32; 2], size: (u32, u32)) -> Option<([u32; 4], [u32; 2])> {
    let (dx, dy) = (shift[0] as i64, shift[1] as i64);
    let (width, height) = (size.0 as i64 - dx.abs(), size.1 as i64 - dy.abs());
    if width <= 0 || height <= 0 {
        return None;
    }
    let from = [(-dx).max(0) as u32, (-dy).max(0) as u32, width as u32, height as u32];
    Some((from, [dx.max(0) as u32, dy.max(0) as u32]))
}

/// The door a tier opens for worlds. Each call moves bytes or mints a
/// handle; `None` from a `make` means the tier could not, and the walk
/// shows the oracle's picture for that box this frame.
pub trait WorldGround {
    fn world_caps(&self) -> WorldCaps;
    /// A target of `width × height`: the resolved picture the 2D pass
    /// samples, with the depth and multisample storage the tier keeps
    /// beside it.
    fn make_target(&mut self, width: u32, height: u32, samples: u8) -> Option<u64>;
    fn drop_target(&mut self, id: u64);
    /// A mesh: vertices in the wire layout, `u32` indices, little-endian.
    /// `deform` is the mesh's second stream (`Mesh::deform_bytes`), read
    /// by the vertex's number: empty for a mesh without a skin.
    fn make_mesh(&mut self, vertices: &[u8], indices: &[u8], deform: &[u8]) -> Option<u64>;
    fn drop_mesh(&mut self, id: u64);
    /// A texture and its whole chain: straight sRGB RGBA8, one slice per
    /// level, the base first, each level half the one above (at least 1).
    fn make_texture(&mut self, width: u32, height: u32, levels: &[&[u8]]) -> Option<u64>;
    fn drop_texture(&mut self, id: u64);
    /// A set of segments, [`crate::world::wire::SEGMENT_STRIDE`] bytes each.
    fn make_segments(&mut self, segments: &[u8]) -> Option<u64>;
    fn drop_segments(&mut self, id: u64);
    /// A shadow map of `size × size` depths: the shadow pass draws the
    /// casters into it, and the surfaces read it through a comparing
    /// sampler. `None` when the tier has none: its worlds draw in full sun.
    fn make_shadow_map(&mut self, size: u32) -> Option<u64> {
        let _ = size;
        None
    }
    fn drop_shadow_map(&mut self, id: u64) {
        let _ = id;
    }
    /// The GPU times of the jobs that finished since the last ask, for a
    /// tier that keeps timers ([`WorldCaps::timers`]). Never waits on the
    /// GPU: a job still running is read on a later ask.
    fn take_timings(&mut self) -> Vec<WorldTiming> {
        Vec::new()
    }
}

/// The sun's shadow map of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowJob {
    /// The map's handle.
    pub map: u64,
    /// The map's side in texels: the frame's.
    pub size: u32,
    /// The map does not hold the frame's shadow yet: the casters are
    /// drawn into it before the frame. False when only the camera moved.
    pub draw: bool,
}

/// One world picture a tier draws this frame, into one target.
#[derive(Clone)]
pub struct WorldJob {
    /// The target's handle.
    pub target: u64,
    /// The target's size in pixels — the frame's size.
    pub size: (u32, u32),
    /// The samples a pixel to draw with.
    pub samples: u8,
    pub frame: Rc<WorldFrame>,
    /// The tier's handle for each of `frame.meshes`.
    pub meshes: Vec<u64>,
    /// The tier's handle for each of `frame.textures`; `None` while the
    /// platform has not decoded it — the surface shows its colour, as the
    /// oracle's does.
    pub textures: Vec<Option<u64>>,
    /// The tier's handle for each of `frame.lines`.
    pub segments: Vec<u64>,
    /// The sun's shadow map, when the frame has one and the tier made it;
    /// without it the surfaces draw in full sun.
    pub shadow: Option<ShadowJob>,
    /// The second map, of what moves, the same way; drawn every frame it
    /// holds something new.
    pub moving_shadow: Option<ShadowJob>,
    /// What of the target to draw.
    pub redraw: Redraw,
}

impl WorldJob {
    /// One frame by itself, and everything it names made on `ground`: a
    /// target of its own, each mesh, texture and set of segments, and the
    /// shadow map to draw. For a parity harness, which reads the target
    /// back and then gives it all back ([`WorldJob::give_back`]).
    pub fn single(ground: &mut dyn WorldGround, frame: &Rc<WorldFrame>, images: &dyn ImageEngine) -> Option<WorldJob> {
        let samples = frame.samples.clamp(1, ground.world_caps().samples.max(1));
        let target = ground.make_target(frame.size.0, frame.size.1, samples)?;
        let job = WorldJob::into_target(ground, target, samples, frame, images);
        if job.is_none() {
            ground.drop_target(target);
        }
        job
    }

    /// `frame` over the picture `under` drew, as a box draws its next
    /// frame: into `under`'s target in `rects` alone ([`Redraw::Rects`]),
    /// or, slid by `shift`, into a target of its own ([`Redraw::Slide`]) —
    /// with everything else the frame names made on `ground`. For a parity
    /// harness, which reads this job's target back and gives it back with
    /// [`WorldJob::give_back_over`] before `under`.
    pub fn over(
        ground: &mut dyn WorldGround,
        under: &WorldJob,
        frame: &Rc<WorldFrame>,
        images: &dyn ImageEngine,
        shift: [i32; 2],
        rects: Vec<[u32; 4]>,
    ) -> Option<WorldJob> {
        if shift == [0, 0] {
            let mut job = WorldJob::into_target(ground, under.target, under.samples, frame, images)?;
            job.redraw = Redraw::Rects(rects);
            return Some(job);
        }
        let target = ground.make_target(frame.size.0, frame.size.1, under.samples)?;
        let Some(mut job) = WorldJob::into_target(ground, target, under.samples, frame, images) else {
            ground.drop_target(target);
            return None;
        };
        job.redraw = Redraw::Slide { from: under.target, shift, rects };
        Some(job)
    }

    /// Gives back what [`WorldJob::over`] made: all of it, but `under`'s
    /// target.
    pub fn give_back_over(self, ground: &mut dyn WorldGround) {
        let own = matches!(self.redraw, Redraw::Slide { .. });
        self.give_back_assets(ground, own);
    }

    /// Everything a frame names, made on `ground`, drawn whole into
    /// `target`.
    fn into_target(ground: &mut dyn WorldGround, target: u64, samples: u8, frame: &Rc<WorldFrame>, images: &dyn ImageEngine) -> Option<WorldJob> {
        let mut meshes = Vec::with_capacity(frame.meshes.len());
        for mesh in &frame.meshes {
            meshes.push(ground.make_mesh(&mesh.vertex_bytes(), &mesh.index_bytes(), &mesh.deform_bytes())?);
        }
        let mut segments = Vec::with_capacity(frame.lines.len());
        for lines in &frame.lines {
            segments.push(ground.make_segments(&segment_bytes(lines.segments()))?);
        }
        let textures = frame
            .textures
            .iter()
            .map(|source| {
                let texture = crate::world::texture::texture_of(images, source)?;
                let levels: Vec<&[u8]> = texture.levels.iter().map(|level| level.rgba.as_slice()).collect();
                ground.make_texture(texture.levels[0].width, texture.levels[0].height, &levels)
            })
            .collect();
        let shadow = frame.shadow.as_ref().and_then(|shadow| {
            Some(ShadowJob { map: ground.make_shadow_map(shadow.size)?, size: shadow.size, draw: true })
        });
        let moving_shadow = frame.moving_shadow.as_ref().and_then(|shadow| {
            Some(ShadowJob { map: ground.make_shadow_map(shadow.size)?, size: shadow.size, draw: true })
        });
        Some(WorldJob { target, size: frame.size, samples, frame: Rc::clone(frame), meshes, textures, segments, shadow, moving_shadow, redraw: Redraw::Whole })
    }

    /// Gives back everything [`WorldJob::single`] made.
    pub fn give_back(self, ground: &mut dyn WorldGround) {
        self.give_back_assets(ground, true);
    }

    fn give_back_assets(self, ground: &mut dyn WorldGround, target: bool) {
        if target {
            ground.drop_target(self.target);
        }
        for id in self.meshes {
            ground.drop_mesh(id);
        }
        for id in self.segments {
            ground.drop_segments(id);
        }
        for id in self.textures.into_iter().flatten() {
            ground.drop_texture(id);
        }
        for shadow in [self.shadow, self.moving_shadow].into_iter().flatten() {
            ground.drop_shadow_map(shadow.map);
        }
    }
}

/// The probe's world: a floor, a block of 36 balls and a sun that casts,
/// at Medium — the work of a lit room with its shadows.
fn probe_frame() -> WorldFrame {
    use crate::layout::Color;
    use crate::world::camera::Camera;
    use crate::world::frame::{Background, WorldStyle};
    use crate::world::material::{Light, Material};
    use crate::world::math::{Vec3, vec3};
    use crate::world::store::World;
    let world = World::new();
    world.add(Mesh::plane(12.0, 12.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
    let ball = Mesh::sphere(0.35);
    for row in 0..6 {
        for column in 0..6 {
            let colour = Color::rgb(60 + 30 * row as u8, 110, 220 - 25 * column as u8);
            world.add(ball.clone(), Material::color(colour).roughness(0.4)).set_position(vec3(column as f64 - 2.5, -0.15, row as f64 - 2.5));
        }
    }
    world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)).with_shadows());
    let style = WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality: Quality::Medium, ..WorldStyle::default() };
    world.frame(&Camera::orbiting(Vec3::ZERO, 30.0, 30.0, 10.0), &style, PROBE_SIZE, 1.0)
}

/// A world picture on a tier that draws worlds: the target the 2D pass
/// composites, and the job that draws into it when the picture moved.
pub(crate) struct ResolvedWorld {
    pub target: u64,
    pub size: (u32, u32),
    pub job: Option<WorldJob>,
}

/// `size` with each side at `share` of itself, at least a pixel.
fn scaled(size: (u32, u32), share: f64) -> (u32, u32) {
    if share >= 1.0 {
        return size;
    }
    (((size.0 as f64 * share).floor() as u32).max(1), ((size.1 as f64 * share).floor() as u32).max(1))
}

/// Walks a target may sit unread before it is given back.
pub const TARGET_IDLE_WALKS: u64 = 120;
/// The probe's picture, and how many times it is drawn.
const PROBE_SIZE: (u32, u32) = (640, 360);
const PROBE_FRAMES: usize = 3;
/// Walks the probe waits for its times before it gives up.
const PROBE_WALKS: u32 = 240;
/// The slot the probe's shadow map is kept under: no box's.
const PROBE_SLOT: u64 = u64::MAX;

/// The first measure of a device: a fixed world drawn a few times off
/// screen, whose time on the GPU starts every box at the quality the
/// device can carry ([`crate::world::governor::probed_quality`]).
#[derive(Default)]
enum Probe {
    /// No world was painted yet.
    #[default]
    Unasked,
    /// Drawn `sent` times into `target`, at `samples`; `times` came back.
    Running { target: u64, samples: u8, frame: Rc<WorldFrame>, sent: usize, times: Vec<f64>, walks: u32 },
    /// The device's measure; `None` when its times never came back.
    Done(Option<Quality>),
}
/// Walks a mesh, a texture or a segment set may sit unread.
pub const ASSET_IDLE_WALKS: u64 = 600;

struct Target {
    id: u64,
    size: (u32, u32),
    samples: u8,
    /// The generation the target holds; `None` on a new target, which
    /// holds nothing yet.
    holds: Option<u64>,
    /// The frame the target holds, when the tier said it drew it: what
    /// the next frame is told apart from. `None` while a job is out.
    held: Option<Rc<WorldFrame>>,
    /// A second target of the same size, which held the picture before:
    /// a pan slides the picture from one into the other.
    spare: Option<u64>,
    walk: u64,
}

struct Asset {
    id: u64,
    generation: u64,
    walk: u64,
}

/// A box's shadow map: its side, and the shadow it holds.
struct HeldMap {
    id: u64,
    size: u32,
    holds: Option<u64>,
    walk: u64,
}

/// What a ground holds for worlds, and when to let it go.
#[derive(Default)]
pub struct WorldResidency {
    targets: HashMap<u64, Target>,
    meshes: HashMap<u64, Asset>,
    textures: HashMap<u64, Asset>,
    segments: HashMap<u64, Asset>,
    /// Each box's shadow maps, by slot and whether it is the map of what
    /// moves.
    shadow_maps: HashMap<(u64, bool), HeldMap>,
    /// Each box's governor, by slot: the quality it draws at.
    governors: HashMap<u64, Governor>,
    /// The walk in progress.
    walk: u64,
    /// What this walk's jobs leave in their targets — kept apart until
    /// the tier says it drew them, so a walk the atlas cuts short, or a
    /// frame that never reached the GPU, draws its worlds again, whole.
    pending: Vec<(u64, u64, Rc<WorldFrame>)>,
    /// What this walk's jobs leave in their shadow maps, the same way.
    pending_shadows: Vec<((u64, bool), u64)>,
    /// Whether the last job into each target drew it whole: the governor
    /// weighs the GPU's time over it by that.
    kinds: HashMap<u64, bool>,
    /// The device's probe.
    probe: Probe,
    /// Whether this walk painted a world: the probe waits for one.
    saw_world: bool,
}

impl WorldResidency {
    /// A walk begins: whatever the last attempt promised is forgotten.
    pub(crate) fn begin_walk(&mut self, walk: u64) {
        self.walk = walk;
        self.saw_world = false;
        self.pending.clear();
        self.pending_shadows.clear();
    }

    /// The tier encoded the walk's jobs: their targets hold what they
    /// draw. A tier that never says so draws every world on every frame
    /// — slower, never wrong.
    pub fn drawn(&mut self) {
        for (slot, generation, frame) in self.pending.drain(..) {
            if let Some(target) = self.targets.get_mut(&slot) {
                target.holds = Some(generation);
                target.held = Some(frame);
            }
        }
        for (slot, generation) in self.pending_shadows.drain(..) {
            if let Some(map) = self.shadow_maps.get_mut(&slot) {
                map.holds = Some(generation);
            }
        }
    }

    /// The device is gone, and every handle with it: nothing is dropped,
    /// everything is made again on the next walk.
    pub fn forget_all(&mut self) {
        self.targets.clear();
        self.meshes.clear();
        self.textures.clear();
        self.segments.clear();
        self.shadow_maps.clear();
        self.governors.clear();
        self.pending.clear();
        self.pending_shadows.clear();
        self.kinds.clear();
    }

    /// The quality the device's probe measured it at, once the probe is
    /// done; `None` before, and on a tier that keeps no timers.
    pub fn probed(&self) -> Option<Quality> {
        match self.probe {
            Probe::Done(quality) => quality,
            _ => None,
        }
    }

    /// How many shadow maps are held.
    pub fn shadow_maps(&self) -> usize {
        self.shadow_maps.len()
    }

    /// How many targets, meshes, textures and segment sets are held.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (self.targets.len(), self.meshes.len(), self.textures.len(), self.segments.len())
    }

    /// The road of one world picture painted into a box of `box_px`
    /// device pixels: its target, and a job when the target does not hold
    /// the picture yet. `None` when the tier could not make what the
    /// picture needs.
    pub(crate) fn resolve(
        &mut self,
        ground: &mut dyn WorldGround,
        images: &dyn ImageEngine,
        shot: &Shot,
        slot: u64,
        generation: u64,
        box_px: (u32, u32),
    ) -> Option<ResolvedWorld> {
        let caps = ground.world_caps();
        self.saw_world = true;
        // a box starts where the device's probe measured it, or where its
        // name says
        let start = match self.probe {
            Probe::Done(Some(quality)) => quality,
            _ => caps.start,
        };
        let governor = self.governors.entry(slot).or_insert_with(|| Governor::new(start));
        governor.camera_moves(shot.is_camera_moving());
        let quality = governor.quality(shot.style().quality);
        let size = quality.target_size(box_px);
        // a moving camera draws at the governor's share of each side, and
        // the box stretches it; at rest, whole
        let size = if shot.is_camera_moving() { scaled(size, governor.scale()) } else { size };
        let samples = quality.samples().min(caps.samples).max(1);
        let walk = self.walk;
        let target = match self.targets.get_mut(&slot) {
            Some(target) if target.size == size && target.samples == samples => {
                target.walk = walk;
                target
            }
            held => {
                if let Some(old) = held {
                    ground.drop_target(old.id);
                    if let Some(spare) = old.spare {
                        ground.drop_target(spare);
                    }
                }
                let id = ground.make_target(size.0, size.1, samples)?;
                self.targets.insert(slot, Target { id, size, samples, holds: None, held: None, spare: None, walk });
                self.targets.get_mut(&slot).expect("the target just made")
            }
        };
        let mut id = target.id;
        if target.holds == Some(generation) {
            return Some(ResolvedWorld { target: id, size, job: None });
        }
        // until the tier says it drew the job, nobody knows what the
        // target holds: the next frame then draws whole
        let held = target.held.take();
        let frame = shot.frame_at(size, quality);
        // what of the target to draw: the rects that changed, where the
        // tier can draw them alone, after a slide where it can slide
        let reuse = held.as_deref().filter(|_| samples <= caps.redraw_samples).and_then(|held| crate::world::damage::reuse(held, &frame));
        let redraw = match reuse {
            Some(reuse) if reuse.shift == [0, 0] && reuse.rects.is_empty() => {
                // nothing it shows changed: the target holds it
                target.holds = Some(generation);
                target.held = Some(frame);
                return Some(ResolvedWorld { target: id, size, job: None });
            }
            Some(reuse) if reuse.shift == [0, 0] => Redraw::Rects(reuse.rects),
            Some(reuse) if caps.slides && samples == 1 => {
                // the picture slides into the spare, which the box shows
                // from now on; the target it leaves becomes the spare
                match target.spare.take().or_else(|| ground.make_target(size.0, size.1, samples)) {
                    Some(into) => {
                        let from = std::mem::replace(&mut target.id, into);
                        target.spare = Some(from);
                        id = into;
                        Redraw::Slide { from, shift: reuse.shift, rects: reuse.rects }
                    }
                    None => Redraw::Whole,
                }
            }
            _ => Redraw::Whole,
        };
        // the world draws: its beat is the governor's measure
        if let Some(governor) = self.governors.get_mut(&slot) {
            governor.drew(shot.time());
        }
        let mut meshes = Vec::with_capacity(frame.meshes.len());
        for mesh in &frame.meshes {
            meshes.push(self.mesh(ground, mesh)?);
        }
        let mut segments = Vec::with_capacity(frame.lines.len());
        for lines in &frame.lines {
            segments.push(self.segments(ground, lines)?);
        }
        let textures: Vec<Option<u64>> = frame.textures.iter().map(|source| self.texture(ground, source, images)).collect();
        // a texture still decoding shows its colour this frame; the
        // target is not marked, so the next walk draws it again
        if textures.iter().all(Option::is_some) {
            self.pending.push((slot, generation, Rc::clone(&frame)));
        }
        // a map is drawn again only when what it holds moved: a camera
        // that moves under a still sun reads the map it has
        let mut held = |shadow: Option<&ShadowFrame>, moving: bool| {
            let shadow = shadow?;
            let (map, holds) = self.shadow_map(ground, (slot, moving), shadow.size)?;
            let draw = holds != Some(shadow.generation);
            if draw {
                self.pending_shadows.push(((slot, moving), shadow.generation));
            }
            Some(ShadowJob { map, size: shadow.size, draw })
        };
        let shadow = held(frame.shadow.as_ref(), false);
        let moving_shadow = held(frame.moving_shadow.as_ref(), true);
        self.kinds.insert(id, redraw == Redraw::Whole);
        let job = WorldJob { target: id, size, samples, frame, meshes, textures, segments, shadow, moving_shadow, redraw };
        Some(ResolvedWorld { target: id, size, job: Some(job) })
    }

    /// The probe's next job, when a world was painted this walk and the
    /// tier keeps timers: the fixed world drawn into a target of its own,
    /// which no box composites. Its times come back through the tier's
    /// timings and end the probe ([`WorldResidency::retire`]).
    pub(crate) fn probe(&mut self, ground: &mut dyn WorldGround, images: &dyn ImageEngine) -> Option<WorldJob> {
        if !self.saw_world {
            return None;
        }
        let caps = ground.world_caps();
        if let Probe::Unasked = self.probe {
            if !caps.timers {
                self.probe = Probe::Done(None);
                return None;
            }
            let samples = Quality::Medium.samples().min(caps.samples).max(1);
            let Some(target) = ground.make_target(PROBE_SIZE.0, PROBE_SIZE.1, samples) else {
                self.probe = Probe::Done(None);
                return None;
            };
            self.probe = Probe::Running { target, samples, frame: Rc::new(probe_frame()), sent: 0, times: Vec::new(), walks: 0 };
        }
        let Probe::Running { target, samples, frame, sent, walks, .. } = &mut self.probe else { return None };
        *walks += 1;
        if *walks > PROBE_WALKS {
            // its times never came back: the boxes start from the name
            let target = *target;
            ground.drop_target(target);
            self.probe = Probe::Done(None);
            return None;
        }
        if *sent >= PROBE_FRAMES {
            return None;
        }
        *sent += 1;
        let first = *sent == 1;
        let (target, samples, frame) = (*target, *samples, Rc::clone(frame));
        let mut meshes = Vec::with_capacity(frame.meshes.len());
        for mesh in &frame.meshes {
            meshes.push(self.mesh(ground, mesh)?);
        }
        let textures: Vec<Option<u64>> = frame.textures.iter().map(|source| self.texture(ground, source, images)).collect();
        // the casters once, as a box draws them while its camera moves: the
        // middle of the three times leaves the first out
        let shadow = frame.shadow.as_ref().and_then(|shadow| {
            let (map, _) = self.shadow_map(ground, (PROBE_SLOT, false), shadow.size)?;
            Some(ShadowJob { map, size: shadow.size, draw: first })
        });
        Some(WorldJob { target, size: PROBE_SIZE, samples, frame, meshes, textures, segments: Vec::new(), shadow, moving_shadow: None, redraw: Redraw::Whole })
    }

    /// The box's shadow map of `size`, made when it has none of that
    /// side, with the generation it holds.
    fn shadow_map(&mut self, ground: &mut dyn WorldGround, slot: (u64, bool), size: u32) -> Option<(u64, Option<u64>)> {
        let walk = self.walk;
        if let Some(held) = self.shadow_maps.get_mut(&slot) {
            if held.size == size {
                held.walk = walk;
                return Some((held.id, held.holds));
            }
            let old = held.id;
            self.shadow_maps.remove(&slot);
            ground.drop_shadow_map(old);
        }
        let id = ground.make_shadow_map(size)?;
        self.shadow_maps.insert(slot, HeldMap { id, size, holds: None, walk });
        Some((id, None))
    }

    fn mesh(&mut self, ground: &mut dyn WorldGround, mesh: &Mesh) -> Option<u64> {
        let walk = self.walk;
        if let Some(asset) = self.meshes.get_mut(&mesh.id()) {
            if asset.generation == mesh.generation() {
                asset.walk = walk;
                return Some(asset.id);
            }
            ground.drop_mesh(asset.id);
            self.meshes.remove(&mesh.id());
        }
        let id = ground.make_mesh(&mesh.vertex_bytes(), &mesh.index_bytes(), &mesh.deform_bytes())?;
        self.meshes.insert(mesh.id(), Asset { id, generation: mesh.generation(), walk });
        Some(id)
    }

    fn texture(&mut self, ground: &mut dyn WorldGround, source: &ImageSource, images: &dyn ImageEngine) -> Option<u64> {
        let walk = self.walk;
        let key = source.key();
        if let Some(asset) = self.textures.get_mut(&key) {
            asset.walk = walk;
            return Some(asset.id);
        }
        let texture = crate::world::texture::texture_of(images, source)?;
        let levels: Vec<&[u8]> = texture.levels.iter().map(|level| level.rgba.as_slice()).collect();
        let base = &texture.levels[0];
        let id = ground.make_texture(base.width, base.height, &levels)?;
        self.textures.insert(key, Asset { id, generation: 0, walk });
        Some(id)
    }

    fn segments(&mut self, ground: &mut dyn WorldGround, lines: &Lines) -> Option<u64> {
        let walk = self.walk;
        if let Some(asset) = self.segments.get_mut(&lines.id()) {
            if asset.generation == lines.generation() {
                asset.walk = walk;
                return Some(asset.id);
            }
            ground.drop_segments(asset.id);
            self.segments.remove(&lines.id());
        }
        let id = ground.make_segments(&segment_bytes(lines.segments()))?;
        self.segments.insert(lines.id(), Asset { id, generation: lines.generation(), walk });
        Some(id)
    }

    /// Gives back what no walk read for a while. The walk asks at its end.
    pub(crate) fn retire(&mut self, ground: &mut dyn WorldGround) {
        // what the GPU took over the jobs that finished: each box's
        // governor weighs its own
        for timing in ground.take_timings().into_iter().filter(|timing| WorldTiming::plausible(timing.seconds)) {
            if let Probe::Running { target, times, .. } = &mut self.probe
                && *target == timing.target
            {
                times.push(timing.seconds);
                if times.len() >= PROBE_FRAMES {
                    // the middle time of the three: the device's measure
                    times.sort_by(f64::total_cmp);
                    let quality = crate::world::governor::probed_quality(times[times.len() / 2]);
                    ground.drop_target(timing.target);
                    if let Some(map) = self.shadow_maps.remove(&(PROBE_SLOT, false)) {
                        ground.drop_shadow_map(map.id);
                    }
                    for governor in self.governors.values_mut() {
                        governor.probed(quality);
                    }
                    self.probe = Probe::Done(Some(quality));
                }
                continue;
            }
            let whole = self.kinds.get(&timing.target).copied().unwrap_or(true);
            let slot = self.targets.iter().find(|(_, target)| target.id == timing.target || target.spare == Some(timing.target)).map(|(slot, _)| *slot);
            if let Some(governor) = slot.and_then(|slot| self.governors.get_mut(&slot)) {
                governor.timed(timing.seconds, whole);
            }
        }
        let walk = self.walk;
        let idle = |since: u64, limit: u64| walk.wrapping_sub(since) > limit;
        self.targets.retain(|_, target| {
            let keep = !idle(target.walk, TARGET_IDLE_WALKS);
            if !keep {
                ground.drop_target(target.id);
                if let Some(spare) = target.spare {
                    ground.drop_target(spare);
                }
            }
            keep
        });
        let targets = &self.targets;
        self.governors.retain(|slot, _| targets.contains_key(slot));
        self.kinds.retain(|id, _| targets.values().any(|target| target.id == *id || target.spare == Some(*id)));
        self.shadow_maps.retain(|_, map| {
            let keep = !idle(map.walk, TARGET_IDLE_WALKS);
            if !keep {
                ground.drop_shadow_map(map.id);
            }
            keep
        });
        type Drop = fn(&mut dyn WorldGround, u64);
        let maps: [(&mut HashMap<u64, Asset>, Drop); 3] = [
            (&mut self.meshes, |ground, id| ground.drop_mesh(id)),
            (&mut self.textures, |ground, id| ground.drop_texture(id)),
            (&mut self.segments, |ground, id| ground.drop_segments(id)),
        ];
        for (map, drop) in maps {
            map.retain(|_, asset| {
                let keep = !idle(asset.walk, ASSET_IDLE_WALKS);
                if !keep {
                    drop(ground, asset.id);
                }
                keep
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::image_engine::RawImages;
    use crate::layout::Color;
    use crate::world::frame::{Quality, WorldStyle};
    use crate::world::material::{LineStyle, Material};
    use crate::world::math::Vec3;
    use crate::world::camera::Camera;
    use crate::world::store::{World, WorldData};

    /// A ground that keeps a ledger of what it made and gave back.
    #[derive(Default)]
    struct Ledger {
        next: u64,
        live: HashSet<u64>,
        made: Vec<&'static str>,
        dropped: Vec<&'static str>,
        refuse: bool,
        samples: u8,
        start: Quality,
        redraw_samples: u8,
        slides: bool,
        timers: bool,
        /// What the GPU took over the jobs, handed over on the next ask.
        timings: Vec<WorldTiming>,
    }

    impl Ledger {
        fn new() -> Ledger {
            Ledger { samples: 4, start: Quality::High, redraw_samples: 4, slides: true, timers: true, ..Ledger::default() }
        }

        fn mint(&mut self, what: &'static str) -> Option<u64> {
            if self.refuse {
                return None;
            }
            self.next += 1;
            self.live.insert(self.next);
            self.made.push(what);
            Some(self.next)
        }

        fn give_back(&mut self, what: &'static str, id: u64) {
            assert!(self.live.remove(&id), "{what} {id} given back twice, or never made");
            self.dropped.push(what);
        }

        fn count(list: &[&'static str], what: &str) -> usize {
            list.iter().filter(|item| **item == what).count()
        }
    }

    impl WorldGround for Ledger {
        fn world_caps(&self) -> WorldCaps {
            WorldCaps { samples: self.samples, start: self.start, redraw_samples: self.redraw_samples, slides: self.slides, timers: self.timers }
        }
        fn make_target(&mut self, _: u32, _: u32, _: u8) -> Option<u64> {
            self.mint("target")
        }
        fn drop_target(&mut self, id: u64) {
            self.give_back("target", id);
        }
        fn make_mesh(&mut self, vertices: &[u8], indices: &[u8], _: &[u8]) -> Option<u64> {
            assert!(!vertices.is_empty() && !indices.is_empty());
            self.mint("mesh")
        }
        fn drop_mesh(&mut self, id: u64) {
            self.give_back("mesh", id);
        }
        fn make_texture(&mut self, _: u32, _: u32, _: &[&[u8]]) -> Option<u64> {
            self.mint("texture")
        }
        fn drop_texture(&mut self, id: u64) {
            self.give_back("texture", id);
        }
        fn make_segments(&mut self, _: &[u8]) -> Option<u64> {
            self.mint("segments")
        }
        fn drop_segments(&mut self, id: u64) {
            self.give_back("segments", id);
        }
        fn make_shadow_map(&mut self, _: u32) -> Option<u64> {
            self.mint("shadow map")
        }
        fn drop_shadow_map(&mut self, id: u64) {
            self.give_back("shadow map", id);
        }
        fn take_timings(&mut self) -> Vec<WorldTiming> {
            std::mem::take(&mut self.timings)
        }
    }

    fn shot(world: &World, quality: Quality) -> Shot {
        let style = WorldStyle { quality, ..WorldStyle::default() };
        Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 5.0), style, 0.0, (100.0, 80.0))
    }

    fn cube_world() -> (World, crate::world::store::Entity) {
        let world = World::new();
        let cube = world.add(crate::world::mesh::Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(200, 90, 50)));
        (world, cube)
    }

    #[test]
    fn a_held_picture_makes_no_job() {
        let (world, cube) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        let first = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (200, 160)).expect("a target");
        let job = first.job.expect("a new target draws");
        assert_eq!((job.size, job.samples, job.meshes.len()), ((200, 160), 4, 1));
        residency.drawn();
        residency.begin_walk(2);
        let again = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (200, 160)).expect("the target");
        assert!(again.job.is_none(), "the target holds the picture");
        assert_eq!(again.target, first.target);
        residency.begin_walk(3);
        let quiet = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 101, (200, 160)).expect("the target");
        assert!(quiet.job.is_none(), "a new generation that shows the same picture draws nothing");
        residency.begin_walk(4);
        cube.set_tint(Color::rgb(30, 60, 200));
        let moved = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 102, (200, 160)).expect("the target");
        assert!(moved.job.is_some(), "a new picture draws");
        assert_eq!(moved.target, first.target, "into the same target");
        assert_eq!(Ledger::count(&ground.made, "mesh"), 1, "the mesh went up once");
    }

    #[test]
    fn a_picture_that_never_reached_the_gpu_draws_again() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        assert!(residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 100, (64, 64)).unwrap().job.is_some());
        // no `drawn`: the walk was cut short, or the frame never encoded
        residency.begin_walk(2);
        assert!(residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 100, (64, 64)).unwrap().job.is_some());
    }

    #[test]
    fn a_new_size_or_sample_count_takes_a_new_target() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        let small = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (64, 64)).unwrap();
        residency.drawn();
        let big = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (96, 64)).unwrap();
        assert_ne!(small.target, big.target);
        assert!(big.job.is_some());
        let low = residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 100, (96, 64)).unwrap();
        assert_eq!(low.job.expect("one sample").samples, 1);
        assert_eq!(Ledger::count(&ground.dropped, "target"), 2, "each old target was given back");
        assert_eq!(residency.counts().0, 1);
    }

    #[test]
    fn the_samples_follow_the_quality_and_the_tier() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        ground.samples = 1;
        let images = RawImages::default();
        residency.begin_walk(1);
        let job = residency.resolve(&mut ground, &images, &shot(&world, Quality::High), 7, 100, (64, 64)).unwrap().job.unwrap();
        assert_eq!(job.samples, 1, "a tier of one sample draws one");
    }

    #[test]
    fn a_revised_mesh_goes_up_again_and_the_old_comes_down() {
        let world = World::new();
        let mesh = crate::world::mesh::Mesh::cuboid(1.0, 1.0, 1.0);
        let entity = world.add(mesh.clone(), Material::color(Color::rgb(200, 90, 50)));
        world.add_lines(mesh.edges(30.0), LineStyle::solid(Color::rgb(0, 0, 0), 1.0));
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 1, (64, 64)).unwrap();
        residency.drawn();
        let taller = mesh.revise(crate::world::mesh::Mesh::cuboid(1.0, 2.0, 1.0).data().clone());
        entity.set_shape(crate::world::store::Shape::Model(crate::world::material::Model::single(taller, Material::color(Color::rgb(200, 90, 50)))));
        residency.begin_walk(2);
        let job = residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 2, (64, 64)).unwrap().job.unwrap();
        assert_eq!(job.meshes.len(), 1);
        assert_eq!(Ledger::count(&ground.made, "mesh"), 2);
        assert_eq!(Ledger::count(&ground.dropped, "mesh"), 1, "the first generation came down");
        assert_eq!(Ledger::count(&ground.made, "segments"), 1, "the lines did not move");
    }

    #[test]
    fn what_no_walk_reads_is_given_back() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 1, (64, 64)).unwrap();
        residency.drawn();
        residency.begin_walk(1 + TARGET_IDLE_WALKS + 1);
        residency.retire(&mut ground);
        assert_eq!(residency.counts(), (0, 1, 0, 0), "the target went first");
        residency.begin_walk(1 + ASSET_IDLE_WALKS + 1);
        residency.retire(&mut ground);
        assert_eq!(residency.counts(), (0, 0, 0, 0));
        assert!(ground.live.is_empty(), "everything made was given back");
    }

    #[test]
    fn a_tier_that_cannot_make_a_target_shows_the_oracle() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        ground.refuse = true;
        let images = RawImages::default();
        residency.begin_walk(1);
        assert!(residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 1, (64, 64)).is_none());
    }

    #[test]
    fn a_shadow_is_drawn_once_and_read_while_the_camera_moves() {
        use crate::world::material::Light;
        let (world, cube) = cube_world();
        world.add(crate::world::mesh::Mesh::plane(8.0, 8.0), Material::color(Color::rgb(200, 196, 188))).set_position(Vec3 { x: 0.0, y: -0.5, z: 0.0 });
        world.add_light(Light::sun().toward(Vec3 { x: 0.5, y: -1.0, z: 0.7 }).with_shadows());
        let at = |yaw: f64, quality: Quality| {
            let style = WorldStyle { quality, ..WorldStyle::default() };
            Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, yaw, 25.0, 5.0), style, 0.0, (100.0, 80.0))
        };
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        let mut walk = 0;
        let mut shadow_of = |residency: &mut WorldResidency, ground: &mut Ledger, shot: &Shot, generation: u64| {
            walk += 1;
            residency.begin_walk(walk);
            let job = residency.resolve(ground, &images, shot, 7, generation, (200, 160)).expect("a target").job.expect("a job");
            residency.drawn();
            job.shadow.expect("a shadow map")
        };
        let first = shadow_of(&mut residency, &mut ground, &at(30.0, Quality::Medium), 1);
        assert!(first.draw, "a new map draws its casters");
        assert_eq!(first.size, 2048);
        let turned = shadow_of(&mut residency, &mut ground, &at(70.0, Quality::Medium), 2);
        assert_eq!((turned.map, turned.draw), (first.map, false), "the camera turned: the map holds");
        cube.set_position(Vec3 { x: 0.0, y: 0.2, z: 0.0 });
        let moved = shadow_of(&mut residency, &mut ground, &at(70.0, Quality::Medium), 3);
        assert_eq!((moved.map, moved.draw), (first.map, true), "a caster moved: the same map, drawn again");
        let low = shadow_of(&mut residency, &mut ground, &at(70.0, Quality::Low), 4);
        assert_ne!(low.map, first.map, "a smaller map for a lower quality");
        assert!(low.draw);
        assert_eq!(Ledger::count(&ground.made, "shadow map"), 2);
        assert_eq!(Ledger::count(&ground.dropped, "shadow map"), 1, "the larger map came down");
        residency.begin_walk(walk + TARGET_IDLE_WALKS + 1);
        residency.retire(&mut ground);
        assert_eq!(residency.shadow_maps(), 0, "an idle map is given back");
        assert!(!ground.live.iter().any(|id| *id == low.map));
    }

    #[test]
    fn a_figure_that_walks_redraws_its_rects_and_a_lost_frame_draws_whole() {
        let (world, cube) = cube_world();
        world.add(crate::world::mesh::Mesh::plane(8.0, 8.0), Material::color(Color::rgb(200, 196, 188))).set_position(Vec3 { x: 0.0, y: -0.5, z: 0.0 });
        let figure = world.add(crate::world::mesh::Mesh::cuboid(0.2, 0.6, 0.2), Material::color(Color::rgb(40, 140, 90)));
        figure.set_position(Vec3 { x: -1.5, y: -0.2, z: 1.0 }).move_to(Vec3 { x: -0.5, y: -0.2, z: 1.5 }, 1.0);
        let at = |time: f64| {
            let style = WorldStyle { quality: Quality::Medium, ..WorldStyle::default() };
            Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 8.0), style, time, (100.0, 80.0))
        };
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        let mut walk = 0;
        let mut job_of = |residency: &mut WorldResidency, ground: &mut Ledger, time: f64, generation: u64, drawn: bool| {
            walk += 1;
            residency.begin_walk(walk);
            let job = residency.resolve(ground, &images, &at(time), 7, generation, (200, 160)).expect("a target").job;
            if drawn {
                residency.drawn();
            }
            job
        };
        let first = job_of(&mut residency, &mut ground, 0.1, 1, true).expect("a new target");
        assert_eq!(first.redraw, Redraw::Whole);
        let Some(Redraw::Rects(rects)) = job_of(&mut residency, &mut ground, 0.2, 2, true).map(|job| job.redraw) else {
            panic!("the walk redraws a part");
        };
        assert!(!rects.is_empty() && rects.iter().all(|r| r[2] * r[3] < 200 * 160 / 2), "{rects:?}");
        // a job the tier never said it drew: the next one is whole
        assert!(matches!(job_of(&mut residency, &mut ground, 0.3, 3, false).map(|job| job.redraw), Some(Redraw::Rects(_))));
        assert_eq!(job_of(&mut residency, &mut ground, 0.4, 4, true).map(|job| job.redraw), Some(Redraw::Whole));
        // the walk is over and only the clock moves: no job at all
        assert!(job_of(&mut residency, &mut ground, 5.0, 5, true).is_some(), "it arrives");
        assert!(job_of(&mut residency, &mut ground, 6.0, 6, true).is_none(), "nothing seen changed");
        // a tier that draws every frame whole is never asked for rects
        cube.set_tint(Color::rgb(10, 10, 200));
        ground.redraw_samples = 0;
        assert_eq!(job_of(&mut residency, &mut ground, 6.0, 7, true).map(|job| job.redraw), Some(Redraw::Whole));
    }

    #[test]
    fn a_pan_at_low_slides_between_two_targets() {
        let (world, _) = cube_world();
        world.add(crate::world::mesh::Mesh::plane(8.0, 8.0), Material::color(Color::rgb(200, 196, 188))).set_position(Vec3 { x: 0.0, y: -0.5, z: 0.0 });
        let at = |x: f64| {
            let mut camera = Camera::orbiting(Vec3 { x, y: 0.0, z: 0.0 }, 45.0, 35.0, 20.0);
            camera.lens = crate::world::camera::Lens::Parallel { height: 6.0 };
            let style = WorldStyle { quality: Quality::Low, ..WorldStyle::default() };
            Shot::new(world.with(WorldData::clone), camera, style, 0.0, (100.0, 80.0))
        };
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        ground.start = Quality::Low;
        let images = RawImages::default();
        let mut walk = 0;
        let mut resolve = |residency: &mut WorldResidency, ground: &mut Ledger, x: f64, generation: u64| {
            walk += 1;
            residency.begin_walk(walk);
            let resolved = residency.resolve(ground, &images, &at(x), 7, generation, (200, 160)).expect("a target");
            residency.drawn();
            resolved
        };
        let first = resolve(&mut residency, &mut ground, 0.0, 1);
        assert_eq!(first.job.as_ref().map(|job| &job.redraw), Some(&Redraw::Whole));
        // the eye pans: the picture slides into a second target, which the
        // box shows from now on
        let second = resolve(&mut residency, &mut ground, 0.3, 2);
        let Some(Redraw::Slide { from, shift, rects }) = second.job.map(|job| job.redraw) else { panic!("a slide") };
        assert_eq!(from, first.target);
        assert_ne!(second.target, first.target);
        assert!(shift != [0, 0] && !rects.is_empty());
        // and back: the first target is the spare it slides into
        let third = resolve(&mut residency, &mut ground, 0.55, 3);
        assert!(matches!(third.job.map(|job| job.redraw), Some(Redraw::Slide { from, .. }) if from == second.target));
        assert_eq!(third.target, first.target);
        assert_eq!(Ledger::count(&ground.made, "target"), 2, "two targets, made once each");
        // a tier that cannot slide draws the pan whole
        ground.slides = false;
        let fourth = resolve(&mut residency, &mut ground, 0.8, 4);
        assert_eq!(fourth.job.map(|job| job.redraw), Some(Redraw::Whole));
        // an idle box gives both back
        residency.begin_walk(walk + TARGET_IDLE_WALKS + 1);
        residency.retire(&mut ground);
        assert_eq!(Ledger::count(&ground.dropped, "target"), 2);
    }

    #[test]
    fn a_world_the_gpu_takes_long_over_steps_down() {
        let (world, cube) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        for walk in 1..=30u64 {
            residency.begin_walk(walk);
            // a new picture each walk, drawn whole
            cube.set_tint(Color::rgb(walk as u8, 90, 50));
            let style = WorldStyle { quality: Quality::High, ..WorldStyle::default() };
            let shot = Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, walk as f64, 25.0, 5.0), style, 0.0, (100.0, 80.0));
            let resolved = residency.resolve(&mut ground, &images, &shot, 7, walk, (200, 160)).expect("a target");
            let job = resolved.job.expect("a job");
            residency.drawn();
            // the tier read 14 ms on its clock
            ground.timings.push(WorldTiming { target: job.target, seconds: 0.014 });
            residency.retire(&mut ground);
        }
        let quality = residency.governors.get(&7).expect("the box's governor").quality(Quality::High);
        assert_eq!(quality, Quality::Medium, "a window of whole frames over 10 ms steps down once");
    }

    #[test]
    fn a_clock_that_lies_moves_no_governor() {
        let (world, cube) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        for walk in 1..=30u64 {
            residency.begin_walk(walk);
            cube.set_tint(Color::rgb(walk as u8, 90, 50));
            let style = WorldStyle { quality: Quality::High, ..WorldStyle::default() };
            let shot = Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, walk as f64, 25.0, 5.0), style, 0.0, (100.0, 80.0));
            let job = residency.resolve(&mut ground, &images, &shot, 7, walk, (200, 160)).expect("a target").job.expect("a job");
            residency.drawn();
            // the time since boot, or nothing: a driver lost the start
            let seconds = if walk % 2 == 0 { 150.0 } else { 0.0 };
            ground.timings.push(WorldTiming { target: job.target, seconds });
            residency.retire(&mut ground);
        }
        assert_eq!(residency.governors[&7].quality(Quality::High), Quality::High);
    }

    #[test]
    fn a_moving_camera_draws_fewer_pixels_while_it_misses_and_whole_at_rest() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        let style = WorldStyle { quality: Quality::High, ..WorldStyle::default() };
        let shot = |yaw: f64, moving: bool| {
            Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, yaw, 25.0, 5.0), style.clone(), 0.0, (100.0, 80.0)).camera_moving(moving)
        };
        let mut sizes = Vec::new();
        for walk in 1..=31u64 {
            residency.begin_walk(walk);
            let resolved = residency.resolve(&mut ground, &images, &shot(walk as f64, true), 7, walk, (200, 160)).expect("a target");
            sizes.push(resolved.size);
            let job = resolved.job.expect("a job");
            residency.drawn();
            ground.timings.push(WorldTiming { target: job.target, seconds: 0.014 });
            residency.retire(&mut ground);
        }
        // a window over budget takes a step of pixels, not of quality
        assert_eq!((sizes[0], sizes[30]), ((200, 160), (160, 128)));
        assert_eq!(residency.governors[&7].quality(Quality::High), Quality::High);
        // the camera rests: the same picture, drawn whole
        residency.begin_walk(32);
        let resolved = residency.resolve(&mut ground, &images, &shot(31.0, false), 7, 32, (200, 160)).expect("a target");
        assert_eq!(resolved.size, (200, 160));
        assert_eq!(resolved.job.expect("a job").redraw, Redraw::Whole);
    }

    #[test]
    fn the_device_is_probed_before_its_boxes_are_weighed() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        ground.start = Quality::Low;
        let images = RawImages::default();
        let style = WorldStyle { quality: Quality::High, ..WorldStyle::default() };
        let shot = |yaw: f64| Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, yaw, 25.0, 5.0), style.clone(), 0.0, (100.0, 80.0));
        // no world painted yet: no probe
        residency.begin_walk(1);
        assert!(residency.probe(&mut ground, &images).is_none());
        let mut probes = Vec::new();
        for walk in 2..6u64 {
            residency.begin_walk(walk);
            residency.resolve(&mut ground, &images, &shot(walk as f64), 7, walk, (200, 160)).expect("a target");
            if let Some(job) = residency.probe(&mut ground, &images) {
                assert_eq!((job.size, job.samples, job.redraw.clone()), (PROBE_SIZE, 4, Redraw::Whole));
                assert_eq!(job.shadow.map(|shadow| shadow.draw), Some(probes.is_empty()), "the casters once");
                // a fast device: under half a millisecond at 640 × 360
                ground.timings.push(WorldTiming { target: job.target, seconds: 0.0004 });
                probes.push(job.target);
            }
            residency.drawn();
            residency.retire(&mut ground);
        }
        assert_eq!(probes.len(), PROBE_FRAMES, "three frames, then done");
        assert!(probes.iter().all(|target| *target == probes[0]), "into one target");
        assert!(!ground.live.contains(&probes[0]), "which is given back");
        // the box drawn before the measure, and a new one, start High
        assert_eq!(residency.governors[&7].quality(Quality::High), Quality::High);
        residency.begin_walk(10);
        residency.resolve(&mut ground, &images, &shot(10.0), 8, 10, (200, 160)).expect("a target");
        assert_eq!(residency.governors[&8].quality(Quality::High), Quality::High);
        // a tier without timers is never probed
        let (mut plain, mut quiet) = (Ledger::new(), WorldResidency::default());
        plain.timers = false;
        quiet.begin_walk(1);
        quiet.resolve(&mut plain, &images, &shot(1.0), 7, 1, (200, 160)).expect("a target");
        assert!(quiet.probe(&mut plain, &images).is_none());
    }

    #[test]
    fn a_tier_without_shadow_maps_draws_in_full_sun() {
        use crate::world::material::Light;
        let (world, _) = cube_world();
        world.add_light(Light::sun().with_shadows());
        struct Plain(Ledger);
        impl WorldGround for Plain {
            fn world_caps(&self) -> WorldCaps {
                self.0.world_caps()
            }
            fn make_target(&mut self, w: u32, h: u32, s: u8) -> Option<u64> {
                self.0.make_target(w, h, s)
            }
            fn drop_target(&mut self, id: u64) {
                self.0.drop_target(id)
            }
            fn make_mesh(&mut self, v: &[u8], i: &[u8], d: &[u8]) -> Option<u64> {
                self.0.make_mesh(v, i, d)
            }
            fn drop_mesh(&mut self, id: u64) {
                self.0.drop_mesh(id)
            }
            fn make_texture(&mut self, w: u32, h: u32, l: &[&[u8]]) -> Option<u64> {
                self.0.make_texture(w, h, l)
            }
            fn drop_texture(&mut self, id: u64) {
                self.0.drop_texture(id)
            }
            fn make_segments(&mut self, s: &[u8]) -> Option<u64> {
                self.0.make_segments(s)
            }
            fn drop_segments(&mut self, id: u64) {
                self.0.drop_segments(id)
            }
        }
        let mut ground = Plain(Ledger::new());
        let mut residency = WorldResidency::default();
        residency.begin_walk(1);
        let job = residency.resolve(&mut ground, &RawImages::default(), &shot(&world, Quality::Low), 7, 1, (64, 64)).unwrap().job.unwrap();
        assert!(job.frame.shadow.is_some(), "the frame asks for a shadow");
        assert!(job.shadow.is_none(), "the tier has no map: the picture still draws");
    }

    #[test]
    fn the_device_starts_the_quality_and_slow_frames_step_it_down() {
        let (world, cube) = cube_world();
        let shot_at = |time: f64, yaw: f64| {
            let style = WorldStyle { quality: Quality::High, ..WorldStyle::default() };
            Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, yaw, 25.0, 5.0), style, time, (100.0, 80.0))
        };
        let images = RawImages::default();
        // a device whose name starts it at the middle draws four samples
        // and the middle's shadows, though the box asks for the most
        let (mut ground, mut residency) = (Ledger { start: Quality::Medium, ..Ledger::new() }, WorldResidency::default());
        residency.begin_walk(1);
        let job = residency.resolve(&mut ground, &images, &shot_at(0.0, 30.0), 7, 1, (200, 160)).unwrap().job.unwrap();
        assert_eq!(job.frame.samples, 4);
        assert_eq!(job.frame.uniforms.shadow, [0.0; 4], "no sun with shadows here");
        // a run of long frames while the camera turns steps it down
        let mut time = 0.0;
        for walk in 2..9u64 {
            time += 0.08;
            residency.begin_walk(walk);
            residency.drawn();
            let job = residency.resolve(&mut ground, &images, &shot_at(time, 30.0 + walk as f64), 7, walk, (200, 160)).unwrap().job.unwrap();
            if walk == 8 {
                assert_eq!(job.frame.samples, 1, "down to one sample");
            }
        }
        let _ = cube;
    }

    #[test]
    fn a_huge_box_is_drawn_inside_the_budget() {
        assert_eq!(Quality::Low.target_size((800, 600)), (800, 600));
        let (w, h) = Quality::Low.target_size((3840, 2160));
        assert!(w * h <= Quality::Low.pixel_budget() && w * h > Quality::Low.pixel_budget() * 99 / 100);
        assert!(((w as f64 / h as f64) - 16.0 / 9.0).abs() < 0.01, "the shape holds");
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let (world, _) = cube_world();
        residency.begin_walk(1);
        let picture = residency.resolve(&mut ground, &RawImages::default(), &shot(&world, Quality::Low), 7, 1, (3840, 2160)).unwrap();
        assert_eq!(picture.size, (w, h));
        assert_eq!(picture.job.unwrap().frame.size, (w, h), "the frame is decided at the target's size");
    }
}
