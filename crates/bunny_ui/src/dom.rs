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
use std::sync::Arc;

use crate::layout::{
    Color, Corners, DrawCommand, Point, Px, Rect, Size, Truncation, VisualProps,
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
    /// A canvas island (`.rendering(Gpu)`): our layout positions the
    /// element; the subtree's draw commands fill it. `origin` is the
    /// island's ABSOLUTE frame origin (the commands translate by it)
    /// and `display` the `[start, end)` range into the pass's list.
    Canvas { origin: (Px, Px), display: (usize, usize) },
    /// An `<img>` — the browser fetches, decodes and paints it. The
    /// record carries the IDENTITY; the shell's registry maps it to a
    /// URL the browser can load.
    Image(DomImage),
    /// An `<svg>` — a vector glyph rendered AT HOME: the browser
    /// scales the drawing, and the tint is `currentColor`, so hover
    /// and press flip through the box above with no patch of their
    /// own.
    Icon(DomIcon),
}

/// One image element. `key` is the source identity ([`crate::
/// image_engine::ImageSource::key`]); `cover` picks `object-fit`
/// (`false` = our frame IS the rect, the element just fills it;
/// `true` = the browser covers-and-clips with the same centered math).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomImage {
    pub key: u64,
    pub cover: bool,
}

/// One vector glyph element. `key` is the SYMBOL's identity — never
/// the tinted one: a re-tint moves the style and leaves the geometry
/// alone. The drawing rides as the `Symbol` (Copy, two words); the
/// encoder reads its verbs only when a patch mounts or changes, so a
/// warm frame never touches them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomIcon {
    pub key: u64,
    pub symbol: crate::icon::Symbol,
    /// The inherited ink — the record of what the element shows.
    pub color: Color,
    /// Under a hover ink the element takes NO color of its own: the
    /// box above declares both states and CSS carries them down.
    pub inherits_ink: bool,
}

/// The visual record of a node — everything CSS will say about it.
/// Hover and pressed live HERE as alternatives, never resolved: the
/// scene is pointer-invariant.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DomStyle {
    pub background: Option<Color>,
    /// A two-stop ramp over the flat background — the browser's own
    /// `radial-gradient`/`linear-gradient`. The geometry is ours (a
    /// proportional centre, a direction); the pixels are the
    /// browser's, like every other paint in this mode.
    pub gradient: Option<crate::layout::Gradient>,
    pub hover_background: Option<Color>,
    pub pressed_background: Option<Color>,
    /// The ink this box hands DOWN. It only travels when a state below
    /// needs it: the text inherits instead of painting its own color,
    /// so the browser flips the whole subtree on `:hover`.
    pub color: Option<Color>,
    pub hover_color: Option<Color>,
    pub pressed_color: Option<Color>,
    pub border: Option<(Color, Px)>,
    pub corner_radius: Option<Corners>,
    pub shadow: Option<(Px, Color)>,
    /// The action path of the enclosing `Interactive` — the glue posts
    /// clicks back with it, and `:hover`/`:active` scope to it.
    pub interactive: Option<String>,
    /// `(response, damping)` of the enclosing animation scope — the
    /// glue lowers it to a CSS transition; the engine never ticks here.
    pub transition: Option<(f64, f64)>,
    /// A field's border while focused — the glue's `:focus` rule (and
    /// its caret color): the browser flips it, the engine never hears.
    pub focus_border: Option<Color>,
    /// A field's placeholder ink — the glue's `::placeholder` rule.
    pub placeholder_color: Option<Color>,
    /// `.clipped()` — the glue's `overflow:hidden`, which pairs with
    /// the radius already on the box: the browser cuts the subtree to
    /// the curve as a LAYER, its own native rounded clip.
    pub clip: bool,
    /// `.tooltip(…)` — in THIS mode the browser owns the wait and the
    /// bubble (a CSS rule on a data attribute), the way it owns the
    /// hover and the inputs: zero patches by construction. The pixel
    /// modes run the engine's own bubble instead.
    pub tooltip: Option<Arc<str>>,
    /// `.opacity(…)` and its two states. In THIS mode the fade is a
    /// real LAYER — the browser composites the subtree once — which is
    /// strictly better than the per-command multiply the pixel
    /// pipelines do, and costs the scene nothing.
    pub opacity: Option<f64>,
    pub hover_opacity: Option<f64>,
    pub pressed_opacity: Option<f64>,
    /// `.group_hovered()` — the ancestor whose `:hover` drives this
    /// box's state paint, as a NUMBER (the same reason images cross as
    /// keys: a path is for people, and the browser only needs an
    /// anchor). The glue turns it into a descendant selector, so the
    /// browser keeps owning the hover and a group frame still costs
    /// zero patches.
    pub group: Option<u64>,
    /// The box a `.hover_group()` owns names itself here — the anchor
    /// every follower's selector points at.
    pub group_owner: Option<u64>,
    /// Inside a `.overlay(…)`/`.background(…)` layer that asks for
    /// nothing: the box lets the pointer THROUGH to what it covers. A
    /// rule or an insertion marker must not eat the click that belongs
    /// to the row underneath, and in this mode the browser routes by
    /// element and not by our hit list.
    pub pass_through: bool,
}

impl DomStyle {
    fn from_props(props: &VisualProps) -> DomStyle {
        DomStyle {
            background: props.background,
            gradient: props.gradient,
            hover_background: props.background_hovered,
            pressed_background: props.background_pressed,
            color: None,
            hover_color: props.foreground_hovered,
            pressed_color: props.foreground_pressed,
            border: props.border,
            corner_radius: props.corner_radius,
            shadow: props.shadow,
            interactive: None,
            transition: None,
            focus_border: None,
            placeholder_color: None,
            clip: props.clip,
            tooltip: None,
            opacity: props.opacity,
            hover_opacity: props.opacity_hovered,
            pressed_opacity: props.opacity_pressed,
            group: None,
            group_owner: None,
            pass_through: false,
        }
    }
}

/// One text node, whole: the browser re-breaks lines inside the box
/// with the SAME measures our layout used (the engine is its canvas).
#[derive(Clone, Debug, PartialEq)]
pub struct DomText {
    pub content: Arc<str>,
    pub color: Color,
    /// Under a hover ink the element takes NO color of its own: the box
    /// above declares both states and CSS inheritance carries them
    /// down. `color` stays as the record of what it inherits.
    pub inherits_ink: bool,
    pub font: FontSpec,
    /// Match highlight spans (byte ranges) + their color.
    pub highlights: Option<(Rc<Vec<(usize, usize)>>, Color)>,
    pub truncation: Option<Truncation>,
}

/// One text field. Focus, caret and composition stay with the browser;
/// the record carries what the input must SHOW — including its text
/// ink (the chrome rides the node's [`DomStyle`], from the theme).
#[derive(Clone, Debug, PartialEq)]
pub struct DomField {
    pub path: String,
    pub content: Arc<str>,
    pub placeholder: Arc<str>,
    pub font: FontSpec,
    pub color: Color,
    /// Many lines: the glue builds a `<textarea>` instead of an
    /// `<input>`, and the browser wraps and scrolls it at home.
    pub multiline: bool,
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
    /// The ancestors that declared themselves hover groups.
    groups: Vec<u64>,
    /// How many overlay layers are open around here. What a layer
    /// paints lets the pointer THROUGH unless it asks for a target of
    /// its own — a rule must not eat the click of the row it crosses.
    overlay_depth: usize,
    /// The BASE ink of every open node, in step with `stack` — the
    /// color a text inherits, never the hovered one the pointer
    /// resolved. This is what keeps the capture pointer-invariant.
    ink: Vec<Color>,
    /// A `.tooltip(…)` waiting for the NEXT opened node — the wrapper
    /// is transparent, so the text lands on its child's element.
    armed_tooltip: Option<Arc<str>>,
    /// Stack depths where a box declared a hover/pressed ink. While one
    /// is open the text below inherits its color instead of setting it,
    /// which is what lets the browser flip the whole subtree.
    ink_scopes: Vec<usize>,
    /// Island nesting depth. Above zero the subtree is PIXELS, not
    /// elements: every open/leaf below the canvas node is swallowed —
    /// the draw commands already carry the content.
    island: usize,
    /// Opens swallowed while inside an island — their closes pair up.
    swallowed: usize,
}

