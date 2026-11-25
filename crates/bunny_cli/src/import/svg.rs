//! A page's inline drawings as bunny symbols.
//!
//! This is bunny-ui's SVG door, copied: the parser of `icon/parse.rs`,
//! the flattening half of `icon/vector.rs` its dashes walk, and the
//! parts of the icon model (`Verb`, `Rule`, `Paint`) the parser writes.
//! The importer stands on the standard library alone and bunny-ui keeps
//! its door behind the `svg` feature, so the parser lives here twice; it
//! keeps bunny-ui's order and names, so the two can be diffed and kept
//! in step.
//!
//! What a page needs that an icon file does not, and changed here:
//!
//! - a colour is any the browser writes (`rgb(…)`, `rgba(…)`) as well
//!   as hex, and black is black. The browser has already resolved
//!   `currentColor` to the ink it stands for, so a draw with no colour
//!   of its own is the SVG default, black, not the symbol's ink;
//! - `opacity`, `fill-opacity` and `stroke-opacity` become the alpha of
//!   the draw's colour;
//! - a fill with no area (the default fill under an open stroke) draws
//!   nothing and is left out;
//! - a drawing with no `viewBox` is drawn on its `width` and `height`,
//!   as a browser draws it;
//! - `<title>` and `<desc>` are read past;
//! - a drawing with nothing to draw, or one that is not square, is
//!   refused: a symbol is a square, and the importer keeps the picture.
//!
//! The paint is written as `bunny_ui::icon` const source, the shape
//! bunny-ui's `svg2icon` prints.

use std::fmt::Write as _;

use super::css::{self, Rgba};

// MARK: - The icon model (`icon/mod.rs`)

/// The glyph grid: every drawing lives in a `0..24` square.
pub const ICON_GRID: f32 = 24.0;

/// One drawing instruction, in grid units.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Verb {
    /// Start a new contour at this point.
    Move(f32, f32),
    /// A straight segment to this point.
    Line(f32, f32),
    /// A quadratic curve: control, then end.
    Quad(f32, f32, f32, f32),
    /// A cubic curve: two controls, then end.
    Cubic(f32, f32, f32, f32, f32, f32),
    /// Seal the contour back to its start.
    Close,
}

/// Which side of a self-crossing contour counts as inside.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    /// Winding count ≠ 0, SVG's default.
    NonZero,
    /// Odd crossing count, the rule that cuts holes.
    EvenOdd,
}

/// How a contour becomes ink. A dash pattern is resolved here, into
/// strokes of its own, so the parser writes no `Paint::Dashed`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Paint {
    /// Cover the inside.
    Fill(Rule),
    /// Ride the contour with a round pen this wide (grid units).
    Stroke { width: f32 },
}

/// A refusal, pointed at its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for SvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for SvgError {}

/// A parsed drawing: its draws in paint order, each with its own colour
/// (`None` is the ink, which only a `currentColor` the browser left
/// unresolved asks for).
#[derive(Debug)]
pub struct ParsedGlyph {
    pub draws: Vec<(Paint, Vec<Verb>, Option<Rgba>)>,
    /// What the pen draws only nearly: a square cap drawn round, a
    /// colour it could not read. A clean drawing has none.
    pub warnings: Vec<String>,
}

impl ParsedGlyph {
    /// The one colour every draw paints in, taken out of the draws: the
    /// view paints it with `.foreground_color`, and the glyph takes a
    /// new ink like a word does. `None`, and the draws untouched, when
    /// they paint in several.
    pub fn take_ink(&mut self) -> Option<Rgba> {
        let first = self.draws.first()?.2?;
        if !self.draws.iter().all(|(_, _, tint)| *tint == Some(first)) {
            return None;
        }
        for draw in &mut self.draws {
            draw.2 = None;
        }
        Some(first)
    }
}

/// The name a drawing's symbol is cached by: its markup, hashed, so two
/// modules that draw the same thing share it and two that draw different
/// things never do.
#[must_use]
pub fn symbol_name(markup: &str) -> String {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let hash = markup.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME));
    format!("drawing.{hash:016x}")
}

/// The drawing as Rust const source: the glyph, then the symbol that
/// names it. The model is named by its full path, as bunny-ui's prelude
/// holds `Symbol`, `Paint`, `Rule`, `Verb` and `Color` but not the glyph
/// and its draws.
#[must_use]
pub fn to_rust_const(const_name: &str, name: &str, parsed: &ParsedGlyph) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "const {const_name}_GLYPH: bunny_ui::icon::Glyph = bunny_ui::icon::Glyph {{");
    let _ = writeln!(out, "    draws: &[");
    for (paint, path, tint) in &parsed.draws {
        let paint_source = match paint {
            Paint::Fill(Rule::NonZero) => "Paint::Fill(Rule::NonZero)".to_owned(),
            Paint::Fill(Rule::EvenOdd) => "Paint::Fill(Rule::EvenOdd)".to_owned(),
            Paint::Stroke { width } => format!("Paint::Stroke {{ width: {} }}", unit(*width)),
        };
        let tint_source = tint.map_or_else(|| "None".to_owned(), |color| format!("Some({})", color.code()));
        let _ = writeln!(out, "        bunny_ui::icon::Draw {{");
        let _ = writeln!(out, "            paint: {paint_source},");
        let _ = writeln!(out, "            tint: {tint_source},");
        let _ = writeln!(out, "            path: &[");
        for verb in path {
            let verb_source = match *verb {
                Verb::Move(x, y) => format!("Move({}, {})", unit(x), unit(y)),
                Verb::Line(x, y) => format!("Line({}, {})", unit(x), unit(y)),
                Verb::Quad(cx, cy, x, y) => format!("Quad({}, {}, {}, {})", unit(cx), unit(cy), unit(x), unit(y)),
                Verb::Cubic(ax, ay, bx, by, x, y) => {
                    format!("Cubic({}, {}, {}, {}, {}, {})", unit(ax), unit(ay), unit(bx), unit(by), unit(x), unit(y))
                }
                Verb::Close => "Close".to_owned(),
            };
            let _ = writeln!(out, "                Verb::{verb_source},");
        }
        let _ = writeln!(out, "            ],");
        let _ = writeln!(out, "        }},");
    }
    let _ = writeln!(out, "    ],");
    let _ = writeln!(out, "}};");
    let _ = writeln!(out);
    let _ = writeln!(out, "const {const_name}: Symbol = Symbol::new({name:?}, &{const_name}_GLYPH);");
    out
}

