//! The browser's text engine — `measureText` and `fillText` through the
//! hand-written FFI border.
//!
//! The same [`TextEngine`] border as every other shell: layout stays
//! ours; the platform lends measurement and the drawing of glyphs. Here
//! the platform is a hidden 2D canvas owned by `glue.js` —
//! `js_measure_text` reads the FONT's bounding box (stable per font: the
//! line height must not jump per string) and `js_raster_text` draws one
//! line and hands the pixels back.
//!
//! `getImageData` returns STRAIGHT alpha by specification — exactly what
//! the house compositor blends — so no unpremultiply pass runs here. The
//! known cost: the canvas stores premultiplied internally, so chroma
//! under low alpha loses a little precision on the way out. Byte parity
//! stays with the pixel font; this engine promises legibility.
//!
//! Each raster crosses the border (one `fillText`, one `getImageData`),
//! so the engine RETAINS finished rasters by content: a scroll repaints
//! the same strings every frame and must not pay the crossing for each.

use std::cell::RefCell;
use std::collections::HashMap;

use bunny_ui::layout::Color;
use bunny_ui::text_engine::{
    FontDesign, FontKey, FontSpec, LineMetrics, TextEngine, TextRaster, Weight,
};

#[link(wasm_import_module = "bunny")]
unsafe extern "C" {
    /// Writes THREE f64 at `out` — width, ascent, descent — in logical
    /// px. Ascent and descent come from the font, not the string's ink.
    #[allow(clippy::too_many_arguments)]
    fn js_measure_text(
        ptr: *const u8,
        len: usize,
        size: f64,
        weight: u32,
        mono: u32,
        italic: u32,
        family_ptr: *const u8,
        family_len: usize,
        out: *mut f64,
    );
    /// Draws one line into a `width × height` physical rectangle and
    /// copies the straight-alpha RGBA into `out`. The glue places the
    /// baseline at `height/scale − descent` in logical coordinates —
    /// the ceil slack stays on top, like the desktop engine.
    #[allow(clippy::too_many_arguments)]
    fn js_raster_text(
        ptr: *const u8,
        len: usize,
        size: f64,
        weight: u32,
        mono: u32,
        italic: u32,
        family_ptr: *const u8,
        family_len: usize,
        scale: f64,
        width: u32,
        height: u32,
        descent: f64,
        color: u32,
        out: *mut u8,
    );
}

/// Does this font lean? One bit across the border, like the mono flag.
fn italic_flag(font: &FontSpec) -> u32 {
    matches!(font.slant, bunny_ui::text_engine::Slant::Italic) as u32
}

/// The numeric weights `ctx.font` understands.
fn css_weight(weight: Weight) -> u32 {
    match weight {
        Weight::Regular => 400,
        Weight::Medium => 500,
        Weight::Semibold => 600,
        Weight::Bold => 700,
    }
}

fn mono_flag(font: &FontSpec) -> u32 {
    match font.design {
        FontDesign::Default => 0,
        FontDesign::Mono => 1,
    }
}

/// `0xRRGGBBAA` — one u32 crosses instead of four channels.
fn pack_color(color: Color) -> u32 {
    (color.r as u32) << 24 | (color.g as u32) << 16 | (color.b as u32) << 8 | color.a as u32
}

/// The identity of a finished raster: the color and scale are baked
/// into the pixels, so they live in the key.
#[derive(PartialEq, Eq, Hash)]
struct RasterKey {
    text: String,
    font: FontKey,
    color: u32,
    scale: usize,
}

/// When the retained rasters reach this count the whole map clears —
/// a hard bound instead of an aging pass. The visible screen refills
/// it within one frame; steady scrolling through distinct rows clears
/// every few hundred, which costs one frame of re-crossing.
const RASTER_KEEP: usize = 512;

fn clone_raster(raster: &TextRaster) -> TextRaster {
    TextRaster {
        width: raster.width,
        height: raster.height,
        baseline: raster.baseline,
        rgba: raster.rgba.clone(),
    }
}

/// The web text engine. Single-thread, like the rest of the shell.
pub struct CanvasTextEngine {
    rasters: RefCell<HashMap<RasterKey, TextRaster>>,
}

impl CanvasTextEngine {
    pub fn new() -> Self {
        CanvasTextEngine { rasters: RefCell::new(HashMap::new()) }
    }
}

impl Default for CanvasTextEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// The CSS generic families a browser guarantees it can shape. The
/// page cannot READ the machine's font list — `queryLocalFonts` is
/// Chromium's alone, asks the reader for permission and answers late —
/// so this roster is what the engine can honestly promise, and a page
/// that ships faces of its own already knows their names.
const WEB_FAMILIES: [&str; 6] =
    ["cursive", "fantasy", "monospace", "sans-serif", "serif", "system-ui"];

/// The family's name for the border, or an empty slice for the
/// system's own face — a null crossing costs nothing, and the face
/// nobody named is the common one.
fn family_name(font: &FontSpec) -> Option<std::sync::Arc<str>> {
    font.family.name()
}

impl TextEngine for CanvasTextEngine {
    fn families(&self) -> Vec<std::sync::Arc<str>> {
        WEB_FAMILIES.iter().map(|name| std::sync::Arc::from(*name)).collect()
    }

    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let mut out = [0.0f64; 3];
        let family = family_name(font);
        let family = family.as_deref().unwrap_or("");
        unsafe {
            js_measure_text(
                text.as_ptr(),
                text.len(),
                font.size,
                css_weight(font.weight),
                mono_flag(font),
                italic_flag(font),
                family.as_ptr(),
                family.len(),
                out.as_mut_ptr(),
            );
        }
        LineMetrics { width: out[0], ascent: out[1], descent: out[2] }
    }

    fn raster_line(
        &self,
        text: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        if text.is_empty() {
            return None;
        }
        let key = RasterKey {
            text: text.to_owned(),
            font: font.key(),
            color: pack_color(color),
            scale,
        };
        if let Some(hit) = self.rasters.borrow().get(&key) {
            return Some(clone_raster(hit));
        }

        let metrics = self.measure_line(text, font);
        let width = (metrics.width * scale as f64).ceil() as usize;
        let height = (metrics.height() * scale as f64).ceil() as usize;
        if width == 0 || height == 0 {
            return None;
        }
        let mut rgba = vec![0u8; width * height * 4];
        let family = family_name(font);
        let family = family.as_deref().unwrap_or("");
        unsafe {
            js_raster_text(
                text.as_ptr(),
                text.len(),
                font.size,
                css_weight(font.weight),
                mono_flag(font),
                italic_flag(font),
                family.as_ptr(),
                family.len(),
                scale as f64,
                width as u32,
                height as u32,
                metrics.descent,
                pack_color(color),
                rgba.as_mut_ptr(),
            );
        }
        let raster = TextRaster {
            width,
            height,
            baseline: (metrics.ascent * scale as f64).round() as usize,
            rgba,
        };

        let mut rasters = self.rasters.borrow_mut();
        if rasters.len() >= RASTER_KEEP {
            rasters.clear();
        }
        rasters.insert(key, clone_raster(&raster));
        Some(raster)
    }
}
