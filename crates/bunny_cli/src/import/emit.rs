//! The page, written as a bunny view.
//!
//! Each layout the browser knows becomes the bunny one that means the
//! same: a flex line a stack, a block a column that fills its width,
//! words and the inline elements around them one paragraph of runs, a
//! grid or a table the stacks its boxes cut into, a box out of the flow
//! an overlay. The box around each view — padding, size, fill, border,
//! corners, shadow — becomes its modifiers. What has no bunny spelling
//! is written down as a gap, beside the element it was found on.

use std::fmt::Write as _;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use super::css::{self, Image, Rgba};
use super::extract::{Kind, Node, Page, Rect};

/// How many stops a bunny ramp holds between its ends (`Stops::MOST`).
const MOST_STOPS: usize = 6;

/// Where the module goes and what it starts with.
#[derive(Clone, Debug)]
pub struct Options {
    /// The `use` line at the top of the module.
    pub prelude: String,
    /// The folder the module is written to: pictures are included from
    /// paths relative to it.
    pub module_dir: PathBuf,
    /// The folder the page was served from: `/assets/x.png` is under it.
    pub site_root: PathBuf,
    /// The page's own folder, for relative picture paths.
    pub page_dir: PathBuf,
    /// The page, as the module's header names it.
    pub source: String,
}

/// Something the page does that the view does not.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Gap {
    /// What, in CSS words: `transform: rotate`, `clip-path`.
    pub feature: String,
    /// The value, or what was drawn instead.
    pub detail: String,
    /// The element, by its `data-fid`.
    pub id: String,
}

/// The module, the names it gave, and what it could not say.
#[derive(Clone, Debug)]
pub struct Converted {
    pub code: String,
    /// Every `data-fid` the view names with `.test_id(…)`.
    pub named: BTreeSet<String>,
    pub gaps: Vec<Gap>,
}

/// Writes the page as a module with `pub fn view() -> impl View`.
#[must_use]
pub fn convert(page: &Page, options: &Options) -> Converted {
    let mut writer = Writer {
        options,
        named: BTreeSet::new(),
        gaps: Vec::new(),
        pictures: BTreeMap::new(),
        root: TextStyle::of(&page.root),
        hug: (false, false),
        container_width: page.width,
    };
    let body = writer.root(&page.root, page.width, page.height);
    let mut code = format!("//! Written by `bunny import html` from {}.\n\n{}\n", options.source, options.prelude);
    for (path, name) in &writer.pictures {
        let _ = write!(code, 
            "\nfn {name}() -> ImageSource {{\n    ImageSource::from_bytes(include_bytes!(\"{path}\").as_slice())\n}}\n"
        );
    }
    let _ = write!(code, "\n#[must_use]\npub fn view() -> impl View {{\n    {}\n}}\n", indent(&body, 1));
    writer.gaps.sort();
    writer.gaps.dedup();
    Converted { code, named: writer.named, gaps: writer.gaps }
}

/// How a box sizes itself on one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Size {
    /// Its content's answer.
    Hug,
    /// What its holder offers, up to a ceiling when it has one.
    Fill(Option<f64>),
    /// This many points.
    Exact(f64),
}

/// The layout a box is a child of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    /// The page itself.
    Root,
    /// Block flow: boxes one under the other, each as wide as the line.
    Block,
    /// A flex row; `stretch` when the row stretches its children's height.
    Row { stretch: bool },
    /// A flex column; `stretch` when it stretches its children's width.
    Column { stretch: bool },
    /// A grid or table cell: the box the browser gave it.
    Cell,
    /// Out of the flow, over its holder.
    Layer,
}

/// An edge on one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Start,
    Center,
    End,
}

const fn alignment(horizontal: Edge, vertical: Edge) -> &'static str {
    match (vertical, horizontal) {
        (Edge::Start, Edge::Start) => "Alignment::TopLeading",
        (Edge::Start, Edge::Center) => "Alignment::Top",
        (Edge::Start, Edge::End) => "Alignment::TopTrailing",
        (Edge::Center, Edge::Start) => "Alignment::Leading",
        (Edge::Center, Edge::Center) => "Alignment::Center",
        (Edge::Center, Edge::End) => "Alignment::Trailing",
        (Edge::End, Edge::Start) => "Alignment::BottomLeading",
        (Edge::End, Edge::Center) => "Alignment::Bottom",
        (Edge::End, Edge::End) => "Alignment::BottomTrailing",
    }
}

/// The text a box's words are written in — what a view says only when it
/// differs from the page's.
#[derive(Clone, Debug, PartialEq)]
struct TextStyle {
    size: f64,
    weight: u16,
    italic: bool,
    color: Option<Rgba>,
    family: String,
    tracking: f64,
    line_height: Option<f64>,
    transform: String,
    underline: bool,
    strike: bool,
    ground: Option<Rgba>,
}

impl TextStyle {
    fn of(node: &Node) -> Self {
        let decoration = node.css("text-decoration-line");
        let ground = css::color(node.css("background-color")).filter(|ground| !ground.invisible());
        Self {
            size: node.px("font-size"),
            weight: node.css("font-weight").parse().unwrap_or(400),
            italic: node.css("font-style") != "normal" && !node.css("font-style").is_empty(),
            // the ink the letters are painted in: their fill, which is the
            // color unless the page set it apart; a fill with no alpha
            // shows what the box cuts out with them, a ramp
            color: css::color(node.css("-webkit-text-fill-color"))
                .or_else(|| css::color(node.css("color")))
                .filter(|fill| fill.a > 0.0),
            family: node.css("font-family").split(',').next().unwrap_or("").trim().trim_matches('"').to_owned(),
            tracking: node.px("letter-spacing"),
            line_height: css::px(node.css("line-height")),
            transform: node.css("text-transform").to_owned(),
            underline: decoration.contains("underline"),
            strike: decoration.contains("line-through"),
            ground: if node.css("display").starts_with("inline") && node.css("display") == "inline" { ground } else { None },
        }
    }
}

const fn weight(weight: u16) -> &'static str {
    match weight {
        0..=150 => "Weight::Thin",
        151..=250 => "Weight::ExtraLight",
        251..=350 => "Weight::Light",
        351..=450 => "Weight::Regular",
        451..=550 => "Weight::Medium",
        551..=650 => "Weight::Semibold",
        651..=750 => "Weight::Bold",
        751..=850 => "Weight::ExtraBold",
        _ => "Weight::Black",
    }
}

/// A number as Rust writes an `f64`: `12.0`, `0.5`, four decimals at most
/// — a grid of equal tracks adds each one's rounding up across the row,
/// and the browser keeps its geometry in sixty-fourths.
fn num(value: f64) -> String {
    let rounded = (value * 10_000.0).round() / 10_000.0;
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    let text = format!("{rounded}");
    if text.contains('.') || text.contains('e') || text.contains("inf") { text } else { format!("{text}.0") }
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(character),
        }
    }
    out.push('"');
    out
}

fn indent(code: &str, levels: usize) -> String {
    code.replace('\n', &format!("\n{}", "    ".repeat(levels)))
}

/// A stack macro around its children, one per line.
fn stack(name: &str, children: &[String]) -> String {
    let mut code = format!("{name}!(");
    for child in children {
        code.push_str("\n    ");
        code.push_str(&indent(child, 1));
        code.push(',');
    }
    code.push_str("\n)");
    code
}

fn transformed(text: &str, transform: &str) -> String {
    match transform {
        "uppercase" => text.to_uppercase(),
        "lowercase" => text.to_lowercase(),
        "capitalize" => text
            .split(' ')
            .map(|word| {
                let mut letters = word.chars();
                letters.next().map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect())
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => text.to_owned(),
    }
}

/// Collapses white space the way `white-space: normal` lays it out.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            space = true;
        } else {
            if space {
                out.push(' ');
            }
            space = false;
            out.push(character);
        }
    }
    if space {
        out.push(' ');
    }
    out
}

/// What a pseudo-element's `content` writes: a quoted string, or nothing
/// when it is a counter or an attribute the page alone can resolve.
fn pseudo_text(content: &str) -> Option<String> {
    let content = content.trim();
    let inner = content.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// A box laid inside a line of words (`inline-block` and its kin, an
/// inline picture) rather than under it.
fn is_inline_box(node: &Node) -> bool {
    node.kind == Kind::Element && !is_layer(node) && (node.css("display").starts_with("inline-") || (node.tag == "img" && node.css("display") == "inline"))
}

/// The children of a block that share a line with words: a run of inline
/// items and inline boxes that holds both words and a box. An inline box
/// with no words beside it stays a box of the block, as it was.
fn shares_words(children: &[&Node]) -> Vec<bool> {
    let mut shares = vec![false; children.len()];
    let mut start = 0;
    while start < children.len() {
        let on_line = |child: &Node| is_inline(child) || is_inline_box(child);
        if !on_line(children[start]) {
            start += 1;
            continue;
        }
        let end = (start..children.len()).find(|at| !on_line(children[*at])).unwrap_or(children.len());
        let run = &children[start..end];
        let words = run.iter().any(|item| item.kind == Kind::Text && !item.text.trim().is_empty());
        let boxes = run.iter().any(|item| !is_inline(item));
        if words && boxes {
            shares[start..end].iter_mut().for_each(|share| *share = true);
        }
        start = end;
    }
    shares
}

fn is_layer(node: &Node) -> bool {
    matches!(node.css("position"), "absolute" | "fixed")
}

fn is_inline(node: &Node) -> bool {
    match node.kind {
        Kind::Text | Kind::Space => true,
        Kind::Before | Kind::After => node.css("display") == "inline" && !is_layer(node),
        Kind::Element => {
            // an inline block that holds words alone and names no size of
            // its own reads as a run of its line: bunny's paragraphs hold
            // runs, not boxes
            let text_block = node.css("display") == "inline-block"
                && css::px(node.size("width")).is_none()
                && css::px(node.size("height")).is_none();
            (node.css("display") == "inline" || text_block)
                && !matches!(node.tag.as_str(), "img" | "svg" | "input" | "select" | "textarea" | "button" | "br" | "progress" | "meter" | "video" | "canvas" | "iframe")
                && node.children.iter().all(|child| is_inline(child) || is_layer(child))
        }
    }
}

struct Writer<'a> {
    options: &'a Options,
    named: BTreeSet<String>,
    gaps: Vec<Gap>,
    /// Pictures by their path from the module, and the function that
    /// reads each.
    pictures: BTreeMap<String, String>,
    /// The page's own text: a view says only what differs.
    root: TextStyle,
    /// Whether the box whose children are being written hugs its content
    /// on each axis: a child that fills such a box takes the size the
    /// browser resolved instead, or its appetite would grow the box.
    hug: (bool, bool),
    /// The width of the content box whose children are being written.
    container_width: f64,
}

