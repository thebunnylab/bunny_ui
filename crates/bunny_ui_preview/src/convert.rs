//! The framework's own types to the wire's and back — what a host does
//! to a frame before sending it, and what a tool does to a frame before
//! painting it with the framework's own painter.
//!
//! Geometry and colour convert one to one. Text travels as an index into
//! the frame's string table, a font's family as its name. An image is
//! named by the key of its innermost source, with the fades and flips
//! the app wrapped it in folded into an [`ImageRef`]; the bytes behind a
//! key are a [`Resource`] the host sends once. A symbol of the
//! framework's own travels by name, which the tool knows; an app's own
//! travels as what it draws — the one traced path, when one path draws
//! it, and the whole glyph otherwise — under its drawing's key. A site's
//! `Location` becomes a file, a line and a column; a site the framework
//! named itself keeps its name. A theme travels as its tokens by name.
//!
//! ## Production gotchas
//!
//! - A `Native` image — a platform surface — has no resource: [`lower`]
//!   still lists it, [`resource`] answers `None`, and [`raise`] draws
//!   nothing for a key the tool holds no source for unless the caller
//!   put a placeholder under that key.
//! - A wrapped image raised from the wire carries keys derived from the
//!   inner one, not the app's original wrapper keys; what paints is the
//!   same, what caches is keyed afresh.
//! - One traced path draws an app's symbol only when every draw takes
//!   the same ink and the draws are one, or pens of one width; it then
//!   rides `RESOURCE`, which every tool reads. A glyph that sets a fill
//!   beside other draws, or draws in several inks, travels whole on the
//!   `GLYPH` kind, and the tool paints it with the framework's own glyph
//!   rasterizer. Splitting it into paths would not draw it: the house
//!   piles one ink's draws in by MAX, while pictures composited over each
//!   other darken where they overlap. A tool that predates `GLYPH` skips
//!   it, so the host sends the symbol by name first — the most such a
//!   tool can show is a stand-in it names.
//! - A tool keeps every glyph it is sent, once per key, for as long as it
//!   runs: the house rasterizes a glyph from `&'static` tables, so a
//!   received drawing is leaked into them, as `Symbol::from_svg` leaks a
//!   drawing it reads. An icon set is small; an edit under hot reload adds
//!   one drawing more.
//! - The traced path scales to its box's width from the top-left corner;
//!   the house centres a glyph on the largest square of its box. The two
//!   agree on a square, the box `icon(…)` and `Painter::icon` always
//!   give a symbol; a symbol stretched into another shape (a resizable
//!   `image(…)`, a hand-placed `Painter::image`) differs.
//! - An app symbol's key on the wire is its drawing's, not its name's:
//!   the host sends a key once per connection, so a drawing edited under
//!   hot reload has to be a new key to reach the tool at all.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hasher;
use std::rc::Rc;
use std::sync::Arc;

use bunny_ui_core::icon;
use bunny_ui_core::image_engine::{ImageSource, PixelFormat};
use bunny_ui_core::layout::{self, DisplayList, DrawCommand, ImagePlacement};
use bunny_ui_core::modifier::Modifier;
use bunny_ui_core::prelude::{Alignment, Edge, LayoutDirection, Locale, Site, SizeClass};
use bunny_ui_core::preview as core_preview;
use bunny_ui_core::provenance::SitePlacement;
use bunny_ui_core::text_engine::{Family, FontDesign, FontSpec, Slant, TextEngine, Weight};
use bunny_ui_core::theme::Theme;

use crate::wire::{self, Draw, ImageRef, Resource, ResourceKind, Strings};

// =============================================================================
// Geometry and colour
// =============================================================================

impl From<layout::Point> for wire::Point {
    fn from(p: layout::Point) -> Self {
        wire::Point { x: p.x, y: p.y }
    }
}

impl From<wire::Point> for layout::Point {
    fn from(p: wire::Point) -> Self {
        layout::Point { x: p.x, y: p.y }
    }
}

impl From<layout::UnitPoint> for wire::Point {
    fn from(p: layout::UnitPoint) -> Self {
        wire::Point { x: p.x, y: p.y }
    }
}

impl From<wire::Point> for layout::UnitPoint {
    fn from(p: wire::Point) -> Self {
        layout::UnitPoint { x: p.x, y: p.y }
    }
}

impl From<layout::Size> for wire::Size {
    fn from(s: layout::Size) -> Self {
        wire::Size { width: s.width, height: s.height }
    }
}

impl From<wire::Size> for layout::Size {
    fn from(s: wire::Size) -> Self {
        layout::Size { width: s.width, height: s.height }
    }
}

impl From<layout::Rect> for wire::Rect {
    fn from(r: layout::Rect) -> Self {
        wire::Rect { origin: r.origin.into(), size: r.size.into() }
    }
}

impl From<wire::Rect> for layout::Rect {
    fn from(r: wire::Rect) -> Self {
        layout::Rect { origin: r.origin.into(), size: r.size.into() }
    }
}

impl From<layout::Color> for wire::Color {
    fn from(c: layout::Color) -> Self {
        wire::Color { r: c.r, g: c.g, b: c.b, a: c.a }
    }
}

impl From<wire::Color> for layout::Color {
    fn from(c: wire::Color) -> Self {
        layout::Color { r: c.r, g: c.g, b: c.b, a: c.a }
    }
}

impl From<layout::Corners> for wire::Corners {
    fn from(c: layout::Corners) -> Self {
        wire::Corners { top_left: c.top_left, top_right: c.top_right, bottom_right: c.bottom_right, bottom_left: c.bottom_left }
    }
}

impl From<wire::Corners> for layout::Corners {
    fn from(c: wire::Corners) -> Self {
        layout::Corners { top_left: c.top_left, top_right: c.top_right, bottom_right: c.bottom_right, bottom_left: c.bottom_left }
    }
}

impl From<layout::Edges> for wire::Edges {
    fn from(e: layout::Edges) -> Self {
        wire::Edges { top: e.top, trailing: e.trailing, bottom: e.bottom, leading: e.leading }
    }
}