/// A grid coordinate as an `f32` literal, to the thousandth: finer than
/// any pixel a glyph is drawn at, and short enough to read.
fn unit(value: f32) -> String {
    let rounded = (value * 1000.0).round() / 1000.0;
    // `-0.0` reads as a mistake
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    format!("{rounded:?}")
}

// MARK: - The tag walk (`icon/parse.rs`)

/// What an element inherits from its ancestors: the paint state.
#[derive(Clone)]
struct Inherited {
    fill: Option<bool>,   // Some(true) = paint, Some(false) = none
    stroke: Option<bool>, // same
    /// The colour of each paint. A page's drawing is black until it
    /// says otherwise.
    fill_tint: Option<Rgba>,
    stroke_tint: Option<Rgba>,
    /// `fill-opacity` and `stroke-opacity`, inherited as they are.
    fill_opacity: f64,
    stroke_opacity: f64,
    /// `opacity` of the element and every ancestor, multiplied: drawn
    /// as each shape's alpha, which is the group's own wherever its
    /// shapes do not overlap.
    opacity: f64,
    stroke_width: f32,
    even_odd: bool,
    /// The composed affine of every ancestor, flattened here.
    transform: [f64; 6],
    /// `stroke-dasharray`: the contour flattens and the dashes become
    /// their own little strokes.
    dash: Option<Vec<f32>>,
}

/// SVG's black, where a drawing names no fill.
const BLACK: Rgba = Rgba { r: 0, g: 0, b: 0, a: 1.0 };

impl Default for Inherited {
    fn default() -> Self {
        // SVG law: fill black, stroke none, until somebody says
        Self {
            fill: Some(true),
            stroke: Some(false),
            fill_tint: Some(BLACK),
            stroke_tint: Some(BLACK),
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            opacity: 1.0,
            stroke_width: 1.0,
            even_odd: false,
            transform: IDENTITY,
            dash: None,
        }
    }
}

/// Column-major 2×3: `[a, b, c, d, e, f]` maps `(x, y)` to
/// `(a·x + c·y + e, b·x + d·y + f)`, SVG's own matrix order.
const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn compose(outer: [f64; 6], inner: [f64; 6]) -> [f64; 6] {
    [
        outer[0] * inner[0] + outer[2] * inner[1],
        outer[1] * inner[0] + outer[3] * inner[1],
        outer[0] * inner[2] + outer[2] * inner[3],
        outer[1] * inner[2] + outer[3] * inner[3],
        outer[0] * inner[4] + outer[2] * inner[5] + outer[4],
        outer[1] * inner[4] + outer[3] * inner[5] + outer[5],
    ]
}

#[allow(clippy::cast_possible_truncation, reason = "a grid point is an f32 by the model")]
fn apply(matrix: [f64; 6], x: f32, y: f32) -> (f32, f32) {
    let (x, y) = (f64::from(x), f64::from(y));
    (
        (matrix[0] * x + matrix[2] * y + matrix[4]) as f32,
        (matrix[1] * x + matrix[3] * y + matrix[5]) as f32,
    )
}

/// `matrix(…)`, `translate(…)`, `scale(…)`, `rotate(deg [cx cy])`: a
/// list of them composes left to right, SVG law.
fn parse_transform(value: &str, line: usize) -> Result<[f64; 6], SvgError> {
    let mut matrix = IDENTITY;
    let mut rest = value.trim();
    while !rest.is_empty() {
        let open = rest.find('(').ok_or_else(|| SvgError { line, message: "The transform does not parse.".into() })?;
        let close = rest.find(')').ok_or_else(|| SvgError { line, message: "The transform does not parse.".into() })?;
        let name = rest[..open].trim();
        let numbers: Vec<f64> = rest[open + 1..close].split([' ', ',']).filter_map(|n| n.trim().parse().ok()).collect();
        let own = match (name, numbers.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => [*a, *b, *c, *d, *e, *f],
            ("translate", [x]) => [1.0, 0.0, 0.0, 1.0, *x, 0.0],
            ("translate", [x, y]) => [1.0, 0.0, 0.0, 1.0, *x, *y],
            ("scale", [s]) => [*s, 0.0, 0.0, *s, 0.0, 0.0],
            ("scale", [x, y]) => [*x, 0.0, 0.0, *y, 0.0, 0.0],
            ("rotate", [degrees]) => rotation(*degrees, 0.0, 0.0),
            ("rotate", [degrees, cx, cy]) => rotation(*degrees, *cx, *cy),
            _ => {
                return Err(SvgError { line, message: format!("The transform {name} is not supported.") });
            }
        };
        matrix = compose(matrix, own);
        rest = rest[close + 1..].trim_start_matches([' ', ',']).trim();
    }
    Ok(matrix)
}

fn rotation(degrees: f64, cx: f64, cy: f64) -> [f64; 6] {
    let radians = degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    // translate(cx cy) · rotate · translate(-cx -cy), composed flat
    [cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy]
}

/// The measured refusals, each with its instruction.
const REFUSED_ELEMENTS: &[(&str, &str)] = &[
    ("defs", "The element <defs> is not supported. Flatten the file before you convert it."),
    ("linearGradient", "The element <linearGradient> is not supported. A glyph has one ink."),
    ("radialGradient", "The element <radialGradient> is not supported. A glyph has one ink."),
    ("mask", "The element <mask> is not supported. Flatten the file before you convert it."),
    ("clipPath", "The element <clipPath> is not supported. Flatten the file before you convert it."),
    ("use", "The element <use> is not supported. Inline the referenced shape."),
    ("text", "The element <text> is not supported. Convert the text to a path."),
    ("style", "The element <style> is not supported. Use presentation attributes."),
    ("image", "The element <image> is not supported. A glyph is vector only."),
];

/// Elements that say what a drawing is and draw nothing.
const READ_PAST: &[&str] = &["title", "desc", "metadata"];

