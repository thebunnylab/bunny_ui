//! The framework's own types to the wire's and back — what a host does
//! to a frame before sending it, and what a tool does to a frame before
//! painting it with the framework's own painter.
//!
//! Geometry and colour convert one to one. Text travels as an index into
//! the frame's string table, a font's family as its name. An image is
//! named by the key of its innermost source, with the fades and flips
//! the app wrapped it in folded into an [`ImageRef`]; the bytes behind a
//! key are a [`Resource`] the host sends once. A site's `Location`
//! becomes a file, a line and a column; a site the framework named
//! itself keeps its name. A theme travels as its tokens by name.
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

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use bunny_ui_core::icon;
use bunny_ui_core::image_engine::{ImageSource, PixelFormat};
use bunny_ui_core::layout::{self, DisplayList, DrawCommand};
use bunny_ui_core::modifier::Modifier;
use bunny_ui_core::prelude::{Alignment, Edge, LayoutDirection, Locale, Site, SizeClass};
use bunny_ui_core::preview as core_preview;
use bunny_ui_core::provenance::SitePlacement;
use bunny_ui_core::text_engine::{Family, FontDesign, FontSpec, Slant, Weight};
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
        }
    }
}

// =============================================================================
// Fonts
// =============================================================================

impl From<Weight> for wire::Weight {
    fn from(w: Weight) -> Self {
        match w {
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
/// fades and flips folded in — and that innermost source.
pub fn image_ref(source: &ImageSource) -> (ImageRef, &ImageSource) {
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
    (ImageRef { key: image_key(at), alpha: alpha as u8, mirrored }, at)
}

/// The key of a source.
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
        | ImageSource::Mirrored { key, .. } => *key,
    }
}

/// The resource behind a source, for the wire — `None` for a platform
/// surface, which has no bytes to send. A wrapped source answers its
/// innermost one's.
pub fn resource(source: &ImageSource) -> Option<Resource> {
    let (_, inner) = image_ref(source);
    let kind = match inner {
        ImageSource::Bytes { bytes, .. } => ResourceKind::Encoded { bytes: bytes.to_vec() },
        ImageSource::FileIcon { path, .. } => ResourceKind::FileIcon { path: path.to_string() },
        ImageSource::Symbol { symbol, color, forced, .. } => {
            ResourceKind::Symbol { name: symbol.name.to_string(), color: (*color).into(), forced: *forced }
        }
        ImageSource::Path { verbs, paint, ink, box_size, .. } => ResourceKind::Vector {
            verbs: verbs.iter().map(|verb| verb_to_wire(*verb)).collect(),
            paint: match *paint {
                icon::Paint::Fill(icon::Rule::NonZero) => wire::Paint::Fill { even_odd: false },
                icon::Paint::Fill(icon::Rule::EvenOdd) => wire::Paint::Fill { even_odd: true },
                icon::Paint::Stroke { width } => wire::Paint::Stroke { width },
            },
            ink: match *ink {
                icon::Ink::Solid(color) => wire::Ink::Solid(color.into()),
                icon::Ink::Ramp(gradient) => wire::Ink::Ramp(gradient.into()),
            },
            box_size: *box_size,
        },
        ImageSource::Rgba { size, rgba, .. } => ResourceKind::Rgba { size: *size, rgba: rgba.to_vec() },
        ImageSource::Feed { generation, size, bytes, .. } => {
            ResourceKind::Feed { generation: *generation, size: *size, rgba: bytes.to_vec() }
        }
        ImageSource::Native { .. } | ImageSource::Faded { .. } | ImageSource::Mirrored { .. } => return None,
    };
    Some(Resource { key: image_key(inner), kind })
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

/// The source a resource describes — `None` for a house symbol this
/// build does not know by name.
pub fn image_source(resource: &Resource) -> Option<ImageSource> {
    let key = resource.key;
    Some(match &resource.kind {
        ResourceKind::Rgba { size, rgba } => ImageSource::Rgba { key, size: *size, rgba: Rc::from(rgba.as_slice()) },
        ResourceKind::Encoded { bytes } => ImageSource::Bytes { key, bytes: Rc::from(bytes.as_slice()) },
        ResourceKind::Vector { verbs, paint, ink, box_size } => ImageSource::Path {
            key,
            verbs: verbs.iter().map(|verb| verb_from_wire(*verb)).collect(),
            paint: match *paint {
                wire::Paint::Fill { even_odd: false } => icon::Paint::Fill(icon::Rule::NonZero),
                wire::Paint::Fill { even_odd: true } => icon::Paint::Fill(icon::Rule::EvenOdd),
                wire::Paint::Stroke { width } => icon::Paint::Stroke { width },
            },
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
            DrawCommand::Image { rect, source } => {
                let (image, inner) = image_ref(source);
                if seen.insert(image.key, ()).is_none() {
                    images.push(inner.clone());
                }
                Draw::Image { rect: (*rect).into(), image }
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
            Draw::Image { rect, image } => {
                let inner = images.get(&image.key).ok_or(Missing::Image { key: image.key })?;
                DrawCommand::Image { rect: (*rect).into(), source: wrap(inner.clone(), *image) }
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
    }
}