impl From<wire::Edges> for layout::Edges {
    fn from(e: wire::Edges) -> Self {
        layout::Edges { top: e.top, trailing: e.trailing, bottom: e.bottom, leading: e.leading }
    }
}

impl From<layout::GradientPaint> for wire::GradientPaint {
    fn from(g: layout::GradientPaint) -> Self {
        match g {
            layout::GradientPaint::Radial { center, start, end, aspect, inner, outer } => {
                wire::GradientPaint::Radial { center: center.into(), start, end, aspect, inner: inner.into(), outer: outer.into() }
            }
            layout::GradientPaint::Linear { start, end, from, to } => {
                wire::GradientPaint::Linear { start: start.into(), end: end.into(), from: from.into(), to: to.into() }
            }
        }
    }
}

impl From<wire::GradientPaint> for layout::GradientPaint {
    fn from(g: wire::GradientPaint) -> Self {
        match g {
            wire::GradientPaint::Radial { center, start, end, aspect, inner, outer } => {
                layout::GradientPaint::Radial { center: center.into(), start, end, aspect, inner: inner.into(), outer: outer.into() }
            }
            wire::GradientPaint::Linear { start, end, from, to } => {
                layout::GradientPaint::Linear { start: start.into(), end: end.into(), from: from.into(), to: to.into() }
            }
        }
    }
}

impl From<layout::Gradient> for wire::Gradient {
    fn from(g: layout::Gradient) -> Self {
        match g {
            layout::Gradient::Radial { center, start, end, aspect, inner, outer } => {
                wire::Gradient::Radial { center: center.into(), start, end, aspect, inner: inner.into(), outer: outer.into() }
            }
            layout::Gradient::Linear { start, end, from, to } => {
                wire::Gradient::Linear { start: start.into(), end: end.into(), from: from.into(), to: to.into() }
            }
        }
    }
}

impl From<wire::Gradient> for layout::Gradient {
    fn from(g: wire::Gradient) -> Self {
        match g {
            wire::Gradient::Radial { center, start, end, aspect, inner, outer } => {
                layout::Gradient::Radial { center: center.into(), start, end, aspect, inner: inner.into(), outer: outer.into() }
            }
            wire::Gradient::Linear { start, end, from, to } => {
                layout::Gradient::Linear { start: start.into(), end: end.into(), from: from.into(), to: to.into() }
            }
        }
    }
}

impl From<layout::GlassPaint> for wire::Glass {
    fn from(g: layout::GlassPaint) -> Self {
        wire::Glass {
            blur: g.blur,
            tint: g.tint.into(),
            refraction_band: g.refraction_band,
            refraction_amount: g.refraction_amount,
            chromatic: g.chromatic,
            highlight: g.highlight.into(),
            highlight_band: g.highlight_band,
            highlight_intensity: g.highlight_intensity,
            saturation: g.saturation,
            brightness: g.brightness,
            sheen: g.sheen,
            spot_center: g.spot_center.into(),
            spot_radius: g.spot_radius,
            spot_alpha: g.spot_alpha,
            opacity: g.opacity,
        }
    }
}

impl From<wire::Glass> for layout::GlassPaint {
    fn from(g: wire::Glass) -> Self {
        layout::GlassPaint {
            blur: g.blur,
            tint: g.tint.into(),
            refraction_band: g.refraction_band,
            refraction_amount: g.refraction_amount,
            chromatic: g.chromatic,
            highlight: g.highlight.into(),
            highlight_band: g.highlight_band,
            highlight_intensity: g.highlight_intensity,
            saturation: g.saturation,
            brightness: g.brightness,
            sheen: g.sheen,
            spot_center: g.spot_center.into(),
            spot_radius: g.spot_radius,
            spot_alpha: g.spot_alpha,
            opacity: g.opacity,
        }
    }
}

// =============================================================================
// Fonts
// =============================================================================

impl From<Weight> for wire::Weight {
    fn from(w: Weight) -> Self {
        match w {
            Weight::Thin => wire::Weight::Thin,
            Weight::ExtraLight => wire::Weight::ExtraLight,
            Weight::Light => wire::Weight::Light,
            Weight::Regular => wire::Weight::Regular,
            Weight::Medium => wire::Weight::Medium,
            Weight::Semibold => wire::Weight::Semibold,
            Weight::Bold => wire::Weight::Bold,
            Weight::ExtraBold => wire::Weight::ExtraBold,
            Weight::Black => wire::Weight::Black,
        }
    }
}

impl From<wire::Weight> for Weight {
    fn from(w: wire::Weight) -> Self {
        match w {
            wire::Weight::Thin => Weight::Thin,
            wire::Weight::ExtraLight => Weight::ExtraLight,
            wire::Weight::Light => Weight::Light,
            wire::Weight::Regular => Weight::Regular,
            wire::Weight::Medium => Weight::Medium,
            wire::Weight::Semibold => Weight::Semibold,
            wire::Weight::Bold => Weight::Bold,
            wire::Weight::ExtraBold => Weight::ExtraBold,
            wire::Weight::Black => Weight::Black,
        }
    }
}

/// A font, its family interned into the frame's strings.
pub fn font(spec: &FontSpec, strings: &mut Strings) -> wire::Font {
    wire::Font {
        size: spec.size,
        weight: spec.weight.into(),
        design: match spec.design {
            FontDesign::Default => wire::Design::Default,
            FontDesign::Mono => wire::Design::Mono,
        },
        slant: match spec.slant {
            Slant::Upright => wire::Slant::Upright,
            Slant::Italic => wire::Slant::Italic,
        },
        family: spec.family.name().map(|name| strings.intern(&name)),
        tracking: spec.tracking,
    }
}

