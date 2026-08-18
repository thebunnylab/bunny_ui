//! The SVG door — closed by default (`--features svg`).
//!
//! ONE parser, two front doors: the offline converter prints Rust
//! const source for an app to paste, and [`Symbol::from_svg`] reads a
//! file at runtime for the app that wants that ergonomy and accepts
//! the parse cost. The default build carries NONE of this.
//!
//! The accepted subset is the measured shape of real icon sets: the
//! elements path, circle, rect, line, polyline, polygon, ellipse and
//! flat groups; path data with M L H V C S Q A Z in both cases; fill,
//! stroke, stroke-width, round caps and joins, and the even-odd rule.
//! Everything else is refused with a plain sentence and the line it
//! sits on. Colors collapse to the ONE ink a symbol has — a glyph is
//! monochrome by design, the tint arrives at paint time.

use super::{Draw, Glyph, Paint, Rule, Symbol, Verb};

/// A refusal, pointed at its line. The message is the product: short,
/// active, one instruction.
#[derive(Debug, Clone, PartialEq)]
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

/// A parsed drawing, owned — the converter prints it, the runtime door
/// leaks it into a [`Symbol`].
#[derive(Debug)]
pub struct ParsedGlyph {
    pub draws: Vec<(Paint, Vec<Verb>, Option<crate::layout::Color>)>,
    /// Non-fatal notes: a hardcoded color collapsed to the ink, a cap
    /// style replaced by round. The converter prints them; a clean
    /// file has none.
    pub warnings: Vec<String>,
}

impl Symbol {
    /// Parses an SVG at RUNTIME. The drawing lives as long as the
    /// program (an icon set is permanent by nature) — parse ONCE at
    /// startup, never in a body.
    pub fn from_svg(name: &'static str, svg: &str) -> Result<Symbol, SvgError> {
        let parsed = parse(svg)?;
        let draws: Vec<Draw> = parsed
            .draws
            .into_iter()
            .map(|(paint, path, tint)| Draw {
                paint,
                path: &*Box::leak(path.into_boxed_slice()),
                tint,
            })
            .collect();
        let glyph: &'static Glyph =
            Box::leak(Box::new(Glyph { draws: &*Box::leak(draws.into_boxed_slice()) }));
        Ok(Symbol::new(name, glyph))
    }
}

/// The converter's other half: the parsed drawing as Rust const
/// source, in the same shape the house set is written.
pub fn to_rust_const(const_name: &str, name: &str, parsed: &ParsedGlyph) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for warning in &parsed.warnings {
        let _ = writeln!(out, "// NOTE: {warning}");
    }
    let _ = writeln!(out, "const {const_name}_GLYPH: Glyph = Glyph {{");
    let _ = writeln!(out, "    draws: &[");
    for (paint, path, tint) in &parsed.draws {
        let paint_source = match paint {
            Paint::Fill(Rule::NonZero) => "Paint::Fill(Rule::NonZero)".to_string(),
            Paint::Fill(Rule::EvenOdd) => "Paint::Fill(Rule::EvenOdd)".to_string(),
            Paint::Stroke { width } => format!("Paint::Stroke {{ width: {width:?} }}"),
        };
        let tint_source = match tint {
            Some(color) => format!(
                "Some(Color {{ r: {}, g: {}, b: {}, a: {} }})",
                color.r, color.g, color.b, color.a
            ),
            None => "None".to_string(),
        };
        let _ = writeln!(out, "        Draw {{");
        let _ = writeln!(out, "            paint: {paint_source},");
        let _ = writeln!(out, "            tint: {tint_source},");
        let _ = writeln!(out, "            path: &[");
        for verb in path {
            let verb_source = match verb {
                Verb::Move(x, y) => format!("Move({x:?}, {y:?})"),
                Verb::Line(x, y) => format!("Line({x:?}, {y:?})"),
                Verb::Quad(cx, cy, x, y) => format!("Quad({cx:?}, {cy:?}, {x:?}, {y:?})"),
                Verb::Cubic(ax, ay, bx, by, x, y) => {
                    format!("Cubic({ax:?}, {ay:?}, {bx:?}, {by:?}, {x:?}, {y:?})")
                }
                Verb::Close => "Close".to_string(),
            };
            let _ = writeln!(out, "                {verb_source},");
        }
        let _ = writeln!(out, "            ],");
        let _ = writeln!(out, "        }},");
    }
    let _ = writeln!(out, "    ],");
    let _ = writeln!(out, "}};");
    let _ = writeln!(out, "pub const {const_name}: Symbol = Symbol::new({name:?}, &{const_name}_GLYPH);");
    out
}