impl DomCapture {
    pub(crate) fn new(size: Size) -> DomCapture {
        // the scene's floor is the THEME's canvas — the same contract
        // as the raster surface's background, never a page stylesheet
        let root = DomNode {
            kind: DomKind::Root,
            x: 0.0,
            y: 0.0,
            width: size.width,
            height: size.height,
            style: DomStyle {
                background: Some(crate::theme::current().canvas),
                ..DomStyle::default()
            },
            children: Vec::new(),
        };
        DomCapture {
            stack: vec![(Point { x: 0.0, y: 0.0 }, root)],
            pending_transition: None,
            pending_interactive: None,
            groups: Vec::new(),
            overlay_depth: 0,
            // the scene's ink floor is the theme's, the same one the
            // place walk starts from
            ink: vec![crate::theme::current().fg],
            armed_tooltip: None,
            ink_scopes: Vec::new(),
            island: 0,
            swallowed: 0,
        }
    }

    /// Opens an element node at `frame` (absolute); children placed
    /// until [`close`] land inside it, positioned relative to
    /// `child_origin` (usually the frame's own origin).
    ///
    /// [`close`]: DomCapture::close
    pub(crate) fn open(&mut self, kind: DomKind, frame: Rect, child_origin: Point) {
        if self.island > 0 {
            self.swallowed += 1;
            return;
        }
        let parent_origin = self.stack.last().map(|(origin, _)| *origin).unwrap_or_default();
        let mut style = match &kind {
            DomKind::Box => DomStyle::default(),
            _ => DomStyle::default(),
        };
        if let DomKind::Box = kind {
            style.interactive = self.pending_interactive.take();
        }
        // inside a layer, a box that asks for nothing lets the pointer
        // through to what it covers
        style.pass_through = self.overlay_depth > 0 && style.interactive.is_none();
        style.transition = self.pending_transition.take();
        style.tooltip = self.armed_tooltip.take();
        let node = DomNode {
            kind,
            x: frame.origin.x - parent_origin.x,
            y: frame.origin.y - parent_origin.y,
            width: frame.size.width,
            height: frame.size.height,
            style,
            children: Vec::new(),
        };
        // the node inherits the ink until a `Styled` says otherwise
        self.ink.push(self.current_ink());
        self.stack.push((child_origin, node));
    }

    /// Opens a styled box straight from a `Styled` node's props.
    pub(crate) fn open_styled(&mut self, props: &VisualProps, frame: Rect) {
        if self.island > 0 {
            self.swallowed += 1;
            return;
        }
        let interactive = self.pending_interactive.take();
        let transition = self.pending_transition.take();
        let group = self.current_group();
        let states = props.foreground_hovered.is_some() || props.foreground_pressed.is_some();
        // inside a hover ink the text inherits, so a box that changes
        // the ink must SAY so — otherwise the inheritance walks past it
        let inheriting = !self.ink_scopes.is_empty() && props.foreground.is_some();
        self.open(DomKind::Box, frame, frame.origin);
        if let Some(color) = props.foreground {
            *self.ink.last_mut().expect("the open node owns an ink") = color;
        }
        let ink = self.current_ink();
        let (_, node) = self.stack.last_mut().expect("just opened");
        // the rebuild must not drop what open() already stamped — a
        // tooltip armed by the wrapper lands on THIS box
        let tooltip = node.style.tooltip.take();
        node.style = DomStyle {
            // the rebuild must not drop what the layer scope decided
            pass_through: self.overlay_depth > 0 && interactive.is_none(),
            interactive,
            transition,
            tooltip,
            group: props.from_group.then(|| group).flatten(),
            ..DomStyle::from_props(props)
        };
        if states || inheriting {
            node.style.color = Some(ink);
        }
        if states {
            self.ink_scopes.push(self.stack.len());
        }
    }

    /// The ink the open node hands down: the BASE color, never the
    /// hovered one. The scene the browser gets stays pointer-invariant.
    fn current_ink(&self) -> Color {
        *self.ink.last().expect("the root seeds the ink")
    }

    /// Paints the OPEN node's background (the plain-box leaves).
    pub(crate) fn set_background(&mut self, color: Color) {
        if self.island > 0 {
            return;
        }
        let (_, node) = self.stack.last_mut().expect("an open node");
        node.style.background = Some(color);
    }

    /// Strokes the OPEN node's border (the stub leaves).
    pub(crate) fn set_border(&mut self, color: Color, width: Px) {
        if self.island > 0 {
            return;
        }
        let (_, node) = self.stack.last_mut().expect("an open node");
        node.style.border = Some((color, width));
    }

    pub(crate) fn close(&mut self) {
        if self.swallowed > 0 {
            self.swallowed -= 1;
            return;
        }
        if self.ink_scopes.last() == Some(&self.stack.len()) {
            self.ink_scopes.pop();
        }
        self.ink.pop();
        let (_, node) = self.stack.pop().expect("close pairs with open");
        let (_, parent) = self.stack.last_mut().expect("the root never closes");
        parent.children.push(node);
    }

    /// A childless element — open and close in one move.
    /// Arms a `.tooltip(…)` for the NEXT opened node — the placement
    /// calls it just before the wrapped child places.
    pub(crate) fn arm_tooltip(&mut self, text: Arc<str>) {
        if self.island == 0 {
            self.armed_tooltip = Some(text);
        }
    }

    pub(crate) fn leaf(&mut self, kind: DomKind, frame: Rect) {
        if self.island > 0 {
            return;
        }
        let mut kind = kind;
        if let DomKind::Text(text) = &mut kind {
            // the ink is the INHERITED one, never the resolved paint —
            // and under a hover ink the element sets no color at all,
            // or its own would outrank the rule that flips it
            text.color = self.current_ink();
            text.inherits_ink = !self.ink_scopes.is_empty();
        }
        if let DomKind::Icon(icon) = &mut kind {
            // the glyph fills with currentColor — same law as the text
            icon.color = self.current_ink();
            icon.inherits_ink = !self.ink_scopes.is_empty();
        }
        self.open(kind, frame, frame.origin);
        self.close();
    }

    /// A childless element that carries its own style (the field's
    /// theme chrome travels in the record, never hardcoded in a glue).
    pub(crate) fn leaf_styled(&mut self, kind: DomKind, frame: Rect, style: DomStyle) {
        if self.island > 0 {
            return;
        }
        let mut kind = kind;
        if let DomKind::Field(field) = &mut kind {
            // the input keeps an inline ink (it never inherits a hover
            // state) — but the INHERITED one, so the record stays
            // pointer-invariant
            field.color = self.current_ink();
        }
        self.open(kind, frame, frame.origin);
        let pass_through = self.overlay_depth > 0 && style.interactive.is_none();
        let (_, node) = self.stack.last_mut().expect("just opened");
        node.style = DomStyle { pass_through, ..style };
        self.close();
    }

    pub(crate) fn arm_transition(&mut self, response: f64, damping: f64) {
        self.pending_transition = Some((response, damping));
    }

