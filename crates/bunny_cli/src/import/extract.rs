//! What the browser knows about a page — every box it laid out, the
//! style it computed for it, the size it was asked for and the words
//! inside — read in one script and brought back as a tree.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::json::Value;

/// The script: walks the body, names every element with a `data-fid`,
/// and answers the tree and the page's own markup, names included.
pub const SCRIPT: &str = r"(() => {
  const PROPS = ['display', 'position', 'top', 'right', 'bottom', 'left', 'z-index', 'float', 'visibility',
    'flex-direction', 'flex-wrap', 'justify-content', 'align-items', 'align-self', 'align-content',
    'flex-grow', 'flex-shrink', 'order', 'row-gap', 'column-gap',
    'grid-template-columns', 'grid-template-rows', 'grid-column-start', 'grid-column-end', 'grid-row-start', 'grid-row-end',
    'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
    'margin-top', 'margin-right', 'margin-bottom', 'margin-left',
    'border-top-width', 'border-right-width', 'border-bottom-width', 'border-left-width',
    'border-top-style', 'border-right-style', 'border-bottom-style', 'border-left-style',
    'border-top-color', 'border-right-color', 'border-bottom-color', 'border-left-color',
    'border-top-left-radius', 'border-top-right-radius', 'border-bottom-right-radius', 'border-bottom-left-radius',
    'background-color', 'background-image', 'background-size', 'background-position', 'background-repeat', 'background-clip',
    'box-shadow', 'opacity', 'overflow-x', 'overflow-y', 'transform', 'transform-origin', 'filter', 'backdrop-filter', 'mix-blend-mode',
    'clip-path', 'mask-image', 'outline-width', 'outline-style', 'outline-color', 'outline-offset',
    'color', 'font-size', 'font-weight', 'font-style', 'font-family', 'font-variant', 'line-height', 'letter-spacing',
    'text-align', 'text-transform', 'text-decoration-line', 'text-decoration-color', 'text-decoration-style',
    'text-shadow', 'white-space', 'text-overflow', '-webkit-line-clamp', 'writing-mode', 'direction', 'vertical-align',
    'object-fit', 'object-position', 'column-count', 'appearance', 'content', 'list-style-type', 'shape-outside',
    'background-blend-mode', '-webkit-text-fill-color', 'accent-color', 'cursor'];
  const SIZES = ['width', 'height', 'min-width', 'max-width', 'min-height', 'max-height', 'flex-basis',
    'margin-top', 'margin-right', 'margin-bottom', 'margin-left'];
  const LEAVES = ['svg', 'img', 'input', 'textarea', 'select', 'video', 'canvas', 'iframe', 'progress', 'meter', 'br', 'hr'];
  let counter = 0;
  const slug = (el) => {
    const first = (el.getAttribute('class') || '').trim().split(/\s+/)[0] || '';
    const base = (el.tagName.toLowerCase() + (first ? '.' + first : '')).replace(/[^A-Za-z0-9._-]/g, '');
    return base + '-' + (counter++);
  };
  const styleOf = (cs) => {
    const out = {};
    for (const p of PROPS) { const v = cs.getPropertyValue(p); if (v !== '') out[p] = v; }
    return out;
  };
  const sizesOf = (el) => {
    const out = {};
    try { const map = el.computedStyleMap(); for (const p of SIZES) out[p] = map.get(p).toString(); } catch (e) {}
    return out;
  };
  const box = (r) => [r.x, r.y, r.width, r.height];
  const textBox = (node) => {
    const range = document.createRange();
    range.selectNodeContents(node);
    const r = range.getBoundingClientRect();
    return box(r);
  };
  const pseudo = (el, which) => {
    const cs = getComputedStyle(el, which);
    const content = cs.getPropertyValue('content');
    if (!content || content === 'none' || content === 'normal' || cs.display === 'none') return null;
    const sizes = { width: cs.width, height: cs.height };
    return { kind: which === '::before' ? 'before' : 'after', style: styleOf(cs), sizes };
  };
  // a transform turns, scales and moves the picture, never the layout:
  // every box is read with the transforms off, the boxes inside a turned
  // one included, and the view's `.rotation_effect` and `.offset` put
  // them back
  const turns = new Map();
  for (const el of [document.body, ...document.body.querySelectorAll('*')]) {
    const transform = getComputedStyle(el).transform;
    if (transform !== 'none') turns.set(el, transform);
  }
  const flat = document.createElement('style');
  flat.textContent = '*{transform:none!important}';
  document.head.appendChild(flat);
  const walk = (el) => {
    const cs = getComputedStyle(el);
    if (cs.display === 'none') return null;
    const tag = el.tagName.toLowerCase();
    const node = { kind: 'element', tag, id: slug(el), rect: box(el.getBoundingClientRect()), style: styleOf(cs),
      sizes: sizesOf(el), attrs: {}, children: [] };
    if (turns.has(el)) node.style.transform = turns.get(el);
    for (const a of ['src', 'alt', 'type', 'value', 'placeholder', 'href', 'dir', 'checked', 'disabled', 'min', 'max', 'low', 'high', 'open', 'colspan', 'rowspan'])
      if (el.hasAttribute(a)) node.attrs[a] = el.getAttribute(a) || 'true';
    // a scrolled box: where its content stands at load (a snap, a
    // remembered position)
    if (el.scrollLeft || el.scrollTop) node.attrs.scroll = el.scrollLeft + ' ' + el.scrollTop;
    if (tag === 'select') node.attrs.label = el.selectedIndex >= 0 ? el.options[el.selectedIndex].text : '';
    if (tag === 'input' || tag === 'textarea') node.attrs.current = el.value;
    if (tag === 'progress' || tag === 'meter') node.attrs.position = String(el.position !== undefined && el.position >= 0 ? el.position : (el.value - (el.min || 0)) / ((el.max || 1) - (el.min || 0)));
    el.setAttribute('data-fid', node.id);
    const before = pseudo(el, '::before');
    if (before) node.children.push(before);
    if (!LEAVES.includes(tag)) {
      for (const child of el.childNodes) {
        if (child.nodeType === 1) {
          const lowered = walk(child);
          if (lowered) node.children.push(lowered);
        } else if (child.nodeType === 3 && child.textContent.length) {
          if (child.textContent.trim() === '' && !/pre/.test(cs.whiteSpace)) {
            node.children.push({ kind: 'space' });
          } else {
            node.children.push({ kind: 'text', text: child.textContent, rect: textBox(child) });
          }
        }
      }
    }
    const after = pseudo(el, '::after');
    if (after) node.children.push(after);
    return node;
  };
  const root = walk(document.body);
  flat.remove();
  return JSON.stringify({ root, html: '<!doctype html>\n' + document.documentElement.outerHTML, width: innerWidth, height: innerHeight });
})()";

