//! The pluggable text boundary — measurement and raster of ONE line.
//!
//! Layout is always ours, on every target; what the platform lends is
//! the MEASUREMENT and the drawing of glyphs: the house pixel font in
//! headless, CoreText on the Mac, `measureText`/DOM on the web one day.
//! No component API knows which engine is active — [`TextEngine`] is the
//! only door (a declared boundary: `Rc<dyn TextEngine>` in the `Runtime`).
//!
//! The [`MeasureCache`] ages per pass: a hit rejuvenates the entry, and
//! whatever sits [`CACHE_KEEP_FRAMES`] passes without use falls out —
//! typing ALTERNATES content (backspace restores, a filter hides and
//! reveals), and shaping does not re-pay itself for one frame of absence.
//! Note for the real wrap system (shape separate from breaking, 2-level
//! cache): the key GAINS the probing mode — a line cache poisoned by a
//! proposal is a classic bug.

use std::cell::RefCell;
use std::sync::Arc;
use motor::hash::FxHashMap as HashMap;

use crate::layout::{Color, Px};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

/// Upright, or leaning. The preview tab of an editor writes its label
/// in italic — the VS Code idiom for "you are only looking" — and that
/// is content, not decoration: the reader must see the lean.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slant {
    Upright,
    Italic,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FontDesign {
    /// The system interface font.
    Default,
    /// Monospaced — a first-class citizen (code grids).
    Mono,
}

/// A font family the app named. The NUMBER is what travels — the
/// layout carries it, the caches key on it, and a shell that speaks
/// strings asks the table for the name once, the same way an image
/// identity travels as a number and the shell keeps the registry.
/// Zero is the system's own face, which is what a scene that never
/// names a family keeps.
///
/// A name the engine cannot shape is not an error here: the table
/// holds it, the engine falls back to the system face, and the app
/// sees the same text in a face it did not ask for — which is what
/// every platform does with a missing family.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Family(u16);

thread_local! {
    /// The named families, in the order they were first named. Slot
    /// zero is the system's and carries no name.
    static FAMILY_NAMES: RefCell<Vec<Arc<str>>> = RefCell::new(vec![Arc::from("")]);
    static FAMILY_IDS: RefCell<HashMap<Arc<str>, u16>> = RefCell::new(HashMap::default());
}

impl Family {
    /// The system's own face — where every scene starts.
    pub const SYSTEM: Family = Family(0);

    /// The family under this name. The same name always gives the same
    /// number: the table only grows, and it grows once per name in the
    /// life of the process.
    pub fn named(name: &str) -> Family {
        if name.is_empty() {
            return Family::SYSTEM;
        }
        if let Some(id) = FAMILY_IDS.with(|ids| ids.borrow().get(name).copied()) {
            return Family(id);
        }
        FAMILY_NAMES.with(|names| {
            let mut names = names.borrow_mut();
            // a table this deep is a leak, not a design: the scene
            // keeps the system face rather than growing without end
            let Ok(id) = u16::try_from(names.len()) else {
                return Family::SYSTEM;
            };
            let name: Arc<str> = Arc::from(name);
            names.push(name.clone());
            FAMILY_IDS.with(|ids| ids.borrow_mut().insert(name, id));
            Family(id)
        })
    }

    /// The name the app gave, or `None` for the system's own face.
    pub fn name(self) -> Option<Arc<str>> {
        match self.0 {
            0 => None,
            id => FAMILY_NAMES.with(|names| names.borrow().get(id as usize).cloned()),
        }
    }
}

/// A resolved font — what the layout carries and the engine consumes.
/// `size` is fractional by contract (10.5px is a real dense-UI case).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FontSpec {
    pub size: Px,
    pub weight: Weight,
    pub design: FontDesign,
    pub slant: Slant,
    /// The family the app named, or the system's own.
    pub family: Family,
}

