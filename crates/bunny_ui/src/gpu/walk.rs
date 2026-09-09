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

use crate::image_engine::{ImageEngine, ImageRaster, ImageSource, raster_source};
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
#[derive(Clone, Copy, PartialEq, Debug)]
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

/// A shelf takes a tile down to two thirds of its own height, and a
/// new shelf opens a quarter taller than the tile that opens it. One
/// shelf per exact height was the Atrium floor's overflow: hundreds of
/// painted paths of nearly as many heights at scale 2 wanted 6209 rows
/// of the 4096 the atlas has, and the same tiles pack in a third of it.
/// The rows under a short tile are the price.
fn shelf_takes(shelf: u32, tile: u32) -> bool {
    shelf >= tile && shelf - tile <= tile / 2
}

/// Append-only shelf packing: a tile lands on the lowest shelf that
/// takes it with room, or opens a shelf below. There is no per-tile
/// free list — reclamation is the atlas RESET (drain, clear, re-insert
/// the live frame), a copying collector in one move.
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
        // the lowest taker, so a tall shelf keeps its rows for tall tiles
        let mut best: Option<usize> = None;
        for (index, shelf) in self.shelves.iter().enumerate() {
            if shelf_takes(shelf.height, height)
                && shelf.cursor + width <= self.width
                && best.is_none_or(|found| self.shelves[found].height > shelf.height)
            {
                best = Some(index);
            }
        }
        if let Some(index) = best {
            let shelf = &mut self.shelves[index];
            let x = shelf.cursor;
            shelf.cursor += width;
            return Some((x, shelf.y));
        }
        let room = self.height - self.next_y;
        if height <= room {
            let y = self.next_y;
            let tall = (height + height / 4).min(room);
            self.next_y += tall;
            self.shelves.push(Shelf { y, height: tall, cursor: width });
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
#[derive(Debug)]
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
/// Dedicated textures retained before a stale one asks the collector.
/// A frame that reads more keeps them all: the collector can only take
/// a texture no walk needs.
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
    /// Images too big for a shelf get a texture of their own. Capped;
    /// overflow rides the same reset the atlas already does — but only
    /// while the map holds a texture the walk in progress did not read.
    /// After a reset the map holds exactly what the walk minted, and a
    /// frame that needs more than the cap keeps every one: asking the
    /// collector again would be a livelock.
    pub dedicated: HashMap<(u64, u32, u32), Dedicated>,
    /// The walk in progress: `build_frame` opens one per attempt, and
    /// every dedicated read stamps its texture with it.
    pub walk: u64,
    /// The walk the last reset closed — `fresh` reads the pair.
    pub reset_walk: u64,
}

/// One dedicated texture — the ground's handle, its size — and the
/// last walk that read it: the question the collector is asked before
/// it is called.
pub struct Dedicated {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub walk: u64,
}

impl RunAtlas {
    pub fn new() -> RunAtlas {
        RunAtlas {
            size: ATLAS_INITIAL_SIZE,
            packer: ShelfPacker::new(ATLAS_INITIAL_SIZE, ATLAS_INITIAL_SIZE),
            entries: HashMap::new(),
            images: HashMap::new(),
            dedicated: HashMap::new(),
            walk: 0,
            reset_walk: 0,
        }
    }

    /// Opens a walk: the stamp every dedicated texture the frame reads
    /// takes. `build_frame` calls it first, on every attempt.
    pub fn begin_walk(&mut self) {
        self.walk = self.walk.wrapping_add(1);
    }