/// A font spec, its family interned again on this side.
pub fn font_spec(font: &wire::Font, strings: &[String]) -> Result<FontSpec, Missing> {
    let family = match font.family {
        None => Family::SYSTEM,
        Some(index) => Family::named(string(strings, index)?),
    };
    Ok(FontSpec {
        size: font.size,
        weight: font.weight.into(),
        design: match font.design {
            wire::Design::Default => FontDesign::Default,
            wire::Design::Mono => FontDesign::Mono,
        },
        slant: match font.slant {
            wire::Slant::Upright => Slant::Upright,
            wire::Slant::Italic => Slant::Italic,
        },
        family,
        tracking: font.tracking,
    })
}

// =============================================================================
// Images
// =============================================================================

/// The wire's name for an image: its innermost source's key with the
/// fades and flips folded in — and that innermost source as the wire
/// carries it: a symbol of the app's own as the traced path it draws when
/// one path draws it, and under its drawing's key when it travels whole.
pub fn image_ref(source: &ImageSource) -> (ImageRef, Cow<'_, ImageSource>) {
    let mut alpha: u32 = 255;
    let mut mirrored = false;
    let mut at = source;
    loop {
        match at {
            ImageSource::Faded { inner, alpha: a, .. } => {
                alpha = alpha * u32::from(*a) / 255;
                at = inner;
            }
            ImageSource::Mirrored { inner, .. } => {
                mirrored = !mirrored;
                at = inner;
            }
            _ => break,
        }
    }
    let inner = carried(at);
    (ImageRef { key: image_key(&inner), alpha: alpha as u8, mirrored }, inner)
}

/// What the wire carries for an innermost source: the source itself, or
/// — for a symbol of the app's own, which no tool knows by name — the one
/// traced path it draws when one path draws it, and the symbol itself,
/// under its drawing's key, when it travels whole.
///
/// A TURNED glyph travels as the drawing it is: the host turns its verbs,
/// and the tool draws them upright. One pen and one ink is one traced
/// path, as ever; anything else travels whole, kept under the key that
/// folds the turn. The wire is frozen, and it never hears of a turn.
fn carried(source: &ImageSource) -> Cow<'_, ImageSource> {
    match source {
        ImageSource::Symbol { symbol, color, forced, turn, .. } if *turn != 0.0 => {
            let draws: Vec<wire::GlyphDraw> = symbol
                .glyph
                .draws
                .iter()
                .map(|draw| wire_draw(draw.paint, draw.path, draw.tint, *turn))
                .collect();
            if let Some(path) = traced_draws(&draws, *color, *forced) {
                return Cow::Owned(path);
            }
            let key = drawing_key(symbol, *color, *forced, *turn);
            Cow::Owned(ImageSource::Symbol {
                key,
                symbol: kept(key, symbol.name, &draws),
                color: *color,
                forced: *forced,
                turn: 0.0,
            })
        }
        ImageSource::Symbol { symbol, color, forced, .. } if !is_house(symbol) => {
            Cow::Owned(traced(symbol.glyph, *color, *forced).unwrap_or_else(|| ImageSource::Symbol {
                key: drawing_key(symbol, *color, *forced, 0.0),
                symbol: *symbol,
                color: *color,
                forced: *forced,
                turn: 0.0,
            }))
        }
        _ => Cow::Borrowed(source),
    }
}

/// One draw as the frozen wire carries it: turned `turn` radians about
/// the grid's centre, and a dashed pen lowered to the plain geometry it
/// draws ([`icon::undashed`]) — the wire knows a fill and a round pen.
fn wire_draw(paint: icon::Paint, path: &[icon::Verb], tint: Option<layout::Color>, turn: f32) -> wire::GlyphDraw {
    let centre = icon::ICON_GRID / 2.0;
    let turned: Cow<'_, [icon::Verb]> = if turn == 0.0 {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(icon::turned(path, f64::from(turn), (centre, centre)))
    };
    let (verbs, paint) = plain(&turned, paint);
    wire::GlyphDraw {
        paint: paint_to_wire(paint),
        verbs: verbs.iter().map(|verb| verb_to_wire(*verb)).collect(),
        tint: tint.map(Into::into),
    }
}

/// A path and its paint as the wire can say them: a dash lowered to its
/// geometry, every other paint as it is.
fn plain(path: &[icon::Verb], paint: icon::Paint) -> (Cow<'_, [icon::Verb]>, icon::Paint) {
    match paint {
        icon::Paint::Dashed { width, dash } => {
            let (verbs, paint) = icon::undashed(path, width, dash);
            (Cow::Owned(verbs), paint)
        }
        other => (Cow::Borrowed(path), other),
    }
}

/// [`traced`]'s rule over draws already on the wire's terms: one draw,
/// or round pens of one width in one ink, is one traced path.
fn traced_draws(draws: &[wire::GlyphDraw], color: layout::Color, forced: bool) -> Option<ImageSource> {
    let ink_of = |draw: &wire::GlyphDraw| if forced { color } else { draw.tint.map_or(color, Into::into) };
    let (first, rest) = draws.split_first()?;
    let one_pen = rest.is_empty()
        || (matches!(first.paint, wire::Paint::Stroke { .. })
            && rest.iter().all(|draw| draw.paint == first.paint && ink_of(draw) == ink_of(first)));
    if !one_pen {
        return None;
    }
    let verbs: Vec<icon::Verb> = draws
        .iter()
        .flat_map(|draw| draw.verbs.iter().map(|verb| verb_from_wire(*verb)))
        .skip_while(|verb| !matches!(verb, icon::Verb::Move(..)))
        .collect();
    let grid = icon::ICON_GRID as f32;
    Some(ImageSource::path(verbs, paint_from_wire(first.paint), icon::Ink::Solid(ink_of(first)), (grid, grid)))
}

/// The domain tag folded into a whole glyph's key: "bny_glyp".
const GLYPH_TAG: u64 = 0x626e_795f_676c_7970;

/// FNV-1a, the hash a symbol's name is folded with — small, and the same
/// on every build.
struct Fnv(u64);