    pub(crate) fn arm_interactive(&mut self, path: &str) {
        self.pending_interactive = Some(path.to_string());
    }

    /// Opens the box a hover group owns. It exists only in this mode
    /// and only for the selector: a descendant's state rules hang off
    /// an ANCESTOR, and an ancestor is the one thing CSS can name.
    pub(crate) fn open_group(&mut self, key: u64, frame: Rect) {
        self.groups.push(key);
        if self.island > 0 {
            self.swallowed += 1;
            return;
        }
        self.open(DomKind::Box, frame, frame.origin);
        let (_, node) = self.stack.last_mut().expect("just opened");
        node.style.group_owner = Some(key);
    }

    /// Opens/closes an overlay LAYER scope: what it paints inside is
    /// decoration until something in it asks to be a target.
    pub(crate) fn enter_layer(&mut self) {
        self.overlay_depth += 1;
    }

    pub(crate) fn leave_layer(&mut self) {
        self.overlay_depth = self.overlay_depth.saturating_sub(1);
    }

    pub(crate) fn close_group(&mut self) {
        self.groups.pop();
        self.close();
    }

    /// The scope that armed a pending attribute closes: whatever no box
    /// consumed must not leak to a later sibling.
    pub(crate) fn disarm(&mut self) {
        self.pending_transition = None;
        self.pending_interactive = None;
    }

    /// The nearest ancestor that declared itself a hover group.
    fn current_group(&self) -> Option<u64> {
        self.groups.last().copied()
    }

    /// Opens a canvas island at `frame`; `start` is where the island's
    /// draw commands begin in the pass's display list. An island inside
    /// an island dissolves — the outer one already owns the pixels.
    pub(crate) fn open_canvas(&mut self, frame: Rect, start: usize) {
        if self.island > 0 {
            self.island += 1;
            return;
        }
        self.open(
            DomKind::Canvas {
                origin: (frame.origin.x, frame.origin.y),
                display: (start, start),
            },
            frame,
            frame.origin,
        );
        self.island = 1;
    }

    /// Closes the island, sealing the display range at `end`.
    pub(crate) fn close_canvas(&mut self, end: usize) {
        if self.island > 1 {
            self.island -= 1;
            return;
        }
        self.island = 0;
        let (_, node) = self.stack.last_mut().expect("an open island");
        if let DomKind::Canvas { display, .. } = &mut node.kind {
            display.1 = end;
        }
        self.close();
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
    Canvas,
    Image,
    Icon,
    /// A `<textarea>`: the field of many lines. A separate kind because
    /// the ELEMENT differs — a field that changes shape is recreated,
    /// which is the only way an input becomes a textarea.
    Editor,
}

/// One island's fresh pixels — the shell blits them into the island's
/// `<canvas>`. Only islands whose commands actually changed re-raster.
pub struct IslandFrame {
    pub id: u32,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
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
    SetImage { id: u32, image: DomImage },
    SetIcon { id: u32, icon: DomIcon },
}

// MARK: - Lowering (retained scene + diff)

/// A retained node: the last frame's value plus the element id the
/// glue knows it by.
struct Retained {
    id: u32,
    node: DomNode,
    children: Vec<Retained>,
}

/// One retained island: its commands already TRANSLATED to island-
/// local coordinates, plus the logical size. `dirty` = the pixels no
/// longer match — the shell asks for them via `take_dirty_islands`.
struct Island {
    commands: Vec<DrawCommand>,
    width: Px,
    height: Px,
    dirty: bool,
}

/// What the lowering walk threads besides the retention itself: the id
/// well, the pass's display list (islands slice it) and the island
/// registry.
struct LowerCtx<'a> {
    next_id: &'a mut u32,
    display: &'a [DrawCommand],
    islands: &'a mut HashMap<u32, Island>,
}

/// The retained side of the Dom mode: last frame's scene with ids.
/// One per runtime; [`lower`] turns each new scene into patches.
///
/// [`lower`]: DomLowering::lower
#[derive(Default)]
pub struct DomLowering {
    root: Option<Retained>,
    next_id: u32,
    islands: HashMap<u32, Island>,
}

impl DomLowering {
    /// Diffs `scene` against the retained one and returns the patch
    /// list that brings the element tree up to date. The first call
    /// mounts everything. `display` is the SAME pass's draw list —
    /// canvas islands slice their command ranges out of it.
    pub fn lower(
        &mut self,
        scene: &DomNode,
        display: &crate::layout::DisplayList,
    ) -> Vec<DomPatch> {
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
                if scene.style != DomStyle::default() {
                    patches.push(DomPatch::SetStyle { id: 0, style: scene.style.clone() });
                }
                let mut next_id = self.next_id;
                let mut ctx = LowerCtx {
                    next_id: &mut next_id,
                    display: display.as_slice(),
                    islands: &mut self.islands,
                };
                root.children = create_children(scene, 0, &mut ctx, &mut patches);
                self.next_id = next_id;
                self.root = Some(root);
            }
            Some(root) => {
                let mut next_id = self.next_id;
                let mut ctx = LowerCtx {
                    next_id: &mut next_id,
                    display: display.as_slice(),
                    islands: &mut self.islands,
                };
                diff_node(root, scene, &mut ctx, &mut patches);
                self.next_id = next_id;
            }
        }
        patches
    }

    /// The islands whose pixels no longer match, cleared of their flag.
    /// Each returns `(id, logical width, logical height, commands)` —
    /// the caller rasterizes and blits.
    pub(crate) fn take_dirty_islands(&mut self) -> Vec<(u32, Px, Px, Vec<DrawCommand>)> {
        self.islands
            .iter_mut()
            .filter(|(_, island)| island.dirty)
            .map(|(id, island)| {
                island.dirty = false;
                (*id, island.width, island.height, island.commands.clone())
            })
            .collect()
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
        DomKind::Field(field) => {
            if field.multiline {
                CreateKind::Editor
            } else {
                CreateKind::Field
            }
        }
        DomKind::Scroll { .. } => CreateKind::Scroll,
        DomKind::Content => CreateKind::Content,
        DomKind::Canvas { .. } => CreateKind::Canvas,
        DomKind::Image(_) => CreateKind::Image,
        DomKind::Icon(_) => CreateKind::Icon,
    }
}

/// The island's slice of the pass's display list, moved to island-
/// local coordinates (the raster surface starts at zero).
fn island_commands(node: &DomNode, ctx: &LowerCtx) -> Vec<DrawCommand> {
    let DomKind::Canvas { origin, display } = &node.kind else {
        return Vec::new();
    };
    let slice = ctx
        .display
        .get(display.0..display.1)
        .unwrap_or_default();
    let (dx, dy) = (-origin.0, -origin.1);
    let shift = |rect: Rect| Rect {
        origin: Point { x: rect.origin.x + dx, y: rect.origin.y + dy },
        size: rect.size,
    };
    slice
        .iter()
        .cloned()
        .map(|command| match command {
            DrawCommand::FillRect { rect, color, corner_radius } => {
                DrawCommand::FillRect { rect: shift(rect), color, corner_radius }
            }
            DrawCommand::StrokeRect { rect, color, width, corner_radius } => {
                DrawCommand::StrokeRect { rect: shift(rect), color, width, corner_radius }
            }
            DrawCommand::Shadow { rect, radius, color, corner_radius } => {
                DrawCommand::Shadow { rect: shift(rect), radius, color, corner_radius }
            }
            DrawCommand::Backdrop { rect, glass, corner_radius } => DrawCommand::Backdrop {
                rect: shift(rect),
                glass: glass.shifted(dx, dy),
                corner_radius,
            },
            DrawCommand::TextLine { origin, content, range, color, font } => {
                DrawCommand::TextLine {
                    origin: Point { x: origin.x + dx, y: origin.y + dy },
                    content,
                    range,
                    color,
                    font,
                }
            }
            DrawCommand::Gradient { rect, paint, corner_radius } => DrawCommand::Gradient {
                rect: shift(rect),
                paint: paint.shifted(dx, dy),
                corner_radius,
            },
            DrawCommand::Image { rect, source } => {
                DrawCommand::Image { rect: shift(rect), source }
            }
            DrawCommand::PushClip { rect, corner_radius } => {
                DrawCommand::PushClip { rect: shift(rect), corner_radius: corner_radius }
            }
            DrawCommand::PopClip => DrawCommand::PopClip,
        })
        .collect()
}