impl FontSpec {
    pub const DEFAULT: FontSpec = FontSpec {
        size: 13.0,
        weight: Weight::Regular,
        design: FontDesign::Default,
        slant: Slant::Upright,
        family: Family::SYSTEM,
    };

    /// The API text styles, in desktop metrics.
    pub fn resolve(font: motor::views::Font) -> FontSpec {
        use motor::views::Font;
        let (size, weight) = match font {
            Font::LargeTitle => (26.0, Weight::Regular),
            Font::Title => (22.0, Weight::Regular),
            Font::Headline => (13.0, Weight::Semibold),
            Font::Subheadline => (11.0, Weight::Regular),
            Font::Body => (13.0, Weight::Regular),
            Font::Callout => (12.0, Weight::Regular),
            Font::Footnote => (10.0, Weight::Regular),
            Font::Caption => (10.0, Weight::Regular),
            Font::Caption2 => (10.0, Weight::Regular),
        };
        FontSpec {
            size,
            weight,
            design: FontDesign::Default,
            slant: Slant::Upright,
            family: Family::SYSTEM,
        }
    }

    /// The same font in a named family — the door a live preview asks
    /// for, and the one a settings page writes through.
    pub fn family(self, name: &str) -> FontSpec {
        FontSpec { family: Family::named(name), ..self }
    }

    /// The hashable key (f64 is not `Eq`): size quantized in thousandths
    /// of a point — no real use distinguishes less than that.
    pub fn key(&self) -> FontKey {
        FontKey {
            size_milli: (self.size * 1000.0).round() as u32,
            weight: self.weight,
            design: self.design,
            family: self.family,
            slant: self.slant,
        }
    }
}

/// The cache/font identity of a [`FontSpec`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontKey {
    size_milli: u32,
    weight: Weight,
    design: FontDesign,
    /// In the KEY as well: two families are two rasters.
    family: Family,
    /// In the KEY as well: an upright and a leaning line are two
    /// rasters, and one cache entry must never answer for the other.
    slant: Slant,
}

/// A partial font patch for inheritance: `.font(…)` sets all three
/// fields; `.bold()` only the weight; `.monospaced()` only the design —
/// each one applies on top of the inherited spec, field by field.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct FontPatch {
    pub size: Option<Px>,
    pub weight: Option<Weight>,
    pub design: Option<FontDesign>,
    pub slant: Option<Slant>,
    pub family: Option<Family>,
}

impl FontPatch {
    /// Merge of the stacked modifiers — the defined (closest) one wins.
    /// A slot nobody named stays empty all the way to the env, which is
    /// what makes the chain order-free: a modifier can only undo what
    /// it actually speaks about.
    pub fn or(self, outer: FontPatch) -> FontPatch {
        FontPatch {
            size: self.size.or(outer.size),
            weight: self.weight.or(outer.weight),
            design: self.design.or(outer.design),
            slant: self.slant.or(outer.slant),
            family: self.family.or(outer.family),
        }
    }

    pub fn apply_over(&self, base: FontSpec) -> FontSpec {
        FontSpec {
            size: self.size.unwrap_or(base.size),
            weight: self.weight.unwrap_or(base.weight),
            design: self.design.unwrap_or(base.design),
            slant: self.slant.unwrap_or(base.slant),
            family: self.family.unwrap_or(base.family),
        }
    }
}

/// Metrics of ONE line. The line height is DERIVED — `ascent +
/// descent`; the engine folds the leading into the descent (one sum, one
/// contract).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LineMetrics {
    pub width: Px,
    pub ascent: Px,
    pub descent: Px,
}

impl LineMetrics {
    pub fn height(&self) -> Px {
        self.ascent + self.descent
    }
}

/// A rasterized line: an RGBA rectangle of STRAIGHT alpha (not pre-
/// multiplied — the house compositor blends straight, on every target),
/// origin at the top-left of the line box, already in PHYSICAL pixels.
/// `baseline` is the baseline measured from the top (informative/tests —
/// compositing uses the top-left directly).
pub struct TextRaster {
    pub width: usize,
    pub height: usize,
    pub baseline: usize,
    pub rgba: Vec<u8>,
}