impl Writer<'_> {
    fn gap(&mut self, node: &Node, feature: &str, detail: impl Into<String>) {
        self.gaps.push(Gap { feature: feature.to_owned(), detail: detail.into(), id: node.id.clone() });
    }

    /// The page: the body's content in a box the viewport's size, in the
    /// page's face, size and ink.
    fn root(&mut self, body: &Node, width: f64, height: f64) -> String {
        let mut view = self.element(body, Flow::Root).unwrap_or_else(|| "spacer()".to_owned());
        let _ = write!(view, "\n.frame_aligned({}, {}, Alignment::TopLeading)", num(width), num(height));
        if !self.root.family.is_empty() {
            let _ = write!(view, "\n.font_family({})", quote(&self.root.family));
        }
        let _ = write!(view, "\n.font_size({})", num(self.root.size));
        if let Some(color) = self.root.color {
            let _ = write!(view, "\n.foreground_color({})", color.code());
        }
        if self.root.weight != 400 {
            let _ = write!(view, "\n.font_weight({})", weight(self.root.weight));
        }
        view
    }

    /// How the box sizes itself on each axis, in the layout it is in.
    fn sizing(&mut self, node: &Node, flow: Flow) -> (Size, Size) {
        let container = self.container_width;
        let ceiling = |value: &str| css::px(value).or_else(|| css::percent(value).map(|share| share * container));
        let axis = |specified: &str, measured: f64| -> Option<Size> {
            if css::px(specified).is_some() || css::percent(specified).is_some() || specified.starts_with("calc") {
                Some(Size::Exact(measured))
            } else {
                None
            }
        };
        let (width_spec, height_spec) = (node.size("width").to_owned(), node.size("height").to_owned());
        if css::percent(&width_spec).is_some() {
            self.gap(node, "width: %", format!("{width_spec} written as {} points", num(node.rect.w)));
        }
        let grows = node.css("flex-grow").parse::<f64>().unwrap_or(0.0) > 0.0;
        let max_width = ceiling(node.size("max-width"));
        let max_height = ceiling(node.size("max-height"));
        let fill_width = Size::Fill(max_width);
        let fill_height = Size::Fill(max_height);
        let inline_level = node.css("display").starts_with("inline");
        let pinned = matches!(node.css("align-self"), "center" | "flex-start" | "flex-end");
        let width = axis(&width_spec, node.rect.w).unwrap_or_else(|| match flow {
            Flow::Root | Flow::Cell | Flow::Layer => Size::Exact(node.rect.w),
            Flow::Block if !inline_level && node.css("float") == "none" => fill_width,
            Flow::Row { .. } if grows => fill_width,
            Flow::Column { stretch: true } if !pinned => fill_width,
            Flow::Block | Flow::Row { .. } | Flow::Column { .. } => Size::Hug,
        });
        // a ratio decides the axis its box leaves open: the page resolved
        // it, and a picture that fills its box must be told
        let ratio = !matches!(node.css("aspect-ratio"), "" | "auto");
        let width = match width {
            Size::Hug if ratio => Size::Exact(node.rect.w),
            other => other,
        };
        let baseline = node.css("align-self") == "baseline";
        let height = axis(&height_spec, node.rect.h).or_else(|| ratio.then_some(Size::Exact(node.rect.h))).unwrap_or(match flow {
            Flow::Root | Flow::Cell | Flow::Layer => Size::Exact(node.rect.h),
            Flow::Row { stretch: true } if !pinned && !baseline => fill_height,
            Flow::Column { .. } if grows => fill_height,
            Flow::Block | Flow::Row { .. } | Flow::Column { .. } => Size::Hug,
        });
        // a box that hugs keeps its floor (`frame_min`); one that fills
        // has none to keep yet
        if node.size("min-width") != "auto" && node.size("min-width") != "0px" && matches!(width, Size::Fill(_)) {
            self.gap(node, "min-width", format!("{} on a box that fills", node.size("min-width")));
        }
        // a box that hugs its content is as wide as the browser resolved
        // it: a child that fills it takes that width
        let width = match width {
            Size::Fill(_) if self.hug.0 => Size::Exact(node.rect.w),
            other => other,
        };
        let height = match height {
            Size::Fill(_) if self.hug.1 => Size::Exact(node.rect.h),
            other => other,
        };
        (width, height)
    }

    /// The floors of a box that hugs its content (`min-width`,
    /// `min-height`), at the border edge its frame wraps; zero where it
    /// has none or does not hug.
    fn floors(&self, node: &Node, width: Size, height: Size) -> (f64, f64) {
        let content_box = node.css("box-sizing") == "content-box";
        let floor = |property: &str, hugs: bool, insets: f64, room: Option<f64>| {
            let value = node.size(property);
            let length = css::px(value).or_else(|| css::percent(value).and_then(|share| room.map(|room| share * room))).unwrap_or(0.0);
            if hugs && length > 0.0 { length + if content_box { insets } else { 0.0 } } else { 0.0 }
        };
        let across = node.px("padding-left") + node.px("padding-right") + node.px("border-left-width") + node.px("border-right-width");
        let down = node.px("padding-top") + node.px("padding-bottom") + node.px("border-top-width") + node.px("border-bottom-width");
        (
            floor("min-width", matches!(width, Size::Hug), across, Some(self.container_width)),
            floor("min-height", matches!(height, Size::Hug), down, None),
        )
    }

    /// Where the box's content sits in it when the box is bigger.
    fn content_alignment(node: &Node) -> (Edge, Edge) {
        let edge = |value: &str| match value {
            "center" | "safe center" => Edge::Center,
            "flex-end" | "end" | "right" | "bottom" => Edge::End,
            _ => Edge::Start,
        };
        let display = node.css("display");
        if display.ends_with("flex") {
            let main = edge(node.css("justify-content"));
            let cross = edge(node.css("align-items"));
            if node.css("flex-direction").starts_with("column") { (cross, main) } else { (main, cross) }
        } else {
            let text = match node.css("text-align") {
                "center" | "-webkit-center" => Edge::Center,
                "right" | "end" | "-webkit-right" => Edge::End,
                _ => Edge::Start,
            };
            (text, Edge::Start)
        }
    }

    /// One element, as a view with its box — `None` when it draws nothing.
    fn element(&mut self, node: &Node, flow: Flow) -> Option<String> {
        if node.css("display") == "none" {
            return None;
        }
        let (width, height) = self.sizing(node, flow);
        let content = self.content(node, width, height);
        Some(self.dressed(node, content, width, height))
    }

    /// The box around a view: padding, size, fill, corners, border,
    /// shadow, and what the box does to everything inside.
    fn dressed(&mut self, node: &Node, content: String, width: Size, height: Size) -> String {
        let mut view = content;
        let control = matches!(node.tag.as_str(), "input" | "textarea") && node.css("appearance") != "none";
        // the padding, and the border's own width: a CSS border takes room,
        // bunny's line is drawn over the box
        let sides = ["top", "right", "bottom", "left"];
        let inset: Vec<f64> = sides
            .iter()
            .map(|side| node.px(&format!("padding-{side}")) + node.px(&format!("border-{side}-width")))
            .collect();
        if !control {
            view.push_str(&padding(&inset));
        }
        let (horizontal, vertical) = Self::content_alignment(node);
        view.push_str(&frame(width, height, alignment(horizontal, vertical)));
        let (floor_w, floor_h) = self.floors(node, width, height);
        if floor_w > 0.0 || floor_h > 0.0 {
            let _ = write!(view, "\n.frame_min({}, {}, {})", num(floor_w), num(floor_h), alignment(horizontal, vertical));
        }
        self.paint(node, &mut view);
        self.layers(node, &mut view);
        if node.kind == Kind::Element && !node.id.is_empty() {
            let _ = write!(view, "\n.test_id({})", quote(&node.id));
            self.named.insert(node.id.clone());
        }
        // turned and moved off its place — a transform, a relative
        // position — after its name, so the name is found where the box
        // is drawn
        if let Some(turn) = rotation(node) {
            view.push_str(&turn);
        }
        let (mut dx, mut dy) = css::transform(node.css("transform")).ok().flatten().map_or((0.0, 0.0), |matrix| (matrix.e, matrix.f));
        // a box drawn under its clip at a scrolled position: its content
        // moved by the scroll (bunny's clip does not scroll)
        if node.css("position") == "relative" {
            dx += node.px("left");
            dy += node.px("top");
        }
        if dx.abs() > 1e-9 || dy.abs() > 1e-9 {
            view = with_offset(&view, dx, dy);
        }
        if matches!(node.tag.as_str(), "button" | "a") || node.css("cursor") == "pointer" {
            view.push_str("\n.on_click(|| {})");
        }
        view
    }

    /// The box's paint: its ground, corners, border, shadow, opacity, the
    /// cut of its overflow, and the filters over it.
    fn paint(&mut self, node: &Node, view: &mut String) {
        let ground = css::color(node.css("background-color")).filter(|ground| !ground.invisible());
        let images = css::images(node.css("background-image"));
        let rect = node.rect;
        if node.css("background-clip") == "text" {
            // the box's ground cut to its text: the letters' ink
            match images.first() {
                Some(Image::Url(url)) => self.gap(node, "background-clip: text", format!("a picture cut to the text: {url}")),
                Some(layer) => {
                    if let Some(ramp) = self.ramp_code(node, layer) {
                        let _ = write!(view, "\n.foreground_gradient({ramp})");
                    }
                }
                None => {
                    if let Some(ground) = ground {
                        let _ = write!(view, "\n.foreground_color({})", ground.code());
                    }
                }
            }
        } else {
            if let Some(ground) = ground {
                let _ = write!(view, "\n.background_color({})", ground.code());
            }
            for layer in images.iter().rev() {
                if let Image::Url(url) = layer {
                    match self.picture(url) {
                        Some(picture) => {
                            let _ = write!(view, "\n.background(UnitPoint::CENTER, image({picture}()).resizable().aspect_ratio(ContentMode::Fill).clipped())");
                        }
                        None => self.gap(node, "background-image: url", url.clone()),
                    }
                } else if let Some(ramp) = self.ramp_code(node, layer) {
                    let _ = write!(view, "\n.background_gradient({ramp})");
                }
            }
        }
        if images.len() > 1 {
            self.gap(node, "background-image: layers", format!("{} layers", images.len()));
        }
        let corners: Vec<f64> = ["top-left", "top-right", "bottom-right", "bottom-left"]
            .iter()
            .map(|corner| {
                let value = node.css(&format!("border-{corner}-radius"));
                let first = value.split_whitespace().next().unwrap_or("0");
                css::px(first).or_else(|| css::percent(first).map(|share| share * rect.w.min(rect.h))).unwrap_or(0.0).min(rect.w.min(rect.h) / 2.0)
            })
            .collect();
        if corners.iter().any(|radius| *radius > 0.0) {
            if corners.iter().all(|radius| (radius - corners[0]).abs() < 0.01) {
                let _ = write!(view, "\n.corner_radius({})", num(corners[0]));
            } else {
                let _ = write!(view, 
                    "\n.corner_radius(Corners {{ top_left: {}, top_right: {}, bottom_right: {}, bottom_left: {} }})",
                    num(corners[0]),
                    num(corners[1]),
                    num(corners[2]),
                    num(corners[3])
                );
            }
        }
        self.border(node, view);
        for shadow in css::shadows(node.css("box-shadow")).iter().take(1) {
            if shadow.inset {
                self.gap(node, "box-shadow: inset", node.css("box-shadow").to_owned());
                continue;
            }
            if shadow.spread.abs() > 1e-9 {
                self.gap(node, "box-shadow: spread", num(shadow.spread));
            }
            let _ = write!(view, "\n.shadow_color({}, {})", num(shadow.blur.max(1.0)), shadow.color.code());
            if shadow.x.abs() > 1e-9 || shadow.y.abs() > 1e-9 {
                let _ = write!(view, "\n.shadow_offset({}, {})", num(shadow.x), num(shadow.y));
            }
        }
        if css::shadows(node.css("box-shadow")).len() > 1 {
            self.gap(node, "box-shadow: layers", node.css("box-shadow").to_owned());
        }
        if node.css("overflow-x") != "visible" && !node.css("overflow-x").is_empty() || node.css("overflow-y") != "visible" && !node.css("overflow-y").is_empty() {
            view.push_str("\n.clipped()");
            if node.css("overflow-x") == "auto" || node.css("overflow-x") == "scroll" || node.css("overflow-y") == "auto" || node.css("overflow-y") == "scroll" {
                self.gap(node, "overflow: scroll", "drawn as a clip");
            }
        }
        self.filters(node, view);
        let opacity = css::number(node.css("opacity")).unwrap_or(1.0);
        if opacity < 1.0 {
            let _ = write!(view, "\n.opacity({})", num(opacity));
        }
        if node.css("visibility") == "hidden" {
            view.push_str("\n.hidden()");
        }
    }

    /// A gradient layer as bunny writes it (a picture is no ramp).
    fn ramp_code(&mut self, node: &Node, layer: &Image) -> Option<String> {
        match layer {
            Image::Linear { degrees, stops, repeating } => {
                if *repeating {
                    self.gap(node, "repeating-linear-gradient", "drawn once");
                }
                // the CSS gradient line runs through the centre, as long as
                // the box's corners need
                let radians = degrees.to_radians();
                let (dx, dy) = (radians.sin(), -radians.cos());
                let (w, h) = (node.rect.w.max(1.0), node.rect.h.max(1.0));
                let length = (w * dx).abs() + (h * dy).abs();
                let ramp = ramp(stops, length)?;
                let (sx, sy) = (0.5 - dx * length / 2.0 / w, 0.5 - dy * length / 2.0 / h);
                let (ex, ey) = (0.5 + dx * length / 2.0 / w, 0.5 + dy * length / 2.0 / h);
                self.crowded(node, &ramp);
                Some(format!(
                    "Gradient::linear({}, {}).direction(UnitPoint {{ x: {}, y: {} }}, UnitPoint {{ x: {}, y: {} }}){}",
                    ramp.from.code(),
                    ramp.to.code(),
                    num(sx),
                    num(sy),
                    num(ex),
                    num(ey),
                    ramp.stops_code()
                ))
            }
            Image::Radial { shape, stops, repeating } => {
                if *repeating {
                    self.gap(node, "repeating-radial-gradient", "drawn once");
                }
                if !shape.is_empty() && shape != "circle" && !shape.starts_with("circle at center") {
                    self.gap(node, "radial-gradient: shape", shape.clone());
                }
                // the rings reach the farthest corner from the centre
                let ramp = ramp(stops, (node.rect.w / 2.0).hypot(node.rect.h / 2.0))?;
                self.crowded(node, &ramp);
                Some(format!("Gradient::radial({}, {}){}", ramp.from.code(), ramp.to.code(), ramp.stops_code()))
            }
            Image::Conic => {
                self.gap(node, "conic-gradient", "not drawn");
                None
            }
            Image::Url(_) => None,
        }
    }

    /// A ramp with more stops than bunny holds keeps the ones that shape
    /// it most.
    fn crowded(&mut self, node: &Node, ramp: &Ramp) {
        if ramp.stops.len() > MOST_STOPS {
            self.gap(node, "gradient: stops", format!("{} stops between the ends; the {MOST_STOPS} that shape it most are kept", ramp.stops.len()));
        }
    }

    fn border(&mut self, node: &Node, view: &mut String) {
        let sides = ["top", "right", "bottom", "left"];
        let width: Vec<f64> = sides.iter().map(|side| node.px(&format!("border-{side}-width"))).collect();
        let style: Vec<&str> = sides.iter().map(|side| node.css(&format!("border-{side}-style"))).collect();
        let color: Vec<Option<Rgba>> = sides.iter().map(|side| css::color(node.css(&format!("border-{side}-color")))).collect();
        let drawn = |index: usize| width[index] > 0.0 && style[index] != "none" && style[index] != "hidden";
        if !(0..4).any(drawn) {
            return;
        }
        let uniform = (0..4).all(|index| drawn(index) && (width[index] - width[0]).abs() < 1e-9 && style[index] == style[0] && color[index] == color[0]);
        if uniform {
            if let Some(ink) = color[0] {
                let _ = write!(view, "\n.border({}, {})", ink.code(), num(width[0]));
                match style[0] {
                    "dashed" => view.push_str("\n.border_style(BorderStyle::Dashed)"),
                    "dotted" => view.push_str("\n.border_style(BorderStyle::Dotted)"),
                    "solid" => {}
                    other => self.gap(node, "border-style", other.to_owned()),
                }
            }
            return;
        }
        // one side at a time: a rule along that edge, over the box
        for (index, side) in sides.iter().enumerate() {
            if !drawn(index) {
                continue;
            }
            let Some(ink) = color[index] else { continue };
            let (point, rule) = match *side {
                "top" => ("TOP", format!("spacer().frame_height({})", num(width[index]))),
                "bottom" => ("BOTTOM", format!("spacer().frame_height({})", num(width[index]))),
                "left" => ("LEADING", format!("spacer().frame_width({})", num(width[index]))),
                _ => ("TRAILING", format!("spacer().frame_width({})", num(width[index]))),
            };
            let _ = write!(view, "\n.overlay(UnitPoint::{point}, {rule}.background_color({}))", ink.code());
            if style[index] != "solid" {
                self.gap(node, "border-style: one side", style[index].to_owned());
            }
        }
    }

    fn filters(&mut self, node: &Node, view: &mut String) {
        for (property, modifier) in [("filter", "blur"), ("backdrop-filter", "backdrop_blur")] {
            let value = node.css(property);
            if value.is_empty() || value == "none" {
                continue;
            }
            for function in css::split_top(value, ' ') {
                if let Some(radius) = function.strip_prefix("blur(").and_then(|rest| rest.strip_suffix(')')).and_then(css::px) {
                    let _ = write!(view, "\n.{modifier}({})", num(radius));
                } else {
                    let name = function.split('(').next().unwrap_or(function);
                    self.gap(node, &format!("{property}: {name}"), function.to_owned());
                }
            }
        }
        for (property, quiet) in [("mix-blend-mode", "normal"), ("clip-path", "none"), ("mask-image", "none"), ("background-blend-mode", "normal")] {
            let value = node.css(property);
            if !value.is_empty() && value != quiet {
                self.gap(node, property, value.to_owned());
            }
        }
        match css::transform(node.css("transform")) {
            Ok(None) => {}
            Ok(Some(matrix)) => {
                let (sx, sy) = matrix.scale();
                if (sx - 1.0).abs() > 0.001 || (sy - 1.0).abs() > 0.001 {
                    self.gap(node, "transform: scale", format!("{} {}", num(sx), num(sy)));
                }
                if matrix.shear().abs() > 0.001 {
                    self.gap(node, "transform: skew", node.css("transform").to_owned());
                }
                // a turn and a translation are the box's
                // `.rotation_effect` and `.offset`, written after its name
                // (`Writer::dressed`)
            }
            Err(text) => self.gap(node, "transform: 3d", text),
        }
        if node.css("outline-style") != "none" && !node.css("outline-style").is_empty() && node.px("outline-width") > 0.0 {
            self.gap(node, "outline", format!("{} offset {}", node.css("outline-width"), node.css("outline-offset")));
        }
        if node.css("text-shadow") != "none" && !node.css("text-shadow").is_empty() {
            self.gap(node, "text-shadow", node.css("text-shadow").to_owned());
        }
        if node.css("writing-mode") != "horizontal-tb" && !node.css("writing-mode").is_empty() {
            self.gap(node, "writing-mode", node.css("writing-mode").to_owned());
        }
        if node.css("float") != "none" && !node.css("float").is_empty() {
            self.gap(node, "float", node.css("float").to_owned());
        }
        if node.css("column-count") != "auto" && !node.css("column-count").is_empty() {
            self.gap(node, "column-count", node.css("column-count").to_owned());
        }
        if node.css("position") == "sticky" {
            self.gap(node, "position: sticky", "drawn in the flow");
        }

    }

    /// The boxes out of the flow — positioned children, and pseudo-boxes
    /// positioned over the box — as overlays at their place.
    fn layers(&mut self, node: &Node, view: &mut String) {
        for child in &node.children {
            if !is_layer(child) {
                continue;
            }
            let (offset, layer) = match child.kind {
                Kind::Element => {
                    let rect = child.rect;
                    let Some(layer) = self.element(child, Flow::Layer) else { continue };
                    ((rect.x - node.rect.x, rect.y - node.rect.y), layer)
                }
                Kind::Before | Kind::After => self.pseudo_layer(node, child),
                _ => continue,
            };
            // the layer sits at its holder's corner, offset to its place:
            // the offset goes before the name, so the box is found where
            // it is drawn
            let (x, y) = offset;
            let placed = if x.abs() < 1e-9 && y.abs() < 1e-9 {
                layer
            } else {
                with_offset(&layer, x, y)
            };
            let _ = write!(view, "\n.overlay(UnitPoint::TOP_LEADING, {})", indent(&placed, 1));
        }
    }

    /// A positioned `::before`/`::after`: where it stands in its element,
    /// and the box it draws.
    fn pseudo_layer(&mut self, node: &Node, pseudo: &Node) -> ((f64, f64), String) {
        let width = css::px(pseudo.sizes.get("width").map_or("", String::as_str)).unwrap_or(0.0);
        let height = css::px(pseudo.sizes.get("height").map_or("", String::as_str)).unwrap_or(0.0);
        let x = css::px(pseudo.css("left")).or_else(|| css::px(pseudo.css("right")).map(|right| node.rect.w - right - width)).unwrap_or(0.0);
        let y = css::px(pseudo.css("top")).or_else(|| css::px(pseudo.css("bottom")).map(|bottom| node.rect.h - bottom - height)).unwrap_or(0.0);
        let mut box_node = pseudo.clone();
        box_node.rect = Rect { x: node.rect.x + x, y: node.rect.y + y, w: width, h: height };
        let content = match pseudo_text(pseudo.css("content")) {
            Some(words) if !words.is_empty() => self.words_view(&box_node, &words),
            Some(_) => "spacer()".to_owned(),
            None => {
                self.gap(node, "content: counter/attr", pseudo.css("content").to_owned());
                "spacer()".to_owned()
            }
        };
        let view = self.dressed(&box_node, content, Size::Exact(width), Size::Exact(height));
        ((x, y), view)
    }

    /// The box's content: a picture, a control, a paragraph, or the
    /// layout of its children.
    fn content(&mut self, node: &Node, width: Size, height: Size) -> String {
        match node.tag.as_str() {
            "img" => return self.image(node),
            "input" | "textarea" if node.css("appearance") != "none" || node.tag == "textarea" => return self.control(node),
            "select" => {
                self.gap(node, "select", "drawn as its label");
                let label = node.attr("label").unwrap_or_default().to_owned();
                return self.words_view(node, &label);
            }
            "progress" | "meter" => return self.meter(node),
            "svg" | "video" | "canvas" | "iframe" => {
                self.gap(node, &node.tag.clone(), "drawn as an empty box");
                return "spacer()".to_owned();
            }
            _ => {}
        }
        let display = node.css("display");
        let in_flow: Vec<&Node> = node.children.iter().filter(|child| !is_layer(child)).collect();
        let saved = (self.hug, self.container_width);
        self.hug = (width == Size::Hug, height == Size::Hug);
        self.container_width = content_width(node);
        let view = if display.ends_with("flex") {
            self.flex(node, &in_flow)
        } else if display.ends_with("grid") || display.starts_with("table") || display == "inline-table" {
            self.cut(node, &in_flow)
        } else {
            self.block(node, &in_flow, width, height)
        };
        (self.hug, self.container_width) = saved;
        match node.attr("scroll").map(|scroll| scroll.split(' ').filter_map(css::number).collect::<Vec<_>>()) {
            Some(scroll) if scroll.len() == 2 && (scroll[0] != 0.0 || scroll[1] != 0.0) => {
                self.gap(node, "scroll position", format!("drawn scrolled by {} {}", num(scroll[0]), num(scroll[1])));
                with_offset(&view, -scroll[0], -scroll[1])
            }
            _ => view,
        }
    }

    /// Block flow: the words and inline boxes of a line as one paragraph,
    /// the block boxes one under the other.
    fn block(&mut self, node: &Node, children: &[&Node], width: Size, height: Size) -> String {
        if children.iter().all(|child| child.kind == Kind::Space) {
            return empty(node, width, height);
        }
        if children.iter().all(|child| is_inline(child)) {
            return self.paragraph(node, children).unwrap_or_else(|| empty(node, width, height));
        }
        // runs of inline content become paragraphs of their own, between
        // the blocks
        let mut views = Vec::new();
        let mut line: Vec<&Node> = Vec::new();
        let content_top = node.rect.y + node.px("padding-top") + node.px("border-top-width");
        let content_left = node.rect.x + node.px("padding-left") + node.px("border-left-width");
        let content_right = node.rect.right() - node.px("padding-right") - node.px("border-right-width");
        let mut previous_bottom = content_top;
        let shares = shares_words(children);
        let flush = |writer: &mut Self, line: &mut Vec<&Node>, views: &mut Vec<String>, previous_bottom: &mut f64| {
            if line.iter().all(|item| item.kind == Kind::Space) {
                line.clear();
                return;
            }
            if line.iter().any(|item| !is_inline(item)) {
                if let Some(row) = writer.mixed_line(node, line) {
                    *previous_bottom = previous_bottom.max(line.iter().map(|item| item.rect.bottom()).fold(f64::NEG_INFINITY, f64::max));
                    views.push(row);
                }
                line.clear();
                return;
            }
            // the next block stands under the line's lowest words
            let bottom = line
                .iter()
                .filter(|item| matches!(item.kind, Kind::Text | Kind::Element) && item.rect.h > 0.0)
                .map(|item| item.rect.bottom())
                .fold(f64::NEG_INFINITY, f64::max);
            if bottom.is_finite() {
                *previous_bottom = previous_bottom.max(bottom);
            }
            if let Some(paragraph) = writer.paragraph(node, line) {
                let edge = match Self::content_alignment(node).0 {
                    Edge::Center => "Center",
                    Edge::End => "Trailing",
                    Edge::Start => "Leading",
                };
                views.push(if writer.hug.0 {
                    format!("{paragraph}\n.frame_width_aligned({}, Alignment::{edge})", num(content_width(node)))
                } else {
                    format!("{paragraph}\n.frame_max(f64::INFINITY, f64::MAX, Alignment::{edge})")
                });
            }
            line.clear();
        };
        for (child, shares) in children.iter().zip(shares) {
            if is_inline(child) || shares {
                line.push(child);
                continue;
            }
            let after_line = line.iter().any(|item| item.kind != Kind::Space);
            flush(self, &mut line, &mut views, &mut previous_bottom);
            let Some(mut view) = self.block_child(child) else { continue };
            let rect = child_rect(node, child);
            // under a line of words the gap is the block's own margin: the
            // words' boxes are shorter than the line they stand in
            let top = if after_line { child.px("margin-top") } else { rect.y - previous_bottom };
            if top.abs() > 0.25 && child.kind == Kind::Element {
                let _ = write!(view, "\n.padding_edge(Edge::Top, {})", num(top));
            }
            let leading = rect.x - content_left;
            if leading.abs() > 0.25 && child.kind == Kind::Element {
                let _ = write!(view, "\n.padding_edge(Edge::Leading, {})", num(leading));
            }
            let trailing = content_right - rect.right();
            if trailing.abs() > 0.25 && child.kind == Kind::Element && matches!(self.sizing(child, Flow::Block).0, Size::Fill(_)) {
                let _ = write!(view, "\n.padding_edge(Edge::Trailing, {})", num(trailing));
            }
            if child.kind == Kind::Element {
                previous_bottom = rect.bottom();
            }
            views.push(view);
        }
        flush(self, &mut line, &mut views, &mut previous_bottom);
        if views.len() == 1 {
            return views.pop().unwrap_or_default();
        }
        format!("{}\n.alignment(HorizontalAlignment::Leading)", stack("vstack", &views))
    }

    /// A line of words with boxes in it: a row of its runs of words and
    /// its boxes, each where the browser set it on the line — its offset
    /// from the line's top, its gap from the item before.
    fn mixed_line(&mut self, node: &Node, line: &[&Node]) -> Option<String> {
        let line_height = css::px(node.css("line-height"));
        // a run of words stands in its line box: the words' own box, grown
        // by the half-leading on each side
        let top_of = |item: &Node| match (item.kind, line_height) {
            (Kind::Text, Some(height)) => item.rect.y - (height - item.rect.h) / 2.0,
            _ => item.rect.y,
        };
        let mut segments: Vec<(f64, f64, f64, String)> = Vec::new();
        let mut words: Vec<&Node> = Vec::new();
        let close = |writer: &mut Self, words: &mut Vec<&Node>, segments: &mut Vec<(f64, f64, f64, String)>| {
            let shown: Vec<&&Node> = words.iter().filter(|item| item.kind != Kind::Space && item.rect.w > 0.0).collect();
            if let (Some(first), Some(paragraph)) = (shown.first(), writer.paragraph(node, words)) {
                let left = shown.iter().map(|item| item.rect.x).fold(f64::INFINITY, f64::min);
                let right = shown.iter().map(|item| item.rect.right()).fold(f64::NEG_INFINITY, f64::max);
                segments.push((left, right, top_of(first), paragraph));
            }
            words.clear();
        };
        for item in line {
            if is_inline(item) {
                words.push(item);
                continue;
            }
            close(self, &mut words, &mut segments);
            // the box's own edges: its margins are the gaps around it
            if let Some(view) = self.element(item, Flow::Row { stretch: false }) {
                segments.push((item.rect.x, item.rect.right(), item.rect.y, view));
            }
        }
        close(self, &mut words, &mut segments);
        let top = segments.iter().map(|segment| segment.2).fold(f64::INFINITY, f64::min);
        if !top.is_finite() {
            return None;
        }
        let mut views = Vec::new();
        let mut right = None;
        for (left, end, at, mut view) in segments {
            let gap = right.map_or(0.0, |right: f64| left - right);
            if gap.abs() > 0.25 {
                let _ = write!(view, "\n.padding_edge(Edge::Leading, {})", num(gap));
            }
            if at - top > 0.25 {
                let _ = write!(view, "\n.padding_edge(Edge::Top, {})", num(at - top));
            }
            views.push(view);
            right = Some(end);
        }
        let edge = match Self::content_alignment(node).0 {
            Edge::Center => "Center",
            Edge::End => "Trailing",
            Edge::Start => "Leading",
        };
        let row = format!("{}\n.spacing(0.0)\n.alignment(VerticalAlignment::Top)", stack("hstack", &views));
        Some(if self.hug.0 {
            format!("{row}\n.frame_width_aligned({}, Alignment::{edge})", num(content_width(node)))
        } else {
            format!("{row}\n.frame_max(f64::INFINITY, f64::MAX, Alignment::{edge})")
        })
    }

    /// A block-level child: an element, or a pseudo-element in the flow.
    fn block_child(&mut self, child: &Node) -> Option<String> {
        match child.kind {
            Kind::Element => {
                if child.css("display").starts_with("inline") && child.css("display") != "inline" {
                    // an inline box among blocks keeps its own width
                }
                self.element(child, Flow::Block)
            }
            Kind::Before | Kind::After => self.pseudo_box(child, Flow::Block),
            _ => None,
        }
    }

    /// A pseudo-element in the flow, as a box of its own.
    fn pseudo_box(&mut self, pseudo: &Node, flow: Flow) -> Option<String> {
        let width = css::px(pseudo.sizes.get("width").map_or("", String::as_str));
        let height = css::px(pseudo.sizes.get("height").map_or("", String::as_str));
        let words = pseudo_text(pseudo.css("content"))?;
        let grows = pseudo.css("flex-grow").parse::<f64>().unwrap_or(0.0) > 0.0;
        let size = |value: Option<f64>| value.filter(|value| *value > 0.0).map_or(Size::Hug, Size::Exact);
        let (mut w, mut h) = (size(width), size(height));
        match flow {
            Flow::Row { .. } if grows => w = if self.hug.0 { size(width) } else { Size::Fill(None) },
            Flow::Column { .. } if grows => h = if self.hug.1 { size(height) } else { Size::Fill(None) },
            _ => {}
        }
        let content = if words.is_empty() {
            match (w, h) {
                (Size::Hug, Size::Hug) => "spacer().frame(0.0, 0.0)".to_owned(),
                _ => "spacer()".to_owned(),
            }
        } else {
            self.words_view(pseudo, &words)
        };
        Some(self.dressed(pseudo, content, w, h))
    }

    /// The words of a line: one text, or a paragraph of runs when the
    /// inline elements around them change the face, the ink or the ground.
    fn paragraph(&mut self, node: &Node, items: &[&Node]) -> Option<String> {
        let mut runs: Vec<(String, TextStyle)> = Vec::new();
        let base = TextStyle::of(node);
        self.runs(node, items, &base, &mut runs);
        // white space: collapsed inside, trimmed at the paragraph's ends
        if let Some(first) = runs.first_mut() {
            first.0 = first.0.trim_start().to_owned();
        }
        if let Some(last) = runs.last_mut() {
            last.0 = last.0.trim_end().to_owned();
        }
        let mut merged: Vec<(String, TextStyle)> = Vec::new();
        for (words, style) in runs {
            if words.is_empty() {
                continue;
            }
            match merged.last_mut() {
                Some((previous, last_style)) if *last_style == style => previous.push_str(&words),
                Some((previous, _)) if previous.ends_with(' ') && words.starts_with(' ') => merged.push((words.trim_start().to_owned(), style)),
                _ => merged.push((words, style)),
            }
        }
        if merged.is_empty() {
            return None;
        }
        let view = if merged.len() == 1 && merged[0].1 == base {
            format!("text({})", quote(&merged[0].0))
        } else {
            let runs: Vec<String> = merged.iter().map(|(words, style)| run(words, style, &base)).collect();
            if runs.len() == 1 {
                format!("rich_text([{}])", runs[0])
            } else {
                let mut code = String::from("rich_text([");
                for run in &runs {
                    let _ = write!(code, "\n    {run},");
                }
                code.push_str("\n])");
                code
            }
        };
        Some(self.text_modifiers(node, view, &base))
    }

    fn runs(&mut self, node: &Node, items: &[&Node], style: &TextStyle, out: &mut Vec<(String, TextStyle)>) {
        let preserve = node.css("white-space").starts_with("pre") || node.css("white-space") == "break-spaces";
        for item in items {
            match item.kind {
                Kind::Space => out.push((" ".to_owned(), style.clone())),
                Kind::Text => {
                    let words = if preserve { item.text.clone() } else { collapse(&item.text) };
                    out.push((transformed(&words, &style.transform), style.clone()));
                }
                Kind::Before | Kind::After => {
                    if let Some(words) = pseudo_text(item.css("content")) {
                        let mut inner = TextStyle::of(item);
                        inner.ground = css::color(item.css("background-color")).filter(|ground| !ground.invisible());
                        out.push((transformed(&words, &inner.transform), inner));
                    } else {
                        self.gap(node, "content: counter/attr", item.css("content").to_owned());
                    }
                }
                Kind::Element => {
                    if item.tag == "br" {
                        out.push(("\n".to_owned(), style.clone()));
                        continue;
                    }
                    let inner = TextStyle::of(item);
                    self.filters(item, &mut String::new());
                    if item.css("display") == "inline-block" {
                        self.gap(item, "inline-block in a line", "drawn as a run of its words");
                    }
                    if item.children.iter().any(is_layer) {
                        self.gap(item, "positioned box in a line", "not drawn");
                    }
                    if item.px("padding-left") > 0.0 || item.px("padding-right") > 0.0 || item.px("border-top-left-radius") > 0.0 {
                        self.gap(item, "inline box: padding/radius", "the run keeps its ground, not its box");
                    }
                    if item.kind == Kind::Element && !item.id.is_empty() {
                        // an inline element is a run, not a view: its name stays off
                    }
                    let children: Vec<&Node> = item.children.iter().filter(|child| !is_layer(child)).collect();
                    self.runs(item, &children, &inner, out);
                }
            }
        }
    }

    /// The text's own modifiers: what its style says beyond the page's.
    fn text_modifiers(&mut self, node: &Node, mut view: String, style: &TextStyle) -> String {
        if (style.size - self.root.size).abs() > 0.01 {
            let _ = write!(view, "\n.font_size({})", num(style.size));
        }
        if style.weight != self.root.weight {
            let _ = write!(view, "\n.font_weight({})", weight(style.weight));
        }
        if style.italic {
            view.push_str("\n.italic()");
        }
        if let Some(color) = style.color.filter(|color| Some(*color) != self.root.color) {
            let _ = write!(view, "\n.foreground_color({})", color.code());
        }
        if !style.family.is_empty() && style.family != self.root.family {
            self.gap(node, "font-family", style.family.clone());
        }
        if style.tracking.abs() > 0.001 {
            let _ = write!(view, "\n.tracking({})", num(style.tracking));
        }
        if let Some(height) = style.line_height {
            let _ = write!(view, "\n.line_height({})", num(height));
        }
        if style.underline {
            view.push_str("\n.underline(None)");
        }
        if style.strike {
            view.push_str("\n.strikethrough(None)");
        }
        match node.css("text-align") {
            "center" | "-webkit-center" => view.push_str("\n.multiline_text_alignment(TextAlignment::Center)"),
            "right" | "end" | "-webkit-right" => view.push_str("\n.multiline_text_alignment(TextAlignment::Trailing)"),
            "justify" => self.gap(node, "text-align: justify", "set flush left"),
            _ => {}
        }
        if node.css("text-overflow") == "ellipsis" && node.css("white-space") == "nowrap" {
            view.push_str("\n.truncation_mode(Truncation::End)");
        } else if node.css("white-space") == "nowrap" {
            self.gap(node, "white-space: nowrap", "the words may wrap");
        }
        if !node.css("-webkit-line-clamp").is_empty() && node.css("-webkit-line-clamp") != "none" {
            // the lines that fit the clamped height show, cut there; the
            // ellipsis the page draws at the end is not drawn
            self.gap(node, "-webkit-line-clamp", format!("{} lines, cut without an ellipsis", node.css("-webkit-line-clamp")));
            let _ = write!(view, "\n.frame_height_aligned({}, Alignment::TopLeading)\n.clipped()", num(content_height(node)));
        }
        if !node.css("font-variant").is_empty() && node.css("font-variant") != "normal" {
            self.gap(node, "font-variant", node.css("font-variant").to_owned());
        }
        if node.css("text-shadow") != "none" && !node.css("text-shadow").is_empty() {
            self.gap(node, "text-shadow", node.css("text-shadow").to_owned());
        }
        view
    }

    /// Words in a box's own style.
    fn words_view(&mut self, node: &Node, words: &str) -> String {
        let style = TextStyle::of(node);
        let words = transformed(words, &style.transform);
        self.text_modifiers(node, format!("text({})", quote(&words)), &style)
    }

    /// A flex container: its items in a stack, its gap the stack's
    /// spacing, its justification spacers.
    fn flex(&mut self, node: &Node, children: &[&Node]) -> String {
        let row = !node.css("flex-direction").starts_with("column");
        let stretch = matches!(node.css("align-items"), "normal" | "stretch" | "");
        let flow = if row { Flow::Row { stretch } } else { Flow::Column { stretch } };
        let gap = css::px(node.css(if row { "column-gap" } else { "row-gap" })).unwrap_or(0.0);
        let mut items: Vec<&Node> = children.iter().copied().filter(|child| child.kind != Kind::Space).collect();
        items.sort_by_key(|child| child.css("order").parse::<i64>().unwrap_or(0));
        if node.css("flex-direction").ends_with("reverse") {
            items.reverse();
        }
        let grows: Vec<f64> = items.iter().map(|child| child.css("flex-grow").parse().unwrap_or(0.0)).collect();
        let growing: Vec<f64> = grows.iter().copied().filter(|grow| *grow > 0.0).collect();
        if growing.iter().any(|grow| (grow - growing[0]).abs() > 1e-9) {
            self.gap(node, "flex-grow: ratios", format!("{grows:?} drawn as equal shares"));
        }
        // the same negative margin before every item but the first —
        // avatars overlapping in a row — is the stack's own spacing
        let lead = if row { "margin-left" } else { "margin-top" };
        let overlap = items
            .iter()
            .skip(1)
            .map(|child| child.px(lead))
            .try_fold(None, |seen: Option<f64>, margin| match seen {
                _ if margin >= 0.0 => Err(()),
                Some(previous) if (previous - margin).abs() > 1e-9 => Err(()),
                _ => Ok(Some(margin)),
            })
            .ok()
            .flatten()
            .filter(|_| items.len() > 1);
        let gap = gap + overlap.unwrap_or(0.0);
        let mut views = Vec::new();
        for (index, child) in items.iter().enumerate() {
            let view = match child.kind {
                Kind::Element => self.element(child, flow),
                Kind::Text => {
                    let words = collapse(&child.text).trim().to_owned();
                    (!words.is_empty()).then(|| self.words_view(node, &words))
                }
                Kind::Before | Kind::After => self.pseudo_box(child, flow),
                Kind::Space => None,
            };
            let Some(mut view) = view else { continue };
            if matches!(child.kind, Kind::Element | Kind::Before | Kind::After) {
                view.push_str(&margins(child, overlap.is_some() && index > 0, row));
            }
            // an item that names its own place across the line: a box the
            // line's cross size, the item on that edge in it
            if child.kind == Kind::Element {
                view.push_str(&self.self_alignment(node, child, row));
            }
            // an `auto` margin takes the line's free room: a spacer
            let (before, after) = if row { ("margin-left", "margin-right") } else { ("margin-top", "margin-bottom") };
            let auto = |side: &str| child.kind == Kind::Element && child.sizes.get(side).is_some_and(|value| value == "auto");
            if auto(before) {
                views.push("spacer()".to_owned());
            }
            views.push(view);
            if auto(after) {
                views.push("spacer()".to_owned());
            }
        }
        let justify = node.css("justify-content");
        let spaced = grows.iter().all(|grow| *grow == 0.0) && views.len() > 1;
        let views = match justify {
            "space-between" if spaced => interleave(views, false),
            "space-around" | "space-evenly" if spaced => {
                if justify == "space-around" {
                    self.gap(node, "justify-content: space-around", "drawn as space-evenly");
                }
                interleave(views, true)
            }
            _ => views,
        };
        let mut code = stack(if row { "hstack" } else { "vstack" }, &views);
        if gap.abs() > 1e-9 {
            let _ = write!(code, "\n.spacing({})", num(gap));
        }
        let cross = node.css("align-items");
        if row {
            match cross {
                "center" => code.push_str("\n.alignment(VerticalAlignment::Center)"),
                "flex-end" | "end" => code.push_str("\n.alignment(VerticalAlignment::Bottom)"),
                "baseline" | "first baseline" => code.push_str("\n.alignment(VerticalAlignment::Baseline)"),
                _ => code.push_str("\n.alignment(VerticalAlignment::Top)"),
            }
            if node.css("flex-wrap").starts_with("wrap") {
                code.push_str("\n.wrapping()");
                let row_gap = css::px(node.css("row-gap")).unwrap_or(0.0);
                if row_gap > 0.0 {
                    let _ = write!(code, "\n.line_spacing({})", num(row_gap));
                }
            }
        } else {
            match cross {
                "center" => code.push_str("\n.alignment(HorizontalAlignment::Center)"),
                "flex-end" | "end" => code.push_str("\n.alignment(HorizontalAlignment::Trailing)"),
                _ => code.push_str("\n.alignment(HorizontalAlignment::Leading)"),
            }
            if node.css("flex-wrap").starts_with("wrap") {
                self.gap(node, "flex-wrap: column", "drawn on one line");
            }
        }
        code
    }

    /// `align-self`: an item placed on its own edge across the line — an
    /// outgoing bubble on the right of a column. The item takes the line's
    /// cross size in a frame and sits on that edge inside it.
    fn self_alignment(&self, line: &Node, child: &Node, row: bool) -> String {
        let edge = match child.css("align-self") {
            "flex-end" | "end" | "self-end" => Edge::End,
            "center" => Edge::Center,
            "flex-start" | "start" | "self-start" => Edge::Start,
            _ => return String::new(),
        };
        let line_edge = match line.css("align-items") {
            "flex-end" | "end" => Edge::End,
            "center" => Edge::Center,
            _ => Edge::Start,
        };
        if edge == line_edge {
            return String::new();
        }
        if row {
            let align = alignment(Edge::Start, edge);
            if self.hug.1 {
                format!("\n.frame_height_aligned({}, {align})", num(content_height(line)))
            } else {
                format!("\n.frame_max(f64::MAX, f64::INFINITY, {align})")
            }
        } else {
            let align = alignment(edge, Edge::Start);
            if self.hug.0 {
                format!("\n.frame_width_aligned({}, {align})", num(content_width(line)))
            } else {
                format!("\n.frame_max(f64::INFINITY, f64::MAX, {align})")
            }
        }
    }

    /// A grid, a table, anything laid out by tracks: the boxes the browser
    /// placed, cut into rows and columns of stacks at their places.
    fn cut(&mut self, node: &Node, children: &[&Node]) -> String {
        let items: Vec<&Node> = children.iter().copied().filter(|child| child.kind == Kind::Element && child.rect.w > 0.0 && child.rect.h > 0.0).collect();
        if items.is_empty() {
            return "spacer()".to_owned();
        }
        let origin = (
            node.rect.x + node.px("padding-left") + node.px("border-left-width"),
            node.rect.y + node.px("padding-top") + node.px("border-top-width"),
        );
        let (view, _) = self.cut_group(node, &items, origin);
        view
    }

    /// A group of placed boxes, cut where no box crosses: columns side by
    /// side, rows one under the other, each at its offset from `origin`.
    fn cut_group(&mut self, node: &Node, items: &[&Node], origin: (f64, f64)) -> (String, Rect) {
        let bounds = items.iter().fold(None, |acc: Option<Rect>, item| {
            let rect = item.rect;
            Some(acc.map_or(rect, |acc| {
                let (x, y) = (acc.x.min(rect.x), acc.y.min(rect.y));
                Rect { x, y, w: acc.right().max(rect.right()) - x, h: acc.bottom().max(rect.bottom()) - y }
            }))
        }).unwrap_or_default();
        let at = |mut view: String, rect: Rect, origin: (f64, f64)| {
            let (dx, dy) = (rect.x - origin.0, rect.y - origin.1);
            if dx > 0.25 {
                let _ = write!(view, "\n.padding_edge(Edge::Leading, {})", num(dx));
            }
            if dy > 0.25 {
                let _ = write!(view, "\n.padding_edge(Edge::Top, {})", num(dy));
            }
            view
        };
        if let [only] = items {
            let view = self.element(only, Flow::Cell).unwrap_or_else(|| "spacer()".to_owned());
            return (at(view, only.rect, origin), bounds);
        }
        for horizontal in [true, false] {
            let groups = split(items, horizontal);
            if groups.len() < 2 {
                continue;
            }
            let mut views = Vec::new();
            let mut edge = if horizontal { bounds.x } else { bounds.y };
            for group in &groups {
                let (view, rect) = self.cut_group(node, group, if horizontal { (edge, bounds.y) } else { (bounds.x, edge) });
                views.push(view);
                edge = if horizontal { rect.right() } else { rect.bottom() };
            }
            let code = if horizontal {
                format!("{}\n.alignment(VerticalAlignment::Top)", stack("hstack", &views))
            } else {
                format!("{}\n.alignment(HorizontalAlignment::Leading)", stack("vstack", &views))
            };
            return (at(code, bounds, origin), bounds);
        }
        // boxes that overlap: layers, each at its place
        self.gap(node, "grid: overlapping items", format!("{} items layered", items.len()));
        let views: Vec<String> = items
            .iter()
            .filter_map(|item| self.element(item, Flow::Cell).map(|view| at(view, item.rect, (bounds.x, bounds.y))))
            .collect();
        (at(format!("{}\n.alignment(Alignment::TopLeading)", stack("zstack", &views)), bounds, origin), bounds)
    }

    fn image(&mut self, node: &Node) -> String {
        let Some(picture) = node.attr("src").and_then(|src| self.picture(src)) else {
            self.gap(node, "img", "a picture the importer could not find");
            return "spacer()".to_owned();
        };
        let mut view = format!("image({picture}())\n.resizable()");
        match node.css("object-fit") {
            "cover" => view.push_str("\n.aspect_ratio(ContentMode::Fill)\n.clipped()"),
            "contain" => view.push_str("\n.aspect_ratio(ContentMode::Fit)"),
            "fill" | "" => {}
            other => self.gap(node, "object-fit", other.to_owned()),
        }
        let position = node.css("object-position");
        if !position.is_empty() && position != "50% 50%" {
            self.gap(node, "object-position", position.to_owned());
        }
        view
    }

    fn control(&mut self, node: &Node) -> String {
        let kind = node.attr("type").unwrap_or("text");
        let placeholder = quote(node.attr("placeholder").unwrap_or_default());
        let value = quote(node.attr("current").unwrap_or_default());
        let inset = format!(
            "Edges {{ top: {}, trailing: {}, bottom: {}, leading: {} }}",
            num(node.px("padding-top") + node.px("border-top-width")),
            num(node.px("padding-right") + node.px("border-right-width")),
            num(node.px("padding-bottom") + node.px("border-bottom-width")),
            num(node.px("padding-left") + node.px("border-left-width"))
        );
        let field = |maker: &str| format!("{maker}({placeholder}, State::new(String::from({value})).binding())\n.bare()\n.content_inset({inset})");
        match (node.tag.as_str(), kind) {
            ("textarea", _) => field("text_editor"),
            (_, "text" | "email" | "search" | "url" | "tel" | "number") => field("text_field"),
            (_, "password") => format!("{}\n.secret(true)", field("text_field")),
            (_, "checkbox") => format!("checkbox(text(\"\"), State::new({}).binding())", node.attr("checked").is_some()),
            _ => {
                self.gap(node, &format!("input type={kind}"), "drawn as an empty box");
                "spacer()".to_owned()
            }
        }
    }

    /// A native `<progress>` or `<meter>`: a track and its fill.
    fn meter(&mut self, node: &Node) -> String {
        self.gap(node, &node.tag.clone(), "the native bar drawn as a track and a fill");
        let share = node.attr("position").and_then(css::number).unwrap_or(0.0).clamp(0.0, 1.0);
        let fill = css::color(node.css("accent-color")).filter(|_| node.css("accent-color") != "auto").map_or_else(|| "Color::rgb(52, 120, 246)".to_owned(), Rgba::code);
        format!(
            "rectangle()\n.foreground_color({fill})\n.frame({}, {})\n.frame_width_aligned({}, Alignment::Leading)\n.background_color(Color::rgb(229, 229, 234))\n.corner_radius({})",
            num(node.rect.w * share),
            num(node.rect.h),
            num(node.rect.w),
            num(node.rect.h / 2.0)
        )
    }

    /// The function that reads the picture at `url`, written once per file.
    fn picture(&mut self, url: &str) -> Option<String> {
        let file = if let Some(absolute) = url.strip_prefix('/') {
            self.options.site_root.join(absolute)
        } else {
            self.options.page_dir.join(url)
        };
        if !file.is_file() {
            return None;
        }
        let path = relative(&self.options.module_dir, &file);
        let count = self.pictures.len();
        let name = self.pictures.entry(path).or_insert_with(|| {
            let stem: String = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect())
                .unwrap_or_default();
            format!("picture_{stem}_{count}")
        });
        Some(name.clone())
    }
}

