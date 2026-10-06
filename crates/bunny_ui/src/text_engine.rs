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
    /// 800 — heavier than bold, which display faces carry and the
    /// vocabulary could not spell.
    ExtraBold,
    /// 900 — the heaviest a face usually offers.
    Black,
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

/// How far apart the letters of a run sit, said the way a design system
/// says it: in POINTS, or in EM of the size that ends up resolved.
///
/// The em form is the one a design token is written in — a wordmark at
/// `.22em`, a display headline at `-.03em` — and it survives a change of
/// size, which the multiplication at the call site does not.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tracking {
    /// Points, straight through.
    Points(Px),
    /// A fraction of the resolved size — `.22em` is `Em(0.22)`.
    Em(Px),
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
    /// Extra advance after every character, in POINTS — SwiftUI's
    /// `.tracking`, the CSS `letter-spacing`. Already resolved: an em
    /// became points against this spec's own size.
    pub tracking: Px,
}

impl FontSpec {
    pub const DEFAULT: FontSpec = FontSpec {
        size: 13.0,
        weight: Weight::Regular,
        design: FontDesign::Default,
        slant: Slant::Upright,
        family: Family::SYSTEM,
        tracking: 0.0,
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
            tracking: 0.0,
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
            tracking_milli: (self.tracking * 1000.0).round() as i32,
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
    /// And so are two trackings — the same string at two spacings is
    /// two widths and two rasters. Signed: tracking closes a line as
    /// often as it opens one.
    tracking_milli: i32,
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
    /// Unresolved on purpose: an em only becomes points once the size
    /// it rides on is known, which is here and not at the call site.
    pub tracking: Option<Tracking>,
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
            tracking: self.tracking.or(outer.tracking),
        }
    }

    pub fn apply_over(&self, base: FontSpec) -> FontSpec {
        let size = self.size.unwrap_or(base.size);
        FontSpec {
            size,
            weight: self.weight.unwrap_or(base.weight),
            design: self.design.unwrap_or(base.design),
            slant: self.slant.unwrap_or(base.slant),
            family: self.family.unwrap_or(base.family),
            // an em resolves against the size THIS patch lands on, so
            // `.tracking_em(.22).font_size(40)` and the two written the
            // other way round mean the same thing
            tracking: match self.tracking {
                Some(Tracking::Points(points)) => points,
                Some(Tracking::Em(em)) => em * size,
                None => base.tracking,
            },
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
    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        // the cell is fixed and the SPACING is not: tracking is a
        // measure the layout asked for, so the house font answers it
        // even though it ignores size, weight and design
        LineMetrics {
            width: (text.chars().count() as Px * (PIXEL_ADVANCE + font.tracking)).max(0.0),
            ascent: PIXEL_ASCENT,
            descent: PIXEL_DESCENT,
        }
    }

    fn raster_line(
        &self,
        text: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let advance = PIXEL_ADVANCE + font.tracking;
        let width = (chars.len() as Px * advance * scale as Px).round().max(0.0) as usize;
        if width == 0 {
            return None;
        }
        let height = 16 * scale;
        let mut rgba = vec![0u8; width * height * 4];
        let mut set = |x: usize, y: usize| {
            // a closed-up line overlaps its own cells, and the last one
            // can reach past the raster: what falls outside the box is
            // dropped, never wrapped onto the next row
            if x >= width {
                return;
            }
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
            let pen = (index as Px * advance).round().max(0.0) as usize;
            let cell_x = (pen + 1) * scale;
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
    // a paragraph that fits is one line, whatever its words: a short
    // printable-ASCII one is decided by a bound (no shaping at all), any
    // other by ONE measure of the whole — the word walk below measures a
    // growing prefix per word, which a page of short lines never needs
    if paragraph_fits(&text[start..stop], font, max_width, engine, cache) {
        lines.push((start, stop));
        return;
    }
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

/// True when the paragraph is one line at `max_width`. Printable ASCII
/// is bounded by its widest glyph times its length (kerning and tracking
/// only nudge a run of these; the bound carries the margin); anything
/// else, or a long ASCII paragraph, is measured once, whole.
fn paragraph_fits(
    paragraph: &str,
    font: &FontSpec,
    max_width: Px,
    engine: &dyn TextEngine,
    cache: &MeasureCache,
) -> bool {
    if paragraph.is_empty() {
        return true;
    }
    let bytes = paragraph.as_bytes();
    if bytes.iter().all(|byte| (0x20..0x7f).contains(byte))
        && cache.ascii_advance(font, engine) * bytes.len() as Px <= max_width
    {
        return true;
    }
    cache.get_or_measure(paragraph, font, engine).width <= max_width
}

/// How many leading bytes two strings share — compared in blocks, which
/// the platform's memcmp runs at memory speed.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const BLOCK: usize = 256;
    let n = a.len().min(b.len());
    let mut at = 0;
    while at + BLOCK <= n && a[at..at + BLOCK] == b[at..at + BLOCK] {
        at += BLOCK;
    }
    while at < n && a[at] == b[at] {
        at += 1;
    }
    at
}

/// How many trailing bytes two strings share, at most `limit`.
fn common_suffix(a: &[u8], b: &[u8], limit: usize) -> usize {
    const BLOCK: usize = 256;
    let (la, lb) = (a.len(), b.len());
    let mut at = 0;
    while at + BLOCK <= limit && a[la - at - BLOCK..la - at] == b[lb - at - BLOCK..lb - at] {
        at += BLOCK;
    }
    while at < limit && a[la - 1 - at] == b[lb - 1 - at] {
        at += 1;
    }
    at
}

/// The lines of `new`, from the lines `old` was broken into: the
/// paragraphs an edit touched are wrapped again, the ones before it
/// stand, and the ones after it shift by the edit's length. The answer is
/// the one [`break_lines`] gives `new` from scratch — the test holds the
/// two to it over random edits.
#[allow(clippy::too_many_arguments)]
fn rewrap(
    old: &str,
    old_lines: &[(usize, usize)],
    new: &str,
    font: &FontSpec,
    max_width: Px,
    engine: &dyn TextEngine,
    cache: &MeasureCache,
) -> Vec<(usize, usize)> {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let prefix = common_prefix(a, b);
    let suffix = common_suffix(a, b, a.len().min(b.len()) - prefix);
    // the touched paragraphs, in NEW coordinates: from the start of the
    // one the edit begins in to the end of the one it ends in (a break
    // typed at the end of a line takes the next paragraph along, which
    // costs one more wrap and keeps the rule simple)
    let start = b[..prefix].iter().rposition(|&byte| byte == b'\n').map_or(0, |at| at + 1);
    let changed_end = b.len() - suffix;
    let end_new = b[changed_end..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(b.len(), |at| changed_end + at);
    // the same end in OLD coordinates: what follows it is the shared tail
    let end_old = a.len() - (b.len() - end_new);
    let head = old_lines.partition_point(|&(line_start, _)| line_start < start);
    let tail = old_lines.partition_point(|&(line_start, _)| line_start <= end_old);
    let mut lines = Vec::with_capacity(old_lines.len() + 4);
    lines.extend_from_slice(&old_lines[..head]);
    let mut paragraph = start;
    loop {
        let stop = new[paragraph..end_new].find('\n').map_or(end_new, |at| paragraph + at);
        wrap_paragraph(new, paragraph, stop, font, max_width, engine, cache, &mut lines);
        if stop == end_new {
            break;
        }
        paragraph = stop + 1;
    }
    let grown = b.len() as isize - a.len() as isize;
    lines.extend(old_lines[tail..].iter().map(|&(line_start, line_end)| {
        ((line_start as isize + grown) as usize, (line_end as isize + grown) as usize)
    }));
    lines
}

// MARK: - Measurement cache

type BreakLines = std::rc::Rc<Vec<(usize, usize)>>;

/// Measures and line breaks, kept by text: shaping is the costly call,
/// and a scene asks for the same strings on every pass that measures.
///
/// An entry AGES, and an old one is dropped — but only in a cache that is
/// full ([`MeasureCache::floor`]). The layout no longer measures a boundary
/// that did not re-run, so a string can go many frames without a lookup and
/// still be on screen. An age rule alone would empty the cache between two
/// keystrokes, and the body that re-runs would re-shape every string in
/// it. Under the floor nothing is dropped, and the pass does not even walk
/// the entries.
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
    /// Entries in both maps. A sweep recounts it.
    entries: std::cell::Cell<usize>,
    /// The size under which nothing ages out. `None` = [`CACHE_FLOOR`].
    floor: Option<usize>,
    lines: RefCell<HashMap<FontKey, HashMap<String, (LineMetrics, std::cell::Cell<u32>)>>>,
    breaks:
        RefCell<HashMap<(FontKey, u32), HashMap<String, (BreakLines, std::cell::Cell<u32>)>>>,
    /// Multi-line fields, by path: their lines at the widths they were
    /// laid out at ([`MeasureCache::field_lines`]).
    fields: RefCell<HashMap<String, Vec<FieldLines>>>,
    /// The widest printable ASCII glyph of each font, with a margin — the
    /// bound that lets a short ASCII paragraph skip shaping.
    ascii_advance: RefCell<HashMap<FontKey, Px>>,
}

/// One multi-line field's visual lines at one width, kept from frame to
/// frame with the text they break.
struct FieldLines {
    mode: (FontKey, u32),
    text: Arc<str>,
    lines: BreakLines,
    used: std::cell::Cell<u32>,
}

/// The widths one field keeps lines for at once — a field measured at a
/// probe width and placed at its own needs two.
const FIELD_WIDTHS_KEPT: usize = 3;

/// How many passes a field's lines outlive its last layout.
const FIELD_KEEP_FRAMES: u32 = 64;

/// How many frames an entry survives without use. Typing ALTERNATES
/// content (backspace restores the string from two frames ago; a filter
/// hides and reveals rows) — shaping is too expensive to re-pay over one
/// frame of absence. Eight frames of slack cost a few KiB.
const CACHE_KEEP_FRAMES: u32 = 8;

/// The size under which the cache drops nothing. A few thousand strings
/// with their metrics are well under a mebibyte, and a product screen
/// holds fewer than that.
const CACHE_FLOOR: usize = 8192;

/// A full cache is swept on one pass in this many, not on every one.
const SWEEP_EVERY: u32 = 8;

impl MeasureCache {
    /// A cache that starts to age its entries at `floor` of them — `0`
    /// ages every entry, which is the rule the age tests pin.
    pub fn with_floor(floor: usize) -> MeasureCache {
        MeasureCache { floor: Some(floor), ..MeasureCache::default() }
    }

    fn floor(&self) -> usize {
        self.floor.unwrap_or(CACHE_FLOOR)
    }

    /// Diagnostics: the entries in both maps.
    pub fn len(&self) -> usize {
        self.entries.get()
    }

    /// Diagnostics: no entry at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The start of a layout pass: the clock ticks. A cache over its floor
    /// is swept — what went [`CACHE_KEEP_FRAMES`] without use is dropped —
    /// on one pass in [`SWEEP_EVERY`]; with a floor of zero, on every pass.
    pub fn begin_frame(&self) {
        let frame = self.frame.get().wrapping_add(1);
        self.frame.set(frame);
        if frame % SWEEP_EVERY == 0 {
            let mut fields = self.fields.borrow_mut();
            for kept in fields.values_mut() {
                kept.retain(|lines| frame.wrapping_sub(lines.used.get()) <= FIELD_KEEP_FRAMES);
            }
            fields.retain(|_, kept| !kept.is_empty());
        }
        let floor = self.floor();
        if self.entries.get() <= floor || (floor > 0 && frame % SWEEP_EVERY != 0) {
            return;
        }
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
        let kept = lines.values().map(HashMap::len).sum::<usize>()
            + breaks.values().map(HashMap::len).sum::<usize>();
        self.entries.set(kept);
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
        self.entries.set(self.entries.get() + 1);
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
        self.entries.set(self.entries.get() + 1);
        broken
    }

    /// A multi-line field's visual lines — what [`MeasureCache::get_or_break`]
    /// answers, kept per field (its `path`) and per width instead of per
    /// text. A string that differs from the kept one is compared with it
    /// (shared head, shared tail) and only the paragraphs between are
    /// wrapped again: a note of thirty thousand lines re-wraps the line
    /// being typed, and the cache holds one copy of the text per field,
    /// not one per keystroke. `shared` is the caller's own handle on the
    /// text, adopted so the next frame's question is a pointer compare.
    pub fn field_lines(
        &self,
        path: &str,
        text: &str,
        shared: Option<&Arc<str>>,
        font: &FontSpec,
        max_width: Px,
        engine: &dyn TextEngine,
    ) -> BreakLines {
        let mode = (font.key(), (max_width * 1000.0).round() as u32);
        let now = self.frame.get();
        let owned = || shared.cloned().unwrap_or_else(|| Arc::from(text));
        let mut fields = self.fields.borrow_mut();
        if let Some(kept) = fields.get_mut(path).and_then(|all| all.iter_mut().find(|kept| kept.mode == mode)) {
            kept.used.set(now);
            let same = shared.is_some_and(|handle| Arc::ptr_eq(handle, &kept.text)) || *kept.text == *text;
            if !same {
                let lines = rewrap(&kept.text, &kept.lines, text, font, max_width, engine, self);
                kept.lines = std::rc::Rc::new(lines);
            }
            if !same || shared.is_some() {
                kept.text = owned();
            }
            return kept.lines.clone();
        }
        let lines = std::rc::Rc::new(break_lines(text, font, max_width, engine, self));
        let fresh = FieldLines { mode, text: owned(), lines: lines.clone(), used: std::cell::Cell::new(now) };
        match fields.get_mut(path) {
            Some(all) => {
                // a field seen at more widths than it keeps forgets the
                // one it used longest ago
                if all.len() >= FIELD_WIDTHS_KEPT
                    && let Some(oldest) = (0..all.len()).max_by_key(|&at| now.wrapping_sub(all[at].used.get()))
                {
                    all.swap_remove(oldest);
                }
                all.push(fresh);
            }
            None => {
                fields.insert(path.to_string(), vec![fresh]);
            }
        }
        lines
    }

    /// The widest printable ASCII glyph of `font`, with a margin for the
    /// nudges of kerning and tracking — measured once per font.
    fn ascii_advance(&self, font: &FontSpec, engine: &dyn TextEngine) -> Px {
        let key = font.key();
        if let Some(&advance) = self.ascii_advance.borrow().get(&key) {
            return advance;
        }
        let mut widest: Px = 0.0;
        for byte in 0x20u8..0x7f {
            let glyph = [byte];
            let glyph = std::str::from_utf8(&glyph).unwrap_or(" ");
            widest = widest.max(engine.measure_line(glyph, font).width);
        }
        let advance = widest * 1.05;
        self.ascii_advance.borrow_mut().insert(key, advance);
        advance
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

    /// A small deterministic generator — the house keeps no dependency
    /// for a test's dice.
    struct Dice(u64);
    impl Dice {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    fn words(dice: &mut Dice, len: usize) -> String {
        const ALPHABET: [&str; 7] = ["a", "bb", " ", " ", "\n", "é", "ccccccc"];
        (0..len).map(|_| ALPHABET[dice.below(ALPHABET.len())]).collect()
    }

    #[test]
    fn a_rewrap_after_an_edit_is_the_break_from_scratch() {
        let mut dice = Dice(0x5eed_cafe);
        let font = FontSpec::DEFAULT;
        for round in 0..400 {
            let cache = MeasureCache::default();
            let width = [24.0, 40.0, 64.0, 100.0, 1000.0][dice.below(5)];
            let old_len = dice.below(40);
            let old = words(&mut dice, old_len);
            let old_lines = break_lines(&old, &font, width, &PixelFont, &cache);
            // an edit: a run replaced by another, anywhere (char boundaries)
            let bounds: Vec<usize> = old.char_indices().map(|(at, _)| at).chain([old.len()]).collect();
            let first = dice.below(bounds.len());
            let last = first + dice.below(bounds.len() - first);
            let (from, to) = (bounds[first], bounds[last]);
            let inserted_len = dice.below(6);
            let inserted = words(&mut dice, inserted_len);
            let new = format!("{}{}{}", &old[..from], inserted, &old[to..]);
            let rewrapped = rewrap(&old, &old_lines, &new, &font, width, &PixelFont, &cache);
            let fresh = break_lines(&new, &font, width, &PixelFont, &cache);
            assert_eq!(rewrapped, fresh, "round {round}: {old:?} → {new:?} at width {width}");
        }
    }

    #[test]
    fn a_field_keeps_one_text_and_its_lines_follow_each_keystroke() {
        let cache = MeasureCache::default();
        let font = FontSpec::DEFAULT;
        let mut note: String = (0..300).map(|line| format!("line {line} of the note\n")).collect();
        for stroke in 0..50 {
            cache.begin_frame();
            note.insert(note.len() / 2, if stroke % 7 == 6 { '\n' } else { 'x' });
            let text: Arc<str> = Arc::from(note.as_str());
            let kept = cache.field_lines("note", &text, Some(&text), &font, 200.0, &PixelFont);
            assert_eq!(*kept, break_lines(&note, &font, 200.0, &PixelFont, &cache), "stroke {stroke}");
        }
        assert_eq!(cache.fields.borrow().len(), 1, "one field");
        assert_eq!(cache.fields.borrow()["note"].len(), 1, "one width, one text");
        assert_eq!(
            cache.breaks.borrow().values().map(HashMap::len).sum::<usize>(),
            0,
            "no note was kept whole as a break key"
        );
    }

    #[test]
    fn a_short_ascii_paragraph_is_one_line_without_shaping() {
        let cache = MeasureCache::default();
        let font = FontSpec::DEFAULT;
        let lines = break_lines("short\nlines\nhere", &font, 1000.0, &PixelFont, &cache);
        assert_eq!(lines, vec![(0, 5), (6, 11), (12, 16)]);
        assert_eq!(cache.len(), 0, "the bound decided: no string was measured");
        // a paragraph past the bound (30 × 5 cells × 8 px × 1.05) that
        // still fits is measured once, whole
        let long = "word ".repeat(30);
        let lines = break_lines(&long, &font, 1230.0, &PixelFont, &cache);
        assert_eq!(lines, vec![(0, long.len())]);
        assert_eq!(cache.len(), 1, "one measure of the whole paragraph");
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
        // a floor of zero: every entry ages, on every pass
        let cache = MeasureCache::with_floor(0);

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

        // …and under its floor a cache drops nothing, however long a
        // string goes without a lookup: a boundary that did not re-run is
        // not measured, and its text is still on screen
        let calls = Rc::new(Cell::new(0));
        let engine = Counting(Rc::clone(&calls));
        let cache = MeasureCache::default();
        cache.begin_frame();
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        for _ in 0..10 * CACHE_KEEP_FRAMES {
            cache.begin_frame();
        }
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 1, "a cache under its floor keeps what it measured");
    }
}
