//! An SVG drawn into the page: `image(svg).inline_svg()` lowers, on the
//! element tiers, to an `<svg>` whose shapes are elements of the document
//! instead of a picture the browser decodes apart.
//!
//! The markup never reaches the browser as markup. The file is read here
//! into a tree and only what [`TAGS`] and [`ATTRIBUTES`] name survives it;
//! the wire carries their codes, so the glue can only create what the
//! tables hold. Everything else is dropped with what it holds: scripts,
//! `<foreignObject>`, `<style>`, event handlers, and any reference that
//! leaves the drawing — an `href` is `#name` of this drawing or nothing, a
//! `url(…)` is `url(#name)` of this drawing or nothing. Ids are renamed per
//! element ([`Value::spell`]), so two copies of one drawing never share a
//! gradient or a clip.

use std::cell::RefCell;
use std::rc::Rc;

use motor::hash::FxHashMap as HashMap;

/// The elements a drawing keeps, by their code on the wire: the glue's
/// `DRAWING_TAGS` is this table, in this order. Code 0 is the root.
pub const TAGS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "mask",
];

/// The attributes a drawing keeps, by their code on the wire: the glue's
/// `DRAWING_ATTRIBUTES`. From [`PRESENTATION`] on they are CSS properties
/// as well, the ones a `style` attribute may set.
pub const ATTRIBUTES: &[&str] = &[
    "id",
    "href",
    "viewBox",
    "preserveAspectRatio",
    "width",
    "height",
    "x",
    "y",
    "d",
    "rx",
    "ry",
    "cx",
    "cy",
    "r",
    "fx",
    "fy",
    "fr",
    "x1",
    "y1",
    "x2",
    "y2",
    "points",
    "pathLength",
    "offset",
    "transform",
    "gradientTransform",
    "gradientUnits",
    "spreadMethod",
    "clipPathUnits",
    "maskUnits",
    "maskContentUnits",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "clip-path",
    "clip-rule",
    "mask",
    "stop-color",
    "stop-opacity",
    "color",
    "display",
    "visibility",
    "vector-effect",
    "shape-rendering",
    "paint-order",
];

/// Where the presentation attributes start in [`ATTRIBUTES`].
pub const PRESENTATION: usize = 31;

const ID: u8 = 0;
const HREF: u8 = 1;
const WIDTH: u8 = 4;
const HEIGHT: u8 = 5;

/// The functions a value may call. `url` is one of them only as
/// `url(#name)`.
const FUNCTIONS: &[&str] =
    &["url", "rgb", "rgba", "hsl", "hsla", "matrix", "translate", "scale", "rotate", "skewx", "skewy"];

const MAX_NODES: usize = 16_384;
const MAX_DEPTH: usize = 64;

/// Why a file is not a drawing; the image then stays an `<img>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingError {
    /// Not UTF-8.
    Encoding,
    /// The markup does not parse: a tag, a comment or a quote that never
    /// ends, a closing tag that closes nothing it opened.
    Markup,
    /// The document's element is not an `<svg>`.
    NotSvg,
    /// More elements, or deeper, than a drawing may hold.
    TooLarge,
}

/// One attribute's value, kept apart from its spelling: an id and a
/// reference name the drawing's n-th id, which [`Value::spell`] writes
/// per element.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(Box<str>),
    Id(u32),
    /// `#name`.
    Href(u32),
    /// `url(#name)`.
    Url(u32),
}