/// Reads one drawing.
///
/// # Errors
///
/// What the door refuses, with its line: an element, a command or a
/// transform it does not draw, path data that does not parse, a drawing
/// with nothing to draw or one that is not square.
#[allow(clippy::too_many_lines, reason = "one walk over the tags, as in bunny-ui")]
pub fn parse(svg: &str) -> Result<ParsedGlyph, SvgError> {
    let mut out = ParsedGlyph { draws: Vec::new(), warnings: Vec::new() };
    let mut stack: Vec<Inherited> = vec![Inherited::default()];
    let mut view_box: Option<(f32, f32, f32, f32)> = None;
    let mut size: (Option<f32>, Option<f32>) = (None, None);
    let mut skipping: Option<&str> = None;
    let mut at = 0usize;
    let bytes = svg.as_bytes();
    let line_of = |at: usize| 1 + svg[..at].bytes().filter(|b| *b == b'\n').count();

    while at < bytes.len() {
        let Some(open) = svg[at..].find('<') else { break };
        let start = at + open;
        if svg[start..].starts_with("<!--") {
            let end = svg[start..].find("-->").ok_or_else(|| SvgError { line: line_of(start), message: "The comment does not end.".into() })?;
            at = start + end + 3;
            continue;
        }
        if svg[start..].starts_with("<?") || svg[start..].starts_with("<!") {
            let end = svg[start..].find('>').unwrap_or(svg.len() - start);
            at = start + end + 1;
            continue;
        }
        let end = svg[start..].find('>').ok_or_else(|| SvgError { line: line_of(start), message: "The tag does not end.".into() })?;
        let tag = &svg[start + 1..start + end];
        at = start + end + 1;
        let line = line_of(start);

        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim();
            if skipping == Some(name) {
                skipping = None;
            } else if skipping.is_none() && matches!(name, "g" | "svg") && stack.len() > 1 {
                // a closing tag pops what its opener pushed
                stack.pop();
            }
            continue;
        }
        if skipping.is_some() {
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, rest) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        if let Some(read_past) = READ_PAST.iter().find(|read_past| **read_past == name) {
            if !self_closing {
                skipping = Some(read_past);
            }
            continue;
        }
        let attributes = parse_attributes(rest, line)?;

        for (refused, message) in REFUSED_ELEMENTS {
            if name == *refused {
                return Err(SvgError { line, message: (*message).into() });
            }
        }
        let mut inherited = stack.last().expect("the root scope stays").clone();
        absorb_paint(&mut inherited, &attributes, &mut out.warnings);
        if let Some(value) = attribute(&attributes, "transform") {
            let own = parse_transform(value, line)?;
            inherited.transform = compose(inherited.transform, own);
        }
        if let Some(value) = paint_attr(&attributes, "stroke-dasharray") {
            let pattern: Vec<f32> = value.split([' ', ',']).filter_map(|n| n.trim().parse().ok()).collect();
            inherited.dash = (!pattern.is_empty() && pattern.iter().any(|d| *d > 0.0)).then_some(pattern);
        }

        match name {
            "svg" => {
                if let Some(value) = attribute(&attributes, "viewBox") {
                    let numbers: Vec<f32> = value.split([' ', ',']).filter_map(|n| n.trim().parse().ok()).collect();
                    if numbers.len() == 4 {
                        view_box = Some((numbers[0], numbers[1], numbers[2], numbers[3]));
                    }
                }
                let length = |name: &str| attribute(&attributes, name).and_then(|value| value.trim().trim_end_matches("px").parse::<f32>().ok());
                size = (length("width"), length("height"));
                if !self_closing {
                    stack.push(inherited);
                }
            }
            "g" => {
                if !self_closing {
                    stack.push(inherited);
                }
            }
            "path" => {
                let data = attribute(&attributes, "d").unwrap_or("");
                let verbs = parse_path_data(data, line)?;
                push_draws(&mut out, &inherited, verbs);
            }
            "circle" => {
                let cx = number_attribute(&attributes, "cx");
                let cy = number_attribute(&attributes, "cy");
                let r = number_attribute(&attributes, "r");
                push_draws(&mut out, &inherited, ellipse_verbs(cx, cy, r, r));
            }
            "ellipse" => {
                let cx = number_attribute(&attributes, "cx");
                let cy = number_attribute(&attributes, "cy");
                let rx = number_attribute(&attributes, "rx");
                let ry = number_attribute(&attributes, "ry");
                push_draws(&mut out, &inherited, ellipse_verbs(cx, cy, rx, ry));
            }
            "rect" => {
                let x = number_attribute(&attributes, "x");
                let y = number_attribute(&attributes, "y");
                let w = number_attribute(&attributes, "width");
                let h = number_attribute(&attributes, "height");
                // one radius names both, SVG law
                let rx = attribute(&attributes, "rx").or_else(|| attribute(&attributes, "ry")).and_then(|value| value.trim().parse().ok()).unwrap_or(0.0);
                push_draws(&mut out, &inherited, rect_verbs(x, y, w, h, rx));
            }
            "line" => {
                let x1 = number_attribute(&attributes, "x1");
                let y1 = number_attribute(&attributes, "y1");
                let x2 = number_attribute(&attributes, "x2");
                let y2 = number_attribute(&attributes, "y2");
                push_draws(&mut out, &inherited, vec![Verb::Move(x1, y1), Verb::Line(x2, y2)]);
            }
            "polyline" | "polygon" => {
                let points = attribute(&attributes, "points").unwrap_or("");
                let mut numbers = Vec::new();
                let mut scanner = Scanner::new(points, line);
                while let Ok(Some(value)) = scanner.try_number() {
                    numbers.push(value);
                }
                let mut verbs = Vec::new();
                for (i, pair) in numbers.as_chunks::<2>().0.iter().enumerate() {
                    let verb = if i == 0 { Verb::Move(pair[0], pair[1]) } else { Verb::Line(pair[0], pair[1]) };
                    verbs.push(verb);
                }
                if name == "polygon" {
                    verbs.push(Verb::Close);
                }
                push_draws(&mut out, &inherited, verbs);
            }
            _ => {
                return Err(SvgError { line, message: format!("The element <{name}> is not supported.") });
            }
        }
    }

    if out.draws.is_empty() {
        return Err(SvgError { line: 1, message: "The drawing has nothing a symbol draws.".into() });
    }
    // without a viewBox the drawing's own size is its grid, as a
    // browser draws it
    let (min_x, min_y, width, height) = view_box.unwrap_or(match size {
        (Some(width), Some(height)) => (0.0, 0.0, width, height),
        _ => (0.0, 0.0, ICON_GRID, ICON_GRID),
    });
    let longer = width.max(height);
    if longer <= 0.0 {
        return Err(SvgError { line: 1, message: "The viewBox has no size.".into() });
    }
    if (width - height).abs() > longer * 1e-3 {
        return Err(SvgError {
            line: 1,
            message: format!("The drawing is {} by {}, and a symbol is square.", unit(width), unit(height)),
        });
    }
    // normalize onto the house grid: the LONGER side becomes 24 and
    // the drawing centers; pen widths ride the same factor
    let scale = ICON_GRID / longer;
    let dx = (ICON_GRID - width * scale) / 2.0 - min_x * scale;
    let dy = (ICON_GRID - height * scale) / 2.0 - min_y * scale;
    let place = |x: f32, y: f32| (x.mul_add(scale, dx), y.mul_add(scale, dy));
    for (paint, path, _) in &mut out.draws {
        if let Paint::Stroke { width } = paint {
            *width *= scale;
        }
        for verb in path {
            *verb = map_verb(*verb, place);
        }
    }
    Ok(out)
}

