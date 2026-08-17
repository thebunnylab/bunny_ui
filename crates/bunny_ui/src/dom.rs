//! The Dom lowering — the SEMANTIC scene diffed into element patches.
//!
//! The web premise's second rendering: the same scene that rasterizes on
//! canvas lowers to real elements, so text selects, scroll carries
//! momentum and the browser stays in charge of what it does best. The
//! lowering never reads the display list — it rides the placement walk
//! itself, where the semantic nodes still exist and geometry is already
//! decided. Layout stays OURS on every target; the Dom receives
//! positions, never questions.
//!
//! Three structural choices carry the design:
//!
//! - **Positions are PARENT-RELATIVE.** Every captured node records its
//!   offset from the nearest ancestor that becomes an element. A moved
//!   component keeps its interior byte-identical — one transform patch,
//!   not one per descendant.
//! - **Pointer state never enters the scene.** A box records its base,
//!   hover and pressed backgrounds side by side; the browser flips them
//!   with `:hover`/`:active`. A hover frame diffs to ZERO patches by
//!   construction — the golden below proves it.
//! - **Identity guides the diff.** Component boundaries match by their
//!   identity path (a virtual window sliding = creates and removes,
//!   never a rebuild); everything else matches by position under its
//!   parent, the honest granularity of a re-run body.
//!
//! The patch stream has a fixed little-endian encoding ([`encode`]) —
//! one `DataView` walk on the other side of the border, no JSON.

use std::collections::HashMap;
use std::rc::Rc;

use crate::layout::{
    Color, Point, Px, Rect, Size, Truncation, VisualProps,
};
use crate::text_engine::{FontDesign, FontSpec, Weight};

// MARK: - The captured scene

/// What a scene node IS — the closed set of element kinds the glue
/// knows how to create. Pure-layout nodes (stacks, padding, frames)
/// never appear: their geometry is baked into the children's offsets.
#[derive(Clone, Debug, PartialEq)]
pub enum DomKind {
    /// The mount point — id 0, never created or removed.
    Root,
    /// A component boundary: the diff matches it by identity path.
    Group { path: String },
    /// A styled box (background, border, radius, shadow, interaction).
    Box,
    /// One run of text — the browser renders and selects it natively.
    Text(DomText),
    /// A native `<input>` — the browser owns the editing.
    Field(DomField),
    /// A scroll viewport; `offset` is ours, the element mirrors it.
    Scroll { path: Option<String>, offset: (Px, Px) },
    /// The sized content inside a scroll — the extent the browser
    /// scrolls through (a virtual list sizes it to ALL rows).
    Content,
}

/// The visual record of a node — everything CSS will say about it.
/// Hover and pressed live HERE as alternatives, never resolved: the
/// scene is pointer-invariant.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DomStyle {
    pub background: Option<Color>,
    pub hover_background: Option<Color>,
    pub pressed_background: Option<Color>,
    pub border: Option<(Color, Px)>,
    pub corner_radius: Option<Px>,
    pub shadow: Option<(Px, Color)>,
    /// The action path of the enclosing `Interactive` — the glue posts
    /// clicks back with it, and `:hover`/`:active` scope to it.
    pub interactive: Option<String>,
    /// `(response, damping)` of the enclosing animation scope — the
    /// glue lowers it to a CSS transition; the engine never ticks here.
    pub transition: Option<(f64, f64)>,
}

impl DomStyle {
    fn from_props(props: &VisualProps) -> DomStyle {
        DomStyle {
            background: props.background,
            hover_background: props.background_hovered,
            pressed_background: props.background_pressed,
            border: props.border,
            corner_radius: props.corner_radius,
            shadow: props.shadow,
            interactive: None,
            transition: None,
        }
    }
}

/// One text node, whole: the browser re-breaks lines inside the box
/// with the SAME measures our layout used (the engine is its canvas).
#[derive(Clone, Debug, PartialEq)]
pub struct DomText {
    pub content: Rc<str>,
    pub color: Color,
    pub font: FontSpec,
    /// Match highlight spans (byte ranges) + their color.
    pub highlights: Option<(Rc<Vec<(usize, usize)>>, Color)>,
    pub truncation: Option<Truncation>,
}

