//! A world in a box: the picture a box paints of it.
//!
//! [`Painter::world`] reads the world and the camera — both reads belong
//! to the box's paint, so a change to either repaints the box and runs
//! no body — and leaves ONE image in the display list. The image carries
//! a frozen shot of the world and names a slot: the target the tier keeps
//! for this box. Its generation is a hash of everything the picture
//! shows, so a frame that changed nothing in the world draws nothing
//! again, and a still world costs no frame at all.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use motor::state::State;

use super::camera::Camera;
use super::frame::{Background, WorldFrame, WorldStyle};
use super::store::{World, WorldData};
use crate::custom::Painter;
use crate::image_engine::{ImageRaster, ImageSource};
use crate::layout::Rect;

/// Where a camera comes from: a fixed one, or a `State<Camera>` the box
/// reads (and repaints when it moves).
pub trait CameraSource: Copy + 'static {
    fn camera(&self) -> Camera;
}

impl CameraSource for Camera {
    fn camera(&self) -> Camera {
        *self
    }
}

impl CameraSource for State<Camera> {
    fn camera(&self) -> Camera {
        self.with(|camera| *camera)
    }
}

/// One picture of a world, frozen at the moment a box painted it.
pub struct Shot {
    world: WorldData,
    camera: Camera,
    style: WorldStyle,
    time: f64,
    /// The box, in points.
    points: (f64, f64),
    frame: RefCell<Option<Decided>>,
}

/// A frame decided for a target size.
type Decided = ((u32, u32), Rc<WorldFrame>);

impl Shot {
    /// A picture of `world` through `camera` (sampled at `time`), for a
    /// box of `points`.
    pub(crate) fn new(world: WorldData, camera: Camera, style: WorldStyle, time: f64, points: (f64, f64)) -> Shot {
        Shot { world, camera, style, time, points, frame: RefCell::new(None) }
    }

    /// The box's size in points, rounded.
    pub fn points(&self) -> (u32, u32) {
        (self.points.0.round().max(0.0) as u32, self.points.1.round().max(0.0) as u32)
    }

    /// The style the box shows its world with.
    pub fn style(&self) -> &WorldStyle {
        &self.style
    }

    /// The frame for a target of `size` pixels, decided once per size.
    pub fn frame(&self, size: (u32, u32)) -> Rc<WorldFrame> {
        if let Some((held, frame)) = self.frame.borrow().as_ref()
            && *held == size
        {
            return Rc::clone(frame);
        }
        let px_per_point = if self.points.0 > 0.0 { (size.0 as f64 / self.points.0) as f32 } else { 1.0 };
        let frame = Rc::new(super::frame::build_frame(&self.world, &self.camera, &self.style, size, px_per_point, self.time));
        *self.frame.borrow_mut() = Some((size, Rc::clone(&frame)));
        frame
    }
}

thread_local! {
    /// The paint that painted the last world, and how many worlds it has
    /// painted: two worlds in one box take two slots, and the count starts
    /// again with every paint.
    static ORDINAL: Cell<(u64, u32)> = const { Cell::new((0, 0)) };
}

/// The most pictures a second a world in motion asks for, unless its
/// style says fewer.
const MOTION_FPS: f64 = 120.0;

impl Painter<'_> {
    /// Paints `world` through `camera` into `rect`, in the box's own
    /// coordinates, with the default style: no backdrop of its own, medium
    /// quality. Paint the box's own marks after it, and they lie on top.
    pub fn world(&mut self, rect: Rect, world: World, camera: impl CameraSource) {
        self.world_styled(rect, world, camera, &WorldStyle::default());
    }

    /// [`Painter::world`] with a style: a backdrop, a quality, a section.
    pub fn world_styled(&mut self, rect: Rect, world: World, camera: impl CameraSource, style: &WorldStyle) {
        let time = motor::task::now();
        let held = camera.camera();
        let moving = held.is_flying(time);
        let fps = style.max_fps.unwrap_or(MOTION_FPS).max(1.0);
        if moving {
            crate::anim::pulse(fps);
        }
        let seen = held.at(time);
        let snapshot = world.with(WorldData::clone);
        let version = world.version();

        let (owner, serial) = motor::identity::painting_key();
        let ordinal = ORDINAL.with(|cell| {
            let (last, count) = cell.get();
            let next = if last == serial && serial != 0 { count + 1 } else { 0 };
            cell.set((serial, next));
            next
        });
        let mut slot = motor::hash::FxHasher::default();
        std::hash::Hasher::write_u64(&mut slot, owner);
        std::hash::Hasher::write_u32(&mut slot, ordinal);

        let mut generation = motor::hash::FxHasher::default();
        let mut put = |value: u64| std::hash::Hasher::write_u64(&mut generation, value);
        put(version.0);
        put(version.1);
        for value in [seen.eye.x, seen.eye.y, seen.eye.z, seen.target.x, seen.target.y, seen.target.z, seen.up.x, seen.up.y, seen.up.z] {
            put(value.to_bits());
        }
        match seen.lens {
            super::camera::Lens::Perspective { fov_y_degrees } => put(fov_y_degrees.to_bits()),
            super::camera::Lens::Parallel { height } => put(height.to_bits() ^ 1),
        }
        put(seen.near.map_or(0, f64::to_bits));
        put(seen.far.map_or(0, f64::to_bits));
        put(rect.size.width.to_bits());
        put(rect.size.height.to_bits());
        put(style_hash(style));
        if moving {
            // a capped motion moves its picture at most `fps` times a second
            put((time * fps).floor() as u64);
        }

        let shot = Shot::new(snapshot, seen, style.clone(), time, (rect.size.width, rect.size.height));
        self.image(
            rect,
            ImageSource::World {
                key: std::hash::Hasher::finish(&slot),
                generation: std::hash::Hasher::finish(&generation),
                veil: 255,
                shot: Rc::new(shot),
            },
        );
    }
}