impl Hasher for Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// The key an app's glyph travels whole under: its name, its draws, its
/// ink and its reading, folded together — the drawing's identity, so an
/// edit under hot reload is a new key. The house's own key folds only the
/// name, and a tool keeps a glyph per key.
fn drawing_key(symbol: &icon::Symbol, color: layout::Color, forced: bool, turn: f32) -> u64 {
    let mut fold = Fnv(0xcbf2_9ce4_8422_2325);
    let colour = |fold: &mut Fnv, color: layout::Color| fold.write(&[color.r, color.g, color.b, color.a]);
    fold.write_u64(GLYPH_TAG);
    fold.write_usize(symbol.name.len());
    fold.write(symbol.name.as_bytes());
    colour(&mut fold, color);
    fold.write_u8(u8::from(forced));
    // only a turned drawing folds its turn: an upright one keeps its key
    if turn != 0.0 {
        fold.write_u32(turn.to_bits());
    }
    fold.write_usize(symbol.glyph.draws.len());
    for draw in symbol.glyph.draws {
        match draw.paint {
            icon::Paint::Fill(icon::Rule::NonZero) => fold.write_u8(0),
            icon::Paint::Fill(icon::Rule::EvenOdd) => fold.write_u8(1),
            icon::Paint::Stroke { width } => {
                fold.write_u8(2);
                fold.write_u32(width.to_bits());
            }
            icon::Paint::Dashed { width, dash } => {
                fold.write_u8(3);
                for value in [width, dash.on, dash.off, dash.phase] {
                    fold.write_u32(value.to_bits());
                }
                fold.write_u8(dash.cap as u8);
            }
        }
        match draw.tint {
            None => fold.write_u8(0),
            Some(tint) => {
                fold.write_u8(1);
                colour(&mut fold, tint);
            }
        }
        fold.write_usize(draw.path.len());
        for verb in draw.path {
            let (tag, numbers): (u8, &[f32]) = match verb {
                icon::Verb::Move(x, y) => (0, &[*x, *y]),
                icon::Verb::Line(x, y) => (1, &[*x, *y]),
                icon::Verb::Quad(a, b, x, y) => (2, &[*a, *b, *x, *y]),
                icon::Verb::Cubic(a, b, c, d, x, y) => (3, &[*a, *b, *c, *d, *x, *y]),
                icon::Verb::Close => (4, &[]),
            };
            fold.write_u8(tag);
            for number in numbers {
                fold.write_u32(number.to_bits());
            }
        }
    }
    fold.finish()
}

/// Whether a symbol is the framework's own of its name — the one a tool
/// draws from the name alone. A symbol the app names like a house one
/// and draws otherwise is the app's.
fn is_house(symbol: &icon::Symbol) -> bool {
    icon::house::named(symbol.name).is_some_and(|house| {
        house == *symbol
            && house.glyph.draws.len() == symbol.glyph.draws.len()
            && house
                .glyph
                .draws
                .iter()
                .zip(symbol.glyph.draws)
                .all(|(ours, theirs)| ours.paint == theirs.paint && ours.path == theirs.path && ours.tint == theirs.tint)
    })
}

/// The one traced path that draws `glyph` in `color` byte for byte on
/// the square an icon is given — `None` when it takes more than one.
///
/// The house lays a glyph on the [`icon::ICON_GRID`] square: the draws
/// of one ink pile their coverage in by MAX and that run blends once; a
/// draw of another ink paints over it. A traced path is one paint and
/// one ink on the same square, so it is the glyph when every draw takes
/// one ink — its own tint, or `color` (every draw's, when `forced`) —
/// and the draws are one, or pens of one width: a pen's coverage is its
/// distance to the nearest segment, piled in by MAX, so one pen over
/// all their contours is their union. Two fills do not merge (one's
/// winding would cut or fill the other), nor do a fill and a pen.
///
/// The pens' verbs join from each draw's first `Move`: a verb before it
/// draws nothing on its own, but after another draw it would carry on
/// that draw's contour.
fn traced(glyph: &icon::Glyph, color: layout::Color, forced: bool) -> Option<ImageSource> {
    let ink_of = |draw: &icon::Draw| if forced { color } else { draw.tint.unwrap_or(color) };
    let (paint, ink, verbs) = match glyph.draws {
        // a glyph with nothing in it draws nothing, not a stand-in
        [] => (icon::Paint::Fill(icon::Rule::NonZero), color, Vec::new()),
        [draw] => (draw.paint, ink_of(draw), draw.path.to_vec()),
        [first, rest @ ..] => {
            let one_pen = matches!(first.paint, icon::Paint::Stroke { .. })
                && rest.iter().all(|draw| draw.paint == first.paint && ink_of(draw) == ink_of(first));
            if !one_pen {
                return None;
            }
            let verbs = glyph
                .draws
                .iter()
                .flat_map(|draw| draw.path.iter().skip_while(|verb| !matches!(verb, icon::Verb::Move(..))))
                .copied()
                .collect();
            (first.paint, ink_of(first), verbs)
        }
    };
    let grid = icon::ICON_GRID as f32;
    Some(ImageSource::path(verbs, paint, icon::Ink::Solid(ink), (grid, grid)))
}

/// The key of a source — its own. The key the wire names an image by is
/// [`image_ref`]'s, which is the drawing's for an app's own symbol.
pub fn image_key(source: &ImageSource) -> u64 {
    match source {
        ImageSource::Bytes { key, .. }
        | ImageSource::FileIcon { key, .. }
        | ImageSource::Symbol { key, .. }
        | ImageSource::Path { key, .. }
        | ImageSource::Rgba { key, .. }
        | ImageSource::Native { key, .. }
        | ImageSource::Feed { key, .. }
        | ImageSource::Faded { key, .. }
        | ImageSource::Mirrored { key, .. }
        | ImageSource::World { key, .. } => *key,
    }
}

