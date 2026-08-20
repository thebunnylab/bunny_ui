//! The platform-neutral half of every GPU present: the wire structs,
//! the shelf atlas, the batching and the display-list walk — shared
//! word for word by the gl and vulkan tiers (the same halves the mac
//! and windows shells keep deliberately identical across crates; one
//! crate shares them outright).
//!
//! The LAW carries over: every policy decision — snapping, radius
//! clamps, stroke thickness, shadow reach, the clip stack — resolves
//! here on the CPU in f64. The tiers below are pure evaluators.
//!
//! The one seam a tier must fill is [`AtlasGround`]: where tiles land
//! (a GL texture, a vulkan image) — the walk neither knows nor cares.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::image_engine::{ImageEngine, ImageSource, raster_source};
use crate::layout::{Color, Corners, DisplayList, DrawCommand, Rect};
use crate::raster::physical_extent;
use crate::text_engine::{FontKey, FontSpec, TextEngine};

/// The run atlas: text tiles append into one shared texture. Runs wider
/// than a chunk split into seamless chunks (texel reads are 1:1, a seam
/// cannot show). Overflow drains the in-flight frames, resets the whole
/// atlas and re-inserts the current frame — a copying collector, not a
/// per-tile free list.
pub const ATLAS_CHUNK_WIDTH: u32 = 1024;
pub const ATLAS_INITIAL_SIZE: u32 = 2048;
pub const ATLAS_MAX_SIZE: u32 = 4096;

// MARK: - The wire format shared with both tiers' shaders

/// One rect primitive: fill, stroke ring or shadow, selected by
/// `params[2]`. Everything is snapped device pixels resolved on the CPU
/// in f64 — the shader is a pure coverage evaluator.
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)] // written whole, read by the GPU — never field by field
pub struct RectInstance {
    pub rect: [f32; 4],   // x0, y0, x1, y1 (the shadow ships EXPANDED)
    pub clip: [f32; 4],   // the snapped clip-stack top
    pub params: [f32; 4], // aspect (the ellipse only), thickness/reach/first, kind, expansion/second
    pub color: [u8; 4],   // straight RGBA (a normalized attribute)
    /// A gradient's second half rides here: the far color plus one
    /// point (centre for the rings, end for the line).
    pub pad: [u8; 12],
    /// The four corners, clockwise from the top left, CLAMPED in
    /// device px — the shader only picks the one its quadrant owns.
    pub radii: [f32; 4],
}

/// The deepest level of the blur pyramid — four levels in all,
/// mirroring `crate::glass::MAX_LEVEL`.
pub const GLASS_MAX_LEVEL: u32 = 3;

/// One pane of liquid glass. Everything is snapped device pixels
/// resolved on the CPU in f64, like every other instance here — the
/// shader only evaluates the material.
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)] // written whole, read by the GPU — never field by field
pub struct GlassInstance {
    pub rect: [f32; 4],   // x0, y0, x1, y1
    pub clip: [f32; 4],   // the snapped clip-stack top
    pub radii: [f32; 4],  // the four corners, clamped
    pub lens: [f32; 4],   // blur, refraction band, amount, chromatic
    pub finish: [f32; 4], // highlight band, intensity, saturation, brightness
    pub touch: [f32; 4],  // sheen, spot x, spot y, spot radius
    pub tint: [u8; 4],    // straight RGBA (a normalized attribute)
    pub highlight: [u8; 4],
    pub spot_alpha: f32,
    pub pad: f32,
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

/// One text run (or chunk of one): a rectangle of atlas texels copied
/// 1:1 to the destination — texel fetch, no sampler, exact bytes.
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)] // written whole, read by the GPU — never field by field
pub struct SpriteInstance {
    pub dest: [f32; 4], // x0, y0, x1, y1 in device px
    pub tex: [f32; 4],  // atlas texel origin + the same extent
    pub clip: [f32; 4],
}