/// The boundary: who measures and draws text. Object-safe on purpose —
/// `Rc<dyn TextEngine>` is the shape that crosses the `Runtime`.
pub trait TextEngine {
    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics;

    /// The families this engine can shape, for an app that offers the
    /// choice. Sorted, and without the system's own face — that one
    /// has no name and is already the default. An engine with a single
    /// built-in face answers nothing, which is the honest answer.
    fn families(&self) -> Vec<Arc<str>> {
        Vec::new()
    }

    /// `None` = nothing to paint (empty string, zero width). `scale` is
    /// the retina factor — the raster comes out in physical pixels.
    fn raster_line(
        &self,
        text: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster>;
}

// MARK: - PixelFont, the default engine

/// The house font: 3×5 pixels per glyph, FIXED 8×16 cell (ignores
/// size/weight/design on purpose) — deterministic metrics that keep the
/// headless tests byte-stable. Uppercase falls into lowercase; what does
/// not exist does not paint (the empty box is honest).
pub struct PixelFont;

/// The pixel font cell: baseline at the glyph's foot (3 of slack on top
/// + 10 of body), 13 above + 3 below = the cell's 16.
const PIXEL_ASCENT: Px = 13.0;
const PIXEL_DESCENT: Px = 3.0;
const PIXEL_ADVANCE: Px = 8.0;

impl TextEngine for PixelFont {
    fn measure_line(&self, text: &str, _font: &FontSpec) -> LineMetrics {
        LineMetrics {
            width: text.chars().count() as Px * PIXEL_ADVANCE,
            ascent: PIXEL_ASCENT,
            descent: PIXEL_DESCENT,
        }
    }