// MARK: - The tag walk

/// What an element inherits from its ancestors — the paint state.
#[derive(Clone)]
struct Inherited {
    fill: Option<bool>,   // Some(true) = paint, Some(false) = none
    stroke: Option<bool>, // same
    /// A HARDCODED color becomes the draw's own tint — the crab stays
    /// orange in any theme. The ink placeholders (currentColor, black)
    /// stay `None` and re-tint with the symbol.
    fill_tint: Option<crate::layout::Color>,
    stroke_tint: Option<crate::layout::Color>,
    stroke_width: f32,
    even_odd: bool,
}

impl Default for Inherited {
    fn default() -> Self {
        // SVG law: fill black, stroke none — until somebody says
        Inherited {
            fill: Some(true),
            stroke: Some(false),
            fill_tint: None,
            stroke_tint: None,
            stroke_width: 1.0,
            even_odd: false,
        }
    }
}

/// The measured refusals — each with its instruction.
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

pub fn parse(svg: &str) -> Result<ParsedGlyph, SvgError> {
    let mut out = ParsedGlyph { draws: Vec::new(), warnings: Vec::new() };
    let mut stack: Vec<Inherited> = vec![Inherited::default()];
    let mut view_box: Option<(f32, f32, f32, f32)> = None;
    let mut at = 0usize;
    let bytes = svg.as_bytes();
    let line_of = |at: usize| 1 + svg[..at].bytes().filter(|b| *b == b'\n').count();

    while at < bytes.len() {
        let Some(open) = svg[at..].find('<') else { break };
        let start = at + open;
        if svg[start..].starts_with("<!--") {
            let end = svg[start..]
                .find("-->")
                .ok_or_else(|| SvgError { line: line_of(start), message: "The comment does not end.".into() })?;
            at = start + end + 3;
            continue;
        }
        if svg[start..].starts_with("<?") || svg[start..].starts_with("<!") {
            let end = svg[start..].find('>').unwrap_or(svg.len() - start);
            at = start + end + 1;
            continue;
        }
        let end = svg[start..]
            .find('>')
            .ok_or_else(|| SvgError { line: line_of(start), message: "The tag does not end.".into() })?;
        let tag = &svg[start + 1..start + end];
        at = start + end + 1;
        let line = line_of(start);

        if let Some(name) = tag.strip_prefix('/') {
            // a closing tag pops what its opener pushed
            if matches!(name.trim(), "g" | "svg") && stack.len() > 1 {
                stack.pop();
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, rest) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let attributes = parse_attributes(rest, line)?;

        for (refused, message) in REFUSED_ELEMENTS {
            if name == *refused {
                return Err(SvgError { line, message: (*message).into() });
            }
        }
        for (key, _) in &attributes {
            if *key == "transform" {
                return Err(SvgError {
                    line,
                    message: "The attribute transform is not supported. Apply it to the coordinates before you convert.".into(),
                });
            }
            if *key == "stroke-dasharray" {
                return Err(SvgError {
                    line,
                    message: "The attribute stroke-dasharray is not supported. The pen draws solid lines.".into(),
                });
            }
        }

        let mut inherited = stack.last().expect("the root scope stays").clone();
        absorb_paint(&mut inherited, &attributes, &mut out.warnings);

        match name {
            "svg" => {
                if let Some(value) = attribute(&attributes, "viewBox") {
                    let numbers: Vec<f32> =
                        value.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                    if numbers.len() == 4 {
                        view_box = Some((numbers[0], numbers[1], numbers[2], numbers[3]));
                    }
                }
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
                let rx = number_attribute(&attributes, "rx");
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
                for (i, pair) in numbers.chunks_exact(2).enumerate() {
                    let verb = if i == 0 {
                        Verb::Move(pair[0], pair[1])
                    } else {
                        Verb::Line(pair[0], pair[1])
                    };
                    verbs.push(verb);
                }
                if name == "polygon" {
                    verbs.push(Verb::Close);
                }
                push_draws(&mut out, &inherited, verbs);
            }
            _ => {
                return Err(SvgError {
                    line,
                    message: format!("The element <{name}> is not supported."),
                });
            }
        }
    }

    // normalize onto the house grid: the LONGER side becomes 24 and
    // the drawing centers — pen widths ride the same factor
    let (min_x, min_y, width, height) = view_box.unwrap_or((0.0, 0.0, 24.0, 24.0));
    let longer = width.max(height);
    if longer <= 0.0 {
        return Err(SvgError { line: 1, message: "The viewBox has no size.".into() });
    }
    let scale = super::ICON_GRID as f32 / longer;
    let dx = (super::ICON_GRID as f32 - width * scale) / 2.0 - min_x * scale;
    let dy = (super::ICON_GRID as f32 - height * scale) / 2.0 - min_y * scale;
    let place = |x: f32, y: f32| (x * scale + dx, y * scale + dy);
    for (paint, path, _) in &mut out.draws {
        if let Paint::Stroke { width } = paint {
            *width *= scale;
        }
        for verb in path {
            *verb = match *verb {
                Verb::Move(x, y) => {
                    let (x, y) = place(x, y);
                    Verb::Move(x, y)
                }
                Verb::Line(x, y) => {
                    let (x, y) = place(x, y);
                    Verb::Line(x, y)
                }
                Verb::Quad(cx, cy, x, y) => {
                    let (cx, cy) = place(cx, cy);
                    let (x, y) = place(x, y);
                    Verb::Quad(cx, cy, x, y)
                }
                Verb::Cubic(ax, ay, bx, by, x, y) => {
                    let (ax, ay) = place(ax, ay);
                    let (bx, by) = place(bx, by);
                    let (x, y) = place(x, y);
                    Verb::Cubic(ax, ay, bx, by, x, y)
                }
                Verb::Close => Verb::Close,
            };
        }
    }
    Ok(out)
}

fn attribute<'a>(attributes: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    attributes.iter().find(|(key, _)| *key == name).map(|(_, value)| *value)
}