const _: () = {
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

// MARK: - The walk vocabulary (all policy in f64)

/// A snapped box in device pixels, `[x0, y0, x1, y1)` — the same tuple
/// the Surface uses for damage and clips.
pub type Box4 = (i64, i64, i64, i64);

pub fn box_intersect(a: Box4, b: Box4) -> Option<Box4> {
    let rect = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (rect.0 < rect.2 && rect.1 < rect.3).then_some(rect)
}

/// The mirror of `snap(scale_rect(rect, factor))` — scale origin and
/// size separately, then round each edge on its own. The operation order
/// matters: it is what makes neighbors close without a seam, and parity
/// is byte-level.
pub fn snap_scaled(rect: Rect, factor: f64) -> Box4 {
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
pub fn corner_clamp(scaled: Corners, snapped: Box4) -> Corners {
    scaled.clamped((snapped.2 - snapped.0) as f64, (snapped.3 - snapped.1) as f64)
}

/// The four corners as a shader reads them, clockwise from the top
/// left — the ONE place the field order is spoken.
pub fn wire_radii(radii: Corners) -> [f32; 4] {
    [
        radii.top_left as f32,
        radii.top_right as f32,
        radii.bottom_right as f32,
        radii.bottom_left as f32,
    ]
}

/// The curve a run is cut by, as the shaders see it — ONE per draw
/// run, bound as 32 bytes of per-run constants, never per instance.
/// Four zero radii are the straight rectangle every clip has been
/// until now — and multiplying coverage by 1.0 is exact.
#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
pub struct RoundClip {
    /// The rounded clip's OWN snapped box in device px — the cut can
    /// be smaller without the corner moving.
    pub box4: [f32; 4],
    /// The four corners. They fit the second 16-byte register the
    /// constant block was already padding out to, so the cut carries
    /// four for the price of one.
    pub radii: [f32; 4],
}

const _: () = {
    assert!(std::mem::size_of::<RoundClip>() == 32);
    assert!(std::mem::offset_of!(RoundClip, box4) == 0);
    assert!(std::mem::offset_of!(RoundClip, radii) == 16);
};

/// Slot zero of every frame — the cut that never bends.
pub const NO_ROUND: RoundClip = RoundClip { box4: [0.0; 4], radii: [0.0; 4] };

pub const KIND_FILL: f32 = 0.0;
pub const KIND_STROKE: f32 = 1.0;
pub const KIND_SHADOW: f32 = 2.0;
pub const KIND_RADIAL: f32 = 3.0;
pub const KIND_LINEAR: f32 = 4.0;
/// The elliptical rings: the ASPECT rides params.x (the corner slot),
/// start and end radii stay in params.y/.w.
pub const KIND_ELLIPTIC: f32 = 5.0;

// MARK: - The ground seam (what a tier must offer the atlas)

/// Where tiles physically land. The walk keeps every allocation
/// decision; the ground only moves bytes and mints handles.
pub trait AtlasGround {
    /// The shared texture exists at `size`×`size` (create if absent).
    fn ensure_shared(&mut self, size: u32) -> bool;
    /// One tile of straight-RGBA rows into virgin shared space.
    /// `pitch_px` is the source raster's row length in PIXELS, and
    /// `bytes` starts at the tile's first texel and runs to the end of
    /// the raster — the rows after the first are `pitch_px` apart.
    fn upload_shared(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: &[u8], pitch_px: u32);
    /// Drops the shared texture (the copying collector's reset).
    fn drop_shared(&mut self);
    /// A whole texture of its own for an image too big to shelf.
    fn make_dedicated(&mut self, w: u32, h: u32, bytes: &[u8], pitch_px: u32) -> Option<u64>;
    fn drop_dedicated(&mut self, id: u64);
}

// MARK: - The run atlas (text tiles, append-only shelves)

/// One rectangle of atlas texels.
#[derive(Clone, Copy)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
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
pub struct ShelfPacker {
    width: u32,
    height: u32,
    shelves: Vec<Shelf>,
    pub next_y: u32,
}

impl ShelfPacker {
    pub fn new(width: u32, height: u32) -> ShelfPacker {
        ShelfPacker { width, height, shelves: Vec::new(), next_y: 0 }
    }

    pub fn place(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
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

    pub fn reset(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }
}

/// The atlas is full — the caller drains the in-flight frames, resets
/// (growing once to the cap) and walks the frame again.
pub struct AtlasFull;

/// One cached run: the engine's raster uploaded as chunk tiles. The
/// color sits IN the key — the engine bakes it, which keeps emoji true
/// and byte parity possible; a theme flip mints new tiles and the old
/// ones fall with the next reset.
pub struct RunEntry {
    font: FontKey,
    color: u32,
    scale: u32,
    content: String,
    pub tiles: Vec<Tile>,
    pub width: u32,
    pub height: u32,
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

/// One cached image on the shared atlas: its chunk tiles at one
/// physical size.
pub struct ImageEntry {
    pub tiles: Vec<Tile>,
}

/// What `resolve_image` hands the frame walk: shared tiles, or one
/// whole dedicated texture.
pub enum ResolvedImage<'a> {
    Tiles(&'a ImageEntry),
    Dedicated(u64, u32, u32),
}

/// The shelf ceiling: taller goes dedicated (uniform shelf heights
/// pack well; one tall image would burn a whole shelf band)…
const DEDICATED_HEIGHT: u32 = 256;
/// …and so does anything larger than this area, atlas-budget-wise.
const DEDICATED_AREA: u32 = 512 * 512;
/// Dedicated textures retained before the reset collects them.
const DEDICATED_KEEP: usize = 8;

/// The text-and-image side of the GPU frame: the DATA of one shared
/// atlas, keyed by (font, color, scale, content) and by (source,
/// size). The pixels live wherever the ground put them.
///
/// The append-only INVARIANT: tiles are only ever written into virgin
/// space, so a frame still riding the GPU never sees its texels
/// change. The only operation that reuses space is `reset`, and reset
/// requires the caller to DRAIN in-flight frames first.
pub struct RunAtlas {
    pub size: u32,
    pub packer: ShelfPacker,
    pub entries: HashMap<u64, Vec<RunEntry>>,
    pub images: HashMap<(u64, u32, u32), ImageEntry>,
    pub dedicated: HashMap<(u64, u32, u32), (u64, u32, u32)>,
}

impl RunAtlas {
    pub fn new() -> RunAtlas {
        RunAtlas {
            size: ATLAS_INITIAL_SIZE,
            packer: ShelfPacker::new(ATLAS_INITIAL_SIZE, ATLAS_INITIAL_SIZE),
            entries: HashMap::new(),
            images: HashMap::new(),
            dedicated: HashMap::new(),
        }
    }

    /// Drops every entry and every shelf. `grow` doubles the texture
    /// once (2048 → 4096); the ground re-makes it lazily. The caller
    /// MUST have drained in-flight frames — this is the one moment
    /// texel space is reused.
    pub fn reset(&mut self, ground: &mut dyn AtlasGround, grow: bool) {
        if grow && self.size < ATLAS_MAX_SIZE {
            self.size = ATLAS_MAX_SIZE;
            ground.drop_shared();
            self.packer = ShelfPacker::new(self.size, self.size);
        } else {
            self.packer.reset();
        }
        self.entries.clear();
        self.images.clear();
        for (id, _, _) in self.dedicated.values() {
            ground.drop_dedicated(*id);
        }
        self.dedicated.clear();
    }

    /// The tiles for one run — warm from the map, or rasterized by the
    /// engine, chunked and uploaded. `Ok(None)` means the engine had
    /// nothing to paint (the CPU path skips those too).
    pub fn resolve(
        &mut self,
        ground: &mut dyn AtlasGround,
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
            if !ground.ensure_shared(self.size) {
                return Err(AtlasFull);
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
                ground.upload_shared(
                    x,
                    y,
                    chunk_width,
                    height,
                    &raster.rgba[chunk_x as usize * 4..],
                    raster.width as u32,
                );
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
    pub fn resolve_image(
        &mut self,
        ground: &mut dyn AtlasGround,
        source: &ImageSource,
        width: u32,
        height: u32,
        engine: &dyn ImageEngine,
    ) -> Result<Option<ResolvedImage<'_>>, AtlasFull> {
        let cache_key = (source.key(), width, height);
        if let Some(&(id, w, h)) = self.dedicated.get(&cache_key) {
            return Ok(Some(ResolvedImage::Dedicated(id, w, h)));
        }
        let shared = height <= DEDICATED_HEIGHT && width * height <= DEDICATED_AREA;
        if shared && !self.images.contains_key(&cache_key) {
            let Some(raster) = raster_source(engine, source, width as usize, height as usize)
            else {
                return Ok(None);
            };
            if !ground.ensure_shared(self.size) {
                return Err(AtlasFull);
            }
            let mut tiles = Vec::new();
            let mut chunk_x: u32 = 0;
            while chunk_x < width {
                let chunk_width = (width - chunk_x).min(ATLAS_CHUNK_WIDTH);
                let Some((x, y)) = self.packer.place(chunk_width, height) else {
                    return Err(AtlasFull);
                };
                ground.upload_shared(
                    x,
                    y,
                    chunk_width,
                    height,
                    &raster.rgba[chunk_x as usize * 4..],
                    raster.width as u32,
                );
                tiles.push(Tile { x, y, width: chunk_width, height });
                chunk_x += chunk_width;
            }
            self.images.insert(cache_key, ImageEntry { tiles });
        }
        if shared {
            return Ok(self.images.get(&cache_key).map(ResolvedImage::Tiles));
        }

        // dedicated: over the cap, the frame asks for the collector —
        // after the drain+reset the map is empty and the walk re-runs
        if self.dedicated.len() >= DEDICATED_KEEP {
            return Err(AtlasFull);
        }
        let Some(raster) = raster_source(engine, source, width as usize, height as usize) else {
            return Ok(None);
        };
        let Some(id) =
            ground.make_dedicated(width, height, &raster.rgba, raster.width as u32)
        else {
            return Err(AtlasFull);
        };
        let entry = self.dedicated.entry(cache_key).or_insert((id, width, height));
        Ok(Some(ResolvedImage::Dedicated(entry.0, entry.1, entry.2)))
    }

    /// The atlas footprint — cached runs + images + dedicated, and the
    /// shelf depth. The warm-frame tests pin upload reuse with it, and
    /// those tests live in the TIERS, one crate over — so this cannot
    /// hide behind `cfg(test)`. Nothing calls it in a shipped build,
    /// and the linker drops it there.
    pub fn footprint(&self) -> (usize, u32) {
        let entries: usize = self.entries.values().map(Vec::len).sum();
        (
            entries + self.images.len() + self.dedicated.len(),
            self.packer.next_y,
        )
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
pub enum RunKind {
    Rects,
    /// A batch of liquid-glass panes. It carries its own pass: the
    /// scene has to be blurred into the pyramid BEFORE the panes read
    /// it, and the pass boundary is what orders the two.
    Glass,
    Sprites,
    /// Sprites read from a DEDICATED texture (an image too big for the
    /// shared atlas) — the index points into the frame's texture list.
    Texture(u16),
}

#[derive(Clone, Copy)]
pub struct DrawRun {
    pub kind: RunKind,
    pub base: u32,
    pub count: u32,
    /// Index into the frame's interned curves — a `u32` compare keeps
    /// run coalescing cheap, and the run only breaks when the SHAPE of
    /// the cut changes.
    pub round: u32,
    /// Glass only: how deep the blur pyramid must go for this batch —
    /// the deepest blur any pane in it asked for.
    pub levels: u32,
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
/// pane already in it. One batch reads ONE capture of the scene, so two
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
pub struct FrameBatches {
    pub rects: Vec<RectInstance>,
    pub sprites: Vec<SpriteInstance>,
    pub glass: Vec<GlassInstance>,
    pub runs: Vec<DrawRun>,
    /// The frame's interned curves — slot 0 is always [`NO_ROUND`].
    pub rounds: Vec<RoundClip>,
    /// Dedicated texture handles this frame reads (borrowed from the
    /// atlas's cache — the ground owns and releases them).
    pub textures: Vec<u64>,
}

/// Walks the display list in paint order and fills the frame batches.
/// The clip stack mirrors `Surface::walk_clips`: snapped, intersected in
/// integers, an empty intersection degenerating to a zero-area box.
/// `Err(AtlasFull)` asks the caller to drain, reset the atlas and walk
/// again.
#[allow(clippy::too_many_arguments)]
pub fn build_frame(
    ground: &mut dyn AtlasGround,
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
                    crate::glass::levels_for(paint.blur) as u32,
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
                    crate::layout::GradientPaint::Radial {
                        center,
                        start,
                        end,
                        aspect,
                        inner,
                        outer,
                    } => {
                        // the circle keeps its kind (and its corner)
                        // byte for byte; the ellipse trades the corner
                        // slot for the aspect
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
                    crate::layout::GradientPaint::Linear { start, end, from, to } => {
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
                let Some(entry) = atlas.resolve(ground, slice, font, *color, scale, engine)?
                else {
                    continue;
                };
                // the composite_text mirror: one snap of the logical
                // origin, texels copied 1:1 from there
                let base_x = (origin.x * factor).round() as i64;
                let base_y = (origin.y * factor).round() as i64;
                let dest =
                    (base_x, base_y, base_x + entry.width as i64, base_y + entry.height as i64);
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
                    note_run(
                        &mut batches.runs,
                        RunKind::Sprites,
                        round_of(&clips),
                        batches.sprites.len() - 1,
                    );
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
                let dest = (base_x, base_y, base_x + width as i64, base_y + height as i64);
                if box_intersect(dest, clip).is_none() {
                    continue;
                }
                match atlas.resolve_image(ground, source, width, height, images)? {
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
                    Some(ResolvedImage::Dedicated(id, tex_w, tex_h)) => {
                        let index = match batches.textures.iter().position(|t| *t == id) {
                            Some(index) => index,
                            None => {
                                batches.textures.push(id);
                                batches.textures.len() - 1
                            }
                        };
                        batches.sprites.push(SpriteInstance {
                            dest: [dest.0 as f32, dest.1 as f32, dest.2 as f32, dest.3 as f32],
                            tex: [0.0, 0.0, tex_w as f32, tex_h as f32],
                            clip: [clip.0 as f32, clip.1 as f32, clip.2 as f32, clip.3 as f32],
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

// MARK: - Tests (the pure allocator and the wire)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_structs_hold_their_layout() {
        // the const asserts already gate the build; this pins the numbers
        // in a place a failing CI can point at
        assert_eq!(std::mem::size_of::<RectInstance>(), 80);
        assert_eq!(std::mem::align_of::<RectInstance>(), 4);
        assert_eq!(std::mem::size_of::<SpriteInstance>(), 48);
        assert_eq!(std::mem::size_of::<RoundClip>(), 32);
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
    fn overlapping_panes_break_the_batch_and_apart_ones_share_it() {
        // one batch reads ONE capture of the scene: two panes that
        // touch take two batches, two that never meet take one
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

    #[test]
    fn the_pane_instance_holds_its_layout() {
        // the const asserts already gate the build; this pins the
        // numbers in a place a person reads
        assert_eq!(std::mem::size_of::<GlassInstance>(), 112);
        assert_eq!(std::mem::offset_of!(GlassInstance, tint), 96);
    }
}