    fn raster_line(
        &self,
        text: &str,
        _font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let width = chars.len() * 8 * scale;
        let height = 16 * scale;
        let mut rgba = vec![0u8; width * height * 4];
        let mut set = |x: usize, y: usize| {
            let index = (y * width + x) * 4;
            rgba[index] = color.r;
            rgba[index + 1] = color.g;
            rgba[index + 2] = color.b;
            rgba[index + 3] = color.a;
        };

        // the SAME offsets as the original rasterizer: glyph ×(2·scale)
        // with a logical (1, 3) slack in the 8×16 cell
        let block = 2 * scale;
        for (index, ch) in chars.iter().enumerate() {
            let Some(rows) = glyph(*ch) else { continue };
            let cell_x = (index * 8 + 1) * scale;
            let cell_y = 3 * scale;
            for row in 0..5usize {
                for col in 0..3usize {
                    let bit = 14 - (row * 3 + col);
                    if rows >> bit & 1 == 1 {
                        for dy in 0..block {
                            for dx in 0..block {
                                set(cell_x + col * block + dx, cell_y + row * block + dy);
                            }
                        }
                    }
                }
            }
        }

        Some(TextRaster { width, height, baseline: 13 * scale, rgba })
    }
}

/// The house font: 15 bits per glyph (3 columns × 5 rows, MSB = top-left
/// corner).
fn glyph(ch: char) -> Option<u16> {
    let rows = match ch.to_ascii_lowercase() {
        '0' => 0b111_101_101_101_111,
        '1' => 0b010_110_010_010_111,
        '2' => 0b111_001_111_100_111,
        '3' => 0b111_001_111_001_111,
        '4' => 0b101_101_111_001_001,
        '5' => 0b111_100_111_001_111,
        '6' => 0b111_100_111_101_111,
        '7' => 0b111_001_001_010_010,
        '8' => 0b111_101_111_101_111,
        '9' => 0b111_101_111_001_111,
        'a' => 0b010_101_111_101_101,
        'b' => 0b110_101_110_101_110,
        'c' => 0b011_100_100_100_011,
        'd' => 0b110_101_101_101_110,
        'e' => 0b111_100_110_100_111,
        'f' => 0b111_100_110_100_100,
        'g' => 0b011_100_101_101_011,
        'h' => 0b101_101_111_101_101,
        'i' => 0b111_010_010_010_111,
        'j' => 0b001_001_001_101_010,
        'k' => 0b101_110_100_110_101,
        'l' => 0b100_100_100_100_111,
        'm' => 0b101_111_111_101_101,
        'n' => 0b110_101_101_101_101,
        'o' => 0b010_101_101_101_010,
        'p' => 0b110_101_110_100_100,
        'q' => 0b011_101_101_011_001,
        'r' => 0b110_101_110_110_101,
        's' => 0b011_100_010_001_110,
        't' => 0b111_010_010_010_010,
        'u' => 0b101_101_101_101_111,
        'v' => 0b101_101_101_101_010,
        'w' => 0b101_101_111_111_101,
        'x' => 0b101_101_010_101_101,
        'y' => 0b101_101_010_010_010,
        'z' => 0b111_001_010_100_111,
        ':' => 0b000_010_000_010_000,
        '.' => 0b000_000_000_000_010,
        ',' => 0b000_000_000_010_100,
        '!' => 0b010_010_010_000_010,
        '-' => 0b000_000_111_000_000,
        '(' => 0b010_100_100_100_010,
        ')' => 0b010_001_001_001_010,
        ' ' => 0b000_000_000_000_000,
        _ => return None,
    };
    Some(rows)
}

/// The caret index closest to X (logical px from the start of the text)
/// — the click-to-place path: measures prefixes per char boundary and
/// keeps the closest one (the cache holds the cost).
pub fn caret_from_x(
    text: &str,
    x: Px,
    font: &FontSpec,
    engine: &dyn TextEngine,
    cache: &MeasureCache,
) -> usize {
    if x <= 0.0 || text.is_empty() {
        return 0;
    }
    let mut best = 0;
    let mut best_distance = x;
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .skip(1)
        .chain(std::iter::once(text.len()));
    for boundary in boundaries {
        let width = cache.get_or_measure(&text[..boundary], font, engine).width;
        let distance = (width - x).abs();
        if distance < best_distance {
            best = boundary;
            best_distance = distance;
        }
        if width > x {
            break; // already past the click — the closest one is behind us
        }
    }
    best
}

// MARK: - Line breaking (shape borrowed from the engine, breaking ours)

/// GREEDY breaking by word with the engine's real measurements:
/// contiguous byte ranges, one per line. A word wider than the line
/// breaks per char (never less than one). Spaces hang at the end of the
/// line (they do not force a break — the classic behavior).
pub fn break_lines(
    text: &str,
    font: &FontSpec,
    max_width: Px,
    engine: &dyn TextEngine,
    cache: &MeasureCache,
) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut paragraph = 0usize;
    loop {
        // a hard break ends a paragraph whatever the width says, and
        // the break itself belongs to NO line: the caret sits at the
        // end of one and at the start of the next
        let stop = text[paragraph..]
            .find('\n')
            .map(|offset| paragraph + offset)
            .unwrap_or(text.len());
        wrap_paragraph(text, paragraph, stop, font, max_width, engine, cache, &mut lines);
        if stop == text.len() {
            return lines;
        }
        paragraph = stop + 1;
    }
}