/// One text field. Focus, caret and composition stay with the browser;
/// the record carries what the input must SHOW.
#[derive(Clone, Debug, PartialEq)]
pub struct DomField {
    pub path: String,
    pub content: Rc<str>,
    pub placeholder: Rc<str>,
    pub font: FontSpec,
}

/// A captured scene node: kind + parent-relative frame + style +
/// children, exactly what one element needs to exist.
#[derive(Clone, Debug, PartialEq)]
pub struct DomNode {
    pub kind: DomKind,
    /// Offset from the parent NODE's origin (logical px).
    pub x: Px,
    pub y: Px,
    pub width: Px,
    pub height: Px,
    pub style: DomStyle,
    pub children: Vec<DomNode>,
}

// MARK: - Capture (rides the placement walk)

/// The sink the placement fills when Dom mode is on: a stack of open
/// nodes, each with the ABSOLUTE origin its children measure from.
/// Costs nothing when off — the field is `None` and every hook is one
/// branch.
#[derive(Debug)]
pub(crate) struct DomCapture {
    /// `(absolute origin for children, node under construction)`.
    stack: Vec<(Point, DomNode)>,
    /// Armed by an `Animated` scope; the next opened node takes it.
    pending_transition: Option<(f64, f64)>,
    /// Armed by an `Interactive`; the next opened box takes it.
    pending_interactive: Option<String>,
}

impl DomCapture {
    pub(crate) fn new(size: Size) -> DomCapture {
        let root = DomNode {
            kind: DomKind::Root,
            x: 0.0,
            y: 0.0,
            width: size.width,
            height: size.height,
            style: DomStyle::default(),
            children: Vec::new(),
        };
        DomCapture {
            stack: vec![(Point { x: 0.0, y: 0.0 }, root)],
            pending_transition: None,
            pending_interactive: None,
        }
    }

    /// Opens an element node at `frame` (absolute); children placed
    /// until [`close`] land inside it, positioned relative to
    /// `child_origin` (usually the frame's own origin).
    ///
    /// [`close`]: DomCapture::close
    pub(crate) fn open(&mut self, kind: DomKind, frame: Rect, child_origin: Point) {
        let parent_origin = self.stack.last().map(|(origin, _)| *origin).unwrap_or_default();
        let mut style = match &kind {
            DomKind::Box => DomStyle::default(),
            _ => DomStyle::default(),
        };
        if let DomKind::Box = kind {
            style.interactive = self.pending_interactive.take();
        }
        style.transition = self.pending_transition.take();
        let node = DomNode {
            kind,
            x: frame.origin.x - parent_origin.x,
            y: frame.origin.y - parent_origin.y,
            width: frame.size.width,
            height: frame.size.height,
            style,
            children: Vec::new(),
        };
        self.stack.push((child_origin, node));
    }

    /// Opens a styled box straight from a `Styled` node's props.
    pub(crate) fn open_styled(&mut self, props: &VisualProps, frame: Rect) {
        let interactive = self.pending_interactive.take();
        let transition = self.pending_transition.take();
        self.open(DomKind::Box, frame, frame.origin);
        let (_, node) = self.stack.last_mut().expect("just opened");
        node.style = DomStyle {
            interactive,
            transition,
            ..DomStyle::from_props(props)
        };
    }

    /// Paints the OPEN node's background (the plain-box leaves).
    pub(crate) fn set_background(&mut self, color: Color) {
        let (_, node) = self.stack.last_mut().expect("an open node");
        node.style.background = Some(color);
    }

    /// Strokes the OPEN node's border (the stub leaves).
    pub(crate) fn set_border(&mut self, color: Color, width: Px) {
        let (_, node) = self.stack.last_mut().expect("an open node");
        node.style.border = Some((color, width));
    }

    pub(crate) fn close(&mut self) {
        let (_, node) = self.stack.pop().expect("close pairs with open");
        let (_, parent) = self.stack.last_mut().expect("the root never closes");
        parent.children.push(node);
    }

    /// A childless element — open and close in one move.
    pub(crate) fn leaf(&mut self, kind: DomKind, frame: Rect) {
        self.open(kind, frame, frame.origin);
        self.close();
    }

    pub(crate) fn arm_transition(&mut self, response: f64, damping: f64) {
        self.pending_transition = Some((response, damping));
    }

    pub(crate) fn arm_interactive(&mut self, path: &str) {
        self.pending_interactive = Some(path.to_string());
    }

