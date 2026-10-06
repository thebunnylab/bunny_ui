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
    Group { path: std::rc::Rc<str> },
    /// A styled box (background, border, radius, shadow, interaction).
    Box,
    /// One run of text — the browser renders and selects it natively.
    Text(DomText),
    /// A native `<input>` — the browser owns the editing. Boxed: the
    /// widest record of the set and among the rarest, it would size
    /// every node of the scene by itself.
    Field(Box<DomField>),
    /// A scroll viewport; `offset` is ours, the element mirrors it.
    Scroll {
        path: Option<String>,
        offset: (Px, Px),
        /// The item id the region follows (`.scroll_target`/.reveal) —
        /// the DIFF turns a CHANGE here into a Reveal (dense) or a
        /// SetScroll at the row's slot (virtual).
        target: Option<String>,
    },
    /// The sized content inside a scroll — the extent the browser
    /// scrolls through (a virtual list sizes it to ALL rows).
    Content,
    /// A canvas island (`.rendering(Gpu)`): our layout positions the
    /// element; the subtree's draw commands fill it. `origin` is the
    /// island's ABSOLUTE frame origin (the commands translate by it)
    /// and `display` the `[start, end)` range into the pass's list.
    Canvas {
        origin: (Px, Px),
        display: (usize, usize),
        /// The island's identity — a flexible island's real box comes
        /// back from the browser keyed by it.
        path: Option<std::rc::Rc<str>>,
    },
    /// An `<img>` — the browser fetches, decodes and paints it. The
    /// record carries the IDENTITY; the shell's registry maps it to a
    /// URL the browser can load.
    Image(DomImage),
    /// An `<svg>` — a vector glyph rendered AT HOME: the browser
    /// scales the drawing, and the tint is `currentColor`, so hover
    /// and press flip through the box above with no patch of their
    /// own.
    Icon(DomIcon),
    /// An `<iframe>` — the native host's web lowering: the browser's
    /// own island, holding a page the way the desktop shells hold the
    /// OS webview. The island contract is the one the DOM already
    /// enforces; a changed `src` navigates, it never re-mounts.
    /// `src` is the url — or, `sealed`, the DOCUMENT itself: a page
    /// from memory under its network policy, held as `srcdoc` inside
    /// the browser's sandbox with no powers (`docs/webview.md`).
    Iframe { src: std::rc::Rc<str>, sealed: bool },
    /// A `<video>` — the video host's web lowering: the browser's own
    /// element playing a media stream the PAGE owns, named by the
    /// handle the glue's registry answered (`stream`; zero names no
    /// stream). The browser decodes and composites it; no pixel
    /// crosses the border in either direction. The element is wired to
    /// its stream once — a changed handle rewires it, a changed flag
    /// alone never restarts the playback (`docs/video.md`).
    Video { stream: u32, mirrored: bool, cover: bool, radius: f32 },
    /// A flow container: `display:flex; flex-direction:column`. Two
    /// variants instead of a payload so the keyed match's discriminant
    /// tells the axes apart — an axis change recreates the element.
    FlexColumn,
    /// The row twin: `display:flex; flex-direction:row`.
    FlexRow,
    /// Layered children: `display:grid`, everyone in the same cell.
    Layers,
    /// A CLEAN boundary, by promise: no body under this path ran this
    /// frame, so the retained subtree still holds — the diff keeps it
    /// wholesale and never descends. Internal to the walk and the
    /// diff; the wire never carries it.
    Reuse { path: std::rc::Rc<str> },
    /// A popover under the root (the portal). The glue positions it
    /// from the anchor's real box — the identity is the overlay path.
    Popover {
        path: String,
        /// The anchor's identity: a Group the walk wraps around the
        /// anchored child (`{path}/#anchor`). The diff resolves it to
        /// an element id and ships the relation as one patch.
        anchor: String,
        /// 0 top, 1 bottom, 2 leading, 3 trailing.
        side: u8,
    },
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
    /// The drawing reads as a MASK: no draw carries its own colour to
    /// the browser, so every path takes the element's ink.
    pub forced: bool,
}

/// The visual record of a node — everything CSS will say about it, in
/// the three parts a page tells apart. The action path is the
/// element's own and rides inline: a row's every link carries one. The
/// LOOK is what a rule shares, and the MARKS (a tooltip, the hover
/// group a box owns) are the element's own and rare: both are boxed,
/// and held as none when they say nothing — a flow scene's cells,
/// links and stacks mostly wear the default look, and a default style
/// is three words that allocate nothing.
#[derive(Clone, Debug, Default)]
pub struct DomStyle {
    /// The action path of the enclosing `Interactive` — the glue posts
    /// clicks back with it, and `:hover`/`:active` scope to it.
    pub interactive: Option<std::rc::Rc<str>>,
    /// `None` = the look that paints nothing ([`DomLook::NONE`]).
    look: Option<Box<DomLook>>,
    /// `None` = no marks.
    marks: Option<Box<DomMarks>>,
}

/// Equal by what the parts SAY: a look boxed at its default reads the
/// same as none.
impl PartialEq for DomStyle {
    fn eq(&self, other: &DomStyle) -> bool {
        self.interactive == other.interactive
            && same_look(self.look(), other.look())
            && self.marks() == other.marks()
    }
}

/// The look nobody boxed.
static NO_LOOK: DomLook = DomLook::NONE;
/// The marks nobody boxed.
static NO_MARKS: DomMarks = DomMarks { tooltip: None, group_owner: None };

impl DomStyle {
    /// A style of its three parts; a default look and empty marks are
    /// held as none.
    pub fn new(interactive: Option<std::rc::Rc<str>>, look: DomLook, marks: DomMarks) -> DomStyle {
        let mut style = DomStyle { interactive, look: None, marks: None };
        style.set_look(look);
        if marks != NO_MARKS {
            style.marks = Some(Box::new(marks));
        }
        style
    }

    /// A style that is a look alone: no action path, no marks.
    pub fn of_look(look: DomLook) -> DomStyle {
        DomStyle::new(None, look, DomMarks::default())
    }

    /// The look — [`DomLook::NONE`] when the style paints nothing.
    pub fn look(&self) -> &DomLook {
        self.look.as_deref().unwrap_or(&NO_LOOK)
    }

    /// The look, to change in place: a style that painted nothing takes
    /// a record of its own here.
    pub fn look_mut(&mut self) -> &mut DomLook {
        self.look.get_or_insert_with(Box::default)
    }

    /// Replaces the look; the default one is held as none.
    pub fn set_look(&mut self, look: DomLook) {
        self.look = (look != NO_LOOK).then(|| Box::new(look));
    }

    /// The transition of the enclosing animation scope — a look's.
    /// Setting none on a style that paints nothing boxes nothing.
    pub fn set_transition(&mut self, transition: Option<(f64, f64)>) {
        if transition.is_some() || self.look.is_some() {
            self.look_mut().transition = transition;
        }
    }

    pub fn take_transition(&mut self) -> Option<(f64, f64)> {
        self.look.as_mut().and_then(|look| look.transition.take())
    }

    /// The element's own marks — empty when it has none.
    pub fn marks(&self) -> &DomMarks {
        self.marks.as_deref().unwrap_or(&NO_MARKS)
    }

    /// Does the element carry a mark of its own?
    pub fn has_marks(&self) -> bool {
        *self.marks() != NO_MARKS
    }

    pub fn tooltip(&self) -> Option<&Arc<str>> {
        self.marks().tooltip.as_ref()
    }

    pub fn group_owner(&self) -> Option<u64> {
        self.marks().group_owner
    }

    /// Setting none on an element without marks boxes nothing.
    pub fn set_tooltip(&mut self, tooltip: Option<Arc<str>>) {
        if tooltip.is_some() || self.marks.is_some() {
            self.marks.get_or_insert_with(Box::default).tooltip = tooltip;
            self.settle_marks();
        }
    }

    pub fn take_tooltip(&mut self) -> Option<Arc<str>> {
        let tooltip = self.marks.as_mut().and_then(|marks| marks.tooltip.take());
        self.settle_marks();
        tooltip
    }

    pub fn set_group_owner(&mut self, owner: Option<u64>) {
        if owner.is_some() || self.marks.is_some() {
            self.marks.get_or_insert_with(Box::default).group_owner = owner;
            self.settle_marks();
        }
    }

    /// Marks emptied are held as none again.
    fn settle_marks(&mut self) {
        if self.marks.as_deref() == Some(&NO_MARKS) {
            self.marks = None;
        }
    }

    /// No action path, the look that paints nothing, no marks.
    pub fn is_default(&self) -> bool {
        self.interactive.is_none() && *self.look() == NO_LOOK && !self.has_marks()
    }
}

/// Two looks, equal? The same record — the unboxed default on both
/// sides, most of a scene — answers without reading a field.
fn same_look(a: &DomLook, b: &DomLook) -> bool {
    std::ptr::eq(a, b) || a == b
}

/// The part of a style a rule shares: everything CSS says about the
/// element but its action path and its marks. Hover and pressed live
/// HERE as alternatives, never resolved: the scene is pointer-invariant.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DomLook {
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
    /// The liquid-glass material, as much of it as a browser owns:
    /// `backdrop-filter` gives the blur, the saturation and the
    /// brightness natively, and the rim goes on as two inset shadows
    /// along the lit diagonals.
    ///
    /// Two parts of the material stay behind in this mode: the LENS
    /// (the rim's refraction, and the fringe with it) and the touch
    /// lights. CSS has no displacement map, and the promise of the
    /// element lowering was never pixels — it is the geometry, with
    /// native text and native controls. A subtree that needs the whole
    /// material asks for `.rendering(Gpu)` and gets it exactly.
    ///
    /// The TINT does not travel here: it is composited into
    /// `background` at capture time, where both colours are known,
    /// because an element has one background colour and the tint sits
    /// directly under whatever the box paints itself.
    pub glass: Option<GlassFilter>,
    /// Inside a `.overlay(…)`/`.background(…)` layer that asks for
    /// nothing: the box lets the pointer THROUGH to what it covers. A
    /// rule or an insertion marker must not eat the click that belongs
    /// to the row underneath, and in this mode the browser routes by
    /// element and not by our hit list.
    pub pass_through: bool,
}

impl DomLook {
    /// The look that paints nothing — what a style with no look of its
    /// own wears.
    pub const NONE: DomLook = DomLook {
        background: None,
        gradient: None,
        hover_background: None,
        pressed_background: None,
        color: None,
        hover_color: None,
        pressed_color: None,
        border: None,
        corner_radius: None,
        shadow: None,
        transition: None,
        focus_border: None,
        placeholder_color: None,
        clip: false,
        opacity: None,
        hover_opacity: None,
        pressed_opacity: None,
        group: None,
        glass: None,
        pass_through: false,
    };

    pub(crate) fn from_props(props: &VisualProps) -> DomLook {
        DomLook {
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
            transition: None,
            focus_border: None,
            placeholder_color: None,
            clip: props.clip,
            opacity: props.opacity,
            hover_opacity: props.opacity_hovered,
            pressed_opacity: props.opacity_pressed,
            group: None,
            glass: props.glass.map(GlassFilter::of),
            pass_through: false,
        }
    }

    /// Do these props paint anything — would [`DomLook::from_props`]
    /// say more than [`DomLook::NONE`]? Read prop by prop, the same
    /// props `from_props` reads and no other, so the walk that asks it
    /// of every styled text builds no look just to compare it.
    pub(crate) fn paints(props: &VisualProps) -> bool {
        let paints = props.background.is_some()
            || props.gradient.is_some()
            || props.background_hovered.is_some()
            || props.background_pressed.is_some()
            || props.foreground_hovered.is_some()
            || props.foreground_pressed.is_some()
            || props.border.is_some()
            || props.corner_radius.is_some()
            || props.shadow.is_some()
            || props.clip
            || props.opacity.is_some()
            || props.opacity_hovered.is_some()
            || props.opacity_pressed.is_some()
            || props.glass.is_some();
        debug_assert_eq!(
            paints,
            DomLook::from_props(props) != DomLook::NONE,
            "a prop from_props reads is missing here"
        );
        paints
    }
}

/// The element's own marks — never a look's: what one element says
/// about itself, attributes on it alone.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DomMarks {
    /// `.tooltip(…)` — in THIS mode the browser owns the wait and the
    /// bubble (a CSS rule on a data attribute), the way it owns the
    /// hover and the inputs: zero patches by construction. The pixel
    /// modes run the engine's own bubble instead.
    pub tooltip: Option<Arc<str>>,
    /// The box a `.hover_group()` owns names itself here — the anchor
    /// every follower's selector points at.
    pub group_owner: Option<u64>,
}

/// What a browser can carry of a pane of glass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassFilter {
    /// The `backdrop-filter` blur, in logical px. CSS takes a standard
    /// deviation here, which is the same number the material means.
    pub blur: Px,
    pub saturation: f64,
    pub brightness: f64,
    /// The specular rim: its colour and its band.
    pub rim: Color,
    pub rim_band: Px,
}

impl GlassFilter {
    fn of(glass: crate::layout::Glass) -> GlassFilter {
        // the box is not known here and the filter needs none of it —
        // only the spot resolves against a frame, and the spot is one
        // of the two things this mode leaves behind
        let resolved = glass.resolve(Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: crate::layout::Size { width: 0.0, height: 0.0 },
        });
        GlassFilter {
            blur: resolved.blur,
            saturation: resolved.saturation,
            brightness: resolved.brightness,
            rim: Color {
                a: (resolved.highlight.a as f64 * resolved.highlight_intensity.clamp(0.0, 1.0))
                    .round() as u8,
                ..resolved.highlight
            },
            rim_band: resolved.highlight_band,
        }
    }

    /// The colour an element paints, once the tint is folded in: the
    /// tint sits under the box's own background, so the background wins
    /// where it is opaque and the tint shows through where it is not.
    pub(crate) fn under(tint: Color, background: Option<Color>) -> Option<Color> {
        let Some(background) = background else { return Some(tint) };
        let over = background.a as f64 / 255.0;
        let channel = |top: u8, under: u8| {
            (top as f64 * over + under as f64 * (1.0 - over)).round() as u8
        };
        let alpha = over + (tint.a as f64 / 255.0) * (1.0 - over);
        Some(Color {
            r: channel(background.r, tint.r),
            g: channel(background.g, tint.g),
            b: channel(background.b, tint.b),
            a: (alpha * 255.0).round() as u8,
        })
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
    /// The line box, when `.line_height(…)` set one — the browser steps
    /// its own lines by it, so the element wraps at the same rhythm the
    /// engine measured. `None` leaves the face's own box. `f32`, the
    /// wire's precision.
    pub line_height: Option<f32>,
    /// Where each wrapped line sits in the box — `None` is leading, the
    /// browser's own default for our writing direction.
    pub text_align: Option<motor::views::TextAlignment>,
    /// Match highlight spans (byte ranges) + their color.
    pub highlights: Option<(Rc<Vec<(usize, usize)>>, Color)>,
    pub truncation: Option<Truncation>,
    /// The face is the one declared above: the text names no font of
    /// its own, and the look it wears carries none.
    pub inherits_face: bool,
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
    /// The field must not show what it holds: the glue builds
    /// `type="password"`, and the browser masks, refuses the copy and
    /// keeps its own caret — the platform's own job, done natively.
    pub secret: bool,
}

/// What a node reads for itself ([`crate::bind`]): the lowering keeps
/// the element id by the binding's key and patches the one element
/// when a write reaches the binding — no walk, no diff.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeBinding {
    /// The text of a text node.
    Text(Rc<crate::bind::Bound<Arc<str>>>),
    /// The class of a group's element (`boundary_class_with`).
    Class(Rc<crate::bind::Bound<String>>),
}

impl NodeBinding {
    /// The key the lowering files the element under.
    pub fn key(&self) -> &Rc<str> {
        match self {
            NodeBinding::Text(bound) => bound.key(),
            NodeBinding::Class(bound) => bound.key(),
        }
    }