/// The resource behind a source, for the wire — `None` for a platform
/// surface, which has no bytes to send. A wrapped source answers its
/// innermost one's, as the wire carries it: an app's own symbol sends
/// the path it draws when one path draws it, and the whole glyph
/// otherwise ([`Resource::by_name`] is what goes just before that).
pub fn resource(source: &ImageSource) -> Option<Resource> {
    let (_, inner) = image_ref(source);
    let kind = match &*inner {
        ImageSource::Bytes { bytes, .. } => ResourceKind::Encoded { bytes: bytes.to_vec() },
        ImageSource::FileIcon { path, .. } => ResourceKind::FileIcon { path: path.to_string() },
        ImageSource::Symbol { symbol, color, forced, .. } if is_house(symbol) => {
            ResourceKind::Symbol { name: symbol.name.to_string(), color: (*color).into(), forced: *forced }
        }
        // an app's own that no single path draws: the glyph, whole
        ImageSource::Symbol { symbol, color, forced, .. } => ResourceKind::Glyph {
            name: symbol.name.to_string(),
            color: (*color).into(),
            forced: *forced,
            draws: symbol
                .glyph
                .draws
                .iter()
                .map(|draw| wire_draw(draw.paint, draw.path, draw.tint, 0.0))
                .collect(),
        },
        ImageSource::Path { verbs, paint, ink, box_size, .. } => {
            let (verbs, paint) = plain(verbs, *paint);
            ResourceKind::Vector {
            verbs: verbs.iter().map(|verb| verb_to_wire(*verb)).collect(),
            paint: paint_to_wire(paint),
            ink: match *ink {
                icon::Ink::Solid(color) => wire::Ink::Solid(color.into()),
                icon::Ink::Ramp(gradient) => wire::Ink::Ramp(gradient.into()),
            },
            box_size: *box_size,
        }
        },
        ImageSource::Rgba { size, rgba, .. } => ResourceKind::Rgba { size: *size, rgba: rgba.to_vec() },
        ImageSource::Feed { generation, size, bytes, .. } => {
            ResourceKind::Feed { generation: *generation, size: *size, rgba: bytes.to_vec() }
        }
        // a world is drawn where it shows, like a platform surface: the
        // wire carries no picture of it
        ImageSource::Native { .. } | ImageSource::Faded { .. } | ImageSource::Mirrored { .. } | ImageSource::World { .. } => {
            return None;
        }
    };
    Some(Resource { key: image_key(&inner), kind })
}

fn paint_to_wire(paint: icon::Paint) -> wire::Paint {
    match paint {
        icon::Paint::Fill(icon::Rule::NonZero) => wire::Paint::Fill { even_odd: false },
        icon::Paint::Fill(icon::Rule::EvenOdd) => wire::Paint::Fill { even_odd: true },
        icon::Paint::Stroke { width } => wire::Paint::Stroke { width },
        // every caller lowers a dash first ([`plain`]); one that did not
        // would still draw its line, solid, rather than nothing
        icon::Paint::Dashed { width, .. } => wire::Paint::Stroke { width },
    }
}

fn paint_from_wire(paint: wire::Paint) -> icon::Paint {
    match paint {
        wire::Paint::Fill { even_odd: false } => icon::Paint::Fill(icon::Rule::NonZero),
        wire::Paint::Fill { even_odd: true } => icon::Paint::Fill(icon::Rule::EvenOdd),
        wire::Paint::Stroke { width } => icon::Paint::Stroke { width },
    }
}

fn verb_to_wire(verb: icon::Verb) -> wire::Verb {
    match verb {
        icon::Verb::Move(x, y) => wire::Verb::Move(x, y),
        icon::Verb::Line(x, y) => wire::Verb::Line(x, y),
        icon::Verb::Quad(a, b, x, y) => wire::Verb::Quad(a, b, x, y),
        icon::Verb::Cubic(a, b, c, d, x, y) => wire::Verb::Cubic(a, b, c, d, x, y),
        icon::Verb::Close => wire::Verb::Close,
    }
}

fn verb_from_wire(verb: wire::Verb) -> icon::Verb {
    match verb {
        wire::Verb::Move(x, y) => icon::Verb::Move(x, y),
        wire::Verb::Line(x, y) => icon::Verb::Line(x, y),
        wire::Verb::Quad(a, b, x, y) => icon::Verb::Quad(a, b, x, y),
        wire::Verb::Cubic(a, b, c, d, x, y) => icon::Verb::Cubic(a, b, c, d, x, y),
        wire::Verb::Close => icon::Verb::Close,
    }
}

/// The source a resource describes — `None` for a symbol this build does
/// not know by name: an app's own, which a host sends by name just before
/// sending it whole (and a host from before it sent drawings, always).
pub fn image_source(resource: &Resource) -> Option<ImageSource> {
    let key = resource.key;
    Some(match &resource.kind {
        ResourceKind::Rgba { size, rgba } => ImageSource::Rgba { key, size: *size, rgba: Rc::from(rgba.as_slice()) },
        ResourceKind::Encoded { bytes } => ImageSource::Bytes { key, bytes: Rc::from(bytes.as_slice()) },
        ResourceKind::Vector { verbs, paint, ink, box_size } => ImageSource::Path {
            key,
            verbs: verbs.iter().map(|verb| verb_from_wire(*verb)).collect(),
            paint: paint_from_wire(*paint),
            ink: match *ink {
                wire::Ink::Solid(color) => icon::Ink::Solid(color.into()),
                wire::Ink::Ramp(gradient) => icon::Ink::Ramp(gradient.into()),
            },
            box_size: *box_size,
        },
        ResourceKind::Symbol { name, color, forced } => ImageSource::Symbol {
            key,
            symbol: icon::house::named(name)?,
            color: (*color).into(),
            forced: *forced,
            turn: 0.0,
        },
        ResourceKind::Glyph { name, color, forced, draws } => ImageSource::Symbol {
            key,
            symbol: kept(key, name, draws),
            color: (*color).into(),
            forced: *forced,
            turn: 0.0,
        },
        ResourceKind::FileIcon { path } => ImageSource::FileIcon { key, path: Rc::from(path.as_str()) },
        ResourceKind::Feed { generation, size, rgba } => ImageSource::Feed {
            key,
            generation: *generation,
            size: *size,
            format: PixelFormat::Rgba8,
            bytes: Rc::from(rgba.as_slice()),
        },
    })
}

