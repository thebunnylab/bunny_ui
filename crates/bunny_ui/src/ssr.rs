//! Build-time rendering: the first paint without a line of JavaScript.
//!
//! The flow lowering already turns a scene into patches. This module
//! applies the MOUNT's patches to a small element tree in Rust — the
//! same moves the browser glue makes, mirrored — and serializes the
//! result as HTML. Rust runs native at build time, so a page can ship
//! painted: the wasm then boots on top, adopts the elements by their
//! ids, and its first diff says nothing.
//!
//! What this is not: a server runtime. `render` is a string builder;
//! deployment stays static files, and hydration is one attribute.

use std::collections::BTreeMap;

use crate::dom::{CreateKind, DomPatch};
use crate::layout::{Color, Size};
use crate::runtime::Runtime;
use crate::view::View;

/// One rendered page: the body markup and the pseudo-state rules.
pub struct SsrPage {
    pub html: String,
    pub css: String,
}

/// Renders `root` at `size` and serializes the mount — the same
/// patches a browser would receive, applied to a toy tree here.
pub fn render(root: &impl View, size: Size) -> SsrPage {
    let runtime = Runtime::new();
    let patches = runtime.dom_frame(root, size);
    let mut tree = Tree::new(size);
    for patch in &patches {
        tree.apply(patch);
    }
    SsrPage { html: tree.serialize_root(), css: tree.rules.values().cloned().collect::<Vec<_>>().join("\n") }
}

/// A whole document: the page, its stylesheet, and the boot scripts.
/// `wasm` names the binary the glue will fetch; the mount carries
/// `data-hydrate` so the glue adopts instead of rebuilding.
pub fn render_document(root: &impl View, size: Size, wasm: &str, glue: &str) -> String {
    let page = render(root, size);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n  <head>\n    <meta charset=\"utf-8\" />\n    \
         <style>\nhtml,body{{margin:0;height:100%;background:#101216;display:grid;place-items:center}}\n\
         #app{{position:relative;width:{width}px;height:{height}px;overflow:hidden}}\n{css}\n</style>\n  </head>\n  <body>\n    \
         {html}\n    <script>\n      window.BUNNY_WASM = \"{wasm}\";\n    </script>\n    \
         <script src=\"{glue}\"></script>\n  </body>\n</html>\n",
        width = size.width,
        height = size.height,
        css = page.css,
        html = page.html,
    )
}

/// A toy element: enough DOM to receive the mount and print itself.
struct Element {
    tag: &'static str,
    /// `data-n` — the identity hydration adopts by.
    id: u32,
    attrs: BTreeMap<&'static str, String>,
    style: BTreeMap<&'static str, String>,
    text: Vec<(String, Option<Color>)>,
    children: Vec<u32>,
}

struct Tree {
    elements: BTreeMap<u32, Element>,
    rules: BTreeMap<u32, String>,
}

fn color(value: Color) -> String {
    format!(
        "rgba({}, {}, {}, {})",
        value.r,
        value.g,
        value.b,
        (value.a as f64) / 255.0
    )
}

fn px(value: f64) -> String {
    // trim the float the way the browser would print it back
    if value.fract() == 0.0 {
        format!("{}px", value as i64)
    } else {
        format!("{value}px")
    }
}

impl Tree {
    fn new(size: Size) -> Tree {
        let mut root = Element {
            tag: "div",
            id: 0,
            attrs: BTreeMap::new(),
            style: BTreeMap::new(),
            text: Vec::new(),
            children: Vec::new(),
        };
        root.attrs.insert("id", "app".to_string());
        root.attrs.insert("data-hydrate", "1".to_string());
        // the window is a one-slot column: its child can take the box
        root.style.insert("display", "flex".into());
        root.style.insert("flex-direction", "column".into());
        let _ = size;
        let mut elements = BTreeMap::new();
        elements.insert(0, root);
        Tree { elements, rules: BTreeMap::new() }
    }