/// A CSS transform's turn: `SwiftUI`'s rotation effect about the
/// transform's origin, which turns the picture and never the layout.
fn rotation(node: &Node) -> Option<String> {
    let degrees = css::transform(node.css("transform")).ok().flatten()?.degrees();
    if degrees.abs() <= 0.01 {
        return None;
    }
    let origin: Vec<f64> = node.css("transform-origin").split_whitespace().filter_map(css::px).collect();
    // a pseudo-element's box is not reported; its computed size stands in
    let extent = |reported: f64, property: &str| if reported > 0.0 { Some(reported) } else { css::px(node.size(property)).filter(|side| *side > 0.0) };
    let size = extent(node.rect.w, "width").zip(extent(node.rect.h, "height"));
    Some(match (origin.as_slice(), size) {
        ([x, y, ..], Some((w, h))) if ((x / w) - 0.5).abs() > 1e-3 || ((y / h) - 0.5).abs() > 1e-3 => {
            format!("\n.rotation_effect_at(Angle::degrees({}), UnitPoint {{ x: {}, y: {} }})", num(degrees), num(x / w), num(y / h))
        }
        _ => format!("\n.rotation_effect(Angle::degrees({}))", num(degrees)),
    })
}

/// A CSS ramp as bunny holds it: the colors at the two ends of the
/// box's gradient line and the stops between them.
struct Ramp {
    from: Rgba,
    to: Rgba,
    stops: Vec<(f64, Rgba)>,
}