    /// True while a reset would give nothing back: the first walk after
    /// one, on the grown texture, holds only its own tiles. A shelf that
    /// refuses a tile then is the frame's own size, not garbage.
    fn fresh(&self) -> bool {
        self.size == ATLAS_MAX_SIZE && self.walk == self.reset_walk.wrapping_add(1)
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
        for entry in self.dedicated.values() {
            ground.drop_dedicated(entry.id);
        }
        self.dedicated.clear();
        self.reset_walk = self.walk;
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
        let walk = self.walk;
        if let Some(entry) = self.dedicated.get_mut(&cache_key) {
            entry.walk = walk;
            return Ok(Some(ResolvedImage::Dedicated(entry.id, entry.width, entry.height)));
        }
        let shelf_size = height <= DEDICATED_HEIGHT && width * height <= DEDICATED_AREA;
        if shelf_size && self.images.contains_key(&cache_key) {
            return Ok(self.images.get(&cache_key).map(ResolvedImage::Tiles));
        }
        let Some(raster) = raster_source(engine, source, width as usize, height as usize) else {
            return Ok(None);
        };
        if shelf_size {
            match self.shelve(ground, &raster, width, height) {
                Ok(tiles) => {
                    self.images.insert(cache_key, ImageEntry { tiles });
                    return Ok(self.images.get(&cache_key).map(ResolvedImage::Tiles));
                }
                // the shelves hold nothing but this walk's own tiles: a
                // reset would give nothing back, so the image takes a
                // texture of its own instead of failing the frame
                Err(AtlasFull) if self.fresh() => {}
                Err(full) => return Err(full),
            }
        }

        // dedicated: over the cap, the frame asks for the collector —
        // but only when the collector has something to take. A texture
        // this walk read is not garbage; after the drain+reset the map
        // holds nothing else, and the walk that re-runs keeps what it
        // needs instead of running into the same wall
        if self.dedicated.len() >= DEDICATED_KEEP
            && self.dedicated.values().any(|entry| entry.walk != walk)
        {
            return Err(AtlasFull);
        }
        let Some(id) = ground.make_dedicated(width, height, &raster.rgba, raster.width as u32)
        else {
            return Err(AtlasFull);
        };
        self.dedicated.insert(cache_key, Dedicated { id, width, height, walk });
        Ok(Some(ResolvedImage::Dedicated(id, width, height)))
    }