thread_local! {
    /// The glyphs this tool was sent, by key, as the house holds them.
    static KEPT: RefCell<HashMap<u64, icon::Symbol>> = RefCell::new(HashMap::new());
}

/// The symbol a glyph sent under `key` draws as: the one kept the first
/// time the key was raised — a tool raises its resources for every
/// picture it paints, and must not leak a drawing per frame. The key is
/// the drawing's identity on the wire, so it never stands for another.
fn kept(key: u64, name: &str, draws: &[wire::GlyphDraw]) -> icon::Symbol {
    KEPT.with(|kept| *kept.borrow_mut().entry(key).or_insert_with(|| leaked(name, draws)))
}

/// A glyph in the `&'static` tables the house rasterizes, leaked — the
/// way `Symbol::from_svg` holds a drawing it reads at runtime.
fn leaked(name: &str, draws: &[wire::GlyphDraw]) -> icon::Symbol {
    let draws: Vec<icon::Draw> = draws
        .iter()
        .map(|draw| icon::Draw {
            paint: paint_from_wire(draw.paint),
            path: Vec::leak(draw.verbs.iter().map(|verb| verb_from_wire(*verb)).collect()),
            tint: draw.tint.map(Into::into),
        })
        .collect();
    let glyph: &'static icon::Glyph = Box::leak(Box::new(icon::Glyph { draws: Vec::leak(draws) }));
    icon::Symbol::new(String::leak(name.to_owned()), glyph)
}

/// `inner` wrapped the way `image` says: mirrored, then faded, with
/// keys derived from the inner one.
pub fn wrap(inner: ImageSource, image: ImageRef) -> ImageSource {
    let key = image_key(&inner);
    let mut source = inner;
    if image.mirrored {
        source = ImageSource::Mirrored { key: key.rotate_left(17) ^ 0x4d49_5252, inner: Rc::new(source) };
    }
    if image.alpha != 255 {
        source = ImageSource::Faded {
            key: key.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(image.alpha),
            inner: Rc::new(source),
            alpha: image.alpha,
        };
    }
    source
}

// =============================================================================
// Draws
// =============================================================================

/// What a frame could not be raised without.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// An image key the tool holds no source for.
    Image { key: u64 },
    /// A string index past the frame's table.
    String { index: u32 },
}

impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Missing::Image { key } => write!(f, "no resource for the image {key:#x}"),
            Missing::String { index } => write!(f, "the frame names string {index}, past its table"),
        }
    }
}

impl std::error::Error for Missing {}

fn string(strings: &[String], index: u32) -> Result<&str, Missing> {
    strings.get(index as usize).map(String::as_str).ok_or(Missing::String { index })
}

/// The turned lines of a list as the pictures they paint, for the wire,
/// which is frozen and has no turned line of its own.
///
/// The host's engine shapes each line, the core turns its raster the way
/// every pipeline does ([`bunny_ui_core::raster::turn_raster`]) and
/// places it at the pixel the pipelines put it on, `scale` pixels to the
/// point. A tool that draws the frame at that scale paints the host's
/// bytes; at another it resamples a picture, as it does any image. A list
/// with no turned line comes back as it went in.
pub fn turned_as_pictures(display: DisplayList, text: &dyn TextEngine, scale: usize) -> DisplayList {
    if !display.iter().any(|command| matches!(command, DrawCommand::TurnedText { .. })) {
        return display;
    }
    let commands = display
        .iter()
        .filter_map(|command| {
            let DrawCommand::TurnedText { pivot, content, range, color, font, turn } = command else {
                return Some(command.clone());
            };
            let slice = content.get(range.0..range.1)?;
            let upright = text.raster_line(slice, font, *color, scale)?;
            let turned = bunny_ui_core::raster::turn_raster(&upright, *turn);
            let (x, y) = bunny_ui_core::raster::turned_corner(
                *pivot,
                (upright.width, upright.height, upright.baseline),
                *turn,
                scale,
            );
            let mut fold = Fnv(0xcbf2_9ce4_8422_2325);
            fold.write(slice.as_bytes());
            fold.write(format!("{font:?}{color:?}{turn:?}{scale}").as_bytes());
            let factor = scale as f64;
            let rect = layout::Rect {
                origin: layout::Point { x: x as f64 / factor, y: y as f64 / factor },
                size: layout::Size { width: turned.width as f64 / factor, height: turned.height as f64 / factor },
            };
            let size = (turned.width as u32, turned.height as u32);
            Some(DrawCommand::Image {
                rect,
                source: ImageSource::rgba(fold.finish(), size, turned.rgba),
                placement: ImagePlacement::Pixel,
            })
        })
        .collect::<Vec<_>>();
    DisplayList::from(commands)
}