    /// The same binding OBJECT — not merely the same key: a body that
    /// re-ran made a new one at the old key, and it may read something
    /// else.
    fn same_as(&self, other: &NodeBinding) -> bool {
        match (self, other) {
            (NodeBinding::Text(a), NodeBinding::Text(b)) => Rc::ptr_eq(a, b),
            (NodeBinding::Class(a), NodeBinding::Class(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Are two nodes driven by the same binding object?
fn same_binding(old: &DomNode, new: &DomNode) -> bool {
    match (&old.binding, &new.binding) {
        (Some(was), Some(now)) => was.same_as(now),
        _ => false,
    }
}

/// Did the hints move, the class of a bound node aside? A class that
/// reads for itself travels on its own road.
fn hints_changed(old: &DomNode, new: &DomNode) -> bool {
    if same_binding(old, new) && matches!(new.binding, Some(NodeBinding::Class(_))) {
        // the class aside, field by field: a copy of the hints with the
        // old class in it was three shared words taken and let go for
        // every kept row of a list
        old.hints.tag != new.hints.tag || old.hints.address != new.hints.address
    } else {
        old.hints != new.hints
    }
}

/// A captured scene node: kind + parent-relative frame + style +
/// children, exactly what one element needs to exist.
#[derive(Clone, Debug, PartialEq)]
pub struct DomNode {
    pub kind: DomKind,
    /// Offset from the parent NODE's origin (logical px). Owned by
    /// the ABSOLUTE lowering; a flow node leaves all four at zero and
    /// speaks through `layout`. `f64`, where the flow record holds
    /// the wire's `f32`: the served page prints the engine's own
    /// number for a placed element, and the island ledger sizes the
    /// pixels by it.
    pub x: Px,
    pub y: Px,
    pub width: Px,
    pub height: Px,
    pub style: DomStyle,
    /// `Some` = this node lives in the FLOW: the browser lays it out
    /// from these semantics and the geometry fields above stay silent.
    pub layout: Option<DomLayout>,
    /// Real-element hints (tag, class, id) — the Dom's alone.
    pub hints: DomHints,
    pub children: Vec<DomNode>,
    /// What the node reads for itself, if anything.
    pub binding: Option<NodeBinding>,
    /// The face this element declares for everything under it: the
    /// root's default, or a box whose modifiers changed the face. A
    /// text with the declared face inherits it and declares none of
    /// its own — a thousand cells share the one declaration above
    /// them, as a page's own stylesheet would have it. Shared: the
    /// boxes a walk declares one face on hold one record of it.
    pub face: Option<Rc<FontSpec>>,
    /// The element id the glue knows the node by, once the lowering
    /// keeps it — zero on a scene node, and on the root, the mount
    /// point. The retained tree is the scene's own nodes: a node the
    /// diff creates or keeps moves into it as it is, its id and its
    /// rule written in place and its children left in the vector they
    /// came in.
    pub(crate) id: u32,
    /// The look the element wears — the rule's hash; zero on a scene
    /// node.
    pub(crate) rule: u64,
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
    pending_interactive: Option<std::rc::Rc<str>>,
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
            style: DomStyle::of_look(DomLook {
                background: Some(crate::theme::current().canvas),
                ..DomLook::default()
            }),
            layout: None,
            hints: DomHints::default(),
            children: Vec::new(),
            binding: None,
            face: None,
            id: 0,
            rule: 0,
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
        crate::stats::note_capture_node();
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
        if self.overlay_depth > 0 && style.interactive.is_none() {
            style.look_mut().pass_through = true;
        }
        style.set_transition(self.pending_transition.take());
        style.set_tooltip(self.armed_tooltip.take());
        let node = DomNode {
            kind,
            x: frame.origin.x - parent_origin.x,
            y: frame.origin.y - parent_origin.y,
            width: frame.size.width,
            height: frame.size.height,
            style,
            layout: None,
            hints: DomHints::default(),
            children: Vec::new(),
            binding: None,
            face: None,
            id: 0,
            rule: 0,
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
        let tooltip = node.style.take_tooltip();
        let mut look = DomLook {
            // the rebuild must not drop what the layer scope decided
            pass_through: self.overlay_depth > 0 && interactive.is_none(),
            transition,
            group: props.from_group.then(|| group).flatten(),
            ..DomLook::from_props(props)
        };
        // the tint has no layer of its own in a browser: it folds into
        // the background, where it belongs — under whatever the box
        // paints itself and over the blurred backdrop
        if let Some(glass) = props.glass {
            let tint = glass.resolve(frame).tint;
            look.background = GlassFilter::under(tint, look.background);
            look.hover_background =
                look.hover_background.and_then(|color| GlassFilter::under(tint, Some(color)));
            look.pressed_background =
                look.pressed_background.and_then(|color| GlassFilter::under(tint, Some(color)));
        }
        if states || inheriting {
            look.color = Some(ink);
        }
        node.style = DomStyle::new(interactive, look, DomMarks { tooltip, group_owner: None });
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
        node.style.look_mut().background = Some(color);
    }

    /// Strokes the OPEN node's border (the stub leaves).
    pub(crate) fn set_border(&mut self, color: Color, width: Px) {
        if self.island > 0 {
            return;
        }
        let (_, node) = self.stack.last_mut().expect("an open node");
        node.style.look_mut().border = Some((color, width));
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

    /// Arms a `.tooltip(…)` for the NEXT opened node — the placement
    /// calls it just before the wrapped child places.
    pub(crate) fn arm_tooltip(&mut self, text: Arc<str>) {
        if self.island == 0 {
            self.armed_tooltip = Some(text);
        }
    }

    /// A childless element — open and close in one move.
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

    /// A childless element that carries its own look (the field's
    /// theme chrome travels in the record, never hardcoded in a glue).
    pub(crate) fn leaf_styled(&mut self, kind: DomKind, frame: Rect, look: DomLook) {
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
        // the look names no action of its own: inside a layer it lets
        // the pointer through
        let pass_through = self.overlay_depth > 0;
        let (_, node) = self.stack.last_mut().expect("just opened");
        node.style = DomStyle::of_look(DomLook { pass_through, ..look });
        self.close();
    }

    pub(crate) fn arm_transition(&mut self, response: f64, damping: f64) {
        self.pending_transition = Some((response, damping));
    }

    pub(crate) fn arm_interactive(&mut self, path: &str) {
        self.pending_interactive = Some(std::rc::Rc::from(path));
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
        node.style.set_group_owner(Some(key));
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
                path: None,
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

// MARK: - The flow records

/// The FLOW record: what a flow node tells the browser about layout —
/// semantics, never coordinates. `None` on a node means the absolute
/// lowering owns its geometry (today's whole scene; tomorrow only a
/// `.layout(Exact)` interior). The wire twin of [`DomStyle`]: a full
/// replace, one write per changed node.
///
/// The numbers are `f32`, the precision the wire carries them at: a
/// record half the size of one in `f64`, and nothing the browser could
/// tell apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DomLayout {
    /// Gap between a flex container's children, px.
    pub gap: Option<f32>,
    /// Cross-axis alignment: 0 start, 1 center, 2 end, 3 baseline.
    pub align: Option<u8>,
    /// Padding `(top, trailing, bottom, leading)`, px — logical sides, so
    /// the page writes them as `padding-block` and `padding-inline` and
    /// a right-to-left mount puts the leading inset on the right.
    pub padding: Option<(f32, f32, f32, f32)>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub max_width: Option<f32>,
    pub max_height: Option<f32>,
    /// The flexible child: `flex:1 1 0` and a zeroed min-size.
    pub grow: bool,
    /// A virtual row's absolute offset inside its content box, px.
    pub slot_y: Option<f32>,
    /// The child follows its container's cross size — `align-self:
    /// stretch`, and no pinned size on the stretched axis.
    pub stretch: bool,
    /// The child takes the container's OFFER and keeps its content
    /// floor — `flex: 1 1 auto`. A wrapper's proposal semantics: the
    /// interior fills a definite box, and an auto box still sizes to
    /// the content instead of collapsing to a zero basis.
    pub fill: bool,
    /// The row WRAPS, with this gap between its lines, px — `flex-wrap:
    /// wrap` and `row-gap`, a flow's lowering.
    pub wrap: Option<f32>,
    /// The element is no flex box: an inline tag around ONE child (a
    /// link around a word) keeps the browser's own display for the
    /// tag, where a flex line per row would be a layout per row.
    pub plain: bool,
    /// The subtree reads the OTHER way from what surrounds it — an
    /// island an `.environment(…)` turned: `direction` and an isolated
    /// `unicode-bidi`, so the browser orders its rows, aligns its
    /// `start` and shapes its words the island's way. `None` inherits.
    pub direction: Option<motor::state::LayoutDirection>,
}

/// Element hints only the Dom consumes — a real tag, a class, an id.
/// Every other lowering ignores them, like `.rendering(Gpu)` on a
/// pixel target. Empty on everything the engine makes by itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DomHints {
    pub tag: Option<std::rc::Rc<str>>,
    pub class: Option<std::rc::Rc<str>>,
    /// The id and, for a link, the href — rare, so shared behind one word.
    pub address: Option<std::rc::Rc<crate::layout::Address>>,
}

impl DomHints {
    pub fn is_empty(&self) -> bool {
        self.tag.is_none() && self.class.is_none() && self.address.is_none()
    }

    pub fn dom_id(&self) -> Option<&str> {
        self.address.as_deref().and_then(|address| address.dom_id.as_deref())
    }

    pub fn href(&self) -> Option<&str> {
        self.address.as_deref().and_then(|address| address.href.as_deref())
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
    FlexColumn,
    FlexRow,
    Layers,
    Popover,
    /// A `<textarea>`: the field of many lines. A separate kind because
    /// the ELEMENT differs — a field that changes shape is recreated,
    /// which is the only way an input becomes a textarea.
    Editor,
    /// An `<iframe>`: the native host's page.
    Iframe,
    /// A `<video>`: the video host's stream, played by the browser.
    Video,
}

/// One island's display list and the box it paints into — what a tier
/// that owns its own pixels needs, and nothing more.
pub struct IslandList {
    pub id: u32,
    pub width: usize,
    pub height: usize,
    pub display: crate::layout::DisplayList,
}

/// One island's fresh pixels — the shell blits them into the island's
/// `<canvas>`. Only islands whose commands actually changed re-raster.
pub struct IslandFrame {
    pub id: u32,
    /// The island's whole box, physical px — the canvas's size.
    pub width: usize,
    pub height: usize,
    /// The pixels of `dirty` alone, straight RGBA, row by row.
    pub rgba: Vec<u8>,
    /// What changed, physical px: `(x, y, width, height)` inside the
    /// box. The first frame of an island, and a resized one, is the
    /// whole box; a row that changed is a strip.
    pub dirty: (u32, u32, u32, u32),
}

/// One mutation of the element tree. A frame's worth of patches is the
/// WHOLE difference between two scenes — applying them in order brings
/// the Dom up to date.
#[derive(Clone, Debug, PartialEq)]
pub enum DomPatch {
    /// A new element under `parent`, placed before sibling `before`
    /// (0 = appended). Under the absolute lowering order only decides
    /// paint stacking; under the flow it IS the layout.
    Create { id: u32, parent: u32, before: u32, kind: CreateKind, hints: DomHints },
    /// Removes the element AND its subtree.
    Remove { id: u32 },
    /// Empties the element: every child and its subtree leaves in one
    /// op — a list that clears, or replaces its rows, costs one word
    /// instead of one per row. `forget` names the ids that leave, as
    /// ranges: elements are numbered in the order they mount, so a
    /// thousand rows mounted together are ONE range, and the glue
    /// forgets them by counting instead of walking the subtree.
    RemoveChildren { id: u32, forget: Vec<(u32, u32)> },
    /// A new element cloned from a live one of the same SHAPE — the
    /// template — with its whole subtree; the ids of the copy count up
    /// in pre-order from `id`, the way a fresh mount numbers them. Only
    /// what is the copy's own follows: its words (`SetContent`), the
    /// action paths that read otherwise than the template's (`SetPath`)
    /// and the bases of the groups inside it (`SetBase`) — the copy's
    /// own base rides here, written on the element in hand. Eight
    /// creates and their styles are one word.
    Clone { id: u32, parent: u32, before: u32, template: u32, base: Option<Rc<str>> },
    /// The action path alone — what a cloned element keeps of its own.
    /// `base_len` is how much of it the element's base already says:
    /// past zero the page shows `~` and the rest, told against the
    /// nearest `data-base` at or above the element (its group's own
    /// path); zero shows the path whole.
    SetPath { id: u32, path: Option<Rc<str>>, base_len: usize },
    /// A group's own path, the base the action paths below it are told
    /// against, written on its element as `data-base`: a created group's
    /// after its subtree (the subtree says whether a path leaned on it),
    /// a group's inside a clone, whose copied base names the template's
    /// group and not its own. A clone's root takes its base on the clone.
    SetBase { id: u32, base: Rc<str> },
    /// The words alone, for a text whose font and ink already stand.
    SetContent { id: u32, text: Arc<str> },
    /// A look the page shares: one rule in its sheet, worn by every
    /// element with that look — defined once per distinct look, by the
    /// hash of what the rule carries (the kind, the shared part of the
    /// flow record, the shared part of the style, a text's face and
    /// ink). What is the element's own — its box, its marks, its
    /// action path — travels beside it.
    DefineRule {
        rule: u64,
        kind: CreateKind,
        /// Bit 0: the element lays itself out — a table-family tag,
        /// whose display is the browser's own and takes no flex line.
        flags: u8,
        style: Box<DomLook>,
        layout: Box<DomLayout>,
        text: Option<Box<DomText>>,
    },
    /// The element wears the look.
    UseRule { id: u32, rule: u64 },
    /// The element's own geometry — a pinned width or height, a
    /// ceiling, a virtual row's slot — inline, the element's and not a
    /// look's. `None` clears.
    SetBox {
        id: u32,
        width: Option<f32>,
        height: Option<f32>,
        max_width: Option<f32>,
        max_height: Option<f32>,
        slot_y: Option<f32>,
    },
    /// The element's own marks: its tooltip and the hover group it owns.
    SetMarks { id: u32, tooltip: Option<Arc<str>>, group_owner: Option<u64> },
    SetTransform { id: u32, x: f64, y: f64 },
    SetSize { id: u32, width: f64, height: f64 },
    /// The FULL style record — the glue resets and applies (styles are
    /// small; one write per changed node).
    SetText { id: u32, text: Box<DomText> },
    SetField { id: u32, field: Box<DomField> },
    SetScroll { id: u32, x: f64, y: f64 },
    SetImage { id: u32, image: DomImage },
    SetIcon { id: u32, icon: DomIcon },
    /// The iframe navigates — the diff only ships a CHANGED src, so
    /// the write is the navigation (writing the same one would
    /// reload).
    SetIframe { id: u32, src: std::rc::Rc<str>, sealed: bool },
    /// The video's whole record — the diff ships it on ANY change, and
    /// the glue decides what to touch: the fit, the mirror and the
    /// radius are attribute writes, the stream is a rewire it performs
    /// only when the handle changed (a rewrite restarts playback).
    SetVideo { id: u32, stream: u32, mirrored: bool, cover: bool, radius: f32 },
    /// The FULL flow record — the glue resets and applies, the exact
    /// twin of `SetStyle` for the other half of an element's truth.
    /// The element moves before sibling `before` (0 = to the end)
    /// under `parent` — one `insertBefore`, identity intact. Emitted
    /// for flow parents only: absolute children never need it.
    Move { id: u32, parent: u32, before: u32 },
    /// Scroll container `id` brings `target` into view — the browser
    /// computes the offset (dense lists only; a virtual list's rows
    /// may not exist, so its reveal stays an engine `SetScroll`).
    Reveal { id: u32, target: u32 },
    /// The popover's anchor relation: the glue positions `id` from
    /// element `anchor`'s real box on `side`, repositioning while
    /// either of them moves. `path` keys the dismissal doors.
    SetAnchor { id: u32, anchor: u32, side: u8, path: String },
    /// The element's LIVE hints changed — class, id and href
    /// re-attribute in place (the tag never changes without a
    /// recreation).
    SetHints {
        id: u32,
        class: Option<std::rc::Rc<str>>,
        address: Option<std::rc::Rc<crate::layout::Address>>,
    },
    /// The element's language and the way it reads — `lang` and `dir`,
    /// the attributes the browser orders, aligns and shapes by, and a
    /// screen reader speaks in. The MOUNT wears them (id 0), written on
    /// the first frame and again when the locale in effect moves; an
    /// island that reads the other way rides its look instead
    /// (`DomLayout::direction`), because a language has no CSS form and
    /// a direction has.
    SetLanguage { id: u32, lang: std::rc::Rc<str>, dir: motor::state::LayoutDirection },
}

// MARK: - Lowering (retained scene + diff)

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
    bindings: &'a mut motor::hash::FxHashMap<Rc<str>, BoundElement>,
    templates: &'a mut Templates,
    /// Subtrees that left this frame, kept until an idle moment frees
    /// them: a thousand rows' nodes are freed off the frame's clock.
    graveyard: &'a mut Vec<Buried>,
    /// The looks the page's sheet already defines, by rule hash.
    rules: &'a mut motor::hash::FxHashSet<u64>,
}

/// The retained side of the Dom mode: last frame's scene with ids.
/// One per runtime; [`lower`] turns each new scene into patches.
///
/// [`lower`]: DomLowering::lower
#[derive(Default)]
pub struct DomLowering {
    root: Option<DomNode>,
    next_id: u32,
    islands: HashMap<u32, Island>,
    /// Anchor relations already shipped: popover element id → (anchor
    /// element id, side). A relation re-ships when the anchor recreates
    /// or the side turns with the direction.
    anchors_sent: HashMap<u32, (u32, u8)>,
    /// The language the mount wears and the way it reads, as last
    /// shipped — compared each frame, written when either moved.
    language: Option<(Rc<str>, motor::state::LayoutDirection)>,
    /// Every retained Group's identity path, with the environment the
    /// walk lowered it in — the walk consults this before promising a
    /// reuse (a promise the diff cannot keep would mount a hole, and a
    /// group lowered in another environment is another scene).
    group_paths: motor::hash::FxHashMap<std::rc::Rc<str>, crate::dom_flow::GroupRecord>,
    /// The elements that read for themselves, by binding key: the id,
    /// the binding, and the record last shipped for it. A write that
    /// reaches a binding is patched from here — one element, no walk.
    bindings: motor::hash::FxHashMap<Rc<str>, BoundElement>,
    /// The shapes on the page a new group can be cloned from.
    templates: Templates,
    /// Subtrees that left and are not freed yet — see
    /// [`DomLowering::collect_garbage`].
    graveyard: Vec<Buried>,
    /// How many of those had their group records and bindings taken out
    /// of the tables. The rest still stand in them, for the idle to take
    /// out — or the next frame, before it reads either table
    /// ([`DomLowering::unpick_buried`]).
    unpicked: usize,
    /// The looks the page's sheet defines — a rule is sent once.
    rules: motor::hash::FxHashSet<u64>,
    /// How many of the groups the walk just noted the page has not
    /// seen — the rows this frame mounts, which its patch list is
    /// sized for.
    fresh_groups: usize,
}

/// The patches a mounted row costs, about: its clone or its creates,
/// its words, its action paths. The list is sized by it once, instead
/// of doubling a dozen times on a thousand rows.
const PATCHES_PER_FRESH_GROUP: usize = 8;

/// What a frame let go, kept whole until an idle moment frees it: one
/// subtree, or every child a parent had — in the one vector they left
/// in, so a list that clears moves no row and frees no vector inside
/// the frame.
enum Buried {
    One(DomNode),
    Many(Vec<DomNode>),
}

impl Buried {
    /// The subtrees, root by root.
    fn roots(&self) -> &[DomNode] {
        match self {
            Buried::One(root) => std::slice::from_ref(root),
            Buried::Many(roots) => roots,
        }
    }
}

/// One element a binding drives.
struct BoundElement {
    id: u32,
    binding: NodeBinding,
    /// What the wire last carried for it — the next patch is a copy of
    /// it with the new reading.
    shipped: Shipped,
}

enum Shipped {
    Text(DomText),
    Hints(DomHints),
}

/// The shapes the lowering can clone: for each, one live element whose
/// subtree IS the shape. A shape is everything about a group's subtree
/// that is not the row's own — kinds, layout, style, tags and classes,
/// fonts and inks — and never its words or its action paths. A patch
/// that reaches a member of the live instance (other than its words)
/// retires the template: the next row of the shape mounts whole and
/// becomes the template itself.
#[derive(Default)]
struct Templates {
    by_shape: motor::hash::FxHashMap<u64, u32>,
    /// A template root → its shape.
    roots: motor::hash::FxHashMap<u32, u64>,
    /// Every element of a template instance → its root.
    members: motor::hash::FxHashMap<u32, u32>,
    /// A root → its members, to forget together.
    members_of: motor::hash::FxHashMap<u32, Vec<u32>>,
    /// A root → the looks of its members, in pre-order: what a clone's
    /// members wear, without hashing them again.
    rules_of: motor::hash::FxHashMap<u32, Rc<[u64]>>,
    /// A root → the action paths its members show, in pre-order, each
    /// with how much of it its base says: a copy that would show the
    /// same string took it with the element.
    paths_of: motor::hash::FxHashMap<u32, Rc<[ShownPath]>>,
}

/// An action path as a member shows it: the path and how much of it
/// the member's base already says (zero: shown whole). `None`: the
/// member shows no path.
type ShownPath = Option<(Rc<str>, usize)>;

impl Templates {
    fn register(
        &mut self,
        shape: u64,
        root: u32,
        members: Vec<u32>,
        rules: Vec<u64>,
        paths: Vec<ShownPath>,
    ) {
        if self.by_shape.contains_key(&shape) {
            return;
        }
        self.by_shape.insert(shape, root);
        self.roots.insert(root, shape);
        for &id in &members {
            self.members.insert(id, root);
        }
        self.members_of.insert(root, members);
        self.rules_of.insert(root, rules.into());
        self.paths_of.insert(root, paths.into());
    }

    /// The looks of a template's members, in pre-order.
    fn rules_of(&self, root: u32) -> Rc<[u64]> {
        self.rules_of.get(&root).cloned().unwrap_or_else(|| Rc::from(Vec::new()))
    }

    /// The action paths a template's members show, in pre-order.
    fn paths_of(&self, root: u32) -> Rc<[ShownPath]> {
        self.paths_of.get(&root).cloned().unwrap_or_else(|| Rc::from(Vec::new()))
    }

    fn forget_root(&mut self, root: u32) {
        let Some(shape) = self.roots.remove(&root) else {
            return;
        };
        if self.by_shape.get(&shape) == Some(&root) {
            self.by_shape.remove(&shape);
        }
        for id in self.members_of.remove(&root).unwrap_or_default() {
            self.members.remove(&id);
        }
        self.rules_of.remove(&root);
        self.paths_of.remove(&root);
    }

    /// Something other than its words reached a member: the live
    /// instance no longer IS the shape.
    fn touched(&mut self, id: u32) {
        if let Some(&root) = self.members.get(&id) {
            self.forget_root(root);
        }
    }
}

impl DomLowering {
    /// The language the mount wears and the way it reads, noted for the
    /// diff: `Some` is the patch that writes them, when either moved —
    /// the first frame always, a frame that changed nothing never. One
    /// short string compared per frame, and no allocation until a move.
    pub(crate) fn note_language(
        &mut self,
        lang: &str,
        dir: motor::state::LayoutDirection,
    ) -> Option<DomPatch> {
        if let Some((held, held_dir)) = &self.language
            && **held == *lang
            && *held_dir == dir
        {
            return None;
        }
        let lang: Rc<str> = Rc::from(lang);
        self.language = Some((Rc::clone(&lang), dir));
        Some(DomPatch::SetLanguage { id: 0, lang, dir })
    }

    /// Diffs `scene` against the retained one and returns the patch
    /// list that brings the element tree up to date. The first call
    /// mounts everything. `display` is the SAME pass's draw list —
    /// canvas islands slice their command ranges out of it.
    ///
    /// The scene comes BY VALUE: a node the diff keeps moves into the
    /// retained tree, so a thousand new rows are moved once, never
    /// copied field by field and then dropped.
    pub fn lower(
        &mut self,
        scene: DomNode,
        display: &crate::layout::DisplayList,
    ) -> Vec<DomPatch> {
        crate::stats::time(crate::stats::Stage::Diff, || self.lower_timed(scene, display))
    }

    fn lower_timed(
        &mut self,
        mut scene: DomNode,
        display: &crate::layout::DisplayList,
    ) -> Vec<DomPatch> {
        let mut patches =
            Vec::with_capacity(std::mem::take(&mut self.fresh_groups) * PATCHES_PER_FRESH_GROUP);
        // a popover is a child of the root, the portal: a scene without
        // one cannot create one
        let popover_in_scene =
            scene.children.iter().any(|child| matches!(child.kind, DomKind::Popover { .. }));
        match self.root.as_mut() {
            None => {
                self.next_id = 1;
                patches.push(DomPatch::SetSize {
                    id: 0,
                    width: scene.width,
                    height: scene.height,
                });
                let rule = look_hash(&scene);
                // the scene becomes the retention: the root is the mount
                // point, id 0, and every node below is made where it stands
                retain(&mut scene, 0, rule);
                let mut next_id = self.next_id;
                let mut ctx = LowerCtx {
                    next_id: &mut next_id,
                    display: display.as_slice(),
                    islands: &mut self.islands,
                    bindings: &mut self.bindings,
                    templates: &mut self.templates,
                    graveyard: &mut self.graveyard,
                    rules: &mut self.rules,
                };
                define_rule(rule, &scene, &mut ctx, &mut patches);
                patches.push(DomPatch::UseRule { id: 0, rule });
                create_children(&mut scene.children, 0, &mut ctx, &mut patches, None);
                self.next_id = next_id;
                self.root = Some(scene);
            }
            Some(root) => {
                let mut next_id = self.next_id;
                let mut ctx = LowerCtx {
                    next_id: &mut next_id,
                    display: display.as_slice(),
                    islands: &mut self.islands,
                    bindings: &mut self.bindings,
                    templates: &mut self.templates,
                    graveyard: &mut self.graveyard,
                    rules: &mut self.rules,
                };
                diff_node(root, scene, &mut ctx, &mut patches);
                self.next_id = next_id;
            }
        }
        // popovers: resolve each portal's anchor to a real element and
        // ship the relation when it changed — the walk only runs while
        // a popover exists (or just left)
        if !self.anchors_sent.is_empty()
            || (popover_in_scene
                && patches
                    .iter()
                    .any(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Popover, .. })))
        {
            let mut relations: Vec<(u32, u32, u8, String)> = Vec::new();
            if let Some(root) = self.root.as_ref() {
                fn group_id(node: &DomNode, path: &str) -> Option<u32> {
                    if let DomKind::Group { path: here } = &node.kind
                        && **here == *path
                    {
                        return Some(node.id);
                    }
                    node.children.iter().find_map(|child| group_id(child, path))
                }
                fn collect(
                    node: &DomNode,
                    root: &DomNode,
                    out: &mut Vec<(u32, u32, u8, String)>,
                ) {
                    if let DomKind::Popover { path, anchor, side } = &node.kind
                        && let Some(anchor_id) = group_id(root, anchor)
                    {
                        out.push((node.id, anchor_id, *side, path.clone()));
                    }
                    for child in &node.children {
                        collect(child, root, out);
                    }
                }
                collect(root, root, &mut relations);
            }
            let live: std::collections::HashSet<u32> =
                relations.iter().map(|(id, ..)| *id).collect();
            self.anchors_sent.retain(|id, _| live.contains(id));
            for (id, anchor, side, path) in relations {
                if self.anchors_sent.get(&id) != Some(&(anchor, side)) {
                    self.anchors_sent.insert(id, (anchor, side));
                    patches.push(DomPatch::SetAnchor { id, anchor, side, path });
                }
            }
        }
        patches
    }

    /// The browser reported a scroll: fold the offset into the
    /// retained scene so the NEXT diff sees its own echo and stays
    /// silent — the browser already moved, patching it back would
    /// fight the wheel.
    pub(crate) fn note_scroll(&mut self, id: u32, x: Px, y: Px) {
        fn walk(retained: &mut DomNode, id: u32, x: Px, y: Px) -> bool {
            if retained.id == id {
                if let DomKind::Scroll { offset, .. } = &mut retained.kind {
                    *offset = (x, y);
                }
                return true;
            }
            retained.children.iter_mut().any(|child| walk(child, id, x, y))
        }
        if let Some(root) = self.root.as_mut() {
            walk(root, id, x, y);
        }
    }

    /// Hydration: the served page already holds the mount, so the
    /// lowering ADOPTS the scene as its retained truth — ids assigned
    /// in the exact pre-order the mount stream used, groups and
    /// islands registered, zero patches emitted. Islands stay dirty:
    /// a built page ships their boxes empty, and the first blit after
    /// boot fills them. The scene comes by value and stays as it came,
    /// each node numbered where it stands.
    pub(crate) fn adopt(&mut self, mut scene: DomNode, display: &crate::layout::DisplayList) {
        fn adopt_node(node: &mut DomNode, ctx: &mut LowerCtx) {
            let id = *ctx.next_id;
            *ctx.next_id += 1;
            match (&node.kind, &node.binding) {
                (DomKind::Text(text), Some(binding)) => file_binding(id, binding, text, ctx),
                (DomKind::Group { .. }, Some(binding)) => file_class_binding(id, binding, &node.hints, ctx),
                _ => {}
            }
            let rule = look_hash(node);
            ctx.rules.insert(rule);
            retain(node, id, rule);
            if matches!(node.kind, DomKind::Canvas { .. }) {
                note_island(id, node, ctx);
            }
            for child in &mut node.children {
                adopt_node(child, ctx);
            }
        }
        self.next_id = 1;
        self.group_paths.clear();
        self.bindings.clear();
        self.templates = Templates::default();
        self.islands.clear();
        self.anchors_sent.clear();
        self.graveyard.clear();
        self.unpicked = 0;
        self.rules.clear();
        // the page holds the mount already: no patch is coming for it
        self.fresh_groups = 0;
        let mut next_id = self.next_id;
        let mut ctx = LowerCtx {
            next_id: &mut next_id,
            display: display.as_slice(),
            islands: &mut self.islands,
            bindings: &mut self.bindings,
            templates: &mut self.templates,
            graveyard: &mut self.graveyard,
            rules: &mut self.rules,
        };
        // the page the build painted defined the root's look too
        let rule = look_hash(&scene);
        ctx.rules.insert(rule);
        retain(&mut scene, 0, rule);
        for child in &mut scene.children {
            adopt_node(child, &mut ctx);
        }
        self.next_id = next_id;
        self.root = Some(scene);
    }

    /// Frees the subtrees that left since the last call. A frame that
    /// removes a thousand rows ships one word and keeps their nodes;
    /// the shell calls this when the page is idle, so the freeing is
    /// never on the clock between a click and its paint. Their group
    /// records and bindings leave the tables first. Returns how many
    /// subtrees were freed.
    pub(crate) fn collect_garbage(&mut self) -> usize {
        self.unpick_buried();
        let count = self.graveyard.iter().map(|buried| buried.roots().len()).sum();
        self.graveyard.clear();
        self.unpicked = 0;
        count
    }

    /// Takes the group records and the bindings of the subtrees that left
    /// out of the tables — the half of their forgetting a frame leaves for
    /// the idle. A thousand rows that leave are a thousand group paths and
    /// two thousand binding keys hashed out, and nothing asks either table
    /// before the next frame walks: the idle does it, with the freeing, or
    /// that frame does, before it reads them, when no idle came between.
    /// A binding the same frame patches by key is asked for in the frame
    /// itself ([`DomLowering::refresh_bindings`]), which does it first.
    pub(crate) fn unpick_buried(&mut self) {
        for buried in &self.graveyard[self.unpicked..] {
            for root in buried.roots() {
                unpick(root, &mut self.group_paths, &mut self.bindings);
            }
        }
        self.unpicked = self.graveyard.len();
    }

    /// Subtrees waiting to be freed.
    pub(crate) fn garbage_pending(&self) -> bool {
        !self.graveyard.is_empty()
    }

    /// Diagnostics: retained nodes, bound elements, group records,
    /// template members, subtrees in the graveyard.
    pub(crate) fn retained_len(&self) -> usize {
        fn count(retained: &DomNode) -> usize {
            1 + retained.children.iter().map(count).sum::<usize>()
        }
        self.root.as_ref().map_or(0, count)
    }

    pub(crate) fn bindings_len(&self) -> usize {
        self.bindings.len()
    }

    pub(crate) fn groups_len(&self) -> usize {
        self.group_paths.len()
    }

    /// Diagnostics: of [`DomLowering::bindings_len`] and
    /// [`DomLowering::groups_len`], the entries that only subtrees that
    /// left still hold, waiting for the idle to take them out.
    pub(crate) fn unpicked_len(&self) -> (usize, usize) {
        fn count(retained: &DomNode, lowering: &DomLowering, counts: &mut (usize, usize)) {
            if let Some(binding) = &retained.binding
                && lowering.bindings.get(binding.key()).is_some_and(|bound| bound.id == retained.id)
            {
                counts.0 += 1;
            }
            if let DomKind::Group { path } = &retained.kind
                && lowering.group_paths.contains_key(path)
            {
                counts.1 += 1;
            }
            for child in &retained.children {
                count(child, lowering, counts);
            }
        }
        let mut counts = (0, 0);
        for root in self.graveyard[self.unpicked..].iter().flat_map(Buried::roots) {
            count(root, self, &mut counts);
        }
        counts
    }

    pub(crate) fn template_members_len(&self) -> usize {
        self.templates.members.len()
    }

    pub(crate) fn graveyard_len(&self) -> usize {
        fn count(retained: &DomNode) -> usize {
            1 + retained.children.iter().map(count).sum::<usize>()
        }
        self.graveyard.iter().flat_map(Buried::roots).map(count).sum()
    }

    /// The retained Groups' records — the flow walk consults them
    /// before promising a reuse. Borrowed, never copied: a frame asks
    /// for a thousand rows' worth of them.
    pub(crate) fn group_paths(&self) -> &motor::hash::FxHashMap<std::rc::Rc<str>, crate::dom_flow::GroupRecord> {
        &self.group_paths
    }

    /// The groups a walk lowered, with what it lowered them in: filed
    /// before the diff, so a group the diff mounts is known to the next
    /// walk with its environment.
    pub(crate) fn note_groups(&mut self, groups: Vec<(std::rc::Rc<str>, crate::dom_flow::GroupRecord)>) {
        // sized once for the rows a frame mounts, not rehashed ten times
        // on the way to a thousand
        self.group_paths.reserve(groups.len());
        let mut fresh = 0;
        let mut fresh_classes = 0;
        for (path, record) in groups {
            let reads_its_class = record.class_binding.is_some();
            if self.group_paths.insert(path, record).is_none() {
                fresh += 1;
                fresh_classes += usize::from(reads_its_class);
            }
        }
        self.fresh_groups = fresh;
        // the classes the new rows read for themselves are filed this
        // frame, whatever else is: what the bindings take at the least
        self.bindings.reserve(fresh_classes);
    }

    /// The bindings a write reached, patched by key: each one is read
    /// again and the element it drives takes the new content — one
    /// patch per changed text, no walk and no diff. A key nobody holds
    /// is a binding whose element left; nothing to do.
    pub(crate) fn refresh_bindings(&mut self, keys: &[Rc<str>]) -> Vec<DomPatch> {
        // an element this frame let go must not be patched: its binding
        // leaves the table before any key is asked
        self.unpick_buried();
        let mut patches = Vec::new();
        for key in keys {
            let Some(bound) = self.bindings.get_mut(key) else {
                continue;
            };
            match (&bound.binding, &mut bound.shipped) {
                (NodeBinding::Text(binding), Shipped::Text(shipped)) => {
                    let content = binding.get();
                    if content != shipped.content {
                        shipped.content = content;
                        patches.push(text_words_patch(bound.id, shipped));
                        crate::stats::note_binding_update();
                    }
                }
                (NodeBinding::Class(binding), Shipped::Hints(shipped)) => {
                    let class = binding.get();
                    let class: Option<Rc<str>> = (!class.is_empty()).then(|| Rc::from(class.as_str()));
                    if class != shipped.class {
                        shipped.class = class;
                        patches.push(DomPatch::SetHints {
                            id: bound.id,
                            class: shipped.class.clone(),
                            address: shipped.address.clone(),
                        });
                        crate::stats::note_binding_update();
                        self.templates.touched(bound.id);
                    }
                }
                _ => {}
            }
        }
        patches
    }

    /// Does the retained scene hold any canvas island? The runtime
    /// skips display-list collection when none is alive.
    pub(crate) fn has_islands(&self) -> bool {
        !self.islands.is_empty()
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

    /// The island path behind a canvas element id — the glue's
    /// resize observer reports by id, the runtime keys the box by
    /// the island's path.
    pub fn island_path(&self, id: u32) -> Option<std::rc::Rc<str>> {
        fn walk(retained: &DomNode, id: u32) -> Option<std::rc::Rc<str>> {
            if retained.id == id {
                return match &retained.kind {
                    DomKind::Canvas { path, .. } => path.clone(),
                    _ => None,
                };
            }
            retained.children.iter().find_map(|child| walk(child, id))
        }
        self.root.as_ref().and_then(|root| walk(root, id))
    }

    /// Does an island with this element id still stand?
    pub(crate) fn has_island(&self, id: u32) -> bool {
        self.islands.contains_key(&id)
    }

    /// Is there an element this binding key drives? A key without one
    /// reads inside pixels — an island — where no patch can reach it.
    pub(crate) fn has_binding(&self, key: &str) -> bool {
        self.bindings.contains_key(key)
    }

    /// The element id of the island with this identity path.
    pub fn island_id(&self, path: &str) -> Option<u32> {
        fn walk(retained: &DomNode, path: &str) -> Option<u32> {
            if let DomKind::Canvas { path: Some(own), .. } = &retained.kind
                && &**own == path
            {
                return Some(retained.id);
            }
            retained.children.iter().find_map(|child| walk(child, path))
        }
        self.root.as_ref().and_then(|root| walk(root, path))
    }

    /// Every element that answers a click or an edit, by id, with the
    /// path the engine knows it by — a target's action path, a field's
    /// own path, whole. What the page's resolved paths must read.
    pub fn action_paths(&self) -> std::collections::BTreeMap<u32, String> {
        fn walk(retained: &DomNode, out: &mut std::collections::BTreeMap<u32, String>) {
            match (&retained.kind, &retained.style.interactive) {
                (DomKind::Field(field), _) => {
                    out.insert(retained.id, field.path.clone());
                }
                (_, Some(path)) => {
                    out.insert(retained.id, path.to_string());
                }
                _ => {}
            }
            for child in &retained.children {
                walk(child, out);
            }
        }
        let mut out = std::collections::BTreeMap::new();
        if let Some(root) = &self.root {
            walk(root, &mut out);
        }
        out
    }

    /// Every island on the page: element id, origin and size in the
    /// layout's frame — what a probe needs to turn a hit inside an
    /// island into a point on the island's own canvas.
    pub fn island_frames(&self) -> Vec<(u32, Px, Px, Px, Px)> {
        fn walk(retained: &DomNode, out: &mut Vec<(u32, Px, Px, Px, Px)>) {
            if let DomKind::Canvas { origin, .. } = &retained.kind {
                out.push((retained.id, origin.0, origin.1, retained.width, retained.height));
            }
            for child in &retained.children {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            walk(root, &mut out);
        }
        out
    }

    /// The scroll region path an element id belongs to — the glue's
    /// scroll observer reports by id, the runtime scrolls by path.
    pub fn scroll_path(&self, id: u32) -> Option<String> {
        fn walk(retained: &DomNode, id: u32) -> Option<String> {
            if retained.id == id {
                return match &retained.kind {
                    DomKind::Scroll { path, .. } => path.clone(),
                    _ => None,
                };
            }
            retained.children.iter().find_map(|child| walk(child, id))
        }
        self.root.as_ref().and_then(|root| walk(root, id))
    }
}

// MARK: - The look

/// The part of a flow record a rule shares: everything but the
/// element's own geometry.
fn look_layout(layout: &DomLayout) -> DomLayout {
    DomLayout {
        width: None,
        height: None,
        max_width: None,
        max_height: None,
        slot_y: None,
        ..layout.clone()
    }
}

/// The part of a style a rule shares: everything but the element's own
/// action path, tooltip and the group it owns.
fn look_style(style: &DomStyle) -> DomLook {
    style.look().clone()
}

/// A text's face and ink, without its words.
fn look_text(text: &DomText) -> DomText {
    DomText { content: Arc::from(""), highlights: None, ..text.clone() }
}

fn hash_color(color: Option<Color>, hasher: &mut motor::hash::FxHasher) {
    use std::hash::Hash;
    color.map(|color| u32::from_be_bytes([color.r, color.g, color.b, color.a])).hash(hasher);
}

fn hash_f64(value: Option<f64>, hasher: &mut motor::hash::FxHasher) {
    use std::hash::Hash;
    value.map(f64::to_bits).hash(hasher);
}

/// An `f32` of the scene, hashed as the `f64` it widens to: the record
/// held `f64` once, and a rule's hash is part of the served page's
/// contract — the same value keeps the same hash.
fn hash_f32(value: Option<f32>, hasher: &mut motor::hash::FxHasher) {
    hash_f64(value.map(f64::from), hasher);
}

/// Does the element lay itself out? The table family's display is the
/// browser's own: a `<tr>` is a row, never a flex line.
pub(crate) fn lays_itself_out(hints: &DomHints) -> bool {
    matches!(
        hints.tag.as_deref(),
        Some("table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th")
    )
}

/// The hash of a node's look: its kind, the shared part of its flow
/// record, the shared part of its style and, for a text, its face and
/// ink. Two elements with one hash wear one rule.
fn look_hash(node: &DomNode) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = motor::hash::FxHasher::default();
    lays_itself_out(&node.hints).hash(&mut hasher);
    let kind: u8 = match &node.kind {
        DomKind::Root => 0,
        DomKind::Group { .. } => 1,
        DomKind::Box => 2,
        DomKind::Text(_) => 3,
        DomKind::Field(_) => 4,
        DomKind::Scroll { .. } => 5,
        DomKind::Content => 6,
        DomKind::Canvas { .. } => 7,
        DomKind::Image(_) => 8,
        DomKind::Icon(_) => 9,
        DomKind::Iframe { .. } => 10,
        DomKind::FlexColumn => 11,
        DomKind::FlexRow => 12,
        DomKind::Layers => 13,
        DomKind::Reuse { .. } => 14,
        DomKind::Popover { .. } => 15,
        DomKind::Video { .. } => 16,
    };
    kind.hash(&mut hasher);
    match &node.layout {
        Some(layout) => {
            1u8.hash(&mut hasher);
            hash_f32(layout.gap, &mut hasher);
            layout.align.hash(&mut hasher);
            layout
                .padding
                .map(|(top, trailing, bottom, leading)| {
                    [top, trailing, bottom, leading].map(|side| f64::from(side).to_bits())
                })
                .hash(&mut hasher);
            layout.grow.hash(&mut hasher);
            layout.stretch.hash(&mut hasher);
            layout.fill.hash(&mut hasher);
            hash_f32(layout.wrap, &mut hasher);
            layout.plain.hash(&mut hasher);
            // only an island adds to the hash: every look without a direction
            // of its own hashes as it always did
            if let Some(direction) = layout.direction {
                direction.is_rtl().hash(&mut hasher);
            }
        }
        None => 0u8.hash(&mut hasher),
    }
    let style = node.style.look();
    hash_color(style.background, &mut hasher);
    hash_color(style.hover_background, &mut hasher);
    hash_color(style.pressed_background, &mut hasher);
    hash_color(style.color, &mut hasher);
    hash_color(style.hover_color, &mut hasher);
    hash_color(style.pressed_color, &mut hasher);
    hash_color(style.focus_border, &mut hasher);
    hash_color(style.placeholder_color, &mut hasher);
    style.border.map(|(color, width)| (u32::from_be_bytes([color.r, color.g, color.b, color.a]), width.to_bits())).hash(&mut hasher);
    style
        .corner_radius
        .map(|corners| [corners.top_left.to_bits(), corners.top_right.to_bits(), corners.bottom_right.to_bits(), corners.bottom_left.to_bits()])
        .hash(&mut hasher);
    style.shadow.map(|(radius, color)| (radius.to_bits(), u32::from_be_bytes([color.r, color.g, color.b, color.a]))).hash(&mut hasher);
    style.transition.map(|(response, damping)| (response.to_bits(), damping.to_bits())).hash(&mut hasher);
    style.clip.hash(&mut hasher);
    hash_f64(style.opacity, &mut hasher);
    hash_f64(style.hover_opacity, &mut hasher);
    hash_f64(style.pressed_opacity, &mut hasher);
    style.group.hash(&mut hasher);
    style.pass_through.hash(&mut hasher);
    match &style.gradient {
        Some(crate::layout::Gradient::Radial { center, start, end, aspect, inner, outer }) => {
            1u8.hash(&mut hasher);
            [center.x.to_bits(), center.y.to_bits(), start.to_bits(), end.unwrap_or(-1.0).to_bits(), aspect.to_bits()].hash(&mut hasher);
            hash_color(Some(*inner), &mut hasher);
            hash_color(Some(*outer), &mut hasher);
        }
        Some(crate::layout::Gradient::Linear { start, end, from, to }) => {
            2u8.hash(&mut hasher);
            [start.x.to_bits(), start.y.to_bits(), end.x.to_bits(), end.y.to_bits()].hash(&mut hasher);
            hash_color(Some(*from), &mut hasher);
            hash_color(Some(*to), &mut hasher);
        }
        None => 0u8.hash(&mut hasher),
    }
    match &style.glass {
        Some(glass) => {
            1u8.hash(&mut hasher);
            [glass.blur.to_bits(), glass.saturation.to_bits(), glass.brightness.to_bits(), glass.rim_band.to_bits()].hash(&mut hasher);
            hash_color(Some(glass.rim), &mut hasher);
        }
        None => 0u8.hash(&mut hasher),
    }
    match &node.face {
        Some(face) => {
            1u8.hash(&mut hasher);
            face.size.to_bits().hash(&mut hasher);
            (face.weight as u8).hash(&mut hasher);
            (face.design as u8).hash(&mut hasher);
            (face.slant as u8).hash(&mut hasher);
            face.family.name().as_deref().hash(&mut hasher);
            face.tracking.to_bits().hash(&mut hasher);
        }
        None => 0u8.hash(&mut hasher),
    }
    if let DomKind::Text(text) = &node.kind {
        text.inherits_face.hash(&mut hasher);
        hash_color(Some(text.color), &mut hasher);
        text.inherits_ink.hash(&mut hasher);
        text.font.size.to_bits().hash(&mut hasher);
        (text.font.weight as u8).hash(&mut hasher);
        (text.font.design as u8).hash(&mut hasher);
        (text.font.slant as u8).hash(&mut hasher);
        text.font.family.name().as_deref().hash(&mut hasher);
        text.font.tracking.to_bits().hash(&mut hasher);
        hash_f32(text.line_height, &mut hasher);
        text.text_align.map(|align| align as u8).hash(&mut hasher);
        text.truncation.map(|mode| mode as u8).hash(&mut hasher);
    }
    hasher.finish()
}

/// Sends the look's rule once: the first element to wear it defines it.
fn define_rule(rule: u64, node: &DomNode, ctx: &mut LowerCtx, patches: &mut Vec<DomPatch>) {
    if !ctx.rules.insert(rule) {
        return;
    }
    let kind = match &node.kind {
        DomKind::Root => CreateKind::Group,
        other => create_kind(other),
    };
    patches.push(DomPatch::DefineRule {
        rule,
        kind,
        flags: u8::from(lays_itself_out(&node.hints)),
        style: Box::new(look_style(&node.style)),
        layout: Box::new(node.layout.as_ref().map(look_layout).unwrap_or_default()),
        text: match &node.kind {
            DomKind::Text(text) => Some(Box::new(look_text(text))),
            _ => node.face.as_deref().map(|face| Box::new(face_only(*face))),
        },
    });
}

/// The look's text record for an element that declares a face for its
/// subtree and shows no words of its own: the face, nothing else.
fn face_only(face: FontSpec) -> DomText {
    DomText {
        content: Arc::from(""),
        color: Color::BLACK,
        inherits_ink: true,
        font: face,
        line_height: None,
        text_align: None,
        highlights: None,
        truncation: None,
        inherits_face: false,
    }
}

/// The element's own geometry, as the box patch carries it.
fn geometry_of(layout: &DomLayout) -> [Option<f32>; 5] {
    [layout.width, layout.height, layout.max_width, layout.max_height, layout.slot_y]
}

fn box_patch(id: u32, geometry: [Option<f32>; 5]) -> DomPatch {
    let [width, height, max_width, max_height, slot_y] = geometry;
    DomPatch::SetBox { id, width, height, max_width, max_height, slot_y }
}

/// The element's own marks.
fn marks_of(style: &DomStyle) -> (Option<Arc<str>>, Option<u64>) {
    (style.tooltip().cloned(), style.group_owner())
}

/// A text's words: alone when nothing is highlighted, with the spans
/// when something is.
fn text_words_patch(id: u32, text: &DomText) -> DomPatch {
    if text.highlights.is_some() {
        DomPatch::SetText { id, text: Box::new(text.clone()) }
    } else {
        DomPatch::SetContent { id, text: Arc::clone(&text.content) }
    }
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
        DomKind::Iframe { .. } => CreateKind::Iframe,
        DomKind::Video { .. } => CreateKind::Video,
        DomKind::FlexColumn => CreateKind::FlexColumn,
        DomKind::FlexRow => CreateKind::FlexRow,
        DomKind::Layers => CreateKind::Layers,
        DomKind::Popover { .. } => CreateKind::Popover,
        // a reuse only exists where a retained group matched; reaching
        // creation means the promise broke — mount an empty anchor and
        // let the next frame heal it
        DomKind::Reuse { .. } => CreateKind::Group,
    }
}

/// The island's slice of the pass's display list, moved to island-
/// local coordinates (the raster surface starts at zero).
fn island_commands(node: &DomNode, ctx: &LowerCtx) -> Vec<DrawCommand> {
    let DomKind::Canvas { origin, display, .. } = &node.kind else {
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

/// [`create_subtree`] with a real position: the root lands `before`
/// its next sibling (0 = append). The interior appends in order — a
/// fresh subtree has nothing to dodge.
fn create_subtree_before(
    node: &mut DomNode,
    parent: u32,
    before: u32,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
    sibling: Option<(&DomNode, u32)>,
) -> Option<u32> {
    let opened = patches.len();
    // a fresh child of a kept parent: the base above it may not be on
    // the page, so only a group of its own tells a path against one
    let created = create_subtree(node, parent, ctx, patches, sibling, None);
    if before != 0
        && let DomPatch::Create { before: slot, .. } | DomPatch::Clone { before: slot, .. } =
            &mut patches[opened]
    {
        *slot = before;
    }
    created
}

/// The group the action paths below it are told against: the nearest
/// Group at or above an element — the element itself when it is one.
/// A path that lies under the base ships as `~` and the rest; the base
/// is written on the group's element only when a path leaned on it.
struct Base {
    path: Rc<str>,
    leaned_on: bool,
}

impl Base {
    /// The base a node opens, when it is a group.
    fn of(node: &DomNode) -> Option<Base> {
        match &node.kind {
            DomKind::Group { path } => Some(Base { path: Rc::clone(path), leaned_on: false }),
            _ => None,
        }
    }
}

/// How much of an action path its base already says: the base's own
/// length when the path lies under it, and the base is leaned on; zero
/// when it does not — an action armed above the group ships whole.
fn told_against(path: &str, base: Option<&mut Base>) -> usize {
    let Some(base) = base else {
        return 0;
    };
    let len = base_len(path, Some(&base.path));
    if len > 0 {
        base.leaned_on = true;
    }
    len
}

/// The length of `base` when `path` lies under it (`base` + `/` + the
/// rest), zero otherwise. `~` starts no identity path, so the page
/// tells a relative path from a whole one by its first character.
fn base_len(path: &str, base: Option<&str>) -> usize {
    match base {
        Some(base)
            if !base.is_empty()
                && path.len() > base.len() + 1
                && path.as_bytes()[base.len()] == b'/'
                && path.starts_with(base) =>
        {
            base.len()
        }
        _ => 0,
    }
}

/// The string the page shows for an action path, `base_len` of it
/// told by its base: `~` and the rest, or the path whole.
pub(crate) fn shown_path(path: &str, base_len: usize) -> String {
    match base_len {
        0 => path.to_string(),
        len => format!("~{}", &path[len..]),
    }
}

/// Does the template's member show this path already — the same
/// string, so the copy took it with the element?
fn shows_already(paths: &[ShownPath], member: usize, path: &str, base_len: usize) -> bool {
    matches!(
        paths.get(member),
        Some(Some((was, was_len)))
            if (*was_len == 0) == (base_len == 0) && was[*was_len..] == path[base_len..]
    )
}

/// What a template's members show, in pre-order: the looks they wear
/// and the action paths on them.
struct Shown<'a> {
    rules: &'a [u64],
    paths: &'a [ShownPath],
}

/// A copy of a live template: one word for the whole subtree, then the
/// copy's own (see [`clone_instance`]). The copy's base, when a path in
/// it leans on one, rides on that word.
fn clone_subtree(
    node: &mut DomNode,
    parent: u32,
    template: u32,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) {
    let id = *ctx.next_id;
    *ctx.next_id += 1;
    let cloned = patches.len();
    patches.push(DomPatch::Clone { id, parent, before: 0, template, base: None });
    crate::stats::note_clone();
    let rules = ctx.templates.rules_of(template);
    let paths = ctx.templates.paths_of(template);
    let mut at = 0;
    let shown = Shown { rules: &rules, paths: &paths };
    clone_instance(node, id, &shown, &mut at, ctx, patches, None, Some(cloned))
}

/// The copy's own: ids in pre-order, every text's words, every action
/// path its template's member does not show already, the base of every
/// group inside it a path leans on — and the bindings filed under the
/// new ids, each written on its node where it stands: the subtree
/// moves into the retention as it came. `base` is the group the
/// member's path is told against; `cloned` is the clone's word when the
/// member is the copy's root, whose base rides on it.
#[allow(clippy::too_many_arguments)]
fn clone_instance(
    node: &mut DomNode,
    id: u32,
    template: &Shown,
    at: &mut usize,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
    base: Option<&mut Base>,
    cloned: Option<usize>,
) {
    let member = *at;
    // the look is the template's, by position; a template that lost its
    // list of looks is hashed again
    let rule = match template.rules.get(member) {
        Some(rule) => *rule,
        None => look_hash(node),
    };
    *at += 1;
    let opened = patches.len();
    if let DomKind::Text(text) = &node.kind {
        patches.push(DomPatch::SetContent { id, text: Arc::clone(&text.content) });
        if let Some(binding) = &node.binding {
            file_binding(id, binding, text, ctx);
        }
    }
    // a group is the base of the paths below it, its own included
    let mut own = Base::of(node);
    let mut base = match own.as_mut() {
        Some(own) => Some(own),
        None => base,
    };
    if let Some(path) = &node.style.interactive {
        let base_len = told_against(path, base.as_deref_mut());
        // the template's member shows the same string: the copy took it
        // with the element
        if !shows_already(template.paths, member, path, base_len) {
            patches.push(DomPatch::SetPath { id, path: Some(Rc::clone(path)), base_len });
        }
    }
    if let (DomKind::Group { .. }, Some(binding)) = (&node.kind, &node.binding) {
        file_class_binding(id, binding, &node.hints, ctx);
    }
    for child in &mut node.children {
        let child_id = *ctx.next_id;
        *ctx.next_id += 1;
        clone_instance(child, child_id, template, at, ctx, patches, base.as_deref_mut(), None);
    }
    // a group a path leans on carries its OWN path: the base it was
    // copied with names the template's group. The copy's root says it
    // on the clone's word; a group inside, ahead of its members' words.
    // A group no path leans on may keep the template's: nothing below
    // it reads a base
    if let Some(own) = own
        && own.leaned_on
    {
        match cloned {
            Some(word) => {
                if let DomPatch::Clone { base, .. } = &mut patches[word] {
                    *base = Some(own.path);
                }
            }
            None => patches.insert(opened, DomPatch::SetBase { id, base: own.path }),
        }
    }
    retain(node, id, rule);
}

/// A scene node becomes a retained one where it stands: its id and its
/// look are written on it, and what its children's vector holds beyond
/// them is given back — a capture may grow a vector as it fills it, and
/// the retention keeps the vector the node came with.
fn retain(node: &mut DomNode, id: u32, rule: u64) {
    node.id = id;
    node.rule = rule;
    node.children.shrink_to_fit();
}

/// The shape of a subtree, hashed — `None` when the subtree holds
/// something a clone cannot carry: a field or a scroll region (the
/// browser's own state), an island, an image, an icon, a frame, a
/// popover, absolute geometry, an id, a highlight, or a style that
/// hangs a rule or an attribute off the element's own id (states,
/// groups, glass, tooltips, gradients).
fn shape_of(node: &DomNode) -> Option<u64> {
    use std::hash::Hasher;
    crate::stats::note_shape_hashed();
    let mut hasher = motor::hash::FxHasher::default();
    shape_into(node, &mut hasher).then(|| hasher.finish())
}

/// Does the subtree have the shape of `old`, a template instance? The
/// twin of [`shape_into`] that compares instead of hashing: the same
/// fields, in the same sense, and the first difference ends it.
fn same_shape(node: &DomNode, old: &DomNode) -> bool {
    if std::mem::discriminant(&node.kind) != std::mem::discriminant(&old.kind) {
        return false;
    }
    match &node.kind {
        DomKind::Group { .. }
        | DomKind::Box
        | DomKind::FlexColumn
        | DomKind::FlexRow
        | DomKind::Layers => {}
        DomKind::Text(text) => {
            if text.highlights.is_some() {
                return false;
            }
            let DomKind::Text(was) = &old.kind else {
                return false;
            };
            // the face and the ink, never the words
            if text.color != was.color
                || text.inherits_face != was.inherits_face
                || text.inherits_ink != was.inherits_ink
                || text.font != was.font
                || text.line_height != was.line_height
                || text.text_align != was.text_align
                || text.truncation != was.truncation
            {
                return false;
            }
        }
        _ => return false,
    }
    if node.hints.address.is_some() || node.style.has_marks() {
        return false;
    }
    let (Some(layout), Some(old_layout)) = (&node.layout, &old.layout) else {
        return false;
    };
    node.face == old.face
        && same_look_layout(layout, old_layout)
        && geometry_of(layout) == geometry_of(old_layout)
        && same_look_style(&node.style, &old.style)
        && node.hints.tag == old.hints.tag
        && node.hints.class == old.hints.class
        && node.style.interactive.is_some() == old.style.interactive.is_some()
        && node.binding.is_some() == old.binding.is_some()
        && node.children.len() == old.children.len()
        && node.children.iter().zip(&old.children).all(|(child, was)| same_shape(child, was))
}

/// The shared part of two flow records, equal? (What [`look_hash`]
/// reads of a layout.)
fn same_look_layout(a: &DomLayout, b: &DomLayout) -> bool {
    a.gap == b.gap
        && a.align == b.align
        && a.padding == b.padding
        && a.grow == b.grow
        && a.stretch == b.stretch
        && a.fill == b.fill
        && a.wrap == b.wrap
        && a.plain == b.plain
}

/// The shared part of two styles, equal? (What [`look_hash`] reads of
/// a style: its look — everything but the element's own path, tooltip
/// and group.)
fn same_look_style(a: &DomStyle, b: &DomStyle) -> bool {
    same_look(a.look(), b.look())
}

fn shape_into(node: &DomNode, hasher: &mut motor::hash::FxHasher) -> bool {
    use std::hash::Hash;
    // the kinds a clone can carry: the ones with no state of the
    // browser's own and no identity of their own
    match &node.kind {
        DomKind::Group { .. }
        | DomKind::Box
        | DomKind::Text(_)
        | DomKind::FlexColumn
        | DomKind::FlexRow
        | DomKind::Layers => {}
        _ => return false,
    }
    if let DomKind::Text(text) = &node.kind
        && text.highlights.is_some()
    {
        return false;
    }
    // an id, a tooltip, a group of its own: the element's, never a shape's
    if node.hints.address.is_some() || node.style.has_marks() {
        return false;
    }
    let Some(layout) = &node.layout else {
        return false;
    };
    // the look says everything a rule shares; the box and the hints
    // say the rest the clone must carry byte for byte
    look_hash(node).hash(hasher);
    geometry_of(layout).map(|value| value.map(f32::to_bits)).hash(hasher);
    node.hints.tag.as_deref().hash(hasher);
    node.hints.class.as_deref().hash(hasher);
    // the action path is the row's own; that there IS one is the shape
    node.style.interactive.is_some().hash(hasher);
    // a binding reads its own value; that there is one is the shape
    node.binding.is_some().hash(hasher);
    node.children.len().hash(hasher);
    node.children.iter().all(|child| shape_into(child, hasher))
}

/// Emits the patches that build `node` under `parent` and makes the
/// node its own retained mirror, where it stands: its id and its look
/// are written on it and on every node below, and no node moves.
/// Returns the template the subtree is an instance of, when it is one
/// (a clone of it, or the template itself), so the next sibling can be
/// compared with it instead of hashed.
///
/// `sibling` is the sibling made just before this one, with the
/// template IT is an instance of. `base` is the group a path here is
/// told against, when one stands above in this subtree (a group opens
/// its own).
fn create_subtree(
    node: &mut DomNode,
    parent: u32,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
    sibling: Option<(&DomNode, u32)>,
    base: Option<&mut Base>,
) -> Option<u32> {
    // the sibling made just before this one, when it is an instance of a
    // live template and this subtree has its shape: cloned from that
    // template at once, by a walk that compares and stops at the first
    // difference — never a hash over every node of every row. A list's
    // rows mostly look alike, and this is the road they take, row after
    // row: each new row is compared with the copy made just before it
    if let Some((sibling, template)) = sibling
        && matches!(node.kind, DomKind::Group { .. })
        && ctx.templates.roots.contains_key(&template)
        && same_shape(node, sibling)
    {
        clone_subtree(node, parent, template, ctx, patches);
        return Some(template);
    }
    // the shape is the whole subtree's, read before anything below is
    // made
    let shape = match node.kind {
        DomKind::Group { .. } => shape_of(node),
        _ => None,
    };
    // a shape already on the page is cloned, never built again: one
    // word for the subtree, then its words and its action paths
    if let Some(shape) = shape
        && let Some(&template) = ctx.templates.by_shape.get(&shape)
    {
        clone_subtree(node, parent, template, ctx, patches);
        return Some(template);
    }
    let id = *ctx.next_id;
    *ctx.next_id += 1;
    patches.push(DomPatch::Create {
        id,
        parent,
        before: 0,
        kind: create_kind(&node.kind),
        hints: node.hints.clone(),
    });
    if let (DomKind::Group { .. }, Some(binding)) = (&node.kind, &node.binding) {
        file_class_binding(id, binding, &node.hints, ctx);
    }
    // the look is shared; the box, the marks and the path are the
    // element's own
    let rule = look_hash(node);
    define_rule(rule, node, ctx, patches);
    patches.push(DomPatch::UseRule { id, rule });
    match &node.layout {
        // a flow node speaks semantics; its geometry fields are silent
        Some(layout) => {
            let geometry = geometry_of(layout);
            if geometry.iter().any(Option::is_some) {
                patches.push(box_patch(id, geometry));
            }
        }
        None => {
            patches.push(DomPatch::SetTransform { id, x: node.x, y: node.y });
            patches.push(DomPatch::SetSize { id, width: node.width, height: node.height });
        }
    }
    if node.style.has_marks() {
        let (tooltip, group_owner) = marks_of(&node.style);
        patches.push(DomPatch::SetMarks { id, tooltip, group_owner });
    }
    // a group is the base of the paths below it, its own included
    let mut own = Base::of(node);
    let mut base = match own.as_mut() {
        Some(own) => Some(own),
        None => base,
    };
    if let Some(path) = &node.style.interactive {
        let base_len = told_against(path, base.as_deref_mut());
        patches.push(DomPatch::SetPath { id, path: Some(Rc::clone(path)), base_len });
    }
    match &node.kind {
        DomKind::Text(text) => {
            patches.push(text_words_patch(id, text));
            if let Some(binding) = &node.binding {
                file_binding(id, binding, text, ctx);
            }
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
        DomKind::Iframe { src, sealed } => {
            patches.push(DomPatch::SetIframe { id, src: std::rc::Rc::clone(src), sealed: *sealed });
        }
        DomKind::Video { stream, mirrored, cover, radius } => {
            patches.push(DomPatch::SetVideo {
                id,
                stream: *stream,
                mirrored: *mirrored,
                cover: *cover,
                radius: *radius,
            });
        }
        _ => {}
    }
    let templates_before = ctx.templates.roots.len();
    create_children(&mut node.children, id, ctx, patches, base);
    // a group a path leans on carries its own path, said once its
    // subtree has: a word after it, where no patch has to move for it
    if let Some(own) = own
        && own.leaned_on
    {
        patches.push(DomPatch::SetBase { id, base: own.path });
    }
    retain(node, id, rule);
    // the first of a shape is the template the next ones clone — unless
    // a template was made inside it. A member answers to ONE template:
    // a subtree holding another template's root would take that
    // template's members as its own, and the inner one, retired by
    // nobody when its row changed or left, would hand out copies of an
    // element no longer on the page
    let holds_a_template = ctx.templates.roots.len() > templates_before;
    if let Some(shape) = shape
        && !holds_a_template
        && !ctx.templates.by_shape.contains_key(&shape)
    {
        let mut members = Vec::new();
        let mut rules = Vec::new();
        let mut paths = Vec::new();
        collect_members(node, None, &mut members, &mut rules, &mut paths);
        ctx.templates.register(shape, id, members, rules, paths);
        return Some(id);
    }
    None
}

/// Every id, every look and every action path of a subtree, in
/// pre-order — the order a clone is numbered in. A path is recorded as
/// the page shows it: told against the nearest group at or above it,
/// the same telling the subtree was made with.
fn collect_members(
    retained: &DomNode,
    base: Option<&str>,
    ids: &mut Vec<u32>,
    rules: &mut Vec<u64>,
    paths: &mut Vec<ShownPath>,
) {
    ids.push(retained.id);
    rules.push(retained.rule);
    let base = match &retained.kind {
        DomKind::Group { path } => Some(&**path),
        _ => base,
    };
    paths.push(
        retained.style.interactive.as_ref().map(|path| (Rc::clone(path), base_len(path, base))),
    );
    for child in &retained.children {
        collect_members(child, base, ids, rules, paths);
    }
}

/// Makes every child where it stands, in order, each compared with the
/// sibling made just before it.
fn create_children(
    children: &mut [DomNode],
    parent: u32,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
    mut base: Option<&mut Base>,
) {
    let mut template = None;
    for at in 0..children.len() {
        let (made, rest) = children.split_at_mut(at);
        let sibling = made.last().zip(template);
        template = create_subtree(&mut rest[0], parent, ctx, patches, sibling, base.as_deref_mut());
    }
}

/// One remove patch frees the whole subtree on the glue's side; the
/// island registry forgets every canvas underneath.
/// Files the element a text binding drives, with the record just shipped.
fn file_binding(id: u32, binding: &NodeBinding, shipped: &DomText, ctx: &mut LowerCtx) {
    ctx.bindings.insert(
        Rc::clone(binding.key()),
        BoundElement { id, binding: binding.clone(), shipped: Shipped::Text(shipped.clone()) },
    );
}

/// Files the element a class binding drives, with the hints just shipped.
fn file_class_binding(id: u32, binding: &NodeBinding, shipped: &DomHints, ctx: &mut LowerCtx) {
    ctx.bindings.insert(
        Rc::clone(binding.key()),
        BoundElement { id, binding: binding.clone(), shipped: Shipped::Hints(shipped.clone()) },
    );
}

/// One subtree leaves: one remove patch frees it on the glue's side.
/// What the diff itself may still ask about it leaves now — a template
/// it held must not be cloned, an island it held must not be painted —
/// and a subtree that held neither is not walked at all. Its groups and
/// bindings wait with it ([`DomLowering::unpick_buried`]).
fn remove_subtree(retained: DomNode, ctx: &mut LowerCtx, patches: &mut Vec<DomPatch>) {
    patches.push(DomPatch::Remove { id: retained.id });
    if !ctx.templates.members.is_empty() || !ctx.islands.is_empty() {
        forget_now(&retained, ctx);
    }
    ctx.graveyard.push(Buried::One(retained));
}

/// Every old child leaves and nothing stays: the parent is emptied in
/// one op, which names every id that leaves, and the children go to the
/// graveyard as the one vector they were.
fn remove_all_children(
    parent: u32,
    leaving: Vec<DomNode>,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let islands = !ctx.islands.is_empty();
    for retained in &leaving {
        // a whole row leaves: the template question is the row's, once
        // — a member below it cannot outlive its root; a template the
        // row holds leaves as the walk passes its root
        if !ctx.templates.members.is_empty() {
            ctx.templates.touched(retained.id);
        }
        number_leaving(retained, &mut runs);
        if islands {
            forget_islands(retained, ctx);
        }
    }
    let forget = id_ranges(runs);
    // a template inside a row that leaves whole, the row none itself (it
    // holds this one): the row's question never reached it, and left
    // standing it would hand out copies of an element no longer on the
    // page. Its root is among the ids that leave — the few templates that
    // stand are looked up in the runs, and no node is walked again. This
    // cannot wait for the idle: the next frame may already ask for a copy
    if !ctx.templates.roots.is_empty() {
        let inside = |root: u32| {
            let at = forget.partition_point(|&(_, end)| end <= root);
            forget.get(at).is_some_and(|&(start, _)| start <= root)
        };
        let gone: Vec<u32> = ctx.templates.roots.keys().copied().filter(|&root| inside(root)).collect();
        for root in gone {
            ctx.templates.forget_root(root);
        }
    }
    patches.push(DomPatch::RemoveChildren { id: parent, forget });
    ctx.graveyard.push(Buried::Many(leaving));
}

/// Runs of ids as sorted half-open ranges `[start, end)`, neighbours
/// merged.
///
/// A subtree numbered in one piece is one run, so the walk hands over a
/// run per row, not an id per node; and a list filled back to front
/// numbers its last row first, so the runs come in reverse. A thousand
/// of them are put in order where nine thousand ids were — and not at
/// all when they already are.
fn id_ranges(mut runs: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    if !runs.is_sorted() {
        runs.sort_unstable();
    }
    let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(runs.len());
    for (start, end) in runs {
        match ranges.last_mut() {
            Some((_, last)) if *last == start => *last = end,
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

/// The id at the end of the runs: the run it continues grows, any other
/// starts its own.
fn push_run(runs: &mut Vec<(u32, u32)>, id: u32) {
    match runs.last_mut() {
        Some((_, end)) if *end == id => *end += 1,
        _ => runs.push((id, id + 1)),
    }
}

/// What the diff of this very frame may still ask about a subtree that
/// left: a template member retires its template (asked only while any
/// template stands), and an island leaves the registry.
fn forget_now(retained: &DomNode, ctx: &mut LowerCtx) {
    if !ctx.templates.members.is_empty() {
        ctx.templates.touched(retained.id);
    }
    if let DomKind::Canvas { .. } = &retained.kind {
        ctx.islands.remove(&retained.id);
    }
    for child in &retained.children {
        forget_now(child, ctx);
    }
}

/// The ids of a row that leaves whole, as runs — the patch names them.
/// Its template question was the row's own, its islands are asked apart
/// and only while any stands, and its groups and bindings wait for the
/// idle: a thousand rows that leave together are nine thousand nodes, and
/// the walk asks each one for its id and its children, and nothing else.
fn number_leaving(retained: &DomNode, runs: &mut Vec<(u32, u32)>) {
    push_run(runs, retained.id);
    for child in &retained.children {
        // most nodes of a row are leaves: they cost no call
        if child.children.is_empty() {
            push_run(runs, child.id);
        } else {
            number_leaving(child, runs);
        }
    }
}

/// The islands of a subtree that left, out of the registry: nothing may
/// paint them again.
fn forget_islands(retained: &DomNode, ctx: &mut LowerCtx) {
    if let DomKind::Canvas { .. } = &retained.kind {
        ctx.islands.remove(&retained.id);
    }
    for child in &retained.children {
        forget_islands(child, ctx);
    }
}

/// A subtree's group records and its bindings, out of the tables: each
/// group by its path, and each binding only while it still drives this
/// element — a node made at the same place since took the key over, and
/// keeps it.
fn unpick(
    retained: &DomNode,
    groups: &mut motor::hash::FxHashMap<std::rc::Rc<str>, crate::dom_flow::GroupRecord>,
    bindings: &mut motor::hash::FxHashMap<Rc<str>, BoundElement>,
) {
    if let DomKind::Group { path } = &retained.kind {
        groups.remove(path);
    }
    if let Some(binding) = &retained.binding
        && let std::collections::hash_map::Entry::Occupied(bound) = bindings.entry(Rc::clone(binding.key()))
        && bound.get().id == retained.id
    {
        bound.remove();
    }
    for child in &retained.children {
        unpick(child, groups, bindings);
    }
}

#[cfg(test)]
mod forget_tests {
    use super::{id_ranges, push_run};

    /// The ranges the old way spelled: every id, sorted, neighbours merged.
    fn by_ids(ids: &[u32]) -> Vec<(u32, u32)> {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for id in ids {
            match ranges.last_mut() {
                Some((_, end)) if *end == id => *end += 1,
                _ => ranges.push((id, id + 1)),
            }
        }
        ranges
    }

    fn by_runs(ids: &[u32]) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        for id in ids {
            push_run(&mut runs, *id);
        }
        id_ranges(runs)
    }

    /// A clear forgets every id that leaves, as ranges the glue prunes by.
    /// The walk now hands them over as runs — a row numbered in one piece
    /// is one — and the ranges must be exactly those every id sorted and
    /// merged would give: rows mounted back to front (the last row
    /// numbered first) close into one range, a reorder's mix into the
    /// same ranges as before, a gap stays a gap.
    #[test]
    fn rows_forgotten_as_runs_give_the_ranges_every_id_gave() {
        // three rows of nine nodes, numbered last row first
        let back_to_front: Vec<u32> = (0..3u32).rev().flat_map(|row| row * 9..row * 9 + 9).collect();
        assert_eq!(by_runs(&back_to_front), [(0, 27)]);
        assert_eq!(by_runs(&back_to_front), by_ids(&back_to_front));
        // a mixed order with a hole, and a row split by a node made later
        let mixed = [12, 13, 14, 3, 4, 5, 40, 6, 7, 8, 9, 30, 31, 0, 1, 2];
        assert_eq!(by_runs(&mixed), by_ids(&mixed));
        assert_eq!(by_runs(&mixed), [(0, 10), (12, 15), (30, 32), (40, 41)]);
        assert_eq!(by_runs(&[]), []);
    }
}

/// Diffs one matched pair: geometry, style, kind payload, children.
fn diff_node(
    retained: &mut DomNode,
    mut new: DomNode,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) {
    // the promise, honored: the walk never descended, the diff never
    // traverses — the retained subtree IS the frame's truth here. Only
    // the SHELL is read: the parent stamped its flags and hints on the
    // promise again, and a parent that ran may have stamped them anew
    if let DomKind::Reuse { .. } = &new.kind {
        crate::stats::note_diff_reuse();
        let id = retained.id;
        let before = patches.len();
        if let Some(layout) = &new.layout
            && retained.layout.as_ref() != Some(layout)
        {
            // the stamp changed: the look may have (a flag), the box
            // may have (a pin) — each travels its own road
            let was = retained.layout.as_ref().map(geometry_of).unwrap_or_default();
            retained.layout = Some(layout.clone());
            let rule = look_hash(retained);
            if rule != retained.rule {
                define_rule(rule, retained, ctx, patches);
                patches.push(DomPatch::UseRule { id, rule });
                retained.rule = rule;
            }
            let now = geometry_of(layout);
            if now != was {
                patches.push(box_patch(id, now));
            }
        }
        if hints_changed(retained, &new) {
            patches.push(DomPatch::SetHints {
                id,
                class: new.hints.class.clone(),
                address: new.hints.address.clone(),
            });
            retained.hints = new.hints.clone();
            if let Some(binding) = &new.binding {
                file_class_binding(id, binding, &new.hints, ctx);
            }
        }
        if patches.len() > before {
            ctx.templates.touched(id);
        }
        return;
    }
    crate::stats::note_diff_visit();
    let id = retained.id;
    let own_patches_from = patches.len();
    let old = &*retained;
    let new_children = std::mem::take(&mut new.children);
    // the look, by its hash — a rule the page has not seen is defined
    let rule = look_hash(&new);
    if rule != retained.rule {
        define_rule(rule, &new, ctx, patches);
        patches.push(DomPatch::UseRule { id, rule });
    }
    match &new.layout {
        // a flow node speaks semantics — its geometry fields are silent
        Some(layout) => {
            let was = old.layout.as_ref().map(geometry_of);
            let now = geometry_of(layout);
            if was != Some(now) {
                patches.push(box_patch(id, now));
            }
        }
        None => {
            // an absolute node that WAS flow clears its box first
            if old.layout.is_some() {
                patches.push(box_patch(id, [None; 5]));
            }
            if (old.x, old.y) != (new.x, new.y) {
                patches.push(DomPatch::SetTransform { id, x: new.x, y: new.y });
            }
            if (old.width, old.height) != (new.width, new.height) {
                patches.push(DomPatch::SetSize { id, width: new.width, height: new.height });
            }
        }
    }
    if old.style.marks() != new.style.marks() {
        let (tooltip, group_owner) = marks_of(&new.style);
        patches.push(DomPatch::SetMarks { id, tooltip, group_owner });
    }
    // a path that changed on a kept element ships whole: the base it
    // could be told against may never have been written on the page
    if old.style.interactive != new.style.interactive {
        patches.push(DomPatch::SetPath { id, path: new.style.interactive.clone(), base_len: 0 });
    }
    let same_binding = same_binding(old, &new);
    if hints_changed(old, &new) {
        // class and id re-attribute live; a TAG change would need a
        // recreation and the walk never changes one on a kept identity
        patches.push(DomPatch::SetHints {
            id,
            class: new.hints.class.clone(),
            address: new.hints.address.clone(),
        });
    }
    if let (DomKind::Group { .. }, Some(binding)) = (&new.kind, &new.binding)
        && (!same_binding || hints_changed(old, &new))
    {
        file_class_binding(id, binding, &new.hints, ctx);
    }
    match (&old.kind, &new.kind) {
        (DomKind::Text(before), DomKind::Text(after)) => {
            // the face is the look's; the words are the element's. The
            // words of a text that reads for itself travel on their own
            // road: the same binding object means the same reads, and
            // the retained record may lag the patch already shipped
            let changed = if same_binding {
                before.highlights != after.highlights
            } else {
                before.content != after.content || before.highlights != after.highlights
            };
            if changed {
                patches.push(text_words_patch(id, after));
            }
            match &new.binding {
                Some(binding) if changed || !same_binding => file_binding(id, binding, after, ctx),
                _ => {}
            }
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
        (_, DomKind::Canvas { .. }) => note_island(id, &new, ctx),
        (DomKind::Image(before), DomKind::Image(after)) if before != after => {
            patches.push(DomPatch::SetImage { id, image: *after });
        }
        (DomKind::Icon(before), DomKind::Icon(after)) if before != after => {
            patches.push(DomPatch::SetIcon { id, icon: *after });
        }
        (
            DomKind::Iframe { src: before, sealed: was },
            DomKind::Iframe { src: after, sealed: is },
        ) if before != after || was != is => {
            patches.push(DomPatch::SetIframe { id, src: std::rc::Rc::clone(after), sealed: *is });
        }
        // the whole record rides on any change; the glue keeps the
        // playback where the stream is the same
        (DomKind::Video { .. }, DomKind::Video { stream, mirrored, cover, radius })
            if old.kind != new.kind =>
        {
            patches.push(DomPatch::SetVideo {
                id,
                stream: *stream,
                mirrored: *mirrored,
                cover: *cover,
                radius: *radius,
            });
        }
        _ => {}
    }
    // a patch other than its words reached a template's member: the
    // live instance is no longer the shape
    if patches.len() > own_patches_from {
        ctx.templates.touched(id);
    }
    let previous_target = old_kind_for_reveal(retained);
    let followed = match (&previous_target, &new.kind) {
        // the region follows an item: a CHANGED target reveals it —
        // virtual rows by their slot (they may not exist yet), dense
        // rows by the browser's own scrollIntoView
        (
            Some(before),
            DomKind::Scroll { target: Some(after), .. },
        ) if before.as_deref() != Some(after.as_str()) => Some(after.clone()),
        (None, DomKind::Scroll { target: Some(after), .. }) => Some(after.clone()),
        _ => None,
    };
    let flow = new.layout.is_some();
    // the node moves into the retention under the id it had, over the
    // children the diff reconciles next: its own were taken above
    new.id = id;
    new.rule = rule;
    new.children = std::mem::take(&mut retained.children);
    *retained = new;
    diff_children(retained, new_children, flow, ctx, patches);
    if let Some(target) = followed {
        reveal_target(retained, &target, patches);
    }
}

/// The retained Scroll's PREVIOUS target (the caller captures it before
/// the new node moves in) — `None` when the node is not a scroll region.
fn old_kind_for_reveal(retained: &DomNode) -> Option<Option<String>> {
    match &retained.kind {
        DomKind::Scroll { target, .. } => Some(target.clone()),
        _ => None,
    }
}

/// Emits the reveal for `target` under an already-diffed scroll node:
/// a virtual row scrolls to its slot, a dense row asks the browser.
/// The retention is the frame's truth by now: it holds the slots and
/// the elements both.
fn reveal_target(retained: &DomNode, target: &str, patches: &mut Vec<DomPatch>) {
    let suffix = format!("[{target}]");
    let slot = retained
        .children
        .first()
        .into_iter()
        .flat_map(|content| content.children.iter())
        .find_map(|row| match &row.kind {
            DomKind::Group { path } if path.ends_with(&suffix) => {
                row.layout.as_ref().and_then(|layout| layout.slot_y)
            }
            _ => None,
        });
    match slot {
        Some(y) => patches.push(DomPatch::SetScroll { id: retained.id, x: 0.0, y: y.into() }),
        None => {
            let row_id = retained
                .children
                .first()
                .into_iter()
                .flat_map(|content| content.children.iter())
                .find_map(|row| match &row.kind {
                    DomKind::Group { path } if path.ends_with(&suffix) => Some(row.id),
                    _ => None,
                });
            if let Some(row) = row_id {
                patches.push(DomPatch::Reveal { id: retained.id, target: row });
            }
        }
    }
}

/// Matches the children lists: groups by identity path (a slid window
/// keeps its rows), everything else by position and kind. Unmatched old
/// children leave; unmatched new ones mount.
fn diff_children(
    retained: &mut DomNode,
    new_children: Vec<DomNode>,
    flow: bool,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) {
    // under a FLOW parent sibling order IS the layout, so the matching
    // must also reconcile positions; under an absolute parent order
    // only decides paint stacking and the old path stays byte-for-byte
    if flow {
        diff_children_ordered(retained, new_children, ctx, patches);
        return;
    }
    let new_len = new_children.len();
    let old_children = std::mem::take(&mut retained.children);
    let mut by_path: motor::hash::FxHashMap<std::rc::Rc<str>, DomNode> = motor::hash::FxHashMap::default();
    let mut by_index: Vec<Option<DomNode>> = Vec::with_capacity(old_children.len());
    for old in old_children {
        if let DomKind::Group { path } = &old.kind {
            by_path.insert(path.clone(), old);
            by_index.push(None);
        } else {
            by_index.push(Some(old));
        }
    }

    let mut next: Vec<DomNode> = Vec::with_capacity(new_len);
    let mut survivors = 0usize;
    // the template the sibling made just before is an instance of —
    // only a sibling made in this walk; a kept one breaks the run
    let mut template = None;
    for (index, child) in new_children.into_iter().enumerate() {
        let matched = match &child.kind {
            DomKind::Group { path } | DomKind::Reuse { path } => by_path.remove(path),
            // a kind change at the same index is remove+create — the
            // mismatched retained goes BACK to its slot so the leftover
            // sweep emits its remove (taking and filtering would drop
            // it silently and leak the element on the browser's side)
            kind => by_index.get_mut(index).and_then(|slot| match slot.take() {
                Some(old)
                    if std::mem::discriminant(&old.kind)
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
                survivors += 1;
                template = None;
            }
            None => {
                let mut child = child;
                let sibling = next.last().zip(template);
                template = create_subtree(&mut child, retained.id, ctx, patches, sibling, None);
                next.push(child);
            }
        }
    }

    let leaving: Vec<DomNode> =
        by_path.into_values().chain(by_index.into_iter().flatten()).collect();
    let left = !leaving.is_empty();
    if survivors == 0 && left {
        // every old child left and none stayed: one op empties the parent
        remove_all_children(retained.id, leaving, ctx, patches);
    } else {
        for leftover in leaving {
            remove_subtree(leftover, ctx, patches);
        }
    }
    if survivors < new_len || left {
        ctx.templates.touched(retained.id);
    }
    retained.children = next;
}

/// The keyed, ORDERED reconciliation a flow parent needs. Groups match
/// by identity path, everything else by old position and kind — and
/// then position itself reconciles: fresh children are created back to
/// front, each placed `before` its already-real next sibling (an
/// insert costs zero moves), while surviving children off the longest
/// increasing subsequence of their old order move with one `Move`
/// each — a swap of two rows is exactly two patches.
///
/// The ends that kept their place never enter the plan: a row appended,
/// removed or replaced leaves the rows before it where they stood, and
/// the rows after it too, found by path from the back. Only the middle
/// is matched, and the old rows stay in their list while it is: a
/// survivor is a position, diffed where it stands. A middle that only
/// reordered is put in order in place, row by row along its cycles —
/// a swap of two rows trades two rows, and the thousand between them
/// never move.
fn diff_children_ordered(
    retained: &mut DomNode,
    new_children: Vec<DomNode>,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) {
    let old_len = retained.children.len();
    let new_len = new_children.len();
    let shortest = old_len.min(new_len);
    // the head: every child that takes the old one at its own place
    let head = (0..shortest)
        .take_while(|&at| takes_place(&retained.children[at].kind, &new_children[at].kind))
        .count();
    // the ALIGNED fast path: same length, every child matching its
    // old position (groups by path, the rest by kind) — the shape of
    // almost every frame. One plain loop, zero allocation; the keyed
    // machinery below only runs when something actually reordered,
    // mounted or left.
    let aligned = old_len == new_len
        && (head..old_len).all(|at| {
            match (&retained.children[at].kind, &new_children[at].kind) {
                (DomKind::Group { path: was }, DomKind::Group { path: now }) => was == now,
                // a reuse promise aligns with the group it promised
                (DomKind::Group { path: was }, DomKind::Reuse { path: now }) => was == now,
                (old_kind, new_kind) => {
                    std::mem::discriminant(old_kind) == std::mem::discriminant(new_kind)
                }
            }
        });
    if aligned {
        for (old, child) in retained.children.iter_mut().zip(new_children) {
            diff_node(old, child, ctx, patches);
        }
        return;
    }
    // the tail, from the back. Where the lengths differ, a place moved
    // with the rows before it and only a path still names a row; where
    // they do not, the places are the same ones and match as the head's
    let shifted = old_len != new_len;
    let tail = (0..shortest - head)
        .take_while(|&back| {
            let was = &retained.children[old_len - 1 - back].kind;
            let now = &new_children[new_len - 1 - back].kind;
            (!shifted || matches!(now, DomKind::Group { .. } | DomKind::Reuse { .. }))
                && takes_place(was, now)
        })
        .count();
    let (old_end, new_end) = (old_len - tail, new_len - tail);
    let parent = retained.id;
    // a list that was empty: every child is fresh, made where it stands,
    // back to front — the scene's vector becomes the retention's (what
    // it holds beyond them given back, as `retain` does), and no node
    // moves
    if old_len == 0 {
        ctx.templates.touched(parent);
        let mut children = new_children;
        create_back_to_front(&mut children, parent, 0, ctx, patches);
        children.shrink_to_fit();
        retained.children = children;
        return;
    }
    let mut nodes = new_children.into_iter();
    for old in &mut retained.children[..head] {
        diff_node(old, nodes.next().expect("the head is in the new list"), ctx, patches);
    }

    // INSERTED, and nothing else: the fresh rows mount before the tail
    if head == old_end {
        if tail == 0 {
            // they end the list: they join it as they came and are made
            // where they stand, back to front
            ctx.templates.touched(parent);
            retained.children.extend(nodes);
            create_back_to_front(&mut retained.children[head..], parent, 0, ctx, patches);
            return;
        }
        let mut fresh: Vec<DomNode> = nodes.by_ref().take(new_end - head).collect();
        for old in &mut retained.children[old_end..] {
            diff_node(old, nodes.next().expect("the tail is in the new list"), ctx, patches);
        }
        ctx.templates.touched(parent);
        let anchor = retained.children[old_end].id;
        create_back_to_front(&mut fresh, parent, anchor, ctx, patches);
        put_made(&mut retained.children, head, fresh);
        return;
    }

    // REMOVED, and nothing else: the tail closes up behind the rows that
    // left. When nothing at all survives, the parent is emptied in one
    // op — a list that clears its rows says one word, not one per row
    if head == new_end {
        for old in &mut retained.children[old_end..] {
            diff_node(old, nodes.next().expect("the tail is in the new list"), ctx, patches);
        }
        if head == 0 && tail == 0 {
            remove_all_children(parent, std::mem::take(&mut retained.children), ctx, patches);
        } else {
            for leftover in retained.children.drain(head..old_end) {
                remove_subtree(leftover, ctx, patches);
            }
        }
        ctx.templates.touched(parent);
        return;
    }

    // the middle, matched — creation waits for the placement walk
    // below. The plan holds each new child's old POSITION, or FRESH;
    // the fresh nodes wait in order beside it
    let mut plan: Vec<usize> = Vec::with_capacity(new_end - head);
    let mut fresh: Vec<DomNode> = Vec::new();
    let mut claimed: Vec<bool> = vec![false; old_end - head];
    // the old rows that may have moved, indexed by path only when a new
    // child does not find its row at its own position — a list that
    // kept its order never asks, and a swap indexes the two rows that
    // traded places, not the rows that stayed
    let mut moved: Option<motor::hash::FxHashMap<std::rc::Rc<str>, usize>> = None;
    for index in head..new_end {
        let child = nodes.next().expect("the middle is in the new list");
        let matched: Option<usize> = match &child.kind {
            DomKind::Group { path } | DomKind::Reuse { path } => {
                let at_place = index < old_end
                    && matches!(
                        &retained.children[index].kind,
                        DomKind::Group { path: there } if there == path
                    );
                if at_place {
                    Some(index)
                } else {
                    moved
                        .get_or_insert_with(|| {
                            rows_that_may_move(
                                &retained.children,
                                head..old_end,
                                index,
                                &claimed,
                                nodes.as_slice(),
                                new_end,
                            )
                        })
                        .remove(path)
                }
            }
            kind => (index < old_end
                && std::mem::discriminant(&retained.children[index].kind)
                    == std::mem::discriminant(kind))
            .then_some(index),
        };
        match matched {
            Some(position) if !claimed[position - head] => {
                claimed[position - head] = true;
                diff_node(&mut retained.children[position], child, ctx, patches);
                plan.push(position);
            }
            _ => {
                // sized once, by the first: the rest of the middle may
                // all be fresh (a list that replaced its rows), and a
                // node is too big to be copied on every doubling
                if fresh.is_empty() {
                    fresh.reserve_exact(new_end - index);
                }
                fresh.push(child);
                plan.push(FRESH);
            }
        }
    }
    for old in &mut retained.children[old_end..] {
        diff_node(old, nodes.next().expect("the tail is in the new list"), ctx, patches);
    }
    let survivors = plan.len() - fresh.len();
    let next_after = |children: &[DomNode], at: usize| children.get(at).map_or(0, |next| next.id);

    // REORDERED, and nothing else: no row mounts, none leaves
    if fresh.is_empty() && survivors == old_end - head {
        let anchor = next_after(&retained.children, old_end);
        let middle = &mut retained.children[head..old_end];
        reorder_in_place(middle, head, plan, parent, anchor, patches);
        return;
    }

    // removals go out before placements: an anchor is never a corpse.
    // When nothing survived, the parent is emptied in one op — a list
    // that replaces its rows says one word, not one per row
    let mut kept: Vec<Option<DomNode>> = Vec::new();
    if survivors == 0 && head == 0 && tail == 0 {
        remove_all_children(parent, std::mem::take(&mut retained.children), ctx, patches);
    } else if survivors == 0 {
        for leftover in retained.children.drain(head..old_end) {
            remove_subtree(leftover, ctx, patches);
        }
    } else {
        kept = retained.children.drain(head..old_end).map(Some).collect();
        for (slot, taken) in kept.iter_mut().zip(&claimed) {
            if !taken && let Some(leftover) = slot.take() {
                remove_subtree(leftover, ctx, patches);
            }
        }
    }
    // a child mounted, left or moved under a template's member: the
    // live instance is no longer the shape
    ctx.templates.touched(parent);
    // Back to front: the anchor below is always already real — the
    // tail's first row, which the drain brought to `head`
    let mut anchor = next_after(&retained.children, head);

    // nothing survived (a list that replaced its rows): the middle is
    // the fresh nodes in their order, made where they wait and put in
    // the list as the vector they wait in
    if survivors == 0 {
        create_back_to_front(&mut fresh, parent, anchor, ctx, patches);
        put_made(&mut retained.children, head, fresh);
        return;
    }

    // the stable spine: survivors whose old order already reads in
    // increasing sequence stay put; everything else moves or mounts
    let stable = longest_increasing(&plan);
    let mut placed: Vec<DomNode> = Vec::with_capacity(plan.len());
    let mut fresh = fresh.into_iter();
    // the template the row made just before (the one BELOW, walking
    // back to front) is an instance of: the next fresh row is compared
    // with that row, never hashed. A survivor breaks the run
    let mut template = None;
    for (at, &position) in plan.iter().enumerate().rev() {
        if position == FRESH {
            let mut child = fresh.next_back().expect("a fresh node for every fresh entry");
            let sibling = placed.last().zip(template);
            template = create_subtree_before(&mut child, parent, anchor, ctx, patches, sibling);
            anchor = child.id;
            placed.push(child);
        } else {
            let node = kept[position - head].take().expect("a survivor is placed once");
            if !stable[at] {
                patches.push(DomPatch::Move { id: node.id, parent, before: anchor });
            }
            anchor = node.id;
            template = None;
            placed.push(node);
        }
    }
    put_placed(&mut retained.children, head, placed);
}

/// A plan entry with no old position: the child mounts.
const FRESH: usize = usize::MAX;

/// Does the new child take the old one at its own place? A group or a
/// reuse promise by its path, anything else by its kind.
fn takes_place(was: &DomKind, now: &DomKind) -> bool {
    match now {
        DomKind::Group { path } | DomKind::Reuse { path } => {
            matches!(was, DomKind::Group { path: there } if there == path)
        }
        kind => std::mem::discriminant(was) == std::mem::discriminant(kind),
    }
}

/// The old groups a new child of the middle may have come from, by
/// path: every one whose own place no new child takes by path. A row
/// that moved left its place to another row, so it is always here; a
/// row still waiting for the new child at its own place is not.
/// Asked once, by the first child (`asking`) that misses its place —
/// `ahead` is the new list after it.
fn rows_that_may_move(
    old: &[DomNode],
    middle: std::ops::Range<usize>,
    asking: usize,
    claimed: &[bool],
    ahead: &[DomNode],
    new_end: usize,
) -> motor::hash::FxHashMap<std::rc::Rc<str>, usize> {
    let mut rows = motor::hash::FxHashMap::default();
    let head = middle.start;
    for position in middle {
        let DomKind::Group { path } = &old[position].kind else {
            continue;
        };
        let waits = position > asking
            && position < new_end
            && takes_place(&old[position].kind, &ahead[position - asking - 1].kind);
        if !claimed[position - head] && !waits {
            rows.insert(std::rc::Rc::clone(path), position);
        }
    }
    rows
}

/// A middle that only reordered: the survivors off the stable spine
/// move, back to front before their next sibling, and then the rows
/// trade places in the list where they stand — cycle by cycle, one
/// swap per row out of place. A row the plan keeps where it was is
/// never touched.
fn reorder_in_place(
    middle: &mut [DomNode],
    head: usize,
    mut plan: Vec<usize>,
    parent: u32,
    mut anchor: u32,
    patches: &mut Vec<DomPatch>,
) {
    let stable = longest_increasing(&plan);
    for (at, &position) in plan.iter().enumerate().rev() {
        let id = middle[position - head].id;
        if !stable[at] {
            patches.push(DomPatch::Move { id, parent, before: anchor });
        }
        anchor = id;
    }
    // `plan[at]` names the row that belongs at `at`; a row put in its
    // place is marked by pointing at itself
    for position in &mut plan {
        *position -= head;
    }
    for start in 0..plan.len() {
        let mut at = start;
        let mut from = plan[at];
        while from != start {
            middle.swap(at, from);
            plan[at] = at;
            at = from;
            from = plan[at];
        }
        plan[at] = at;
    }
}

/// Mounts the fresh children where they stand, back to front: the last
/// before `anchor` and each one before the one made just below it, which
/// it is compared with. No node moves — the slice is the retention.
fn create_back_to_front(
    fresh: &mut [DomNode],
    parent: u32,
    mut anchor: u32,
    ctx: &mut LowerCtx,
    patches: &mut Vec<DomPatch>,
) {
    let mut template = None;
    for at in (0..fresh.len()).rev() {
        let (child, below) = fresh[at..].split_first_mut().expect("a child at every place");
        let sibling = below.first().zip(template);
        template = create_subtree_before(child, parent, anchor, ctx, patches, sibling);
        anchor = child.id;
    }
}

/// Puts the children a placement walk made back to front into the
/// list at `at`, in their order. An empty list takes them as they are.
fn put_placed(children: &mut Vec<DomNode>, at: usize, mut placed: Vec<DomNode>) {
    if children.is_empty() {
        placed.reverse();
        *children = placed;
    } else {
        children.splice(at..at, placed.into_iter().rev());
    }
}

/// Puts children made in their order into the list at `at`. An empty
/// list takes the vector they were made in as its own.
fn put_made(children: &mut Vec<DomNode>, at: usize, made: Vec<DomNode>) {
    if children.is_empty() {
        *children = made;
    } else {
        children.splice(at..at, made);
    }
}

/// Which entries of the plan stand on the longest increasing run of
/// old positions — the survivors that need no move. A fresh entry is
/// never on it. O(n log n), std only, and sized once.
fn longest_increasing(plan: &[usize]) -> Vec<bool> {
    const NONE: usize = usize::MAX;
    let mut tails: Vec<usize> = Vec::with_capacity(plan.len()); // indices into `plan`
    let mut parents: Vec<usize> = vec![NONE; plan.len()];
    for (at, &position) in plan.iter().enumerate() {
        if position == FRESH {
            continue;
        }
        // the tails rise, so a position past the last one extends the
        // run — the common case, a list that mostly kept its order: no
        // search for the rows that did
        let extends = tails.last().is_none_or(|&last| plan[last] < position);
        let place = if extends { tails.len() } else { tails.partition_point(|&tail| plan[tail] < position) };
        if place > 0 {
            parents[at] = tails[place - 1];
        }
        if place == tails.len() {
            tails.push(at);
        } else {
            tails[place] = at;
        }
    }
    let mut stable = vec![false; plan.len()];
    let mut cursor = tails.last().copied().unwrap_or(NONE);
    while cursor != NONE {
        stable[cursor] = true;
        cursor = parents[cursor];
    }
    stable
}


// MARK: - The wire encoding

/// The version of the wire contract between [`encode`] and the glue.
///
/// The glue is a hand-written mirror of this module, and the two bind
/// once, at page load. So the gate lives at boot: the shell exports
/// this number, the glue compares it with the number it was written
/// for, and refuses to start on a mismatch. A stale pairing dies with
/// one clear sentence — never with a `RangeError` half-way down a
/// stream it cannot read.
///
/// Bump this constant when ANY of these change:
/// - the op codes or their payloads (the table on [`encode`])
/// - the create kinds (0 group .. 15 video)
/// - the style mask bits or their field order
/// - the weight or truncation codes
/// - the key table or the modifier bits (the shell's `named_key`)
/// - the field padding the glue mirrors (`FIELD_PAD_V`/`FIELD_PAD_H`)
/// - the import surface: the modules' names or the verbs in them (the
///   glue's import object is keyed by those names)
/// - what the glue WRITES for a field of the records — a physical side
///   that becomes a logical one paints another page from the same bytes
///
/// A test pins the glue to this number: bump one side alone and the
/// suite goes red before the browser ever gets the chance to.
///
/// 9 (2026-09-14): the import modules are named as relative specifiers
/// (`./bunny.js`, `./bunny_gpu.js`) so a foreign ES-module loader resolves
/// them, and the shell gained `js_clipboard_write` and `js_set_cursor`.
///
/// 10 (2026-09-24): the flow record carries a wrapping row (bit 11 and
/// its line gap, after the slot), and the key table grew the function
/// row (101 to 124, `bunny_key` now answering whether a key was taken).
///
/// 11 (2026-10-02): op 18 empties an element (`RemoveChildren` — a list
/// that clears or replaces its rows is one word, with the id ranges it
/// forgets), a removal (op 2) unregisters its own subtree in the glue
/// instead of a sweep over every element at the end of the batch, op
/// 17 clones a shape already on the page (`Clone`, ids in pre-order
/// from the copy's), op 19 sets an action path alone (`SetPath`), op
/// 20 sets a text's words alone (`SetContent`), and the shell imports
/// `js_now` — the page's clock, for the stage table a `?stats` page
/// reads through `bunny_stats_*`.
///
/// 12 (2026-10-02): the flow record carries `plain` (bit 12, no payload):
/// an inline tag around one child keeps the browser's own display for
/// the tag instead of a flex line.
///
/// 13 (2026-10-02): an island's pixels arrive by the rect that changed
/// (`js_island_rect`, with the box's size and the rect's place and
/// size) instead of the whole box every time (`js_island` is gone).
///
/// 14 (2026-10-03): looks are shared. An element's style, flow record
/// and text face no longer travel inline per element (ops 5 and 11 are
/// gone, op 6 carries words and spans alone): a look is defined once
/// by its hash (op 21, `DefineRule`: kind, style, flow record, text
/// face) and worn by its class (op 22, `UseRule`); what is the element's
/// own travels beside it — its box (op 23, `SetBox`: pinned sizes,
/// ceilings, a virtual row's slot), its marks (op 24, `SetMarks`:
/// tooltip, hover group owned), its action path (op 19).
///
/// 15 (2026-10-03): a face is declared once and inherited. The text
/// look's record ends with one byte, `inherits_face`: 1 means the text
/// takes the face declared above it and its look names no font; the
/// root's look and the look of a box whose modifiers changed the face
/// carry that face as their text record, words aside.
///
/// 16 (2026-10-03): the video host — create kind 15 and op 25
/// (`SetVideo`), and the canvas shell's three host verbs
/// (`js_host_begin`, `js_host_video`, `js_host_end`) in the `./bunny.js`
/// module.
///
/// 17 (2026-10-03): action paths are told against their row. A path
/// under the nearest group at or above its element crosses as `~` and
/// the rest (op 19), and the group carries its own path as `data-base`:
/// a clone's root on the clone (op 17, u16 len + utf8 after the
/// template), any other group by op 26 (`SetBase`). A click resolves
/// `~` against the nearest `data-base` at or above the element, and a
/// clone whose relative paths read as its template's ships none.
///
/// 18 (2026-10-03): links. Create (op 1) and SetHints (op 15) carry a
/// fourth hint after the id — the href, u16 len + utf8, empty for none.
///
/// 19 (2026-10-03): tracking. The text look's record carries the face's
/// extra advance (f32 points, resolved) after the line height; the glue
/// writes it as `letter-spacing`.
///
/// 20 (2026-10-06): the page's writing. Op 27 (`SetLanguage`) puts `lang`
/// and `dir` on the mount — the first frame, and again when the locale
/// in effect moves; the flow record carries a direction of its own (bit
/// 13, u8: 0 ltr, 1 rtl) for an island that reads the other way, which
/// the glue writes as `direction` and an isolated `unicode-bidi`; the
/// record's four paddings read (top, trailing, bottom, leading) and are
/// written as `padding-block` and `padding-inline`; a trailing text
/// aligns to `end`; a popover's side crosses PHYSICAL, turned by the
/// walk where the anchor's direction is known; and a horizontal scroll
/// offset is logical on both sides of the wire — the glue negates
/// `scrollLeft` under a right-to-left mount, which reports it at or
/// below zero.
pub const ABI_VERSION: u32 = 20;

/// Encodes a patch list into the fixed little-endian stream the glue
/// decodes with one `DataView` walk. Layout:
///
/// ```text
/// u32 count
/// per patch: u8 op, u32 id, payload
///   1 create        u32 parent, u32 before (0 = append), three hint
///                   strings (u8 len + utf8 each: tag, class, id),
///                   the href (u16 len + utf8, 0 = none),
///                   u8 kind (0 group, 1 box, 2 text, 3 field,
///                            4 scroll, 5 content, 6 canvas, 7 image,
///                            8 icon, 9 flex column, 10 flex row,
///                            11 layers, 12 popover, 13 editor — the
///                            field of many lines, a `<textarea>`,
///                            14 iframe, 15 video)
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
///                   23 glass f32 blur, f32 saturation, f32 brightness,
///                      u32 rim rgba, f32 rim band — the half of the
///                      liquid-glass material a browser owns natively.
///                      The TINT is not here: it folds into bit 0 at
///                      capture time, because an element has one
///                      background colour and the tint sits under
///                      whatever the box paints itself
///   6 set text      u32 rgba, u8 inherits ink (1 = no color of its
///                   own — the box above owns both states),
///                   f32 size, u8 weight, u8 mono, u8 italic,
///                   u16 len + utf8 family (0 = the system's own face),
///                   u8 truncation (0 none, 1 start, 2 middle, 3 end),
///                   u32 len + utf8, u16 span count,
///                   spans (u32 start, u32 end), u32 span rgba
///   7 set field     u32 rgba text ink, f32 size, u8 weight, u8 mono,
///                   u8 italic,
///                   u16 len + utf8 family (0 = the system's own face),
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
///  11 set layout    (retired in 14: the record rides op 21) u16 mask,
///                   fields in bit order:
///                   0 gap f32,  1 align u8 (0 start, 1 center,
///                   2 end, 3 baseline),  2 padding f32 x4 (top,
///                   trailing, bottom, leading — logical sides, written
///                   as padding-block and padding-inline),
///                   3 width f32,  4 height f32,  5 max width f32,
///                   6 max height f32,  7 grow (flag, no payload),
///                   8 slot y f32 (a virtual row's offset),
///                   9 stretch (flag, no payload),
///                   10 fill (flag, no payload),
///                   11 wrap f32 (the line gap of a wrapping row),
///                   12 plain (flag, no payload),
///                   13 direction u8 (0 ltr, 1 rtl) — an island that
///                   reads the other way: `direction` and an isolated
///                   `unicode-bidi` on its box
///  12 move          u32 parent, u32 before (0 = to the end)
///  13 reveal        u32 target — the container scrolls it into view
///  14 set anchor    u32 anchor element, u8 side (0 top, 1 bottom,
///                   2 left, 3 right — PHYSICAL: the walk turns a
///                   leading side where the anchor's direction is
///                   known), u16 len + utf8 path — the browser
///                   positions the card from the anchor's real box,
///                   and the path keys the dismissal doors
///  15 set hints     two hint strings (u8 len + utf8 each: class, id)
///                   — the LIVE half of the hints, re-attributed in
///                   place. The tag never changes without a recreation
///  16 set iframe    u8 sealed, u32 len + utf8 — the url the frame
///                   navigates to, or (sealed) the DOCUMENT it holds
///                   as `srcdoc` inside a sandbox with no powers
///  17 clone         u32 parent, u32 before, u32 template, u16 len +
///                   utf8 base — the copy's own path when a path in
///                   it is told against it (0 = none)
///  19 set path      u16 len + utf8 — `~` and the rest when the path
///                   lies under its base (the nearest `data-base` at
///                   or above the element), the path whole otherwise
///  25 set video     u32 stream (the glue's handle; 0 = none), u8
///                   mirrored, u8 cover (1 = `object-fit: cover`, 0 =
///                   `contain`), f32 radius — the whole record; the
///                   glue rewires the element only when the stream
///                   changed, because a rewrite restarts playback
///  26 set base      u16 len + utf8 — a group's own path, the base
///                   the paths below it are told against: a created
///                   group's, or a group's inside a clone (its copied
///                   base names the template's)
///  27 set language  u8 dir (0 ltr, 1 rtl), u8 len + utf8 tag — the
///                   element's `lang` and `dir`: the mount's, on the
///                   first frame and when the locale in effect moves
/// ```
pub fn encode(patches: &[DomPatch]) -> Vec<u8> {
    crate::stats::time(crate::stats::Stage::Encode, || {
        let out = encode_unclocked(patches);
        crate::stats::note_encode(patches.len(), out.len());
        out
    })
}

fn encode_unclocked(patches: &[DomPatch]) -> Vec<u8> {
    let mut out = Vec::with_capacity(patches.len() * 16 + 4);
    push_u32(&mut out, patches.len() as u32);
    for patch in patches {
        match patch {
            DomPatch::Create { id, parent, before, kind, hints } => {
                out.push(1);
                push_u32(&mut out, *id);
                push_u32(&mut out, *parent);
                push_u32(&mut out, *before);
                push_hint(&mut out, hints.tag.as_deref());
                push_hint(&mut out, hints.class.as_deref());
                push_hint(&mut out, hints.dom_id());
                push_bytes_u16(&mut out, hints.href().unwrap_or("").as_bytes());
                out.push(kind_code(*kind));
            }
            DomPatch::Remove { id } => {
                out.push(2);
                push_u32(&mut out, *id);
            }
            DomPatch::Clone { id, parent, before, template, base } => {
                out.push(17);
                push_u32(&mut out, *id);
                push_u32(&mut out, *parent);
                push_u32(&mut out, *before);
                push_u32(&mut out, *template);
                push_bytes_u16(&mut out, base.as_deref().unwrap_or("").as_bytes());
            }
            DomPatch::SetPath { id, path, base_len } => {
                out.push(19);
                push_u32(&mut out, *id);
                match path {
                    // told against the base: `~` and the rest
                    Some(path) if *base_len > 0 => {
                        let rest = &path.as_bytes()[*base_len..];
                        push_u16(&mut out, (rest.len() + 1) as u16);
                        out.push(b'~');
                        out.extend_from_slice(rest);
                    }
                    path => push_bytes_u16(&mut out, path.as_deref().unwrap_or("").as_bytes()),
                }
            }
            DomPatch::SetBase { id, base } => {
                out.push(26);
                push_u32(&mut out, *id);
                push_bytes_u16(&mut out, base.as_bytes());
            }
            DomPatch::SetContent { id, text } => {
                out.push(20);
                push_u32(&mut out, *id);
                push_u32(&mut out, text.len() as u32);
                out.extend_from_slice(text.as_bytes());
            }
            DomPatch::RemoveChildren { id, forget } => {
                out.push(18);
                push_u32(&mut out, *id);
                push_u16(&mut out, forget.len().min(u16::MAX as usize) as u16);
                for (start, end) in forget.iter().take(u16::MAX as usize) {
                    push_u32(&mut out, *start);
                    push_u32(&mut out, *end);
                }
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
            DomPatch::SetText { id, text } => {
                // the words and the spans: the face is the look's
                out.push(6);
                push_u32(&mut out, *id);
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
                push_family(&mut out, &field.font);
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
                    // a forced drawing hands over NO colour of its
                    // own, which is what makes every path inherit the
                    // element's ink — the browser already had the rule
                    match draw.tint.filter(|_| !icon.forced) {
                        Some(tint) => {
                            out.push(1);
                            push_u32(&mut out, pack_color(tint));
                        }
                        None => out.push(0),
                    }
                    push_bytes_u32(&mut out, crate::icon::to_svg_path(draw.path).as_bytes());
                }
            }
            DomPatch::SetIframe { id, src, sealed } => {
                out.push(16);
                push_u32(&mut out, *id);
                out.push(u8::from(*sealed));
                push_bytes_u32(&mut out, src.as_bytes());
            }
            DomPatch::SetVideo { id, stream, mirrored, cover, radius } => {
                out.push(25);
                push_u32(&mut out, *id);
                push_u32(&mut out, *stream);
                out.push(u8::from(*mirrored));
                out.push(u8::from(*cover));
                push_f32(&mut out, *radius as f64);
            }
            DomPatch::DefineRule { rule, kind, flags, style, layout, text } => {
                out.push(21);
                push_u32(&mut out, (*rule >> 32) as u32);
                push_u32(&mut out, *rule as u32);
                out.push(kind_code(*kind));
                out.push(*flags);
                encode_style(&mut out, style);
                encode_layout(&mut out, layout);
                match text {
                    Some(text) => {
                        out.push(1);
                        encode_text_look(&mut out, text);
                    }
                    None => out.push(0),
                }
            }
            DomPatch::UseRule { id, rule } => {
                out.push(22);
                push_u32(&mut out, *id);
                push_u32(&mut out, (*rule >> 32) as u32);
                push_u32(&mut out, *rule as u32);
            }
            DomPatch::SetBox { id, width, height, max_width, max_height, slot_y } => {
                out.push(23);
                push_u32(&mut out, *id);
                let fields = [width, height, max_width, max_height, slot_y];
                let mut mask = 0u8;
                for (bit, field) in fields.iter().enumerate() {
                    if field.is_some() {
                        mask |= 1 << bit;
                    }
                }
                out.push(mask);
                for field in fields.into_iter().flatten() {
                    push_f32(&mut out, *field as f64);
                }
            }
            DomPatch::SetMarks { id, tooltip, group_owner } => {
                out.push(24);
                push_u32(&mut out, *id);
                let mut mask = 0u8;
                if tooltip.is_some() {
                    mask |= 1;
                }
                if group_owner.is_some() {
                    mask |= 2;
                }
                out.push(mask);
                if let Some(tooltip) = tooltip {
                    push_bytes_u16(&mut out, tooltip.as_bytes());
                }
                if let Some(owner) = group_owner {
                    push_u32(&mut out, (*owner >> 32) as u32);
                    push_u32(&mut out, *owner as u32);
                }
            }
            DomPatch::Move { id, parent, before } => {
                out.push(12);
                push_u32(&mut out, *id);
                push_u32(&mut out, *parent);
                push_u32(&mut out, *before);
            }
            DomPatch::Reveal { id, target } => {
                out.push(13);
                push_u32(&mut out, *id);
                push_u32(&mut out, *target);
            }
            DomPatch::SetAnchor { id, anchor, side, path } => {
                out.push(14);
                push_u32(&mut out, *id);
                push_u32(&mut out, *anchor);
                out.push(*side);
                push_bytes_u16(&mut out, path.as_bytes());
            }
            DomPatch::SetHints { id, class, address } => {
                let address = address.as_deref();
                out.push(15);
                push_u32(&mut out, *id);
                push_hint(&mut out, class.as_deref());
                push_hint(&mut out, address.and_then(|address| address.dom_id.as_deref()));
                push_bytes_u16(
                    &mut out,
                    address.and_then(|address| address.href.as_deref()).unwrap_or("").as_bytes(),
                );
            }
            DomPatch::SetLanguage { id, lang, dir } => {
                out.push(27);
                push_u32(&mut out, *id);
                out.push(dir.is_rtl() as u8);
                push_hint(&mut out, Some(lang));
            }
        }
    }
    out
}

/// A look's style record. The format keeps a bit for the element's own
/// action path (7), tooltip (15) and owned group (20): a look never
/// sets them — those travel as `SetPath` and `SetMarks`.
fn encode_style(out: &mut Vec<u8>, style: &DomLook) {
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
    if style.pass_through {
        // payload-free, like the clip bit: the bit IS the value
        mask |= 1 << 21;
    }
    if style.glass.is_some() {
        mask |= 1 << 23;
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
    if let Some(radii) = style.corner_radius.filter(|radii| radii.uniform().is_none()) {
        push_f32(out, radii.top_left);
        push_f32(out, radii.top_right);
        push_f32(out, radii.bottom_right);
        push_f32(out, radii.bottom_left);
    }
    if let Some(glass) = style.glass {
        push_f32(out, glass.blur);
        push_f32(out, glass.saturation);
        push_f32(out, glass.brightness);
        push_u32(out, pack_color(glass.rim));
        push_f32(out, glass.rim_band);
    }
}

fn encode_layout(out: &mut Vec<u8>, layout: &DomLayout) {
                let mut mask = 0u16;
                if layout.gap.is_some() {
                    mask |= 1;
                }
                if layout.align.is_some() {
                    mask |= 1 << 1;
                }
                if layout.padding.is_some() {
                    mask |= 1 << 2;
                }
                if layout.width.is_some() {
                    mask |= 1 << 3;
                }
                if layout.height.is_some() {
                    mask |= 1 << 4;
                }
                if layout.max_width.is_some() {
                    mask |= 1 << 5;
                }
                if layout.max_height.is_some() {
                    mask |= 1 << 6;
                }
                if layout.grow {
                    mask |= 1 << 7;
                }
                if layout.slot_y.is_some() {
                    mask |= 1 << 8;
                }
                if layout.stretch {
                    mask |= 1 << 9;
                }
                if layout.fill {
                    mask |= 1 << 10;
                }
                if layout.wrap.is_some() {
                    mask |= 1 << 11;
                }
                if layout.plain {
                    mask |= 1 << 12;
                }
                if layout.direction.is_some() {
                    mask |= 1 << 13;
                }
                push_u16(out, mask);
                if let Some(gap) = layout.gap {
                    push_f32(out, gap);
                }
                if let Some(align) = layout.align {
                    out.push(align);
                }
                if let Some((top, trailing, bottom, leading)) = layout.padding {
                    push_f32(out, top);
                    push_f32(out, trailing);
                    push_f32(out, bottom);
                    push_f32(out, leading);
                }
                if let Some(width) = layout.width {
                    push_f32(out, width);
                }
                if let Some(height) = layout.height {
                    push_f32(out, height);
                }
                if let Some(max_width) = layout.max_width {
                    push_f32(out, max_width);
                }
                if let Some(max_height) = layout.max_height {
                    push_f32(out, max_height);
                }
                if let Some(slot_y) = layout.slot_y {
                    push_f32(out, slot_y);
                }
                if let Some(wrap) = layout.wrap {
                    push_f32(out, wrap);
                }
                if let Some(direction) = layout.direction {
                    out.push(direction.is_rtl() as u8);
                }
}

/// A text's face and ink, as a look carries them.
fn encode_text_look(out: &mut Vec<u8>, text: &DomText) {
    push_u32(out, pack_color(text.color));
    // 1 = take no color of your own; the box above owns it
    out.push(text.inherits_ink as u8);
    push_f32(out, text.font.size);
    out.push(weight_code(text.font.weight));
    out.push(matches!(text.font.design, FontDesign::Mono) as u8);
    out.push(matches!(text.font.slant, crate::text_engine::Slant::Italic) as u8);
    push_family(out, &text.font);
    // the line box, or 0 for "the face's own" — the browser steps its
    // lines by the same number our placement does
    push_f32(out, text.line_height.unwrap_or(0.0));
    // the extra advance after every character, in points, resolved
    push_f32(out, text.font.tracking);
    // 0 leading (the default), 1 centre, 2 trailing
    out.push(match text.text_align {
        None | Some(motor::views::TextAlignment::Leading) => 0,
        Some(motor::views::TextAlignment::Center) => 1,
        Some(motor::views::TextAlignment::Trailing) => 2,
    });
    out.push(match text.truncation {
        None => 0,
        Some(Truncation::Start) => 1,
        Some(Truncation::Middle) => 2,
        Some(Truncation::End) => 3,
    });
    // 1 = the face is the one declared above: the look names no font
    out.push(text.inherits_face as u8);
}

/// The kind's byte on the wire, shared by the create op and the rule.
fn kind_code(kind: CreateKind) -> u8 {
    match kind {
        CreateKind::Group => 0,
        CreateKind::Box => 1,
        CreateKind::Text => 2,
        CreateKind::Field => 3,
        CreateKind::Scroll => 4,
        CreateKind::Content => 5,
        CreateKind::Canvas => 6,
        CreateKind::Image => 7,
        CreateKind::Icon => 8,
        CreateKind::FlexColumn => 9,
        CreateKind::FlexRow => 10,
        CreateKind::Layers => 11,
        CreateKind::Popover => 12,
        CreateKind::Editor => 13,
        CreateKind::Iframe => 14,
        CreateKind::Video => 15,
    }
}

fn weight_code(weight: Weight) -> u8 {
    match weight {
        Weight::Regular => 0,
        Weight::Medium => 1,
        Weight::Semibold => 2,
        Weight::Bold => 3,
        Weight::ExtraBold => 4,
        Weight::Black => 5,
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

/// One hint string: `u8` length + utf8 (0 = none). Hints are short by
/// construction — a tag or a class list, never content.
fn push_hint(out: &mut Vec<u8>, hint: Option<&str>) {
    match hint {
        Some(value) => {
            let bytes = value.as_bytes();
            let len = bytes.len().min(u8::MAX as usize);
            out.push(len as u8);
            out.extend_from_slice(&bytes[..len]);
        }
        None => out.push(0),
    }
}

/// Every number crosses as `f32` — the scene's own `f32`s exactly, an
/// `f64` rounded once.
fn push_f32(out: &mut Vec<u8>, value: impl Into<f64>) {
    out.extend_from_slice(&(value.into() as f32).to_le_bytes());
}

fn push_bytes_u32(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn push_bytes_u16(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u16(out, bytes.len() as u16);
    out.extend_from_slice(bytes);
}

/// The family's NAME, because the browser shapes by name and knows no
/// table of ours. An empty one is the face nobody named, which is
/// every run in a scene that never offers the choice — two bytes.
fn push_family(out: &mut Vec<u8>, font: &FontSpec) {
    match font.family.name() {
        Some(name) => push_bytes_u16(out, name.as_bytes()),
        None => push_u16(out, 0),
    }
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
            | DomPatch::RemoveChildren { id, .. }
            | DomPatch::Clone { id, .. }
            | DomPatch::SetPath { id, .. }
            | DomPatch::SetBase { id, .. }
            | DomPatch::SetContent { id, .. }
            | DomPatch::SetTransform { id, .. }
            | DomPatch::SetSize { id, .. }
            | DomPatch::UseRule { id, .. }
            | DomPatch::SetBox { id, .. }
            | DomPatch::SetMarks { id, .. }
            | DomPatch::SetText { id, .. }
            | DomPatch::SetField { id, .. }
            | DomPatch::SetImage { id, .. }
            | DomPatch::SetIcon { id, .. }
            | DomPatch::SetIframe { id, .. }
            | DomPatch::SetVideo { id, .. }
            | DomPatch::SetScroll { id, .. }
            | DomPatch::Move { id, .. }
            | DomPatch::Reveal { id, .. }
            | DomPatch::SetAnchor { id, .. }
            | DomPatch::SetHints { id, .. }
            | DomPatch::SetLanguage { id, .. } => *id,
            DomPatch::DefineRule { .. } => 0,
        }
    }

    /// The look an element wears at the end of the stream: the style
    /// and the flow record of the rule it was last told to wear.
    fn look_of(patches: &[DomPatch], wanted: u32) -> Option<(&DomLook, &DomLayout)> {
        let rule = patches.iter().rev().find_map(|patch| match patch {
            DomPatch::UseRule { id, rule } if *id == wanted => Some(*rule),
            _ => None,
        })?;
        patches.iter().find_map(|patch| match patch {
            DomPatch::DefineRule { rule: at, style, layout, .. } if *at == rule => {
                Some((&**style, &**layout))
            }
            _ => None,
        })
    }

    /// A look with this style alone, as a stream would define it.
    fn define(style: DomLook) -> DomPatch {
        DomPatch::DefineRule {
            rule: 0,
            kind: CreateKind::Box,
            flags: 0,
            style: Box::new(style),
            layout: Box::new(DomLayout::default()),
            text: None,
        }
    }

    /// Where a look's style mask starts in its encoding: after the
    /// count, the op, the rule's two words, the kind and the flags.
    const MASK_AT: usize = 4 + 1 + 8 + 1 + 1;
    /// What follows a look's style record when the flow record is bare
    /// and no text face rides along: the flow mask and the text flag.
    const BARE_TAIL: usize = 2 + 1;

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
                DomPatch::Create { id, parent, kind, .. } => Some((*id, *parent, *kind)),
                _ => None,
            })
            .collect();
        // one text per row plus the header — a row of a shape already
        // on the page is a clone of it, and its text arrives as a word
        let texts = creates.iter().filter(|(_, _, kind)| *kind == CreateKind::Text).count();
        let clones = patches.iter().filter(|patch| matches!(patch, DomPatch::Clone { .. })).count();
        assert_eq!(texts + clones, 4, "header + three rows: {creates:?}");
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
            matches!(patch, DomPatch::SetPath { path: Some(_), .. })
        });
        assert!(interactive, "rows are clickable in the scene");
    }

    /// The mount wears the page's language and the way it reads: op 27
    /// on the first frame, nothing on a still one, and one patch when
    /// the locale in effect moves.
    #[test]
    fn the_mount_carries_the_language_and_the_direction() {
        use motor::state::{LayoutDirection, Locale};
        let bytes = encode(&[DomPatch::SetLanguage {
            id: 0,
            lang: std::rc::Rc::from("ar"),
            dir: LayoutDirection::RightToLeft,
        }]);
        assert_eq!(bytes, [1, 0, 0, 0, 27, 0, 0, 0, 0, 1, 2, b'a', b'r']);

        let languages = |patches: &[DomPatch]| -> Vec<(u32, String, LayoutDirection)> {
            patches
                .iter()
                .filter_map(|patch| match patch {
                    DomPatch::SetLanguage { id, lang, dir } => Some((*id, lang.to_string(), *dir)),
                    _ => None,
                })
                .collect()
        };
        let (runtime, view, size) = mini();
        let first = runtime.dom_frame(&view, size);
        assert!(matches!(first[0], DomPatch::SetSize { id: 0, .. }), "the mount's size still leads");
        assert_eq!(languages(&first), [(0, "en".to_string(), LayoutDirection::LeftToRight)]);
        let still = runtime.dom_frame(&view, size);
        assert!(languages(&still).is_empty(), "a still frame says nothing: {still:?}");
        runtime.set_system_locale(Locale::new("ar"));
        let moved = runtime.dom_frame(&view, size);
        assert_eq!(languages(&moved), [(0, "ar".to_string(), LayoutDirection::RightToLeft)]);
    }

    /// A direction of an island's own is one byte more on the wire and
    /// another look to the sheet.
    #[test]
    fn a_direction_is_a_look_of_its_own() {
        use motor::state::LayoutDirection;
        let look = |direction| DomPatch::DefineRule {
            rule: 1,
            kind: CreateKind::Box,
            flags: 0,
            style: Box::default(),
            layout: Box::new(DomLayout { direction, ..DomLayout::default() }),
            text: None,
        };
        let plain = encode(&[look(None)]);
        let turned = encode(&[look(Some(LayoutDirection::RightToLeft))]);
        assert_eq!(turned.len(), plain.len() + 1, "one byte: the direction");
        // the record ends with the face byte (none here): the direction sits before it
        assert_eq!(turned[turned.len() - 2], 1, "right to left is 1");
        let mut inherits = flow_row("a");
        inherits.kind = DomKind::Box;
        let mut turned_node = flow_row("a");
        turned_node.kind = DomKind::Box;
        turned_node.layout = Some(DomLayout { direction: Some(LayoutDirection::LeftToRight), ..DomLayout::default() });
        assert_ne!(look_hash(&inherits), look_hash(&turned_node), "another rule");
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
        assert!(runtime.pointer_moved(target.0, target.1, false), "the hover state flipped");

        let patches = runtime.dom_frame(&view, size);
        assert_eq!(patches, vec![], "hover is the browser's — the scene never moves");
    }

    #[test]
    fn a_selection_change_patches_only_the_two_looks() {
        let (runtime, view, size) = mini();
        let _ = runtime.dom_frame(&view, size);

        view.selected.set(1);
        let patches = runtime.dom_frame(&view, size);

        assert!(!patches.is_empty());
        // a look the page has not seen is defined once; the rows wear
        for patch in &patches {
            assert!(
                matches!(patch, DomPatch::DefineRule { .. } | DomPatch::UseRule { .. }),
                "only looks move on a selection change: {patch:?}"
            );
        }
        let worn = patches.iter().filter(|patch| matches!(patch, DomPatch::UseRule { .. })).count();
        assert_eq!(worn, 2, "the old row and the new one: {patches:?}");
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

        // under the flow the browser reflows: the padded node changes
        // its ONE look (defined, then worn), and the component beside
        // it hears NOTHING — not even a transform
        let on_inner: Vec<_> =
            patches.iter().filter(|patch| patch_id(patch) >= inner_group).collect();
        assert!(on_inner.is_empty(), "the sibling never hears a padding: {patches:?}");
        assert_eq!(patches.len(), 2, "{patches:?}");
        assert!(matches!(
            &patches[0],
            DomPatch::DefineRule { layout, .. } if layout.padding == Some((12.0, 12.0, 12.0, 12.0))
        ));
        assert!(matches!(&patches[1], DomPatch::UseRule { .. }), "{patches:?}");
    }

    /// The browser owns the wheel in this mode: a scroll it reported
    /// folds into the retained scene BEFORE the next diff, so the
    /// frame meets its own echo and says nothing back.
    #[test]
    fn a_browser_scroll_echoes_to_silence() {
        let (runtime, view, size) = mini();
        view.count.set(30);
        let mount = runtime.dom_frame(&view, size);
        let scroll_id = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Scroll, .. } => Some(*id),
                _ => None,
            })
            .expect("a scroll region mounted");

        runtime.dom_scrolled(scroll_id, 0.0, 40.0);
        let patches = runtime.dom_frame(&view, size);
        assert!(patches.is_empty(), "the echo stays silent: {patches:?}");
    }

    /// The same region held in the app's own state, in this mode: the
    /// browser is the clamp and the observer is the report, so the
    /// binding tells the truth without the engine measuring anything.
    #[test]
    fn a_commanded_region_travels_both_ways_in_the_browser() {
        use crate::layout::Point;

        #[derive(Clone, Copy)]
        struct Page {
            at: State<Point>,
        }
        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(crate::views::for_each(
                    (0..30).collect::<Vec<i32>>(),
                    |line| line.to_string(),
                    |line| text(format!("line {line}")).frame(200.0, 20.0),
                ))
                .offset(self.at.binding())
            }
        }

        let runtime = Runtime::new();
        let page = Page { at: State::new(Point::default()) };
        let size = Size { width: 200.0, height: 150.0 };
        let mount = runtime.dom_frame(&page, size);
        let scroll_id = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Scroll, .. } => Some(*id),
                _ => None,
            })
            .expect("a scroll region mounted");