/// One verb with every point it names moved.
fn map_verb(verb: Verb, map: impl Fn(f32, f32) -> (f32, f32)) -> Verb {
    match verb {
        Verb::Move(x, y) => {
            let (x, y) = map(x, y);
            Verb::Move(x, y)
        }
        Verb::Line(x, y) => {
            let (x, y) = map(x, y);
            Verb::Line(x, y)
        }
        Verb::Quad(cx, cy, x, y) => {
            let (cx, cy) = map(cx, cy);
            let (x, y) = map(x, y);
            Verb::Quad(cx, cy, x, y)
        }
        Verb::Cubic(ax, ay, bx, by, x, y) => {
            let (ax, ay) = map(ax, ay);
            let (bx, by) = map(bx, by);
            let (x, y) = map(x, y);
            Verb::Cubic(ax, ay, bx, by, x, y)
        }
        Verb::Close => Verb::Close,
    }
}

fn attribute<'a>(attributes: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    attributes.iter().find(|(key, _)| *key == name).map(|(_, value)| *value)
}

fn number_attribute(attributes: &[(&str, &str)], name: &str) -> f32 {
    attribute(attributes, name).and_then(|value| value.trim().parse().ok()).unwrap_or(0.0)
}

fn parse_attributes(rest: &str, line: usize) -> Result<Vec<(&str, &str)>, SvgError> {
    let mut out = Vec::new();
    let mut at = 0;
    let bytes = rest.as_bytes();
    while at < bytes.len() {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() {
            break;
        }
        let name_start = at;
        while at < bytes.len() && bytes[at] != b'=' && !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let name = &rest[name_start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] != b'=' {
            continue; // a bare attribute carries nothing we read
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || (bytes[at] != b'"' && bytes[at] != b'\'') {
            return Err(SvgError { line, message: format!("The attribute {name} has no quoted value.") });
        }
        let quote = bytes[at];
        at += 1;
        let value_start = at;
        while at < bytes.len() && bytes[at] != quote {
            at += 1;
        }
        out.push((name, &rest[value_start..at]));
        at += 1;
    }
    Ok(out)
}

/// One declaration out of an inline `style="fill:#b6aa9d;stroke:none"`.
fn style_value<'a>(attributes: &'a [(&'a str, &'a str)], property: &str) -> Option<&'a str> {
    let style = attribute(attributes, "style")?;
    style.split(';').find_map(|declaration| {
        let (key, value) = declaration.split_once(':')?;
        (key.trim() == property).then(|| value.trim())
    })
}