pub(crate) fn style_hash(style: &WorldStyle) -> u64 {
    let mut hasher = motor::hash::FxHasher::default();
    let mut put = |value: u64| std::hash::Hasher::write_u64(&mut hasher, value);
    match style.background {
        Background::Transparent => put(0),
        Background::Color(c) => put(1 << 32 | u32::from_le_bytes([c.r, c.g, c.b, c.a]) as u64),
        Background::Gradient { top, bottom } => {
            put(2 << 32 | u32::from_le_bytes([top.r, top.g, top.b, top.a]) as u64);
            put(u32::from_le_bytes([bottom.r, bottom.g, bottom.b, bottom.a]) as u64);
        }
        Background::Sky { zenith, horizon, ground } => {
            put(3 << 32 | u32::from_le_bytes([zenith.r, zenith.g, zenith.b, zenith.a]) as u64);
            put(u32::from_le_bytes([horizon.r, horizon.g, horizon.b, horizon.a]) as u64);
            put(u32::from_le_bytes([ground.r, ground.g, ground.b, ground.a]) as u64);
        }
    }
    put(style.quality as u64);
    if let Some(plane) = style.section {
        for value in [plane.normal.x, plane.normal.y, plane.normal.z, plane.d] {
            put(value.to_bits());
        }
    }
    put(style.exposure.to_bits() as u64);
    put(style.xray as u64);
    put(style.max_fps.map_or(0, f64::to_bits));
    std::hash::Hasher::finish(&hasher)
}

// MARK: - The oracle's door

/// How many CPU pictures of worlds stay warm. The damage replay asks for
/// the same picture several times a frame.
const WORLD_KEEP: usize = 8;

/// A CPU picture of a world, named by slot, generation, veil and size.
type Kept = ((u64, u64, u8, usize, usize), Rc<ImageRaster>);

thread_local! {
    static WORLD_RASTERS: RefCell<Vec<Kept>> = const { RefCell::new(Vec::new()) };
}

/// The world drawn on the CPU at exactly `width × height`, straight alpha,
/// the veil multiplied in — once per picture and size.
pub(crate) fn world_raster(
    images: &dyn crate::image_engine::ImageEngine,
    key: u64,
    generation: u64,
    veil: u8,
    shot: &Shot,
    width: usize,
    height: usize,
) -> Option<Rc<ImageRaster>> {
    if width == 0 || height == 0 {
        return None;
    }
    let id = (key, generation, veil, width, height);
    if let Some(hit) = WORLD_RASTERS.with(|cache| cache.borrow().iter().find(|(held, _)| *held == id).map(|(_, raster)| Rc::clone(raster))) {
        return Some(hit);
    }
    let frame = shot.frame((width as u32, height as u32));
    let picture = super::soft::draw_with(&frame, &|index| super::texture::texture_of(images, &frame.textures[index]));
    let mut rgba = picture.unpremultiplied();
    if veil < 255 {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = ((pixel[3] as u32 * veil as u32 + 127) / 255) as u8;
        }
    }
    let raster = Rc::new(ImageRaster { width, height, rgba });
    WORLD_RASTERS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= WORLD_KEEP {
            cache.remove(0);
        }
        cache.push((id, Rc::clone(&raster)));
    });
    Some(raster)
}