/// A display list as draws, its strings interned, and the distinct
/// innermost images it needs, in first-use order.
pub fn lower(display: &DisplayList, strings: &mut Strings) -> (Vec<Draw>, Vec<ImageSource>) {
    let mut images: Vec<ImageSource> = Vec::new();
    let mut seen: HashMap<u64, ()> = HashMap::new();
    let commands = display
        .iter()
        .map(|command| match command {
            DrawCommand::FillRect { rect, color, corner_radius } => {
                Draw::FillRect { rect: (*rect).into(), color: (*color).into(), corners: (*corner_radius).into() }
            }
            DrawCommand::Gradient { rect, paint, corner_radius } => {
                Draw::Gradient { rect: (*rect).into(), paint: (*paint).into(), corners: (*corner_radius).into() }
            }
            DrawCommand::Backdrop { rect, glass, corner_radius } => {
                Draw::Backdrop { rect: (*rect).into(), glass: (*glass).into(), corners: (*corner_radius).into() }
            }
            DrawCommand::Shadow { rect, radius, color, corner_radius } => Draw::Shadow {
                rect: (*rect).into(),
                radius: *radius,
                color: (*color).into(),
                corners: (*corner_radius).into(),
            },
            DrawCommand::StrokeRect { rect, color, width, corner_radius } => Draw::StrokeRect {
                rect: (*rect).into(),
                color: (*color).into(),
                width: *width,
                corners: (*corner_radius).into(),
            },
            DrawCommand::TextLine { origin, content, range, color, font: spec } => Draw::TextLine {
                origin: (*origin).into(),
                text: strings.intern(content),
                range: (range.0 as u32, range.1 as u32),
                color: (*color).into(),
                font: font(spec, strings),
            },
            // the frozen wire has no turned line: [`turned_as_pictures`]
            // makes each one the picture it paints before the list comes
            // here, and a list that skipped it shows the line upright,
            // starting where it would have started
            DrawCommand::TurnedText { pivot, content, range, color, font: spec, .. } => Draw::TextLine {
                origin: (*pivot).into(),
                text: strings.intern(content),
                range: (range.0 as u32, range.1 as u32),
                color: (*color).into(),
                font: font(spec, strings),
            },
            DrawCommand::Image { rect, source, placement } => {
                let (image, inner) = image_ref(source);
                if seen.insert(image.key, ()).is_none() {
                    images.push(inner.into_owned());
                }
                Draw::Image { rect: (*rect).into(), image, subpixel: *placement == ImagePlacement::Subpixel }
            }
            DrawCommand::PushClip { rect, corner_radius } => {
                Draw::PushClip { rect: (*rect).into(), corners: (*corner_radius).into() }
            }
            DrawCommand::PopClip => Draw::PopClip,
        })
        .collect();
    (commands, images)
}

/// Draws back into a display list, the images resolved by key through
/// what the tool holds.
pub fn raise(commands: &[Draw], strings: &[String], images: &HashMap<u64, ImageSource>) -> Result<DisplayList, Missing> {
    let mut out = Vec::with_capacity(commands.len());
    for command in commands {
        out.push(match command {
            Draw::FillRect { rect, color, corners } => {
                DrawCommand::FillRect { rect: (*rect).into(), color: (*color).into(), corner_radius: (*corners).into() }
            }
            Draw::Gradient { rect, paint, corners } => {
                DrawCommand::Gradient { rect: (*rect).into(), paint: (*paint).into(), corner_radius: (*corners).into() }
            }
            Draw::Backdrop { rect, glass, corners } => {
                DrawCommand::Backdrop { rect: (*rect).into(), glass: (*glass).into(), corner_radius: (*corners).into() }
            }
            Draw::Shadow { rect, radius, color, corners } => DrawCommand::Shadow {
                rect: (*rect).into(),
                radius: *radius,
                color: (*color).into(),
                corner_radius: (*corners).into(),
            },
            Draw::StrokeRect { rect, color, width, corners } => DrawCommand::StrokeRect {
                rect: (*rect).into(),
                color: (*color).into(),
                width: *width,
                corner_radius: (*corners).into(),
            },
            Draw::TextLine { origin, text, range, color, font } => DrawCommand::TextLine {
                origin: (*origin).into(),
                content: Arc::from(string(strings, *text)?),
                range: (range.0 as usize, range.1 as usize),
                color: (*color).into(),
                font: font_spec(font, strings)?,
            },
            Draw::Image { rect, image, subpixel } => {
                let inner = images.get(&image.key).ok_or(Missing::Image { key: image.key })?;
                DrawCommand::Image {
                    rect: (*rect).into(),
                    source: wrap(inner.clone(), *image),
                    placement: if *subpixel { ImagePlacement::Subpixel } else { ImagePlacement::Pixel },
                }
            }
            Draw::PushClip { rect, corners } => DrawCommand::PushClip { rect: (*rect).into(), corner_radius: (*corners).into() },
            Draw::PopClip => DrawCommand::PopClip,
        });
    }
    Ok(DisplayList::from(out))
}

// =============================================================================
// Sites
// =============================================================================

/// A node's placement as the wire carries it.
pub fn site_box(placement: &SitePlacement, strings: &mut Strings) -> wire::SiteBox {
    let (file, name, line, column) = match placement.site {
        Site::Caller(location) => (Some(strings.intern(location.file())), None, location.line(), location.column()),
        Site::Named(named) => (None, Some(strings.intern(named)), 0, 0),
    };
    wire::SiteBox {
        index: placement.index,
        parent: placement.parent,
        kind: strings.intern(placement.kind.name()),
        modifier: placement.modifier.map(|modifier| strings.intern(modifier)),
        file,
        name,
        line,
        column,
        frame: placement.frame.into(),
        path: placement.path.as_deref().map(|path| strings.intern(path)),
    }
}

// =============================================================================
// Traits
// =============================================================================

/// The framework's traits on the wire. A theme travels as its tokens by name.
pub fn traits(traits: &core_preview::Traits) -> wire::Traits {
    wire::Traits {
        scheme: match traits.scheme {
            core_preview::Scheme::Light => wire::Scheme::Light,
            core_preview::Scheme::Dark => wire::Scheme::Dark,
            core_preview::Scheme::Both => wire::Scheme::Both,
        },
        theme: traits
            .theme
            .map(|theme| theme.tokens().into_iter().map(|(name, color)| (name.to_string(), color.into())).collect()),
        size: match traits.size {
            core_preview::Sizing::Fixed(size) => wire::Sizing::Fixed(size.into()),
            core_preview::Sizing::Fits => wire::Sizing::Fits,
            core_preview::Sizing::Device(device) => wire::Sizing::Device(device_to_wire(device)),
        },
        locale: traits.locale.as_ref().map(|locale| locale.as_str().to_string()),
        direction: traits.direction.map(|direction| match direction {
            LayoutDirection::LeftToRight => wire::Direction::LeftToRight,
            LayoutDirection::RightToLeft => wire::Direction::RightToLeft,
        }),
        size_class: traits.size_class.map(|class| match class {
            SizeClass::Compact => wire::SizeClass::Compact,
            SizeClass::Regular => wire::SizeClass::Regular,
        }),
        scale: traits.scale,
        safe_area: traits.safe_area.map(Into::into),
    }
}