    /// The glue's `createElementOf`, mirrored.
    fn create(&mut self, id: u32, kind: CreateKind, hints: &crate::dom::DomHints) {
        let (tag, style): (&'static str, &[(&'static str, &str)]) = match kind {
            CreateKind::Canvas => ("canvas", &[]),
            CreateKind::Image => ("img", &[("pointer-events", "none")]),
            CreateKind::Icon => ("svg", &[("pointer-events", "none")]),
            CreateKind::Field => (
                "input",
                &[
                    ("box-sizing", "border-box"),
                    ("padding", "5px 8px"),
                    ("outline", "none"),
                ],
            ),
            CreateKind::Editor => (
                "textarea",
                &[
                    ("box-sizing", "border-box"),
                    ("padding", "5px 8px"),
                    ("outline", "none"),
                    ("resize", "none"),
                    ("font", "inherit"),
                ],
            ),
            CreateKind::FlexColumn => (
                "div",
                &[
                    ("display", "flex"),
                    ("flex-direction", "column"),
                    ("box-sizing", "border-box"),
                    ("min-width", "0"),
                    ("min-height", "0"),
                ],
            ),
            CreateKind::FlexRow => (
                "div",
                &[
                    ("display", "flex"),
                    ("flex-direction", "row"),
                    ("box-sizing", "border-box"),
                    ("min-width", "0"),
                    ("min-height", "0"),
                ],
            ),
            CreateKind::Layers => (
                "div",
                &[
                    ("display", "grid"),
                    ("box-sizing", "border-box"),
                    ("min-width", "0"),
                    ("min-height", "0"),
                ],
            ),
            CreateKind::Popover => (
                "div",
                &[
                    ("position", "absolute"),
                    ("left", "0"),
                    ("top", "0"),
                    ("box-sizing", "border-box"),
                ],
            ),
            CreateKind::Text => (
                "div",
                &[
                    ("box-sizing", "border-box"),
                    ("min-width", "0"),
                    ("min-height", "0"),
                    ("white-space", "pre-wrap"),
                    ("cursor", "default"),
                ],
            ),
            CreateKind::Scroll => (
                "div",
                &[
                    ("box-sizing", "border-box"),
                    ("min-width", "0"),
                    ("min-height", "0"),
                    ("overflow", "auto"),
                    ("scroll-behavior", "smooth"),
                ],
            ),
            CreateKind::Content => (
                "div",
                &[
                    ("box-sizing", "border-box"),
                    ("min-width", "0"),
                    ("min-height", "0"),
                    ("position", "relative"),
                ],
            ),
            // a wrapper is a COLUMN, not a block: the engine proposes
            // its box to the child, and only a flex line can hand the
            // offer down (width by the stretch default, height by the
            // fill flag)
            CreateKind::Group | CreateKind::Box => (
                "div",
                &[
                    ("display", "flex"),
                    ("flex-direction", "column"),
                    ("box-sizing", "border-box"),
                    ("min-width", "0"),
                    ("min-height", "0"),
                ],
            ),
        };
        let mut element = Element {
            tag,
            id,
            attrs: BTreeMap::new(),
            style: BTreeMap::new(),
            text: Vec::new(),
            children: Vec::new(),
        };
        for (name, value) in style {
            element.style.insert(name, (*value).to_string());
        }
        if let Some(tag_hint) = &hints.tag {
            element.tag = leak_tag(tag_hint);
            // the table family lays itself out — the browser's own
            // display wins and our flex steps aside (the glue's rule,
            // mirrored: a serialized page must agree with a mounted one)
            if matches!(
                element.tag,
                "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
            ) {
                element.style.remove("display");
                element.style.remove("flex-direction");
                element.style.remove("min-width");
                element.style.remove("min-height");
            }
        }
        if let Some(class) = &hints.class {
            element.attrs.insert("class", class.to_string());
        }
        if let Some(dom_id) = &hints.dom_id {
            element.attrs.insert("id", dom_id.to_string());
        }
        self.elements.insert(id, element);
    }

    fn apply(&mut self, patch: &DomPatch) {
        match patch {
            DomPatch::Create { id, parent, before, kind, hints } => {
                self.create(*id, *kind, hints);
                let parent = self.elements.get_mut(parent).expect("the parent exists");
                match before {
                    0 => parent.children.push(*id),
                    anchor => {
                        let at = parent
                            .children
                            .iter()
                            .position(|child| child == anchor)
                            .unwrap_or(parent.children.len());
                        parent.children.insert(at, *id);
                    }
                }
            }
            DomPatch::Remove { id } => {
                for element in self.elements.values_mut() {
                    element.children.retain(|child| child != id);
                }
                self.elements.remove(id);
            }
            DomPatch::SetTransform { id, x, y } => {
                if let Some(element) = self.elements.get_mut(id) {
                    element.style.insert("position", "absolute".into());
                    element.style.insert("left", "0".into());
                    element.style.insert("top", "0".into());
                    element
                        .style
                        .insert("transform", format!("translate({}, {})", px(*x), px(*y)));
                }
            }
            DomPatch::SetSize { id, width, height } => {
                if *id == 0 {
                    return; // the page styles #app; the root op is the browser's
                }
                if let Some(element) = self.elements.get_mut(id) {
                    element.style.insert("width", px(*width));
                    element.style.insert("height", px(*height));
                }
            }
            DomPatch::SetLayout { id, layout } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                for name in [
                    "gap",
                    "align-items",
                    "padding",
                    "width",
                    "height",
                    "max-width",
                    "max-height",
                    "flex",
                    "position",
                    "top",
                    "left",
                    "right",
                    "transform",
                ] {
                    element.style.remove(name);
                }
                element.style.insert("min-width", "0".into());
                element.style.insert("min-height", "0".into());
                if let Some(gap) = layout.gap {
                    element.style.insert("gap", px(gap));
                }
                if let Some(align) = layout.align {
                    element.style.insert(
                        "align-items",
                        match align {
                            1 => "center",
                            2 => "flex-end",
                            3 => "baseline",
                            _ => "flex-start",
                        }
                        .into(),
                    );
                }
                if let Some((top, right, bottom, left)) = layout.padding {
                    element.style.insert(
                        "padding",
                        format!("{} {} {} {}", px(top), px(right), px(bottom), px(left)),
                    );
                }
                if let Some(width) = layout.width {
                    element.style.insert("width", px(width));
                }
                if let Some(height) = layout.height {
                    element.style.insert("height", px(height));
                }
                if let Some(max) = layout.max_width {
                    element.style.insert("max-width", px(max));
                }
                if let Some(max) = layout.max_height {
                    element.style.insert("max-height", px(max));
                }
                if layout.grow {
                    element.style.insert("flex", "1 1 0".into());
                }
                if let Some(slot) = layout.slot_y {
                    element.style.insert("position", "absolute".into());
                    element.style.insert("top", px(slot));
                    element.style.insert("left", "0".into());
                    element.style.insert("right", "0".into());
                }
                if layout.stretch {
                    element.style.insert("align-self", "stretch".into());
                }
                if layout.fill {
                    element.style.insert("flex", "1 1 auto".into());
                }
            }
            DomPatch::SetStyle { id, style } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                for name in [
                    "background-color",
                    "background-image",
                    "border",
                    "border-radius",
                    "box-shadow",
                    "transition",
                    "color",
                    "overflow",
                ] {
                    element.style.remove(name);
                }
                let name = format!("[data-n=\"{id}\"]");
                let mut pseudo: Vec<String> = Vec::new();
                if let Some(background) = style.background {
                    element.style.insert("background-color", color(background));
                }
                if let Some(hover) = style.hover_background {
                    pseudo.push(format!("{name}:hover{{background:{} !important}}", color(hover)));
                }
                if let Some(pressed) = style.pressed_background {
                    pseudo
                        .push(format!("{name}:active{{background:{} !important}}", color(pressed)));
                }
                if let Some((border, width)) = style.border {
                    element
                        .style
                        .insert("border", format!("{} solid {}", px(width), color(border)));
                }
                if let Some(radii) = style.corner_radius {
                    // one number when every corner shares it, four in
                    // the CSS order otherwise — clockwise from top left
                    let uniform = radii.top_left == radii.top_right
                        && radii.top_left == radii.bottom_right
                        && radii.top_left == radii.bottom_left;
                    let value = if uniform {
                        px(radii.top_left)
                    } else {
                        format!(
                            "{} {} {} {}",
                            px(radii.top_left),
                            px(radii.top_right),
                            px(radii.bottom_right),
                            px(radii.bottom_left),
                        )
                    };
                    element.style.insert("border-radius", value);
                }
                if let Some((radius, shadow)) = style.shadow {
                    element
                        .style
                        .insert("box-shadow", format!("0 0 {} {}", px(radius), color(shadow)));
                }
                if let Some((response, _)) = style.transition {
                    element.style.insert(
                        "transition",
                        format!(
                            "background-color {response}s ease-out, transform {response}s ease-out"
                        ),
                    );
                }
                if let Some(path) = &style.interactive {
                    element.attrs.insert("data-path", path.to_string());
                    element.style.insert("cursor", "default".into());
                }
                if let Some(focus) = style.focus_border {
                    pseudo.push(format!(
                        "{name}:focus{{border-color:{c} !important;caret-color:{c}}}",
                        c = color(focus)
                    ));
                }
                if let Some(placeholder) = style.placeholder_color {
                    pseudo.push(format!("{name}::placeholder{{color:{}}}", color(placeholder)));
                }
                if let Some(ink) = style.color {
                    element.style.insert("color", color(ink));
                }
                if let Some(hover) = style.hover_color {
                    pseudo.push(format!("{name}:hover{{color:{} !important}}", color(hover)));
                }
                if let Some(pressed) = style.pressed_color {
                    pseudo.push(format!("{name}:active{{color:{} !important}}", color(pressed)));
                }
                if let Some(gradient) = &style.gradient {
                    element.style.insert("background-image", css_gradient(gradient));
                }
                if style.clip {
                    element.style.insert("overflow", "hidden".into());
                }
                if pseudo.is_empty() {
                    self.rules.remove(id);
                } else {
                    self.rules.insert(*id, pseudo.join("\n"));
                }
            }
            DomPatch::SetText { id, text } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                element.style.insert("font", css_font(&text.font));
                // after the font shorthand, which resets it — the served
                // page steps its lines the way the engine measured them
                if let Some(height) = text.line_height {
                    element.style.insert("line-height", format!("{height}px"));
                }
                if text.inherits_ink {
                    element.style.remove("color");
                } else {
                    element.style.insert("color", color(text.color));
                }
                if text.truncation.is_some() {
                    element.style.insert("overflow", "hidden".into());
                    element.style.insert("text-overflow", "ellipsis".into());
                    element.style.insert("white-space", "nowrap".into());
                }
                element.text.clear();
                match &text.highlights {
                    Some((ranges, highlight)) => {
                        let raw = text.content.as_bytes();
                        let mut cursor = 0usize;
                        for (start, end) in ranges.iter() {
                            if *start > cursor {
                                element.text.push((
                                    String::from_utf8_lossy(&raw[cursor..*start]).into_owned(),
                                    None,
                                ));
                            }
                            element.text.push((
                                String::from_utf8_lossy(&raw[*start..*end]).into_owned(),
                                Some(*highlight),
                            ));
                            cursor = *end;
                        }
                        if cursor < raw.len() {
                            element
                                .text
                                .push((String::from_utf8_lossy(&raw[cursor..]).into_owned(), None));
                        }
                    }
                    None => element.text.push((text.content.to_string(), None)),
                }
            }
            DomPatch::SetField { id, field } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                element.style.insert("font", css_font(&field.font));
                element.style.insert("color", color(field.color));
                element.attrs.insert("value", field.content.to_string());
                element.attrs.insert("placeholder", field.placeholder.to_string());
                element.attrs.insert("data-path", field.path.clone());
            }
            DomPatch::SetHints { id, class, dom_id } => {
                if let Some(element) = self.elements.get_mut(id) {
                    match class {
                        Some(class) => element.attrs.insert("class", class.to_string()),
                        None => element.attrs.remove("class"),
                    };
                    match dom_id {
                        Some(dom_id) => element.attrs.insert("id", dom_id.to_string()),
                        None => element.attrs.remove("id"),
                    };
                }
            }
            DomPatch::SetScroll { .. }
            | DomPatch::SetImage { .. }
            | DomPatch::SetIcon { .. }
            | DomPatch::Move { .. }
            | DomPatch::Reveal { .. }
            | DomPatch::SetAnchor { .. } => {
                // scroll offsets, image bytes and icon geometry arrive
                // after boot; a built page starts at rest
            }
        }
    }

    fn serialize_root(&self) -> String {
        let mut out = String::new();
        self.serialize(0, &mut out);
        out
    }

    fn serialize(&self, id: u32, out: &mut String) {
        let Some(element) = self.elements.get(&id) else {
            return;
        };
        out.push('<');
        out.push_str(element.tag);
        out.push_str(&format!(" data-n=\"{}\"", element.id));
        for (name, value) in &element.attrs {
            out.push_str(&format!(" {name}=\"{}\"", escape_attr(value)));
        }
        if !element.style.is_empty() {
            let style: Vec<String> = element
                .style
                .iter()
                .map(|(name, value)| format!("{name}:{value}"))
                .collect();
            out.push_str(&format!(" style=\"{}\"", escape_attr(&style.join(";"))));
        }
        if element.tag == "input" || element.tag == "img" {
            out.push_str(" />");
            return;
        }
        out.push('>');
        for (run, highlight) in &element.text {
            match highlight {
                Some(mark) => out.push_str(&format!(
                    "<span style=\"color:{}\">{}</span>",
                    color(*mark),
                    escape_text(run)
                )),
                None => out.push_str(&escape_text(run)),
            }
        }
        for child in &element.children {
            self.serialize(*child, out);
        }
        out.push_str(&format!("</{}>", element.tag));
    }
}