    /// The scope that armed a pending attribute closes: whatever no box
    /// consumed must not leak to a later sibling.
    pub(crate) fn disarm(&mut self) {
        self.pending_transition = None;
        self.pending_interactive = None;
    }

    pub(crate) fn finish(mut self) -> DomNode {
        debug_assert_eq!(self.stack.len(), 1, "every open closed");
        self.stack.pop().expect("the root").1
    }
}

// MARK: - Patches

/// The element kind a `Create` patch carries — what the glue
/// instantiates before the follow-up patches dress it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateKind {
    Group,
    Box,
    Text,
    Field,
    Scroll,
    Content,
}

/// One mutation of the element tree. A frame's worth of patches is the
/// WHOLE difference between two scenes — applying them in order brings
/// the Dom up to date.
#[derive(Clone, Debug, PartialEq)]
pub enum DomPatch {
    /// A new element under `parent`, appended (positions are absolute
    /// within the parent, so sibling order only decides paint stacking).
    Create { id: u32, parent: u32, kind: CreateKind },
    /// Removes the element AND its subtree.
    Remove { id: u32 },
    SetTransform { id: u32, x: f64, y: f64 },
    SetSize { id: u32, width: f64, height: f64 },
    /// The FULL style record — the glue resets and applies (styles are
    /// small; one write per changed node).
    SetStyle { id: u32, style: DomStyle },
    SetText { id: u32, text: DomText },
    SetField { id: u32, field: DomField },
    SetScroll { id: u32, x: f64, y: f64 },
}

// MARK: - Lowering (retained scene + diff)

/// A retained node: the last frame's value plus the element id the
/// glue knows it by.
struct Retained {
    id: u32,
    node: DomNode,
    children: Vec<Retained>,
}

/// The retained side of the Dom mode: last frame's scene with ids.
/// One per runtime; [`lower`] turns each new scene into patches.
///
/// [`lower`]: DomLowering::lower
#[derive(Default)]
pub struct DomLowering {
    root: Option<Retained>,
    next_id: u32,
}

impl DomLowering {
    /// Diffs `scene` against the retained one and returns the patch
    /// list that brings the element tree up to date. The first call
    /// mounts everything.
    pub fn lower(&mut self, scene: &DomNode) -> Vec<DomPatch> {
        let mut patches = Vec::new();
        match self.root.as_mut() {
            None => {
                self.next_id = 1;
                let mut root = Retained {
                    id: 0,
                    node: shallow(scene),
                    children: Vec::new(),
                };
                patches.push(DomPatch::SetSize {
                    id: 0,
                    width: scene.width,
                    height: scene.height,
                });
                let mut next_id = self.next_id;
                root.children = create_children(scene, 0, &mut next_id, &mut patches);
                self.next_id = next_id;
                self.root = Some(root);
            }
            Some(root) => {
                let mut next_id = self.next_id;
                diff_node(root, scene, &mut next_id, &mut patches);
                self.next_id = next_id;
            }
        }
        patches
    }

    /// The scroll region path an element id belongs to — the glue's
    /// scroll observer reports by id, the runtime scrolls by path.
    pub fn scroll_path(&self, id: u32) -> Option<String> {
        fn walk(retained: &Retained, id: u32) -> Option<String> {
            if retained.id == id {
                return match &retained.node.kind {
                    DomKind::Scroll { path, .. } => path.clone(),
                    _ => None,
                };
            }
            retained.children.iter().find_map(|child| walk(child, id))
        }
        self.root.as_ref().and_then(|root| walk(root, id))
    }
}

/// The node without its children — what the retention stores per level.
fn shallow(node: &DomNode) -> DomNode {
    DomNode { children: Vec::new(), ..node.clone() }
}

fn create_kind(kind: &DomKind) -> CreateKind {
    match kind {
        DomKind::Root => unreachable!("the root is never created"),
        DomKind::Group { .. } => CreateKind::Group,
        DomKind::Box => CreateKind::Box,
        DomKind::Text(_) => CreateKind::Text,
        DomKind::Field(_) => CreateKind::Field,
        DomKind::Scroll { .. } => CreateKind::Scroll,
        DomKind::Content => CreateKind::Content,
    }
}

