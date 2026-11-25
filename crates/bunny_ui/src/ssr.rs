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
use motor::state::Locale;
use crate::runtime::Runtime;
use crate::view::View;

/// One rendered page: the body markup and the pseudo-state rules.
pub struct SsrPage {
    pub html: String,
    /// The looks the page wears, one rule each — and the targets' cursor.
    pub css: String,
    /// The language the page was rendered in — the first tag of the
    /// locale, what `<html lang>` wants; the mount wears it already.
    pub lang: String,
    /// Which way the page reads, as `<html dir>` spells it: `"ltr"` or
    /// `"rtl"`; the mount wears it already.
    pub dir: &'static str,
}

/// Renders `root` at `size` in the default locale and serializes the
/// mount — the same patches a browser would receive, applied to a toy
/// tree here. [`render_in`] renders in a locale of the server's
/// choosing.
pub fn render(root: &impl View, size: Size) -> SsrPage {
    render_in(root, size, &Locale::default())
}

/// [`render`] in `locale` — a server that read the request's
/// `Accept-Language` passes `Locale::parse(header)`; the page is laid
/// out in that locale's direction, its words resolve against it, and
/// the mount wears its `lang` and `dir`. The glue sends the mount's own
/// language before the start, so the adopt runs in the language the
/// page was built in and only the reader's own report moves it.
pub fn render_in(root: &impl View, size: Size, locale: &Locale) -> SsrPage {
    let runtime = Runtime::new();
    runtime.set_locale(Some(locale.clone()));
    let patches = runtime.dom_frame(root, size);
    let mut tree = Tree::new(size);
    for patch in &patches {
        tree.apply(patch);
    }
    let mut css: Vec<String> = vec![
        "[data-path]{cursor:default}".to_string(),
        // a link is pressable wherever it stands — the glue's twin rule
        "a[href][href]{cursor:pointer;pointer-events:auto}".to_string(),
        // a semantic tag never brings the browser's own look — the
        // glue's twin rules
        ":where(#app) :where(h1,h2,h3,h4,h5,h6,p,figure,blockquote,pre,ol,ul,li,dl,dd){margin:0;padding:0}".to_string(),
        ":where(#app) :where(h1,h2,h3,h4,h5,h6,code,pre,kbd,samp,small,b,strong,i,em){font:inherit}".to_string(),
        ":where(#app) :where(ol,ul){list-style:none}".to_string(),
        ":where(#app) :where(a){color:inherit;text-decoration:none}".to_string(),
    ];
    css.extend(tree.rules.values().cloned());
    SsrPage {
        html: tree.serialize_root(),
        css: css.join("\n"),
        lang: runtime.locale().identifier().to_string(),
        dir: if runtime.layout_direction().is_rtl() { "rtl" } else { "ltr" },
    }
}

/// A whole document: the page, its stylesheet, and the boot scripts.
/// `wasm` names the binary the glue will fetch; the mount carries
/// `data-hydrate` so the glue adopts instead of rebuilding.
pub fn render_document(root: &impl View, size: Size, wasm: &str, glue: &str) -> String {
    render_document_in(root, size, wasm, glue, &Locale::default())
}

/// [`render_document`] in `locale`: the document's root says the
/// language and the direction too.
pub fn render_document_in(
    root: &impl View,
    size: Size,
    wasm: &str,
    glue: &str,
    locale: &Locale,
) -> String {
    let page = render_in(root, size, locale);
    format!(
        "<!doctype html>\n<html lang=\"{lang}\" dir=\"{dir}\">\n  <head>\n    <meta charset=\"utf-8\" />\n    \
         <style>\nhtml,body{{margin:0;height:100%;background:#101216;display:grid;place-items:center}}\n\
         #app{{position:relative;width:{width}px;height:{height}px;overflow:hidden}}\n{css}\n</style>\n  </head>\n  <body>\n    \
         {html}\n    <script>\n      window.BUNNY_WASM = \"{wasm}\";\n    </script>\n    \
         <script src=\"{glue}\"></script>\n  </body>\n</html>\n",
        lang = page.lang,
        dir = page.dir,
        width = size.width,
        height = size.height,
        css = page.css,
        html = page.html,
    )
}

/// The page kept alive across frames: the toy tree a build renders
/// into, taking every frame's patches the way the browser's glue does.
/// What a test replays a session on, to see the page a browser would
/// hold after it — and the action path a click on each element sends.
pub struct Replay {
    tree: Tree,
}

impl Replay {
    pub fn new(size: Size) -> Replay {
        Replay { tree: Tree::new(size) }
    }

    /// One frame's patches, in order.
    pub fn apply(&mut self, patches: &[DomPatch]) {
        for patch in patches {
            self.tree.apply(patch);
        }
    }