/// Registers (or refreshes) the island behind a canvas node; the dirty
/// flag rises only when the pixels would actually change.
fn note_island(id: u32, node: &DomNode, ctx: &mut LowerCtx) {
    let commands = island_commands(node, ctx);
    let entry = ctx.islands.entry(id).or_insert(Island {
        commands: Vec::new(),
        width: 0.0,
        height: 0.0,
        dirty: true,
    });
    if entry.commands != commands
        || (entry.width, entry.height) != (node.width, node.height)
    {
        entry.commands = commands;
        entry.width = node.width;
        entry.height = node.height;
        entry.dirty = true;
    }
}

/// Emits the patches that build `node` (already positioned) under
/// `parent` and returns its retained mirror.
fn create_subtree(
    node: &DomNode,
    parent: u32,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) -> Retained {
    let id = *ctx.next_id;
    *ctx.next_id += 1;
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
        DomKind::Canvas { .. } => note_island(id, node, ctx),
        DomKind::Image(image) => {
            patches.push(DomPatch::SetImage { id, image: *image });
        }
        DomKind::Icon(icon) => {
            patches.push(DomPatch::SetIcon { id, icon: *icon });
        }
        _ => {}
    }
    let children = create_children(node, id, ctx, patches);
    Retained { id, node: shallow(node), children }
}

fn create_children(
    node: &DomNode,
    parent: u32,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) -> Vec<Retained> {
    node.children
        .iter()
        .map(|child| create_subtree(child, parent, ctx, patches))
        .collect()
}

/// One remove patch frees the whole subtree on the glue's side; the
/// island registry forgets every canvas underneath.
fn remove_subtree(retained: &Retained, ctx: &mut LowerCtx, patches: &mut Vec<DomPatch>) {
    patches.push(DomPatch::Remove { id: retained.id });
    fn forget_islands(retained: &Retained, islands: &mut HashMap<u32, Island>) {
        if matches!(retained.node.kind, DomKind::Canvas { .. }) {
            islands.remove(&retained.id);
        }
        for child in &retained.children {
            forget_islands(child, islands);
        }
    }
    forget_islands(retained, ctx.islands);
}

/// Diffs one matched pair: geometry, style, kind payload, children.
fn diff_node(
    retained: &mut Retained,
    new: &DomNode,
    ctx: &mut LowerCtx,
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
        (_, DomKind::Canvas { .. }) => note_island(id, new, ctx),
        (DomKind::Image(before), DomKind::Image(after)) if before != after => {
            patches.push(DomPatch::SetImage { id, image: *after });
        }
        (DomKind::Icon(before), DomKind::Icon(after)) if before != after => {
            patches.push(DomPatch::SetIcon { id, icon: *after });
        }
        _ => {}
    }
    retained.node = shallow(new);
    diff_children(retained, new, ctx, patches);
}

/// Matches the children lists: groups by identity path (a slid window
/// keeps its rows), everything else by position and kind. Unmatched old
/// children leave; unmatched new ones mount.
fn diff_children(
    retained: &mut Retained,
    new: &DomNode,
    ctx: &mut LowerCtx,
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
            // a kind change at the same index is remove+create — the
            // mismatched retained goes BACK to its slot so the leftover
            // sweep emits its remove (taking and filtering would drop
            // it silently and leak the element on the browser's side)
            kind => by_index.get_mut(index).and_then(|slot| match slot.take() {
                Some(old)
                    if std::mem::discriminant(&old.node.kind)
                        == std::mem::discriminant(kind) =>
                {
                    Some(old)
                }
                Some(old) => {
                    *slot = Some(old);
                    None
                }
                None => None,
            }),
        };
        match matched {
            Some(mut old) => {
                diff_node(&mut old, child, ctx, patches);
                next.push(old);
            }
            None => next.push(create_subtree(child, retained.id, ctx, patches)),
        }
    }

    for (_, leftover) in by_path {
        remove_subtree(&leftover, ctx, patches);
    }
    for leftover in by_index.into_iter().flatten() {
        remove_subtree(&leftover, ctx, patches);
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
///                                        3 field, 4 scroll, 5 content,
///                                        6 canvas, 7 image, 8 icon,
///                                        9 editor — the field of many
///                                        lines, a `<textarea>`)
///   2 remove        —
///   3 set transform f32 x, f32 y
///   4 set size      f32 w, f32 h
///   5 set style     u32 mask, fields in bit order:
///                   0 background u32 rgba   1 hover u32   2 pressed u32
///                   3 border u32 rgba + f32 width
///                   4 radius f32 (all four corners the same)
///                   5 shadow f32 radius + u32 rgba
///                   6 transition f32 response + f32 damping
///                   7 interactive u16 len + utf8
///                   8 focus border u32 rgba   9 placeholder u32 rgba
///                   10 ink u32 rgba (what the subtree inherits)
///                   11 hover ink u32 rgba     12 pressed ink u32 rgba
///                   13 gradient u8 kind (0 rings, 1 line),
///                      rings: f32 x5 — centre x, centre y (0..1),
///                      start px, end px (negative = the box's reach),
///                      aspect (1 = the circle; else the ellipse's
///                      Y radius is end·aspect);
///                      line: f32 x4 — start x, start y, end x, end y
///                      (0..1) — then u32 near rgba, u32 far rgba
///                   14 clip (no payload — the bit IS the value:
///                      overflow:hidden beside the radius of bit 4)
///                   15 tooltip u16 len + utf8 — a data attribute; the
///                      browser owns the wait and the bubble (a static
///                      CSS rule), the way it owns hover and inputs
///                   16 opacity f32   17 hover opacity   18 pressed
///                   19 group u32 key hi, u32 key lo — the ancestor
///                      whose `:hover` drives bits 1, 2, 11, 12, 17
///                      and 18 of THIS box, as a descendant selector.
///                      The browser still owns the hover, so a group
///                      frame is zero patches
///                   20 group owner u32 hi, u32 lo — the box a
///                      `.hover_group()` owns names itself, and the
///                      followers below point their selectors at it
///                   21 pass through (no payload — the bit IS the
///                      value): `pointer-events:none`, for a layer
///                      that covers a box and must not take its
///                      clicks
///                   22 radii f32 x4 — top left, top right, bottom
///                      right, bottom left. It REPLACES bit 4: a box
///                      sends one number or four, never both, and a
///                      box that rounds all four the same never pays
///                      the three extra floats
///   6 set text      u32 rgba, u8 inherits ink (1 = no color of its
///                   own — the box above owns both states),
///                   f32 size, u8 weight, u8 mono, u8 italic,
///                   u8 truncation (0 none, 1 start, 2 middle, 3 end),
///                   u32 len + utf8, u16 span count,
///                   spans (u32 start, u32 end), u32 span rgba
///   7 set field     u32 rgba text ink, f32 size, u8 weight, u8 mono,
///                   u8 italic,
///                   u32 len + utf8 content, u32 len + utf8 placeholder,
///                   u16 len + utf8 path
///   8 set scroll    f32 x, f32 y
///   9 set image     u32 key hi, u32 key lo, u8 cover — identity as a
///                   NUMBER (the shell's registry maps key → URL)
///  10 set icon      u32 key hi, u32 key lo (the SYMBOL identity, for
///                   the debugger's eyes), u32 ink rgba, u8 inherits
///                   ink (1 = no color of its own — the box above owns
///                   both states), u8 draw count, then per draw:
///                   u8 paint (0 fill, 1 fill even-odd, 2 stroke),
///                   f32 pen width (grid units; 0 for fills),
///                   u8 tinted + u32 rgba when 1 (the draw's OWN
///                   palette — a crab stays orange in any theme),
///                   u32 len + utf8 `d` on the house 24 grid — the
///                   glue's viewBox mirrors that constant, and the
///                   default preserveAspectRatio is the SAME centred
///                   square the rasterizers paint. The tint never
///                   rides the geometry: the `<svg>` draws with
///                   `currentColor`, so hover and press flip through
///                   the box above with no patch of their own
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
                    CreateKind::Canvas => 6,
                    CreateKind::Image => 7,
                    CreateKind::Icon => 8,
                    CreateKind::Editor => 9,
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
                // 1 = take no color of your own; the box above owns it
                out.push(text.inherits_ink as u8);
                push_f32(&mut out, text.font.size);
                out.push(weight_code(text.font.weight));
                out.push(matches!(text.font.design, FontDesign::Mono) as u8);
                out.push(matches!(text.font.slant, crate::text_engine::Slant::Italic) as u8);
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
                push_u32(&mut out, pack_color(field.color));
                push_f32(&mut out, field.font.size);
                out.push(weight_code(field.font.weight));
                out.push(matches!(field.font.design, FontDesign::Mono) as u8);
                out.push(matches!(field.font.slant, crate::text_engine::Slant::Italic) as u8);
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
            DomPatch::SetImage { id, image } => {
                out.push(9);
                push_u32(&mut out, *id);
                push_u32(&mut out, (image.key >> 32) as u32);
                push_u32(&mut out, image.key as u32);
                out.push(image.cover as u8);
            }
            DomPatch::SetIcon { id, icon } => {
                out.push(10);
                push_u32(&mut out, *id);
                push_u32(&mut out, (icon.key >> 32) as u32);
                push_u32(&mut out, icon.key as u32);
                push_u32(&mut out, pack_color(icon.color));
                out.push(icon.inherits_ink as u8);
                let draws = icon.symbol.glyph.draws;
                out.push(draws.len() as u8);
                for draw in draws {
                    let (paint, width) = match draw.paint {
                        crate::icon::Paint::Fill(crate::icon::Rule::NonZero) => (0u8, 0.0f32),
                        crate::icon::Paint::Fill(crate::icon::Rule::EvenOdd) => (1, 0.0),
                        crate::icon::Paint::Stroke { width } => (2, width),
                    };
                    out.push(paint);
                    push_f32(&mut out, width as f64);
                    match draw.tint {
                        Some(tint) => {
                            out.push(1);
                            push_u32(&mut out, pack_color(tint));
                        }
                        None => out.push(0),
                    }
                    push_bytes_u32(&mut out, crate::icon::to_svg_path(draw.path).as_bytes());
                }
            }
        }
    }
    out
}