/// A paint property, from the inline `style` first and the presentation
/// attribute second: inline style wins over the attribute of the same
/// name.
fn paint_attr<'a>(attributes: &'a [(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    style_value(attributes, key).or_else(|| attribute(attributes, key))
}

/// Reads the paint attributes into the inherited state.
fn absorb_paint(state: &mut Inherited, attributes: &[(&str, &str)], warnings: &mut Vec<String>) {
    if let Some(value) = paint_attr(attributes, "fill") {
        state.fill = Some(value != "none");
        state.fill_tint = tint_of(value, "fill", warnings);
    }
    if let Some(value) = paint_attr(attributes, "stroke") {
        state.stroke = Some(value != "none");
        state.stroke_tint = tint_of(value, "stroke", warnings);
    }
    if let Some(value) = paint_attr(attributes, "stroke-width")
        && let Ok(width) = value.trim().trim_end_matches("px").parse()
    {
        state.stroke_width = width;
    }
    for key in ["fill-rule", "clip-rule"] {
        if let Some(value) = paint_attr(attributes, key) {
            state.even_odd = value == "evenodd";
        }
    }
    let fraction = |key: &str| paint_attr(attributes, key).and_then(|value| css::number(value).or_else(|| css::percent(value))).map(|value| value.clamp(0.0, 1.0));
    if let Some(value) = fraction("fill-opacity") {
        state.fill_opacity = value;
    }
    if let Some(value) = fraction("stroke-opacity") {
        state.stroke_opacity = value;
    }
    if let Some(value) = fraction("opacity") {
        state.opacity *= value;
    }
    for (key, kept) in [("stroke-linecap", "round"), ("stroke-linejoin", "round")] {
        if let Some(value) = paint_attr(attributes, key)
            && value != kept
        {
            warnings.push(format!("{key}=\"{value}\" becomes round — the one shape the pen has."));
        }
    }
}

/// A colour becomes the draw's own; `currentColor`, which the browser
/// resolves before the importer reads the drawing, is the ink. A colour
/// this parser cannot read earns a note and is drawn black.
fn tint_of(value: &str, of: &str, warnings: &mut Vec<String>) -> Option<Rgba> {
    let value = value.trim();
    if value == "none" {
        return None;
    }
    if value.eq_ignore_ascii_case("currentColor") {
        return None;
    }
    if value.eq_ignore_ascii_case("black") {
        return Some(BLACK);
    }
    if let Some(color) = hex_color(value).or_else(|| css::color(value)) {
        return Some(color);
    }
    warnings.push(format!("{of}=\"{value}\" is not a colour the door reads — the draw is black."));
    Some(BLACK)
}

/// `#rgb` and `#rrggbb`, the two forms icon sets write.
fn hex_color(value: &str) -> Option<Rgba> {
    let digits = value.strip_prefix('#')?;
    let nibble = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let bytes = digits.as_bytes();
    let mut out = [0u8; 3];
    match bytes.len() {
        3 => {
            for (i, byte) in bytes.iter().enumerate() {
                let value = nibble(*byte)?;
                out[i] = value << 4 | value;
            }
        }
        6 => {
            for (i, channel) in out.iter_mut().enumerate() {
                *channel = nibble(bytes[i * 2])? << 4 | nibble(bytes[i * 2 + 1])?;
            }
        }
        _ => return None,
    }
    Some(Rgba { r: out[0], g: out[1], b: out[2], a: 1.0 })
}

/// A colour under an opacity.
fn faded(tint: Option<Rgba>, opacity: f64) -> Option<Rgba> {
    tint.map(|color| Rgba { a: color.a * opacity, ..color })
}

/// Adds the element's contours under the effective paint: a fill draw,
/// a stroke draw, or both (the stroke paints on top, SVG order).
fn push_draws(out: &mut ParsedGlyph, state: &Inherited, verbs: Vec<Verb>) {
    if verbs.is_empty() {
        return;
    }
    // the ancestors' affine lands HERE, once: cubics carry an affine
    // exactly, and every arc already became one
    let verbs: Vec<Verb> = if state.transform == IDENTITY {
        verbs
    } else {
        verbs.into_iter().map(|verb| map_verb(verb, |x, y| apply(state.transform, x, y))).collect()
    };
    let fills = state.fill == Some(true);
    let strokes = state.stroke == Some(true);
    // a fill with no area paints nothing: the default black under an
    // open stroke is not a draw of its own
    if fills && area(&verbs) > 1e-6 {
        let rule = if state.even_odd { Rule::EvenOdd } else { Rule::NonZero };
        out.draws.push((Paint::Fill(rule), verbs.clone(), faded(state.fill_tint, state.fill_opacity * state.opacity)));
    }
    if strokes {
        // a non-uniform scale bends the pen width; the root of the
        // determinant is the honest average
        #[allow(clippy::cast_possible_truncation, reason = "a pen width is an f32 by the model")]
        let scale = (state.transform[0] * state.transform[3] - state.transform[1] * state.transform[2]).abs().sqrt() as f32;
        let width = state.stroke_width * scale;
        let tint = faded(state.stroke_tint, state.stroke_opacity * state.opacity);
        match &state.dash {
            Some(pattern) => {
                // the dashes resolve here: flatten, walk, and every dash
                // becomes its own little stroke
                let dashed = dash_verbs(&verbs, pattern);
                if !dashed.is_empty() {
                    out.draws.push((Paint::Stroke { width }, dashed, tint));
                }
            }
            None => out.draws.push((Paint::Stroke { width }, verbs, tint)),
        }
    }
}

/// The area the contours enclose, every contour on its own: the
/// shoelace over the flattened points.
fn area(verbs: &[Verb]) -> f64 {
    flatten(verbs)
        .contours()
        .map(|(points, _)| {
            let twice: f64 = points.iter().zip(points.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
            (twice / 2.0).abs()
        })
        .sum()
}

/// Walks the flattened contours emitting alternating on/off runs of
/// the pattern as plain line verbs.
#[allow(clippy::cast_possible_truncation, reason = "a grid point is an f32 by the model")]
fn dash_verbs(verbs: &[Verb], pattern: &[f32]) -> Vec<Verb> {
    let flat = flatten(verbs);
    let cycle: f32 = pattern.iter().sum();
    if cycle <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (points, sealed) in flat.contours() {
        let count = points.len();
        let segments = if sealed { count } else { count.saturating_sub(1) };
        // where we are inside the repeating pattern
        let mut slot = 0usize;
        let mut left = pattern[0];
        let mut drawing = true;
        let mut pen_down = false;
        for i in 0..segments {
            let (ax, ay) = points[i];
            let (bx, by) = points[(i + 1) % count];
            let length = (bx - ax).hypot(by - ay) as f32;
            let mut travelled = 0.0f32;
            while travelled < length {
                let step = (length - travelled).min(left);
                let t0 = f64::from(travelled / length);
                let t1 = f64::from((travelled + step) / length);
                if drawing {
                    let from = ((bx - ax).mul_add(t0, ax), (by - ay).mul_add(t0, ay));
                    let to = ((bx - ax).mul_add(t1, ax), (by - ay).mul_add(t1, ay));
                    if !pen_down {
                        out.push(Verb::Move(from.0 as f32, from.1 as f32));
                        pen_down = true;
                    }
                    out.push(Verb::Line(to.0 as f32, to.1 as f32));
                }
                travelled += step;
                left -= step;
                if left <= 0.0 {
                    slot = (slot + 1) % pattern.len();
                    left = pattern[slot];
                    // an odd pattern doubles, SVG law: on/off alternates
                    drawing = !drawing;
                    pen_down = pen_down && drawing;
                }
            }
        }
    }
    out
}

fn ellipse_verbs(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Verb> {
    const KAPPA: f32 = 0.552_284_8;
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    vec![
        Verb::Move(cx + rx, cy),
        Verb::Cubic(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry),
        Verb::Cubic(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy),
        Verb::Cubic(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry),
        Verb::Cubic(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy),
        Verb::Close,
    ]
}

fn rect_verbs(x: f32, y: f32, w: f32, h: f32, rx: f32) -> Vec<Verb> {
    const KAPPA: f32 = 0.552_284_8;
    let r = rx.max(0.0).min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return vec![Verb::Move(x, y), Verb::Line(x + w, y), Verb::Line(x + w, y + h), Verb::Line(x, y + h), Verb::Close];
    }
    let k = r * KAPPA;
    vec![
        Verb::Move(x + r, y),
        Verb::Line(x + w - r, y),
        Verb::Cubic(x + w - r + k, y, x + w, y + r - k, x + w, y + r),
        Verb::Line(x + w, y + h - r),
        Verb::Cubic(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h),
        Verb::Line(x + r, y + h),
        Verb::Cubic(x + r - k, y + h, x, y + h - r + k, x, y + h - r),
        Verb::Line(x, y + r),
        Verb::Cubic(x, y + r - k, x + r - k, y, x + r, y),
        Verb::Close,
    ]
}

// MARK: - Path data

struct Scanner<'a> {
    data: &'a str,
    at: usize,
    line: usize,
}

impl<'a> Scanner<'a> {
    const fn new(data: &'a str, line: usize) -> Scanner<'a> {
        Scanner { data, at: 0, line }
    }

    fn skip_separators(&mut self) {
        let bytes = self.data.as_bytes();
        while self.at < bytes.len() && (bytes[self.at].is_ascii_whitespace() || bytes[self.at] == b',') {
            self.at += 1;
        }
    }

    fn peek_command(&mut self) -> Option<char> {
        self.skip_separators();
        let byte = *self.data.as_bytes().get(self.at)?;
        byte.is_ascii_alphabetic().then_some(byte as char)
    }

    /// One number, or `None` at a command letter or the end. The SVG
    /// grammar packs numbers tight: `10-5` is two, `1.5.5` is two,
    /// `1e-3` is one.
    fn try_number(&mut self) -> Result<Option<f32>, SvgError> {
        self.skip_separators();
        let bytes = self.data.as_bytes();
        if self.at >= bytes.len() {
            return Ok(None);
        }
        let start = self.at;
        let mut at = self.at;
        if bytes[at] == b'+' || bytes[at] == b'-' {
            at += 1;
        }
        let digits_start = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        if at < bytes.len() && bytes[at] == b'.' {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_digit() {
                at += 1;
            }
        }
        if at == digits_start || (at == digits_start + 1 && bytes[digits_start] == b'.') {
            return Ok(None); // a command letter, or nothing
        }
        if at < bytes.len() && (bytes[at] == b'e' || bytes[at] == b'E') {
            let mut exponent = at + 1;
            if exponent < bytes.len() && (bytes[exponent] == b'+' || bytes[exponent] == b'-') {
                exponent += 1;
            }
            if exponent < bytes.len() && bytes[exponent].is_ascii_digit() {
                at = exponent;
                while at < bytes.len() && bytes[at].is_ascii_digit() {
                    at += 1;
                }
            }
        }
        self.at = at;
        self.data[start..at]
            .parse()
            .map(Some)
            .map_err(|_| SvgError { line: self.line, message: format!("The number {:?} does not parse.", &self.data[start..at]) })
    }

    fn number(&mut self) -> Result<f32, SvgError> {
        self.try_number()?.ok_or_else(|| SvgError { line: self.line, message: "A number is missing in the path data.".into() })
    }

    /// An arc FLAG is one single digit: `011` is flag 0, flag 1, then
    /// the next number starts at 1.
    fn flag(&mut self) -> Result<bool, SvgError> {
        self.skip_separators();
        let byte = self.data.as_bytes().get(self.at).copied();
        match byte {
            Some(b'0') => {
                self.at += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.at += 1;
                Ok(true)
            }
            _ => Err(SvgError { line: self.line, message: "An arc flag must be 0 or 1.".into() }),
        }
    }
}

/// One path's `d` as verbs.
///
/// # Errors
///
/// A command the door does not draw (`T`, an arc with an x-axis
/// rotation), data that does not start with `M`, or a number missing.
#[allow(clippy::too_many_lines, reason = "one arm per command, as in bunny-ui")]
pub fn parse_path_data(data: &str, line: usize) -> Result<Vec<Verb>, SvgError> {
    let mut scanner = Scanner::new(data, line);
    let mut verbs = Vec::new();
    let mut current = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    // the reflected control of S; the PREVIOUS cubic's second control
    let mut last_cubic_control: Option<(f32, f32)> = None;
    let mut command: Option<char> = None;

    loop {
        if let Some(letter) = scanner.peek_command() {
            scanner.at += 1;
            if matches!(letter, 'T' | 't') {
                return Err(SvgError { line, message: "The command T is not supported. Use Q with its control point.".into() });
            }
            command = Some(letter);
        } else {
            scanner.skip_separators();
            if scanner.at >= scanner.data.len() {
                break; // the data ends
            }
            // a bare number ahead: the previous command repeats
        }
        let Some(letter) = command else {
            return Err(SvgError { line, message: "The path data does not start with M.".into() });
        };
        let relative = letter.is_ascii_lowercase();
        let base = |current: (f32, f32)| if relative { current } else { (0.0, 0.0) };

        match letter.to_ascii_uppercase() {
            'M' => {
                let b = base(current);
                let x = scanner.number()? + b.0;
                let y = scanner.number()? + b.1;
                verbs.push(Verb::Move(x, y));
                current = (x, y);
                start = current;
                last_cubic_control = None;
                // the implicit repetition of a moveto is a LINETO
                command = Some(if relative { 'l' } else { 'L' });
            }
            'L' => {
                let b = base(current);
                let x = scanner.number()? + b.0;
                let y = scanner.number()? + b.1;
                verbs.push(Verb::Line(x, y));
                current = (x, y);
                last_cubic_control = None;
            }
            'H' => {
                let x = scanner.number()? + if relative { current.0 } else { 0.0 };
                verbs.push(Verb::Line(x, current.1));
                current.0 = x;
                last_cubic_control = None;
            }
            'V' => {
                let y = scanner.number()? + if relative { current.1 } else { 0.0 };
                verbs.push(Verb::Line(current.0, y));
                current.1 = y;
                last_cubic_control = None;
            }
            'C' => {
                let b = base(current);
                let ax = scanner.number()? + b.0;
                let ay = scanner.number()? + b.1;
                let bx = scanner.number()? + b.0;
                let by = scanner.number()? + b.1;
                let x = scanner.number()? + b.0;
                let y = scanner.number()? + b.1;
                verbs.push(Verb::Cubic(ax, ay, bx, by, x, y));
                last_cubic_control = Some((bx, by));
                current = (x, y);
            }
            'S' => {
                let b = base(current);
                // the first control reflects the previous cubic's, or
                // IS the current point, after anything else
                let (ax, ay) = last_cubic_control.map_or(current, |(px, py)| (2.0f32.mul_add(current.0, -px), 2.0f32.mul_add(current.1, -py)));
                let bx = scanner.number()? + b.0;
                let by = scanner.number()? + b.1;
                let x = scanner.number()? + b.0;
                let y = scanner.number()? + b.1;
                verbs.push(Verb::Cubic(ax, ay, bx, by, x, y));
                last_cubic_control = Some((bx, by));
                current = (x, y);
            }
            'Q' => {
                let b = base(current);
                let cx = scanner.number()? + b.0;
                let cy = scanner.number()? + b.1;
                let x = scanner.number()? + b.0;
                let y = scanner.number()? + b.1;
                verbs.push(Verb::Quad(cx, cy, x, y));
                current = (x, y);
                last_cubic_control = None;
            }
            'A' => {
                let b = base(current);
                let rx = scanner.number()?;
                let ry = scanner.number()?;
                let rotation = scanner.number()?;
                let large = scanner.flag()?;
                let sweep = scanner.flag()?;
                let x = scanner.number()? + b.0;
                let y = scanner.number()? + b.1;
                if rotation != 0.0 {
                    return Err(SvgError { line, message: "An arc with x-axis-rotation is not supported.".into() });
                }
                arc_to_cubics(&mut verbs, current, (rx, ry), large, sweep, (x, y));
                current = (x, y);
                last_cubic_control = None;
            }
            'Z' => {
                verbs.push(Verb::Close);
                current = start;
                last_cubic_control = None;
            }
            other => {
                return Err(SvgError { line, message: format!("The command {other} is not supported.") });
            }
        }
    }
    Ok(verbs)
}

/// One elliptical arc as cubics: the endpoint form becomes the centre
/// form (the SVG book's F.6.5), the sweep splits at ninety degrees,
/// and each piece is one cubic with the 4/3·tan(θ/4) handles.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "a grid point is an f32 by the model; a sweep is at most four pieces"
)]
fn arc_to_cubics(verbs: &mut Vec<Verb>, from: (f32, f32), (rx, ry): (f32, f32), large: bool, sweep: bool, to: (f32, f32)) {
    let (x1, y1) = (f64::from(from.0), f64::from(from.1));
    let (x2, y2) = (f64::from(to.0), f64::from(to.1));
    let (mut rx, mut ry) = (f64::from(rx).abs(), f64::from(ry).abs());
    if rx == 0.0 || ry == 0.0 || (x1 == x2 && y1 == y2) {
        verbs.push(Verb::Line(to.0, to.1));
        return;
    }
    // the midpoint form
    let (mx, my) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
    // radii that cannot reach get scaled up (SVG law)
    let lambda = (mx * mx) / (rx * rx) + (my * my) / (ry * ry);
    if lambda > 1.0 {
        let grow = lambda.sqrt();
        rx *= grow;
        ry *= grow;
    }
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let numerator = (rx * rx * ry * ry - rx * rx * my * my - ry * ry * mx * mx).max(0.0);
    let denominator = rx * rx * my * my + ry * ry * mx * mx;
    let coefficient = sign * (numerator / denominator).sqrt();
    let (cx_mid, cy_mid) = (coefficient * rx * my / ry, -coefficient * ry * mx / rx);
    let (cx, cy) = (cx_mid + f64::midpoint(x1, x2), cy_mid + f64::midpoint(y1, y2));
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        let dot = ux * vx + uy * vy;
        let magnitude = ux.hypot(uy) * vx.hypot(vy);
        let a = (dot / magnitude).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let start = angle(1.0, 0.0, (mx - cx_mid) / rx, (my - cy_mid) / ry);
    let mut extent = angle((mx - cx_mid) / rx, (my - cy_mid) / ry, (-mx - cx_mid) / rx, (-my - cy_mid) / ry);
    if !sweep && extent > 0.0 {
        extent -= std::f64::consts::TAU;
    }
    if sweep && extent < 0.0 {
        extent += std::f64::consts::TAU;
    }
    let pieces = (extent.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = extent / pieces as f64;
    let handle = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |theta: f64| (rx.mul_add(theta.cos(), cx), ry.mul_add(theta.sin(), cy));
    let tangent = |theta: f64| (-rx * theta.sin(), ry * theta.cos());
    let mut theta = start;
    for _ in 0..pieces {
        let next = theta + step;
        let (px, py) = point(theta);
        let (qx, qy) = point(next);
        let (tx, ty) = tangent(theta);
        let (ux, uy) = tangent(next);
        verbs.push(Verb::Cubic(
            handle.mul_add(tx, px) as f32,
            handle.mul_add(ty, py) as f32,
            (-handle).mul_add(ux, qx) as f32,
            (-handle).mul_add(uy, qy) as f32,
            qx as f32,
            qy as f32,
        ));
        theta = next;
    }
}

// MARK: - Flattening (`icon/vector.rs`)

/// How far the polyline may sit from the true curve, in grid units. The
/// drawing is flattened once, on its own grid, to walk its dashes and
/// measure its area; bunny-ui's three hundredths of a device pixel are
/// finer than either needs.
const TOLERANCE: f64 = 0.03;

/// Wang's bound caps the segment count too: a degenerate table of verbs
/// cannot ask for an unbounded fan.
const MAX_SEGMENTS: usize = 128;

/// Every contour of one path, flattened. Points sit back to back;
/// `contours` marks where each begins and whether a `Close` sealed it.
struct Flattened {
    points: Vec<(f64, f64)>,
    /// `(first point index, sealed)` per contour, in path order.
    contours: Vec<(usize, bool)>,
}

impl Flattened {
    /// The contours, each as `(points, sealed)`.
    fn contours(&self) -> impl Iterator<Item = (&[(f64, f64)], bool)> {
        self.contours.iter().enumerate().map(move |(i, &(start, sealed))| {
            let end = self.contours.get(i + 1).map_or(self.points.len(), |&(next, _)| next);
            (&self.points[start..end], sealed)
        })
    }
}

/// Flattens one verb table in its own units. Zero-length segments are
/// dropped on the spot.
fn flatten(path: &[Verb]) -> Flattened {
    let at = |x: f32, y: f32| (f64::from(x), f64::from(y));
    let mut out = Flattened { points: Vec::new(), contours: Vec::new() };
    // where the OPEN contour started: both the index and the point, so
    // `Close` can seal and a verb after it can reopen at the seam
    let mut start: Option<(usize, (f64, f64))> = None;
    let mut current: Option<(f64, f64)> = None;
    // did a drawing verb touch the OPEN contour?
    let mut drawn = false;

    for verb in path {
        match *verb {
            Verb::Move(x, y) => {
                let point = at(x, y);
                begin(&mut out, &mut start, &mut drawn, point);
                current = Some(point);
            }
            Verb::Line(x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut drawn, current) else { continue };
                let to = at(x, y);
                push(&mut out, from, to);
                current = Some(to);
            }
            Verb::Quad(cx, cy, x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut drawn, current) else { continue };
                let (control, to) = (at(cx, cy), at(x, y));
                let mut last = from;
                for point in quad_points(from, control, to) {
                    push(&mut out, last, point);
                    last = point;
                }
                current = Some(to);
            }
            Verb::Cubic(ax, ay, bx, by, x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut drawn, current) else { continue };
                let (a, b, to) = (at(ax, ay), at(bx, by), at(x, y));
                let mut last = from;
                for point in cubic_points(from, a, b, to) {
                    push(&mut out, last, point);
                    last = point;
                }
                current = Some(to);
            }
            Verb::Close => {
                if let Some((index, seam)) = start {
                    let last = out.contours.len() - 1;
                    out.contours[last] = (index, true);
                    // the pen returns to the seam (SVG law)
                    current = Some(seam);
                    start = None;
                }
            }
        }
    }
    // a contour that never grew past its Move carries no ink
    prune_empty(&mut out, drawn);
    out
}