impl Value {
    /// The value as the element `scope` wears it.
    pub fn spell(&self, scope: u32) -> String {
        match self {
            Value::Text(text) => text.to_string(),
            Value::Id(n) => format!("bunny-{scope}-{n}"),
            Value::Href(n) => format!("#bunny-{scope}-{n}"),
            Value::Url(n) => format!("url(#bunny-{scope}-{n})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// A code into [`TAGS`].
    pub tag: u8,
    /// Codes into [`ATTRIBUTES`], each once.
    pub attributes: Vec<(u8, Value)>,
    pub children: Vec<Node>,
}

/// A parsed, allow-listed SVG: the root `<svg>` and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawing {
    pub root: Node,
}

impl Drawing {
    pub fn parse(bytes: &[u8]) -> Result<Drawing, DrawingError> {
        let text = std::str::from_utf8(bytes).map_err(|_| DrawingError::Encoding)?;
        let raw = read(text)?;
        if raw.name != "svg" {
            return Err(DrawingError::NotSvg);
        }
        let mut ids = HashMap::default();
        // the root's own id would be the element's: it names nothing here
        for child in raw.children.iter().filter(|child| kept(child)) {
            collect_ids(child, &mut ids);
        }
        Ok(Drawing { root: keep(&raw, &ids, &mut Vec::new()) })
    }

    /// The root's attributes the element wears: its `width` and `height`
    /// only when the picture keeps its own size.
    pub fn root_attributes(&self, own_size: bool) -> impl Iterator<Item = &(u8, Value)> {
        self.root.attributes.iter().filter(move |(code, _)| own_size || !matches!(*code, WIDTH | HEIGHT))
    }

    /// The drawing a picture's bytes hold, read once per identity.
    pub fn of(source: &crate::image_engine::ImageSource) -> Option<Rc<Drawing>> {
        let crate::image_engine::ImageSource::Bytes { key, bytes } = source else {
            return None;
        };
        thread_local! {
            static READ: RefCell<HashMap<u64, Option<Rc<Drawing>>>> = RefCell::new(HashMap::default());
        }
        READ.with(|read| {
            let mut read = read.borrow_mut();
            if let Some(drawing) = read.get(key) {
                return drawing.clone();
            }
            if read.len() >= 256 {
                read.clear();
            }
            let drawing = Drawing::parse(bytes).ok().map(Rc::new);
            read.insert(*key, drawing.clone());
            drawing
        })
    }
}

// MARK: - The markup

struct Raw<'a> {
    name: &'a str,
    attributes: Vec<(&'a str, Option<String>)>,
    children: Vec<Raw<'a>>,
}

fn read(text: &str) -> Result<Raw<'_>, DrawingError> {
    let mut stack: Vec<Raw<'_>> = Vec::new();
    let mut root: Option<Raw<'_>> = None;
    let mut nodes = 0usize;
    let mut at = 0usize;
    while let Some(open) = text[at..].find('<') {
        let start = at + open;
        let rest = &text[start..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            at = start + 4 + comment.find("-->").ok_or(DrawingError::Markup)? + 3;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            at = start + rest.find("]]>").ok_or(DrawingError::Markup)? + 3;
            continue;
        }
        if rest.starts_with("<?") {
            at = start + rest.find("?>").ok_or(DrawingError::Markup)? + 2;
            continue;
        }
        if rest.starts_with("<!") {
            at = start + declaration_end(rest).ok_or(DrawingError::Markup)?;
            continue;
        }
        if let Some(close) = rest.strip_prefix("</") {
            let end = close.find('>').ok_or(DrawingError::Markup)?;
            let name = close[..end].trim();
            let node = stack.pop().ok_or(DrawingError::Markup)?;
            if node.name != name {
                return Err(DrawingError::Markup);
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None if root.is_none() => root = Some(node),
                None => return Err(DrawingError::Markup),
            }
            at = start + 2 + end + 1;
            continue;
        }
        let (node, length, closed) = element(&text[start + 1..])?;
        nodes += 1;
        if nodes > MAX_NODES || stack.len() >= MAX_DEPTH {
            return Err(DrawingError::TooLarge);
        }
        at = start + 1 + length;
        if !closed {
            stack.push(node);
            continue;
        }
        match stack.last_mut() {
            Some(parent) => parent.children.push(node),
            None if root.is_none() => root = Some(node),
            None => return Err(DrawingError::Markup),
        }
    }
    if !stack.is_empty() {
        return Err(DrawingError::Markup);
    }
    root.ok_or(DrawingError::NotSvg)
}

/// The end of a `<!…>` declaration, past an internal subset in brackets.
fn declaration_end(rest: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    for (index, byte) in rest.bytes().enumerate() {
        match (quote, byte) {
            (Some(open), _) if byte == open => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'[') => depth += 1,
            (None, b']') => depth -= 1,
            (None, b'>') if depth <= 0 => return Some(index + 1),
            _ => {}
        }
    }
    None
}