fn css_font(font: &crate::text_engine::FontSpec) -> String {
    let weight = match font.weight {
        crate::text_engine::Weight::Regular => 400,
        crate::text_engine::Weight::Medium => 500,
        crate::text_engine::Weight::Semibold => 600,
        crate::text_engine::Weight::Bold => 700,
    };
    let family = match font.design {
        crate::text_engine::FontDesign::Mono => {
            "ui-monospace, Menlo, Consolas, monospace"
        }
        _ => "system-ui, -apple-system, \"Segoe UI\", sans-serif",
    };
    format!("{weight} {}px {family}", font.size)
}

/// The glue's gradient lowering, mirrored: a proportional centre or
/// line, a reach that spells farthest-corner when the box decides it.
fn css_gradient(gradient: &crate::layout::Gradient) -> String {
    match gradient {
        crate::layout::Gradient::Radial { center, start, end, inner, outer, aspect } => {
            let reach = match end {
                Some(radius) => px(*radius),
                None => "farthest-corner".to_string(),
            };
            let stop = match end {
                Some(radius) => px(*radius),
                None => "100%".to_string(),
            };
            match end {
                // the ellipse: the X radius is on the wire and the Y
                // radius is that times the aspect
                Some(radius) if *aspect != 1.0 && *radius > 0.0 => format!(
                    "radial-gradient(ellipse {} {} at {}% {}%, {} {:.2}%, {} 100%)",
                    px(*radius),
                    px(radius * aspect),
                    center.x * 100.0,
                    center.y * 100.0,
                    color(*inner),
                    (start / radius) * 100.0,
                    color(*outer),
                ),
                _ => format!(
                    "radial-gradient(circle {reach} at {}% {}%, {} {}, {} {stop})",
                    center.x * 100.0,
                    center.y * 100.0,
                    color(*inner),
                    px(*start),
                    color(*outer),
                ),
            }
        }
        crate::layout::Gradient::Linear { start, end, from, to } => {
            let degrees =
                ((end.x - start.x).atan2(-(end.y - start.y))).to_degrees();
            format!("linear-gradient({degrees:.2}deg, {}, {})", color(*from), color(*to))
        }
    }
}

fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}

/// Tag hints are a tiny closed-ish set (tr, td, table, a, span…) — a
/// leaked str keeps the toy tree's static tags simple.
fn leak_tag(tag: &str) -> &'static str {
    match tag {
        "table" => "table",
        "thead" => "thead",
        "tbody" => "tbody",
        "tr" => "tr",
        "td" => "td",
        "th" => "th",
        "a" => "a",
        "span" => "span",
        "h1" => "h1",
        "button" => "button",
        _ => "div",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[derive(Clone)]
    struct Page {
        on: State<bool>,
    }

    impl Component for Page {
        fn body(self, _ctx: &Context) -> impl View {
            let on = self.on.get();
            crate::vstack!(
                text("hello, prerender").foreground_color(Color::hex(0xF5F5F5)),
                text(if on { "on" } else { "off" }),
            )
            .background_color(Color::hex(0x101216))
        }
    }

    /// The page paints from the string alone: markup with ids, inline
    /// styles at rest, pseudo rules aside — and twice over, the same
    /// bytes (the serializer is deterministic by construction).
    #[test]
    fn a_page_renders_to_stable_html() {
        let size = Size { width: 300.0, height: 200.0 };
        let first = render(&Page { on: State::new(false) }, size);
        let second = render(&Page { on: State::new(false) }, size);
        assert_eq!(first.html, second.html, "deterministic bytes");
        assert!(first.html.contains("data-hydrate=\"1\""));
        assert!(first.html.contains("hello, prerender"));
        assert!(first.html.contains("display:flex"));
        assert!(!first.html.contains("position:absolute"), "a flow page ships in the flow");
    }

    /// Hydration's other half: a fresh runtime adopts the same scene
    /// and its first frame says NOTHING — the page was already true.
    #[test]
    fn adoption_diffs_to_silence() {
        let size = Size { width: 300.0, height: 200.0 };
        let built = Page { on: State::new(false) };
        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&built, size);
        assert!(!mount.is_empty());

        // the same state, a new world: adopt, then diff
        let served = Page { on: State::new(false) };
        let fresh = Runtime::new();
        fresh.dom_adopt(&served, size);
        let first = fresh.dom_frame(&served, size);
        assert!(first.is_empty(), "the adopted page is already true: {first:?}");

        // and the page is LIVE: a state change speaks normally
        served.on.set(true);
        let patches = fresh.dom_frame(&served, size);
        assert!(!patches.is_empty());
    }
}