/// Opens a contour at `point`.
fn begin(out: &mut Flattened, start: &mut Option<(usize, (f64, f64))>, drawn: &mut bool, point: (f64, f64)) {
    prune_empty(out, *drawn);
    *drawn = false;
    *start = Some((out.points.len(), point));
    out.contours.push((out.points.len(), false));
    out.points.push(point);
}

/// The current point a drawing verb continues from; `None` where a path
/// does not begin with `Move`, and the verb is skipped.
fn reopen(out: &mut Flattened, start: &mut Option<(usize, (f64, f64))>, drawn: &mut bool, current: Option<(f64, f64)>) -> Option<(f64, f64)> {
    let point = current?;
    if start.is_none() {
        // after a Close the next subpath opens AT the seam
        begin(out, start, drawn, point);
    }
    *drawn = true;
    Some(point)
}

/// One segment, unless it is nothing.
fn push(out: &mut Flattened, from: (f64, f64), to: (f64, f64)) {
    if to != from {
        out.points.push(to);
    }
}

/// Drops a contour that holds only its `Move` and was never drawn to.
fn prune_empty(out: &mut Flattened, drawn: bool) {
    if drawn {
        return;
    }
    if let Some(&(first, _)) = out.contours.last()
        && out.points.len() - first < 2
    {
        out.points.truncate(first);
        out.contours.pop();
    }
}