        // the browser scrolled: the binding hears where it landed, and
        // the echo is still silent
        runtime.dom_scrolled(scroll_id, 0.0, 40.0);
        let echo = runtime.dom_frame(&page, size);
        assert_eq!(page.at.get(), Point { x: 0.0, y: 40.0 }, "the app was told");
        assert!(echo.is_empty(), "and the echo stays silent: {echo:?}");

        // the app commands: one patch, and it is the offset
        page.at.set(Point { x: 0.0, y: 260.0 });
        let commanded = runtime.dom_frame(&page, size);
        assert!(
            commanded.iter().any(|patch| matches!(
                patch,
                DomPatch::SetScroll { id, y, .. } if *id == scroll_id && *y == 260.0
            )),
            "the browser is told where to go: {commanded:?}"
        );
    }

    #[test]
    fn a_virtual_jump_is_creates_removes_and_the_offset() {
        #[derive(Clone, Copy)]
        struct Big;

        impl Component for Big {
            fn body(self, _ctx: &Context) -> impl View {
                // the flow's one requirement: the app DECLARES the row
                // extent (the browser owns layout; nothing measures)
                virtual_list(10_000, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
                .row_height(20.0)
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 200.0, height: 150.0 };
        let mount = runtime.dom_frame(&Big, size);
        let scroll_id = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Scroll, .. } => Some(*id),
                _ => None,
            })
            .expect("the region mounted");

        runtime.dom_scrolled(scroll_id, 0.0, 50_000.0);
        let patches = runtime.dom_frame(&Big, size);

        let created: Vec<u32> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        // a window that jumped far has no row in common with the old
        // one: the content empties in one op, then the far band mounts
        let removes = patches
            .iter()
            .filter(|p| matches!(p, DomPatch::Remove { .. } | DomPatch::RemoveChildren { .. }))
            .count();
        assert!(!created.is_empty(), "the far band mounted");
        assert!(removes > 0, "the old window left");
        // surviving nodes sit at index × extent inside the content box —
        // a slid window never drags an existing element around (the only
        // transforms dress the freshly created ones)
        let moved_survivor = patches.iter().any(|patch| {
            matches!(patch, DomPatch::SetTransform { id, .. } | DomPatch::Move { id, .. }
                if !created.contains(id))
        });
        assert!(!moved_survivor, "nothing moves in content coordinates: {patches:?}");
        // the offset was the BROWSER's news — echoing it back would
        // fight the wheel
        assert!(
            !patches.iter().any(|p| matches!(p, DomPatch::SetScroll { .. })),
            "the reported offset never echoes: {patches:?}"
        );
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
            matches!(patch, DomPatch::DefineRule { style, .. }
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

        // the wire says 13, behind the flow vocabulary that took 9 —
        // the kind is the LAST byte of a create, after the parent, the
        // sibling it lands before, and the three hints
        let wire = encode(&[DomPatch::Create {
            id: 1,
            parent: 0,
            before: 0,
            kind: CreateKind::Editor,
            hints: DomHints::default(),
        }]);
        assert_eq!(
            *wire.last().expect("a create on the wire"),
            13,
            "the editor kind rides the stream: {wire:?}"
        );
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
                DomPatch::DefineRule { style, .. } if style.border.is_some() => Some(style),
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
            matches!(patch, DomPatch::DefineRule { style, .. } if style.hover_background.is_some())
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
                DomPatch::DefineRule { style, .. } if style.hover_color.is_some() => {
                    Some((style.color, style.hover_color))
                }
                _ => None,
            })
            .expect("the box declares the ink it hands down");
        assert_eq!(ink, (Some(FAINT), Some(BRIGHT)));
        // and the text takes NO color of its own: an inline one would
        // outrank the rule that flips it
        let inherits = patches.iter().any(|patch| {
            matches!(patch, DomPatch::DefineRule { text: Some(text), .. } if text.inherits_ink)
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
        assert!(runtime.pointer_moved(target.0, target.1, false), "the hover state flipped");
        assert_eq!(runtime.dom_frame(&CloseGlyph, size), vec![], "hover is the browser's");
    }

    #[cfg(feature = "canvas")]
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
        assert_eq!(
            (islands[0].width, islands[0].height),
            (20, 10),
            "the pixels match the island's box"
        );
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

    /// Rows that each hold an island leave — one alone, then the rest
    /// together, a list that clears in one op for the page — and their
    /// islands leave the registry in the frame that lets them go, while
    /// their groups and bindings wait for the idle: no island of theirs is
    /// painted, though none was painted yet when they left.
    #[cfg(feature = "canvas")]
    #[test]
    fn the_islands_of_rows_that_leave_are_never_painted() {
        #[derive(Clone, Copy)]
        struct Swatch(usize);

        impl Component for Swatch {
            fn body(self, _ctx: &Context) -> impl View {
                spacer()
                    .frame(10.0 + self.0 as f64, 10.0)
                    .background_color(Color::hex(0x3B82F6))
                    .rendering(Rendering::Gpu)
            }
        }

        #[derive(Clone, Copy)]
        struct Swatches {
            ids: State<Rc<Vec<usize>>>,
        }

        impl Component for Swatches {
            fn body(self, _ctx: &Context) -> impl View {
                crate::views::for_each(self.ids, |id| id.to_string(), |id| Swatch(*id))
            }
        }

        let runtime = Runtime::new();
        let page = Swatches { ids: State::new(Rc::new(vec![1, 2, 3, 4])) };
        let size = Size { width: 120.0, height: 80.0 };
        let mount = runtime.dom_frame(&page, size);
        let canvases = mount.iter().filter(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Canvas, .. })).count();
        assert_eq!(canvases, 4, "an island a row: {mount:?}");

        // one row leaves alone, then the rest together, before a single
        // pixel was asked for
        page.ids.set(Rc::new(vec![1, 3, 4]));
        let patches = runtime.dom_frame(&page, size);
        assert!(patches.iter().any(|patch| matches!(patch, DomPatch::Remove { .. })), "{patches:?}");
        page.ids.set(Rc::new(Vec::new()));
        let patches = runtime.dom_frame(&page, size);
        assert!(patches.iter().any(|patch| matches!(patch, DomPatch::RemoveChildren { .. })), "{patches:?}");
        assert!(runtime.dom_islands(1).is_empty(), "the islands that left are not painted");
        assert!(runtime.island_frames().is_empty());
    }

    /// A FLEXIBLE island guesses at mount, then the browser reports
    /// the box it really gave the element — the island re-measures
    /// against that box and the pixels agree with the element. The
    /// observer's echo of what the engine already said buys nothing.
    #[cfg(feature = "canvas")]
    /// An island keeps a paint target of its own: a change inside it
    /// repaints the rows it touched and ships that strip, byte for byte
    /// what a full repaint would put there; an identical frame ships
    /// nothing.
    #[test]
    fn an_island_repaints_its_damage_not_its_box() {
        #[derive(Clone)]
        struct Island {
            count: State<usize>,
        }

        impl Component for Island {
            fn body(self, _ctx: &Context) -> impl View {
                let count = self.count;
                crate::vstack!(
                    text("a fixed line"),
                    crate::text!("count {}", count.get()),
                    text("another fixed line"),
                )
                .frame(200.0, 90.0)
                .rendering(crate::layout::Rendering::Gpu)
            }
        }

        let size = Size { width: 240.0, height: 120.0 };
        let count = State::new(0usize);
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&Island { count }, size);
        let first = runtime.dom_islands(1);
        assert_eq!(first.len(), 1, "one island");
        let (width, height) = (first[0].width, first[0].height);
        assert_eq!(first[0].dirty, (0, 0, width as u32, height as u32), "the first frame is the whole box");
        assert_eq!(first[0].rgba.len(), width * height * 4);

        count.set(1);
        let _ = runtime.dom_frame(&Island { count }, size);
        let second = runtime.dom_islands(1);
        assert_eq!(second.len(), 1, "the island changed");
        let (x, y, dirty_width, dirty_height) = second[0].dirty;
        assert!(
            (dirty_height as usize) < height,
            "one line changed: a strip, not the box ({dirty_height} of {height})"
        );
        assert_eq!(second[0].rgba.len(), dirty_width as usize * dirty_height as usize * 4);

        // the strip is what a fresh runtime paints there
        let fresh = Runtime::new();
        let _ = fresh.dom_frame(&Island { count: State::new(1usize) }, size);
        let whole = fresh.dom_islands(1).remove(0);
        let mut expected = Vec::new();
        for row in y..y + dirty_height {
            let from = (row as usize * width + x as usize) * 4;
            expected.extend_from_slice(&whole.rgba[from..from + dirty_width as usize * 4]);
        }
        assert_eq!(second[0].rgba, expected, "the strip matches a full repaint byte for byte");

        // nothing moved: nothing ships
        let _ = runtime.dom_frame(&Island { count }, size);
        assert!(runtime.dom_islands(1).is_empty(), "an identical frame blits nothing");
    }

    #[test]
    fn a_flexible_island_takes_the_browsers_box() {
        #[derive(Clone)]
        struct WithFlexIsland;

        impl Component for WithFlexIsland {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(
                    text("above the island"),
                    spacer()
                        .background_color(Color::hex(0x3B82F6))
                        .rendering(Rendering::Gpu)
                )
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 120.0, height: 80.0 };
        let mount = runtime.dom_frame(&WithFlexIsland, size);
        let canvas_id = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Canvas, .. } => Some(*id),
                _ => None,
            })
            .expect("the island mounted");
        let _ = runtime.dom_islands(1);

        // the browser gave the flexible element ITS box
        assert!(
            runtime.dom_island_box(canvas_id, 300.0, 40.0),
            "a fresh box is news"
        );
        let patches = runtime.dom_frame(&WithFlexIsland, size);
        assert!(
            patches
                .iter()
                .all(|patch| !matches!(patch, DomPatch::Create { .. } | DomPatch::Remove { .. })),
            "the element already belongs to the browser — only pixels move: {patches:?}"
        );
        let islands = runtime.dom_islands(1);
        assert_eq!(islands.len(), 1, "fresh pixels at the reported size");
        assert_eq!((islands[0].width, islands[0].height), (300, 40));

        // the observer echoes what the engine now says — no frame
        assert!(
            !runtime.dom_island_box(canvas_id, 300.0, 40.0),
            "an echo is not news"
        );
    }

    /// A flexible island whose box is finer than an `f32`: the element's
    /// pin crosses at the wire's precision, and the box the island seeds
    /// is the one its pixels were sized at — the next walk measures
    /// against the same number, and pixels that did not change stay put.
    #[cfg(feature = "canvas")]
    #[test]
    fn a_fine_island_box_survives_the_next_walk() {
        #[derive(Clone)]
        struct FineIsland {
            count: State<usize>,
        }

        impl Component for FineIsland {
            fn body(self, _ctx: &Context) -> impl View {
                // a label that gives way: flexible along its row, and
                // natural against any offer
                let label = text("abc")
                    .truncation_mode(crate::layout::Truncation::End)
                    .padding_length(0.1);
                crate::vstack!(
                    text(format!("count {}", self.count.get())),
                    crate::hstack!(label).rendering(crate::layout::Rendering::Gpu),
                )
            }
        }

        let size = Size { width: 240.0, height: 120.0 };
        let count = State::new(0usize);
        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&FineIsland { count }, size);
        let fine = 24.0 + 2.0 * 0.1;
        assert!(f64::from(fine as f32) != fine, "the box is finer than an f32");
        assert!(
            mount.iter().any(|patch| matches!(
                patch,
                DomPatch::SetBox { width: Some(width), .. } if *width == fine as f32
            )),
            "the element is pinned at the natural width: {mount:?}"
        );
        assert_eq!(runtime.dom_island_lists(1).len(), 1, "the island paints once");

        // a walk the island takes part in, with nothing in it changed
        count.set(1);
        let _ = runtime.dom_frame(&FineIsland { count }, size);
        assert!(runtime.dom_island_lists(1).is_empty(), "the same box, the same pixels");
    }

    /// The Scratch pattern: a custom element whose measure EATS the
    /// width proposal. The island discovers that axis by probing two
    /// proposals, leaves it to the browser (`align-self: stretch`, no
    /// pinned width), and re-measures against the box the observer
    /// reports — the pixels and the element converge in one round.
    #[cfg(feature = "canvas")]
    #[test]
    fn a_hungry_island_stretches_and_takes_the_reported_box() {
        struct EatsWidth;

        impl crate::custom::CustomElement for EatsWidth {
            fn name(&self) -> &str {
                "eats-width"
            }
            fn measure(
                &self,
                proposal: crate::layout::Proposal,
                _metrics: &crate::custom::Metrics,
            ) -> Size {
                Size { width: proposal.width.unwrap_or(24.0), height: 30.0 }
            }
            fn paint(&self, ctx: &crate::custom::PaintCtx, painter: &mut crate::custom::Painter) {
                painter.fill(ctx.bounds(), Color::hex(0x3B82F6));
            }
        }

        #[derive(Clone)]
        struct WithHungryIsland;

        impl Component for WithHungryIsland {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(text("above"), crate::custom::custom(EatsWidth))
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 240.0, height: 120.0 };
        let mount = runtime.dom_frame(&WithHungryIsland, size);
        let canvas_id = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Canvas, .. } => Some(*id),
                _ => None,
            })
            .expect("the island mounted");
        // the stretch is the look's; the pinned height is the box's own
        let stretched = look_of(&mount, canvas_id).is_some_and(|(_, layout)| layout.stretch)
            && mount.iter().any(|patch| {
                matches!(
                    patch,
                    DomPatch::SetBox { id, width: None, height: Some(height), .. }
                        if *id == canvas_id && *height == 30.0
                )
            });
        assert!(stretched, "width is the browser's, height is pinned: {mount:?}");
        let _ = runtime.dom_islands(1);

        // the browser stretched the element and the observer reported
        assert!(runtime.dom_island_box(canvas_id, 500.0, 30.0));
        let _ = runtime.dom_frame(&WithHungryIsland, size);
        let islands = runtime.dom_islands(1);
        assert_eq!(islands.len(), 1);
        assert_eq!((islands[0].width, islands[0].height), (500, 30));
    }

    /// The window's box FLOWS DOWN: the mount point is a one-slot
    /// column, and a vertically flexible app takes the offer through
    /// every wrapper on the way (`fill`, `flex: 1 1 auto`) — the
    /// finder's padded panel reaches the bottom of the window, like
    /// the engine that proposes its box has always guaranteed.
    /// A row of chips that wraps is the browser's own flex row that wraps,
    /// with the chips' gap and the lines' gap both on the wire.
    #[test]
    fn a_wrapping_row_lowers_to_a_flex_row_that_wraps() {
        #[derive(Clone)]
        struct Chips;

        impl Component for Chips {
            fn body(self, _ctx: &Context) -> impl View {
                crate::hstack!(text("Objective D8"), text("Variables D2:D6"), text("Constraints 2"))
                    .spacing(6.0)
                    .line_spacing(4.0)
            }
        }

        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&Chips, Size { width: 272.0, height: 200.0 });
        let wraps = mount.iter().any(|patch| {
            matches!(
                patch,
                DomPatch::DefineRule { layout, .. }
                    if layout.wrap == Some(4.0) && layout.gap == Some(6.0)
            )
        });
        assert!(wraps, "the row wraps, both gaps on the record: {mount:?}");
        // and the record survives its own encoding: bit 11, its gap last
        let bytes = encode(&mount);
        assert!(!bytes.is_empty());
    }

    /// A face is declared once and inherited: the root's look carries
    /// the default face, a text with that face names none of its own,
    /// a text with another face declares it — and a box whose modifier
    /// changes the face declares it for everything under it.
    #[test]
    fn a_face_is_declared_once_and_inherited() {
        #[derive(Clone)]
        struct Page;

        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(
                    text("plain"),
                    text("big").font(Font::Title),
                    crate::vstack!(text("a"), text("b")).font_family("Menlo"),
                )
            }
        }

        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&Page, Size { width: 300.0, height: 200.0 });
        let looks: Vec<(CreateKind, Option<&DomText>)> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::DefineRule { kind, text, .. } => Some((*kind, text.as_deref())),
                _ => None,
            })
            .collect();
        // the root declares the default face
        let root = look_of(&mount, 0).map(|_| ()).is_some();
        assert!(root, "{mount:?}");
        assert!(
            mount.iter().any(|patch| matches!(patch,
                DomPatch::DefineRule { kind: CreateKind::Group, text: Some(text), .. }
                    if text.font == crate::text_engine::FontSpec::DEFAULT && !text.inherits_face)),
            "the root's look carries the default face: {looks:?}"
        );
        // the plain text inherits; the title declares
        let texts: Vec<&DomText> = looks
            .iter()
            .filter_map(|(kind, text)| (*kind == CreateKind::Text).then_some(*text).flatten())
            .collect();
        assert!(
            texts.iter().any(|text| text.inherits_face && text.font == crate::text_engine::FontSpec::DEFAULT),
            "a text with the default face names none: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| !text.inherits_face && text.font.size > crate::text_engine::FontSpec::DEFAULT.size),
            "a title names its face: {texts:?}"
        );
        // the Menlo box declares the family; its texts inherit it
        assert!(
            looks.iter().any(|(kind, text)| *kind != CreateKind::Text
                && text.is_some_and(|text| text.font.family.name().as_deref() == Some("Menlo"))),
            "the box declares the family it sets: {looks:?}"
        );
        assert!(
            texts.iter().any(|text| text.inherits_face && text.font.family.name().as_deref() == Some("Menlo")),
            "the texts under it inherit it: {texts:?}"
        );
    }

    /// A chain of hints is one hinted node, and the words a page hints
    /// with are held once: the same tag a thousand times is one copy.
    #[test]
    fn hints_fold_into_one_node_and_share_their_words() {
        let a = crate::modifier::hint("td");
        let b = crate::modifier::hint("td");
        assert!(std::rc::Rc::ptr_eq(&a, &b), "one copy of the word");

        #[derive(Clone)]
        struct Cell;

        impl Component for Cell {
            fn body(self, _ctx: &Context) -> impl View {
                text("x").element("a").css_class("lbl")
            }
        }

        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&Cell, Size { width: 100.0, height: 40.0 });
        let hints: Vec<&DomHints> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { kind: CreateKind::Text, hints, .. } => Some(hints),
                _ => None,
            })
            .collect();
        let [hints] = hints.as_slice() else {
            panic!("one text element: {mount:?}");
        };
        assert_eq!(hints.tag.as_deref(), Some("a"));
        assert_eq!(hints.class.as_deref(), Some("lbl"));
        // and the words are the shared ones
        assert!(hints.tag.as_ref().is_some_and(|tag| std::rc::Rc::ptr_eq(tag, &crate::modifier::hint("a"))));
    }

    /// A link around one word is no flex box: the record says `plain`,
    /// and the browser keeps the tag's own display. The cell that
    /// holds the link, a table cell, is not touched by the fold; the
    /// word itself, folded into the span, is not a box at all.
    #[test]
    fn an_inline_tag_around_one_child_is_no_flex_box() {
        #[derive(Clone)]
        struct Link;

        impl Component for Link {
            fn body(self, _ctx: &Context) -> impl View {
                crate::hstack!(
                    crate::hstack!(crate::hstack!(empty()).element("span").css_class("glyph"))
                        .element("a"),
                    crate::hstack!(text("one"), text("two")).element("a").css_class("pair"),
                )
                .element("td")
            }
        }

        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&Link, Size { width: 300.0, height: 100.0 });
        let a_ids: Vec<u32> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { id, hints, .. } if hints.tag.as_deref() == Some("a") => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(a_ids.len(), 2, "two links: {mount:?}");
        let plain_of = |id: u32| look_of(&mount, id).is_some_and(|(_, layout)| layout.plain);
        assert!(plain_of(a_ids[0]), "a link around one child is plain: {mount:?}");
        assert!(!plain_of(a_ids[1]), "a link around two children keeps its flex line: {mount:?}");
        // and the empty span inside the first link: no child, no flex
        // line — the page's stylesheet draws the glyph in its own font
        let span = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, hints, .. } if hints.tag.as_deref() == Some("span") => Some(*id),
                _ => None,
            })
            .expect("the span mounted");
        assert!(plain_of(span), "an empty inline tag is plain: {mount:?}");
        assert!(
            look_of(&mount, span).is_some_and(|(style, _)| style.color.is_none()),
            "and wears no face of ours: {mount:?}"
        );
        let bytes = encode(&mount);
        assert!(!bytes.is_empty());
    }

    /// A column around one table, on the leading edge, is a block: the
    /// table is laid out once, not measured as a flex item and laid out
    /// again. A centred column keeps its flex line (a block cannot
    /// centre), and so does a column around a table that fills it.
    #[test]
    fn a_lone_table_on_the_leading_edge_sits_in_a_block() {
        #[derive(Clone)]
        struct Tables;

        impl Component for Tables {
            fn body(self, _ctx: &Context) -> impl View {
                let table = |class: &'static str| {
                    crate::hstack!(crate::hstack!(text("cell")).element("tbody"))
                        .element("table")
                        .css_class(class)
                };
                crate::vstack!(
                    table("leading").frame_max(f64::INFINITY, f64::INFINITY, motor::views::Alignment::Leading),
                    table("centred").frame_max(f64::INFINITY, f64::INFINITY, motor::views::Alignment::Center),
                    table("filling")
                        .frame_max(f64::INFINITY, f64::INFINITY, motor::views::Alignment::Leading)
                        .frame_max(f64::INFINITY, f64::INFINITY, motor::views::Alignment::Leading),
                )
            }
        }

        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&Tables, Size { width: 600.0, height: 400.0 });
        let parent_of = |class: &str| {
            mount
                .iter()
                .find_map(|patch| match patch {
                    DomPatch::Create { parent, hints, .. }
                        if hints.tag.as_deref() == Some("table") && hints.class.as_deref() == Some(class) =>
                    {
                        Some(*parent)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("the {class} table mounted: {mount:?}"))
        };
        let plain_of = |id: u32| look_of(&mount, id).is_some_and(|(_, layout)| layout.plain);
        assert!(plain_of(parent_of("leading")), "the leading column is a block: {mount:?}");
        assert!(!plain_of(parent_of("centred")), "a centred column keeps its flex line: {mount:?}");
        // the inner frame of the doubled one is a block around the table;
        // the outer one holds a block, not a table, and stays a column
        let inner = parent_of("filling");
        assert!(plain_of(inner), "the frame right around the table is a block: {mount:?}");
        let outer = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, parent, .. } if *id == inner => Some(*parent),
                _ => None,
            })
            .expect("the inner frame mounted");
        assert!(!plain_of(outer), "a column around a block is still a column: {mount:?}");
        // the block is still an item of the column above it, and still
        // takes that line's offer
        assert!(
            look_of(&mount, inner).is_some_and(|(_, layout)| layout.grow || layout.fill),
            "the block still takes its own line's offer: {mount:?}"
        );
    }

    #[test]
    fn the_windows_box_flows_down_to_a_padded_panel() {
        #[derive(Clone)]
        struct Paned;

        impl Component for Paned {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(
                    text("toolbar"),
                    virtual_list(100, |row| format!("r{row}"), |row| {
                        text(format!("row {row}"))
                    })
                )
                .padding_length(28.0)
                .background_color(Color::hex(0x10141B))
            }
        }

        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&Paned, Size { width: 400.0, height: 300.0 });
        // the first element under the root carries the offer
        let first = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, parent: 0, .. } => Some(*id),
                _ => None,
            })
            .expect("the app mounted");
        let takes = |wanted: u32| look_of(&mount, wanted).is_some_and(|(_, layout)| layout.fill);
        assert!(takes(first), "the root child takes the window: {mount:?}");
    }

    /// The finder's exact shape, kept honest: a width-hungry custom
    /// under padding wrappers, between a toolbar and a virtual list.
    /// The browser's report must reach it through the whole chain.
    #[cfg(feature = "canvas")]
    #[test]
    fn a_report_reaches_an_island_behind_wrappers() {
        struct EatsRow;

        impl crate::custom::CustomElement for EatsRow {
            fn name(&self) -> &str {
                "eats-row"
            }
            fn flexible(&self, _axis: crate::layout::Axis) -> bool {
                false
            }
            fn measure(
                &self,
                proposal: crate::layout::Proposal,
                _metrics: &crate::custom::Metrics,
            ) -> Size {
                Size { width: proposal.width.unwrap_or(0.0), height: 46.0 }
            }
            fn paint(&self, ctx: &crate::custom::PaintCtx, painter: &mut crate::custom::Painter) {
                painter.fill(ctx.bounds(), Color::hex(0x3B82F6));
            }
        }

        #[derive(Clone)]
        struct Pane;

        impl Component for Pane {
            fn body(self, _ctx: &Context) -> impl View {
                use motor::views::Edge;
                crate::vstack!(crate::vstack!(
                    crate::hstack!(text("toolbar")),
                    crate::custom::custom(EatsRow)
                        .padding_edge(Edge::Leading, 10.0)
                        .padding_edge(Edge::Trailing, 10.0)
                        .padding_edge(Edge::Bottom, 8.0),
                    virtual_list(1_000, |row| format!("r{row}"), |row| {
                        text(format!("row {row}"))
                    })
                ))
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 760.0, height: 640.0 };
        let mount = runtime.dom_frame(&Pane, size);
        let canvas_id = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Canvas, .. } => Some(*id),
                _ => None,
            })
            .expect("the island mounted");
        let _ = runtime.dom_islands(1);

        assert!(
            runtime.dom_island_box(canvas_id, 682.0, 46.0),
            "the report is news"
        );
        let _ = runtime.dom_frame(&Pane, size);
        let islands = runtime.dom_islands(1);
        assert_eq!(islands.len(), 1, "the report re-rastered the island");
        assert_eq!((islands[0].width, islands[0].height), (682, 46));
    }

    /// The Scratch demo's whole life under flow: a click on the
    /// canvas reaches the app's box in the box's OWN coordinates,
    /// the release hands it the keyboard, and typing lands as text —
    /// the same doors the desktop and the canvas mode use.
    #[cfg(feature = "canvas")]
    #[test]
    fn a_click_on_an_island_reaches_the_apps_box() {
        #[derive(Clone)]
        struct Pad {
            mark: State<f64>,
            note: State<std::sync::Arc<str>>,
        }

        impl crate::custom::CustomElement for Pad {
            fn name(&self) -> &str {
                "pad"
            }
            fn flexible(&self, _axis: crate::layout::Axis) -> bool {
                false
            }
            fn accepts_keys(&self) -> bool {
                true
            }
            fn measure(
                &self,
                proposal: crate::layout::Proposal,
                _metrics: &crate::custom::Metrics,
            ) -> Size {
                Size { width: proposal.width.unwrap_or(200.0), height: 40.0 }
            }
            fn paint(&self, ctx: &crate::custom::PaintCtx, painter: &mut crate::custom::Painter) {
                painter.fill(ctx.bounds(), Color::hex(0x3B82F6));
                painter.fill(
                    Rect {
                        origin: Point { x: self.mark.get(), y: 0.0 },
                        size: Size { width: 2.0, height: 4.0 },
                    },
                    Color::hex(0xFFFFFF),
                );
            }
            fn event(
                &self,
                event: &crate::custom::ElementEvent,
                _ctx: &crate::custom::EventCtx,
            ) -> crate::custom::Response {
                match event {
                    crate::custom::ElementEvent::PointerDown { at, .. } => {
                        self.mark.set(at.x);
                        crate::custom::Response::handled()
                    }
                    crate::custom::ElementEvent::Text(text) => {
                        self.note
                            .set(std::sync::Arc::from(format!("{}{text}", self.note.get())));
                        crate::custom::Response::handled()
                    }
                    _ => crate::custom::Response::ignored(),
                }
            }
        }

        #[derive(Clone)]
        struct WithPad {
            mark: State<f64>,
            note: State<std::sync::Arc<str>>,
        }

        impl Component for WithPad {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(
                    text("above"),
                    crate::custom::custom(Pad { mark: self.mark, note: self.note })
                )
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 400.0, height: 200.0 };
        let view = WithPad {
            mark: State::new(0.0),
            note: State::new(std::sync::Arc::from("")),
        };
        let mount = runtime.dom_frame(&view, size);
        let canvas_id = mount
            .iter()
            .find_map(|patch| match patch {
                DomPatch::Create { id, kind: CreateKind::Canvas, .. } => Some(*id),
                _ => None,
            })
            .expect("the island mounted");
        let _ = runtime.dom_islands(1);

        // the press lands in the box's own coordinates
        assert!(runtime.dom_island_pointer(canvas_id, 0, 25.0, 10.0, false));
        assert_eq!(view.mark.get(), 25.0, "the box heard the press where it happened");
        assert!(runtime.dom_island_pointer(canvas_id, 2, 25.0, 10.0, false));

        // the release handed it the keyboard: typing reaches the box
        let answer = runtime.key(EditCommand::Insert("hi".into()));
        assert!(answer.applied, "the focused box types");
        assert_eq!(view.note.get().as_ref(), "hi");

        // and the pixels follow the state
        let _ = runtime.dom_frame(&view, size);
        assert_eq!(runtime.dom_islands(1).len(), 1, "fresh pixels follow the press");
    }

    #[test]
    fn the_encoding_is_byte_stable() {
        let patches = vec![
            DomPatch::Create {
                id: 7,
                parent: 0,
                before: 0,
                kind: CreateKind::Box,
                hints: DomHints::default(),
            },
            DomPatch::SetTransform { id: 7, x: 10.0, y: 20.0 },
            DomPatch::SetPath { id: 7, path: Some(std::rc::Rc::from("go")), base_len: 0 },
            DomPatch::Remove { id: 7 },
        ];
        let bytes = encode(&patches);
        let expected: Vec<u8> = [
            &4u32.to_le_bytes()[..],
            &[1],
            &7u32.to_le_bytes()[..],
            &0u32.to_le_bytes()[..],
            &0u32.to_le_bytes()[..],
            &[0, 0, 0],
            &0u16.to_le_bytes()[..],
            &[1],
            &[3],
            &7u32.to_le_bytes()[..],
            &10f32.to_le_bytes()[..],
            &20f32.to_le_bytes()[..],
            &[19],
            &7u32.to_le_bytes()[..],
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
                matches!(
                    patch,
                    DomPatch::SetSize { .. }
                        | DomPatch::SetTransform { .. }
                        | DomPatch::SetBox { .. }
                ),
                "a resize is geometry records only — the image never re-travels: {patch:?}"
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

    #[cfg(feature = "canvas")]
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
        // the swapped subtree leaves whole (one remove on its root
        // covers the image inside — or its parent empties, when it was
        // the only child) and the replacement mounts fresh — nothing
        // ever mutates the old element in place
        assert!(
            patches
                .iter()
                .any(|patch| matches!(patch, DomPatch::Remove { .. } | DomPatch::RemoveChildren { .. })),
            "the old subtree leaves: {patches:?}"
        );
        assert!(patches
            .iter()
            .any(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Box, .. })));
        assert!(
            !patches.iter().any(|patch| matches!(
                patch,
                DomPatch::SetImage { id, .. } if *id == image_id
            )),
            "the image element is never retargeted into something else: {patches:?}"
        );
    }

    #[cfg(feature = "canvas")]
    #[derive(Clone)]
    struct Isle;

    #[cfg(feature = "canvas")]
    impl Component for Isle {
        fn body(self, _ctx: &Context) -> impl View {
            image(tiny_image(200))
                .resizable()
                .frame(8.0, 8.0)
                .rendering(Rendering::Gpu)
        }
    }

    #[cfg(feature = "canvas")]
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
        // the PORTAL: the popover hangs off the root, and its anchor
        // relation travels as one patch. Opening re-wraps the anchored
        // child in its anchor group (bounded churn, that subtree only)
        // — every OTHER sibling stays silent.
        assert!(
            top_level.iter().any(|(_, parent)| *parent == 0),
            "the popover hangs off the root: {patches:?}"
        );
        assert!(
            patches.iter().any(|patch| matches!(patch, DomPatch::SetAnchor { .. })),
            "the anchor relation travels: {patches:?}"
        );
        let fresh: Vec<u32> = top_level.iter().map(|(id, _)| *id).collect();
        let anchored: Vec<u32> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Remove { id } => Some(*id),
                _ => None,
            })
            .collect();
        for patch in &patches {
            let id = patch_id(patch);
            assert!(
                !mounted.contains(&id) || fresh.contains(&id) || anchored.contains(&id),
                "an untouched sibling moved on open: {patch:?}"
            );
        }

        // closing removes the portal and unwraps the anchor — nothing
        // beyond those two subtrees moves
        view.open.set(false);
        let patches = runtime.dom_frame(&view, size);
        assert!(!patches.is_empty());
        assert!(
            patches.iter().any(|patch| matches!(
                patch,
                DomPatch::Remove { id } if fresh.contains(id)
            )),
            "the popover left: {patches:?}"
        );
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
                DomPatch::DefineRule { style, .. } if style.gradient.is_some() => Some(style),
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
            .find(|patch| matches!(patch, DomPatch::DefineRule { style, .. } if style.gradient.is_some()))
            .cloned()
            .expect("the look")]);
        let mask = u16::from_le_bytes([bytes[MASK_AT], bytes[MASK_AT + 1]]);
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

    /// The walk asks whether a style paints without building one: the
    /// answer must be the one the built style gives, prop by prop — a
    /// prop it missed would fold a painted box into its text.
    #[test]
    fn whether_props_paint_is_asked_prop_by_prop() {
        let ink = Color::hex(0x123456);
        let each: [fn(&mut VisualProps); 14] = [
            |props| props.background = Some(Color::hex(0x123456)),
            |props| {
                props.gradient = Some(crate::layout::Gradient::Linear {
                    start: crate::layout::UnitPoint { x: 0.0, y: 0.0 },
                    end: crate::layout::UnitPoint { x: 1.0, y: 1.0 },
                    from: Color::BLACK,
                    to: Color::WHITE,
                })
            },
            |props| props.background_hovered = Some(Color::BLACK),
            |props| props.background_pressed = Some(Color::BLACK),
            |props| props.foreground_hovered = Some(Color::BLACK),
            |props| props.foreground_pressed = Some(Color::BLACK),
            |props| props.border = Some((Color::BLACK, 1.0)),
            |props| props.corner_radius = Some(Corners::all(4.0)),
            |props| props.shadow = Some((8.0, Color::BLACK)),
            |props| props.clip = true,
            |props| props.opacity = Some(0.5),
            |props| props.opacity_hovered = Some(0.5),
            |props| props.opacity_pressed = Some(0.5),
            |props| props.glass = Some(crate::layout::Glass::regular()),
        ];
        // an ink alone paints nothing a style records: the text takes it
        let bare = VisualProps { foreground: Some(ink), ..VisualProps::default() };
        assert!(!DomLook::paints(&bare));
        assert_eq!(DomLook::from_props(&bare), DomLook::NONE);
        for (at, set) in each.iter().enumerate() {
            let mut props = bare;
            set(&mut props);
            assert!(DomLook::paints(&props), "prop {at} paints");
            assert_ne!(DomLook::from_props(&props), DomLook::NONE, "prop {at} is recorded");
        }
    }

    /// A style that says nothing holds nothing: setting none boxes no
    /// record, marks emptied and a look set back to the default are
    /// held as none again, and a look boxed at its default reads as
    /// none.
    #[test]
    fn a_style_that_says_nothing_boxes_nothing() {
        let mut style = DomStyle::default();
        style.set_transition(None);
        style.set_tooltip(None);
        style.set_group_owner(None);
        assert!(style.look.is_none() && style.marks.is_none());
        style.set_tooltip(Some(Arc::from("tip")));
        assert!(style.has_marks());
        assert_eq!(style.take_tooltip().as_deref(), Some("tip"));
        assert!(style.marks.is_none(), "emptied marks are none again");
        style.set_look(DomLook { background: Some(Color::BLACK), ..DomLook::default() });
        assert!(style.look.is_some());
        style.set_look(DomLook::NONE);
        assert!(style.look.is_none());
        let mut boxed = DomStyle::default();
        boxed.look_mut();
        assert_eq!(boxed, DomStyle::default());
        assert!(boxed.is_default());
    }

    /// The padding record reads (top, trailing, bottom, leading) and
    /// crosses the wire in that order — the same four floats it always
    /// did, so a page built before the sides were named logical reads
    /// the same bytes.
    #[test]
    fn the_padding_record_reads_top_trailing_bottom_leading() {
        let rule = DomPatch::DefineRule {
            rule: 1,
            kind: CreateKind::FlexColumn,
            flags: 0,
            style: Box::default(),
            layout: Box::new(DomLayout { padding: Some((1.0, 2.0, 3.0, 4.0)), ..DomLayout::default() }),
            text: None,
        };
        let bytes = encode(&[rule]);
        let mut expected = Vec::new();
        for side in [1.0f32, 2.0, 3.0, 4.0] {
            expected.extend_from_slice(&side.to_le_bytes());
        }
        assert!(
            bytes.windows(expected.len()).any(|window| window == expected),
            "the four sides cross in record order: {bytes:?}"
        );
    }

    /// A look's hash names its rule, and a served page is adopted by
    /// trusting that the rules this build hashes are the ones the page
    /// defined: the hash is part of the contract, pinned here as
    /// numbers — every field a look reads, and none of the element's own.
    #[test]
    fn a_look_hashes_to_the_rule_the_served_page_names() {
        let mut boxed = flow_row("a");
        boxed.kind = DomKind::Box;
        boxed.layout = Some(DomLayout {
            gap: Some(8.0),
            align: Some(1),
            padding: Some((12.5, 4.0, 0.5, 2.25)),
            grow: true,
            stretch: true,
            fill: true,
            wrap: Some(2.25),
            plain: true,
            width: Some(120.0),
            ..DomLayout::default()
        });
        boxed.style = DomStyle::of_look(DomLook {
            background: Some(Color::hex(0x112233)),
            gradient: Some(crate::layout::Gradient::Linear {
                start: crate::layout::UnitPoint { x: 0.0, y: 0.0 },
                end: crate::layout::UnitPoint { x: 1.0, y: 0.5 },
                from: Color::BLACK,
                to: Color::WHITE,
            }),
            hover_background: Some(Color::hex(0x223344)),
            pressed_background: Some(Color::hex(0x334455)),
            color: Some(Color::hex(0x445566)),
            hover_color: Some(Color::hex(0x556677)),
            pressed_color: Some(Color::hex(0x667788)),
            border: Some((Color::hex(0x778899), 1.5)),
            corner_radius: Some(Corners::all(6.0)),
            shadow: Some((4.0, Color::hex_a(0x0000_0040))),
            transition: Some((0.4, 0.8)),
            focus_border: Some(Color::hex(0x8899AA)),
            placeholder_color: Some(Color::hex(0x99AABB)),
            clip: true,
            opacity: Some(0.5),
            hover_opacity: Some(0.75),
            pressed_opacity: Some(0.25),
            group: Some(7),
            glass: Some(GlassFilter {
                blur: 8.0,
                saturation: 1.5,
                brightness: 1.0,
                rim: Color::WHITE,
                rim_band: 1.5,
            }),
            pass_through: true,
        });
        boxed.face = Some(Rc::new(FontSpec::DEFAULT));
        let mut words = flow_row("b");
        words.kind = DomKind::Text(DomText {
            content: Arc::from("words"),
            color: Color::hex(0x202020),
            inherits_ink: true,
            font: FontSpec::DEFAULT,
            line_height: Some(18.5),
            text_align: Some(motor::views::TextAlignment::Center),
            highlights: None,
            truncation: Some(Truncation::End),
            inherits_face: false,
        });
        let plain = flow_row("c");
        assert_eq!(look_hash(&boxed), 0x14d3_3cb5_f0e2_1ffa);
        assert_eq!(look_hash(&words), 0xaeb5_8869_66cf_63d7);
        assert_eq!(look_hash(&plain), 0x5727_5724_1e9c_57df);
        // the element's own parts wear the same rule
        boxed.style.interactive = Some(Rc::from("a/b"));
        boxed.style.set_tooltip(Some(Arc::from("tip")));
        boxed.style.set_group_owner(Some(9));
        boxed.layout.as_mut().expect("flow").slot_y = Some(40.0);
        assert_eq!(look_hash(&boxed), 0x14d3_3cb5_f0e2_1ffa);
    }

    #[test]
    fn a_clipped_box_sets_the_overflow_bit_and_nothing_else() {
        let bare = DomLook {
            background: Some(Color::hex(0x123456)),
            corner_radius: Some(Corners::all(6.0)),
            ..DomLook::default()
        };
        let cut = DomLook { clip: true, ..bare.clone() };
        let without = encode(&[define(bare)]);
        let with = encode(&[define(cut)]);
        // the first payload-free bit: the streams differ by ONE bit in
        // the mask's high byte and nothing else
        assert_eq!(with.len(), without.len(), "the bit carries no payload");
        let mask = u16::from_le_bytes([with[MASK_AT], with[MASK_AT + 1]]);
        assert_eq!(mask & (1 << 14), 1 << 14, "bit 14 says the overflow hides");
        let mut expected = without.clone();
        expected[MASK_AT + 1] |= 0x40;
        assert_eq!(with, expected);
    }

    #[test]
    fn four_corners_take_their_own_bit_and_leave_the_one_radius_alone() {
        // one radius keeps bit 4 and its single float — the wire a box
        // that rounds all four has always sent
        let one = DomLook {
            corner_radius: Some(Corners::all(6.0)),
            ..DomLook::default()
        };
        let bytes = encode(&[define(one)]);
        let mask = u32::from_le_bytes(bytes[MASK_AT..MASK_AT + 4].try_into().unwrap());
        assert_eq!(mask, 1 << 4, "the one radius is bit 4, alone");
        assert_eq!(bytes.len(), MASK_AT + 4 + 4 + BARE_TAIL, "and it costs one float");

        // four different ones take bit 22 INSTEAD, with the four in
        // CSS order behind it
        let four = DomLook {
            corner_radius: Some(Corners {
                top_left: 1.0,
                top_right: 2.0,
                bottom_right: 3.0,
                bottom_left: 4.0,
            }),
            ..DomLook::default()
        };
        let bytes = encode(&[define(four)]);
        let mask = u32::from_le_bytes(bytes[MASK_AT..MASK_AT + 4].try_into().unwrap());
        assert_eq!(mask, 1 << 22, "four radii take bit 22, and bit 4 stays clear");
        let radii: Vec<f32> = bytes[MASK_AT + 4..MASK_AT + 4 + 16]
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
            forced: false,
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

    /// A forced drawing hands the browser NO colour of its own, so
    /// every path inherits the element's ink — the rule the glue
    /// already had, reached by saying nothing instead of saying more.
    #[test]
    fn a_forced_icon_hands_over_no_palette() {
        const ORANGE: Color = Color::hex(0xF78C3C);
        const TWO_TONE: crate::icon::Glyph = crate::icon::Glyph {
            draws: &[crate::icon::Draw {
                paint: crate::icon::Paint::Stroke { width: 2.0 },
                path: MARK_PATH,
                tint: Some(ORANGE),
            }],
        };
        const CRAB: crate::icon::Symbol = crate::icon::Symbol::new("test.crab", &TWO_TONE);
        let record = |forced| DomIcon {
            key: CRAB.key,
            symbol: CRAB,
            color: Color::hex(0x8957E5),
            inherits_ink: false,
            forced,
        };
        let plain = encode(&[DomPatch::SetIcon { id: 7, icon: record(false) }]);
        let forced = encode(&[DomPatch::SetIcon { id: 7, icon: record(true) }]);
        // the tinted stream carries the flag AND four colour bytes the
        // forced one never sends
        assert_eq!(plain.len(), forced.len() + 4);
        assert_ne!(plain, forced);
        assert_ne!(record(false), record(true), "the diff sees the two apart");
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
                DomPatch::DefineRule { style, .. } if style.hover_color.is_some() => {
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
        assert!(runtime.pointer_moved(target.0, target.1, false), "the hover state flipped");
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

    #[test]
    fn a_pane_of_glass_becomes_a_native_backdrop_filter() {
        #[derive(Clone, Copy)]
        struct Panel;
        impl Component for Panel {
            fn body(self, _ctx: &Context) -> impl View {
                text("hello")
                    .padding_length(10.0)
                    .corner_radius(16.0)
                    .glass(crate::layout::Glass::regular())
            }
        }

        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&Panel, Size { width: 200.0, height: 80.0 });
        let glass = patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::DefineRule { style, .. } => style.glass,
                _ => None,
            })
            .expect("the pane carries a filter");
        assert_eq!(glass.blur, crate::layout::Glass::TUNED_BLUR);
        assert_eq!(glass.saturation, crate::layout::Glass::TUNED_SATURATION);
        assert_eq!(glass.brightness, 1.0);
        assert!(glass.rim_band > 0.0, "the rim rides along as an inset shadow");

        // the TINT is not on the wire: an element has one background
        // colour, and the tint sits under whatever the box paints
        let background = patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::DefineRule { style, .. } if style.glass.is_some() => style.background,
                _ => None,
            })
            .expect("the tint became the background");
        assert_eq!(background, crate::layout::Glass::TUNED_TINT);

        // and the bit reaches the stream where the glue reads it
        let bytes = encode(&patches);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn a_background_under_glass_paints_over_the_tint() {
        // the tint is UNDER the box's own paint: an opaque background
        // hides it, a translucent one lets it through
        let tint = Color { r: 255, g: 255, b: 255, a: 51 };
        let opaque = Color::hex(0x203040);
        assert_eq!(
            GlassFilter::under(tint, Some(opaque)),
            Some(opaque),
            "an opaque background wins outright"
        );
        let veil = Color { r: 0, g: 0, b: 0, a: 128 };
        let folded = GlassFilter::under(tint, Some(veil)).expect("a colour");
        assert!(folded.r > 0 && folded.r < 255, "a veil mixes with the tint: {folded:?}");
        assert!(folded.a > veil.a, "and the two alphas compose: {folded:?}");
    }

    /// Two runtimes in sequence on ONE thread — every #[test] runs on
    /// its own thread, so both must live in this body. The second
    /// runtime opens its own world: its reads bind to the states that
    /// are alive NOW, and invalidation works. Before the world reset
    /// this pinned the opposite: the second runtime adopted the first
    /// one's retention, every set landed on unread slots, and every
    /// update diffed to nothing, forever.
    #[test]
    fn a_second_runtime_opens_its_own_world() {
        #[derive(Clone, Copy)]
        struct Lamp {
            on: State<bool>,
        }

        impl Component for Lamp {
            fn body(self, _ctx: &Context) -> impl View {
                let lit = self.on.get();
                text("lamp")
                    .background_color(if lit {
                        Color::hex(0xFFD75A)
                    } else {
                        Color::hex(0x30343A)
                    })
                    .on_click(|| {})
            }
        }

        let size = Size { width: 120.0, height: 60.0 };

        // world one, alive and invalidating
        let first = Lamp { on: State::new(false) };
        let elder = Runtime::new();
        assert!(!elder.dom_frame(&first, size).is_empty(), "the first world mounts");
        first.on.set(true);
        assert!(
            !elder.dom_frame(&first, size).is_empty(),
            "the first world invalidates"
        );

        // world two: the same shape, new states, a new runtime
        let second = Lamp { on: State::new(false) };
        let newborn = Runtime::new();
        assert!(!newborn.dom_frame(&second, size).is_empty(), "the second world mounts");
        second.on.set(true);
        let patches = newborn.dom_frame(&second, size);
        assert!(
            !patches.is_empty(),
            "the second world invalidates — its reads bind to living states"
        );
    }

    /// A kept row's hints move by its tag and its id. Its class moves by
    /// them too — unless the class reads for itself through the same
    /// binding object on both sides, and then the class travels on its
    /// own road and says nothing here.
    #[test]
    fn a_kept_rows_hints_move_by_tag_and_id_and_not_by_a_class_that_reads_for_itself() {
        let binding = NodeBinding::Class(crate::bind::Bound::new(Rc::from("row/#class"), Rc::new(String::new)));
        let hinted = |tag: &str, class: &str, id: &str, bound: bool| {
            let mut node = flow_row("row");
            node.hints = DomHints {
                tag: Some(tag.into()),
                class: Some(class.into()),
                address: Some(Rc::new(crate::layout::Address {
                    dom_id: Some(id.into()),
                    href: None,
                })),
            };
            node.binding = bound.then(|| match &binding {
                NodeBinding::Class(bound) => NodeBinding::Class(Rc::clone(bound)),
                NodeBinding::Text(bound) => NodeBinding::Text(Rc::clone(bound)),
            });
            node
        };
        let was = hinted("tr", "a", "x", true);
        assert!(!hints_changed(&was, &hinted("tr", "a", "x", true)), "nothing moved");
        assert!(!hints_changed(&was, &hinted("tr", "b", "x", true)), "the bound class rides its own road");
        assert!(hints_changed(&was, &hinted("td", "a", "x", true)), "the tag moved");
        assert!(hints_changed(&was, &hinted("tr", "a", "y", true)), "the id moved");
        let plain = hinted("tr", "a", "x", false);
        assert!(hints_changed(&plain, &hinted("tr", "b", "x", false)), "a class that does not read for itself moved");
    }

    /// The rows that need no move are found the same with the search and
    /// without it: a position past the last tail extends the run, which
    /// is where the search would have put it. Plans of every shape — a
    /// swap, a reversal, a rotation, a few out of place, fresh entries
    /// among them, and scrambles — stand on the same rows.
    #[test]
    fn the_stable_rows_are_the_ones_the_search_finds() {
        fn searched(plan: &[usize]) -> Vec<bool> {
            const NONE: usize = usize::MAX;
            let mut tails: Vec<usize> = Vec::new();
            let mut parents = vec![NONE; plan.len()];
            for (at, &position) in plan.iter().enumerate() {
                if position == FRESH {
                    continue;
                }
                let place = tails.partition_point(|&tail| plan[tail] < position);
                if place > 0 {
                    parents[at] = tails[place - 1];
                }
                if place == tails.len() {
                    tails.push(at);
                } else {
                    tails[place] = at;
                }
            }
            let mut stable = vec![false; plan.len()];
            let mut cursor = tails.last().copied().unwrap_or(NONE);
            while cursor != NONE {
                stable[cursor] = true;
                cursor = parents[cursor];
            }
            stable
        }
        let mut plans: Vec<Vec<usize>> = Vec::new();
        let mut swapped: Vec<usize> = (0..40).collect();
        swapped.swap(1, 38);
        plans.push(swapped);
        plans.push((0..40).rev().collect());
        plans.push((5..40).chain(0..5).collect());
        let mut few: Vec<usize> = (0..40).collect();
        few.swap(3, 9);
        few.swap(20, 21);
        few[30] = FRESH;
        few.insert(12, FRESH);
        plans.push(few);
        let mut seed = 0x2545_f491_u64;
        for length in [1usize, 2, 7, 64, 300] {
            let mut scramble: Vec<usize> = (0..length).collect();
            for at in (1..length).rev() {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                scramble.swap(at, (seed >> 33) as usize % (at + 1));
                if seed % 11 == 0 {
                    scramble[at] = FRESH;
                }
            }
            plans.push(scramble);
        }
        for plan in &plans {
            assert_eq!(longest_increasing(plan), searched(plan), "{plan:?}");
        }
    }

    // MARK: - The flow vocabulary (the wire half; the capture rides
    // in the next round)

    fn flow_row(path: &str) -> DomNode {
        DomNode {
            kind: DomKind::Group { path: std::rc::Rc::from(path) },
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            style: DomStyle::default(),
            layout: Some(DomLayout::default()),
            hints: DomHints::default(),
            children: Vec::new(),
            binding: None,
            face: None,
            id: 0,
            rule: 0,
        }
    }

    fn flow_root(children: Vec<DomNode>) -> DomNode {
        DomNode {
            kind: DomKind::Root,
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 300.0,
            style: DomStyle::default(),
            layout: Some(DomLayout { gap: Some(8.0), ..DomLayout::default() }),
            hints: DomHints::default(),
            children,
            binding: None,
            face: None,
            id: 0,
            rule: 0,
        }
    }


    /// The reorder law, in bulk: whatever a keyed list becomes — rows
    /// swapped, moved, inserted, removed, replaced, reversed, kept by
    /// promise or lowered again — the patches bring the page's children
    /// to exactly the order asked for, the retained mirror agrees with
    /// the page, and the survivors that travel are the fewest that can:
    /// one move for each survivor off the longest run of its old order,
    /// none for a row on it. The diff trims the ends a frame did not
    /// touch and reorders the middle in place; this is what it may never
    /// get wrong while doing so.
    #[test]
    fn every_reorder_lands_in_the_order_asked_with_the_fewest_moves() {
        let display = crate::layout::DisplayList::default();
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut roll = move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below.max(1) as u64) as usize
        };
        // a row the page already holds may come back as a promise
        let scene = |order: &[String], promised: &dyn Fn(usize) -> bool| {
            flow_root(
                order
                    .iter()
                    .enumerate()
                    .map(|(at, path)| {
                        let mut row = flow_row(path);
                        if promised(at) {
                            row.kind = DomKind::Reuse { path: std::rc::Rc::from(path.as_str()) };
                        }
                        row
                    })
                    .collect(),
            )
        };
        let mut minted = 0usize;
        let mut mint = move || {
            minted += 1;
            format!("row{minted}")
        };
        for _ in 0..400 {
            let mut lowering = DomLowering::default();
            let mut page: Vec<u32> = Vec::new();
            let mut order: Vec<String> = (0..roll(20)).map(|_| mint()).collect();
            let mut was: Vec<String> = Vec::new();
            for _ in 0..10 {
                let held: std::collections::HashSet<&str> =
                    was.iter().map(String::as_str).collect();
                let coin: Vec<bool> = (0..order.len()).map(|_| roll(2) == 0).collect();
                let patches = lowering.lower(
                    scene(&order, &|at| held.contains(order[at].as_str()) && coin[at]),
                    &display,
                );
                // the page, as the browser would be left by the patches
                for patch in &patches {
                    match patch {
                        DomPatch::Create { id, parent: 0, before, .. }
                        | DomPatch::Clone { id, parent: 0, before, .. } => {
                            let at = page.iter().position(|el| el == before).unwrap_or(page.len());
                            page.insert(at, *id);
                        }
                        DomPatch::Move { id, before, .. } => {
                            page.retain(|el| el != id);
                            let at = page.iter().position(|el| el == before).unwrap_or(page.len());
                            page.insert(at, *id);
                        }
                        DomPatch::Remove { id } => page.retain(|el| el != id),
                        DomPatch::RemoveChildren { id: 0, .. } => page.clear(),
                        _ => {}
                    }
                }
                let mirror = &lowering.root.as_ref().expect("mounted").children;
                let paths: Vec<String> = mirror
                    .iter()
                    .map(|row| match &row.kind {
                        DomKind::Group { path } => path.to_string(),
                        other => panic!("a row is a group: {other:?}"),
                    })
                    .collect();
                assert_eq!(paths, order, "the mirror holds the order asked for");
                assert_eq!(
                    page,
                    mirror.iter().map(|row| row.id).collect::<Vec<_>>(),
                    "the page holds the mirror's elements, in its order: {patches:#?}"
                );
                // the fewest moves: the survivors off the longest
                // increasing run of their old positions
                let old_at: std::collections::HashMap<&String, usize> =
                    was.iter().enumerate().map(|(at, path)| (path, at)).collect();
                let survivors: Vec<usize> =
                    order.iter().filter_map(|path| old_at.get(path).copied()).collect();
                let mut tails: Vec<usize> = Vec::new();
                for &position in &survivors {
                    let place = tails.partition_point(|&tail| tail < position);
                    if place == tails.len() {
                        tails.push(position);
                    } else {
                        tails[place] = position;
                    }
                }
                let moves = patches.iter().filter(|p| matches!(p, DomPatch::Move { .. })).count();
                assert_eq!(
                    moves,
                    survivors.len() - tails.len(),
                    "{was:?} -> {order:?}: {patches:#?}"
                );

                // the next frame: one of the shapes a keyed list takes
                was = order.clone();
                let len = order.len();
                match roll(7) {
                    0 if len > 1 => {
                        let (i, j) = (roll(len), roll(len));
                        order.swap(i, j);
                    }
                    1 if len > 0 => {
                        order.remove(roll(len));
                    }
                    2 => {
                        let at = roll(len + 1);
                        order.insert(at, mint());
                    }
                    3 => {
                        for _ in 0..roll(4) {
                            if order.len() > 1 {
                                let row = order.remove(roll(order.len()));
                                let at = roll(order.len() + 1);
                                order.insert(at, row);
                            }
                        }
                    }
                    4 => order = (0..roll(12)).map(|_| mint()).collect(),
                    5 => {
                        for _ in 0..roll(4) {
                            let at = roll(order.len() + 1);
                            order.insert(at, mint());
                        }
                        for _ in 0..roll(3) {
                            if !order.is_empty() {
                                order.remove(roll(order.len()));
                            }
                        }
                    }
                    _ => order.reverse(),
                }
            }
        }
    }

    /// The reorder contract: two rows trade places in a five-row flow
    /// list, and the wire carries exactly two `Move`s — the stable
    /// spine never travels.
    #[test]
    fn a_swap_under_flow_is_two_moves() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        let rows = |order: &[&str]| flow_root(order.iter().map(|p| flow_row(p)).collect());

        let mount = lowering.lower(rows(&["a", "b", "c", "d", "e"]), &display);
        // five rows of one shape: the first mounts whole, the others
        // are clones of it
        assert_eq!(
            mount
                .iter()
                .filter(|p| matches!(p, DomPatch::Create { .. } | DomPatch::Clone { .. }))
                .count(),
            5,
            "{mount:#?}"
        );

        let swapped = lowering.lower(rows(&["a", "d", "c", "b", "e"]), &display);
        let moves: Vec<_> =
            swapped.iter().filter(|p| matches!(p, DomPatch::Move { .. })).collect();
        assert_eq!(moves.len(), 2, "a swap is two moves: {swapped:#?}");
        assert!(
            !swapped.iter().any(|p| matches!(
                p,
                DomPatch::Create { .. }
                    | DomPatch::Clone { .. }
                    | DomPatch::Remove { .. }
                    | DomPatch::SetTransform { .. }
            )),
            "nothing mounts, nothing leaves, nothing is positioned by hand: {swapped:#?}"
        );
    }

    /// An iframe's src is a PATCH, never a re-mount: the element (and
    /// whatever state the page holds in it) survives the navigation,
    /// and an unchanged page ships nothing at all.
    #[test]
    fn an_iframe_navigates_by_patch() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        let page = |src: &str| {
            flow_root(vec![DomNode {
                kind: DomKind::Iframe { src: std::rc::Rc::from(src), sealed: false },
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                style: DomStyle::default(),
                layout: Some(DomLayout {
                    grow: true,
                    stretch: true,
                    ..DomLayout::default()
                }),
                hints: DomHints::default(),
                children: Vec::new(),
                binding: None,
                face: None,
                id: 0,
                rule: 0,
            }])
        };

        let mount = lowering.lower(page("https://a.test/"), &display);
        assert!(
            mount.iter().any(|patch| matches!(
                patch,
                DomPatch::Create { kind: CreateKind::Iframe, .. }
            )),
            "the element mounts as an iframe: {mount:#?}"
        );
        assert!(
            mount.iter().any(|patch| matches!(
                patch,
                DomPatch::SetIframe { src, .. } if &**src == "https://a.test/"
            )),
            "the src rides the mount: {mount:#?}"
        );

        let steady = lowering.lower(page("https://a.test/"), &display);
        assert!(steady.is_empty(), "an unchanged page ships nothing: {steady:#?}");

        let moved = lowering.lower(page("https://b.test/"), &display);
        assert_eq!(moved.len(), 1, "a navigation is ONE patch: {moved:#?}");
        assert!(matches!(
            &moved[0],
            DomPatch::SetIframe { src, .. } if &**src == "https://b.test/"
        ));
        // and the wire carries it without complaint
        assert!(!encode(&moved).is_empty());
    }

    /// A DOCUMENT rides sealed: the patch carries the seal so the glue
    /// can put the sandbox up before the page lands, sealing an open
    /// frame is a patch like a navigation, and the wire spells the
    /// seal as one byte ahead of the page.
    #[test]
    fn a_document_seals_its_iframe() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        let page = |src: &str, sealed: bool| {
            flow_root(vec![DomNode {
                kind: DomKind::Iframe { src: std::rc::Rc::from(src), sealed },
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                style: DomStyle::default(),
                layout: Some(DomLayout {
                    grow: true,
                    stretch: true,
                    ..DomLayout::default()
                }),
                hints: DomHints::default(),
                children: Vec::new(),
                binding: None,
                face: None,
                id: 0,
                rule: 0,
            }])
        };

        let mount = lowering.lower(page("<meta><p>a letter</p>", true), &display);
        assert!(
            mount.iter().any(|patch| matches!(
                patch,
                DomPatch::SetIframe { src, sealed: true, .. } if &**src == "<meta><p>a letter</p>"
            )),
            "the document rides the mount, sealed: {mount:#?}"
        );

        // the same page, unsealed: ONE patch, the seal flipped — the
        // element survives, its powers change
        let opened = lowering.lower(page("<meta><p>a letter</p>", false), &display);
        assert_eq!(opened.len(), 1, "{opened:#?}");
        assert!(matches!(&opened[0], DomPatch::SetIframe { sealed: false, .. }));

        // the wire: op, id, the seal byte, then the page
        let wire = encode(&[DomPatch::SetIframe {
            id: 7,
            src: std::rc::Rc::from("<p>"),
            sealed: true,
        }]);
        assert_eq!(&wire[4..], &[16, 7, 0, 0, 0, 1, 3, 0, 0, 0, b'<', b'p', b'>']);
    }

    /// A video mounts ONCE and its stream rides the mount; a frame that
    /// changes nothing ships nothing; a flipped mirror is one patch that
    /// carries the SAME stream — the glue rewires the element only when
    /// the handle changed, because a rewrite restarts the playback — and
    /// a new stream is one patch too, never a re-mount. The wire's shape
    /// and the glue's gate are pinned beside it.
    #[test]
    fn a_video_creates_once_and_rewires_only_on_a_changed_stream() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        let feed = |stream: u32, mirrored: bool| {
            flow_root(vec![DomNode {
                kind: DomKind::Video { stream, mirrored, cover: true, radius: 12.0 },
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                style: DomStyle::default(),
                layout: Some(DomLayout {
                    grow: true,
                    stretch: true,
                    ..DomLayout::default()
                }),
                hints: DomHints::default(),
                children: Vec::new(),
                binding: None,
                face: None,
                id: 0,
                rule: 0,
            }])
        };

        let mount = lowering.lower(feed(3, false), &display);
        assert_eq!(
            mount
                .iter()
                .filter(|patch| matches!(patch, DomPatch::Create { kind: CreateKind::Video, .. }))
                .count(),
            1,
            "the element mounts once, as a video: {mount:#?}"
        );
        assert!(
            mount.iter().any(|patch| matches!(
                patch,
                DomPatch::SetVideo { stream: 3, mirrored: false, cover: true, .. }
            )),
            "the stream rides the mount: {mount:#?}"
        );

        let steady = lowering.lower(feed(3, false), &display);
        assert!(steady.is_empty(), "an unchanged feed ships nothing: {steady:#?}");

        let flipped = lowering.lower(feed(3, true), &display);
        assert_eq!(flipped.len(), 1, "a flip is ONE patch: {flipped:#?}");
        assert!(
            matches!(&flipped[0], DomPatch::SetVideo { stream: 3, mirrored: true, .. }),
            "the flip carries the same stream, so the glue leaves the playback alone: {flipped:#?}"
        );

        let switched = lowering.lower(feed(4, true), &display);
        assert_eq!(switched.len(), 1, "a new stream is ONE patch: {switched:#?}");
        assert!(
            matches!(&switched[0], DomPatch::SetVideo { stream: 4, .. }),
            "the element survives, its stream changes: {switched:#?}"
        );

        // the wire: op, id, the stream, the two flags, then the radius
        let wire = encode(&[DomPatch::SetVideo {
            id: 7,
            stream: 3,
            mirrored: true,
            cover: false,
            radius: 12.0,
        }]);
        assert_eq!(&wire[4..], &[25, 7, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 0x40, 0x41]);

        // the glue's gate: the handle is compared before the element is
        // rewired, so a flag alone never restarts the playback
        let glue = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        assert!(
            glue.contains("if (el.__stream !== stream)"),
            "glue_dom.js must rewire a video only on a changed stream"
        );
    }

    /// A mid-list insert lands `before` its real next sibling — zero
    /// moves, and the survivors stay silent.
    #[test]
    fn an_insert_under_flow_is_one_positioned_create() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        let rows = |order: &[&str]| flow_root(order.iter().map(|p| flow_row(p)).collect());

        let mount = lowering.lower(rows(&["a", "b", "c"]), &display);
        let ids: Vec<u32> = mount
            .iter()
            .filter_map(|p| match p {
                DomPatch::Create { id, .. } | DomPatch::Clone { id, .. } => Some(*id),
                _ => None,
            })
            .collect();

        let grown = lowering.lower(rows(&["a", "new", "b", "c"]), &display);
        // the fresh row is a clone of the shape already on the page,
        // placed where a create would be
        let creates: Vec<_> = grown
            .iter()
            .filter_map(|p| match p {
                DomPatch::Create { id, before, .. } | DomPatch::Clone { id, before, .. } => {
                    Some((*id, *before))
                }
                _ => None,
            })
            .collect();
        assert_eq!(creates.len(), 1, "{grown:#?}");
        assert_eq!(
            creates[0].1, ids[1],
            "the fresh row lands before what was row b: {grown:#?}"
        );
        assert!(
            !grown.iter().any(|p| matches!(p, DomPatch::Move { .. })),
            "an insert never moves a survivor: {grown:#?}"
        );
    }

    /// The retention is the scene's own nodes, numbered where they stand
    /// and kept in the vectors they came in — and a vector the capture
    /// grew past its nodes gives the room back as it is kept: a slot is a
    /// whole node's worth of memory, held for as long as its row stands.
    /// A list that mounts, mounts again into nothing, grows at its end,
    /// grows before its tail and is replaced whole: every row it keeps,
    /// built or copied, holds its nodes and no room beyond them.
    #[test]
    fn the_retention_is_the_scene_with_no_room_beyond_its_nodes() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        // a row and a list with room for more than they hold: what a
        // capture that grows its vectors as it fills them hands over
        let row = |id: usize| {
            let mut row = flow_row(&format!("L/[{id}]/Item"));
            let mut cells = Vec::with_capacity(8);
            cells.push(DomNode { kind: DomKind::Box, ..flow_row("") });
            cells.push(DomNode { kind: DomKind::Box, ..flow_row("") });
            row.children = cells;
            row
        };
        let rows = |ids: &[usize]| {
            let mut list = Vec::with_capacity(64);
            list.extend(ids.iter().map(|&id| row(id)));
            flow_root(list)
        };
        fn assert_tight(node: &DomNode, ids: &mut std::collections::HashSet<u32>) {
            assert!(ids.insert(node.id), "id {} is kept twice", node.id);
            assert_eq!(
                node.children.capacity(),
                node.children.len(),
                "element {} holds room beyond its {} children",
                node.id,
                node.children.len()
            );
            for child in &node.children {
                assert_tight(child, ids);
            }
        }
        // the rows alone, how many
        fn rows_tight(lowering: &DomLowering) -> usize {
            let root = lowering.root.as_ref().expect("mounted");
            let mut ids = std::collections::HashSet::from([0]);
            for row in &root.children {
                assert_tight(row, &mut ids);
            }
            root.children.len()
        }

        let mount = lowering.lower(rows(&[1, 2, 3, 4]), &display);
        assert!(mount.iter().any(|p| matches!(p, DomPatch::Clone { .. })), "{mount:#?}");
        assert_tight(lowering.root.as_ref().expect("mounted"), &mut std::collections::HashSet::new());
        lowering.lower(rows(&[]), &display);
        // into nothing: the scene's list is the retention's
        lowering.lower(rows(&[1, 2, 3, 4]), &display);
        assert_tight(lowering.root.as_ref().expect("mounted"), &mut std::collections::HashSet::new());
        // at the end and before the tail: the list itself grows as a
        // list grows, its rows hold no more than theirs
        lowering.lower(rows(&[1, 2, 3, 4, 5, 6]), &display);
        assert_eq!(rows_tight(&lowering), 6);
        lowering.lower(rows(&[1, 2, 7, 8, 3, 4, 5, 6]), &display);
        assert_eq!(rows_tight(&lowering), 8);
        // replaced whole: the fresh rows and the list they wait in
        let replaced = lowering.lower(rows(&[9, 10, 11]), &display);
        assert!(replaced.iter().any(|p| matches!(p, DomPatch::RemoveChildren { .. })), "{replaced:#?}");
        assert_tight(lowering.root.as_ref().expect("mounted"), &mut std::collections::HashSet::new());
    }

    /// A row that holds a template is none itself — so when the rows
    /// leave whole, the row's question reaches no template, and the one
    /// inside it must leave with it: a template left standing hands the
    /// next row built around it a copy of an element no longer on the
    /// page, and every row copied from that one inherits the hole.
    #[test]
    fn a_template_inside_a_row_leaves_with_the_row() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        let leaf = || DomNode { kind: DomKind::Box, ..flow_row("") };
        // each row a component holding one of its own after a leaf
        let rows = |ids: std::ops::Range<usize>| {
            flow_root(
                ids.map(|id| {
                    let mut inner = flow_row(&format!("L/[{id}]/Item/#1/Inner"));
                    inner.children = vec![leaf()];
                    let mut row = flow_row(&format!("L/[{id}]/Item"));
                    row.children = vec![leaf(), inner];
                    row
                })
                .collect(),
            )
        };
        // every template stands on an element of the page, and every
        // copy the frame made is of one
        fn assert_live(lowering: &DomLowering, patches: &[DomPatch]) {
            fn walk(retained: &DomNode, live: &mut std::collections::HashSet<u32>) {
                live.insert(retained.id);
                for child in &retained.children {
                    walk(child, live);
                }
            }
            let mut live = std::collections::HashSet::new();
            if let Some(root) = &lowering.root {
                walk(root, &mut live);
            }
            for root in lowering.templates.roots.keys() {
                assert!(live.contains(root), "template {root} stands on a removed element");
            }
            for patch in patches {
                if let DomPatch::Clone { template, .. } = patch {
                    assert!(live.contains(template), "a copy of a removed template {template}");
                }
            }
        }
        let built = |patches: &[DomPatch]| {
            let creates = patches.iter().filter(|p| matches!(p, DomPatch::Create { .. })).count();
            let clones = patches.iter().filter(|p| matches!(p, DomPatch::Clone { .. })).count();
            (creates, clones)
        };

        // row 1 holds the inner template; row 2 is built around a copy
        // of it and is the template rows 3 and 4 copy
        let mount = lowering.lower(rows(1..5), &display);
        assert_live(&lowering, &mount);
        assert_eq!(built(&mount), (6, 3), "{mount:#?}");

        let cleared = lowering.lower(rows(5..5), &display);
        assert!(cleared.iter().any(|p| matches!(p, DomPatch::RemoveChildren { .. })), "{cleared:#?}");
        assert_live(&lowering, &cleared);
        assert!(lowering.templates.roots.is_empty(), "no row is left to be a template");
        assert!(lowering.templates.members.is_empty());

        // a run after the clear builds as the mount did
        let again = lowering.lower(rows(5..9), &display);
        assert_live(&lowering, &again);
        assert_eq!(built(&again), built(&mount), "{again:#?}");
    }

    /// A flow node's layout travels as ONE record — and its geometry
    /// fields never do.
    #[test]
    fn a_flow_layout_change_is_one_setlayout() {
        let mut lowering = DomLowering::default();
        let display = crate::layout::DisplayList::default();
        let with_gap = |gap: f32| {
            let mut root = flow_root(vec![flow_row("a")]);
            root.layout = Some(DomLayout { gap: Some(gap), ..DomLayout::default() });
            root
        };

        let _ = lowering.lower(with_gap(8.0), &display);
        let regapped = lowering.lower(with_gap(12.0), &display);
        // the root changes looks: the new one defined, then worn
        assert_eq!(regapped.len(), 2, "{regapped:#?}");
        assert!(matches!(
            &regapped[0],
            DomPatch::DefineRule { layout, .. } if layout.gap == Some(12.0)
        ));
        assert!(matches!(&regapped[1], DomPatch::UseRule { id: 0, .. }), "{regapped:#?}");
    }

    /// The box an element owns, a move and a reveal, pinned byte for
    /// byte: the box is a mask of five and the floats it names, in order.
    #[test]
    fn the_flow_encoding_is_byte_stable() {
        let patches = vec![
            DomPatch::SetBox {
                id: 5,
                width: None,
                height: Some(24.0),
                max_width: None,
                max_height: None,
                slot_y: Some(120.0),
            },
            DomPatch::Move { id: 5, parent: 1, before: 9 },
            DomPatch::Reveal { id: 3, target: 44 },
        ];
        let bytes = encode(&patches);
        let expected: Vec<u8> = [
            &3u32.to_le_bytes()[..],
            &[23],
            &5u32.to_le_bytes()[..],
            &[1 << 1 | 1 << 4],
            &24f32.to_le_bytes()[..],
            &120f32.to_le_bytes()[..],
            &[12],
            &5u32.to_le_bytes()[..],
            &1u32.to_le_bytes()[..],
            &9u32.to_le_bytes()[..],
            &[13],
            &3u32.to_le_bytes()[..],
            &44u32.to_le_bytes()[..],
        ]
        .concat();
        assert_eq!(bytes, expected);
    }

    /// A look on the wire, pinned byte for byte: its two words, the
    /// kind, the flags, the style and flow records and the text flag —
    /// then an element that wears it, and the marks of its own.
    #[test]
    fn the_look_encoding_is_byte_stable() {
        let rule = 0x0000_0001_0000_0002u64;
        let patches = vec![
            DomPatch::DefineRule {
                rule,
                kind: CreateKind::FlexRow,
                flags: 1,
                style: Box::new(DomLook {
                    background: Some(Color::hex(0x112233)),
                    ..DomLook::default()
                }),
                layout: Box::new(DomLayout { gap: Some(8.0), grow: true, ..DomLayout::default() }),
                text: None,
            },
            DomPatch::UseRule { id: 5, rule },
            DomPatch::SetMarks { id: 5, tooltip: Some(Arc::from("Hi")), group_owner: Some(7) },
        ];
        let bytes = encode(&patches);
        let expected: Vec<u8> = [
            &3u32.to_le_bytes()[..],
            &[21],
            &1u32.to_le_bytes()[..],
            &2u32.to_le_bytes()[..],
            &[10, 1],
            &1u32.to_le_bytes()[..],
            &0x112233FFu32.to_le_bytes()[..],
            &(1u16 | 1 << 7).to_le_bytes()[..],
            &8f32.to_le_bytes()[..],
            &[0],
            &[22],
            &5u32.to_le_bytes()[..],
            &1u32.to_le_bytes()[..],
            &2u32.to_le_bytes()[..],
            &[24],
            &5u32.to_le_bytes()[..],
            &[3],
            &2u16.to_le_bytes()[..],
            b"Hi",
            &0u32.to_le_bytes()[..],
            &7u32.to_le_bytes()[..],
        ]
        .concat();
        assert_eq!(bytes, expected);
    }

    /// The keyboard's reveal under flow: the region follows its item,
    /// and a CHANGED target scrolls to the row's slot — the engine
    /// commands once, the browser's echo comes back silent.
    #[test]
    fn a_reveal_scrolls_to_the_slot() {
        #[derive(Clone)]
        struct Follows {
            selected: State<usize>,
        }

        impl Component for Follows {
            fn body(self, _ctx: &Context) -> impl View {
                let selected = self.selected.get();
                virtual_list(1_000, |row| format!("r{row}"), |row| {
                    text(format!("item {row}"))
                })
                .row_height(20.0)
                .reveal(selected)
            }
        }

        let runtime = Runtime::new();
        let view = Follows { selected: State::new(0) };
        let size = Size { width: 200.0, height: 100.0 };
        let _ = runtime.dom_frame(&view, size);

        view.selected.set(500);
        let patches = runtime.dom_frame(&view, size);
        let scrolled = patches.iter().find_map(|patch| match patch {
            DomPatch::SetScroll { y, .. } => Some(*y),
            _ => None,
        });
        assert_eq!(scrolled, Some(500.0 * 20.0), "the region jumps to the slot: {patches:?}");
    }

    /// The LAW, pinned: a flow frame never asks the text engine for a
    /// number. The browser wraps, measures and breaks — zero cache
    /// calls, zero crossings, on the mount and on every update.
    #[test]
    fn a_flow_frame_never_measures_text() {
        #[derive(Clone)]
        struct Wordy {
            flip: State<bool>,
        }

        impl Component for Wordy {
            fn body(self, _ctx: &Context) -> impl View {
                let on = self.flip.get();
                crate::vstack!(
                    text("a long paragraph that would have wrapped through the cache"),
                    text(if on { "state one" } else { "state two" }),
                    text("another line of prose beside a spacer"),
                )
            }
        }

        let runtime = Runtime::new();
        let view = Wordy { flip: State::new(false) };
        let size = Size { width: 120.0, height: 200.0 };
        let _ = crate::stats::take();
        let _ = runtime.dom_frame(&view, size);
        let mount = crate::stats::take();
        assert_eq!(mount.measure_misses, 0, "the mount never measured");
        assert_eq!(mount.measure_hits, 0, "not even a warm hit");

        view.flip.set(true);
        let _ = runtime.dom_frame(&view, size);
        let update = crate::stats::take();
        assert_eq!(update.measure_misses + update.measure_hits, 0, "nor the update");
    }

    /// The O(change) proof, pinned by NUMBER: an untouched component
    /// is not even traversed. One row flips among fifty; the diff
    /// visits a handful of nodes and reuses every clean sibling.
    #[test]
    fn an_untouched_subtree_is_not_even_traversed() {
        #[derive(Clone, Copy)]
        struct Cell {
            on: State<bool>,
        }

        impl Component for Cell {
            fn body(self, _ctx: &Context) -> impl View {
                let on = self.on.get();
                let toggle = self.on;
                text(if on { "on" } else { "off" })
                    .background_color(if on {
                        Color::hex(0x3B82F6)
                    } else {
                        Color::rgba(0, 0, 0, 0)
                    })
                    .on_click(move || toggle.set(!toggle.get()))
            }
        }

        #[derive(Clone)]
        struct Grid {
            cells: std::rc::Rc<Vec<State<bool>>>,
        }

        impl Component for Grid {
            fn body(self, _ctx: &Context) -> impl View {
                let cells = self.cells.clone();
                crate::vstack!(list(
                    (0..50).collect::<Vec<_>>(),
                    |i| i.to_string(),
                    move |i| Cell { on: cells[*i] },
                ))
            }
        }

        let runtime = Runtime::new();
        let view = Grid { cells: std::rc::Rc::new((0..50).map(|_| State::new(false)).collect()) };
        let size = Size { width: 200.0, height: 400.0 };
        let _ = runtime.dom_frame(&view, size);

        let _ = crate::stats::take();
        view.cells[7].set(true);
        let patches = runtime.dom_frame(&view, size);
        let stats = crate::stats::take();

        assert_eq!(patches.len(), 3, "the flip is a look defined, worn, and the words: {patches:?}");
        assert!(
            stats.diff_visited < 20,
            "the diff visited {} nodes for one flipped cell",
            stats.diff_visited
        );
        assert!(
            stats.diff_reused >= 49,
            "every clean sibling reused wholesale, got {}",
            stats.diff_reused
        );
        assert!(
            stats.capture_nodes < 30,
            "the walk never descended the clean rows, built {}",
            stats.capture_nodes
        );
    }

    /// `.layout(Exact)`: the subtree keeps the engine's numbers on
    /// the element lowering — absolute geometry inside a relative box
    /// the flow carries. The interior positions are the SAME ones the
    /// pixel targets compute: parity by construction, pinned here.
    #[cfg(feature = "canvas")]
    #[test]
    fn an_exact_subtree_keeps_the_engines_numbers() {
        use crate::layout::LayoutMode;

        #[derive(Clone, Copy)]
        struct Mixed;

        impl Component for Mixed {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(
                    text("flow above"),
                    crate::vstack!(text("pinned"), text("exact"))
                        .frame(120.0, 60.0)
                        .layout(LayoutMode::Exact),
                    text("flow below"),
                )
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 200.0, height: 200.0 };
        let patches = runtime.dom_frame(&Mixed, size);

        // the exact interior speaks geometry: transforms and sizes
        let transforms =
            patches.iter().filter(|p| matches!(p, DomPatch::SetTransform { .. })).count();
        assert!(
            transforms >= 2,
            "the exact interior is positioned by our numbers: {patches:#?}"
        );
        // and the flow around it never is (the wrapper itself carries
        // a layout record, not a transform)
        let flow_texts = patches
            .iter()
            .filter(|p| matches!(p, DomPatch::SetContent { .. }))
            .count();
        assert_eq!(flow_texts, 4, "{patches:#?}");
        // a second frame with nothing changed is silent — the exact
        // subtree diffs like everything else
        assert!(runtime.dom_frame(&Mixed, size).is_empty());
    }

    // MARK: - Action paths told against their row

    /// The page a browser holds after these frames sends, for a click
    /// on each element, the path the engine knows the element by.
    fn assert_clicks_resolve(replay: &crate::ssr::Replay, runtime: &Runtime) {
        let engine = runtime.dom_action_paths();
        assert!(!engine.is_empty(), "the page has targets");
        assert_eq!(replay.action_paths(), engine, "{}", replay.html());
    }

    /// The path a click on the target named `name` in row `row` sends,
    /// read off the replayed page.
    fn target_of(replay: &crate::ssr::Replay, name: &str, row: usize) -> String {
        replay
            .action_paths()
            .into_values()
            .find(|path| path.contains(&format!("[{row}]")) && path.ends_with(&format!("[{name}]")))
            .unwrap_or_else(|| panic!("no target `{name}` in row {row}: {}", replay.html()))
    }

    #[derive(Clone, Copy)]
    struct Counted {
        id: usize,
        value: State<usize>,
    }

    fn counted(ids: std::ops::Range<usize>) -> Rc<Vec<Counted>> {
        Rc::new(ids.map(|id| Counted { id, value: State::new(id * 10) }).collect())
    }

    /// The rows kept, and fresh ones after them.
    fn appended<T: Copy>(rows: &[T], fresh: &[T]) -> Rc<Vec<T>> {
        Rc::new(rows.iter().chain(fresh).copied().collect())
    }

    /// A component of its own inside each row: a group inside the clone.
    #[derive(Clone, Copy)]
    struct Stepper {
        value: State<usize>,
    }

    impl Component for Stepper {
        fn body(self, _ctx: &Context) -> impl View {
            let value = self.value;
            crate::hstack!(
                text("-").on_click(move || value.set(value.get().saturating_sub(1))).id("less"),
                text("+").on_click(move || value.set(value.get() + 1)).id("more"),
            )
        }
    }

    #[derive(Clone, Copy)]
    struct CounterRow {
        row: Counted,
    }

    impl Component for CounterRow {
        fn body(self, _ctx: &Context) -> impl View {
            let value = self.row.value;
            crate::hstack!(
                text(format!("row {}", self.row.id)).on_click(move || value.set(0)).id("reset"),
                Stepper { value },
            )
        }
    }

    #[derive(Clone)]
    struct Counters {
        rows: State<Rc<Vec<Counted>>>,
    }

    impl Component for Counters {
        fn body(self, _ctx: &Context) -> impl View {
            crate::vstack!(crate::views::for_each(
                self.rows,
                |row| row.id.to_string(),
                |row| CounterRow { row: *row },
            ))
        }
    }

    /// A row's paths cross told against the row, and a copy whose paths
    /// read as its template's ships none of them. A group inside each
    /// copy carries its OWN path: the base it was copied with names the
    /// template's group, and a click told against that one would reach
    /// another row's counter.
    #[test]
    fn a_group_inside_a_clone_is_told_its_own_base() {
        let size = Size { width: 400.0, height: 400.0 };
        let page = Counters { rows: State::new(counted(1..5)) };
        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&page, size);
        let mut replay = crate::ssr::Replay::new(size);
        replay.apply(&mount);
        assert_clicks_resolve(&replay, &runtime);

        // row 1 holds the stepper's template, so it is none itself: row
        // 2 is built around a copy of row 1's stepper, and becomes the
        // template rows 3 and 4 copy — a template holding a clone
        let clones: Vec<&str> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Clone { base, .. } => Some(base.as_deref().expect("every copy here is a base")),
                _ => None,
            })
            .collect();
        assert_eq!(clones.len(), 3, "{mount:#?}");
        assert!(clones[0].contains("[2]") && clones[0].ends_with("/Stepper"), "{clones:?}");
        for (row, base) in (3..).zip(&clones[1..]) {
            assert!(base.contains(&format!("[{row}]")) && base.ends_with("/CounterRow"), "{clones:?}");
        }
        // the paths of the two rows built cross told against their
        // groups; the copies ship none
        let paths: Vec<(&str, usize)> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::SetPath { path: Some(path), base_len, .. } => Some((&**path, *base_len)),
                _ => None,
            })
            .collect();
        assert_eq!(paths.len(), 4, "row 1's three, row 2's own reset: {paths:?}");
        assert!(paths.iter().all(|(_, base_len)| *base_len > 0), "{paths:?}");
        // a group built is told its base after its subtree; the stepper
        // inside each copied row is told its own, once
        let bases: Vec<&str> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::SetBase { base, .. } => Some(&**base),
                _ => None,
            })
            .collect();
        assert_eq!(
            bases,
            [
                "Counters/#0/Keyed/[1]/CounterRow/#1/Stepper",
                "Counters/#0/Keyed/[1]/CounterRow",
                "Counters/#0/Keyed/[2]/CounterRow",
                "Counters/#0/Keyed/[3]/CounterRow/#1/Stepper",
                "Counters/#0/Keyed/[4]/CounterRow/#1/Stepper",
            ]
        );

        // a click on row 3's "+" reaches row 3's counter, and no other
        let more = target_of(&replay, "more", 3);
        assert!(runtime.dom_action(&more, 1), "the resolved path is a live action: {more}");
        let rows = page.rows.get();
        let values: Vec<usize> = rows.iter().map(|row| row.value.get()).collect();
        assert_eq!(values, [10, 20, 31, 40]);

        // rows appended later copy the same template the same way
        page.rows.set(appended(&page.rows.get(), &counted(5..7)));
        let appended = runtime.dom_frame(&page, size);
        replay.apply(&appended);
        assert_clicks_resolve(&replay, &runtime);
        assert_eq!(
            appended.iter().filter(|patch| matches!(patch, DomPatch::SetBase { .. })).count(),
            2,
            "{appended:#?}"
        );
    }

    /// Rows that hold a component of their own, cleared and run again:
    /// the run copies only what the page still holds, tells the same
    /// bases the mount told, and every click reaches its own row's
    /// action — the stepper inside each row included.
    #[test]
    fn a_run_after_a_clear_resolves_as_the_mount_did() {
        let size = Size { width: 400.0, height: 400.0 };
        let page = Counters { rows: State::new(counted(1..5)) };
        let runtime = Runtime::new();
        let mut replay = crate::ssr::Replay::new(size);
        let mount = runtime.dom_frame(&page, size);
        replay.apply(&mount);
        assert_clicks_resolve(&replay, &runtime);

        page.rows.set(Rc::new(Vec::new()));
        replay.apply(&runtime.dom_frame(&page, size));
        assert!(runtime.dom_action_paths().is_empty(), "the clear leaves no target");
        assert!(replay.action_paths().is_empty(), "{}", replay.html());

        page.rows.set(counted(5..9));
        let run = runtime.dom_frame(&page, size);
        replay.apply(&run);
        assert_clicks_resolve(&replay, &runtime);
        // built as the mount built its rows: row 5 whole (seven
        // elements) holding the stepper's template, row 6 around a copy
        // of it (three), two copies of row 6 — and the mount's copies,
        // bases and paths, told again
        let built = |patches: &[DomPatch]| {
            let count = |wanted: fn(&DomPatch) -> bool| patches.iter().filter(|p| wanted(p)).count();
            [
                count(|p| matches!(p, DomPatch::Create { .. })),
                count(|p| matches!(p, DomPatch::Clone { .. })),
                count(|p| matches!(p, DomPatch::SetBase { .. })),
                count(|p| matches!(p, DomPatch::SetPath { .. })),
            ]
        };
        let (run_built, mount_built) = (built(&run), built(&mount));
        assert_eq!(run_built[0], 10, "{run:#?}");
        assert_eq!(run_built[1..], mount_built[1..], "{run:#?}");

        // a click on row 7's "+" reaches row 7's counter, and no other
        let more = target_of(&replay, "more", 7);
        assert!(runtime.dom_action(&more, 1), "{more}");
        let values: Vec<usize> = page.rows.get().iter().map(|row| row.value.get()).collect();
        assert_eq!(values, [50, 60, 71, 80]);
    }

    #[derive(Clone, Copy)]
    struct Badge {
        id: usize,
    }

    impl Component for Badge {
        fn body(self, _ctx: &Context) -> impl View {
            text(format!("badge {}", self.id)).background_color(Color::hex(0x334455))
        }
    }

    #[derive(Clone)]
    struct Badges {
        rows: State<Rc<Vec<Counted>>>,
        picked: State<usize>,
    }

    impl Component for Badges {
        fn body(self, _ctx: &Context) -> impl View {
            let picked = self.picked;
            crate::vstack!(crate::views::for_each(
                self.rows,
                |row| row.id.to_string(),
                move |row| {
                    let id = row.id;
                    // the click is armed ABOVE the row's component: its
                    // path is the list's, never under the group's
                    Badge { id }.on_click(move || picked.set(id))
                },
            ))
        }
    }

    /// An action armed above its group does not lie under the group's
    /// path: it ships whole, the group carries no base, and every copy
    /// says its own path — the template's would be another row's.
    #[test]
    fn a_path_armed_above_its_group_ships_whole() {
        let size = Size { width: 400.0, height: 400.0 };
        let page = Badges { rows: State::new(counted(1..5)), picked: State::new(0) };
        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&page, size);
        let mut replay = crate::ssr::Replay::new(size);
        replay.apply(&mount);
        assert_clicks_resolve(&replay, &runtime);

        let paths: Vec<(&str, usize)> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::SetPath { path: Some(path), base_len, .. } => Some((&**path, *base_len)),
                _ => None,
            })
            .collect();
        assert_eq!(paths.len(), 4, "every row says its own: {paths:?}");
        assert!(paths.iter().all(|(path, base_len)| *base_len == 0 && !path.contains("/Badge/")), "{paths:?}");
        assert!(mount.iter().any(|patch| matches!(patch, DomPatch::Clone { base: None, .. })), "{mount:#?}");
        assert!(!mount.iter().any(|patch| matches!(patch, DomPatch::Clone { base: Some(_), .. })));
        assert!(!mount.iter().any(|patch| matches!(patch, DomPatch::SetBase { .. })));
        assert!(!replay.html().contains("data-path=\"~"), "{}", replay.html());

        let third = replay
            .action_paths()
            .into_values()
            .find(|path| path.contains("[3]"))
            .expect("row 3 is a target");
        assert!(runtime.dom_action(&third, 1));
        assert_eq!(page.picked.get(), 3);
    }

    #[derive(Clone, Copy)]
    struct Folding {
        id: usize,
        open: State<bool>,
        hits: State<usize>,
    }

    impl Component for Folding {
        fn body(self, _ctx: &Context) -> impl View {
            let open = self.open;
            let hits = self.hits;
            let opened = open.get();
            crate::vstack!(
                // the name moves with the state: a kept element whose
                // path changes on the re-run
                text(format!("row {}", self.id))
                    .on_click(move || open.set(!open.get()))
                    .id(if opened { "fold" } else { "unfold" }),
                // a target born inside a kept row
                opened.then(|| text("details").on_click(move || hits.set(hits.get() + 1)).id("details")),
            )
        }
    }

    #[derive(Clone)]
    struct Folds {
        rows: State<Rc<Vec<Folding>>>,
    }

    impl Component for Folds {
        fn body(self, _ctx: &Context) -> impl View {
            crate::vstack!(crate::views::for_each(self.rows, |row| row.id.to_string(), |row| *row))
        }
    }

    fn folds(ids: std::ops::Range<usize>) -> Rc<Vec<Folding>> {
        Rc::new(
            ids.map(|id| Folding { id, open: State::new(false), hits: State::new(0) }).collect(),
        )
    }

    /// A row that re-runs and changes its structure: the target born in
    /// it, and the target whose path moved, ship whole — the base their
    /// row would lend them was told only to rows that leaned on it — and
    /// every click on the page still reaches its own action, the rows
    /// mounted after the change included.
    #[test]
    fn a_row_that_changes_its_structure_still_resolves() {
        let size = Size { width: 400.0, height: 400.0 };
        let page = Folds { rows: State::new(folds(1..5)) };
        let runtime = Runtime::new();
        let mut replay = crate::ssr::Replay::new(size);
        replay.apply(&runtime.dom_frame(&page, size));
        assert_clicks_resolve(&replay, &runtime);

        // row 2 (a copy) and row 1 (the template) open
        for row in [2, 1] {
            let unfold = target_of(&replay, "unfold", row);
            assert!(runtime.dom_action(&unfold, 1), "{unfold}");
            let patches = runtime.dom_frame(&page, size);
            let paths: Vec<usize> = patches
                .iter()
                .filter_map(|patch| match patch {
                    DomPatch::SetPath { base_len, .. } => Some(*base_len),
                    _ => None,
                })
                .collect();
            assert_eq!(paths, [0, 0], "the moved path and the new one, whole: {patches:#?}");
            replay.apply(&patches);
            assert_clicks_resolve(&replay, &runtime);
        }

        // the new target reaches its own row
        let details = target_of(&replay, "details", 2);
        assert!(runtime.dom_action(&details, 1));
        let rows = page.rows.get();
        assert_eq!(rows.iter().map(|row| row.hits.get()).collect::<Vec<_>>(), [0, 1, 0, 0]);

        // rows mounted after the change, and a row closing again
        page.rows.set(appended(&rows, &folds(5..8)));
        let appended = runtime.dom_frame(&page, size);
        replay.apply(&appended);
        assert_clicks_resolve(&replay, &runtime);
        let fold = target_of(&replay, "fold", 2);
        assert!(runtime.dom_action(&fold, 1));
        replay.apply(&runtime.dom_frame(&page, size));
        assert_clicks_resolve(&replay, &runtime);
        let unfold = target_of(&replay, "unfold", 6);
        assert!(runtime.dom_action(&unfold, 1));
        replay.apply(&runtime.dom_frame(&page, size));
        assert_clicks_resolve(&replay, &runtime);
        assert!(page.rows.get()[5].open.get(), "row 6 opened through its resolved path");
    }

    /// A served page carries the same relative paths and bases as a
    /// mounted one and resolves every click the same way — and a page
    /// that hydrated takes later frames on top of the served tree.
    #[test]
    fn a_served_page_resolves_clicks_as_a_mounted_one() {
        let size = Size { width: 400.0, height: 400.0 };
        let page = Counters { rows: State::new(counted(1..4)) };
        let served = crate::ssr::render(&page, size);
        assert!(served.html.contains("data-base=\""), "{}", served.html);
        assert!(served.html.contains("data-path=\"~/"), "{}", served.html);

        let runtime = Runtime::new();
        let mut replay = crate::ssr::Replay::new(size);
        replay.apply(&runtime.dom_frame(&page, size));
        assert_eq!(replay.html(), served.html, "the mount and the served page are one tree");
        assert_clicks_resolve(&replay, &runtime);

        // the browser adopts the served page, and the next change lands
        // on it — the served bases still hold the paths below them
        let hydrated = Runtime::new();
        hydrated.dom_adopt(&page, size);
        assert!(hydrated.dom_frame(&page, size).is_empty());
        page.rows.set(appended(&page.rows.get(), &counted(4..6)));
        replay.apply(&hydrated.dom_frame(&page, size));
        assert_clicks_resolve(&replay, &hydrated);
        let more = target_of(&replay, "more", 2);
        assert!(hydrated.dom_action(&more, 1));
        assert_eq!(page.rows.get()[1].value.get(), 21);
    }

    /// The new words on the wire: a clone's base, a path told against
    /// its base, a group's base alone — and a clone with no base, a path
    /// under none.
    #[test]
    fn a_relative_path_and_its_bases_cross_the_wire() {
        let patches = vec![
            DomPatch::Clone { id: 9, parent: 0, before: 1, template: 1, base: Some(Rc::from("A/[2]")) },
            DomPatch::SetPath { id: 3, path: Some(Rc::from("A/[1]/#0")), base_len: 5 },
            DomPatch::SetBase { id: 12, base: Rc::from("A/[2]/B") },
        ];
        let expected: Vec<u8> = [
            &3u32.to_le_bytes()[..],
            &[17],
            &9u32.to_le_bytes()[..],
            &0u32.to_le_bytes()[..],
            &1u32.to_le_bytes()[..],
            &1u32.to_le_bytes()[..],
            &5u16.to_le_bytes()[..],
            b"A/[2]",
            &[19],
            &3u32.to_le_bytes()[..],
            &4u16.to_le_bytes()[..],
            b"~/#0",
            &[26],
            &12u32.to_le_bytes()[..],
            &7u16.to_le_bytes()[..],
            b"A/[2]/B",
        ]
        .concat();
        assert_eq!(encode(&patches), expected);
        let whole = encode(&[
            DomPatch::Clone { id: 9, parent: 0, before: 0, template: 1, base: None },
            DomPatch::SetPath { id: 2, path: Some(Rc::from("A/#0")), base_len: 0 },
        ]);
        let expected: Vec<u8> = [
            &2u32.to_le_bytes()[..],
            &[17],
            &9u32.to_le_bytes()[..],
            &0u32.to_le_bytes()[..],
            &0u32.to_le_bytes()[..],
            &1u32.to_le_bytes()[..],
            &0u16.to_le_bytes()[..],
            &[19],
            &2u32.to_le_bytes()[..],
            &4u16.to_le_bytes()[..],
            b"A/#0",
        ]
        .concat();
        assert_eq!(whole, expected);
    }

    /// The glue's half of the contract: it reads a base on every clone
    /// and on op 26, and it resolves `~` against the nearest
    /// `data-base` AT OR ABOVE the element — the element itself when
    /// it carries one, as the engine tells a group's own path against
    /// the group.
    #[test]
    fn the_glue_tells_a_relative_path_against_the_nearest_base() {
        let glue = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        for (line, why) in [
            ("if (base) el.setAttribute(\"data-base\", base);", "a copy's base lands on the element in hand"),
            ("if (path === null || path.charCodeAt(0) !== 126) return path || \"\";", "a path without `~` is whole"),
            ("const home = el.closest(\"[data-base]\");", "the base at or above the element, itself included"),
            ("return home ? home.getAttribute(\"data-base\") + path.slice(1) : \"\";", "the base, then the rest"),
            ("const path = target ? actionPath(target) : \"\";", "a click sends the path resolved"),
            ("window.__bunnyPath = actionPath;", "drivers and probes read paths whole"),
        ] {
            assert!(glue.contains(line), "glue_dom.js: {why}");
        }
        let clone = glue.find("} else if (op === 17) {").expect("the glue reads the clone op");
        let next = glue[clone + 1..].find("} else if (op ===").map_or(glue.len(), |at| clone + 1 + at);
        let body = &glue[clone..next];
        let read = body.find("const base = text(u16());").expect("op 17 reads the copy's base");
        assert!(
            read < body.find("if (source) {").expect("the clone"),
            "op 17 reads its base whether or not the template stands"
        );
        assert!(glue.contains("} else if (op === 26) {"), "glue_dom.js reads op 26");
    }

    // MARK: - The ABI handshake

    /// The glue mirrors this module by hand, so the two halves of the
    /// contract are pinned to one number. Bump [`ABI_VERSION`] without
    /// touching the glue — or the other way around — and this test
    /// goes red before any browser meets the mismatch.
    #[test]
    fn the_glue_expects_this_abi() {
        let pin = format!("const EXPECTED_ABI = {};", ABI_VERSION);
        let element = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        assert!(
            element.contains(&pin),
            "glue_dom.js expects a different ABI than the engine encodes"
        );
        let canvas = include_str!("../../bunny_ui_web/glue/glue.js");
        assert!(
            canvas.contains(&pin),
            "glue.js expects a different ABI than the engine encodes"
        );
        let module = include_str!("../../bunny_ui_web/glue/esm/bunny.js");
        assert!(
            module.contains(&pin),
            "glue/esm/bunny.js expects a different ABI than the engine encodes"
        );
    }

    /// A string crosses as its count of bytes and then its UTF-8, and
    /// the glue reads a short one of ASCII a byte at a time, each byte
    /// being its char. That reading agrees with the decoder's only while
    /// the count is of BYTES and the loop gives up at the first byte from
    /// 0x80 up, leaving the whole string to the decoder; and the words
    /// op must step over the bytes of an element it cannot find, or
    /// every op after it reads from the middle of a string. The engine's
    /// half is pinned on the wire, the glue's in its source.
    #[test]
    fn a_string_crosses_as_bytes_and_only_ascii_passes_the_decoder_by() {
        // "é" is two bytes and one char: the count says three
        let wire = encode(&[DomPatch::SetContent { id: 7, text: Arc::from("é1") }]);
        assert_eq!(&wire[4..], &[20, 7, 0, 0, 0, 3, 0, 0, 0, 0xc3, 0xa9, b'1']);

        let glue = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        for (line, why) in [
            ("if (count === 0) return \"\";", "an empty string takes no view and no decoder"),
            ("if (byte >= 0x80) break;", "a byte from 0x80 up hands the string to the decoder"),
            ("return decoder.decode(bytes(count));", "the decoder reads what the loop gives up"),
        ] {
            assert!(glue.contains(line), "glue_dom.js: {why}");
        }
        let words = glue.find("} else if (op === 20) {").expect("the glue reads the words op");
        let next = glue[words + 1..].find("} else if (op ===").map_or(glue.len(), |at| words + 1 + at);
        assert!(
            glue[words..next].contains("at += count;"),
            "glue_dom.js: the words op steps over the bytes of an element it cannot find"
        );
    }

    /// The shell declares its imports in Rust (`#[link(wasm_import_module
    /// = "./bunny.js")]` and `"./bunny_gpu.js"`); every glue must answer
    /// each one, or the module fails to instantiate with a `LinkError`
    /// that names a verb and nothing about which file forgot it. This
    /// reads the names off the Rust sources and looks for each in the
    /// three glues: a named export in the ES modules, a method in the
    /// classic import objects.
    #[test]
    fn every_import_the_shell_declares_has_a_verb_in_every_glue() {
        let sources = [
            include_str!("../../bunny_ui_web/src/lib.rs"),
            include_str!("../../bunny_ui_web/src/text.rs"),
            include_str!("../../bunny_ui_web/src/image.rs"),
            include_str!("../../bunny_ui_web/src/gpu.rs"),
        ];
        let mut names: Vec<&str> = Vec::new();
        for source in sources {
            for line in source.lines() {
                // an extern declaration: `fn js_x(` / `fn gl_x(`, never a body
                let Some(rest) = line.trim_start().strip_prefix("fn ").or_else(|| {
                    line.trim_start().strip_prefix("pub(crate) fn ")
                }) else {
                    continue;
                };
                let name = rest.split('(').next().unwrap_or("");
                if (name.starts_with("js_") || name.starts_with("gl_")) && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        assert!(names.len() > 50, "the import scan found too few verbs: {names:?}");
        let esm = [
            include_str!("../../bunny_ui_web/glue/esm/bunny.js"),
            include_str!("../../bunny_ui_web/glue/esm/bunny_gpu.js"),
        ]
        .concat();
        let classic = [
            include_str!("../../bunny_ui_web/glue/glue.js"),
            include_str!("../../bunny_ui_web/glue/glue_gl.js"),
        ]
        .concat();
        let element = [
            include_str!("../../bunny_ui_web/glue/glue_dom.js"),
            include_str!("../../bunny_ui_web/glue/glue_gl.js"),
        ]
        .concat();
        for name in names {
            assert!(
                esm.contains(&format!("export function {name}(")),
                "glue/esm does not export `{name}`"
            );
            assert!(
                classic.contains(&format!("{name}(")),
                "glue.js + glue_gl.js do not answer `{name}`"
            );
            assert!(
                element.contains(&format!("{name}(")),
                "glue_dom.js + glue_gl.js do not answer `{name}`"
            );
        }
    }

    /// The other half of the border: every export a glue calls is one
    /// the shell defines. A glue asks for the newer doors before it uses
    /// them (`if (wasm.bunny_touch)`), so a renamed export never fails a
    /// load — the page quietly takes the older road, and a finger on a
    /// phone is a mouse that drags again. This reads the calls off the
    /// glues and looks for each among the shell's `extern "C"` doors.
    #[test]
    fn every_export_a_glue_calls_is_a_door_the_shell_defines() {
        let shell = [
            include_str!("../../bunny_ui_web/src/lib.rs"),
            include_str!("../../bunny_ui_web/src/gpu.rs"),
        ]
        .concat();
        let glues = [
            ("glue.js", include_str!("../../bunny_ui_web/glue/glue.js")),
            ("glue_gl.js", include_str!("../../bunny_ui_web/glue/glue_gl.js")),
            ("glue_dom.js", include_str!("../../bunny_ui_web/glue/glue_dom.js")),
            ("esm/bunny.js", include_str!("../../bunny_ui_web/glue/esm/bunny.js")),
            ("esm/bunny_gpu.js", include_str!("../../bunny_ui_web/glue/esm/bunny_gpu.js")),
        ];
        let mut asked = 0;
        for (file, source) in glues {
            // a member access `.bunny_<name>` is a call into the exports
            for (at, _) in source.match_indices(".bunny_") {
                let name: String = source[at + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                asked += 1;
                assert!(
                    shell.contains(&format!("pub extern \"C\" fn {name}(")),
                    "{file} calls `{name}`, which the shell does not export"
                );
            }
        }
        assert!(asked > 60, "the export scan found too few calls: {asked}");
        // the phone's doors are among them, on both canvas roads
        for name in ["bunny_touch", "bunny_keyboard", "bunny_keyboard_dismissed"] {
            for glue in [glues[0].1, glues[3].1] {
                assert!(glue.contains(&format!(".{name}(")), "a canvas glue never calls `{name}`");
            }
        }
    }

    /// The canonical glue lives beside the shell crate; every app
    /// ships a byte-identical copy. One diverging copy is a fork of
    /// the wire contract — this keeps the fleet on one file.
    #[test]
    fn the_shipped_glue_is_the_canonical_glue() {
        assert_eq!(
            include_str!("../../bunny_ui_web/glue/glue_dom.js"),
            include_str!("../../../apps/finder_web/web/glue_dom.js"),
            "finder_web ships a glue_dom.js that drifted from the canonical copy"
        );
        assert_eq!(
            include_str!("../../bunny_ui_web/glue/glue.js"),
            include_str!("../../../apps/finder_web/web/glue.js"),
            "finder_web ships a glue.js that drifted from the canonical copy"
        );
        assert_eq!(
            include_str!("../../bunny_ui_web/glue/surface.js"),
            include_str!("../../../apps/finder_web/web/surface.js"),
            "finder_web ships a surface.js that drifted from the canonical copy"
        );
        assert_eq!(
            include_str!("../../bunny_ui_web/glue/glue_gl.js"),
            include_str!("../../../apps/finder_web/web/glue_gl.js"),
            "finder_web ships a glue.js that drifted from the canonical copy"
        );
        assert_eq!(
            include_str!("../../bunny_ui_web/glue/glue_dom.js"),
            include_str!("../../../apps/bench_web/web/glue_dom.js"),
            "bench_web ships a glue_dom.js that drifted from the canonical copy"
        );
        assert_eq!(
            include_str!("../../bunny_ui_web/glue/glue_dom.js"),
            include_str!("../../../apps/landing_web/web/glue_dom.js"),
            "landing_web ships a glue_dom.js that drifted from the canonical copy"
        );
    }
}

#[cfg(test)]
mod size_tests {
    /// The nodes a frame moves by the thousand: their size is a cost
    /// of every create — printed here so a diet has a number to start
    /// from, and pinned at the diet's sizes so a field added in passing
    /// shows here before it shows in the memory a row list moves.
    #[test]
    fn the_scene_nodes_stay_small() {
        use std::mem::size_of;
        let sizes = [
            ("DomNode", size_of::<super::DomNode>()),
            ("DomStyle", size_of::<super::DomStyle>()),
            ("DomKind", size_of::<super::DomKind>()),
            ("DomText", size_of::<super::DomText>()),
            ("DomLayout", size_of::<Option<super::DomLayout>>()),
            ("DomHints", size_of::<super::DomHints>()),
            ("DomPatch", size_of::<super::DomPatch>()),
            ("DomPatch max", 96),
            ("LayoutNode", size_of::<crate::layout::LayoutNode>()),
            ("RenderNode", size_of::<motor::view::RenderNode>()),
            ("FontSpec", size_of::<crate::text_engine::FontSpec>()),
            ("Modifier", size_of::<crate::modifier::Modifier>()),
        ];
        for (name, size) in sizes {
            eprintln!("size {name:<11} {size:>5} bytes");
        }
        // the bounds are the 64-bit sizes; a 32-bit target is smaller
        // (wasm: DomNode 248, DomStyle 16, DomKind 56, Modifier 56); a
        // placed element's box stays f64, the served page prints it. The
        // node is the retention too: its element id and its rule are the
        // eight bytes over 320, where a wrapper around every kept node
        // was forty and a second vector per parent
        assert!(size_of::<super::DomNode>() <= 328, "DomNode grew: box the rare record, not the node");
        // the action path inline, the look and the marks boxed
        assert!(size_of::<super::DomStyle>() <= 32, "DomStyle grew: a look's field belongs in DomLook");
        // no kind wider than a text, the commonest: a wider one is boxed
        assert!(size_of::<super::DomKind>() <= 72, "DomKind grew: box the new payload");
        assert!(size_of::<super::DomText>() <= 72, "DomText grew");
        // the flow record holds the wire's f32s
        assert!(size_of::<Option<super::DomLayout>>() <= 84, "DomLayout grew: a length is an f32");
        // a stack, a text and a style carry their hints and their action,
        // the text the widest: a wrapper for each was a box per row
        assert!(size_of::<crate::layout::LayoutNode>() <= 120, "LayoutNode grew: box the rare payload");
        // a patch list is thousands long on a create: its slot must stay
        // small, the fat records boxed
        assert!(size_of::<super::DomPatch>() <= 96, "DomPatch grew: box the record, not the list");
        // a view holds one per modifier, and a row's body is built and
        // moved whole in every run: the rare wide payload is boxed
        assert!(size_of::<crate::modifier::Modifier>() <= 80, "Modifier grew: box the wide payload");
    }
}