/// Emits the patches that build `node` (already positioned) under
/// `parent` and returns its retained mirror.
fn create_subtree(
    node: &DomNode,
    parent: u32,
    next_id: &mut u32,
    patches: &mut Vec<DomPatch>,
) -> Retained {
    let id = *next_id;
    *next_id += 1;
    patches.push(DomPatch::Create { id, parent, kind: create_kind(&node.kind) });
    patches.push(DomPatch::SetTransform { id, x: node.x, y: node.y });
    patches.push(DomPatch::SetSize { id, width: node.width, height: node.height });
    if node.style != DomStyle::default() {
        patches.push(DomPatch::SetStyle { id, style: node.style.clone() });
    }
    match &node.kind {
        DomKind::Text(text) => {
            patches.push(DomPatch::SetText { id, text: text.clone() });
        }
        DomKind::Field(field) => {
            patches.push(DomPatch::SetField { id, field: field.clone() });
        }
        DomKind::Scroll { offset, .. } if *offset != (0.0, 0.0) => {
            patches.push(DomPatch::SetScroll { id, x: offset.0, y: offset.1 });
        }
        _ => {}
    }
    let children = create_children(node, id, next_id, patches);
    Retained { id, node: shallow(node), children }
}

fn create_children(
    node: &DomNode,
    parent: u32,
    next_id: &mut u32,
    patches: &mut Vec<DomPatch>,
) -> Vec<Retained> {
    node.children
        .iter()
        .map(|child| create_subtree(child, parent, next_id, patches))
        .collect()
}

/// Every element id in the subtree — freed when the root goes.
fn remove_subtree(retained: &Retained, patches: &mut Vec<DomPatch>) {
    patches.push(DomPatch::Remove { id: retained.id });
}

/// Diffs one matched pair: geometry, style, kind payload, children.
fn diff_node(
    retained: &mut Retained,
    new: &DomNode,
    next_id: &mut u32,
    patches: &mut Vec<DomPatch>,
) {
    let id = retained.id;
    let old = &retained.node;
    if (old.x, old.y) != (new.x, new.y) {
        patches.push(DomPatch::SetTransform { id, x: new.x, y: new.y });
    }
    if (old.width, old.height) != (new.width, new.height) {
        patches.push(DomPatch::SetSize { id, width: new.width, height: new.height });
    }
    if old.style != new.style {
        patches.push(DomPatch::SetStyle { id, style: new.style.clone() });
    }
    match (&old.kind, &new.kind) {
        (DomKind::Text(before), DomKind::Text(after)) if before != after => {
            patches.push(DomPatch::SetText { id, text: after.clone() });
        }
        (DomKind::Field(before), DomKind::Field(after)) if before != after => {
            patches.push(DomPatch::SetField { id, field: after.clone() });
        }
        (
            DomKind::Scroll { offset: before, .. },
            DomKind::Scroll { offset: after, .. },
        ) if before != after => {
            patches.push(DomPatch::SetScroll { id, x: after.0, y: after.1 });
        }
        _ => {}
    }
    retained.node = shallow(new);
    diff_children(retained, new, next_id, patches);
}

/// Matches the children lists: groups by identity path (a slid window
/// keeps its rows), everything else by position and kind. Unmatched old
/// children leave; unmatched new ones mount.
fn diff_children(
    retained: &mut Retained,
    new: &DomNode,
    next_id: &mut u32,
    patches: &mut Vec<DomPatch>,
) {
    let old_children = std::mem::take(&mut retained.children);
    let mut by_path: HashMap<String, Retained> = HashMap::new();
    let mut by_index: Vec<Option<Retained>> = Vec::with_capacity(old_children.len());
    for old in old_children {
        if let DomKind::Group { path } = &old.node.kind {
            by_path.insert(path.clone(), old);
            by_index.push(None);
        } else {
            by_index.push(Some(old));
        }
    }

    let mut next: Vec<Retained> = Vec::with_capacity(new.children.len());
    for (index, child) in new.children.iter().enumerate() {
        let matched = match &child.kind {
            DomKind::Group { path } => by_path.remove(path),
            kind => by_index
                .get_mut(index)
                .and_then(Option::take)
                .filter(|old| {
                    std::mem::discriminant(&old.node.kind) == std::mem::discriminant(kind)
                }),
        };
        match matched {
            Some(mut old) => {
                diff_node(&mut old, child, next_id, patches);
                next.push(old);
            }
            None => next.push(create_subtree(child, retained.id, next_id, patches)),
        }
    }

    for (_, leftover) in by_path {
        remove_subtree(&leftover, patches);
    }
    for leftover in by_index.into_iter().flatten() {
        remove_subtree(&leftover, patches);
    }
    retained.children = next;
}