/// Wang's segment count for a quadratic.
fn quad_segments(p0: (f64, f64), c: (f64, f64), p2: (f64, f64)) -> usize {
    let dx = 2.0f64.mul_add(-c.0, p0.0) + p2.0;
    let dy = 2.0f64.mul_add(-c.1, p0.1) + p2.1;
    segments_for(2.0 * dx.hypot(dy))
}

/// Wang's segment count for a cubic.
fn cubic_segments(p0: (f64, f64), a: (f64, f64), b: (f64, f64), p3: (f64, f64)) -> usize {
    let first = (2.0f64.mul_add(-a.0, p0.0) + b.0).hypot(2.0f64.mul_add(-a.1, p0.1) + b.1);
    let second = (2.0f64.mul_add(-b.0, a.0) + p3.0).hypot(2.0f64.mul_add(-b.1, a.1) + p3.1);
    segments_for(6.0 * first.max(second))
}

/// `n ≥ sqrt(|B''|max / (8·tolerance))`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a count, clamped to MAX_SEGMENTS")]
fn segments_for(second_max: f64) -> usize {
    let n = (second_max / (8.0 * TOLERANCE)).sqrt().ceil();
    (n as usize).clamp(1, MAX_SEGMENTS)
}

/// The quadratic's points AFTER the start, at uniform parameters.
#[allow(clippy::cast_precision_loss, reason = "at most MAX_SEGMENTS")]
fn quad_points(p0: (f64, f64), c: (f64, f64), p2: (f64, f64)) -> impl Iterator<Item = (f64, f64)> {
    let n = quad_segments(p0, c, p2);
    (1..=n).map(move |i| {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let x = u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p2.0;
        let y = u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p2.1;
        (x, y)
    })
}