fn number_attribute(attributes: &[(&str, &str)], name: &str) -> f32 {
    attribute(attributes, name).and_then(|value| value.trim().parse().ok()).unwrap_or(0.0)
}

fn parse_attributes<'a>(rest: &'a str, line: usize) -> Result<Vec<(&'a str, &'a str)>, SvgError> {
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

/// Reads the paint attributes into the inherited state. Colors
/// collapse: any value that is not `none` means "paint with the ink".
fn absorb_paint(state: &mut Inherited, attributes: &[(&str, &str)], warnings: &mut Vec<String>) {
    if let Some(value) = attribute(attributes, "fill") {
        state.fill = Some(value != "none");
        state.fill_tint = tint_of(value, "fill", warnings);
    }
    if let Some(value) = attribute(attributes, "stroke") {
        state.stroke = Some(value != "none");
        state.stroke_tint = tint_of(value, "stroke", warnings);
    }
    if let Some(value) = attribute(attributes, "stroke-width") {
        if let Ok(width) = value.trim().parse() {
            state.stroke_width = width;
        }
    }
    for key in ["fill-rule", "clip-rule"] {
        if let Some(value) = attribute(attributes, key) {
            state.even_odd = value == "evenodd";
        }
    }
    for (key, kept) in [("stroke-linecap", "round"), ("stroke-linejoin", "round")] {
        if let Some(value) = attribute(attributes, key) {
            if value != kept {
                warnings.push(format!("{key}=\"{value}\" becomes round — the one shape the pen has."));
            }
        }
    }
}

/// A real color becomes the draw's own tint; the ink placeholders
/// (currentColor, black — sixty-five corpus files write `#000` meaning
/// "the ink") stay `None`. A color this parser cannot read earns a
/// note and falls back to the ink.
fn tint_of(
    value: &str,
    of: &str,
    warnings: &mut Vec<String>,
) -> Option<crate::layout::Color> {
    let quiet = ["none", "currentColor", "", "black", "#000", "#000000"];
    if quiet.contains(&value) {
        return None;
    }
    match hex_color(value) {
        Some(color) => Some(color),
        None => {
            warnings.push(format!(
                "{of}=\"{value}\" is not a hex color — the draw takes the symbol ink."
            ));
            None
        }
    }
}

/// `#rgb` and `#rrggbb` — the two forms icon sets write.
fn hex_color(value: &str) -> Option<crate::layout::Color> {
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
    match bytes.len() {
        3 => {
            let mut out = [0u8; 3];
            for (i, byte) in bytes.iter().enumerate() {
                let value = nibble(*byte)?;
                out[i] = value << 4 | value;
            }
            Some(crate::layout::Color { r: out[0], g: out[1], b: out[2], a: 255 })
        }
        6 => {
            let mut out = [0u8; 3];
            for i in 0..3 {
                out[i] = nibble(bytes[i * 2])? << 4 | nibble(bytes[i * 2 + 1])?;
            }
            Some(crate::layout::Color { r: out[0], g: out[1], b: out[2], a: 255 })
        }
        _ => None,
    }
}

/// Adds the element's contours under the effective paint: a fill draw,
/// a stroke draw, or both (the stroke paints on top, SVG order).
fn push_draws(out: &mut ParsedGlyph, state: &Inherited, verbs: Vec<Verb>) {
    if verbs.is_empty() {
        return;
    }
    let fills = state.fill == Some(true);
    let strokes = state.stroke == Some(true);
    if fills {
        let rule = if state.even_odd { Rule::EvenOdd } else { Rule::NonZero };
        out.draws.push((Paint::Fill(rule), verbs.clone(), state.fill_tint));
    }
    if strokes {
        out.draws.push((Paint::Stroke { width: state.stroke_width }, verbs, state.stroke_tint));
    }
}

fn ellipse_verbs(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Verb> {
    const KAPPA: f32 = 0.552_284_75;
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
    let r = rx.max(0.0).min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return vec![
            Verb::Move(x, y),
            Verb::Line(x + w, y),
            Verb::Line(x + w, y + h),
            Verb::Line(x, y + h),
            Verb::Close,
        ];
    }
    const KAPPA: f32 = 0.552_284_75;
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
    fn new(data: &'a str, line: usize) -> Scanner<'a> {
        Scanner { data, at: 0, line }
    }

    fn skip_separators(&mut self) {
        let bytes = self.data.as_bytes();
        while self.at < bytes.len() && (bytes[self.at].is_ascii_whitespace() || bytes[self.at] == b',')
        {
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
        self.data[start..at].parse().map(Some).map_err(|_| SvgError {
            line: self.line,
            message: format!("The number {:?} does not parse.", &self.data[start..at]),
        })
    }

    fn number(&mut self) -> Result<f32, SvgError> {
        self.try_number()?.ok_or_else(|| SvgError {
            line: self.line,
            message: "A number is missing in the path data.".into(),
        })
    }

    /// An arc FLAG is one single digit — `011` is flag 0, flag 1,
    /// then the next number starts at 1.
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
            _ => Err(SvgError {
                line: self.line,
                message: "An arc flag must be 0 or 1.".into(),
            }),
        }
    }
}

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
                return Err(SvgError {
                    line,
                    message: "The command T is not supported. Use Q with its control point.".into(),
                });
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
                // the first control reflects the previous cubic's —
                // or IS the current point, after anything else
                let (ax, ay) = match last_cubic_control {
                    Some((px, py)) => (2.0 * current.0 - px, 2.0 * current.1 - py),
                    None => current,
                };
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
                    return Err(SvgError {
                        line,
                        message: "An arc with x-axis-rotation is not supported.".into(),
                    });
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
                return Err(SvgError {
                    line,
                    message: format!("The command {other} is not supported."),
                });
            }
        }
    }
    Ok(verbs)
}