/// What a node of the tree is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Element,
    /// A run of words.
    Text,
    /// White space between two elements: a word gap inside a line.
    Space,
    /// `::before`.
    Before,
    /// `::after`.
    After,
}

/// A box the browser laid out, in CSS pixels from the page's corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    #[must_use]
    pub fn right(self) -> f64 {
        self.x + self.w
    }

    #[must_use]
    pub fn bottom(self) -> f64 {
        self.y + self.h
    }
}

/// One node of the page.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub kind: Kind,
    pub tag: String,
    /// The `data-fid` the script wrote on the element.
    pub id: String,
    pub rect: Rect,
    /// Computed values, by property name.
    pub style: BTreeMap<String, String>,
    /// Sizes as specified (`auto`, `120px`, `50%`), by property name.
    pub sizes: BTreeMap<String, String>,
    pub attrs: BTreeMap<String, String>,
    pub text: String,
    pub children: Vec<Self>,
}

impl Node {
    /// A computed value, or the empty string.
    #[must_use]
    pub fn css(&self, property: &str) -> &str {
        self.style.get(property).map_or("", String::as_str)
    }

    /// A computed length in pixels, zero when it is not one.
    #[must_use]
    pub fn px(&self, property: &str) -> f64 {
        super::css::px(self.css(property)).unwrap_or(0.0)
    }

    /// A specified size, `auto` when unknown.
    #[must_use]
    pub fn size(&self, property: &str) -> &str {
        self.sizes.get(property).map_or("auto", String::as_str)
    }

    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }
}

/// The page, read.
#[derive(Clone, Debug)]
pub struct Page {
    pub root: Node,
    /// The page's markup with every element named.
    pub html: String,
    pub width: f64,
    pub height: f64,
}

/// Reads the script's answer.
///
/// # Errors
///
/// An answer that is not the tree the script writes.
pub fn parse(answer: &Value) -> Result<Page> {
    let text = answer.as_str().ok_or_else(|| Error::new("the page answered no tree"))?;
    let value = crate::json::parse(text).map_err(Error::new)?;
    let root = value.get("root").map(node).ok_or_else(|| Error::new("the page has no body"))?;
    let html = value.str_at(&["html"]).unwrap_or_default().to_owned();
    let number = |key: &str| value.get(key).and_then(as_f64).unwrap_or(0.0);
    Ok(Page { root, html, width: number("width"), height: number("height") })
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(text) => text.parse().ok(),
        _ => None,
    }
}

fn strings(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .map(Value::members)
        .unwrap_or_default()
        .iter()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect()
}

fn node(value: &Value) -> Node {
    let kind = match value.str_at(&["kind"]) {
        Some("text") => Kind::Text,
        Some("space") => Kind::Space,
        Some("before") => Kind::Before,
        Some("after") => Kind::After,
        _ => Kind::Element,
    };
    let rect = value.get("rect").map(Value::as_array).unwrap_or_default();
    let at = |index: usize| rect.get(index).and_then(as_f64).unwrap_or(0.0);
    let mut rect = Rect { x: at(0), y: at(1), w: at(2), h: at(3) };
    let style = strings(value.get("style"));
    // a box moved by `position: relative` stands, in the layout, where it
    // would have been
    if style.get("position").is_some_and(|position| position == "relative") {
        let shift = |key: &str| style.get(key).and_then(|value| super::css::px(value)).unwrap_or(0.0);
        rect.x -= shift("left");
        rect.y -= shift("top");
    }
    let mut node = Node {
        kind,
        tag: value.str_at(&["tag"]).unwrap_or_default().to_owned(),
        id: value.str_at(&["id"]).unwrap_or_default().to_owned(),
        rect,
        style,
        sizes: strings(value.get("sizes")),
        attrs: strings(value.get("attrs")),
        text: value.str_at(&["text"]).unwrap_or_default().to_owned(),
        children: value.get("children").map(Value::as_array).unwrap_or_default().iter().map(node).collect(),
    };
    // a pseudo-element answers to its element's name, for the report
    let id = node.id.clone();
    for child in &mut node.children {
        match child.kind {
            Kind::Before => child.id = format!("{id}::before"),
            Kind::After => child.id = format!("{id}::after"),
            _ => {}
        }
    }
    node
}