// MARK: - The wire encoding

/// Encodes a patch list into the fixed little-endian stream the glue
/// decodes with one `DataView` walk. Layout:
///
/// ```text
/// u32 count
/// per patch: u8 op, u32 id, payload
///   1 create        u32 parent, u8 kind (0 group, 1 box, 2 text,
///                                        3 field, 4 scroll, 5 content)
///   2 remove        —
///   3 set transform f32 x, f32 y
///   4 set size      f32 w, f32 h
///   5 set style     u16 mask, fields in bit order:
///                   0 background u32 rgba   1 hover u32   2 pressed u32
///                   3 border u32 rgba + f32 width          4 radius f32
///                   5 shadow f32 radius + u32 rgba
///                   6 transition f32 response + f32 damping
///                   7 interactive u16 len + utf8
///   6 set text      u32 rgba, f32 size, u8 weight, u8 mono,
///                   u8 truncation (0 none, 1 start, 2 middle, 3 end),
///                   u32 len + utf8, u16 span count,
///                   spans (u32 start, u32 end), u32 span rgba
///   7 set field     f32 size, u8 weight, u8 mono,
///                   u32 len + utf8 content, u32 len + utf8 placeholder,
///                   u16 len + utf8 path
///   8 set scroll    f32 x, f32 y
/// ```
pub fn encode(patches: &[DomPatch]) -> Vec<u8> {
    let mut out = Vec::with_capacity(patches.len() * 16 + 4);
    push_u32(&mut out, patches.len() as u32);
    for patch in patches {
        match patch {
            DomPatch::Create { id, parent, kind } => {
                out.push(1);
                push_u32(&mut out, *id);
                push_u32(&mut out, *parent);
                out.push(match kind {
                    CreateKind::Group => 0,
                    CreateKind::Box => 1,
                    CreateKind::Text => 2,
                    CreateKind::Field => 3,
                    CreateKind::Scroll => 4,
                    CreateKind::Content => 5,
                });
            }
            DomPatch::Remove { id } => {
                out.push(2);
                push_u32(&mut out, *id);
            }
            DomPatch::SetTransform { id, x, y } => {
                out.push(3);
                push_u32(&mut out, *id);
                push_f32(&mut out, *x);
                push_f32(&mut out, *y);
            }
            DomPatch::SetSize { id, width, height } => {
                out.push(4);
                push_u32(&mut out, *id);
                push_f32(&mut out, *width);
                push_f32(&mut out, *height);
            }
            DomPatch::SetStyle { id, style } => {
                out.push(5);
                push_u32(&mut out, *id);
                encode_style(&mut out, style);
            }
            DomPatch::SetText { id, text } => {
                out.push(6);
                push_u32(&mut out, *id);
                push_u32(&mut out, pack_color(text.color));
                push_f32(&mut out, text.font.size);
                out.push(weight_code(text.font.weight));
                out.push(matches!(text.font.design, FontDesign::Mono) as u8);
                out.push(match text.truncation {
                    None => 0,
                    Some(Truncation::Start) => 1,
                    Some(Truncation::Middle) => 2,
                    Some(Truncation::End) => 3,
                });
                push_bytes_u32(&mut out, text.content.as_bytes());
                match &text.highlights {
                    Some((ranges, color)) => {
                        push_u16(&mut out, ranges.len() as u16);
                        for (start, end) in ranges.iter() {
                            push_u32(&mut out, *start as u32);
                            push_u32(&mut out, *end as u32);
                        }
                        push_u32(&mut out, pack_color(*color));
                    }
                    None => {
                        push_u16(&mut out, 0);
                        push_u32(&mut out, 0);
                    }
                }
            }
            DomPatch::SetField { id, field } => {
                out.push(7);
                push_u32(&mut out, *id);
                push_f32(&mut out, field.font.size);
                out.push(weight_code(field.font.weight));
                out.push(matches!(field.font.design, FontDesign::Mono) as u8);
                push_bytes_u32(&mut out, field.content.as_bytes());
                push_bytes_u32(&mut out, field.placeholder.as_bytes());
                push_bytes_u16(&mut out, field.path.as_bytes());
            }
            DomPatch::SetScroll { id, x, y } => {
                out.push(8);
                push_u32(&mut out, *id);
                push_f32(&mut out, *x);
                push_f32(&mut out, *y);
            }
        }
    }
    out
}