impl Ramp {
    fn stops_code(&self) -> String {
        self.stops.iter().fold(String::new(), |mut out, (location, color)| {
            let _ = write!(out, ".stop({}, {})", num(*location), color.code());
            out
        })
    }
}

/// The ramp of `stops` on a line `length` px long, each place resolved as
/// CSS resolves it: the first at the start and the last at the end when
/// unnamed, the unnamed between share out the room of their neighbours,
/// and a place behind an earlier one stands at it. What lies past the
/// box's line is cut off at its ends.
fn ramp(stops: &[css::Stop], length: f64) -> Option<Ramp> {
    let count = stops.len();
    let mut places: Vec<Option<f64>> = stops.iter().map(|stop| stop.at.map(|at| at.fraction(length))).collect();
    *places.first_mut()? = places[0].or(Some(0.0));
    if count > 1 {
        places[count - 1] = places[count - 1].or(Some(1.0));
    }
    let mut most = f64::NEG_INFINITY;
    for place in places.iter_mut().flatten() {
        most = most.max(*place);
        *place = most;
    }
    let mut index = 1;
    while index < count {
        if places[index].is_some() {
            index += 1;
            continue;
        }
        let after = (index..count).find(|at| places[*at].is_some()).unwrap_or(count - 1);
        let (low, high) = (places[index - 1].unwrap_or(0.0), places[after].unwrap_or(1.0));
        #[allow(clippy::cast_precision_loss, reason = "a handful of stops")]
        let room = (after - index + 1) as f64;
        for (step, at) in (index..after).enumerate() {
            #[allow(clippy::cast_precision_loss, reason = "a handful of stops")]
            let share = (step + 1) as f64 / room;
            places[at] = Some((high - low).mul_add(share, low));
        }
        index = after + 1;
    }
    let points: Vec<(f64, Rgba)> = places.into_iter().map(|place| place.unwrap_or(0.0)).zip(stops.iter().map(|stop| stop.color)).collect();
    // the color at `place`, seen from after it (where two stops share a
    // place, the later) or from before it (the earlier)
    let color_at = |place: f64, from_after: bool| {
        let next = points.iter().position(|(at, _)| if from_after { *at > place } else { *at >= place });
        match next {
            None => points[points.len() - 1].1,
            Some(0) => points[0].1,
            Some(next) => {
                let ((low, below), (high, above)) = (points[next - 1], points[next]);
                if high - low <= 0.0 { above } else { lerp(below, above, (place - low) / (high - low)) }
            }
        }
    };
    let line: Vec<(f64, Rgba)> = std::iter::once((0.0, color_at(0.0, true)))
        .chain(points.iter().copied().filter(|(at, _)| *at > 0.0 && *at < 1.0))
        .chain(std::iter::once((1.0, color_at(1.0, false))))
        .collect();
    // CSS mixes with the alpha premultiplied, so a color with none takes
    // the hue of its neighbour on each side; bunny mixes straight, and
    // fades a ramp to its own color with no alpha (`Color::fade`)
    let faded = |color: Rgba| Rgba { a: 0.0, ..color };
    let mut out: Vec<(f64, Rgba)> = Vec::new();
    for (index, (at, color)) in line.iter().copied().enumerate() {
        if color.a > 0.0 {
            out.push((at, color));
            continue;
        }
        let before = line[..index].iter().rev().find(|(_, color)| color.a > 0.0).map(|(_, color)| faded(*color));
        let after = line[index + 1..].iter().find(|(_, color)| color.a > 0.0).map(|(_, color)| faded(*color));
        match (before, after) {
            (Some(before), Some(after)) if index != 0 && index != line.len() - 1 => {
                out.push((at, before));
                if after != before {
                    out.push((at, after));
                }
            }
            (_, Some(after)) if index == 0 => out.push((at, after)),
            (Some(near), _) | (None, Some(near)) => out.push((at, near)),
            (None, None) => out.push((at, color)),
        }
    }
    let (from, to) = (out[0].1, out[out.len() - 1].1);
    Some(Ramp { from, to, stops: out[1..out.len() - 1].to_vec() })
}