/// One elliptical arc as cubics — the endpoint form becomes the centre
/// form (the SVG book's F.6.5), the sweep splits at ninety degrees,
/// and each piece is one cubic with the 4/3·tan(θ/4) handles.
fn arc_to_cubics(
    verbs: &mut Vec<Verb>,
    from: (f32, f32),
    (rx, ry): (f32, f32),
    large: bool,
    sweep: bool,
    to: (f32, f32),
) {
    let (x1, y1) = (from.0 as f64, from.1 as f64);
    let (x2, y2) = (to.0 as f64, to.1 as f64);
    let (mut rx, mut ry) = ((rx as f64).abs(), (ry as f64).abs());
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
    let sign = if large != sweep { 1.0 } else { -1.0 };
    let numerator = (rx * rx * ry * ry - rx * rx * my * my - ry * ry * mx * mx).max(0.0);
    let denominator = rx * rx * my * my + ry * ry * mx * mx;
    let coefficient = sign * (numerator / denominator).sqrt();
    let (cx_mid, cy_mid) = (coefficient * rx * my / ry, -coefficient * ry * mx / rx);
    let (cx, cy) = (cx_mid + (x1 + x2) / 2.0, cy_mid + (y1 + y2) / 2.0);
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        let dot = ux * vx + uy * vy;
        let magnitude = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / magnitude).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let start = angle(1.0, 0.0, (mx - cx_mid) / rx, (my - cy_mid) / ry);
    let mut extent =
        angle((mx - cx_mid) / rx, (my - cy_mid) / ry, (-mx - cx_mid) / rx, (-my - cy_mid) / ry);
    if !sweep && extent > 0.0 {
        extent -= std::f64::consts::TAU;
    }
    if sweep && extent < 0.0 {
        extent += std::f64::consts::TAU;
    }
    let pieces = (extent.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = extent / pieces as f64;
    let handle = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |theta: f64| (cx + rx * theta.cos(), cy + ry * theta.sin());
    let tangent = |theta: f64| (-rx * theta.sin(), ry * theta.cos());
    let mut theta = start;
    for _ in 0..pieces {
        let next = theta + step;
        let (px, py) = point(theta);
        let (qx, qy) = point(next);
        let (tx, ty) = tangent(theta);
        let (ux, uy) = tangent(next);
        verbs.push(Verb::Cubic(
            (px + handle * tx) as f32,
            (py + handle * ty) as f32,
            (qx - handle * ux) as f32,
            (qy - handle * uy) as f32,
            qx as f32,
            qy as f32,
        ));
        theta = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_number_scanner_reads_the_packed_grammar() {
        let mut scanner = Scanner::new("1.5.5 10-5 +.5 1e-3", 1);
        let mut numbers = Vec::new();
        while let Some(value) = scanner.try_number().unwrap() {
            numbers.push(value);
        }
        assert_eq!(numbers, vec![1.5, 0.5, 10.0, -5.0, 0.5, 0.001]);
    }

    #[test]
    fn packed_arc_flags_read_one_digit_each() {
        // a1 1 0 011 1 — flags 0 and 1, then the endpoint (1, 1)
        let verbs = parse_path_data("M0 10a1 1 0 011 1", 1).unwrap();
        assert!(matches!(verbs[0], Verb::Move(0.0, 10.0)));
        assert!(verbs.len() > 1, "the arc produced curves: {verbs:?}");
        let end = verbs.last().unwrap();
        if let Verb::Cubic(_, _, _, _, x, y) = end {
            assert!((x - 1.0).abs() < 1e-4 && (y - 11.0).abs() < 1e-4, "landed at {x},{y}");
        } else {
            panic!("an arc ends in a cubic: {end:?}");
        }
    }

    #[test]
    fn implicit_repetition_after_a_moveto_is_a_lineto() {
        let verbs = parse_path_data("m3 4 2 0 0 2z", 1).unwrap();
        assert_eq!(
            verbs,
            vec![
                Verb::Move(3.0, 4.0),
                Verb::Line(5.0, 4.0),
                Verb::Line(5.0, 6.0),
                Verb::Close
            ]
        );
    }

    #[test]
    fn a_smooth_cubic_reflects_and_a_cold_one_sits_still() {
        let verbs = parse_path_data("M0 0C1 2 3 4 5 6S9 10 11 12", 1).unwrap();
        // the S control reflects (3,4) around (5,6) → (7,8)
        assert_eq!(verbs[2], Verb::Cubic(7.0, 8.0, 9.0, 10.0, 11.0, 12.0));
        let cold = parse_path_data("M0 0L5 6S9 10 11 12", 1).unwrap();
        // after a line the reflection has nothing to mirror: the
        // control IS the current point
        assert_eq!(cold[2], Verb::Cubic(5.0, 6.0, 9.0, 10.0, 11.0, 12.0));
    }

    #[test]
    fn each_refusal_speaks_its_sentence() {
        let cases = [
            (
                r#"<svg viewBox="0 0 24 24"><defs></defs></svg>"#,
                "The element <defs> is not supported. Flatten the file before you convert it.",
            ),
            (
                "<svg viewBox=\"0 0 24 24\">\n<g transform=\"rotate(90)\"/></svg>",
                "The attribute transform is not supported. Apply it to the coordinates before you convert.",
            ),
            (
                "<svg viewBox=\"0 0 16 16\">\n\n<circle cx=\"8\" cy=\"8\" r=\"6\" stroke-dasharray=\"21 40\"/></svg>",
                "The attribute stroke-dasharray is not supported. The pen draws solid lines.",
            ),
        ];
        for (svg, sentence) in cases {
            let error = parse(svg).unwrap_err();
            assert_eq!(error.message, sentence);
        }
        // the line number points at the sinner, not at the file
        let error = parse("<svg viewBox=\"0 0 24 24\">\n\n<mask/></svg>").unwrap_err();
        assert_eq!(error.line, 3);
        // T is refused at the path
        let error = parse_path_data("M0 0Q1 1 2 2T4 4", 7).unwrap_err();
        assert!(error.message.starts_with("The command T"));
    }

    #[test]
    fn a_sixteen_grid_file_lands_on_the_house_grid() {
        // the folder-ish square: viewBox 16, stroke 2 → grid 24,
        // pen 3 — same drawing, one and a half times the scale
        let svg = r##"<svg xmlns="x" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="2"><path d="M2 2h12v12H2z"/></svg>"##;
        let parsed = parse(svg).unwrap();
        assert_eq!(parsed.draws.len(), 1);
        let (paint, path, _) = &parsed.draws[0];
        assert_eq!(*paint, Paint::Stroke { width: 3.0 });
        assert_eq!(path[0], Verb::Move(3.0, 3.0));
        assert_eq!(path[1], Verb::Line(21.0, 3.0));
    }

    #[test]
    fn fill_and_stroke_make_two_draws_in_svg_order() {
        let svg = r#"<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1"><circle cx="12" cy="12" r="8"/></svg>"#;
        let parsed = parse(svg).unwrap();
        assert_eq!(parsed.draws.len(), 2);
        assert!(matches!(parsed.draws[0].0, Paint::Fill(Rule::NonZero)));
        assert!(matches!(parsed.draws[1].0, Paint::Stroke { .. }));
    }

    #[test]
    fn a_hardcoded_color_becomes_the_draws_own_tint() {
        let svg = r##"<svg viewBox="0 0 24 24" fill="#89b4fa"><rect x="4" y="4" width="16" height="16" rx="2"/></svg>"##;
        let parsed = parse(svg).unwrap();
        assert_eq!(parsed.draws.len(), 1);
        assert_eq!(
            parsed.draws[0].2,
            Some(crate::layout::Color { r: 0x89, g: 0xb4, b: 0xfa, a: 255 }),
            "the palette rides the draw"
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        // the short form reads too, and the ink placeholder stays None
        assert_eq!(hex_color("#fa0"), Some(crate::layout::Color { r: 0xff, g: 0xaa, b: 0x00, a: 255 }));
        let plain = parse(r##"<svg viewBox="0 0 24 24" fill="#000"><rect x="4" y="4" width="16" height="16"/></svg>"##).unwrap();
        assert_eq!(plain.draws[0].2, None, "black is the ink placeholder");
    }

    #[test]
    fn one_parser_two_doors() {
        // the runtime door and the codegen door must agree verb for
        // verb — this is the test that pins the module decision
        let svg = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21 21-4.8-4.8"/><circle cx="11" cy="11" r="7"/></svg>"#;
        let symbol = Symbol::from_svg("test.search", svg).unwrap();
        let parsed = parse(svg).unwrap();
        assert_eq!(symbol.glyph.draws.len(), parsed.draws.len());
        for (leaked, (paint, path, tint)) in symbol.glyph.draws.iter().zip(&parsed.draws) {
            assert_eq!(leaked.paint, *paint);
            assert_eq!(leaked.path, &path[..]);
            assert_eq!(leaked.tint, *tint);
        }
        // and the printed const carries the same counts
        let source = to_rust_const("SEARCH", "search", &parsed);
        assert_eq!(source.matches("Draw {").count(), parsed.draws.len());
        assert!(source.contains("pub const SEARCH: Symbol"));
    }

    #[test]
    fn the_real_corpus_shape_parses() {
        // a verbatim Lucide-style folder (the measured common case)
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h3l2 2h9a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"##;
        let parsed = parse(svg).unwrap();
        assert_eq!(parsed.draws.len(), 1);
        assert!(matches!(parsed.draws[0].0, Paint::Stroke { .. }));
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let closes = parsed.draws[0].1.iter().filter(|v| matches!(v, Verb::Close)).count();
        assert_eq!(closes, 1);
    }
}