/// One paragraph's soft breaks, pushed in order. Always pushes at
/// least one line — an empty paragraph is an empty visual line, which
/// is where a caret goes after a lone break.
#[allow(clippy::too_many_arguments)]
fn wrap_paragraph(
    text: &str,
    start: usize,
    stop: usize,
    font: &FontSpec,
    max_width: Px,
    engine: &dyn TextEngine,
    cache: &MeasureCache,
    lines: &mut Vec<(usize, usize)>,
) {
    let mut line_start = start;
    let mut cursor = start;

    while cursor < stop {
        let rest = &text[cursor..stop];
        let is_space = rest.starts_with(' ');
        let token_len = if is_space {
            rest.find(|c| c != ' ').unwrap_or(rest.len())
        } else {
            rest.find(' ').unwrap_or(rest.len())
        };
        let token_end = cursor + token_len;

        if !is_space {
            let width = cache.get_or_measure(&text[line_start..token_end], font, engine).width;
            if width > max_width && cursor > line_start {
                // the word did not fit: break BEFORE it (the spaces
                // already walked hang at the end of the previous line)
                lines.push((line_start, cursor));
                line_start = cursor;
                continue;
            }
            if width > max_width {
                // a lone word wider than the line: break per char at the
                // largest prefix that fits — at least one
                let mut end = cursor + rest.chars().next().map(char::len_utf8).unwrap_or(1);
                for (offset, _) in rest[..token_len].char_indices().skip(1) {
                    if cache
                        .get_or_measure(&text[line_start..cursor + offset], font, engine)
                        .width
                        > max_width
                    {
                        break;
                    }
                    end = cursor + offset;
                }
                lines.push((line_start, end));
                line_start = end;
                cursor = end;
                continue;
            }
        }
        cursor = token_end;
    }
    lines.push((line_start, stop));
}

// MARK: - Measurement cache

type BreakLines = std::rc::Rc<Vec<(usize, usize)>>;

/// A prev/current double-buffer swapped per layout pass: a hit promotes
/// to current, the swap discards what nobody asked for in the frame — an
/// exact-frame LRU, no timer.
///
/// The maps are NESTED by font (and by width, for the breaks): the
/// hot-path lookup queries by `&str` without allocating any key — only
/// the MISS pays the `to_string`. Breaking has its own map with the
/// WIDTH in the key — the probing mode never shares an entry with the
/// unrestricted measurement (a cache poisoned by a proposal is
/// unrepresentable).
#[derive(Default)]
pub struct MeasureCache {
    /// The cache clock: one tick per layout pass — entry age is measured
    /// against it.
    frame: std::cell::Cell<u32>,
    lines: RefCell<HashMap<FontKey, HashMap<String, (LineMetrics, std::cell::Cell<u32>)>>>,
    breaks:
        RefCell<HashMap<(FontKey, u32), HashMap<String, (BreakLines, std::cell::Cell<u32>)>>>,
}

/// How many frames an entry survives without use. Typing ALTERNATES
/// content (backspace restores the string from two frames ago; a filter
/// hides and reveals rows) — shaping is too expensive to re-pay over one
/// frame of absence. Eight frames of slack cost a few KiB.
const CACHE_KEEP_FRAMES: u32 = 8;

impl MeasureCache {
    /// The start of a layout pass: clock tick + age sweep (drops what
    /// went [`CACHE_KEEP_FRAMES`] without use).
    pub fn begin_frame(&self) {
        let frame = self.frame.get().wrapping_add(1);
        self.frame.set(frame);
        let mut lines = self.lines.borrow_mut();
        for by_text in lines.values_mut() {
            by_text.retain(|_, (_, used)| frame.wrapping_sub(used.get()) <= CACHE_KEEP_FRAMES);
        }
        lines.retain(|_, by_text| !by_text.is_empty());
        let mut breaks = self.breaks.borrow_mut();
        for by_text in breaks.values_mut() {
            by_text.retain(|_, (_, used)| frame.wrapping_sub(used.get()) <= CACHE_KEEP_FRAMES);
        }
        breaks.retain(|_, by_text| !by_text.is_empty());
    }