    /// Every element that shows an action path, by id, with the path a
    /// click on it sends: one shown as `~` and the rest is told against
    /// the nearest `data-base` at or above the element — the element
    /// itself when it carries one — which is the glue's own reading. A
    /// relative path with no base above it stays as it is shown.
    pub fn action_paths(&self) -> BTreeMap<u32, String> {
        fn walk(tree: &Tree, id: u32, base: Option<&str>, out: &mut BTreeMap<u32, String>) {
            let Some(element) = tree.elements.get(&id) else {
                return;
            };
            let base = element.attrs.get("data-base").map(String::as_str).or(base);
            if let Some(shown) = element.attrs.get("data-path") {
                let path = match (shown.strip_prefix('~'), base) {
                    (Some(rest), Some(base)) => format!("{base}{rest}"),
                    _ => shown.clone(),
                };
                out.insert(id, path);
            }
            for child in &element.children {
                walk(tree, *child, base, out);
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.tree, 0, None, &mut out);
        out
    }

    /// The page's markup, as a build would serve it.
    pub fn html(&self) -> String {
        self.tree.serialize_root()
    }
}

/// A toy element: enough DOM to receive the mount and print itself.
#[derive(Clone)]
struct Element {
    tag: &'static str,
    /// `data-n` — the identity hydration adopts by.
    id: u32,
    attrs: BTreeMap<&'static str, String>,
    /// The element's own inline declarations: its box, its geometry.
    style: BTreeMap<&'static str, String>,
    /// The words, each stretch with the declarations of its `<span>` —
    /// `None` for plain words between them.
    text: Vec<(String, Option<String>)>,
    children: Vec<u32>,
    /// The look it wears — a class on the page's sheet.
    rule: Option<u64>,
    /// An inline drawing's shapes, written after the element's own.
    drawing: Option<std::rc::Rc<crate::drawing::Drawing>>,
}

struct Tree {
    elements: BTreeMap<u32, Element>,
    /// The looks, by hash: the rule's whole text, states included.
    rules: BTreeMap<u64, String>,
}

/// The class a look is worn by. The selector doubles it so the rule
/// outranks a page's own class rules, as an inline declaration did.
pub(crate) fn rule_class(rule: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = Vec::new();
    let mut value = rule;
    loop {
        digits.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.reverse();
    format!("b_{}", String::from_utf8(digits).expect("ascii digits"))
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

/// A length: an `f64`, or a flow record's `f32` printed as the `f64`
/// it widens to — for every value an `f32` holds, the text the page
/// served when the record held `f64`.
fn px(value: impl Into<f64>) -> String {
    let value: f64 = value.into();
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
            rule: None,
            drawing: None,
        };
        root.attrs.insert("id", "app".to_string());
        root.attrs.insert("data-hydrate", "1".to_string());
        // the box the page was laid out in: the boot adopts the served
        // elements at THIS size, where the scene is the one they show,
        // and lays out at the reader's from there
        root.attrs.insert("data-width", size.width.to_string());
        root.attrs.insert("data-height", size.height.to_string());
        // the window is a one-slot column: its child can take the box
        root.style.insert("display", "flex".into());
        root.style.insert("flex-direction", "column".into());
        let mut elements = BTreeMap::new();
        elements.insert(0, root);
        Tree { elements, rules: BTreeMap::new() }
    }

    /// The glue's `createElementOf`, mirrored.
    fn create(&mut self, id: u32, kind: CreateKind, hints: &crate::dom::DomHints) {
        // the kind's declarations are the look's: the element keeps its
        // tag and its attributes
        let (tag, _) = kind_shape(kind);
        let mut element = Element {
            tag,
            id,
            attrs: BTreeMap::new(),
            style: BTreeMap::new(),
            text: Vec::new(),
            children: Vec::new(),
            rule: None,
            drawing: None,
        };
        // the glue's three attributes on a video: muted, inline and
        // autoplaying — the audio is the app's business
        if matches!(kind, CreateKind::Video) {
            for name in ["autoplay", "muted", "playsinline"] {
                element.attrs.insert(name, String::new());
            }
        }
        if let Some(tag_hint) = &hints.tag {
            element.tag = leak_tag(tag_hint);
        }
        if matches!(kind, CreateKind::Scroll) {
            // a scroll box wears its kind as a mark: its look is a
            // class, which says nothing to a hydration that must wire
            // the wheel
            element.attrs.insert("data-k", "4".to_string());
        }
        if let Some(class) = &hints.class {
            element.attrs.insert("class", class.to_string());
        }
        if let Some(dom_id) = hints.dom_id() {
            element.attrs.insert("id", dom_id.to_string());
        }
        if let Some(href) = hints.href() {
            element.attrs.insert("href", href.to_string());
        }
        if let Some(test_id) = hints.test_id() {
            element.attrs.insert("data-testid", test_id.to_string());
        }
        write_control(&mut element, hints.control());
        write_voice(&mut element, hints.voice(), hints.label());
        self.elements.insert(id, element);
    }

    /// Copies `source` and its subtree under fresh ids, pre-order from
    /// `next` — the twin of the glue's `cloneNode`.
    fn clone_into(&mut self, source: u32, next: &mut u32) {
        let Some(mut copy) = self.elements.get(&source).cloned() else {
            return;
        };
        let id = *next;
        *next += 1;
        copy.id = id;
        let children = std::mem::take(&mut copy.children);
        copy.children = children
            .iter()
            .map(|child| {
                let child_id = *next;
                self.clone_into(*child, next);
                child_id
            })
            .collect();
        self.elements.insert(id, copy);
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
            DomPatch::Clone { id, parent, before, template, base } => {
                // the copy takes the template's subtree with ids counted
                // in pre-order from its own, as the lowering numbered them
                let mut next = *id;
                self.clone_into(*template, &mut next);
                // and its own base, over the one it was copied with
                if let Some(base) = base
                    && let Some(element) = self.elements.get_mut(id)
                {
                    element.attrs.insert("data-base", base.to_string());
                }
                let Some(parent) = self.elements.get_mut(parent) else {
                    return;
                };
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
            DomPatch::SetPath { id, path, base_len } => {
                if let Some(element) = self.elements.get_mut(id) {
                    match path {
                        // as the page shows it: `~` and the rest when
                        // the path lies under its base
                        Some(path) => {
                            element.attrs.insert("data-path", crate::dom::shown_path(path, *base_len));
                        }
                        None => {
                            element.attrs.remove("data-path");
                        }
                    }
                }
            }
            DomPatch::SetBase { id, base } => {
                if let Some(element) = self.elements.get_mut(id) {
                    element.attrs.insert("data-base", base.to_string());
                }
            }
            DomPatch::SetContent { id, text } => {
                if let Some(element) = self.elements.get_mut(id) {
                    element.text = vec![(text.to_string(), None)];
                }
            }
            DomPatch::RemoveChildren { id, .. } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                let mut doomed = std::mem::take(&mut element.children);
                while let Some(child) = doomed.pop() {
                    if let Some(gone) = self.elements.remove(&child) {
                        doomed.extend(gone.children);
                    }
                }
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
            DomPatch::SetIframe { id, src, sealed } => {
                if let Some(element) = self.elements.get_mut(id) {
                    if *sealed {
                        // a document: the sandbox with no powers, and
                        // the page itself instead of a url
                        element.attrs.remove("src");
                        element.attrs.insert("sandbox", String::new());
                        element.attrs.insert("srcdoc", src.to_string());
                    } else {
                        element.attrs.remove("sandbox");
                        element.attrs.remove("srcdoc");
                        element.attrs.insert("src", src.to_string());
                    }
                }
            }
            DomPatch::SetVideo { id, mirrored, cover, radius, .. } => {
                // the stream is a runtime object the page registers
                // after boot — nothing of it serializes; the fit, the
                // mirror and the radius do, as the glue writes them
                if let Some(element) = self.elements.get_mut(id) {
                    let fit = if *cover { "cover" } else { "contain" };
                    element.style.insert("object-fit", fit.to_string());
                    if *mirrored {
                        element.style.insert("scale", "-1 1".to_string());
                    } else {
                        element.style.remove("scale");
                    }
                    if *radius > 0.0 {
                        element.style.insert("border-radius", px(f64::from(*radius)));
                    } else {
                        element.style.remove("border-radius");
                    }
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
            DomPatch::DefineRule { rule, kind, flags, style, layout, text } => {
                let class = rule_class(*rule);
                let css = rule_text(&class, *kind, *flags & 1 != 0, style, layout, text.as_deref());
                self.rules.insert(*rule, css);
            }
            DomPatch::UseRule { id, rule } => {
                if let Some(element) = self.elements.get_mut(id) {
                    element.rule = Some(*rule);
                }
            }
            DomPatch::SetBox {
                id,
                width,
                height,
                max_width,
                max_height,
                slot_y,
                at,
                span,
                sticky,
            } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                // a pinned size is a floor too: a flex line never
                // shrinks a box below its minimum, whichever its axis
                for (name, value) in [
                    ("width", width),
                    ("min-width", width),
                    ("height", height),
                    ("min-height", height),
                    ("max-width", max_width),
                    ("max-height", max_height),
                ] {
                    match value {
                        Some(value) => {
                            element.style.insert(name, px(f64::from(*value)));
                        }
                        None => {
                            element.style.remove(name);
                        }
                    }
                }
                match slot_y {
                    Some(slot) => {
                        element.style.insert("position", "absolute".into());
                        element.style.insert("top", px(f64::from(*slot)));
                        element.style.insert("left", "0".into());
                        element.style.insert("right", "0".into());
                    }
                    None => {
                        for name in ["position", "top", "left", "right"] {
                            element.style.remove(name);
                        }
                    }
                }
                for name in [
                    "margin-left",
                    "margin-top",
                    "align-self",
                    "justify-self",
                    "z-index",
                    "grid-column",
                    "grid-row",
                    "bottom",
                    "pointer-events",
                ] {
                    element.style.remove(name);
                }
                if let Some(at) = at {
                    if at.layered {
                        element.style.insert("margin-left", px(at.x));
                        element.style.insert("margin-top", px(at.y));
                        element.style.insert(
                            "align-self",
                            if at.fill_y { "stretch" } else { "start" }.into(),
                        );
                        element.style.insert(
                            "justify-self",
                            if at.fill_x { "stretch" } else { "left" }.into(),
                        );
                    } else {
                        element.style.insert("position", "absolute".into());
                        element.style.insert("left", px(at.x));
                        element.style.insert("top", px(at.y));
                        element.style.insert("z-index", "1".into());
                        if at.fill_x {
                            element.style.insert("right", "0".into());
                        }
                        if at.fill_y {
                            element.style.insert("bottom", "0".into());
                        }
                    }
                }
                if let Some(span) = span {
                    if span.fill_x {
                        element.style.insert("justify-self", "stretch".into());
                    }
                    element
                        .style
                        .insert("grid-column", format!("span {}", span.columns));
                    element
                        .style
                        .insert("grid-row", format!("span {}", span.rows));
                }
                // last: the browser holds the box at the line — unless the
                // stack took it out of the flow, where CSS has no stick
                if let Some(sticky) = sticky.filter(|_| at.is_none_or(|at| at.layered)) {
                    element.style.insert("position", "sticky".into());
                    element.style.insert("top", px(f64::from(sticky.top)));
                    if sticky.takes_pointer {
                        element.style.insert("pointer-events", "auto".into());
                    }
                }
            }
            DomPatch::SetMarks { id, tooltip, group_owner } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                match tooltip {
                    Some(tip) => {
                        element.attrs.insert("data-tip", tip.to_string());
                    }
                    None => {
                        element.attrs.remove("data-tip");
                    }
                }
                match group_owner {
                    Some(owner) => {
                        element.attrs.insert("data-g", owner.to_string());
                    }
                    None => {
                        element.attrs.remove("data-g");
                    }
                }
            }
            DomPatch::SetText { id, text } => {
                // the words and their spans: the face is the look's
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                element.text.clear();
                let raw = text.content.as_bytes();
                let words = |from: usize, to: usize| String::from_utf8_lossy(&raw[from..to]).into_owned();
                let marks = text.highlights.as_deref();
                let mut cursor = 0usize;
                for stretch in marks.map(|marks| crate::dom::text_stretches(&text.content, marks)).unwrap_or_default() {
                    if stretch.start > cursor {
                        element.text.push((words(cursor, stretch.start), None));
                    }
                    element.text.push((words(stretch.start, stretch.end), Some(stretch_css(&stretch))));
                    cursor = stretch.end;
                }
                if cursor < raw.len() || element.text.is_empty() {
                    element.text.push((words(cursor, raw.len()), None));
                }
                let rules = marks.map_or(crate::layout::Decoration::NONE, |marks| marks.decoration);
                match rules_css(rules) {
                    Some((line, rule_color)) => {
                        element.style.insert("text-decoration-line", line);
                        match rule_color {
                            Some(rule_color) => element.style.insert("text-decoration-color", rule_color),
                            None => element.style.remove("text-decoration-color"),
                        };
                    }
                    None => {
                        element.style.remove("text-decoration-line");
                        element.style.remove("text-decoration-color");
                    }
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
                // what the field is — the glue's op 7, mirrored: a secret
                // is a password box, and the two attributes a field does
                // not say are not written
                if element.tag == "input" {
                    let kind = if field.secret { "password" } else { "text" };
                    element.attrs.insert("type", kind.to_string());
                }
                match field.autocomplete {
                    Some(holds) => element.attrs.insert("autocomplete", holds.token().to_string()),
                    None => element.attrs.remove("autocomplete"),
                };
                match &field.label {
                    Some(label) => element.attrs.insert("aria-label", label.to_string()),
                    None => element.attrs.remove("aria-label"),
                };
            }
            DomPatch::SetHints { id, class, address } => {
                let address = address.as_deref();
                let dom_id = address.and_then(|address| address.dom_id.as_ref());
                let href = address.and_then(|address| address.href.as_ref());
                let test_id = address.and_then(|address| address.test_id.as_ref());
                if let Some(element) = self.elements.get_mut(id) {
                    match class {
                        Some(class) => element.attrs.insert("class", class.to_string()),
                        None => element.attrs.remove("class"),
                    };
                    match dom_id {
                        Some(dom_id) => element.attrs.insert("id", dom_id.to_string()),
                        None => element.attrs.remove("id"),
                    };
                    match href {
                        Some(href) => element.attrs.insert("href", href.to_string()),
                        None => element.attrs.remove("href"),
                    };
                    match test_id {
                        Some(test_id) => element.attrs.insert("data-testid", test_id.to_string()),
                        None => element.attrs.remove("data-testid"),
                    };
                    write_control(
                        element,
                        address
                            .and_then(|address| address.control)
                            .map_or(0, crate::layout::ControlState::wire),
                    );
                    write_voice(
                        element,
                        address.map_or(0, |address| address.voice.wire()),
                        address.and_then(|address| address.label.as_deref()),
                    );
                }
            }
            DomPatch::Move { id, parent, before } => {
                // a mount never moves an element; a replayed session does
                for element in self.elements.values_mut() {
                    element.children.retain(|child| child != id);
                }
                let Some(parent) = self.elements.get_mut(parent) else {
                    return;
                };
                let at = match before {
                    0 => parent.children.len(),
                    anchor => parent
                        .children
                        .iter()
                        .position(|child| child == anchor)
                        .unwrap_or(parent.children.len()),
                };
                parent.children.insert(at, *id);
            }
            DomPatch::SetImage { id, image } => {
                // the bytes arrive with the wasm: the element wears the
                // identity it waits for, and the glue fills the source
                // when the platform has the picture
                if let Some(element) = self.elements.get_mut(id) {
                    let key = image.key;
                    element
                        .attrs
                        .insert("data-img", format!("{}:{}", key >> 32, key as u32));
                    if image.cover {
                        element.style.insert("object-fit", "cover".into());
                    } else if image.contain {
                        element.style.insert("object-fit", "contain".into());
                    }
                }
            }
            DomPatch::SetDrawing { id, drawing } => {
                // the same tree the glue builds: the root's attributes on
                // the element, its shapes inside, every value escaped
                if let Some(element) = self.elements.get_mut(id) {
                    for (code, value) in drawing.drawing.root_attributes(drawing.own_size) {
                        element.attrs.insert(crate::drawing::ATTRIBUTES[usize::from(*code)], value.spell(*id));
                    }
                    element.drawing = Some(std::rc::Rc::clone(&drawing.drawing));
                }
            }
            DomPatch::SetLanguage { id, lang, dir } => {
                if let Some(element) = self.elements.get_mut(id) {
                    element.attrs.insert("lang", lang.to_string());
                    element.attrs.insert("dir", if dir.is_rtl() { "rtl" } else { "ltr" }.to_string());
                }
            }
            DomPatch::SetScroll { .. }
            | DomPatch::SetIcon { .. }
            | DomPatch::Reveal { .. }
            | DomPatch::SetAnchor { .. }
            | DomPatch::Focus { .. } => {
                // scroll offsets, icon geometry and the keyboard's
                // place arrive after boot; a built page starts at rest
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
            if *name == "class" {
                continue;
            }
            out.push_str(&format!(" {name}=\"{}\"", escape_attr(value)));
        }
        // the page's own classes first, the look's last
        let mut classes: Vec<String> = Vec::new();
        if let Some(class) = element.attrs.get("class") {
            classes.push(class.clone());
        }
        if let Some(rule) = element.rule {
            classes.push(rule_class(rule));
        }
        if !classes.is_empty() {
            out.push_str(&format!(" class=\"{}\"", escape_attr(&classes.join(" "))));
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
        for (run, look) in &element.text {
            match look {
                Some(look) => out.push_str(&format!(
                    "<span style=\"{}\">{}</span>",
                    escape_attr(look),
                    escape_text(run)
                )),
                None => out.push_str(&escape_text(run)),
            }
        }
        if let Some(drawing) = &element.drawing {
            for shape in &drawing.root.children {
                serialize_shape(shape, element.id, out);
            }
        }
        for child in &element.children {
            self.serialize(*child, out);
        }
        out.push_str(&format!("</{}>", element.tag));
    }
}

/// One shape of a drawing and what it holds, as element `scope` wears
/// them: the tag and the names from the tables, every value escaped.
fn serialize_shape(shape: &crate::drawing::Node, scope: u32, out: &mut String) {
    let tag = crate::drawing::TAGS[usize::from(shape.tag)];
    out.push('<');
    out.push_str(tag);
    for (code, value) in &shape.attributes {
        let name = crate::drawing::ATTRIBUTES[usize::from(*code)];
        out.push_str(&format!(" {name}=\"{}\"", escape_attr(&value.spell(scope))));
    }
    out.push('>');
    for child in &shape.children {
        serialize_shape(child, scope, out);
    }
    out.push_str(&format!("</{tag}>"));
}

/// The tag an element of this kind is, and the declarations its kind
/// brings along — the look carries them, the element wears the tag.
fn kind_shape(kind: CreateKind) -> (&'static str, &'static [(&'static str, &'static str)]) {
    match kind {
        CreateKind::Canvas => ("canvas", &[]),
        CreateKind::Image => ("img", &[("pointer-events", "none")]),
        CreateKind::Iframe => (
            "iframe",
            &[
                ("border", "0"),
                ("box-sizing", "border-box"),
                ("min-width", "0"),
                ("min-height", "0"),
            ],
        ),
        CreateKind::Video => (
            "video",
            &[
                ("display", "block"),
                ("pointer-events", "none"),
                ("box-sizing", "border-box"),
                ("min-width", "0"),
                ("min-height", "0"),
            ],
        ),
        CreateKind::Icon | CreateKind::Drawing => ("svg", &[("pointer-events", "none")]),
        CreateKind::Field => (
            "input",
            &[
                ("box-sizing", "border-box"),
                ("padding", "5px 8px"),
                ("outline", "none"),
                ("border", "0"),
                ("background", "transparent"),
            ],
        ),
        CreateKind::Editor => (
            "textarea",
            &[
                ("box-sizing", "border-box"),
                ("padding", "5px 8px"),
                ("outline", "none"),
                ("border", "0"),
                ("background", "transparent"),
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
            ],
        ),
        CreateKind::FlexRow => (
            "div",
            &[
                ("display", "flex"),
                ("flex-direction", "row"),
                ("box-sizing", "border-box"),
            ],
        ),
        CreateKind::Layers | CreateKind::Grid => (
            "div",
            &[
                ("display", "grid"),
                ("box-sizing", "border-box"),
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
        CreateKind::Rectangle => (
            "div",
            &[("box-sizing", "border-box"),
              ("background", "currentColor")],
        ),
        CreateKind::Ellipse => (
            "div",
            &[("box-sizing", "border-box"),
              ("background", "currentColor"), ("border-radius", "50%")],
        ),
        CreateKind::Group | CreateKind::Box => (
            "div",
            &[
                ("display", "flex"),
                ("flex-direction", "column"),
                ("box-sizing", "border-box"),
            ],
        ),
    }
}

/// The four sides of a line set side by side, clockwise from the top: the
/// border longhand and margin each one names, and the offset of the inset
/// shadow that draws it on a kind that keeps no border (the glue's `EDGES`).
const EDGES: [Side; 4] = [
    ("border-top", "margin-top", |width| format!("0 {}", px(width))),
    ("border-inline-end", "margin-inline-end", |width| format!("{} 0", px(-width))),
    ("border-bottom", "margin-bottom", |width| format!("0 {}", px(-width))),
    ("border-inline-start", "margin-inline-start", |width| format!("{} 0", px(width))),
];

/// A side's border longhand, its margin and its inset shadow's offset.
type Side = (&'static str, &'static str, fn(f64) -> String);

/// One layer of the shadow's own, as CSS writes it.
fn css_shadow(layer: crate::shapes::BoxShadow) -> String {
    format!(
        "{}{} {} {} {} {}",
        if layer.inset { "inset " } else { "" },
        px(layer.x),
        px(layer.y),
        px(layer.blur),
        px(layer.spread),
        color(layer.color)
    )
}

/// The house halo falls off as (1 − d/r)² from the edge and stops at
/// its reach; one CSS blur is a Gaussian. Two blurred layers — a spread
/// and a blur in reaches, and an exponent on the ink's opacity — follow
/// the curve within 4/255 for the opacities a shadow wears, fitted
/// against Chrome's own blur (σ ≈ 0.52 × the blur radius). The glue
/// spells the same two.
const HALO_LAYERS: [(f64, f64, f64); 2] = [(0.3262, 0.5124, 0.8489), (0.0738, 0.1164, 0.3444)];

fn halo(radius: f64, ink: Color, (x, y): (f64, f64)) -> impl Iterator<Item = String> {
    let reach = radius.max(1.0);
    let opacity = f64::from(ink.a) / 255.0;
    HALO_LAYERS.iter().map(move |&(spread, blur, weight)| {
        let alpha = 1.0 - (1.0 - opacity).powf(weight);
        format!(
            "{}px {}px {}px {}px rgba({}, {}, {}, {})",
            decimal(x, 3),
            decimal(y, 3),
            decimal(blur * reach, 3),
            decimal(spread * reach, 3),
            ink.r,
            ink.g,
            ink.b,
            decimal(alpha, 4),
        )
    })
}

/// A number rounded to `places` and written without trailing zeros — the
/// glue's `Number(value.toFixed(places))`.
fn decimal(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    if text == "-0" { "0".to_string() } else { text.to_string() }
}

/// The ring a control the keyboard reached wears — the glue's sheet
/// spells the same one.
const FOCUS_RING: &str = "outline:2px solid Highlight;outline-offset:2px";

/// The kinds whose line is a CSS border: the wrappers, whose children
/// each fill the box (a group, a box, layers), and the shapes, which hold
/// none. A text, a field, a picture, a scroll or a stack keeps the outline.
fn lined(kind: CreateKind) -> bool {
    matches!(
        kind,
        CreateKind::Group | CreateKind::Box | CreateKind::Layers | CreateKind::Rectangle | CreateKind::Ellipse
    )
}

/// The look's rule: the kind's defaults, the flow record, the style,
/// the text's face — and its states, one rule each. The twin of the
/// glue's `defineRule`: a served page must agree with a mounted one.
fn rule_text(
    class: &str,
    kind: CreateKind,
    lays_itself_out: bool,
    style: &crate::dom::DomLook,
    layout: &crate::dom::DomLayout,
    text: Option<&crate::dom::DomText>,
) -> String {
    let selector = format!(".{class}.{class}");
    let mut base: BTreeMap<&'static str, String> = BTreeMap::new();
    let (_, defaults) = kind_shape(kind);
    for &(name, value) in defaults {
        base.insert(name, value.to_string());
    }
    // the table family lays itself out — the browser's own display
    // wins and our flex steps aside; a plain inline tag keeps its own
    // display too, and the floor its kind gave it
    if lays_itself_out {
        base.remove("display");
        base.remove("flex-direction");
        base.remove("min-width");
        base.remove("min-height");
    } else if layout.plain {
        base.remove("display");
        base.remove("flex-direction");
    }
    if layout.relative {
        base.insert("position", "relative".into());
        base.insert("isolation", "isolate".into());
    }
    // a negative spacing overlaps the items, as SwiftUI's does; CSS has
    // no negative gap, so each item after the first is pulled back by a
    // negative margin along the line instead, and painted whole over the
    // one before, as a layer is
    let overlap = layout
        .gap
        .filter(|gap| *gap < 0.0 && layout.wrap.is_none())
        .and_then(|gap| match kind {
            CreateKind::FlexRow => Some(("margin-inline-start", gap)),
            CreateKind::FlexColumn => Some(("margin-block-start", gap)),
            _ => None,
        });
    if let Some(gap) = layout.gap.filter(|_| overlap.is_none()) {
        base.insert(
            if matches!(kind, CreateKind::Grid) {
                "column-gap"
            } else {
                "gap"
            },
            px(gap),
        );
    }
    if matches!(kind, CreateKind::Layers) && layout.base().is_some() {
        base.insert("grid-template-columns", "auto 0fr".into());
        base.insert("grid-template-rows", "auto 0fr".into());
    }
    if let Some(columns) = layout.columns() {
        base.insert(
            "grid-template-columns",
            format!("repeat({columns},minmax(0,1fr))"),
        );
        base.insert("align-items", "center".into());
        base.insert("align-content", "start".into());
    }
    if let Some(align) = layout.align.map(|align| align & 15) {
        base.insert(
            if matches!(kind, CreateKind::Grid) {
                "justify-items"
            } else {
                "align-items"
            },
            match align {
                1 => "center",
                2 => "flex-end",
                3 => "baseline",
                _ => "flex-start",
            }
            .into(),
        );
    }
    // layers: the same alignment across the cell, so a centred stack
    // centres both ways
    if matches!(kind, CreateKind::Layers)
        && let Some(align) = base.get("align-items").cloned()
    {
        base.insert("justify-items", align);
    }
    // a named vertical edge: a frame's column places its child along its
    // main axis, a layer pile and a grid hold every cell's child on the
    // block axis
    if let Some(edge) = layout.align.map(|align| align >> 4).filter(|edge| *edge > 0) {
        let edge = match edge {
            2 => "center",
            3 => "flex-end",
            _ => "flex-start",
        };
        match kind {
            CreateKind::Layers | CreateKind::Grid => base.insert("align-items", edge.into()),
            _ => base.insert("justify-content", edge.into()),
        };
    }
    // the record's sides are logical, and so are the properties: a
    // right-to-left mount puts the leading inset on the right by itself
    if let Some((top, trailing, bottom, leading)) = layout.padding {
        // a negative side is a negative margin: the content moves out and
        // the box gives the flow that much back, as a negative padding
        // does in the engine — CSS has no negative padding
        let inward = |side: f32| px(side.max(0.0));
        base.insert("padding-block", format!("{} {}", inward(top), inward(bottom)));
        base.insert("padding-inline", format!("{} {}", inward(leading), inward(trailing)));
        for (side, name) in [(top, "margin-block-start"), (bottom, "margin-block-end"), (leading, "margin-inline-start"), (trailing, "margin-inline-end")] {
            if side < 0.0 {
                base.insert(name, px(side));
            }
        }
        // pulled back over the box before it, it is painted whole over
        // that box, as SwiftUI's later view is, layers and all
        if top < 0.0 || leading < 0.0 {
            base.insert("isolation", "isolate".into());
        }
    }
    // an island that reads the other way: the browser orders its rows,
    // aligns its `start` and shapes its words that way, isolated from
    // the text around it
    if let Some(direction) = layout.direction {
        base.insert("direction", if direction.is_rtl() { "rtl" } else { "ltr" }.into());
        base.insert("unicode-bidi", "isolate".into());
    }
    if layout.grow {
        // the flexible child — and the classic flex footgun: a zeroed
        // min-size, or content refuses to shrink
        base.insert("flex", "1 1 0".into());
        base.insert("min-width", "0".into());
        base.insert("min-height", "0".into());
    }
    if layout.stretch {
        base.insert("align-self", "stretch".into());
    }
    if layout.fill {
        // take the offer, keep the content floor
        base.insert("flex", "1 1 auto".into());
        base.insert("min-width", "0".into());
        base.insert("min-height", "0".into());
        // in a grid cell the grow means nothing: the box stretches across
        base.insert("justify-self", "stretch".into());
        // a resizable picture takes its frame across as well (the glue's
        // `lookRules` says why)
        if matches!(kind, CreateKind::Image | CreateKind::Drawing) {
            base.insert("align-self", "stretch".into());
        }
    }
    if let Some(line_gap) = layout.wrap {
        if !matches!(kind, CreateKind::Grid) {
            base.insert("flex-wrap", "wrap".into());
        }
        base.insert("row-gap", px(line_gap));
    }
    let mut states: Vec<String> = Vec::new();
    // layers: one grid cell, and every child IN it — auto-placement
    // would give each layer a row of its own. Each child paints whole and
    // in its turn: a moved or positioned box inside a background would
    // otherwise paint over the base
    if matches!(kind, CreateKind::Layers) {
        match layout.base() {
            // an overlay's cell: the base in the first track, every layer
            // across it and a second track that is always empty, so no
            // layer grows the cell
            Some(base) => {
                let at = match base {
                    crate::dom::LayerBase::First => "first-child",
                    crate::dom::LayerBase::Last => "last-child",
                };
                states.push(format!("{selector}>*{{grid-area:1/1/3/3;z-index:0}}"));
                states.push(format!("{selector}>:{at}{{grid-area:1/1}}"));
            }
            None => states.push(format!("{selector}>*{{grid-area:1/1;z-index:0}}")),
        }
    }
    if let Some((side, gap)) = overlap {
        states.push(format!("{selector}>*+*{{{side}:{};isolation:isolate}}", px(gap)));
    }
    // a follower hangs its states off the GROUP's pointer: the same
    // rules, hung off the group's selector; a box without one listens
    // to its own
    let on = |state: &str| match style.group {
        Some(group) => format!("[data-g=\"{group}\"]:{state} {selector}"),
        None => format!("{selector}:{state}"),
    };
    if let Some(background) = style.background {
        base.insert("background-color", color(background));
    }
    if let Some(hover) = style.hover_background {
        states.push(format!("{}{{background-color:{}}}", on("hover"), color(hover)));
    }
    if let Some(pressed) = style.pressed_background {
        states.push(format!("{}{{background-color:{}}}", on("active"), color(pressed)));
    }
    // the line drawn inward: on a box, a CSS border, painted with the
    // background as the one box a page's own border is (an outline is a
    // second paint, and the browser blends its edges differently), and
    // the children pulled back under it by its width, so it lays nothing
    // out. A text, a picture or a control has no child to pull back: an
    // outline pulled in by its own width
    let edged = !style.border_edges.is_empty();
    let lined = (style.border.is_some() || edged) && lined(kind);
    let line = style.border_style.map_or("solid", crate::shapes::BorderStyle::css);
    // a line along one edge alone: each side its own longhand, as a page
    // writes `border-bottom`, or an inset shadow on a kind without one
    let sides: Vec<_> = style
        .border_edges
        .sides()
        .into_iter()
        .zip(EDGES)
        .filter(|((_, width), _)| *width > 0.0)
        .collect();
    if edged && lined {
        for &((ink, width), (border, _, _)) in &sides {
            base.insert(border, format!("{} {line} {}", px(width), color(ink)));
        }
        let pulls: Vec<String> = sides.iter().map(|&((_, width), (_, margin, _))| format!("{margin}:{}", px(-width))).collect();
        if !pulls.is_empty() {
            states.push(format!(":where({selector}>*){{{}}}", pulls.join(";")));
        }
    }
    if let Some((border, width)) = style.border {
        if lined {
            base.insert("border", format!("{} {line} {}", px(width), color(border)));
            // at no specificity: a margin the child names itself still wins
            states.push(format!(":where({selector}>*){{margin:{}}}", px(-width)));
        } else {
            base.insert("outline", format!("{} {line} {}", px(width), color(border)));
            base.insert("outline-offset", px(-width));
        }
    }
    if let Some(radii) = style.corner_radius {
        // one number when every corner shares it, four in the CSS
        // order otherwise — clockwise from top left
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
        base.insert("border-radius", value);
    }
    // the shadow's own layers, as CSS writes them, the halo and the
    // glass rim share one property
    let mut shadows: Vec<String> = style.box_shadows.iter().map(css_shadow).collect();
    if let Some((radius, shadow)) = style.shadow {
        shadows.extend(halo(radius, shadow, style.shadow_offset.unwrap_or_default()));
    }
    // `.offset(x, y)`: drawn away from its place, its neighbours unmoved
    if let Some((x, y)) = style.offset {
        base.insert("translate", format!("{} {}", px(x), px(y)));
    }
    // `.rotation_effect`: drawn turned about its pivot, the layout unmoved
    if let Some((degrees, x, y)) = style.rotation {
        // the f32 the wire carries, as the glue writes it
        #[allow(clippy::cast_possible_truncation, reason = "the wire's own precision")]
        let degrees = f64::from(degrees as f32);
        base.insert("rotate", format!("{degrees}deg"));
        base.insert("transform-origin", format!("{}% {}%", x * 100.0, y * 100.0));
    }
    if let Some((response, _)) = style.transition {
        // every colour the engine's springs move — the fill, the ink,
        // the border, the halo — and the transform
        let springs = ["background-color", "color", "border-color", "outline-color", "box-shadow", "transform"];
        let eased: Vec<String> = springs
            .iter()
            .map(|property| format!("{property} {response}s ease-out"))
            .collect();
        base.insert("transition", eased.join(", "));
    }
    if let Some(focus) = style.focus_border {
        states.push(format!("{selector}:focus{{outline-color:{c};caret-color:{c}}}", c = color(focus)));
    }
    if let Some(placeholder) = style.placeholder_color {
        states.push(format!("{selector}::placeholder{{color:{}}}", color(placeholder)));
    }
    if let Some(ink) = style.color {
        base.insert("color", color(ink));
    }
    if let Some(hover) = style.hover_color {
        states.push(format!("{}{{color:{}}}", on("hover"), color(hover)));
    }
    if let Some(pressed) = style.pressed_color {
        states.push(format!("{}{{color:{}}}", on("active"), color(pressed)));
    }
    if let Some(gradient) = &style.gradient {
        base.insert("background-image", css_gradient(gradient));
    }
    // the ramp the text paints with: the top layer, cut to the text;
    // what the box paints under it keeps its own box
    if let Some(ink) = &style.ink_gradient {
        let under = base.remove("background-image").unwrap_or_else(|| "none".into());
        base.insert("background-image", format!("{}, {under}", css_gradient(ink)));
        base.insert("-webkit-background-clip", "text, border-box".into());
        base.insert("background-clip", "text, border-box".into());
        base.insert("-webkit-text-fill-color", "transparent".into());
    }
    // inside a ramp, a box with its own ink paints its text in it again
    if style.own_ink {
        base.insert("-webkit-text-fill-color", "currentcolor".into());
    }
    // the box's floor
    if let Some((width, height)) = style.floor {
        if width > 0.0 {
            base.insert("min-width", px(width));
        }
        if height > 0.0 {
            base.insert("min-height", px(height));
        }
    }
    if style.clip {
        base.insert("overflow", "hidden".into());
    }
    if let Some(indicators) = style.scrollbar {
        base.insert(
            "scrollbar-width",
            match indicators {
                crate::layout::ScrollIndicators::Never => "none",
                crate::layout::ScrollIndicators::Hidden | crate::layout::ScrollIndicators::Visible => "thin",
            }
            .into(),
        );
    }
    if style.hidden {
        // the box keeps its place in the flow and shows nothing
        base.insert("visibility", "hidden".into());
    }
    if let Some(opacity) = style.opacity {
        base.insert("opacity", format!("{opacity}"));
    }
    if let Some(opacity) = style.hover_opacity {
        states.push(format!("{}{{opacity:{opacity}}}", on("hover")));
    }
    if let Some(opacity) = style.pressed_opacity {
        states.push(format!("{}{{opacity:{opacity}}}", on("active")));
    }
    if style.pass_through {
        base.insert("pointer-events", "none".into());
    }
    // the color filters in their order, then the blur
    let mut filters: Vec<String> = style.filters.iter().map(css_filter).collect();
    if let Some(blur) = style.blur {
        filters.push(format!("blur({blur}px)"));
    }
    if !filters.is_empty() {
        base.insert("filter", filters.join(" "));
    }
    // liquid glass, the half a browser owns: one native filter over
    // what is behind the element, and the rim as two inset shadows
    // along the lit diagonals
    if let Some(glass) = style.glass {
        let filter = format!(
            "blur({}px) saturate({}) brightness({})",
            glass.blur, glass.saturation, glass.brightness
        );
        base.insert("backdrop-filter", filter.clone());
        base.insert("-webkit-backdrop-filter", filter);
        if glass.rim_band > 0.0 {
            let spread = glass.rim_band.max(1.0);
            let soft = spread * 1.5;
            let rim = color(glass.rim);
            shadows.push(format!("inset {spread}px {spread}px {soft}px -{spread}px {rim}"));
            shadows.push(format!("inset -{spread}px -{spread}px {soft}px -{spread}px {rim}"));
        }
    }
    if edged && !lined {
        shadows.extend(sides.iter().map(|&((ink, width), (_, _, inward))| format!("inset {} 0 0 {}", inward(width), color(ink))));
    }
    if !shadows.is_empty() {
        base.insert("box-shadow", shadows.join(","));
    }
    // the keyboard's ring is an outline as well (the glue's sheet): while
    // it shows on a control, the ring takes the outline and the line
    // moves beneath it as an inset shadow — the box keeps its line
    if let Some((border, width)) = style.border.filter(|_| !lined) {
        shadows.push(format!("inset 0 0 0 {} {}", px(width), color(border)));
        states.push(format!("{selector}[role][tabindex]:focus-visible{{{FOCUS_RING};box-shadow:{}}}", shadows.join(",")));
    }
    if let Some(text) = text {
        // a text with the face declared above it names none of its own
        if !text.inherits_face {
            base.insert("font", css_font(&text.font));
            // the shorthand leaves the spacing alone; the face's own
            // advance rides beside it
            if text.font.tracking != 0.0 {
                base.insert("letter-spacing", format!("{}px", text.font.tracking));
            }
            if text.font.monospaced_digits {
                base.insert("font-variant-numeric", "tabular-nums".into());
            }
        }
        // after the font shorthand, which resets it — the served page
        // steps its lines the way the engine measured them
        if let Some(height) = text.line_height {
            base.insert("line-height", format!("{}px", f64::from(height)));
        }
        match text.text_align {
            Some(motor::views::TextAlignment::Center) => {
                base.insert("text-align", "center".into());
            }
            // `end`, not `right`: the trailing edge is the left one in a
            // right-to-left mount, and leading is the browser's own
            // `start`, which is why it is never written
            Some(motor::views::TextAlignment::Trailing) => {
                base.insert("text-align", "end".into());
            }
            _ => {}
        }
        // an inherited ink takes no color — on a text. A box's face
        // record is the face alone: the box's own ink stays
        if text.inherits_ink {
            if matches!(kind, CreateKind::Text) {
                base.remove("color");
            }
        } else {
            base.insert("color", color(text.color));
        }
        if text.truncation.is_some() {
            base.insert("overflow", "hidden".into());
            base.insert("text-overflow", "ellipsis".into());
            base.insert("white-space", "nowrap".into());
        }
        // at most this many lines, the last one closed with an ellipsis
        // right after its last word: the space a wrap leaves is dropped
        if text.line_limit > 0 {
            base.insert("display", "-webkit-box".into());
            base.insert("-webkit-box-orient", "vertical".into());
            base.insert("-webkit-line-clamp", text.line_limit.to_string());
            base.insert("overflow", "hidden".into());
            base.insert("white-space", "pre-line".into());
        }
    }
    let declarations: Vec<String> = base.iter().map(|(name, value)| format!("{name}:{value}")).collect();
    let mut out = format!("{selector}{{{}}}", declarations.join(";"));
    for state in states {
        out.push('\n');
        out.push_str(&state);
    }
    out
}

/// A stretch's `<span>` declarations: what it changes over the text.
fn stretch_css(stretch: &crate::dom::Stretch) -> String {
    let mut declarations = Vec::new();
    if let Some(ink) = stretch.color {
        declarations.push(format!("color:{}", color(ink)));
    }
    if let Some(ground) = stretch.background {
        declarations.push(format!("background-color:{}", color(ground)));
    }
    if let Some(weight) = stretch.weight {
        declarations.push(format!("font-weight:{}", weight.number()));
    }
    if let Some(italic) = stretch.italic {
        declarations.push(format!("font-style:{}", if italic { "italic" } else { "normal" }));
    }
    match (&stretch.family, stretch.mono) {
        (Some(family), mono) => declarations.push(format!(
            "font-family:\"{}\", {}",
            family.replace('"', ""),
            house_family(mono == Some(true))
        )),
        (None, Some(mono)) => declarations.push(format!("font-family:{}", house_family(mono))),
        (None, None) => {}
    }
    if let Some(size) = stretch.size {
        declarations.push(format!("font-size:{size}px"));
    }
    if let Some((line, rule_color)) = rules_css(stretch.decoration) {
        declarations.push(format!("text-decoration-line:{line}"));
        if let Some(rule_color) = rule_color {
            declarations.push(format!("text-decoration-color:{rule_color}"));
        }
    }
    declarations.join(";")
}

/// The `text-decoration-line` of a set of rules, and its colour when one
/// rule names its own — CSS has one decoration colour per element, so a
/// pair of rules in two colours wears the underline's.
fn rules_css(decoration: crate::layout::Decoration) -> Option<(String, Option<String>)> {
    use crate::layout::RuleInk;
    let line = match (decoration.underline, decoration.strikethrough) {
        (None, None) => return None,
        (Some(_), None) => "underline",
        (None, Some(_)) => "line-through",
        (Some(_), Some(_)) => "underline line-through",
    };
    let own = [decoration.underline, decoration.strikethrough].into_iter().flatten().find_map(|rule| match rule {
        RuleInk::Color(rule_color) => Some(color(rule_color)),
        RuleInk::Text => None,
    });
    Some((line.to_string(), own))
}

/// The house stacks a face falls back to.
fn house_family(mono: bool) -> &'static str {
    if mono {
        "ui-monospace, Menlo, Consolas, monospace"
    } else {
        "system-ui, -apple-system, \"Segoe UI\", sans-serif"
    }
}

fn css_font(font: &crate::text_engine::FontSpec) -> String {
    let weight = match font.weight {
        crate::text_engine::Weight::Thin => 100,
        crate::text_engine::Weight::ExtraLight => 200,
        crate::text_engine::Weight::Light => 300,
        crate::text_engine::Weight::Regular => 400,
        crate::text_engine::Weight::Medium => 500,
        crate::text_engine::Weight::Semibold => 600,
        crate::text_engine::Weight::Bold => 700,
        crate::text_engine::Weight::ExtraBold => 800,
        crate::text_engine::Weight::Black => 900,
    };
    let house = match font.design {
        crate::text_engine::FontDesign::Mono => "ui-monospace, Menlo, Consolas, monospace",
        _ => "system-ui, -apple-system, \"Segoe UI\", sans-serif",
    };
    // a face named goes first, the house stack behind it; the lean is
    // a real face too — the glue's `cssFont`, mirrored: a served page
    // must agree with a mounted one, or the looks it defines are worn
    // by the elements the glue mounts later under the same hash
    let family = match font.family.name() {
        Some(name) => format!("\"{}\", {house}", name.replace('"', "")),
        None => house.to_string(),
    };
    let lean = match font.slant {
        crate::text_engine::Slant::Italic => "italic ",
        _ => "",
    };
    format!("{lean}{weight} {}px {family}", font.size)
}

/// The glue's color filter, mirrored: its CSS function at the f32 the
/// wire carries. A negative contrast is an inversion and the contrast
/// it keeps, which CSS reads as one and refuses to write.
fn css_filter(filter: crate::color_filter::ColorFilter) -> String {
    use crate::color_filter::ColorFilter;
    #[allow(clippy::cast_possible_truncation, reason = "the wire's own precision")]
    let amount = f64::from(filter.wire().1 as f32);
    match filter {
        ColorFilter::Grayscale(_) => format!("grayscale({amount})"),
        ColorFilter::Saturation(_) => format!("saturate({amount})"),
        ColorFilter::Contrast(_) if amount < 0.0 => format!("invert(1) contrast({})", -amount),
        ColorFilter::Contrast(_) => format!("contrast({amount})"),
        ColorFilter::HueRotation(_) => format!("hue-rotate({amount}deg)"),
        ColorFilter::Invert => "invert(1)".into(),
    }
}

/// The glue's gradient lowering, mirrored: a proportional centre or
/// line, a reach that spells farthest-corner when the box decides it.
fn css_gradient(gradient: &crate::layout::Gradient) -> String {
    // the colors between the two ends, each at the place `at` names for
    // its location along the ramp
    let between = |at: &dyn Fn(f64) -> String| -> String {
        gradient.stops().iter().fold(String::new(), |mut out, (location, ink)| {
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!(", {} {}", color(ink), at(location)));
            out
        })
    };
    match gradient {
        crate::layout::Gradient::Radial { center, start, end, inner, outer, aspect, stops } => {
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
                Some(radius) if *aspect != 1.0 && *radius > 0.0 => {
                    let near = start / radius;
                    format!(
                        "radial-gradient(ellipse {} {} at {}% {}%, {} {:.2}%{}, {} 100%)",
                        px(*radius),
                        px(radius * aspect),
                        center.x * 100.0,
                        center.y * 100.0,
                        color(*inner),
                        near * 100.0,
                        between(&|location| format!("{:.2}%", (near + location * (1.0 - near)) * 100.0)),
                        color(*outer),
                    )
                }
                _ => {
                    let inside = match end {
                        Some(radius) if !stops.is_empty() => {
                            between(&|location| format!("{:.2}px", (radius - start).mul_add(location, *start)))
                        }
                        _ => between(&|location| format!("calc({:.2}px + {:.2}%)", (1.0 - location) * start, location * 100.0)),
                    };
                    format!(
                        "radial-gradient(circle {reach} at {}% {}%, {} {}{inside}, {} {stop})",
                        center.x * 100.0,
                        center.y * 100.0,
                        color(*inner),
                        px(*start),
                        color(*outer),
                    )
                }
            }
        }
        crate::layout::Gradient::Linear { start, end, from, to, stops } => {
            let degrees =
                ((end.x - start.x).atan2(-(end.y - start.y))).to_degrees();
            if stops.is_empty() {
                format!("linear-gradient({degrees:.2}deg, {}, {})", color(*from), color(*to))
            } else {
                format!(
                    "linear-gradient({degrees:.2}deg, {} 0%{}, {} 100%)",
                    color(*from),
                    between(&|location| format!("{:.2}%", location * 100.0)),
                    color(*to)
                )
            }
        }
    }
}

fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}

/// The tags a served page writes as themselves — the glue builds any
/// tag it is handed, so a tag missing here would serve a `<div>` the
/// mounted page calls a `<section>`. A static table keeps the toy tree's
/// tags `&'static`; an unknown word still serves as a `<div>`.
const SERVED_TAGS: &[&str] = &[
    "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "a", "span", "button",
    "h1", "h2", "h3", "h4", "h5", "h6", "p", "code", "pre", "kbd", "samp", "b", "strong", "i",
    "em", "small", "mark", "abbr", "cite", "q", "sub", "sup", "time", "var", "u", "s", "label",
    "nav", "header", "footer", "main", "section", "article", "aside", "figure", "figcaption",
    "blockquote", "ol", "ul", "li", "dl", "dt", "dd", "address",
];

/// The tags the browser already makes a control of: their role and their
/// tab stop are the element's own, and writing either over them would
/// only say it twice.
const NATIVE_CONTROLS: &[&str] = &["button", "a", "input", "select", "textarea", "summary"];

/// What a control publishes (`ControlState::wire`), as the attributes a
/// keyboard and a screen reader read — the same words the glue writes for
/// an element it creates, so a served page and a mounted one agree. Zero
/// takes them all off: the element stopped being a control.
fn write_control(element: &mut Element, control: u8) {
    for name in ["role", "tabindex", "aria-disabled", "aria-checked", "aria-selected", "aria-haspopup", "aria-expanded"] {
        element.attrs.remove(name);
    }
    if control == 0 {
        return;
    }
    let (kind, enabled, checked) = (control & 7, control & 8 != 0, (control >> 4) & 3);
    let expanded = control >> 6;
    if expanded != 0 {
        element.attrs.insert("aria-expanded", (expanded == 2).to_string());
    }
    if !NATIVE_CONTROLS.contains(&element.tag) {
        let role = match kind {
            2 => "checkbox",
            3 => "switch",
            4 => "radio",
            5 => "tab",
            _ => "button",
        };
        element.attrs.insert("role", role.to_string());
        if enabled {
            element.attrs.insert("tabindex", "0".to_string());
        }
    }
    if kind == 6 {
        element.attrs.insert("aria-haspopup", "true".to_string());
    }
    if !enabled {
        element.attrs.insert("aria-disabled", "true".to_string());
    }
    if checked != 0 {
        let name = if kind == 5 { "aria-selected" } else { "aria-checked" };
        element.attrs.insert(name, (checked == 2).to_string());
    }
}

/// The elements whose name is the field's own (`SetField` writes it): a
/// hint never names them, and never takes their name away.
const FIELDS: &[&str] = &["input", "textarea"];

/// What an element says of itself beyond what it is (`Voice::wire`) and
/// the name it answers to, as attributes — after [`write_control`], whose
/// role wins: a live region is a `status` or an `alert` only where the
/// element is no control.
fn write_voice(element: &mut Element, voice: u8, label: Option<&str>) {
    if !FIELDS.contains(&element.tag) {
        match label {
            Some(label) => element.attrs.insert("aria-label", label.to_string()),
            None => element.attrs.remove("aria-label"),
        };
    }
    match voice & 1 != 0 {
        true => element.attrs.insert("aria-hidden", "true".to_string()),
        false => element.attrs.remove("aria-hidden"),
    };
    let (live, role) = match (voice >> 1) & 3 {
        1 => (Some("polite"), "status"),
        2 => (Some("assertive"), "alert"),
        _ => (None, ""),
    };
    match live {
        Some(live) => {
            element.attrs.insert("aria-live", live.to_string());
            if !element.attrs.contains_key("role") {
                element.attrs.insert("role", role.to_string());
            }
        }
        None => {
            element.attrs.remove("aria-live");
        }
    }
}

fn leak_tag(tag: &str) -> &'static str {
    SERVED_TAGS.iter().find(|served| **served == tag).copied().unwrap_or("div")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    /// The padding record is logical and so are the properties the page
    /// writes: `padding-block` and `padding-inline`, never a physical
    /// `padding`, so a right-to-left mount puts the leading inset on
    /// the right without a word from the engine.
    #[test]
    fn leading_padding_is_inline_start_on_the_page() {
        let layout = crate::dom::DomLayout {
            padding: Some((1.0, 2.0, 3.0, 4.0)),
            ..crate::dom::DomLayout::default()
        };
        let rule = rule_text(
            "k",
            crate::dom::CreateKind::FlexColumn,
            false,
            &crate::dom::DomLook::default(),
            &layout,
            None,
        );
        assert!(rule.contains("padding-block:1px 3px"), "{rule}");
        assert!(rule.contains("padding-inline:4px 2px"), "leading first: {rule}");
        assert!(!rule.contains("padding:"), "no physical padding: {rule}");
    }

    /// A box's line is a CSS border, painted with its background as the
    /// one box a page's own bordered box is: an outline is a second paint,
    /// and the browser blends its edges otherwise (no bleed avoidance, a
    /// rounded corner a level or more apart). The child is pulled back
    /// under the line by its width, so the line still lays nothing out.
    #[test]
    fn a_served_box_draws_its_border_with_its_background() {
        #[derive(Clone)]
        struct Card;
        impl Component for Card {
            fn body(self) -> impl View {
                vstack!(text("card"))
                    .padding_length(8.0)
                    .background_color(Color::rgb(4, 5, 6))
                    .corner_radius(12.0)
                    .border(Color::rgb(1, 2, 3), 1.5)
                    .border_style(BorderStyle::Dashed)
            }
        }

        let page = render(&Card, Size { width: 240.0, height: 120.0 });
        let css = &page.css;
        let rule = css
            .split('}')
            .find(|rule| rule.contains("border:1.5px dashed rgba(1, 2, 3, 1)"))
            .unwrap_or_else(|| panic!("the box wears a border: {css}"));
        assert!(rule.contains("background-color:rgba(4, 5, 6, 1)"), "with its background: {rule}");
        assert!(rule.contains("border-radius:12px"), "and its corners: {rule}");
        assert!(!rule.contains("outline"), "no outline beside it: {rule}");
        let class = rule.split('.').nth(1).expect("a class selector");
        assert!(
            css.contains(&format!(":where(.{class}.{class}>*){{margin:-1.5px}}")),
            "the child is pulled back under the line, at no specificity: {css}"
        );
        assert!(!css.contains("outline:1.5px"), "{css}");
    }

    /// A shadow's own layers are served as CSS writes them: offset, blur,
    /// spread, ink, inset, the first one written on top, and over the
    /// house halo when the box wears one too.
    #[test]
    fn a_served_box_shadow_is_the_css_a_design_writes() {
        #[derive(Clone)]
        struct Window;
        impl Component for Window {
            fn body(self) -> impl View {
                vstack!(
                    text("window")
                        .padding_length(8.0)
                        .background_color(Color::rgb(30, 30, 32))
                        .corner_radius(18.0)
                        .box_shadow(BoxShadow::new(Color::rgba(0, 0, 0, 179)).offset(0.0, 50.0).blur(120.0).spread(-20.0))
                        .box_shadow(BoxShadow::new(Color::rgba(0, 0, 0, 102)).spread(1.0)),
                    text("pressed").box_shadow(BoxShadow::new(Color::rgba(0, 0, 0, 51)).offset(0.0, 1.5).blur(2.0).inset()).shadow_color(10.0, Color::rgba(0, 0, 0, 128)),
                )
            }
        }

        let page = render(&Window, Size { width: 240.0, height: 120.0 });
        let css = &page.css;
        let rule = css
            .split('}')
            .find(|rule| rule.contains("background-color:rgba(30, 30, 32, 1)"))
            .unwrap_or_else(|| panic!("the window has a rule: {css}"));
        assert!(
            rule.contains("box-shadow:0px 50px 120px -20px rgba(0, 0, 0, 0.7019607843137254),0px 0px 0px 1px rgba(0, 0, 0, 0.4)"),
            "both layers, the first on top: {rule}"
        );
        assert!(
            css.contains("box-shadow:inset 0px 1.5px 2px 0px rgba(0, 0, 0, 0.2),0px 0px 5.124px 3.262px"),
            "an inset layer, over the halo: {css}"
        );
    }

    /// A line along one edge is served as its own longhand, as a page
    /// writes `border-bottom`, the ring's line on the other sides, and
    /// the child pulled back under it on that side alone. A text keeps no
    /// border: its edge is an inset shadow.
    #[test]
    fn a_served_edge_is_its_own_border_longhand() {
        #[derive(Clone)]
        struct Ruled;
        impl Component for Ruled {
            fn body(self) -> impl View {
                vstack!(
                    vstack!(text("bar")).padding_edge(Edge::Bottom, 1.0).border_edge(Edge::Bottom, Color::rgb(1, 2, 3), 1.0),
                    vstack!(text("card"))
                        .background_color(Color::rgb(9, 9, 9))
                        .border(Color::rgb(4, 5, 6), 1.0)
                        .border_edge(Edge::Leading, Color::rgb(7, 8, 9), 3.0),
                )
            }
        }

        let page = render(&Ruled, Size { width: 240.0, height: 120.0 });
        let css = &page.css;
        let bar = css
            .split('}')
            .find(|rule| rule.contains("border-bottom:1px solid rgba(1, 2, 3, 1)"))
            .unwrap_or_else(|| panic!("the bar wears its rule: {css}"));
        assert!(!bar.contains("border-top") && !bar.contains("border:"), "and no other side: {bar}");
        let class = bar.split('.').nth(1).expect("a class selector").split(['{', '>']).next().expect("a class");
        assert!(css.contains(&format!(":where(.{class}.{class}>*){{margin-bottom:-1px}}")), "pulled back at the bottom alone: {css}");
        let card = css
            .split('}')
            .find(|rule| rule.contains("background-color:rgba(9, 9, 9, 1)"))
            .unwrap_or_else(|| panic!("the card has a rule: {css}"));
        for side in ["border-top:1px solid rgba(4, 5, 6, 1)", "border-inline-end:1px", "border-bottom:1px", "border-inline-start:3px solid rgba(7, 8, 9, 1)"] {
            assert!(card.contains(side), "{side}: {card}");
        }
        assert!(css.contains("{margin-top:-1px;margin-inline-end:-1px;margin-bottom:-1px;margin-inline-start:-3px}"), "{css}");

        let ruled = crate::dom::DomLook {
            border_edges: crate::shapes::BorderEdges::one(Edge::Bottom, Color::rgb(1, 2, 3), 2.0),
            ..crate::dom::DomLook::default()
        };
        let field = rule_text("k", crate::dom::CreateKind::Field, false, &ruled, &crate::dom::DomLayout::default(), None);
        assert!(field.contains("box-shadow:inset 0 -2px 0 0 rgba(1, 2, 3, 1)"), "{field}");
        assert!(!field.contains("border-bottom") && !field.contains("margin-bottom"), "{field}");
    }

    /// The glue writes both as the page does: the layers into the shadow
    /// list, the edges as longhands from the same table of sides.
    #[test]
    fn the_glue_writes_shadow_layers_and_edges_as_the_page_does() {
        let glue = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        assert!(glue.contains("const last = u8();"), "the third mask");
        assert!(glue.contains("${u8() ? \"inset \" : \"\"}${x}px ${y}px ${blur}px ${spread}px ${ink}"), "a layer as CSS writes it");
        assert!(glue.contains("style.shadows.unshift(...layers);"), "over the halo");
        assert!(glue.contains("decl[`border-${side}`] = `${width}px ${style.lineStyle} ${ink}`;"), "the longhand");
        assert!(glue.contains("`margin-${side}:${-width}px`"), "the pull-back");
        for (border, margin, inward) in EDGES {
            let side = border.trim_start_matches("border-");
            assert_eq!(margin, format!("margin-{side}"));
            let spelled = inward(1.0).replace("1px", "${w}px").replace("-${w}px", "${-w}px");
            assert!(glue.contains(&format!("[\"{side}\", (w) => `{spelled}`]")), "the glue's {side}: {spelled}");
        }
    }

    /// The glue lowers a box's line the way the page does: a border on the
    /// kinds whose children fill the box, the outline on the rest.
    #[test]
    fn the_glue_draws_a_box_line_as_the_page_does() {
        let glue = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        let kinds = [CreateKind::Group, CreateKind::Box, CreateKind::Layers, CreateKind::Rectangle, CreateKind::Ellipse];
        let lined: Vec<u8> = kinds
            .into_iter()
            .map(crate::dom::kind_code)
            .collect();
        let set = lined.iter().map(u8::to_string).collect::<Vec<_>>().join(", ");
        assert!(glue.contains(&format!("const LINED = new Set([{set}]);")), "the glue's kinds: {set}");
        assert!(glue.contains("decl.border = style.border;"), "a border on them");
        let pull = "rules.push(`:where(${selector}>*){margin:${-style.borderWidth}px}`);";
        assert!(glue.contains(pull), "the child pulled back");
        assert!(glue.contains("\"border-color\", \"outline-color\""), "the spring moves the border's colour");
    }

    /// A border is a line drawn inward over the box and its children,
    /// laying nothing out. A box's is a CSS border with its child pulled
    /// back under it; a field's, which holds no child, an outline pulled
    /// in by its own width. A broken line keeps its dashes, a field's
    /// focus colors its line, and the keyboard's ring, an outline too,
    /// moves a field's line beneath it.
    #[test]
    fn a_served_border_is_drawn_inward_and_lays_nothing_out() {
        #[derive(Clone)]
        struct Bordered(State<String>);
        impl Component for Bordered {
            fn body(self) -> impl View {
                vstack!(
                    text("solid").border(Color::rgb(1, 2, 3), 2.0),
                    text("dashed").border(Color::rgb(1, 2, 3), 1.5).border_style(BorderStyle::Dashed),
                    text_field("name", self.0.binding()),
                )
            }
        }

        let page = render(&Bordered(State::new(String::new())), Size { width: 240.0, height: 120.0 });
        let css = &page.css;
        assert!(css.contains("border:2px solid rgba(1, 2, 3, 1)"), "{css}");
        assert!(css.contains(">*){margin:-2px}"), "{css}");
        assert!(css.contains("border:1.5px dashed rgba(1, 2, 3, 1)"), "{css}");
        assert!(css.contains(">*){margin:-1.5px}"), "{css}");
        assert!(!css.contains("outline:2px solid rgba") && !css.contains("outline:1.5px"), "{css}");
        // the field keeps the outline, its focus colors it, and the ring takes it
        assert!(css.contains("outline:1px solid rgba(150, 155, 165, 1);outline-offset:-1px"), "{css}");
        assert!(css.contains(":focus{outline-color:"), "{css}");
        assert!(
            css.contains(
                "[role][tabindex]:focus-visible{outline:2px solid Highlight;outline-offset:2px;\
                 box-shadow:inset 0 0 0 1px rgba(150, 155, 165, 1)}"
            ),
            "{css}"
        );
        assert!(!css.contains("box-shadow:inset 0 0 0 2px"), "a box's line needs no ring of its own: {css}");
    }

    /// A field's chrome is the theme's or none: the browser's own white
    /// box and sunken edge never show, so a bare field shows what is
    /// behind it, and a themed one wears the look's ground over the reset.
    #[test]
    fn a_served_field_never_wears_the_browsers_box() {
        #[derive(Clone)]
        struct Fields(State<String>);
        impl Component for Fields {
            fn body(self) -> impl View {
                vstack!(text_field("bare", self.0.binding()).bare(), text_field("themed", self.0.binding()))
            }
        }

        let page = render(&Fields(State::new(String::new())), Size { width: 240.0, height: 120.0 });
        let css = &page.css;
        let fields: Vec<&str> = css.lines().filter(|rule| rule.contains("padding:5px 8px")).collect();
        assert_eq!(fields.len(), 2, "{css}");
        for rule in &fields {
            assert!(rule.contains("background:transparent;") && rule.contains("border:0;"), "{rule}");
        }
        assert!(fields.iter().any(|rule| rule.contains("background-color:")), "the theme's ground: {css}");
    }

    /// The house halo falls off as (1 − d/r)² and stops at its reach;
    /// one CSS blur is a Gaussian, lighter close in and longer far out.
    /// The page stacks two blurred layers fitted to the curve, each a
    /// spread and a blur in reaches and the ink's opacity raised to its
    /// own power — the two the glue spells.
    #[test]
    fn a_served_shadow_follows_the_house_halo() {
        #[derive(Clone, Copy)]
        struct Lifted;
        impl Component for Lifted {
            fn body(self) -> impl View {
                text("lifted").shadow_color(10.0, Color::rgba(0, 0, 0, 128))
            }
        }

        let page = render(&Lifted, Size { width: 240.0, height: 120.0 });
        assert!(
            page.css.contains("box-shadow:0px 0px 5.124px 3.262px rgba(0, 0, 0, 0.4466),0px 0px 1.164px 0.738px rgba(0, 0, 0, 0.2134)"),
            "{}",
            page.css
        );
    }

    /// A frame pins what it names: a child that fills the width stops at
    /// the frame, and the box around the frame — its background — hugs
    /// the frame instead of stretching to whatever holds it.
    #[test]
    fn a_box_around_an_exact_frame_hugs_the_frame() {
        #[derive(Clone, Copy)]
        struct Column;
        impl Component for Column {
            fn body(self) -> impl View {
                vstack!(text("wide").frame_max(f64::INFINITY, f64::MAX, Alignment::Leading))
                    .frame_width_aligned(300.0, Alignment::Leading)
                    .background_color(Color::rgb(1, 2, 3))
            }
        }

        let page = render(&Column, Size { width: 400.0, height: 120.0 });
        let painted = page.css.lines().find(|rule| rule.contains("background-color:rgba(1, 2, 3, 1)")).expect("the background's rule");
        assert!(!painted.contains("align-self:stretch"), "{}", page.css);
    }

    /// A layer meets its box at its point: the cell places it by the
    /// point — the box is the cell's own size and stays put — and a
    /// layer that takes the width stretches across the cell.
    #[test]
    fn an_overlay_is_placed_by_its_point() {
        #[derive(Clone, Copy)]
        struct Badged;
        impl Component for Badged {
            fn body(self) -> impl View {
                vstack!(
                    rectangle().frame(120.0, 60.0).overlay(UnitPoint::TOP_TRAILING, rectangle().frame(16.0, 16.0)),
                    rectangle()
                        .frame(120.0, 40.0)
                        .overlay(UnitPoint::BOTTOM, spacer().frame_height(3.0).background_color(Color::rgb(4, 5, 6))),
                )
            }
        }

        let page = render(&Badged, Size { width: 400.0, height: 200.0 });
        let cells: Vec<&str> = page.css.lines().filter(|rule| rule.contains("display:grid")).collect();
        assert!(cells.iter().any(|cell| cell.contains("align-items:flex-start") && cell.contains("justify-items:flex-end")), "{}", page.css);
        assert!(cells.iter().any(|cell| cell.contains("align-items:flex-end") && cell.contains("justify-items:center")), "{}", page.css);
        let band = page.css.lines().find(|rule| rule.contains("background-color:rgba(4, 5, 6, 1)")).expect("the band's rule");
        assert!(band.contains("justify-self:stretch"), "{}", page.css);
    }

    /// A served field says what the mounted one says — the glue's op 7,
    /// mirrored: a secret is a password box, what it holds is the
    /// autofill's token, its name is `aria-label`, and a field that said
    /// neither carries neither attribute.
    #[test]
    fn a_served_field_says_what_it_is() {
        use crate::text_input::Autocomplete;

        #[derive(Clone)]
        struct SignIn {
            email: State<String>,
            password: State<String>,
        }
        impl Component for SignIn {
            fn body(self) -> impl View {
                vstack!(
                    text_field("you@example.com", self.email.binding()),
                    text_field("", self.password.binding())
                        .secret(true)
                        .autocomplete(Autocomplete::CurrentPassword)
                        .accessibility_label("Password"),
                )
            }
        }
        let view = SignIn { email: State::new(String::new()), password: State::new(String::new()) };
        let page = render(&view, Size { width: 240.0, height: 120.0 });
        let inputs: Vec<&str> = page.html.split("<input").skip(1).collect();
        assert_eq!(inputs.len(), 2, "{}", page.html);
        let (plain, password) = (inputs[0], inputs[1]);
        assert!(plain.contains(" type=\"text\""), "{plain}");
        assert!(!plain.contains("autocomplete=") && !plain.contains("aria-label="), "{plain}");
        assert!(password.contains(" type=\"password\""), "{password}");
        assert!(password.contains(" autocomplete=\"current-password\""), "{password}");
        assert!(password.contains(" aria-label=\"Password\""), "{password}");
    }

    /// A trailing text aligns to `end`, never `right`; a leading one
    /// writes nothing and takes the browser's own `start`.
    #[test]
    fn a_trailing_text_aligns_to_the_end_not_the_right() {
        let face = |align| crate::dom::DomText {
            content: std::sync::Arc::from("words"),
            color: Color::BLACK,
            inherits_ink: false,
            font: crate::text_engine::FontSpec::DEFAULT,
            line_height: None,
            text_align: align,
            highlights: None,
            truncation: None,
            inherits_face: false,
            line_limit: 0,
        };
        let rule = |align| {
            rule_text(
                "k",
                crate::dom::CreateKind::Text,
                false,
                &crate::dom::DomLook::default(),
                &crate::dom::DomLayout::default(),
                Some(&face(align)),
            )
        };
        let trailing = rule(Some(TextAlignment::Trailing));
        assert!(trailing.contains("text-align:end"), "{trailing}");
        assert!(!trailing.contains("right"), "{trailing}");
        assert!(!rule(None).contains("text-align"), "leading is the browser's own start");
        assert!(rule(Some(TextAlignment::Center)).contains("text-align:center"));
    }

    /// A rich paragraph is served as styled spans: a run says what it
    /// changes, and the rule a text wears is the element's own.
    #[test]
    fn a_rich_paragraph_is_served_as_styled_spans() {
        let clay = Color::hex(0xcaa396);
        let paragraph = rich_text([
            run("payable within "),
            run("thirty").strikethrough(Some(clay)),
            run(" "),
            run("forty-five").weight(crate::text_engine::Weight::Light).monospaced(),
        ]);
        let page = render(&paragraph, Size { width: 400.0, height: 100.0 });
        assert!(page.html.contains("payable within <span"), "{}", page.html);
        assert!(
            page.html.contains(&format!("text-decoration-line:line-through;text-decoration-color:{}", color(clay))),
            "{}",
            page.html
        );
        assert!(page.html.contains("font-weight:300;font-family:ui-monospace"), "{}", page.html);
        let underlined = render(&text("link").underline(None), Size { width: 100.0, height: 40.0 });
        assert!(underlined.html.contains("text-decoration-line:underline"), "{}", underlined.html);
    }

    /// A served page's mount wears its language and the way it reads:
    /// English and left to right by default, Arabic and right to left
    /// when the runtime says so.
    #[test]
    fn the_mount_wears_the_language_and_the_direction() {
        let size = Size { width: 300.0, height: 200.0 };
        let page = render(&Page { on: State::new(false) }, size);
        assert!(page.html.contains("lang=\"en\""), "{}", page.html);
        assert!(page.html.contains("dir=\"ltr\""), "{}", page.html);
    }

    /// An island that reads the other way carries its own `direction`
    /// in its rule, isolated.
    #[test]
    fn an_rtl_island_sets_its_own_dir() {
        let layout = crate::dom::DomLayout {
            direction: Some(LayoutDirection::RightToLeft),
            ..crate::dom::DomLayout::default()
        };
        let rule = rule_text(
            "k",
            crate::dom::CreateKind::Box,
            false,
            &crate::dom::DomLook::default(),
            &layout,
            None,
        );
        assert!(rule.contains("direction:rtl"), "{rule}");
        assert!(rule.contains("unicode-bidi:isolate"), "{rule}");
        let plain = rule_text(
            "k",
            crate::dom::CreateKind::Box,
            false,
            &crate::dom::DomLook::default(),
            &crate::dom::DomLayout::default(),
            None,
        );
        assert!(
            !plain.contains("direction:ltr") && !plain.contains("unicode-bidi"),
            "a box that inherits says nothing: {plain}"
        );
    }

    /// The page and the glue write one CSS: the logical names the page
    /// uses are the ones the glue spells, and neither says a side.
    #[test]
    fn the_page_and_the_glue_write_the_same_logical_properties() {
        let glue = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        assert!(glue.contains("if (mask & 16777216) style.hidden = true;"), "the glue reads the hidden bit");
        assert!(glue.contains("if (style.hidden) decl.visibility = \"hidden\";"), "and keeps the box's place with visibility");
        for name in ["padding-block", "padding-inline"] {
            assert!(glue.contains(&format!("decl[\"{name}\"]")), "the glue writes {name}");
        }
        assert!(glue.contains("decl[\"text-align\"] = \"end\""), "the glue aligns to end");
        assert!(!glue.contains("decl[\"text-align\"] = \"right\""), "and never to right");
        assert!(!glue.contains("decl.padding ="), "and writes no physical padding");
        assert!(glue.contains("decl.direction ="), "the glue writes an island's direction");
        assert!(glue.contains("decl[\"unicode-bidi\"] = \"isolate\""), "isolated");
    }

    #[derive(Clone)]
    struct Page {
        on: State<bool>,
    }

    impl Component for Page {
        fn body(self) -> impl View {
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
    #[derive(Clone)]
    struct Unseen;

    impl Component for Unseen {
        fn body(self) -> impl View {
            crate::vstack!(text("seen"), text("unseen").hidden(), text("after"))
        }
    }

    /// `.hidden()` on the page: the box is in the markup, keeps its
    /// place, and is unseen by visibility — never left out, or the text
    /// after it would move up.
    #[test]
    fn a_hidden_box_keeps_its_place_unseen() {
        let page = render(&Unseen, Size { width: 300.0, height: 200.0 });
        assert!(page.css.contains("visibility:hidden"), "{}", page.css);
        assert!(page.html.contains("unseen"), "the box is in the markup: {}", page.html);
        assert!(!page.css.contains("display:none"), "{}", page.css);
    }

    #[test]
    fn a_page_renders_to_stable_html() {
        let size = Size { width: 300.0, height: 200.0 };
        let first = render(&Page { on: State::new(false) }, size);
        let second = render(&Page { on: State::new(false) }, size);
        assert_eq!(first.html, second.html, "deterministic bytes");
        assert!(first.html.contains("data-hydrate=\"1\""));
        assert!(first.html.contains("data-width=\"300\""), "the served box");
        assert!(first.html.contains("data-height=\"200\""), "the served box");
        assert!(first.html.contains("hello, prerender"));
        assert!(first.html.contains("display:flex"));
        assert!(!first.html.contains("position:absolute"), "a flow page ships in the flow");
    }

    /// A page rendered in a locale is laid out and worded in it, and says
    /// so on its mount and its document root — Arabic reads right to left.
    #[test]
    fn a_served_page_in_arabic_is_already_rtl() {
        let size = Size { width: 300.0, height: 200.0 };
        let page = render_in(&Page { on: State::new(false) }, size, &Locale::new("ar"));
        assert_eq!((page.lang.as_str(), page.dir), ("ar", "rtl"));
        assert!(page.html.contains("lang=\"ar\""), "{}", page.html);
        assert!(page.html.contains("dir=\"rtl\""), "{}", page.html);
        let document = render_document_in(
            &Page { on: State::new(false) },
            size,
            "page.wasm",
            "glue_dom.js",
            &Locale::parse("ar,en"),
        );
        assert!(document.contains("<html lang=\"ar\" dir=\"rtl\">"), "{document}");
        let english = render(&Page { on: State::new(false) }, size);
        assert_eq!((english.lang.as_str(), english.dir), ("en", "ltr"));
        assert!(render_document(&Page { on: State::new(false) }, size, "w", "g").contains("<html lang=\"en\" dir=\"ltr\">"));
    }

    /// The locale a page is sealed in is the whole list's first tag.
    #[test]
    fn a_page_is_sealed_in_its_language() {
        let size = Size { width: 300.0, height: 200.0 };
        let page = render_in(&Page { on: State::new(false) }, size, &Locale::parse("pt-BR,en"));
        assert_eq!((page.lang.as_str(), page.dir), ("pt-BR", "ltr"));
        assert!(page.html.contains("lang=\"pt-BR\""));
    }

    /// A page served in Arabic is adopted in silence by a runtime that
    /// speaks Arabic: the mount's language and direction are noted, not
    /// sent again.
    #[test]
    fn an_adopted_page_keeps_its_language_in_silence() {
        let size = Size { width: 300.0, height: 200.0 };
        let built = Page { on: State::new(false) };
        let runtime = Runtime::new();
        runtime.set_locale(Some(Locale::new("ar")));
        let mount = runtime.dom_frame(&built, size);
        assert!(mount.iter().any(|patch| matches!(patch, DomPatch::SetLanguage { .. })));

        let served = Page { on: State::new(false) };
        let fresh = Runtime::new();
        fresh.set_locale(Some(Locale::new("ar")));
        fresh.dom_adopt(&served, size);
        let first = fresh.dom_frame(&served, size);
        assert!(first.is_empty(), "the adopted page is already true, language included: {first:?}");
        // the reader's own report moves it: one patch on the mount
        fresh.set_locale(None);
        assert!(fresh.set_system_locale(Locale::new("en")));
        let moved = fresh.dom_frame(&served, size);
        assert!(
            moved.iter().any(|patch| matches!(patch, DomPatch::SetLanguage { dir, .. } if !dir.is_rtl())),
            "{moved:?}"
        );
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

    /// A `.layout(Exact)` interior is served at the engine's own
    /// numbers: a box centred in a fractional frame, and one placed
    /// under a padding no `f32` holds, print the `f64`s the engine
    /// placed them at — the text a page has always served for them.
    #[cfg(feature = "canvas")]
    #[test]
    fn an_exact_interior_serves_the_engines_own_numbers() {
        #[derive(Clone, Copy)]
        struct Placed;

        impl Component for Placed {
            fn body(self) -> impl View {
                crate::vstack!(
                    text("flow above"),
                    crate::vstack!(
                        text("pinned"),
                        text("x").padding_length(7.8).background_color(Color::hex(0x3B82F6)),
                    )
                        .frame(120.3, 60.7)
                        .layout(crate::layout::LayoutMode::Exact),
                    text("flow below"),
                )
            }
        }

        let page = render(&Placed, Size { width: 200.0, height: 200.0 });
        for served in [
            // the flow's wrapper: a pinned box, at the wire's precision,
            // and a floor of the same size the flex line cannot shrink
            "style=\"height:60.70000076293945px;min-height:60.70000076293945px;\
             min-width:120.30000305175781px;width:120.30000305175781px\"",
            // the interior: the engine's own numbers
            "style=\"height:16px;left:0;position:absolute;top:0;\
             transform:translate(36.15px, 6.550000000000001px);width:48px\"",
            "style=\"height:31.6px;left:0;position:absolute;top:0;\
             transform:translate(48.349999999999994px, 22.55px);width:23.6px\"",
            "style=\"height:16px;left:0;position:absolute;top:0;\
             transform:translate(7.799999999999997px, 7.800000000000001px);width:8px\"",
        ] {
            assert!(page.html.contains(served), "{served} in {}", page.html);
        }
    }

    /// A stack of layers is ONE cell with every layer in it — grid
    /// auto-placement alone would give each layer a row of its own.
    #[test]
    fn layers_share_one_cell() {
        #[derive(Clone, Copy)]
        struct Badge;

        impl Component for Badge {
            fn body(self) -> impl View {
                crate::zstack!(rectangle().frame(80.0, 20.0), text("on top"))
            }
        }

        let page = render(&Badge, Size { width: 200.0, height: 200.0 });
        assert!(page.css.contains(">*{grid-area:1/1;z-index:0}"), "{}", page.css);
        assert!(page.css.contains("justify-items:center"), "{}", page.css);
    }

    /// An overlay's cell is its base's size, as `SwiftUI`'s is: a layer
    /// larger than the box — a glow behind a button, a ribbon past a
    /// card's edge — overflows it and never grows it. The base takes
    /// the first track and every layer spans an empty second one,
    /// which takes no room.
    #[test]
    fn an_overlay_never_grows_its_base() {
        #[derive(Clone, Copy)]
        struct Card;

        impl Component for Card {
            fn body(self) -> impl View {
                crate::vstack!(
                    rectangle().frame(100.0, 40.0).overlay(UnitPoint::TOP_LEADING, rectangle().frame(140.0, 10.0)),
                    text("glow").background(UnitPoint::CENTER, rectangle().frame(200.0, 200.0)),
                )
            }
        }

        let page = render(&Card, Size { width: 300.0, height: 300.0 });
        assert_eq!(page.css.matches("grid-template-columns:auto 0fr").count(), 2, "{}", page.css);
        assert!(page.css.contains("grid-template-rows:auto 0fr"), "{}", page.css);
        assert!(page.css.contains(">*{grid-area:1/1/3/3;z-index:0}"), "{}", page.css);
        assert!(page.css.contains(">:first-child{grid-area:1/1}"), "the overlay's base comes first: {}", page.css);
        assert!(page.css.contains(">:last-child{grid-area:1/1}"), "the background's, last: {}", page.css);
    }

    /// A background lies under its base however it is moved, as
    /// `SwiftUI`'s does: the page moves it with `translate`, which alone
    /// would paint it over a base nothing moved, so each layer of the cell
    /// paints whole and in its turn.
    #[test]
    fn a_moved_background_stays_under_its_base() {
        let page = render(
            &text("over the hills").background(UnitPoint::TOP_LEADING, rectangle().frame(200.0, 40.0).offset(0.0, 30.0)),
            Size { width: 300.0, height: 300.0 },
        );
        assert!(page.css.contains("translate:0px 30px"), "{}", page.css);
        assert!(page.css.contains(">*{grid-area:1/1/3/3;z-index:0}"), "{}", page.css);
    }

    /// A served picture has no bytes yet: it wears the identity the
    /// glue fills the source by, once the wasm hands the bytes over.
    #[test]
    fn a_served_image_waits_by_its_identity() {
        #[derive(Clone)]
        struct Picture(crate::image_engine::ImageSource);

        impl Component for Picture {
            fn body(self) -> impl View {
                image(self.0.clone()).resizable().frame(20.0, 20.0)
            }
        }

        let source = crate::image_engine::ImageSource::bytes_keyed(
            (7u64 << 32) | 9,
            vec![0u8; 4],
        );
        let page = render(&Picture(source), Size { width: 200.0, height: 200.0 });
        assert!(page.html.contains("data-img=\"7:9\""), "{}", page.html);
        // and it takes its frame on both axes, as the layout sizes it: a
        // width left to the ratio would spill a contained picture
        assert!(page.css.contains("flex:1 1 auto") && page.css.contains("align-self:stretch"), "{}", page.css);
    }

    /// A served drawing is the tree the glue builds: the allowed shapes
    /// with every value escaped, and nothing that runs — no script, no
    /// foreign object, no style sheet, no handler, no reference that
    /// leaves the drawing, and no value that breaks out of its quotes.
    #[test]
    fn a_served_drawing_keeps_only_what_draws() {
        #[derive(Clone)]
        struct Drawn(crate::image_engine::ImageSource);

        impl Component for Drawn {
            fn body(self) -> impl View {
                image(self.0.clone()).inline_svg().resizable().frame(20.0, 20.0)
            }
        }

        let svg = r##"<svg viewBox="0 0 2 2" preserveAspectRatio="none" onload="alert(1)">
            <script>alert(2)</script>
            <foreignObject><p onclick="alert(3)">x</p></foreignObject>
            <style>*{fill:url(https://evil.test/x)}</style>
            <clipPath id="c"><rect width="2" height="1"/></clipPath>
            <path d="M0 0" clip-path="url(#c)" fill="url(https://evil.test/x)" onclick="alert(4)"/>
            <use href="javascript:alert(5)"/>
            <path d="M0 0&quot; onmouseover=&quot;go&lt;/svg&gt;&lt;script&gt;"/>
        </svg>"##;
        let source = crate::image_engine::ImageSource::from_bytes(svg.as_bytes().to_vec());
        let page = render(&Drawn(source), Size { width: 200.0, height: 200.0 });
        let start = page.html.find("<svg").expect("the drawing is an svg");
        let drawing = &page.html[start..start + page.html[start..].find("</svg>").expect("closed") + 6];
        let n: u32 = drawing["<svg data-n=\"".len()..].split('"').next().and_then(|n| n.parse().ok()).expect("its number");
        assert!(drawing.contains("viewBox=\"0 0 2 2\"") && drawing.contains("preserveAspectRatio=\"none\""), "{drawing}");
        assert!(drawing.contains(&format!("<clipPath id=\"bunny-{n}-0\"><rect width=\"2\" height=\"1\"></rect></clipPath>")), "{drawing}");
        assert!(drawing.contains(&format!("<path d=\"M0 0\" clip-path=\"url(#bunny-{n}-0)\"></path>")), "{drawing}");
        assert!(drawing.contains("<use></use>"), "{drawing}");
        assert!(drawing.contains("d=\"M0 0&quot; onmouseover=&quot;go&lt;/svg&gt;&lt;script&gt;\""), "{drawing}");
        for gone in ["<script", "<foreignObject", "<style", "onload", "onclick", "onmouseover=\"", "javascript", "evil.test", "<p>", "<p "] {
            assert!(!page.html.contains(gone), "{gone}: {drawing}");
        }
        assert!(page.css.contains("pointer-events:none"), "{}", page.css);
    }

    /// A served control says what it is before any script runs: the
    /// role, the tab stop and the state a keyboard and a screen reader
    /// read are attributes of the page itself — the same words the glue
    /// writes on an element it mounts. A tag that is a control already
    /// keeps the browser's own role and tab stop.
    #[test]
    fn a_control_serves_its_role_its_state_and_its_tab_stop() {
        #[derive(Clone, Copy)]
        struct Controls;

        impl Component for Controls {
            fn body(self) -> impl View {
                crate::vstack!(
                    button(text("save"), || {}),
                    button(text("gone"), || {}).disabled(true),
                    button(text("pick"), || {}).plain().checked(true),
                    button(text("native"), || {}).element("button"),
                    text("bare").on_click(|| {}),
                )
            }
        }

        let page = render(&Controls, Size { width: 300.0, height: 300.0 });
        assert_eq!(page.html.matches("role=\"button\"").count(), 2, "{}", page.html);
        assert_eq!(page.html.matches("tabindex=\"0\"").count(), 2, "the enabled ones stop a Tab: {}", page.html);
        assert_eq!(page.html.matches("aria-disabled=\"true\"").count(), 1, "{}", page.html);
        assert!(page.html.contains("role=\"checkbox\""), "{}", page.html);
        assert!(page.html.contains("aria-checked=\"true\""), "{}", page.html);
        assert_eq!(page.html.matches("<button ").count(), 1, "{}", page.html);
    }

    /// The served page says what the mounted one says: a name on the
    /// control and on the landmark, decoration hidden, a status live, an
    /// opener collapsed, a page-drawn choice a radio.
    #[test]
    fn a_served_element_says_its_name_its_voice_and_whether_it_is_open() {
        #[derive(Clone, Copy)]
        struct Page;

        impl Component for Page {
            fn body(self) -> impl View {
                crate::vstack!(
                    button(text("×"), || {}).plain().accessibility_label("Close"),
                    button(text("English"), || {}).plain().expanded(false),
                    button(text("日本語"), || {}).plain().selected(true),
                    text("✦").accessibility_hidden(),
                    text("saved").accessibility_live(crate::accessibility::Live::Polite),
                    text("gone wrong").accessibility_live(crate::accessibility::Live::Assertive),
                    crate::vstack!(text("access")).element("nav").accessibility_label("Pages"),
                )
            }
        }

        let page = render(&Page, Size { width: 300.0, height: 300.0 });
        for said in [
            "aria-label=\"Close\"",
            "aria-expanded=\"false\"",
            "role=\"radio\"",
            "aria-checked=\"true\"",
            "aria-hidden=\"true\"",
            "role=\"status\"",
            "aria-live=\"polite\"",
            "role=\"alert\"",
            "aria-live=\"assertive\"",
            "aria-label=\"Pages\"",
        ] {
            assert_eq!(page.html.matches(said).count(), 1, "{said}: {}", page.html);
        }
        assert!(page.html.contains("<nav "), "{}", page.html);
    }

    /// A link is an `<a href>`: the browser owns the navigation, and
    /// an id beside it rides the same record.
    #[test]
    fn a_link_serves_its_href() {
        #[derive(Clone, Copy)]
        struct Links;

        impl Component for Links {
            fn body(self) -> impl View {
                crate::vstack!(
                    text("source").link("https://example.com/a?b=1&c=2"),
                    text("top").link("#top").element_id("back"),
                )
            }
        }

        let page = render(&Links, Size { width: 200.0, height: 200.0 });
        assert!(
            page.html.contains("href=\"https://example.com/a?b=1&amp;c=2\""),
            "{}",
            page.html
        );
        assert!(page.html.contains("href=\"#top\""), "{}", page.html);
        assert!(page.html.contains("id=\"back\""), "{}", page.html);
        assert_eq!(page.html.matches("<a ").count(), 2, "{}", page.html);
    }

    /// The harness's name is served as the glue writes it.
    #[test]
    fn a_test_id_is_served_as_data_testid() {
        let page = render(
            &crate::vstack!(text("a").test_id("word")).test_id("column"),
            Size { width: 200.0, height: 200.0 },
        );
        assert!(page.html.contains("data-testid=\"word\""), "{}", page.html);
        assert!(page.html.contains("data-testid=\"column\""), "{}", page.html);
    }

    /// Tracking is the face's own advance: an eyebrow's wide spacing
    /// reaches the served page as `letter-spacing`, in points.
    #[test]
    fn tracking_serves_as_letter_spacing() {
        #[derive(Clone, Copy)]
        struct Eyebrow;

        impl Component for Eyebrow {
            fn body(self) -> impl View {
                text("QUICK LOOK").font_size(12.0).tracking(2.0)
            }
        }

        let page = render(&Eyebrow, Size { width: 200.0, height: 200.0 });
        assert!(page.css.contains("letter-spacing:2px"), "{}", page.css);
    }

    /// A box that answers the pointer with its ink and declares a face
    /// for its subtree keeps its own ink: the face record carries the
    /// face alone, and the text under it inherits the box's colour.
    #[test]
    fn a_box_that_declares_a_face_keeps_its_ink() {
        #[derive(Clone, Copy)]
        struct Link;

        impl Component for Link {
            fn body(self) -> impl View {
                text("docs")
                    .font_size(12.0)
                    .foreground_color(Color::hex(0x8F86A8))
                    .foreground_hovered(Color::hex(0xF2EEFB))
                    .link("#docs")
            }
        }

        let page = render(&Link, Size { width: 200.0, height: 200.0 });
        let rule = page
            .css
            .lines()
            .find(|rule| rule.contains("font:400 12px") && !rule.contains(":hover"))
            .expect("the box that declares the face");
        assert!(rule.contains("color:rgba(143, 134, 168"), "{}", page.css);
    }

    /// A body that bends with the width — two columns or one — adopts
    /// the scene the build served: the adoption reads the window the
    /// build read, and the first frame after it says nothing.
    #[test]
    fn a_page_that_reads_the_window_adopts_in_silence() {
        #[derive(Clone, Copy)]
        struct Shaped;

        impl Component for Shaped {
            fn body(self) -> impl View {
                let wide = environment::<Viewport>().width >= 800.0;
                if wide {
                    Either::First(crate::hstack!(text("words"), text("code")))
                } else {
                    Either::Second(crate::vstack!(text("words"), text("code"), text("more")))
                }
            }
        }

        let size = Size { width: 1200.0, height: 800.0 };
        let built = Runtime::new();
        assert!(!built.dom_frame(&Shaped, size).is_empty());
        let fresh = Runtime::new();
        fresh.dom_adopt(&Shaped, size);
        assert_eq!(fresh.dom_frame(&Shaped, size), Vec::new(), "the served page is already true");
    }
}