fn encode_style(out: &mut Vec<u8>, style: &DomStyle) {
    let mut mask: u16 = 0;
    if style.background.is_some() {
        mask |= 1;
    }
    if style.hover_background.is_some() {
        mask |= 1 << 1;
    }
    if style.pressed_background.is_some() {
        mask |= 1 << 2;
    }
    if style.border.is_some() {
        mask |= 1 << 3;
    }
    if style.corner_radius.is_some() {
        mask |= 1 << 4;
    }
    if style.shadow.is_some() {
        mask |= 1 << 5;
    }
    if style.transition.is_some() {
        mask |= 1 << 6;
    }
    if style.interactive.is_some() {
        mask |= 1 << 7;
    }
    push_u16(out, mask);
    if let Some(color) = style.background {
        push_u32(out, pack_color(color));
    }
    if let Some(color) = style.hover_background {
        push_u32(out, pack_color(color));
    }
    if let Some(color) = style.pressed_background {
        push_u32(out, pack_color(color));
    }
    if let Some((color, width)) = style.border {
        push_u32(out, pack_color(color));
        push_f32(out, width);
    }
    if let Some(radius) = style.corner_radius {
        push_f32(out, radius);
    }
    if let Some((radius, color)) = style.shadow {
        push_f32(out, radius);
        push_u32(out, pack_color(color));
    }
    if let Some((response, damping)) = style.transition {
        push_f32(out, response);
        push_f32(out, damping);
    }
    if let Some(path) = &style.interactive {
        push_bytes_u16(out, path.as_bytes());
    }
}

fn weight_code(weight: Weight) -> u8 {
    match weight {
        Weight::Regular => 0,
        Weight::Medium => 1,
        Weight::Semibold => 2,
        Weight::Bold => 3,
    }
}

fn pack_color(color: Color) -> u32 {
    (color.r as u32) << 24 | (color.g as u32) << 16 | (color.b as u32) << 8 | color.a as u32
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&(value as f32).to_le_bytes());
}