/// The cubic's points AFTER the start, at uniform parameters.
#[allow(clippy::cast_precision_loss, reason = "at most MAX_SEGMENTS")]
fn cubic_points(p0: (f64, f64), a: (f64, f64), b: (f64, f64), p3: (f64, f64)) -> impl Iterator<Item = (f64, f64)> {
    let n = cubic_segments(p0, a, b, p3);
    (1..=n).map(move |i| {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let x = u * u * u * p0.0 + 3.0 * u * u * t * a.0 + 3.0 * u * t * t * b.0 + t * t * t * p3.0;
        let y = u * u * u * p0.1 + 3.0 * u * u * t * a.1 + 3.0 * u * t * t * b.1 + t * t * t * p3.1;
        (x, y)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawing_without_a_view_box_is_drawn_on_its_size() {
        let parsed = parse(r#"<svg width="12" height="12"><path d="M0 0H12V12Z"/></svg>"#).expect("parses");
        assert_eq!(parsed.draws[0].1, [Verb::Move(0.0, 0.0), Verb::Line(24.0, 0.0), Verb::Line(24.0, 24.0), Verb::Close]);
        assert_eq!(parsed.draws[0].2, Some(BLACK), "a page's drawing is black until it says");
    }

    #[test]
    fn a_drawing_that_is_not_square_is_refused() {
        let error = parse(r#"<svg viewBox="0 0 32 16"><path d="M0 0H32V16Z"/></svg>"#).expect_err("refused");
        assert_eq!(error.to_string(), "line 1: The drawing is 32.0 by 16.0, and a symbol is square.");
    }

    #[test]
    fn a_drawing_with_nothing_to_draw_is_refused() {
        let error = parse(r#"<svg viewBox="0 0 16 16"><title>Empty</title></svg>"#).expect_err("refused");
        assert_eq!(error.message, "The drawing has nothing a symbol draws.");
    }

    #[test]
    fn a_title_is_read_past() {
        let parsed = parse(r#"<svg viewBox="0 0 24 24"><title>A <b>bold</b> mark</title><path d="M0 0H24V24Z" fill="rgb(1, 2, 3)"/></svg>"#).expect("parses");
        assert_eq!(parsed.draws.len(), 1);
        assert_eq!(parsed.draws[0].2, Some(Rgba { r: 1, g: 2, b: 3, a: 1.0 }));
    }

    #[test]
    fn a_dash_pattern_becomes_strokes_of_its_own() {
        let parsed = parse(r##"<svg viewBox="0 0 24 24"><path d="M0 12H24" fill="none" stroke="#fff" stroke-dasharray="4 4"/></svg>"##).expect("parses");
        let [(Paint::Stroke { width }, path, _)] = parsed.draws.as_slice() else { panic!("{:?}", parsed.draws) };
        assert!((width - 1.0).abs() < 1e-6);
        let dashes = path.iter().filter(|verb| matches!(verb, Verb::Move(..))).count();
        assert_eq!(dashes, 3, "{path:?}");
    }

    #[test]
    fn opacity_multiplies_down_and_the_paint_opacities_inherit() {
        let parsed = parse(r##"<svg viewBox="0 0 24 24" opacity="0.5" fill-opacity="0.5"><path d="M0 0H24V24Z" fill="#fff"/></svg>"##).expect("parses");
        assert_eq!(parsed.draws[0].2, Some(Rgba { r: 255, g: 255, b: 255, a: 0.25 }));
    }

    #[test]
    fn one_colour_leaves_the_draws_and_several_stay() {
        let mut one = parse(r##"<svg viewBox="0 0 24 24"><path d="M0 0H24V24Z" fill="#fff"/><path d="M0 0H8" stroke="#fff" fill="none"/></svg>"##).expect("parses");
        assert_eq!(one.take_ink(), Some(Rgba { r: 255, g: 255, b: 255, a: 1.0 }));
        assert!(one.draws.iter().all(|(_, _, tint)| tint.is_none()));
        let mut several = parse(r##"<svg viewBox="0 0 24 24"><path d="M0 0H24V24Z" fill="#fff"/><path d="M0 0H8" stroke="#000" fill="none"/></svg>"##).expect("parses");
        assert_eq!(several.take_ink(), None);
        assert!(several.draws.iter().all(|(_, _, tint)| tint.is_some()));
    }

    #[test]
    fn a_coordinate_reads_to_the_thousandth() {
        assert_eq!(unit(4.2 * 1.5), "6.3");
        assert_eq!(unit(12.0), "12.0");
        assert_eq!(unit(-0.000_1), "0.0");
    }
}