    pub fn get_or_measure(
        &self,
        text: &str,
        font: &FontSpec,
        engine: &dyn TextEngine,
    ) -> LineMetrics {
        let font_key = font.key();
        if let Some((metrics, used)) = self
            .lines
            .borrow()
            .get(&font_key)
            .and_then(|by_text| by_text.get(text))
        {
            // hot hit: zero allocation, zero movement — just rejuvenates
            used.set(self.frame.get());
            crate::stats::note_measure(true);
            return *metrics;
        }
        crate::stats::note_measure(false);
        let measured = engine.measure_line(text, font);
        self.lines
            .borrow_mut()
            .entry(font_key)
            .or_default()
            .insert(text.to_string(), (measured, std::cell::Cell::new(self.frame.get())));
        measured
    }

    /// The text's breaks for THIS width (quantized in thousandths).
    pub fn get_or_break(
        &self,
        text: &str,
        font: &FontSpec,
        max_width: Px,
        engine: &dyn TextEngine,
    ) -> BreakLines {
        let mode = (font.key(), (max_width * 1000.0).round() as u32);
        if let Some((broken, used)) = self
            .breaks
            .borrow()
            .get(&mode)
            .and_then(|by_text| by_text.get(text))
        {
            used.set(self.frame.get());
            return broken.clone();
        }
        let broken = std::rc::Rc::new(break_lines(text, font, max_width, engine, self));
        self.breaks
            .borrow_mut()
            .entry(mode)
            .or_default()
            .insert(text.to_string(), (broken.clone(), std::cell::Cell::new(self.frame.get())));
        broken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_font_metrics_match_the_layout_cell() {
        let metrics = PixelFont.measure_line("abc", &FontSpec::DEFAULT);
        assert_eq!(metrics.width, 24.0);
        assert_eq!(metrics.ascent, 13.0);
        assert_eq!(metrics.descent, 3.0);
        assert_eq!(metrics.height(), crate::layout::LINE_H);
    }

    #[test]
    fn empty_text_rasters_to_nothing() {
        assert!(PixelFont.raster_line("", &FontSpec::DEFAULT, Color::BLACK, 1).is_none());
    }

    #[test]
    fn break_cache_keys_by_width_and_survives_a_frame() {
        let cache = MeasureCache::default();
        cache.begin_frame();

        let wide = cache.get_or_break("aa bb cc", &FontSpec::DEFAULT, 100.0, &PixelFont);
        assert_eq!(wide.len(), 1, "fits whole");
        let narrow = cache.get_or_break("aa bb cc", &FontSpec::DEFAULT, 40.0, &PixelFont);
        assert_eq!(narrow.len(), 2, "widths NEVER share an entry");

        // age: the next frame returns the SAME allocation
        cache.begin_frame();
        let promoted = cache.get_or_break("aa bb cc", &FontSpec::DEFAULT, 40.0, &PixelFont);
        assert!(std::rc::Rc::ptr_eq(&narrow, &promoted));
    }

    #[test]
    fn measure_cache_ages_out_after_keep_frames() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct Counting(Rc<Cell<usize>>);
        impl TextEngine for Counting {
            fn measure_line(&self, text: &str, _font: &FontSpec) -> LineMetrics {
                self.0.set(self.0.get() + 1);
                LineMetrics { width: text.len() as Px, ascent: 1.0, descent: 0.0 }
            }
            fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
                None
            }
        }

        let calls = Rc::new(Cell::new(0));
        let engine = Counting(Rc::clone(&calls));
        let cache = MeasureCache::default();

        cache.begin_frame();
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 1, "hit within the frame");

        cache.begin_frame();
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 1, "the next frame rejuvenates — zero new measurements");

        // within the age window the entry survives WITHOUT use — typing
        // alternates content; shaping does not re-pay for a frame of absence
        for _ in 0..CACHE_KEEP_FRAMES {
            cache.begin_frame();
        }
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 1, "absence WITHIN the window does not discard");

        for _ in 0..=CACHE_KEEP_FRAMES {
            cache.begin_frame();
        }
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 2, "going past the window discards the entry");
    }
}
