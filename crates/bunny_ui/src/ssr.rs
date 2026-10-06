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
    text: Vec<(String, Option<Color>)>,
    children: Vec<u32>,
    /// The look it wears — a class on the page's sheet.
    rule: Option<u64>,
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
            DomPatch::SetBox { id, width, height, max_width, max_height, slot_y } => {
                let Some(element) = self.elements.get_mut(id) else {
                    return;
                };
                for (name, value) in [
                    ("width", width),
                    ("height", height),
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
            DomPatch::SetHints { id, class, address } => {
                let address = address.as_deref();
                let dom_id = address.and_then(|address| address.dom_id.as_ref());
                let href = address.and_then(|address| address.href.as_ref());
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
                    }
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
            | DomPatch::SetAnchor { .. } => {
                // scroll offsets and icon geometry arrive
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
    }
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
    if let Some(gap) = layout.gap {
        base.insert("gap", px(gap));
    }
    if let Some(align) = layout.align {
        base.insert(
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
    // layers: the same alignment across the cell, so a centred stack
    // centres both ways
    if matches!(kind, CreateKind::Layers) {
        if let Some(align) = base.get("align-items").cloned() {
            base.insert("justify-items", align);
        }
    }
    // the record's sides are logical, and so are the properties: a
    // right-to-left mount puts the leading inset on the right by itself
    if let Some((top, trailing, bottom, leading)) = layout.padding {
        base.insert("padding-block", format!("{} {}", px(top), px(bottom)));
        base.insert("padding-inline", format!("{} {}", px(leading), px(trailing)));
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
    }
    if let Some(line_gap) = layout.wrap {
        base.insert("flex-wrap", "wrap".into());
        base.insert("row-gap", px(line_gap));
    }
    let mut states: Vec<String> = Vec::new();
    // layers: one grid cell, and every child IN it — auto-placement
    // would give each layer a row of its own
    if matches!(kind, CreateKind::Layers) {
        states.push(format!("{selector}>*{{grid-area:1/1}}"));
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
    if let Some((border, width)) = style.border {
        base.insert("border", format!("{} solid {}", px(width), color(border)));
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
    // the halo and the glass rim share one property
    let mut shadows: Vec<String> = Vec::new();
    if let Some((radius, shadow)) = style.shadow {
        shadows.push(format!("0 0 {} {}", px(radius), color(shadow)));
    }
    if let Some((response, _)) = style.transition {
        // every colour the engine's springs move — the fill, the ink,
        // the border, the halo — and the transform
        let eased: Vec<String> = ["background-color", "color", "border-color", "box-shadow", "transform"]
            .iter()
            .map(|property| format!("{property} {response}s ease-out"))
            .collect();
        base.insert("transition", eased.join(", "));
    }
    if let Some(focus) = style.focus_border {
        states.push(format!("{selector}:focus{{border-color:{c};caret-color:{c}}}", c = color(focus)));
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
    if style.clip {
        base.insert("overflow", "hidden".into());
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
    if !shadows.is_empty() {
        base.insert("box-shadow", shadows.join(","));
    }
    if let Some(text) = text {
        // a text with the face declared above it names none of its own
        if !text.inherits_face {
            base.insert("font", css_font(&text.font));
            // the shorthand leaves the spacing alone; the face's own
            // advance rides beside it
            if text.font.tracking != 0.0 {
                base.insert("letter-spacing", format!("{}px", f64::from(text.font.tracking)));
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
    }
    let declarations: Vec<String> = base.iter().map(|(name, value)| format!("{name}:{value}")).collect();
    let mut out = format!("{selector}{{{}}}", declarations.join(";"));
    for state in states {
        out.push('\n');
        out.push_str(&state);
    }
    out
}

fn css_font(font: &crate::text_engine::FontSpec) -> String {
    let weight = match font.weight {
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
            fn body(self, _ctx: &Context) -> impl View {
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
            // the flow's wrapper: a pinned box, at the wire's precision
            "style=\"height:60.70000076293945px;width:120.30000305175781px\"",
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
            fn body(self, _ctx: &Context) -> impl View {
                crate::zstack!(rectangle().frame(80.0, 20.0), text("on top"))
            }
        }

        let page = render(&Badge, Size { width: 200.0, height: 200.0 });
        assert!(page.css.contains(">*{grid-area:1/1}"), "{}", page.css);
        assert!(page.css.contains("justify-items:center"), "{}", page.css);
    }

    /// A served picture has no bytes yet: it wears the identity the
    /// glue fills the source by, once the wasm hands the bytes over.
    #[test]
    fn a_served_image_waits_by_its_identity() {
        #[derive(Clone)]
        struct Picture(crate::image_engine::ImageSource);

        impl Component for Picture {
            fn body(self, _ctx: &Context) -> impl View {
                image(self.0.clone()).resizable().frame(20.0, 20.0)
            }
        }

        let source = crate::image_engine::ImageSource::bytes_keyed(
            (7u64 << 32) | 9,
            vec![0u8; 4],
        );
        let page = render(&Picture(source), Size { width: 200.0, height: 200.0 });
        assert!(page.html.contains("data-img=\"7:9\""), "{}", page.html);
    }

    /// A link is an `<a href>`: the browser owns the navigation, and
    /// an id beside it rides the same record.
    #[test]
    fn a_link_serves_its_href() {
        #[derive(Clone, Copy)]
        struct Links;

        impl Component for Links {
            fn body(self, _ctx: &Context) -> impl View {
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

    /// Tracking is the face's own advance: an eyebrow's wide spacing
    /// reaches the served page as `letter-spacing`, in points.
    #[test]
    fn tracking_serves_as_letter_spacing() {
        #[derive(Clone, Copy)]
        struct Eyebrow;

        impl Component for Eyebrow {
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, _ctx: &Context) -> impl View {
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
            fn body(self, ctx: &Context) -> impl View {
                let wide = ctx.environment::<Viewport>().width >= 800.0;
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