fn encode_style(out: &mut Vec<u8>, style: &DomStyle) {
    let mut mask: u32 = 0;
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
    // one radius keeps bit 4 and its single float — the wire a box
    // that rounds all four corners has always sent. Four different
    // ones take bit 22 instead, and only they pay for it
    match style.corner_radius.map(|radii| radii.uniform()) {
        Some(Some(_)) => mask |= 1 << 4,
        Some(None) => mask |= 1 << 22,
        None => {}
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
    if style.focus_border.is_some() {
        mask |= 1 << 8;
    }
    if style.placeholder_color.is_some() {
        mask |= 1 << 9;
    }
    if style.color.is_some() {
        mask |= 1 << 10;
    }
    if style.hover_color.is_some() {
        mask |= 1 << 11;
    }
    if style.pressed_color.is_some() {
        mask |= 1 << 12;
    }
    if style.gradient.is_some() {
        mask |= 1 << 13;
    }
    if style.clip {
        // the first payload-free bit of the format: the bit IS the value
        mask |= 1 << 14;
    }
    if style.tooltip.is_some() {
        mask |= 1 << 15;
    }
    if style.opacity.is_some() {
        mask |= 1 << 16;
    }
    if style.hover_opacity.is_some() {
        mask |= 1 << 17;
    }
    if style.pressed_opacity.is_some() {
        mask |= 1 << 18;
    }
    if style.group.is_some() {
        mask |= 1 << 19;
    }
    if style.group_owner.is_some() {
        mask |= 1 << 20;
    }
    if style.pass_through {
        // payload-free, like the clip bit: the bit IS the value
        mask |= 1 << 21;
    }
    push_u32(out, mask);
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
    if let Some(radius) = style.corner_radius.and_then(|radii| radii.uniform()) {
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
    if let Some(color) = style.focus_border {
        push_u32(out, pack_color(color));
    }
    if let Some(color) = style.placeholder_color {
        push_u32(out, pack_color(color));
    }
    if let Some(color) = style.color {
        push_u32(out, pack_color(color));
    }
    if let Some(color) = style.hover_color {
        push_u32(out, pack_color(color));
    }
    if let Some(color) = style.pressed_color {
        push_u32(out, pack_color(color));
    }
    if let Some(gradient) = style.gradient {
        match gradient {
            crate::layout::Gradient::Radial { center, start, end, aspect, inner, outer } => {
                out.push(0);
                push_f32(out, center.x);
                push_f32(out, center.y);
                push_f32(out, start);
                // no reach given = the box's own farthest corner, which
                // CSS spells `farthest-corner`
                push_f32(out, end.unwrap_or(-1.0));
                push_f32(out, aspect);
                push_u32(out, pack_color(inner));
                push_u32(out, pack_color(outer));
            }
            crate::layout::Gradient::Linear { start, end, from, to } => {
                out.push(1);
                push_f32(out, start.x);
                push_f32(out, start.y);
                push_f32(out, end.x);
                push_f32(out, end.y);
                push_u32(out, pack_color(from));
                push_u32(out, pack_color(to));
            }
        }
    }
    if let Some(tooltip) = &style.tooltip {
        push_bytes_u16(out, tooltip.as_bytes());
    }
    if let Some(opacity) = style.opacity {
        push_f32(out, opacity);
    }
    if let Some(opacity) = style.hover_opacity {
        push_f32(out, opacity);
    }
    if let Some(opacity) = style.pressed_opacity {
        push_f32(out, opacity);
    }
    if let Some(group) = style.group {
        push_u32(out, (group >> 32) as u32);
        push_u32(out, group as u32);
    }
    if let Some(owner) = style.group_owner {
        push_u32(out, (owner >> 32) as u32);
        push_u32(out, owner as u32);
    }
    if let Some(radii) = style.corner_radius.filter(|radii| radii.uniform().is_none()) {
        push_f32(out, radii.top_left);
        push_f32(out, radii.top_right);
        push_f32(out, radii.bottom_right);
        push_f32(out, radii.bottom_left);
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
            | DomPatch::SetImage { id, .. }
            | DomPatch::SetIcon { id, .. }
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
        let mount = runtime.dom_frame(&view, size);
        let chrome = mount.iter().any(|patch| {
            matches!(patch, DomPatch::SetStyle { style, .. }
                if style.focus_border.is_some()
                    && style.background.is_some()
                    && style.placeholder_color.is_some())
        });
        assert!(chrome, "the input mounts wearing the theme: {mount:?}");

        assert!(runtime.key(crate::text_input::EditCommand::Insert("x".into())).applied);
        let patches = runtime.dom_frame(&view, size);
        assert_eq!(patches.len(), 1, "the input mirrors the content: {patches:?}");
        match &patches[0] {
            DomPatch::SetField { field, .. } => assert_eq!(field.content.as_ref(), "x"),
            other => panic!("a field patch, not {other:?}"),
        }
    }

    #[test]
    fn a_many_line_field_mounts_as_a_textarea() {
        // two components, so the two trees never share an identity —
        // and neither does anything the reconciler retains under it
        #[derive(Clone)]
        struct Note {
            text: State<String>,
        }
        #[derive(Clone)]
        struct Name {
            text: State<String>,
        }

        impl Component for Note {
            fn body(self, _ctx: &Context) -> impl View {
                text_editor("note", self.text.binding())
            }
        }
        impl Component for Name {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("note", self.text.binding())
            }
        }

        let size = Size { width: 200.0, height: 80.0 };
        fn kind_of(root: &impl View, size: Size) -> CreateKind {
            let runtime = Runtime::new();
            runtime
                .dom_frame(root, size)
                .into_iter()
                .find_map(|patch| match patch {
                    DomPatch::Create { kind, .. } if kind != CreateKind::Group => Some(kind),
                    _ => None,
                })
                .expect("the field mounts")
        }
        // the ELEMENT differs, so the kind does: an input cannot become
        // a textarea in place, and a swapped kind recreates the element
        assert_eq!(kind_of(&Note { text: State::new(String::new()) }, size), CreateKind::Editor);
        assert_eq!(kind_of(&Name { text: State::new(String::new()) }, size), CreateKind::Field);

        // the wire says 9, beside the eight that came before it
        let runtime = Runtime::new();
        let view = Note { text: State::new(String::new()) };
        let bytes = encode(&runtime.dom_frame(&view, size));
        let creates: Vec<u8> = bytes
            .windows(2)
            .filter(|pair| pair[0] == 1)
            .map(|pair| pair[1])
            .collect();
        assert!(!creates.is_empty(), "the stream carries at least one create");
        assert!(bytes.contains(&9), "the editor kind rides the stream: {creates:?}");
    }

    #[test]
    fn border_radius_and_shadow_ride_into_the_scene() {
        #[derive(Clone, Copy)]
        struct Panel;

        impl Component for Panel {
            fn body(self, _ctx: &Context) -> impl View {
                text("chrome")
                    .background_color(Color::hex(0xFFFFFF))
                    .corner_radius(12.0)
                    .border(Color::hex(0xDDDDE2), 1.0)
                    .shadow_color(24.0, Color::hex_a(0x0000_0040))
            }
        }

        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&Panel, Size { width: 200.0, height: 100.0 });
        let style = patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::SetStyle { style, .. } if style.border.is_some() => Some(style),
                _ => None,
            })
            .expect("the panel chrome reached the patches");
        assert_eq!(style.corner_radius, Some(Corners::all(12.0)));
        assert_eq!(style.border, Some((Color::hex(0xDDDDE2), 1.0)));
        assert_eq!(style.shadow, Some((24.0, Color::hex_a(0x0000_0040))));
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
    fn a_hover_ink_lowers_to_inheritance() {
        const FAINT: Color = Color::hex(0x8A8A8A);
        const BRIGHT: Color = Color::hex(0xF5F5F5);

        #[derive(Clone, Copy)]
        struct CloseGlyph;

        impl Component for CloseGlyph {
            fn body(self, _ctx: &Context) -> impl View {
                text("x")
                    .foreground_color(FAINT)
                    .foreground_hovered(BRIGHT)
                    .on_click(|| {})
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 100.0, height: 50.0 };
        let patches = runtime.dom_frame(&CloseGlyph, size);

        // the box declares both inks; the browser owns the swap
        let ink = patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::SetStyle { style, .. } if style.hover_color.is_some() => {
                    Some((style.color, style.hover_color))
                }
                _ => None,
            })
            .expect("the box declares the ink it hands down");
        assert_eq!(ink, (Some(FAINT), Some(BRIGHT)));
        // and the text takes NO color of its own: an inline one would
        // outrank the rule that flips it
        let inherits = patches.iter().any(|patch| {
            matches!(patch, DomPatch::SetText { text, .. } if text.inherits_ink)
        });
        assert!(inherits, "the glyph inherits its ink: {patches:#?}");

        // the LAW still holds: hovering patches nothing
        let target = runtime
            .layout(&CloseGlyph, crate::layout::Proposal::exact(size))
            .hits
            .last()
            .map(|(_, rect)| {
                (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0)
            })
            .expect("the glyph is a target");
        assert!(runtime.pointer_moved(target.0, target.1), "the hover state flipped");
        assert_eq!(runtime.dom_frame(&CloseGlyph, size), vec![], "hover is the browser's");
    }

    #[test]
    fn an_island_mounts_as_one_canvas_and_redraws_only_on_change() {
        #[derive(Clone)]
        struct WithIsland {
            level: State<f64>,
        }

        impl Component for WithIsland {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(
                    text("above the island"),
                    spacer()
                        .frame(20.0, self.level.get())
                        .background_color(Color::hex(0x3B82F6))
                        .rendering(Rendering::Gpu)
                )
            }
        }

        let runtime = Runtime::new();
        let view = WithIsland { level: State::new(10.0) };
        let size = Size { width: 120.0, height: 80.0 };
        let mount = runtime.dom_frame(&view, size);

        let canvases = mount
            .iter()
            .filter(|patch| {
                matches!(patch, DomPatch::Create { kind: CreateKind::Canvas, .. })
            })
            .count();
        assert_eq!(canvases, 1, "one island, one element: {mount:?}");
        // the subtree below the island is PIXELS — no box mounts for it
        let boxes = mount
            .iter()
            .filter(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Box, .. }))
            .count();
        assert_eq!(boxes, 0, "the styled inside drew, never lowered: {mount:?}");

        let islands = runtime.dom_islands(1);
        assert_eq!(islands.len(), 1);
        assert!(
            islands[0].rgba.chunks_exact(4).any(|pixel| pixel[3] > 0),
            "the island has ink"
        );

        // an unchanged frame re-rasters nothing
        let _ = runtime.dom_frame(&view, size);
        assert!(runtime.dom_islands(1).is_empty(), "clean pixels stay put");

        // content change → the island redraws (and only the island)
        view.level.set(40.0);
        let patches = runtime.dom_frame(&view, size);
        assert!(
            patches
                .iter()
                .all(|patch| !matches!(patch, DomPatch::Create { .. } | DomPatch::Remove { .. })),
            "no structure churn on a redraw: {patches:?}"
        );
        assert_eq!(runtime.dom_islands(1).len(), 1, "fresh pixels follow the state");
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
            &(1u32 | 1 << 7).to_le_bytes()[..],
            &0x112233FFu32.to_le_bytes()[..],
            &2u16.to_le_bytes()[..],
            b"go",
            &[2],
            &7u32.to_le_bytes()[..],
        ]
        .concat();
        assert_eq!(bytes, expected);
    }

    // MARK: - Images

    fn tiny_image(seed: u8) -> ImageSource {
        ImageSource::from_bytes(RawImages::encode(2, 2, &[seed; 16]))
    }

    #[derive(Clone)]
    struct Gallery {
        source: State<ImageSource>,
    }

    impl Component for Gallery {
        fn body(self, _ctx: &Context) -> impl View {
            image(self.source.get()).resizable().frame(24.0, 24.0)
        }
    }

    #[test]
    fn an_image_mounts_and_retargets_by_key() {
        let runtime = Runtime::new();
        let view = Gallery { source: State::new(tiny_image(10)) };
        let size = Size { width: 100.0, height: 60.0 };

        let patches = runtime.dom_frame(&view, size);
        let creates = patches
            .iter()
            .filter(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Image, .. }))
            .count();
        assert_eq!(creates, 1, "one element for one image: {patches:?}");
        assert!(
            patches.iter().any(|patch| matches!(
                patch,
                DomPatch::SetImage { image, .. }
                    if image.key == tiny_image(10).key() && !image.cover
            )),
            "the mount dresses the element with the identity: {patches:?}"
        );

        // the same source again: nothing moves
        assert_eq!(runtime.dom_frame(&view, size), vec![]);

        // a new source under the same geometry is ONE image patch
        view.source.set(tiny_image(11));
        let patches = runtime.dom_frame(&view, size);
        assert_eq!(patches.len(), 1, "{patches:?}");
        assert!(matches!(
            &patches[0],
            DomPatch::SetImage { image, .. } if image.key == tiny_image(11).key()
        ));
    }

    #[derive(Clone)]
    struct Sized {
        width: State<f64>,
    }

    impl Component for Sized {
        fn body(self, _ctx: &Context) -> impl View {
            image(tiny_image(10)).resizable().frame(self.width.get(), 24.0)
        }
    }

    #[test]
    fn a_resize_moves_geometry_never_the_image() {
        let runtime = Runtime::new();
        let view = Sized { width: State::new(24.0) };
        let size = Size { width: 100.0, height: 60.0 };
        let _ = runtime.dom_frame(&view, size);

        view.width.set(48.0);
        let patches = runtime.dom_frame(&view, size);
        assert!(!patches.is_empty());
        for patch in &patches {
            assert!(
                matches!(patch, DomPatch::SetSize { .. } | DomPatch::SetTransform { .. }),
                "a resize is geometry only: {patch:?}"
            );
        }
    }

    #[derive(Clone)]
    struct Swaps {
        image_on: State<bool>,
    }

    impl Component for Swaps {
        fn body(self, _ctx: &Context) -> impl View {
            if self.image_on.get() {
                erased(image(tiny_image(10)).resizable().frame(24.0, 24.0))
            } else {
                erased(spacer().frame(24.0, 24.0).background_color(Color::hex(0x334455)))
            }
        }
    }

    #[test]
    fn a_kind_swap_recreates_the_element() {
        let runtime = Runtime::new();
        let view = Swaps { image_on: State::new(true) };
        let size = Size { width: 100.0, height: 60.0 };
        let patches = runtime.dom_frame(&view, size);
        let image_id = patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Image, .. } => Some(*id),
                _ => None,
            })
            .expect("the image mounted");

        view.image_on.set(false);
        let patches = runtime.dom_frame(&view, size);
        assert!(
            patches.iter().any(|patch| matches!(
                patch,
                DomPatch::Remove { id } if *id == image_id
            )),
            "a kind change never mutates in place: {patches:?}"
        );
        assert!(patches
            .iter()
            .any(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Box, .. })));
    }

    #[derive(Clone)]
    struct Isle;

    impl Component for Isle {
        fn body(self, _ctx: &Context) -> impl View {
            image(tiny_image(200))
                .resizable()
                .frame(8.0, 8.0)
                .rendering(Rendering::Gpu)
        }
    }

    #[test]
    fn an_image_inside_an_island_stays_pixels() {
        let runtime = Runtime::new();
        let size = Size { width: 40.0, height: 20.0 };
        let patches = runtime.dom_frame(&Isle, size);
        assert!(
            !patches
                .iter()
                .any(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Image, .. })),
            "the island swallows the element: {patches:?}"
        );
        assert_eq!(
            patches
                .iter()
                .filter(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Canvas, .. }))
                .count(),
            1
        );
        // and the island's pixels carry the image
        let islands = runtime.dom_islands(1);
        assert_eq!(islands.len(), 1);
        assert!(
            islands[0].rgba.chunks(4).any(|pixel| pixel[3] != 0),
            "the image landed in the island's raster"
        );
    }

    // MARK: - Popovers (the portal)

    #[derive(Clone)]
    struct Popped {
        open: State<bool>,
    }

    impl Component for Popped {
        fn body(self, _ctx: &Context) -> impl View {
            crate::vstack!(
                text("base"),
                text("anchor").popover(self.open.binding(), crate::layout::Side::Bottom, |_| {
                    erased(text("tip").background_color(Color::hex(0x334455)))
                }),
            )
        }
    }

    #[test]
    fn a_popover_mounts_under_the_root_and_unmounts_clean() {
        let runtime = Runtime::new();
        let view = Popped { open: State::new(false) };
        let size = Size { width: 200.0, height: 120.0 };
        let mounted: Vec<u32> = runtime
            .dom_frame(&view, size)
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { id, .. } => Some(*id),
                _ => None,
            })
            .collect();

        // opening mounts the popover as a child of the ROOT — the
        // portal: outside every scroll element, last in paint order —
        // and never touches the siblings that were already there
        view.open.set(true);
        let patches = runtime.dom_frame(&view, size);
        let top_level: Vec<(u32, u32)> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { id, parent, .. } => Some((*id, *parent)),
                _ => None,
            })
            .collect();
        assert!(!top_level.is_empty(), "the popover mounted: {patches:?}");
        assert_eq!(top_level[0].1, 0, "the first created node hangs off the root");
        let fresh: Vec<u32> = top_level.iter().map(|(id, _)| *id).collect();
        for patch in &patches {
            assert!(
                !mounted.contains(&patch_id(patch)) || fresh.contains(&patch_id(patch)),
                "an old sibling moved on open: {patch:?}"
            );
        }

        // closing removes the subtree and, again, nothing else
        view.open.set(false);
        let patches = runtime.dom_frame(&view, size);
        assert!(
            patches
                .iter()
                .all(|patch| matches!(patch, DomPatch::Remove { id } if fresh.contains(id))),
            "closing is removal only: {patches:?}"
        );
        assert!(!patches.is_empty());
    }

    #[test]
    fn a_gradient_reaches_the_browser_as_a_style() {
        #[derive(Clone)]
        struct Glow;
        impl Component for Glow {
            fn body(self, _ctx: &Context) -> impl View {
                use crate::layout::{Gradient, UnitPoint};
                let violet = Color::hex(0x8B5CF6);
                spacer().frame(80.0, 40.0).background_color(Color::hex(0x101014)).background_gradient(
                    Gradient::radial(violet, violet.fade())
                        .center(UnitPoint::TOP)
                        .radius(0.0, 120.0),
                )
            }
        }
        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&Glow, Size { width: 100.0, height: 60.0 });
        let style = patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::SetStyle { style, .. } if style.gradient.is_some() => Some(style),
                _ => None,
            })
            .expect("the ramp travels as style, not as pixels");
        assert!(style.background.is_some(), "the flat color rides along under it");
        match style.gradient.expect("a gradient") {
            crate::layout::Gradient::Radial { center, start, end, .. } => {
                assert_eq!(center.y, 0.0, "anchored to the top edge");
                assert_eq!((start, end), (0.0, Some(120.0)));
            }
            other => panic!("{other:?}"),
        }
        // the mask bit and the payload are the wire contract
        let bytes = encode(&[patches
            .iter()
            .find(|patch| matches!(patch, DomPatch::SetStyle { style, .. } if style.gradient.is_some()))
            .cloned()
            .expect("the style patch")]);
        let mask = u16::from_le_bytes([bytes[9], bytes[10]]);
        assert_eq!(mask & (1 << 13), 1 << 13, "bit 13 says a ramp follows");
    }

    #[test]
    fn the_image_encoding_is_byte_stable() {
        let bytes = encode(&[DomPatch::SetImage {
            id: 7,
            image: DomImage { key: 0x1122_3344_5566_7788, cover: true },
        }]);
        let expected: Vec<u8> = [
            &1u32.to_le_bytes()[..],
            &[9],
            &7u32.to_le_bytes()[..],
            &0x1122_3344u32.to_le_bytes()[..],
            &0x5566_7788u32.to_le_bytes()[..],
            &[1],
        ]
        .concat();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn a_clipped_box_sets_the_overflow_bit_and_nothing_else() {
        let bare = DomStyle {
            background: Some(Color::hex(0x123456)),
            corner_radius: Some(Corners::all(6.0)),
            ..DomStyle::default()
        };
        let cut = DomStyle { clip: true, ..bare.clone() };
        let without = encode(&[DomPatch::SetStyle { id: 3, style: bare }]);
        let with = encode(&[DomPatch::SetStyle { id: 3, style: cut }]);
        // the first payload-free bit: the streams differ by ONE bit in
        // the mask's high byte and nothing else
        assert_eq!(with.len(), without.len(), "the bit carries no payload");
        let mask = u16::from_le_bytes([with[9], with[10]]);
        assert_eq!(mask & (1 << 14), 1 << 14, "bit 14 says the overflow hides");
        let mut expected = without.clone();
        expected[10] |= 0x40;
        assert_eq!(with, expected);
    }

    #[test]
    fn four_corners_take_their_own_bit_and_leave_the_one_radius_alone() {
        // one radius keeps bit 4 and its single float — the wire a box
        // that rounds all four has always sent
        let one = DomStyle {
            corner_radius: Some(Corners::all(6.0)),
            ..DomStyle::default()
        };
        let bytes = encode(&[DomPatch::SetStyle { id: 3, style: one }]);
        let mask = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
        assert_eq!(mask, 1 << 4, "the one radius is bit 4, alone");
        assert_eq!(bytes.len(), 13 + 4, "and it costs one float");

        // four different ones take bit 22 INSTEAD, with the four in
        // CSS order behind it
        let four = DomStyle {
            corner_radius: Some(Corners {
                top_left: 1.0,
                top_right: 2.0,
                bottom_right: 3.0,
                bottom_left: 4.0,
            }),
            ..DomStyle::default()
        };
        let bytes = encode(&[DomPatch::SetStyle { id: 3, style: four }]);
        let mask = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
        assert_eq!(mask, 1 << 22, "four radii take bit 22, and bit 4 stays clear");
        let radii: Vec<f32> = bytes[13..]
            .chunks_exact(4)
            .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        assert_eq!(radii, vec![1.0, 2.0, 3.0, 4.0]);
    }

    const MARK_PATH: &[crate::icon::Verb] = &[
        crate::icon::Verb::Move(4.0, 12.0),
        crate::icon::Verb::Line(10.0, 18.0),
        crate::icon::Verb::Line(20.0, 6.0),
    ];
    const MARK_GLYPH: crate::icon::Glyph = crate::icon::Glyph {
        draws: &[crate::icon::Draw {
            paint: crate::icon::Paint::Stroke { width: 2.0 },
            path: MARK_PATH, tint: None,
        }],
    };
    const MARK: crate::icon::Symbol = crate::icon::Symbol::new("test.mark", &MARK_GLYPH);

    #[test]
    fn the_icon_encoding_is_byte_stable() {
        let icon = DomIcon {
            key: MARK.key,
            symbol: MARK,
            color: Color::hex(0x8A94A6),
            inherits_ink: false,
        };
        let bytes = encode(&[DomPatch::SetIcon { id: 7, icon }]);
        let expected: Vec<u8> = [
            &1u32.to_le_bytes()[..],
            &[10],
            &7u32.to_le_bytes()[..],
            &((MARK.key >> 32) as u32).to_le_bytes()[..],
            &(MARK.key as u32).to_le_bytes()[..],
            &0x8A94_A6FFu32.to_le_bytes()[..],
            &[0],
            &[1],
            &[2],
            &2.0f32.to_le_bytes()[..],
            &[0],
            &16u32.to_le_bytes()[..],
            b"M4 12L10 18L20 6",
        ]
        .concat();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn an_icon_under_a_hover_ink_takes_no_color_of_its_own() {
        const FAINT: Color = Color::hex(0x8A8A8A);
        const BRIGHT: Color = Color::hex(0xF5F5F5);

        #[derive(Clone, Copy)]
        struct CloseButton;

        impl Component for CloseButton {
            fn body(self, _ctx: &Context) -> impl View {
                icon(MARK)
                    .foreground_color(FAINT)
                    .foreground_hovered(BRIGHT)
                    .on_click(|| {})
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 100.0, height: 50.0 };
        let patches = runtime.dom_frame(&CloseButton, size);

        // the box declares both inks; the browser owns the swap
        let ink = patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::SetStyle { style, .. } if style.hover_color.is_some() => {
                    Some((style.color, style.hover_color))
                }
                _ => None,
            })
            .expect("the box declares the ink it hands down");
        assert_eq!(ink, (Some(FAINT), Some(BRIGHT)));
        // and the glyph takes NO color of its own — currentColor
        // inherits through, exactly the law the text keeps
        let inherits = patches.iter().any(|patch| {
            matches!(patch, DomPatch::SetIcon { icon, .. } if icon.inherits_ink)
        });
        assert!(inherits, "the glyph inherits its ink: {patches:#?}");

        // the LAW still holds: hovering patches nothing
        let target = runtime
            .layout(&CloseButton, crate::layout::Proposal::exact(size))
            .hits
            .last()
            .map(|(_, rect)| {
                (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0)
            })
            .expect("the glyph is a target");
        assert!(runtime.pointer_moved(target.0, target.1), "the hover state flipped");
        assert_eq!(runtime.dom_frame(&CloseButton, size), vec![], "hover is the browser's");
    }

    #[test]
    fn a_new_tint_is_one_icon_patch() {
        #[derive(Clone)]
        struct Tinted {
            ink: State<Color>,
        }

        impl Component for Tinted {
            fn body(self, _ctx: &Context) -> impl View {
                icon(MARK).foreground_color(self.ink.get())
            }
        }

        let runtime = Runtime::new();
        let view = Tinted { ink: State::new(Color::hex(0x333333)) };
        let size = Size { width: 60.0, height: 40.0 };
        let mount = runtime.dom_frame(&view, size);
        let creates = mount
            .iter()
            .filter(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Icon, .. }))
            .count();
        assert_eq!(creates, 1, "one element for one glyph: {mount:?}");

        // the same scene again: nothing moves
        assert_eq!(runtime.dom_frame(&view, size), vec![]);

        // a re-tint is ONE icon patch — the geometry never re-travels
        // in a style, and no other element hears about it
        view.ink.set(Color::hex(0xAA2211));
        let patches = runtime.dom_frame(&view, size);
        assert_eq!(patches.len(), 1, "{patches:#?}");
        assert!(matches!(
            &patches[0],
            DomPatch::SetIcon { icon, .. }
                if icon.color == Color::hex(0xAA2211) && !icon.inherits_ink
        ));
    }
}