/// One start tag after its `<`: the element, the bytes it took and
/// whether it closed itself.
fn element(text: &str) -> Result<(Raw<'_>, usize, bool), DrawingError> {
    let bytes = text.as_bytes();
    let name_end = bytes
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || *byte == b'/' || *byte == b'>')
        .ok_or(DrawingError::Markup)?;
    if name_end == 0 {
        return Err(DrawingError::Markup);
    }
    let mut node = Raw { name: &text[..name_end], attributes: Vec::new(), children: Vec::new() };
    let mut at = name_end;
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        match bytes.get(at) {
            None => return Err(DrawingError::Markup),
            Some(b'>') => return Ok((node, at + 1, false)),
            Some(b'/') if bytes.get(at + 1) == Some(&b'>') => return Ok((node, at + 2, true)),
            Some(_) => {}
        }
        let name_start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'=' | b'>' | b'/') {
            at += 1;
        }
        let name = &text[name_start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if name.is_empty() || bytes.get(at) != Some(&b'=') {
            return Err(DrawingError::Markup);
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let quote = *bytes.get(at).ok_or(DrawingError::Markup)?;
        if quote != b'"' && quote != b'\'' {
            return Err(DrawingError::Markup);
        }
        let value_start = at + 1;
        let length = bytes[value_start..].iter().position(|byte| *byte == quote).ok_or(DrawingError::Markup)?;
        let raw = &text[value_start..value_start + length];
        if raw.contains('<') {
            return Err(DrawingError::Markup);
        }
        node.attributes.push((name, unescape(raw)));
        at = value_start + length + 1;
    }
}

/// The five entities and the numeric references; a value naming any
/// other entity reads as nothing.
fn unescape(raw: &str) -> Option<String> {
    if !raw.contains('&') {
        return Some(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp + 1..];
        let end = tail.find(';')?;
        let entity = &tail[..end];
        let decoded = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let number = match entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => entity.strip_prefix('#')?.parse().ok()?,
                };
                char::from_u32(number)?
            }
        };
        out.push(decoded);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

// MARK: - The allow-list

fn tag_code(name: &str) -> Option<u8> {
    TAGS.iter().position(|tag| *tag == name).map(|code| code as u8)
}

fn attribute_code(name: &str) -> Option<u8> {
    let name = if name == "xlink:href" { "href" } else { name };
    ATTRIBUTES.iter().position(|attribute| *attribute == name).map(|code| code as u8)
}

/// The ids the kept elements declare, numbered in document order; the
/// first of two equal ids wins.
fn collect_ids(raw: &Raw<'_>, ids: &mut HashMap<String, u32>) {
    if let Some((_, Some(id))) = raw.attributes.iter().find(|(name, _)| *name == "id") {
        let id = id.trim();
        if !id.is_empty() && !ids.contains_key(id) {
            let next = ids.len() as u32;
            ids.insert(id.to_string(), next);
        }
    }
    for child in raw.children.iter().filter(|child| kept(child)) {
        collect_ids(child, ids);
    }
}

/// An element under the root the drawing keeps: one the tables name, and
/// no svg of its own, which would open a viewport.
fn kept(raw: &Raw<'_>) -> bool {
    tag_code(raw.name).is_some_and(|tag| tag != 0)
}

fn keep(raw: &Raw<'_>, ids: &HashMap<String, u32>, claimed: &mut Vec<u32>) -> Node {
    let tag = tag_code(raw.name).unwrap_or(0);
    let mut attributes: Vec<(u8, Value)> = Vec::new();
    let mut set = |code: u8, value: Value| match attributes.iter_mut().find(|(have, _)| *have == code) {
        Some(slot) => slot.1 = value,
        None => attributes.push((code, value)),
    };
    let mut declared_id = false;
    for (name, value) in &raw.attributes {
        let Some(value) = value else { continue };
        let Some(code) = attribute_code(name) else { continue };
        if code == ID {
            // the first element to declare an id owns it; a twin keeps none
            if let Some(n) = ids.get(value.trim()).filter(|n| !declared_id && !claimed.contains(n)) {
                claimed.push(*n);
                set(code, Value::Id(*n));
            }
            declared_id = true;
            continue;
        }
        if let Some(value) = clean(code, value, ids) {
            set(code, value);
        }
    }
    // a style attribute sets presentation attributes, over the plain ones
    if let Some((_, Some(style))) = raw.attributes.iter().find(|(name, _)| *name == "style") {
        for declaration in style.split(';') {
            let Some((name, value)) = declaration.split_once(':') else { continue };
            let Some(code) = attribute_code(name.trim()).filter(|code| usize::from(*code) >= PRESENTATION) else {
                continue;
            };
            if value.contains('!') {
                continue;
            }
            if let Some(value) = clean(code, value, ids) {
                set(code, value);
            }
        }
    }
    let children = raw.children.iter().filter(|child| kept(child)).map(|child| keep(child, ids, claimed)).collect();
    Node { tag, attributes, children }
}