    /// Cuts one raster into chunk tiles on the shared shelves and hands
    /// them to the ground. `Err` = a chunk found no shelf; the chunks
    /// already cut stay where they lie and fall with the next reset.
    fn shelve(
        &mut self,
        ground: &mut dyn AtlasGround,
        raster: &ImageRaster,
        width: u32,
        height: u32,
    ) -> Result<Vec<Tile>, AtlasFull> {
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
        Ok(tiles)
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
#[derive(Clone, Copy, PartialEq, Debug)]
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
    atlas.begin_walk();
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

// MARK: - The ground a host can watch

/// An [`AtlasGround`] that allocates nothing and remembers everything.
/// A tier's ground talks to a device; this one talks to a `Vec`, so the
/// walk runs on any machine — including the two whose tiers no desk
/// here can execute.
#[derive(Default)]
pub struct RecordingGround {
    /// The shared texture's side, once it exists.
    pub shared: Option<u32>,
    /// Every tile handed over: x, y, w, h, and the pitch it was cut from.
    pub uploads: Vec<(u32, u32, u32, u32, u32)>,
    /// Dedicated textures minted, by handle and size.
    pub dedicated: Vec<(u64, u32, u32)>,
    /// How many times the copying collector wiped the shared texture.
    pub drops: usize,
    next: u64,
}

impl AtlasGround for RecordingGround {
    fn ensure_shared(&mut self, size: u32) -> bool {
        self.shared = Some(size);
        true
    }

    fn upload_shared(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: &[u8], pitch_px: u32) {
        // the rows after the first are a pitch apart, and the last one
        // is only `w` wide — what the slice must hold, exactly
        let needed = (h as usize - 1) * pitch_px as usize * 4 + w as usize * 4;
        assert!(
            bytes.len() >= needed,
            "a tile of {w}x{h} at pitch {pitch_px} wants {needed} bytes, got {}",
            bytes.len()
        );
        self.uploads.push((x, y, w, h, pitch_px));
    }

    fn drop_shared(&mut self) {
        self.shared = None;
        self.drops += 1;
    }

    fn make_dedicated(&mut self, w: u32, h: u32, bytes: &[u8], pitch_px: u32) -> Option<u64> {
        assert!(bytes.len() >= (h as usize - 1) * pitch_px as usize * 4 + w as usize * 4);
        self.next += 1;
        self.dedicated.push((self.next, w, h));
        Some(self.next)
    }

    fn drop_dedicated(&mut self, id: u64) {
        self.dedicated.retain(|(held, _, _)| *held != id);
    }
}

impl FrameBatches {
    /// One number over the instance bytes, for a tier to compare against
    /// another machine's. Same walk, same number — so a scene that
    /// disagrees on pixels but agrees here has a bug BELOW the walk, and
    /// that halves the search.
    pub fn checksum(&self) -> u64 {
        fn feed(hash: &mut u64, bytes: &[u8]) {
            for byte in bytes {
                *hash ^= *byte as u64;
                *hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for rect in &self.rects {
            feed(&mut hash, &rect.rect.map(f32::to_le_bytes).concat());
            feed(&mut hash, &rect.clip.map(f32::to_le_bytes).concat());
            feed(&mut hash, &rect.params.map(f32::to_le_bytes).concat());
            feed(&mut hash, &rect.color);
            feed(&mut hash, &rect.pad);
            feed(&mut hash, &rect.radii.map(f32::to_le_bytes).concat());
        }
        for sprite in &self.sprites {
            feed(&mut hash, &sprite.dest.map(f32::to_le_bytes).concat());
            feed(&mut hash, &sprite.tex.map(f32::to_le_bytes).concat());
            feed(&mut hash, &sprite.clip.map(f32::to_le_bytes).concat());
        }
        for round in &self.rounds {
            feed(&mut hash, &round.box4.map(f32::to_le_bytes).concat());
            feed(&mut hash, &round.radii.map(f32::to_le_bytes).concat());
        }
        for run in &self.runs {
            feed(&mut hash, &run.base.to_le_bytes());
            feed(&mut hash, &run.count.to_le_bytes());
            feed(&mut hash, &run.round.to_le_bytes());
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_engine::RawImages;

    /// The scene the golden walks: one of every shape the tiers answer,
    /// in paint order, with a rounded clip around the middle of it.
    fn scene() -> DisplayList {
        let mut display = DisplayList::default();
        let box_at = |x: f64, y: f64, w: f64, h: f64| Rect {
            origin: crate::layout::Point { x, y },
            size: crate::layout::Size { width: w, height: h },
        };
        display.push(DrawCommand::FillRect {
            rect: box_at(0.0, 0.0, 100.0, 60.0),
            color: Color::rgba(20, 30, 40, 255),
            corner_radius: Corners::ZERO,
        });
        display.push(DrawCommand::Shadow {
            rect: box_at(10.0, 10.0, 40.0, 20.0),
            radius: 6.0,
            color: Color::rgba(0, 0, 0, 80),
            corner_radius: Corners::all(4.0),
        });
        display.push(DrawCommand::PushClip {
            rect: box_at(8.0, 8.0, 60.0, 40.0),
            corner_radius: Corners::all(5.0),
        });
        display.push(DrawCommand::FillRect {
            rect: box_at(12.0, 12.0, 30.0, 16.0),
            color: Color::rgba(200, 40, 40, 255),
            corner_radius: Corners::all(3.0),
        });
        display.push(DrawCommand::StrokeRect {
            rect: box_at(12.0, 12.0, 30.0, 16.0),
            color: Color::rgba(255, 255, 255, 128),
            width: 2.0,
            corner_radius: Corners::all(3.0),
        });
        display.push(DrawCommand::TextLine {
            origin: crate::layout::Point { x: 14.0, y: 32.0 },
            content: std::sync::Arc::from("hi"),
            range: (0, 2),
            color: Color::rgba(240, 240, 240, 255),
            font: FontSpec::DEFAULT,
        });
        display.push(DrawCommand::PopClip);
        display
    }

    #[test]
    fn the_walk_says_the_same_bytes_to_every_tier() {
        let mut ground = RecordingGround::default();
        let mut atlas = RunAtlas::new();
        let mut batches = FrameBatches::default();
        build_frame(
            &mut ground,
            &scene(),
            2,
            (200, 120),
            &crate::text_engine::PixelFont,
            &crate::image_engine::RawImages::default(),
            &mut atlas,
            &mut batches,
        )
        .expect("the scene fits the atlas");

        // three rect kinds and no more: the fill, the halo, the ring,
        // and the fill inside the cut
        assert_eq!(batches.rects.len(), 4, "one instance per rect command");
        assert!(!batches.sprites.is_empty(), "the label rode the atlas");
        assert!(batches.glass.is_empty(), "no pane in this scene");

        // slot 0 is the straight cut, and the rounded clip interned once
        assert_eq!(batches.rounds.len(), 2, "NO_ROUND, then the one curve");
        assert_eq!(batches.rounds[0], NO_ROUND);
        assert_eq!(batches.rounds[1].radii, [10.0, 10.0, 10.0, 10.0], "5 points at scale 2");

        // the runs break where the KIND changes and where the CUT does,
        // never in between
        let shape: Vec<(RunKind, u32, u32)> =
            batches.runs.iter().map(|run| (run.kind, run.count, run.round)).collect();
        assert_eq!(
            shape,
            vec![
                (RunKind::Rects, 2, 0),
                (RunKind::Rects, 2, 1),
                (RunKind::Sprites, batches.sprites.len() as u32, 1),
            ],
            "the fill and halo outside the cut, then the pair inside it, then the label"
        );

        // the tiles never overlap and never leave the atlas
        for (x, y, w, h, _) in &ground.uploads {
            assert!(x + w <= ATLAS_INITIAL_SIZE && y + h <= ATLAS_INITIAL_SIZE);
        }

        // the number a tier on another machine can compare against
        let first = batches.checksum();
        let mut again = FrameBatches::default();
        let mut ground2 = RecordingGround::default();
        let mut atlas2 = RunAtlas::new();
        build_frame(
            &mut ground2,
            &scene(),
            2,
            (200, 120),
            &crate::text_engine::PixelFont,
            &crate::image_engine::RawImages::default(),
            &mut atlas2,
            &mut again,
        )
        .expect("the scene fits the atlas");
        assert_eq!(first, again.checksum(), "the same scene is the same bytes");
    }

    #[test]
    fn a_warm_frame_uploads_nothing_new() {
        let mut ground = RecordingGround::default();
        let mut atlas = RunAtlas::new();
        let mut batches = FrameBatches::default();
        let walk = |ground: &mut RecordingGround, atlas: &mut RunAtlas, batches: &mut FrameBatches| {
            build_frame(
                ground,
                &scene(),
                2,
                (200, 120),
                &crate::text_engine::PixelFont,
                &crate::image_engine::RawImages::default(),
                atlas,
                batches,
            )
            .expect("the scene fits the atlas");
        };
        walk(&mut ground, &mut atlas, &mut batches);
        let cold = ground.uploads.len();
        assert!(cold > 0, "the first frame cut the tiles");
        walk(&mut ground, &mut atlas, &mut batches);
        assert_eq!(ground.uploads.len(), cold, "a warm frame re-cuts nothing");
    }

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
        // the pure allocator: a shelf opens a quarter taller than its
        // first tile, takes tiles down to two thirds of its height, the
        // lowest taker wins, refusal at the brim, a clean slate after reset
        let mut packer = ShelfPacker::new(64, 64);
        assert_eq!(packer.place(20, 12), Some((0, 0)), "the first shelf: 15 rows for a 12");
        assert_eq!(packer.place(20, 10), Some((20, 0)), "a 10 rides the 15 shelf");
        assert_eq!(packer.place(20, 9), Some((0, 15)), "a 9 is under two thirds of 15: a new shelf");
        assert_eq!(packer.place(20, 15), Some((40, 0)), "exact height still exact");
        assert_eq!(packer.place(20, 11), Some((20, 15)), "an 11 rides the 11 shelf");
        assert_eq!(packer.place(20, 12), Some((0, 26)), "no room left on the 15 shelf: a new one");
        assert_eq!(packer.place(20, 10), Some((40, 15)), "the lowest taker wins: the 11, not the 15");
        assert_eq!(packer.place(65, 1), None, "wider than the atlas never fits");
        packer.reset();
        assert_eq!(packer.place(64, 64), Some((0, 0)), "reset reclaims everything; headroom stops at the brim");
        assert_eq!(packer.place(1, 1), None, "the atlas is full below");
    }

    #[test]
    fn a_hundred_heights_share_a_dozen_shelves() {
        // the Atrium floor at scale 2 cuts hundreds of tiles of nearly as
        // many heights — and ascending is the order a shelf likes least,
        // every tile taller than every shelf so far
        let mut packer = ShelfPacker::new(ATLAS_MAX_SIZE, ATLAS_MAX_SIZE);
        let mut shelves = 0;
        for height in (20..=218).step_by(2) {
            let before = packer.next_y;
            assert!(packer.place(40, height).is_some(), "a {height}-row tile found no shelf");
            shelves += usize::from(packer.next_y != before);
        }
        assert!(
            shelves <= 12 && packer.next_y <= 1200,
            "{shelves} shelves over {} rows for a hundred heights",
            packer.next_y
        );
    }

    /// A 32×32 gradient in the house raw format, under `key`.
    fn photo(key: u64) -> ImageSource {
        let mut rgba = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32u32 {
            for x in 0..32u32 {
                rgba.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, 128, 255]);
            }
        }
        ImageSource::bytes_keyed(key, RawImages::encode(32, 32, &rgba))
    }

    /// `count` photos of one logical size, keys from `first_key`, all at
    /// the origin — the atlas sees every one, the target need not.
    fn photos(count: u64, first_key: u64, size: (f64, f64)) -> DisplayList {
        let mut display = DisplayList::default();
        for key in first_key..first_key + count {
            display.push(DrawCommand::Image {
                rect: crate::layout::Rect {
                    origin: crate::layout::Point { x: 0.0, y: 0.0 },
                    size: crate::layout::Size { width: size.0, height: size.1 },
                },
                source: photo(key),
            });
        }
        display
    }

    /// The tiers' retry loop: walk; on overflow reset (growing once) and
    /// walk again, twice. `false` = the frame never walked whole.
    fn present(ground: &mut RecordingGround, atlas: &mut RunAtlas, display: &DisplayList) -> bool {
        let mut batches = FrameBatches::default();
        for attempt in 0..3 {
            let walked = build_frame(
                ground,
                display,
                2,
                (1000, 250),
                &crate::text_engine::PixelFont,
                &RawImages::default(),
                atlas,
                &mut batches,
            );
            match walked {
                Ok(()) => return true,
                Err(AtlasFull) if attempt < 2 => atlas.reset(ground, true),
                Err(AtlasFull) => return false,
            }
        }
        false
    }

    #[test]
    fn a_frame_of_more_big_images_than_the_cap_keeps_every_one() {
        // twelve 40×130pt photos: 80×260 physical, each taller than a
        // shelf, more of them than the cap. The cap is a retention budget
        // between frames, not a limit on a frame
        let mut ground = RecordingGround::default();
        let mut atlas = RunAtlas::new();
        let crowd = photos(12, 10, (40.0, 130.0));
        assert!(present(&mut ground, &mut atlas, &crowd), "the frame walked whole");
        assert_eq!(atlas.dedicated.len(), 12, "every photo keeps its texture");
        let minted = ground.dedicated.len();
        assert!(present(&mut ground, &mut atlas, &crowd));
        assert_eq!(ground.dedicated.len(), minted, "a warm frame past the cap mints nothing");
        // twelve OTHER photos: the first twelve are stale, the collector runs
        assert!(present(&mut ground, &mut atlas, &photos(12, 50, (40.0, 130.0))));
        assert_eq!(atlas.dedicated.len(), 12, "the collector took the stale textures");
        assert_eq!(ground.dedicated.len(), 12, "and the ground dropped them");
    }

    #[test]
    fn a_full_shelf_on_a_fresh_walk_sends_the_image_to_its_own_texture() {
        // seventy 500×125pt photos: 1000×250 physical, shelf-sized every
        // one, and more of them than 4096 rows of shelves hold
        let mut ground = RecordingGround::default();
        let mut atlas = RunAtlas::new();
        assert!(
            present(&mut ground, &mut atlas, &photos(70, 100, (500.0, 125.0))),
            "the frame walked whole"
        );
        assert_eq!(atlas.size, ATLAS_MAX_SIZE, "the atlas grew once");
        assert_eq!(atlas.images.len() + atlas.dedicated.len(), 70, "every photo is somewhere");
        assert!(!atlas.dedicated.is_empty(), "the overflow took textures of its own");
        assert!(atlas.images.len() >= 40, "the shelves took their share: {}", atlas.images.len());
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