/// Straight interpolation, as bunny mixes a ramp.
fn lerp(from: Rgba, to: Rgba, t: f64) -> Rgba {
    let channel = |a: u8, b: u8| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "between two bytes")]
        let mixed = (f64::from(b) - f64::from(a)).mul_add(t, f64::from(a)).round() as u8;
        mixed
    };
    Rgba { r: channel(from.r, to.r), g: channel(from.g, to.g), b: channel(from.b, to.b), a: (to.a - from.a).mul_add(t, from.a) }
}

/// The view with `.offset(x, y)` around it, its name inside: the
/// offset moves the frame the named view is placed in, so the view is
/// found where it is drawn, on both roads.
fn with_offset(view: &str, x: f64, y: f64) -> String {
    format!("{view}\n.offset({}, {})", num(x), num(y))
}

/// The child's box; a pseudo-element, which the browser does not report,
/// stands where the flow would put it.
const fn child_rect(_node: &Node, child: &Node) -> Rect {
    child.rect
}

/// Spacers between the items — and around them, when `around`.
fn interleave(views: Vec<String>, around: bool) -> Vec<String> {
    let mut out = Vec::new();
    if around {
        out.push("spacer()".to_owned());
    }
    let count = views.len();
    for (index, view) in views.into_iter().enumerate() {
        out.push(view);
        if index + 1 < count || around {
            out.push("spacer()".to_owned());
        }
    }
    out
}