/// The wire's traits as the framework's. A token the theme does not
/// have is left out; the rest start from the scheme's theme.
pub fn core_traits(traits: &wire::Traits) -> core_preview::Traits {
    let scheme = match traits.scheme {
        wire::Scheme::Light => core_preview::Scheme::Light,
        wire::Scheme::Dark => core_preview::Scheme::Dark,
        wire::Scheme::Both => core_preview::Scheme::Both,
    };
    let theme = traits.theme.as_ref().map(|tokens| {
        let mut theme = match scheme {
            core_preview::Scheme::Dark => Theme::dark(),
            core_preview::Scheme::Light | core_preview::Scheme::Both => Theme::light(),
        };
        for (name, color) in tokens {
            theme.set_token(name, (*color).into());
        }
        theme
    });
    core_preview::Traits {
        scheme,
        theme,
        size: match traits.size {
            wire::Sizing::Fixed(size) => core_preview::Sizing::Fixed(size.into()),
            wire::Sizing::Fits => core_preview::Sizing::Fits,
            wire::Sizing::Device(device) => core_preview::Sizing::Device(device_from_wire(device)),
        },
        locale: traits.locale.as_deref().map(Locale::new),
        direction: traits.direction.map(|direction| match direction {
            wire::Direction::LeftToRight => LayoutDirection::LeftToRight,
            wire::Direction::RightToLeft => LayoutDirection::RightToLeft,
        }),
        size_class: traits.size_class.map(|class| match class {
            wire::SizeClass::Compact => SizeClass::Compact,
            wire::SizeClass::Regular => SizeClass::Regular,
        }),
        scale: traits.scale,
        safe_area: traits.safe_area.map(Into::into),
    }
}

fn device_to_wire(device: core_preview::Device) -> wire::Device {
    match device {
        core_preview::Device::IPhone16 => wire::Device::IPhone16,
        core_preview::Device::IPhone16ProMax => wire::Device::IPhone16ProMax,
        core_preview::Device::IPadPro11 => wire::Device::IPadPro11,
        core_preview::Device::Pixel9 => wire::Device::Pixel9,
        core_preview::Device::MacWindow => wire::Device::MacWindow,
    }
}

fn device_from_wire(device: wire::Device) -> core_preview::Device {
    match device {
        wire::Device::IPhone16 => core_preview::Device::IPhone16,
        wire::Device::IPhone16ProMax => core_preview::Device::IPhone16ProMax,
        wire::Device::IPadPro11 => core_preview::Device::IPadPro11,
        wire::Device::Pixel9 => core_preview::Device::Pixel9,
        wire::Device::MacWindow => core_preview::Device::MacWindow,
    }
}

// =============================================================================
// Patches
// =============================================================================

/// The modifier an additive placement patch stands for.
pub fn place_modifier(patch: wire::PlacePatch) -> Modifier {
    match patch {
        wire::PlacePatch::Position(point) => Modifier::Position(point.into()),
        wire::PlacePatch::Span(columns, rows) => Modifier::Span(usize::from(columns.max(1)), usize::from(rows.max(1))),
    }
}

/// The modifier a patch stands for.
pub fn modifier(patch: wire::Patch) -> Modifier {
    match patch {
        wire::Patch::Padding => Modifier::Padding,
        wire::Patch::PaddingLength(length) => Modifier::PaddingLength(length),
        wire::Patch::PaddingEdge(edge, length) => Modifier::PaddingEdge(edge_from_wire(edge), length),
        wire::Patch::Frame { width, height, alignment } => Modifier::FrameWH(width, height, alignment_from_wire(alignment)),
        wire::Patch::FrameMax { width, height, alignment } => Modifier::FrameMax(width, height, alignment_from_wire(alignment)),
        wire::Patch::FrameWidth(width, alignment) => Modifier::FrameWidth(width, alignment_from_wire(alignment)),
        wire::Patch::FrameHeight(height) => Modifier::FrameHeight(height),
        wire::Patch::BackgroundColor(color) => Modifier::BackgroundColor(color.into()),
        wire::Patch::ForegroundColor(color) => Modifier::ForegroundColor(color.into()),
        wire::Patch::CornerRadius(corners) => Modifier::CornerRadius(corners.into()),
        wire::Patch::FontSize(size) => Modifier::FontSize(size),
        wire::Patch::Opacity(opacity) => Modifier::Opacity(opacity),
        wire::Patch::Border(color, width) => Modifier::Border(color.into(), width),
        wire::Patch::Shadow(radius, color) => Modifier::Shadow(radius, color.into()),
        wire::Patch::Bold => Modifier::Bold,
        wire::Patch::Italic => Modifier::Italic,
        wire::Patch::Hidden => Modifier::Hidden,
        wire::Patch::Blur(radius) => Modifier::Blur(radius),
        wire::Patch::LineHeight(height) => Modifier::LineHeight(height),
        wire::Patch::FontWeight(weight) => Modifier::FontWeight(weight.into()),
        wire::Patch::Monospaced => Modifier::Monospaced,
        wire::Patch::Clipped => Modifier::Clipped,
        wire::Patch::HugHeight => Modifier::HugHeight,
    }
}

fn edge_from_wire(edge: wire::Edge) -> Edge {
    match edge {
        wire::Edge::Top => Edge::Top,
        wire::Edge::Bottom => Edge::Bottom,
        wire::Edge::Leading => Edge::Leading,
        wire::Edge::Trailing => Edge::Trailing,
    }
}

fn alignment_from_wire(alignment: wire::Alignment) -> Alignment {
    match alignment {
        wire::Alignment::Leading => Alignment::Leading,
        wire::Alignment::Center => Alignment::Center,
        wire::Alignment::Trailing => Alignment::Trailing,
        wire::Alignment::TopLeading => Alignment::TopLeading,
        wire::Alignment::Top => Alignment::Top,
        wire::Alignment::TopTrailing => Alignment::TopTrailing,
        wire::Alignment::BottomLeading => Alignment::BottomLeading,
        wire::Alignment::Bottom => Alignment::Bottom,
        wire::Alignment::BottomTrailing => Alignment::BottomTrailing,
    }
}