fn push_bytes_u32(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn push_bytes_u16(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u16(out, bytes.len() as u16);
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Size;
    use crate::prelude::*;
    use crate::runtime::Runtime;

    fn patch_id(patch: &DomPatch) -> u32 {
        match patch {
            DomPatch::Create { id, .. }
            | DomPatch::Remove { id }
            | DomPatch::SetTransform { id, .. }
            | DomPatch::SetSize { id, .. }
            | DomPatch::SetStyle { id, .. }
            | DomPatch::SetText { id, .. }
            | DomPatch::SetField { id, .. }
            | DomPatch::SetScroll { id, .. } => *id,
        }
    }

    #[derive(Clone)]
    struct MiniList {
        selected: State<usize>,
        count: State<usize>,
    }

    impl Component for MiniList {
        fn body(self, _ctx: &Context) -> impl View {
            let count = self.count.get();
            let selected = self.selected;
            let selected_index = selected.get();
            crate::vstack!(
                text("header"),
                list(
                    (0..count).collect::<Vec<_>>(),
                    |row| format!("row{row}"),
                    move |row| {
                        let row = *row;
                        let on = row == selected_index;
                        text(format!("item {row}"))
                            .background_color(if on {
                                Color::hex(0x334455)
                            } else {
                                Color::hex_a(0x0000_0000)
                            })
                            .on_click(move || selected.set(row))
                    },
                )
            )
        }
    }

    fn mini() -> (Runtime, MiniList, Size) {
        let runtime = Runtime::new();
        let view = MiniList { selected: State::new(0), count: State::new(3) };
        let size = Size { width: 200.0, height: 150.0 };
        (runtime, view, size)
    }

    #[test]
    fn the_first_frame_mounts_the_whole_scene() {
        let (runtime, view, size) = mini();
        let patches = runtime.dom_frame(&view, size);

        assert!(matches!(patches[0], DomPatch::SetSize { id: 0, .. }));
        let creates: Vec<_> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { id, parent, kind } => Some((*id, *parent, *kind)),
                _ => None,
            })
            .collect();
        // one text per row plus the header
        let texts = creates.iter().filter(|(_, _, kind)| *kind == CreateKind::Text).count();
        assert_eq!(texts, 4, "header + three rows: {creates:?}");
        assert_eq!(
            creates.iter().filter(|(_, _, kind)| *kind == CreateKind::Scroll).count(),
            1
        );
        assert_eq!(
            creates.iter().filter(|(_, _, kind)| *kind == CreateKind::Content).count(),
            1
        );
        // parents always exist before their children — the glue applies
        // in order with no lookahead
        let mut known = vec![0u32];
        for (id, parent, _) in &creates {
            assert!(known.contains(parent), "parent {parent} unseen for {id}");
            known.push(*id);
        }
        // the interactive rows carry their action paths
        let interactive = patches.iter().any(|patch| {
            matches!(patch, DomPatch::SetStyle { style, .. } if style.interactive.is_some())
        });
        assert!(interactive, "rows are clickable in the scene");
    }

    #[test]
    fn a_hover_frame_diffs_to_zero_patches() {
        let (runtime, view, size) = mini();
        let _ = runtime.dom_frame(&view, size);

        // find a row to hover over — the layout knows the hit targets
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let target = result
            .hits
            .iter()
            .find(|(path, _)| path.contains("[row1]"))
            .map(|(_, rect)| {
                (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0)
            })
            .expect("row1 is clickable");
        assert!(runtime.pointer_moved(target.0, target.1), "the hover state flipped");

        let patches = runtime.dom_frame(&view, size);
        assert_eq!(patches, vec![], "hover is the browser's — the scene never moves");
    }

    #[test]
    fn a_selection_change_patches_only_the_two_styles() {
        let (runtime, view, size) = mini();
        let _ = runtime.dom_frame(&view, size);

        view.selected.set(1);
        let patches = runtime.dom_frame(&view, size);

        assert!(!patches.is_empty());
        for patch in &patches {
            assert!(
                matches!(patch, DomPatch::SetStyle { .. }),
                "only styles move on a selection change: {patch:?}"
            );
        }
        assert_eq!(patches.len(), 2, "the old row and the new one: {patches:?}");
    }

    #[test]
    fn a_removed_row_leaves_and_nothing_mounts() {
        let (runtime, view, size) = mini();
        let _ = runtime.dom_frame(&view, size);

        view.count.set(2);
        let patches = runtime.dom_frame(&view, size);

        let removes = patches.iter().filter(|p| matches!(p, DomPatch::Remove { .. })).count();
        let creates = patches.iter().filter(|p| matches!(p, DomPatch::Create { .. })).count();
        assert_eq!(removes, 1, "row2's boundary goes, one subtree remove: {patches:?}");
        assert_eq!(creates, 0, "the surviving rows matched by identity");
    }

    #[test]
    fn a_moved_component_is_one_transform_and_an_untouched_interior() {
        #[derive(Clone, Copy)]
        struct Inner;

        impl Component for Inner {
            fn body(self, _ctx: &Context) -> impl View {
                text("steady").background_color(Color::hex(0x223344))
            }
        }

        #[derive(Clone)]
        struct Outer {
            gap: State<f64>,
        }

        impl Component for Outer {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(text("mover").padding_length(self.gap.get()), Inner)
            }
        }

        let runtime = Runtime::new();
        let view = Outer { gap: State::new(4.0) };
        let size = Size { width: 200.0, height: 150.0 };
        let mount = runtime.dom_frame(&view, size);
        let inner_group = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Group, .. } => Some(*id),
                _ => None,
            })
            .last()
            .expect("Inner mounted as a group");

        view.gap.set(12.0);
        let patches = runtime.dom_frame(&view, size);

        let on_inner: Vec<_> =
            patches.iter().filter(|patch| patch_id(patch) >= inner_group).collect();
        assert_eq!(on_inner.len(), 1, "the moved component: {patches:?}");
        assert!(
            matches!(on_inner[0], DomPatch::SetTransform { id, .. } if *id == inner_group),
            "one transform on the boundary, interior byte-identical: {patches:?}"
        );
    }

    #[test]
    fn a_wheel_is_one_scroll_patch() {
        let (runtime, view, size) = mini();
        view.count.set(30);
        let _ = runtime.dom_frame(&view, size);

        assert!(runtime.wheel(100.0, 80.0, 0.0, -40.0), "the region moved");
        let patches = runtime.dom_frame(&view, size);
        assert_eq!(patches.len(), 1, "content never moves — the offset does: {patches:?}");
        assert!(matches!(patches[0], DomPatch::SetScroll { .. }));
    }

    #[test]
    fn a_virtual_jump_is_creates_removes_and_the_offset() {
        #[derive(Clone, Copy)]
        struct Big;

        impl Component for Big {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(10_000, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 200.0, height: 150.0 };
        let _ = runtime.dom_frame(&Big, size);

        assert!(runtime.wheel(100.0, 80.0, 0.0, -50_000.0));
        let patches = runtime.dom_frame(&Big, size);

        let created: Vec<u32> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        let removes = patches.iter().filter(|p| matches!(p, DomPatch::Remove { .. })).count();
        assert!(!created.is_empty(), "the far band mounted");
        assert!(removes > 0, "the old window left");
        // surviving nodes sit at index × extent inside the content box —
        // a slid window never drags an existing element around (the only
        // transforms dress the freshly created ones)
        let moved_survivor = patches.iter().any(|patch| {
            matches!(patch, DomPatch::SetTransform { id, .. } if !created.contains(id))
        });
        assert!(!moved_survivor, "nothing moves in content coordinates: {patches:?}");
        assert!(patches.iter().any(|p| matches!(p, DomPatch::SetScroll { .. })));
    }

    #[test]
    fn typing_in_a_field_is_one_field_patch() {
        #[derive(Clone)]
        struct WithField {
            query: State<String>,
        }

        impl Component for WithField {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("type here", self.query.binding()).auto_focus()
            }
        }

        let runtime = Runtime::new();
        let view = WithField { query: State::new(String::new()) };
        let size = Size { width: 200.0, height: 60.0 };
        let _ = runtime.dom_frame(&view, size);

        assert!(runtime.key(crate::text_input::EditCommand::Insert("x".into())).applied);
        let patches = runtime.dom_frame(&view, size);
        assert_eq!(patches.len(), 1, "the input mirrors the content: {patches:?}");
        match &patches[0] {
            DomPatch::SetField { field, .. } => assert_eq!(field.content.as_ref(), "x"),
            other => panic!("a field patch, not {other:?}"),
        }
    }

    #[test]
    fn hover_variants_ride_into_the_scene() {
        #[derive(Clone, Copy)]
        struct Hoverable;

        impl Component for Hoverable {
            fn body(self, _ctx: &Context) -> impl View {
                text("hi")
                    .background_color(Color::hex(0x111111))
                    .background_hovered(Color::hex(0x222222))
                    .animated(crate::anim::Spring::snappy())
                    .on_click(|| {})
            }
        }

        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&Hoverable, Size { width: 100.0, height: 50.0 });
        let hovered = patches.iter().any(|patch| {
            matches!(patch, DomPatch::SetStyle { style, .. } if style.hover_background.is_some())
        });
        assert!(hovered, "the :hover alternative reached the patches: {patches:#?}");
    }

    #[test]
    fn the_encoding_is_byte_stable() {
        let patches = vec![
            DomPatch::Create { id: 7, parent: 0, kind: CreateKind::Box },
            DomPatch::SetTransform { id: 7, x: 10.0, y: 20.0 },
            DomPatch::SetStyle {
                id: 7,
                style: DomStyle {
                    background: Some(Color::hex(0x112233)),
                    interactive: Some("go".to_string()),
                    ..DomStyle::default()
                },
            },
            DomPatch::Remove { id: 7 },
        ];
        let bytes = encode(&patches);
        let expected: Vec<u8> = [
            &4u32.to_le_bytes()[..],
            &[1],
            &7u32.to_le_bytes()[..],
            &0u32.to_le_bytes()[..],
            &[1],
            &[3],
            &7u32.to_le_bytes()[..],
            &10f32.to_le_bytes()[..],
            &20f32.to_le_bytes()[..],
            &[5],
            &7u32.to_le_bytes()[..],
            &(1u16 | 1 << 7).to_le_bytes()[..],
            &0x112233FFu32.to_le_bytes()[..],
            &2u16.to_le_bytes()[..],
            b"go",
            &[2],
            &7u32.to_le_bytes()[..],
        ]
        .concat();
        assert_eq!(bytes, expected);
    }
}