/// A flex item's margins, as the padding outside its box — but the
/// leading one when the stack's spacing already took it.
fn margins(child: &Node, lead_taken: bool, row: bool) -> String {
    let mut code = String::new();
    let lead = if row { "left" } else { "top" };
    for (side, edge) in [("top", "Top"), ("right", "Trailing"), ("bottom", "Bottom"), ("left", "Leading")] {
        if lead_taken && side == lead {
            continue;
        }
        // a negative margin is a negative padding: the box overlaps its
        // neighbour and gives the line that much back
        let margin = child.px(&format!("margin-{side}"));
        if margin.abs() > 1e-9 && child.sizes.get(&format!("margin-{side}")).is_none_or(|value| value != "auto") {
            let _ = write!(code, "\n.padding_edge(Edge::{edge}, {})", num(margin));
        }
    }
    code
}

/// Splits boxes where no box crosses: by x when `horizontal`, by y else.
fn split<'n>(items: &[&'n Node], horizontal: bool) -> Vec<Vec<&'n Node>> {
    let start = |node: &Node| if horizontal { node.rect.x } else { node.rect.y };
    let end = |node: &Node| if horizontal { node.rect.right() } else { node.rect.bottom() };
    let mut sorted: Vec<&Node> = items.to_vec();
    sorted.sort_by(|a, b| start(a).total_cmp(&start(b)));
    let mut groups: Vec<Vec<&Node>> = Vec::new();
    let mut reach = f64::NEG_INFINITY;
    for item in sorted {
        if groups.is_empty() || start(item) >= reach - 0.5 {
            groups.push(Vec::new());
        }
        reach = reach.max(end(item));
        if let Some(group) = groups.last_mut() {
            group.push(item);
        }
    }
    groups
}