/// A value the drawing may keep, or nothing. No CSS escape, no call to a
/// function outside [`FUNCTIONS`], no reference outside the drawing.
fn clean(code: u8, value: &str, ids: &HashMap<String, u32>) -> Option<Value> {
    let value = value.trim();
    if value.contains(['\\', '\0']) {
        return None;
    }
    if code == HREF {
        let name = value.strip_prefix('#')?;
        return ids.get(name).map(|n| Value::Href(*n));
    }
    let lower = value.to_ascii_lowercase();
    let mut calls_url = false;
    for (open, _) in lower.match_indices('(') {
        let name_start = lower[..open]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .map_or(0, |index| index + 1);
        let name = &lower[name_start..open];
        if !FUNCTIONS.contains(&name) {
            return None;
        }
        calls_url |= name == "url";
    }
    if !calls_url {
        return Some(Value::Text(value.into()));
    }
    let inner = lower.strip_prefix("url(")?.strip_suffix(')')?;
    let inner = value[4..4 + inner.len()].trim();
    let inner = inner
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| inner.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')))
        .unwrap_or(inner);
    let name = inner.strip_prefix('#')?;
    ids.get(name).map(|n| Value::Url(*n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawing(svg: &str) -> Drawing {
        Drawing::parse(svg.as_bytes()).expect("a drawing")
    }

    /// Every element of the drawing, root first, as `tag name=value …` in
    /// the spelling element 7 wears.
    fn spelled(drawing: &Drawing) -> Vec<String> {
        fn walk(node: &Node, out: &mut Vec<String>) {
            let mut line = TAGS[usize::from(node.tag)].to_string();
            for (code, value) in &node.attributes {
                line.push_str(&format!(" {}={}", ATTRIBUTES[usize::from(*code)], value.spell(7)));
            }
            out.push(line);
            for child in &node.children {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        walk(&drawing.root, &mut out);
        out
    }

    #[test]
    fn a_drawing_keeps_its_shapes_groups_and_paint_servers() {
        let parsed = drawing(
            r##"<?xml version="1.0"?>
            <!-- a comment -->
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" preserveAspectRatio="none">
              <defs>
                <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0.5" stop-color="#123456"/></linearGradient>
                <clipPath id="round"><circle cx="12" cy="12" r="10"/></clipPath>
                <mask id="veil"><rect width="24" height="24" fill="white"/></mask>
              </defs>
              <g transform="translate(1 2)" clip-path="url(#round)" mask="url(#veil)">
                <path d="M0 0L24 24" fill="url(#sky)" stroke-width="2"/>
                <use href="#round"/>
              </g>
            </svg>"##,
        );
        assert_eq!(
            spelled(&parsed),
            [
                "svg viewBox=0 0 24 24 preserveAspectRatio=none",
                "defs",
                "linearGradient id=bunny-7-0 x1=0 y1=0 x2=0 y2=1",
                "stop offset=0.5 stop-color=#123456",
                "clipPath id=bunny-7-1",
                "circle cx=12 cy=12 r=10",
                "mask id=bunny-7-2",
                "rect width=24 height=24 fill=white",
                "g transform=translate(1 2) clip-path=url(#bunny-7-1) mask=url(#bunny-7-2)",
                "path d=M0 0L24 24 fill=url(#bunny-7-0) stroke-width=2",
                "use href=#bunny-7-1",
            ]
        );
    }

    #[test]
    fn a_script_and_everything_in_it_is_dropped() {
        let parsed = drawing(
            r#"<svg><script>alert(1)</script><script href="x.js"/><g><script><![CDATA[alert(2)]]></script><rect width="1"/></g></svg>"#,
        );
        assert_eq!(spelled(&parsed), ["svg", "g", "rect width=1"]);
    }

    #[test]
    fn a_foreign_object_and_its_html_are_dropped() {
        let parsed = drawing(
            r#"<svg><foreignObject width="10" height="10"><div xmlns="http://www.w3.org/1999/xhtml"><img src="x" onerror="alert(1)"/></div></foreignObject><circle r="1"/></svg>"#,
        );
        assert_eq!(spelled(&parsed), ["svg", "circle r=1"]);
    }

    #[test]
    fn a_style_element_is_dropped() {
        let parsed = drawing(r#"<svg><style>path{fill:url(https://evil.test/x)}</style><path d="M0 0"/></svg>"#);
        assert_eq!(spelled(&parsed), ["svg", "path d=M0 0"]);
    }

    #[test]
    fn event_handlers_are_dropped() {
        let parsed = drawing(
            r#"<svg onload="alert(1)" ONLOAD="alert(2)"><rect onclick="alert(3)" onmouseover="x()" onfocusin="y()" width="2"/></svg>"#,
        );
        assert_eq!(spelled(&parsed), ["svg", "rect width=2"]);
    }

    #[test]
    fn an_href_that_leaves_the_drawing_is_dropped() {
        let parsed = drawing(
            r##"<svg><path id="p" d="M0 0"/>
            <use href="javascript:alert(1)"/>
            <use xlink:href="JavaScript:alert(1)"/>
            <use href="https://evil.test/sprite.svg#p"/>
            <use href="//evil.test/sprite.svg#p"/>
            <use href="data:image/svg+xml,&lt;svg onload=alert(1)&gt;"/>
            <use href="sprite.svg#p"/>
            <use href="#missing"/>
            <use xlink:href="#p"/></svg>"##,
        );
        assert_eq!(
            spelled(&parsed),
            ["svg", "path id=bunny-7-0 d=M0 0", "use", "use", "use", "use", "use", "use", "use", "use href=#bunny-7-0"]
        );
    }

    #[test]
    fn a_url_that_leaves_the_drawing_is_dropped() {
        let parsed = drawing(
            r##"<svg><linearGradient id="g"/>
            <rect fill="url(https://evil.test/a.svg#g)" stroke="URL(#g)"/>
            <rect mask="url('//evil.test/m.svg#m')" clip-path="url(&quot;#g&quot;)"/>
            <rect fill="url(#g) red" stroke="url(#nowhere)"/>
            <rect fill="u\72l(https://evil.test/x)" stroke="expression(alert(1))"/>
            <rect fill="var(--x)" stroke="rgb(1, 2, 3)"/></svg>"##,
        );
        assert_eq!(
            spelled(&parsed),
            [
                "svg",
                "linearGradient id=bunny-7-0",
                "rect stroke=url(#bunny-7-0)",
                "rect clip-path=url(#bunny-7-0)",
                "rect",
                "rect",
                "rect stroke=rgb(1, 2, 3)",
            ]
        );
    }

    #[test]
    fn a_style_attribute_keeps_only_clean_presentation() {
        let parsed = drawing(
            r##"<svg><radialGradient id="g"/><path fill="red" style="fill: #00ff00; stroke:url(#g); opacity: .5 !important; mask: url(https://evil.test/m); behavior: url(x.htc); transform: scale(9); -moz-binding: url(x)" d="M0 0"/></svg>"##,
        );
        assert_eq!(
            spelled(&parsed),
            ["svg", "radialGradient id=bunny-7-0", "path fill=#00ff00 d=M0 0 stroke=url(#bunny-7-0)"]
        );
    }

    #[test]
    fn elements_outside_the_table_are_dropped_with_what_they_hold() {
        let parsed = drawing(
            r#"<svg><a href="https://evil.test"><rect width="1"/></a><image href="https://evil.test/x.png"/><animate attributeName="href" to="javascript:alert(1)"/><set attributeName="onload" to="alert(1)"/><filter id="f"><feImage href="https://evil.test"/></filter><text>hi</text><metadata><rect/></metadata><svg><rect width="9"/></svg><iframe src="x"/><rect width="3"/></svg>"#,
        );
        assert_eq!(spelled(&parsed), ["svg", "rect width=3"]);
    }

    #[test]
    fn two_copies_of_one_drawing_never_share_an_id() {
        let parsed = drawing(r##"<svg><clipPath id="c"><rect/></clipPath><g clip-path="url(#c)"/></svg>"##);
        let id = &parsed.root.children[0].attributes[0].1;
        let reference = &parsed.root.children[1].attributes[0].1;
        assert_ne!(id.spell(1), id.spell(2));
        assert_eq!(reference.spell(1), format!("url(#{})", id.spell(1)));
        assert_eq!(reference.spell(2), format!("url(#{})", id.spell(2)));
    }

    #[test]
    fn a_repeated_id_belongs_to_its_first_element() {
        let parsed = drawing(r##"<svg><rect id="a" id="b"/><circle id="a"/><use href="#a"/><use href="#b"/></svg>"##);
        assert_eq!(spelled(&parsed), ["svg", "rect id=bunny-7-0", "circle", "use href=#bunny-7-0", "use"]);
    }

    #[test]
    fn entities_read_as_their_characters_and_unknown_ones_as_nothing() {
        let parsed = drawing(r#"<svg><path d="M0&#32;0&#x20;L1 1" fill="&ns_fill;" stroke="a&amp;b"/></svg>"#);
        assert_eq!(spelled(&parsed), ["svg", "path d=M0 0 L1 1 stroke=a&b"]);
    }

    #[test]
    fn what_is_not_an_svg_is_no_drawing() {
        assert_eq!(Drawing::parse(b"\x89PNG\r\n\x1a\n\xff\xfe"), Err(DrawingError::Encoding));
        assert_eq!(Drawing::parse(b"<html><body/></html>"), Err(DrawingError::NotSvg));
        assert_eq!(Drawing::parse(b"just words"), Err(DrawingError::NotSvg));
        assert_eq!(Drawing::parse(b"<svg><g></svg>"), Err(DrawingError::Markup));
        assert_eq!(Drawing::parse(b"<svg><rect width=1/></svg>"), Err(DrawingError::Markup));
        assert_eq!(Drawing::parse(b"<svg><rect width=\"1/></svg>"), Err(DrawingError::Markup));
        assert_eq!(Drawing::parse(b"<svg/><svg/>"), Err(DrawingError::Markup));
        assert_eq!(Drawing::parse(b"<svg><!-- open"), Err(DrawingError::Markup));
    }

    #[test]
    fn a_doctype_defines_nothing() {
        let parsed = drawing(
            r#"<!DOCTYPE svg [ <!ENTITY evil "<script>alert(1)</script>"> ]><svg><path d="M0 0" fill="&evil;"/></svg>"#,
        );
        assert_eq!(spelled(&parsed), ["svg", "path d=M0 0"]);
    }

    #[test]
    fn a_drawing_too_deep_or_too_large_is_no_drawing() {
        let deep = format!("<svg>{}{}</svg>", "<g>".repeat(MAX_DEPTH), "</g>".repeat(MAX_DEPTH));
        assert_eq!(Drawing::parse(deep.as_bytes()), Err(DrawingError::TooLarge));
        let wide = format!("<svg>{}</svg>", "<rect/>".repeat(MAX_NODES));
        assert_eq!(Drawing::parse(wide.as_bytes()), Err(DrawingError::TooLarge));
    }

    /// On an `<svg>` the root's `width` and `height` are CSS sizes: a
    /// resizable picture leaves them to the layout, one at its own size
    /// keeps them. The root's own id is the element's, never the drawing's.
    #[test]
    fn the_root_keeps_its_size_only_at_its_own_size() {
        let parsed = drawing(r#"<svg id="root" viewBox="0 0 4 2" width="126" height="92" fill="red"><rect/></svg>"#);
        let names = |own_size| -> Vec<&str> {
            parsed.root_attributes(own_size).map(|(code, _)| ATTRIBUTES[usize::from(*code)]).collect()
        };
        assert_eq!(names(true), ["viewBox", "width", "height", "fill"]);
        assert_eq!(names(false), ["viewBox", "fill"]);
    }

    #[test]
    fn a_picture_is_read_once_per_identity() {
        let source = crate::image_engine::ImageSource::bytes_keyed(0xD0D0, b"<svg><rect width=\"1\"/></svg>".to_vec());
        let first = Drawing::of(&source).expect("a drawing");
        let again = Drawing::of(&source).expect("a drawing");
        assert!(Rc::ptr_eq(&first, &again));
        let picture = crate::image_engine::ImageSource::bytes_keyed(0xD0D1, vec![0x89, b'P', b'N', b'G']);
        assert!(Drawing::of(&picture).is_none());
    }
}