fn padding(inset: &[f64]) -> String {
    let [top, trailing, bottom, leading] = [inset[0], inset[1], inset[2], inset[3]];
    let same = |a: f64, b: f64| (a - b).abs() < 1e-9;
    if [top, trailing, bottom, leading].iter().all(|side| same(*side, 0.0)) {
        return String::new();
    }
    if same(top, trailing) && same(trailing, bottom) && same(bottom, leading) {
        return format!("\n.padding_length({})", num(top));
    }
    let mut code = String::new();
    for (value, edge) in [(top, "Top"), (trailing, "Trailing"), (bottom, "Bottom"), (leading, "Leading")] {
        if value > 0.0 {
            let _ = write!(code, "\n.padding_edge(Edge::{edge}, {})", num(value));
        }
    }
    code
}

fn frame(width: Size, height: Size, alignment: &str) -> String {
    let ceiling = |limit: Option<f64>| limit.map_or_else(|| "f64::INFINITY".to_owned(), num);
    match (width, height) {
        (Size::Exact(w), Size::Exact(h)) => format!("\n.frame_aligned({}, {}, {alignment})", num(w), num(h)),
        (Size::Exact(w), Size::Fill(limit)) => format!("\n.frame_width_aligned({}, {alignment})\n.frame_max(f64::MAX, {}, {alignment})", num(w), ceiling(limit)),
        (Size::Exact(w), Size::Hug) => format!("\n.frame_width_aligned({}, {alignment})", num(w)),
        (Size::Fill(limit), Size::Exact(h)) => format!("\n.frame_height_aligned({}, {alignment})\n.frame_max({}, f64::MAX, {alignment})", num(h), ceiling(limit)),
        (Size::Fill(across), Size::Fill(down)) => format!("\n.frame_max({}, {}, {alignment})", ceiling(across), ceiling(down)),
        (Size::Fill(limit), Size::Hug) => format!("\n.frame_max({}, f64::MAX, {alignment})", ceiling(limit)),
        (Size::Hug, Size::Exact(h)) => format!("\n.frame_height_aligned({}, {alignment})", num(h)),
        (Size::Hug, Size::Fill(limit)) => format!("\n.frame_max(f64::MAX, {}, {alignment})", ceiling(limit)),
        (Size::Hug, Size::Hug) => String::new(),
    }
}

/// The width of a box's content: inside its padding and its border.
fn content_width(node: &Node) -> f64 {
    node.rect.w - node.px("padding-left") - node.px("padding-right") - node.px("border-left-width") - node.px("border-right-width")
}

/// The height of a box's content: inside its padding and its border.
fn content_height(node: &Node) -> f64 {
    node.rect.h - node.px("padding-top") - node.px("padding-bottom") - node.px("border-top-width") - node.px("border-bottom-width")
}

/// An empty box: nothing to draw inside, so a hugging axis is the size
/// the browser gave its content.
fn empty(node: &Node, width: Size, height: Size) -> String {
    let content_w = node.rect.w - node.px("padding-left") - node.px("padding-right") - node.px("border-left-width") - node.px("border-right-width");
    let content_h = node.rect.h - node.px("padding-top") - node.px("padding-bottom") - node.px("border-top-width") - node.px("border-bottom-width");
    match (width, height) {
        (Size::Hug, Size::Hug) => format!("spacer().frame({}, {})", num(content_w.max(0.0)), num(content_h.max(0.0))),
        (Size::Hug, _) => format!("spacer().frame_width({})", num(content_w.max(0.0))),
        (_, Size::Hug) => format!("spacer().frame_height({})", num(content_h.max(0.0))),
        _ => "spacer()".to_owned(),
    }
}

/// One run of a paragraph, with what it says beyond the paragraph.
fn run(words: &str, style: &TextStyle, base: &TextStyle) -> String {
    let mut code = format!("run({})", quote(words));
    if style.weight != base.weight {
        let _ = write!(code, ".weight({})", weight(style.weight));
    }
    if style.italic && !base.italic {
        code.push_str(".italic()");
    }
    if (style.size - base.size).abs() > 0.01 {
        let _ = write!(code, ".font_size({})", num(style.size));
    }
    if style.color != base.color
        && let Some(color) = style.color
    {
        let _ = write!(code, ".foreground_color({})", color.code());
    }
    if let Some(ground) = style.ground {
        let _ = write!(code, ".background_color({})", ground.code());
    }
    if style.underline && !base.underline {
        code.push_str(".underline(None)");
    }
    if style.strike && !base.strike {
        code.push_str(".strikethrough(None)");
    }
    code
}

/// `to` as a path from the folder `from`, `/`-separated, for `include_bytes!`.
fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<Component<'_>> = from.components().collect();
    let to_parts: Vec<Component<'_>> = to.components().collect();
    let shared = from.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - shared];
    parts.extend(to_parts[shared..].iter().map(|part| part.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_as_rust_floats() {
        assert_eq!(num(12.0), "12.0");
        assert_eq!(num(0.333_33), "0.3333");
        assert_eq!(num(-0.000_01), "0.0");
    }

    #[test]
    fn white_space_collapses_as_a_line_lays_it_out() {
        assert_eq!(collapse("  a \n\t b "), " a b ");
        assert_eq!(transformed("now playing", "uppercase"), "NOW PLAYING");
        assert_eq!(transformed("hello world", "capitalize"), "Hello World");
    }

    #[test]
    fn a_picture_path_is_written_from_the_module() {
        assert_eq!(relative(Path::new("/a/fidelity/cases/app"), Path::new("/a/fidelity/assets/x.png")), "../../assets/x.png");
    }

    /// A CSS turn is the view's rotation effect, about the transform's
    /// origin as a fraction of the box — a pseudo-element's computed size
    /// when the page reports no box for it.
    #[test]
    fn a_turn_is_a_rotation_effect_about_its_origin() {
        let turned = |w: f64, transform: &str, origin: &str| {
            let mut node = Node { rect: Rect { x: 0.0, y: 0.0, w, h: 20.0 }, ..Node::default() };
            node.style.insert("transform".to_owned(), transform.to_owned());
            node.style.insert("transform-origin".to_owned(), origin.to_owned());
            node.sizes.insert("width".to_owned(), "40px".to_owned());
            node.sizes.insert("height".to_owned(), "20px".to_owned());
            rotation(&node)
        };
        let quarter = "matrix(0, 1, -1, 0, 0, 0)";
        assert_eq!(turned(40.0, quarter, "20px 10px").as_deref(), Some("\n.rotation_effect(Angle::degrees(90.0))"));
        assert_eq!(turned(40.0, quarter, "0px 0px").as_deref(), Some("\n.rotation_effect_at(Angle::degrees(90.0), UnitPoint { x: 0.0, y: 0.0 })"));
        assert_eq!(turned(0.0, quarter, "20px 10px").as_deref(), Some("\n.rotation_effect(Angle::degrees(90.0))"), "a pseudo-element");
        assert_eq!(turned(40.0, "matrix(1, 0, 0, 1, 5, 0)", "20px 10px"), None, "a translation alone");
    }

    /// A CSS ramp resolves its places as CSS does and is cut at the ends
    /// of the box's line.
    #[test]
    fn a_ramp_places_its_stops_as_css_does() {
        let ink = |r: u8| Rgba { r, g: 0, b: 0, a: 1.0 };
        let stop = |r: u8, at: Option<css::Place>| css::Stop { color: ink(r), at };
        let reds = |ramp: &Ramp| ramp.stops.iter().map(|(at, color)| ((at * 1000.0).round() / 1000.0, color.r)).collect::<Vec<_>>();
        // unnamed places share the room evenly
        let even = ramp(&[stop(0, None), stop(100, None), stop(200, None)], 100.0).expect("a ramp");
        assert_eq!((even.from.r, even.to.r, reds(&even)), (0, 200, vec![(0.5, 100)]));
        // a place in px is a fraction of the line; one behind an earlier
        // place stands at it, a hard edge
        let edge = ramp(&[stop(0, None), stop(100, Some(css::Place::Px(25.0))), stop(200, Some(css::Place::Fraction(0.1))), stop(250, None)], 100.0)
            .expect("a ramp");
        assert_eq!(reds(&edge), [(0.25, 100), (0.25, 200)]);
        // what lies past the line is cut off at its ends
        let wide = ramp(&[stop(0, Some(css::Place::Fraction(-1.0))), stop(200, Some(css::Place::Fraction(2.0)))], 100.0).expect("a ramp");
        assert_eq!((wide.from.r, wide.to.r, wide.stops.len()), (67, 133, 0));
        assert_eq!(edge.stops_code(), ".stop(0.25, Color::rgb(100, 0, 0)).stop(0.25, Color::rgb(200, 0, 0))");
        // a color with no alpha takes its neighbours' hues, faded
        let clear = Rgba { r: 0, g: 0, b: 0, a: 0.0 };
        let scrim = ramp(&[stop(10, None), css::Stop { color: clear, at: Some(css::Place::Fraction(0.4)) }, stop(90, None)], 100.0).expect("a ramp");
        assert_eq!(scrim.stops.iter().map(|(at, color)| (*at, color.r, color.a)).collect::<Vec<_>>(), [(0.4, 10, 0.0), (0.4, 90, 0.0)]);
        let glow = ramp(&[stop(30, None), css::Stop { color: clear, at: None }], 100.0).expect("a ramp");
        assert_eq!((glow.to.r, glow.to.a), (30, 0.0));
    }

    #[test]
    fn boxes_cut_where_nothing_crosses() {
        let boxed = |x: f64, y: f64, w: f64, h: f64| Node { rect: Rect { x, y, w, h }, ..Node::default() };
        let (a, b, c) = (boxed(0.0, 0.0, 100.0, 50.0), boxed(110.0, 0.0, 100.0, 50.0), boxed(0.0, 60.0, 210.0, 20.0));
        assert_eq!(split(&[&a, &b, &c], false).len(), 2, "two rows");
        assert_eq!(split(&[&a, &b], true).len(), 2, "two columns");
        assert_eq!(split(&[&a, &b, &c], true).len(), 1, "the wide row crosses every column");
    }

    #[test]
    fn pseudo_content_reads_its_string() {
        assert_eq!(pseudo_text("\"01\"").as_deref(), Some("01"));
        assert_eq!(pseudo_text("counter(item)"), None);
    }
}
