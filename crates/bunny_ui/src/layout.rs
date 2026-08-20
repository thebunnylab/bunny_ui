//! Proposal/response layout — the protocol and the algorithms, headless.
//!
//! The contract has two phases and three rules:
//!
//! 1. **The parent proposes** ([`Proposal`] — `None` = "you decide"),
//!    **the child answers** with a [`Size`], **the parent positions**. The
//!    answer is a TOTAL function of the proposal: every proposal has an
//!    answer, there is no layout error.
//! 2. **Measuring returns a [`Fit`]** — what measurement found and
//!    placement reuses. `place` consumes the `Fit` **by value**: the type
//!    system guarantees the phase order (no placing without measuring, no
//!    placing twice) and that nothing gets measured twice.
//! 3. **Shrinking is the container's decision, in the proposal.** There is
//!    no "automatic minimum" that leaks from the content behind the
//!    scenes, and no visual property that changes size semantics: a
//!    [`LayoutNode::Scroll`] answers what it was offered on the scroll
//!    axis and keeps the content size to itself — no `min_h(0)` anywhere.
//!
//! The [`LayoutNode`] is the RUNTIME tree that render produces: the body
//! runs ONCE per pass and emits print and layout together (evaluating
//! twice would duplicate identity anchors). After the bodies are
//! evaluated, everything reduces to the built-ins — a closed set, so an
//! enum. Text metrics come from the frame's [`TextEngine`] (the house's
//! deterministic [`PixelFont`] by default — 8px per character, 16px per
//! line; CoreText on Mac); the frames come out addressed by the identity
//! path of the boundaries.
//!
//! [`PixelFont`]: crate::text_engine::PixelFont

use motor::hash::FxHashMap as HashMap;
use motor::views::ContentMode;
use std::rc::Rc;
use std::sync::Arc;

use crate::image_engine::{ImageEngine, ImageSource, RawImages, intrinsic_of};
use crate::text_engine::{FontPatch, FontSpec, MeasureCache, PixelFont, TextEngine};

/// Logical pixels. Snapping to device pixels is the real backend's
/// decision, at a single point of the pipeline — never spread around.
pub type Px = f64;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Size {
    pub width: Px,
    pub height: Px,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Point {
    pub x: Px,
    pub y: Px,
}

impl Point {
    /// The origin — where a box's own coordinates start.
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

/// The four corners of a box, clockwise from the top left.
///
/// One number spreads to all four, so everything that has a single
/// radius stays what it was: `.corner_radius(8.0)` and
/// `Corners::all(8.0)` are the same value.
///
/// The shape that needs four is a figure made of BANDS — a selection
/// over three lines rounds the top of the first, nothing on the
/// middle, and the bottom of the last:
///
/// ```ignore
/// painter.fill_rounded(first,  tint, Corners::top(4.0));
/// painter.fill_rounded(middle, tint, Corners::ZERO);
/// painter.fill_rounded(last,   tint, Corners::bottom(4.0));
/// ```
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Corners {
    pub top_left: Px,
    pub top_right: Px,
    pub bottom_right: Px,
    pub bottom_left: Px,
}

impl Corners {
    /// Four square corners — the plain rectangle.
    pub const ZERO: Corners =
        Corners { top_left: 0.0, top_right: 0.0, bottom_right: 0.0, bottom_left: 0.0 };

    /// The same radius on all four.
    pub const fn all(radius: Px) -> Corners {
        Corners {
            top_left: radius,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        }
    }

    /// Rounds the two TOP corners and leaves the bottom square — the
    /// first band of a stack.
    pub const fn top(radius: Px) -> Corners {
        Corners { top_left: radius, top_right: radius, ..Corners::ZERO }
    }

    /// Rounds the two BOTTOM corners — the last band of a stack.
    pub const fn bottom(radius: Px) -> Corners {
        Corners { bottom_right: radius, bottom_left: radius, ..Corners::ZERO }
    }

    /// Rounds the two LEFT corners — the first cell of a row.
    pub const fn left(radius: Px) -> Corners {
        Corners { top_left: radius, bottom_left: radius, ..Corners::ZERO }
    }

    /// Rounds the two RIGHT corners — the last cell of a row.
    pub const fn right(radius: Px) -> Corners {
        Corners { top_right: radius, bottom_right: radius, ..Corners::ZERO }
    }

    /// Are all four square? The straight rectangle takes the short path
    /// in every pipeline.
    pub fn is_zero(&self) -> bool {
        self.max() <= 0.0
    }

    /// The one radius all four share, if they do — what a consumer with
    /// room for a single number asks before it spends four.
    pub fn uniform(&self) -> Option<Px> {
        (self.top_left == self.top_right
            && self.top_left == self.bottom_right
            && self.top_left == self.bottom_left)
            .then_some(self.top_left)
    }

    /// The largest of the four — the reach a corner loop must cover.
    pub fn max(&self) -> Px {
        self.top_left.max(self.top_right).max(self.bottom_right).max(self.bottom_left)
    }

    /// Each corner cut to the box it rounds: never negative, never more
    /// than half a side. A radius under half a pixel becomes square —
    /// the straight rectangle it already painted as.
    pub fn clamped(&self, width: Px, height: Px) -> Corners {
        let cut = |radius: Px| {
            let radius = radius.max(0.0).min(width / 2.0).min(height / 2.0);
            if radius < 0.5 { 0.0 } else { radius }
        };
        Corners {
            top_left: cut(self.top_left),
            top_right: cut(self.top_right),
            bottom_right: cut(self.bottom_right),
            bottom_left: cut(self.bottom_left),
        }
    }
}

impl From<Px> for Corners {
    fn from(radius: Px) -> Corners {
        Corners::all(radius)
    }
}

impl std::ops::Mul<Px> for &Corners {
    type Output = Corners;

    fn mul(self, factor: Px) -> Corners {
        *self * factor
    }
}

impl std::ops::Mul<Px> for Corners {
    type Output = Corners;

    /// The device-pixel scale reaches all four at once.
    fn mul(self, factor: Px) -> Corners {
        Corners {
            top_left: self.top_left * factor,
            top_right: self.top_right * factor,
            bottom_right: self.bottom_right * factor,
            bottom_left: self.bottom_left * factor,
        }
    }
}

/// The parent's proposal. `None` on an axis = "answer your ideal size".
///
/// Deliberately **no `Default`**: a forgotten value cannot silently
/// degrade into "minimum" — whoever proposes chooses, always.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Proposal {
    pub width: Option<Px>,
    pub height: Option<Px>,
}

impl Proposal {
    /// An exact proposal on both axes.
    pub fn exact(size: Size) -> Self {
        Proposal { width: Some(size.width), height: Some(size.height) }
    }

    /// "You decide" on both axes — the child's ideal size.
    pub fn unspecified() -> Self {
        Proposal { width: None, height: None }
    }
}

/// The main axis of a stack.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// Alignment on the cross axis, already in layout terms (the per-axis API
/// types — `HorizontalAlignment`/`VerticalAlignment` — converge here when
/// the node is built).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrossAlign {
    Start,
    Center,
    End,
    /// Rows sit on a SHARED first baseline: text by its ascent, a
    /// baselineless box by its bottom edge. Meaningful in horizontal
    /// stacks; elsewhere it behaves as `Start`.
    Baseline,
}

/// The per-row extent closure of a variable-height virtual list —
/// wrapped so the layout tree stays `Debug`.
#[derive(Clone)]
pub struct RowHeights(pub Rc<dyn Fn(usize) -> Px>);

impl std::fmt::Debug for RowHeights {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("row-heights(fn)")
    }
}

/// Which edge of its anchor a popover prefers. The cross axis centers
/// on the anchor; when the preferred side has no room the frame flips,
/// and what still does not fit clamps inside the overlay container.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Top,
    Bottom,
    Leading,
    Trailing,
}

/// One of the window's own buttons, marked on a scene-drawn control
/// so the platform treats it as the real thing (snap layouts hover
/// the maximize button; the system closes on close). Shells without
/// window chrome of their own ignore it honestly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowControl {
    Close,
    Minimize,
    Maximize,
}

impl Side {
    fn opposite(self) -> Side {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
            Side::Leading => Side::Trailing,
            Side::Trailing => Side::Leading,
        }
    }
}

/// Padding insets, per edge.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Edges {
    pub top: Px,
    pub trailing: Px,
    pub bottom: Px,
    pub leading: Px,
}

impl Edges {
    pub fn uniform(amount: Px) -> Self {
        Edges { top: amount, trailing: amount, bottom: amount, leading: amount }
    }

    fn horizontal(&self) -> Px {
        self.leading + self.trailing
    }

    fn vertical(&self) -> Px {
        self.top + self.bottom
    }
}

/// Line height of the [`PixelFont`] — public for the frame tests.
///
/// [`PixelFont`]: crate::text_engine::PixelFont
pub const LINE_H: Px = 16.0;

/// The context that goes down through both phases: the frame's text
/// engine, the measure cache, the scroll offsets (the `Runtime` owns
/// them) and the current INHERITED font — [`LayoutNode::Styled`] swaps it
/// on the way down.
#[derive(Clone, Copy)]
pub struct LayoutEnv<'a> {
    pub text: &'a dyn TextEngine,
    /// The frame's image edge — decode and resample stay the
    /// platform's; geometry consults it for intrinsic sizes.
    pub images: &'a dyn ImageEngine,
    pub cache: &'a MeasureCache,
    pub scroll_offsets: &'a HashMap<String, Point>,
    pub font: FontSpec,
    /// The inherited line height a paragraph steps by. `None` = the
    /// face's own box; a value overrides it, set by `.line_height(…)`.
    pub line_height: Option<Px>,
    /// The pass's frame state — consulted BY PATH during placement.
    pub stamp: FrameStamp<'a>,
    /// The frame's animator — `None` in bare layouts (tests, direct
    /// [`layout`]): animated props then paint their targets.
    pub animator: Option<&'a std::cell::RefCell<crate::anim::Animator>>,
    /// The animation scope opened by the nearest `Animated` ancestor.
    pub anim: Option<AnimScope<'a>>,
    /// The loop opened by the nearest `.looping(...)` ancestor — the
    /// custom boxes below paint by its phase.
    pub live: Option<crate::anim::Loop>,
    /// Where overlays may live. `None` = the pass's viewport (web,
    /// headless); the mac shell sets the SCREEN in layout coordinates —
    /// a popover then overflows the window by plain geometry, and the
    /// whole policy stays testable headless.
    pub overlay_bounds: Option<Rect>,
    /// How many PHYSICAL pixels one point covers. It only reaches a
    /// custom box's paint — the geometry never consults it, so layout
    /// stays resolution independent by construction.
    pub scale: Px,
}

/// An open animation scope, walking down with the placement. The
/// `Animated` node itself flies its origin (anchored to the enclosing
/// scroll box); the nearest styled below takes the colors and disarms
/// them; crossing a boundary closes the scope — `.animated` styles the
/// view that declared it, never a child component's — and the boundary
/// un-shifts its recorded frame, so retained frames stay the REAL
/// target (scroll-to never chases a moving row).
#[derive(Clone, Copy)]
pub struct AnimScope<'a> {
    /// The identity captured when `.animated` applied.
    pub key: &'a str,
    pub spec: crate::anim::Spring,
    /// The nearest styled below still animates its colors.
    pub colors: bool,
    /// Painted-minus-real origin of the flight in progress.
    pub shift: (Px, Px),
}

/// The FRAME state that placement consults by path: pointer
/// (hover/pressed of the `Interactive`s), focus and caret (of the
/// `Field`s). It lives in the ENV, never in the tree — retention and the
/// layout tree stay free of frame state by construction (the hover LAW,
/// now by type), and a hover/blink frame re-places without cloning a
/// single node.
#[derive(Clone, Copy)]
pub struct FrameStamp<'a> {
    pub interaction: &'a Interaction,
    pub focus: Option<&'a str>,
    pub carets: &'a HashMap<String, crate::text_input::CaretState>,
    /// Blink phase — the caret only paints when visible.
    pub caret_visible: bool,
}

impl<'a> FrameStamp<'a> {
    /// A frame with no pointer and no focus — the default for tests and
    /// for direct [`layout`].
    pub fn idle(
        interaction: &'a Interaction,
        carets: &'a HashMap<String, crate::text_input::CaretState>,
    ) -> Self {
        FrameStamp { interaction, focus: None, carets, caret_visible: true }
    }
}

/// Colored spans ON TOP of the text (the match highlight of a finder):
/// BYTE ranges into the content + the color. The font does not change —
/// only the ink; the measure stays intact by construction.
#[derive(Clone, Debug)]
pub struct TextHighlight {
    pub ranges: Rc<Vec<(usize, usize)>>,
    pub color: Color,
}

/// Where the ellipsis lives when the text does not fit and wrapping is off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Truncation {
    /// `…end/of/path` — keeps the end (paths).
    Start,
    /// `start…end` — keeps both ends (file names).
    Middle,
    /// `start…` — the classic.
    End,
}

/// The layout tree that a render pass emits. A closed set (everything
/// reduces to the built-ins after the bodies), children in a `Vec` — the
/// static dispatch lives in the VIEW tree; this is the runtime structure.
#[derive(Clone, Debug)]
pub enum LayoutNode {
    /// Text: wraps by word against the proposal, or shows an ellipsis
    /// when truncation is on; highlight paints spans without touching the
    /// measure.
    Text {
        content: Arc<str>,
        highlights: Option<TextHighlight>,
        truncation: Option<Truncation>,
    },
    /// Flexible on the main axis of the stack that contains it.
    Spacer,
    /// Rigid box (ProgressView and friends, until they exist for real).
    Leaf { size: Size },
    /// An image: the platform decodes and resamples ([`ImageEngine`]),
    /// the layout owns geometry. `source: None` is the print-parity
    /// stub — a rigid 40×40 outline box. Non-resizable draws at the
    /// intrinsic size (1 pixel = 1 point); `.resizable()` negotiates
    /// with the proposal, and `fit` picks contain ([`ContentMode::Fit`])
    /// or cover-with-built-in-clip ([`ContentMode::Fill`]). While the
    /// platform has not decoded (async web), the node measures zero and
    /// reflows when the engine reports in — pin a `.frame` around it
    /// when the layout must not shift.
    Image {
        source: Option<ImageSource>,
        resizable: bool,
        fit: Option<ContentMode>,
    },
    /// A vector glyph. Rigid by nature — the natural size follows the
    /// INHERITED font (a glyph beside text scales with the text), and
    /// `.resizable()` hands the axes to the proposal, the file-icon
    /// idiom. It paints the largest CENTRED square of its frame, which
    /// is also what the browser's default `preserveAspectRatio` does —
    /// the pixel pipelines and the Dom agree without an attribute.
    Icon { symbol: crate::icon::Symbol, resizable: bool },
    /// Fills whatever the proposal gives (Rectangle).
    Fill,
    Stack { axis: Axis, spacing: Px, align: CrossAlign, children: Vec<LayoutNode> },
    /// Overlay: all children in the same frame (ZStack, sheet on top).
    /// `align` is the HORIZONTAL edge each child sits on; the vertical
    /// one stays centered, exactly what SwiftUI's `.leading` means.
    ///
    /// `modal` says the topmost child CAPTURES what it COVERS: the
    /// pointer and the wheel do not reach anything the scene painted
    /// before it, whether or not the modal draws over that spot — a
    /// press beside the card is as dead as a press on it.
    ///
    /// "Covers" is paint, not the window: a sibling drawn AFTER the
    /// pile is drawn on TOP of the modal, and a thing on top of a
    /// modal was never behind it. So a sheet hung deep in a stack
    /// captures its own subtree and what came before, and nothing
    /// else — which is why a sheet belongs where a sheet belongs, over
    /// the scene it is modal to. A plain pile captures nothing.
    Layered { align: CrossAlign, modal: bool, children: Vec<LayoutNode> },
    /// `.overlay(at, view)` — a layer painted OVER the box (or UNDER
    /// it, with `behind`), taking the box's size and giving it NOTHING
    /// back.
    ///
    /// The base measures alone and its answer IS this node's answer;
    /// the layer only ever negotiates against that resolved box. So a
    /// rule wide enough to cross a box that HUGS its content never asks
    /// the parent for room — which is the whole difference from
    /// [`LayoutNode::Layered`], where every child enters the max and a
    /// flexible one makes the stack above it flexible too.
    ///
    /// `at` places the layer the way a [`UnitPoint`] places anything: a
    /// point of the layer meets the same point of the box, so
    /// `UnitPoint::BOTTOM` hangs a rule on the bottom edge and
    /// `TOP_TRAILING` parks a badge in the corner.
    Overlay {
        at: UnitPoint,
        behind: bool,
        layer: Box<LayoutNode>,
        child: Box<LayoutNode>,
    },
    Padding { edges: Edges, child: Box<LayoutNode> },
    /// `.frame(width:height:)` — `Some` axes override proposal and answer.
    Frame { width: Option<Px>, height: Option<Px>, child: Box<LayoutNode> },
    /// `.frame(maxWidth:maxHeight:)` — `∞` = "fill what was proposed".
    MaxFrame { max_width: Px, max_height: Px, align: CrossAlign, child: Box<LayoutNode> },
    /// Vertical scroll region: answers what it was offered, measures the
    /// content without restriction and keeps the excess to itself (the
    /// shrink contract). `path` is the region's structural identity — the
    /// address of the retained offset (scrolling restores when the list
    /// remounts).
    Scroll {
        path: Option<String>,
        /// The item id the region follows: when it CHANGES, the runtime
        /// scrolls just enough to reveal the row — the wheel stays
        /// sovereign in between (`.scroll_target(id)`).
        target: Option<String>,
        /// Which way the region travels. A list is vertical; an editor
        /// without wrap, a terminal and a spreadsheet go sideways too.
        axes: ScrollAxes,
        child: Box<LayoutNode>,
    },
    /// A two-lane split with a user-draggable divider. The position is
    /// APP state — the view renders the binding's value into `at`, the
    /// runtime writes the drag back through a retained closure — and
    /// the node only resolves it against its clamps. Children are lane
    /// A, the divider's own visual (an ORDINARY child: a 1px styled
    /// strut that lowers honestly on every target), then lane B. Place
    /// exposes a grip band over the divider to the hit-test as
    /// `{path}/#split`; the band wins the reverse walk, so content
    /// near the seam stays clickable right up to it.
    Split {
        path: String,
        axis: Axis,
        unit: SeamUnit,
        at: Px,
        min_a: Px,
        min_b: Px,
        children: Vec<LayoutNode>,
    },
    /// A virtualized vertical run: `count` rows, only a window of them
    /// materialized — each child carries its row index (the window
    /// plus pins need not be contiguous). The full extent keeps the
    /// scroll geometry honest (scrollbar, wheel travel, clamps see ALL
    /// the content) while off-window rows do not exist. Uniform by
    /// default (`row_extent` seeds the window math; the measured first
    /// child is authoritative); with `heights` every row's extent
    /// comes from the closure and offsets are prefix sums — the
    /// closure is the AUTHORITY, and a row that measures taller than
    /// its slot overlaps the next (the app's contract to keep).
    VirtualStack {
        row_extent: Px,
        count: usize,
        children: Vec<(usize, LayoutNode)>,
        heights: Option<RowHeights>,
    },
    /// Semantic visual property: background behind the child, border on
    /// top, foreground inherited. Transparent to the measure — by type.
    Styled { props: Box<VisualProps>, child: Box<LayoutNode> },
    /// An animation scope: the nearest styled below interpolates its
    /// colors through this spring, keyed by the identity captured at
    /// render (`key` is `None` outside a pass — the scope is inert).
    /// Transparent to geometry; in Dom it lowers to a CSS transition.
    Animated { key: Option<Rc<str>>, spec: crate::anim::Spring, child: Box<LayoutNode> },
    /// Text field — semantic end to end (in Dom it becomes an `<input>`
    /// or a `<textarea>`; in Gpu, chrome + text + caret + selection
    /// from here). Focus, caret, selection and IME composition do NOT
    /// live here: placement consults the env's [`FrameStamp`] by
    /// `path` — the tree never carries frame state.
    Field {
        path: String,
        content: Arc<str>,
        placeholder: Arc<str>,
        /// One line, or many. A one-line field is rigid in height and
        /// scrolls sideways; a many-line one takes the height the
        /// parent offers, wraps inside it, and scrolls down.
        multiline: bool,
        /// `.auto_focus()`: the runtime focuses this field on its FIRST
        /// appearance — and never again (a user blur is final).
        auto_focus: bool,
    },
    /// View boundary (`Component`): records the frame at the identity
    /// path — the address for tests and, later on, for hit-testing.
    Boundary { path: String, children: Vec<LayoutNode> },
    /// Interaction target (Button): the frame enters the hit-test list
    /// with the path that indexes the action registered in the
    /// reconciler. Hover and pressed do NOT live here — placement
    /// consults the env's [`FrameStamp`] by `path` (pointer state never
    /// sticks to a tree).
    Interactive { path: String, child: Box<LayoutNode> },
    /// `.hover_group()` — the subtree can paint by THIS box's pointer
    /// state instead of by its own nearest target. The group is hovered
    /// while the hovered target is the group's path or anything under
    /// it, which is what makes the mark inside a chip stay lit when the
    /// pointer finally reaches it (the rule CSS keeps for `:hover`).
    HoverGroup { path: String, child: Box<LayoutNode> },
    /// Reference to a retained boundary (skipped by the reconciler);
    /// measure and place resolve ON-THE-FLY against the retention — the
    /// frame's tree is never stitched into a copy.
    BoundaryRef { path: String },
    /// `.rendering(Gpu)`: this subtree insists on the pixel pipeline.
    /// Transparent to geometry everywhere; in Dom mode it becomes a
    /// CANVAS ISLAND — an element our layout positions, filled with the
    /// subtree's own draw commands. On pixel targets it dissolves:
    /// everything is the pixel pipeline there already. `path` is the
    /// island's identity: a flexible island has no slot to trust, so it
    /// keys the box the browser reports for it by this path.
    Island { path: Option<String>, child: Box<LayoutNode> },
    /// `.looping(...)`: the boxes below paint by a repeating clock.
    /// Transparent to geometry — the phase reaches the paint and
    /// nothing else, so a step of the loop repaints the box and the
    /// scene stays byte-identical.
    Live { spec: crate::anim::Loop, child: Box<LayoutNode> },
    /// `.popover(...)`: the child is the ANCHOR; the overlay does not
    /// descend here. Placement queues it — the pass places every
    /// overlay AFTER the root, so the popover paints on top, its hits
    /// win, no scroll clip cuts it, and the Dom capture mounts it as
    /// the root's last child (the portal, by construction). The anchor
    /// re-resolves on every layout: scroll and resize re-anchor free.
    Anchored {
        /// The popover's identity (dismiss registrations key on it).
        path: String,
        side: Side,
        overlay: Rc<LayoutNode>,
        child: Box<LayoutNode>,
    },
    /// `.window_drag_region()`: pressing the child's frame (where no
    /// interactive target wins) drags the WINDOW — the scene's own
    /// title bar on a chrome-less window. Transparent to geometry;
    /// shells without windows ignore it honestly.
    DragRegion { child: Box<LayoutNode> },
    /// `.window_control(…)`: the child IS one of the window's own
    /// buttons on a scene-drawn title bar. The region wins by design —
    /// the platform activates it, so a press never reaches the scene.
    ControlRegion { control: WindowControl, child: Box<LayoutNode> },
    /// `.tooltip(…)`: hovering the child long enough shows a small
    /// framework-drawn label beside it. Transparent to geometry and to
    /// interaction — the region never steals a hover. The RUNTIME owns
    /// when it shows (the scene stays pointer-invariant); the bubble
    /// itself rides the overlay machinery, so on the desktop it leaves
    /// the window like a popover does.
    Tooltip { text: Arc<str>, side: Side, child: Box<LayoutNode> },
    /// `.context_menu(…)`: a right press inside the child offers these
    /// items at the pointer. The RUNTIME owns the open menu (macOS has
    /// no app state for menus and neither do we); the panel rides the
    /// overlay machinery and leaves the window on the desktop.
    ContextSource { items: std::rc::Rc<[crate::views::MenuItem]>, child: Box<LayoutNode> },
    /// `.on_drag(…)`: pressing the child and moving past the threshold
    /// begins a typed drag. The closure builds the payload AT LIFT —
    /// fresh state, never a stale capture.
    DragSource { payload: DragBuilder, child: Box<LayoutNode> },
    /// `.on_drop(…)`: while a drag of the accepted type is over the
    /// child, the framework rings it; on release the action takes the
    /// value. The runtime finds targets by GEOMETRY — a drop lands
    /// through any opaque hover gate, which is the transparent catcher
    /// the dock asked for.
    ///
    /// Nested targets resolve INNERMOST first: a chip inside a pane
    /// takes its own drop, and an ancestor of the same type stops being
    /// a catch-all for what lands on its own children.
    DropTarget {
        accepts: std::any::TypeId,
        action: DropAction,
        /// The app's own preview: called with the position while a
        /// compatible drag moves over this box, and with `None` the
        /// moment it leaves, lands or is cancelled. Declaring it makes
        /// the framework's ring stand down — the box paints its own.
        over: Option<DragOverAction>,
        child: Box<LayoutNode>,
    },
    /// `.layout(Exact)`: this subtree keeps the ENGINE's layout on
    /// the element lowering — measured, placed and captured with the
    /// absolute machinery, pixel-partner to the canvas. Transparent on
    /// every pixel target (they are exact already, by construction).
    ExactLayout { child: Box<LayoutNode> },
    /// A class for the ENCLOSING boundary's element, declared from
    /// inside its body — the door a row uses to flip its own `<tr>`
    /// class without its parent hearing. Invisible everywhere: zero
    /// size, no paint, no hit.
    BoundaryHint { class: Option<String> },
    /// Element hints for the Dom lowering — a real tag, a class, an
    /// id. Transparent everywhere else, like `.rendering()`: a pixel
    /// target never knows the child was ever going to be a `<tr>`.
    Hinted {
        tag: Option<std::rc::Rc<str>>,
        class: Option<std::rc::Rc<str>>,
        dom_id: Option<std::rc::Rc<str>>,
        child: Box<LayoutNode>,
    },
    /// The escape hatch (`custom(…)` / `canvas(…)`): a box the APP
    /// measures and paints, in the same command vocabulary the built-ins
    /// emit. `path` is its identity — the address of the events it
    /// answers. On the element lowering it becomes a canvas island by
    /// construction: what the app paints is PIXELS, never elements.
    Custom { path: String, element: crate::custom::Custom },
}

/// How the element lowering lays a subtree out: the browser's flow
/// (the default), or the engine's own numbers (`Exact` — pixel parity
/// with the canvas, at the price of our layout running for it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    Flow,
    Exact,
}

/// Where a subtree renders when the scene lowers to elements. The v1
/// capability table is total and deterministic: every built-in lowers
/// to Dom; only an explicit [`Rendering::Gpu`] claims a canvas island.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rendering {
    /// The table decides (v1: everything lowers to Dom).
    Auto,
    /// Force the subtree onto a canvas island.
    Gpu,
}

/// The handoff between the phases — the structural mirror of the
/// [`LayoutNode`], consumed by value: no placing without measuring, and
/// never twice.
#[derive(Debug)]
pub enum Fit {
    Leaf,
    /// Sizes and fits of the children, in order — measured ONCE.
    Children(Vec<(Size, Fit)>),
    /// The virtual run's handoff: the AUTHORITATIVE row extent (from
    /// the measured first child) plus the materialized window, each
    /// child with its row index. Variable-height runs carry the
    /// prefix-sum offsets instead (`offsets[i]` = row `i`'s start;
    /// one extra entry holds the total).
    Virtual {
        row_extent: Px,
        children: Vec<(usize, Size, Fit)>,
        offsets: Option<Rc<Vec<Px>>>,
    },
    Wrapped(Size, Box<Fit>),
    /// The real content size (it can exceed the frame — that is what scrolls).
    ScrollContent(Size, Box<Fit>),
}

/// RGBA color, no drama. Real styling arrives with the visual modifiers;
/// for now the pipeline uses the one-pencil-theme defaults.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 20, g: 20, b: 25, a: 255 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };
    pub const FILL: Color = Color { r: 200, g: 205, b: 215, a: 255 };
    pub const OUTLINE: Color = Color { r: 150, g: 155, b: 165, a: 255 };
    /// Default window background — cool off-white, the floor of the one-pencil theme.
    pub const CANVAS: Color = Color::hex(0xF2F3F7);
    /// The scrollbar thumb — a translucent veil (the blending is real).
    pub const SCROLLBAR: Color = Color { r: 0, g: 0, b: 0, a: 90 };
    /// Border of a focused field.
    pub const FOCUS: Color = Color::hex(0x3B82F6);
    /// Placeholder text.
    pub const PLACEHOLDER: Color = Color::hex(0x9AA2B1);
    /// Text selection veil.
    pub const SELECTION: Color = Color::hex_a(0x3B82F640);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    /// `0xRRGGBB`, alpha 255 — color written the way you read it.
    pub const fn hex(rgb: u32) -> Color {
        Color { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: 255 }
    }

    /// `0xRRGGBBAA` — real-world veils carry alpha for real.
    pub const fn hex_a(rgba: u32) -> Color {
        Color { r: (rgba >> 24) as u8, g: (rgba >> 16) as u8, b: (rgba >> 8) as u8, a: rgba as u8 }
    }

    /// The same color with no alpha — where a ramp fades OUT.
    /// Interpolation is straight, so a glow that ends at
    /// `rgba(0,0,0,0)` drags its ramp through grey; ending at its own
    /// color with alpha zero keeps the hue to the last pixel.
    pub const fn fade(self) -> Color {
        Color { a: 0, ..self }
    }
}

impl std::fmt::Display for Color {
    /// `#RRGGBB` (alpha only when it is not 255) — readable print
    /// suffixes and test messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.a == 255 {
            write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            write!(f, "#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }
}

/// A point inside a box, in its own proportions: `(0, 0)` is the
/// top-left corner and `(1, 1)` the bottom-right. What a gradient
/// anchors to, so one declaration follows a box of any size.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct UnitPoint {
    pub x: f64,
    pub y: f64,
}

impl UnitPoint {
    pub const TOP_LEADING: UnitPoint = UnitPoint { x: 0.0, y: 0.0 };
    pub const TOP: UnitPoint = UnitPoint { x: 0.5, y: 0.0 };
    pub const TOP_TRAILING: UnitPoint = UnitPoint { x: 1.0, y: 0.0 };
    pub const LEADING: UnitPoint = UnitPoint { x: 0.0, y: 0.5 };
    pub const CENTER: UnitPoint = UnitPoint { x: 0.5, y: 0.5 };
    pub const TRAILING: UnitPoint = UnitPoint { x: 1.0, y: 0.5 };
    pub const BOTTOM_LEADING: UnitPoint = UnitPoint { x: 0.0, y: 1.0 };
    pub const BOTTOM: UnitPoint = UnitPoint { x: 0.5, y: 1.0 };
    pub const BOTTOM_TRAILING: UnitPoint = UnitPoint { x: 1.0, y: 1.0 };

    pub const fn new(x: f64, y: f64) -> UnitPoint {
        UnitPoint { x, y }
    }

    /// The point in the scene's coordinates.
    fn resolve(self, rect: Rect) -> Point {
        Point {
            x: rect.origin.x + self.x * rect.size.width,
            y: rect.origin.y + self.y * rect.size.height,
        }
    }
}

/// A two-stop paint ramp behind a view — the glow of a hero panel, the
/// sheen of a bar.
///
/// Declared in the box's own proportions (a [`UnitPoint`] centre, a
/// direction) so it survives a resize, and resolved to px once the
/// frame is known. Deliberately TWO stops: what a UI paints is a ramp,
/// and two colors keep every backend's wire format fixed.
///
/// A ramp that fades out must fade to the SAME color with no alpha
/// ([`Color::fade`]): interpolation is straight, so fading to a
/// transparent black drags the ramp through grey.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Gradient {
    /// Rings around `center`: `inner` at `start` px, `outer` at `end`
    /// px. `end: None` = the box's own reach — the farthest corner.
    /// `aspect` scales the field's Y — `1.0` is the circle it always
    /// was; `260.0 / 560.0` is the D89 wash, an ellipse whose radii
    /// are `end` across and `end · aspect` down (the `start` fraction
    /// rides both axes).
    Radial {
        center: UnitPoint,
        start: Px,
        end: Option<Px>,
        aspect: f64,
        inner: Color,
        outer: Color,
    },
    /// A ramp along the line from `start` to `end`.
    Linear { start: UnitPoint, end: UnitPoint, from: Color, to: Color },
}

impl Gradient {
    /// Rings from the centre, reaching the box's farthest corner.
    pub fn radial(inner: Color, outer: Color) -> Gradient {
        Gradient::Radial {
            center: UnitPoint::CENTER,
            start: 0.0,
            end: None,
            aspect: 1.0,
            inner,
            outer,
        }
    }

    /// A ramp from the top edge to the bottom one.
    pub fn linear(from: Color, to: Color) -> Gradient {
        Gradient::Linear { start: UnitPoint::TOP, end: UnitPoint::BOTTOM, from, to }
    }

    /// Moves a radial gradient's centre (a linear one keeps its line).
    pub fn center(self, center: UnitPoint) -> Gradient {
        match self {
            Gradient::Radial { start, end, aspect, inner, outer, .. } => {
                Gradient::Radial { center, start, end, aspect, inner, outer }
            }
            other => other,
        }
    }

    /// The two radii of a radial gradient, in px.
    pub fn radius(self, start: Px, end: Px) -> Gradient {
        match self {
            Gradient::Radial { center, aspect, inner, outer, .. } => {
                Gradient::Radial { center, start, end: Some(end), aspect, inner, outer }
            }
            other => other,
        }
    }

    /// Squeezes a radial gradient into an ELLIPSE: the Y radius is the
    /// X radius times `aspect`. A 560×260 wash is
    /// `.radius(0.0, 560.0).aspect(260.0 / 560.0)` — and its 70% stop
    /// is `.radius(392.0, 560.0)`, because `start` rides both axes.
    /// One honest limit: an elliptical ramp ignores the box's corner
    /// radius (the wire has no room for both) — a rounded wash clips
    /// through `.clipped()`, which cuts every paint anyway.
    pub fn aspect(self, aspect: f64) -> Gradient {
        match self {
            Gradient::Radial { center, start, end, inner, outer, .. } => {
                Gradient::Radial { center, start, end, aspect, inner, outer }
            }
            other => other,
        }
    }

    /// The line a linear gradient runs along (a radial one keeps its
    /// rings).
    pub fn direction(self, start: UnitPoint, end: UnitPoint) -> Gradient {
        match self {
            Gradient::Linear { from, to, .. } => Gradient::Linear { start, end, from, to },
            other => other,
        }
    }

    /// The gradient in px against the box that carries it — the form
    /// every rasterizer consumes (the CPU resolves in f64; the shaders
    /// only evaluate).
    pub fn resolve(self, rect: Rect) -> GradientPaint {
        match self {
            Gradient::Radial { center, start, end, aspect, inner, outer } => {
                let center = center.resolve(rect);
                let reach = end.unwrap_or_else(|| corner_reach(center, rect));
                GradientPaint::Radial {
                    center,
                    start,
                    end: reach.max(start + 0.001),
                    aspect: if aspect > 0.0 { aspect } else { 1.0 },
                    inner,
                    outer,
                }
            }
            Gradient::Linear { start, end, from, to } => GradientPaint::Linear {
                start: start.resolve(rect),
                end: end.resolve(rect),
                from,
                to,
            },
        }
    }
}

/// The distance from `center` to the farthest corner of `rect`.
fn corner_reach(center: Point, rect: Rect) -> Px {
    let x = (center.x - rect.origin.x).max(rect.origin.x + rect.size.width - center.x);
    let y = (center.y - rect.origin.y).max(rect.origin.y + rect.size.height - center.y);
    x.hypot(y)
}

/// A [`Gradient`] with its geometry in logical px — what a draw command
/// carries.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GradientPaint {
    Radial { center: Point, start: Px, end: Px, aspect: f64, inner: Color, outer: Color },
    Linear { start: Point, end: Point, from: Color, to: Color },
}

impl GradientPaint {
    /// The color at a point, straight sRGB — the one formula, which the
    /// shaders repeat.
    pub fn at(&self, point: Point) -> Color {
        match *self {
            GradientPaint::Radial { center, start, end, aspect, inner, outer } => {
                // the ellipse is a circle in a Y-scaled space: at the
                // point (0, end·aspect) the division lands on `end`
                let distance =
                    (point.x - center.x).hypot((point.y - center.y) / aspect);
                mix(inner, outer, ((distance - start) / (end - start)).clamp(0.0, 1.0))
            }
            GradientPaint::Linear { start, end, from, to } => {
                let (dx, dy) = (end.x - start.x, end.y - start.y);
                let length = dx * dx + dy * dy;
                if length <= 0.0 {
                    return to;
                }
                let along = ((point.x - start.x) * dx + (point.y - start.y) * dy) / length;
                mix(from, to, along.clamp(0.0, 1.0))
            }
        }
    }

    /// The same ramp in device pixels — the rasterizers work there.
    pub fn scaled(self, factor: f64) -> GradientPaint {
        let scale = |point: Point| Point { x: point.x * factor, y: point.y * factor };
        match self {
            GradientPaint::Radial { center, start, end, aspect, inner, outer } => {
                GradientPaint::Radial {
                    center: scale(center),
                    start: start * factor,
                    end: end * factor,
                    aspect,
                    inner,
                    outer,
                }
            }
            GradientPaint::Linear { start, end, from, to } => {
                GradientPaint::Linear { start: scale(start), end: scale(end), from, to }
            }
        }
    }

    /// Shifted into another surface's coordinates.
    pub(crate) fn shifted(self, dx: Px, dy: Px) -> GradientPaint {
        let shift = |point: Point| Point { x: point.x + dx, y: point.y + dy };
        match self {
            GradientPaint::Radial { center, start, end, aspect, inner, outer } => {
                GradientPaint::Radial { center: shift(center), start, end, aspect, inner, outer }
            }
            GradientPaint::Linear { start, end, from, to } => {
                GradientPaint::Linear { start: shift(start), end: shift(end), from, to }
            }
        }
    }
}

/// Straight per-channel interpolation — the formula both shaders repeat.
fn mix(from: Color, to: Color, t: f64) -> Color {
    let channel = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
    Color {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: channel(from.a, to.a),
    }
}

/// The liquid-glass material: a pane that reads what is painted behind
/// it, blurs that, bends it through a rounded lens and lays a tint over
/// the result.
///
/// Every knob is optional and an unset knob takes the tuned value, so
/// two glass modifiers on one view MERGE knob by knob (the one closest
/// to the view wins). `.liquid_glass().backdrop_blur(16.0)` is one
/// material with one changed number, not two materials.
///
/// The pane samples the draw list ALREADY BEHIND IT. Three consequences
/// are part of the contract:
///
/// - The box's own background, border, text and children paint AFTER,
///   and stay sharp. That is what makes glass usable.
/// - The box's own halo is captured INTO the glass: the shadow paints
///   first and it overlaps. Blurred and tinted it reads as depth. To
///   keep a halo out of the pane, hang it on a wrapper box.
/// - A pane over nothing shows nothing. Glass is a material, not a
///   color: it needs something behind it to bend.
///
/// Declared in the box's own proportions (a [`UnitPoint`] for the
/// spot), the same as [`Gradient`], so it survives a resize.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Glass {
    /// Gaussian sigma of the backdrop blur, in logical px. The floor is
    /// the material's own: a value below the first level of the blur
    /// pyramid renders as that level. The minimum glass is light
    /// glass, never clear glass.
    ///
    /// CAUTION: the blur decides what the LENS has left to bend. A
    /// blur erases every detail thinner than itself, so a heavy blur
    /// over hairlines leaves a tinted box with a bright rim and no
    /// bend at all — which is how a fake looks. Give the scene behind
    /// the pane shapes wider than the blur, or lower the blur.
    pub blur: Option<Px>,
    /// Composited OVER the blurred backdrop.
    pub tint: Option<Color>,
    /// `(band, amount)`: how far inward the rim bends the backdrop, and
    /// the peak displacement at the very edge, both in logical px. An
    /// amount of zero makes a flat pane — blur and tint only, which is
    /// what a nested panel and text-heavy glass want.
    pub refraction: Option<(Px, Px)>,
    /// Per-channel spread of the rim displacement. `0.0`, the tuned
    /// value, has no fringe at all.
    pub chromatic: Option<f64>,
    /// `(color, band, intensity)` of the specular rim.
    pub highlight: Option<(Color, Px, f64)>,
    /// `1.0` leaves the backdrop as it is. Above that keeps color alive
    /// through a heavy blur.
    pub saturation: Option<f64>,
    /// `1.0` leaves the backdrop as it is.
    pub brightness: Option<f64>,
    /// Flat additive white over the whole pane, `0..1` — the uniform
    /// half of a touch sheen.
    pub sheen: Option<f64>,
    /// `(centre, radius, alpha)` of a radial additive white spot: where
    /// it sits in the pane, its radius as a fraction of the pane's
    /// SMALLER side, and its peak white.
    pub spot: Option<(UnitPoint, f64, f64)>,
}

impl Glass {
    /// The tuned material. The numbers are a lens, not a frost: a small
    /// blur over nearly-legible content, a displacement about one and a
    /// half times its band, saturation above one, and no fringe.
    pub const TUNED_BLUR: Px = 8.0;
    pub const TUNED_TINT: Color = Color { r: 255, g: 255, b: 255, a: 51 };
    pub const TUNED_REFRACTION: (Px, Px) = (16.0, 24.0);
    pub const TUNED_HIGHLIGHT: (Color, Px, f64) = (Color::WHITE, 2.0, 0.9);
    pub const TUNED_SATURATION: f64 = 1.5;

    /// The tuned material, with nothing pinned — an outer
    /// `.backdrop_*()` can still move any knob.
    pub const fn regular() -> Glass {
        Glass {
            blur: None,
            tint: None,
            refraction: None,
            chromatic: None,
            highlight: None,
            saturation: None,
            brightness: None,
            sheen: None,
            spot: None,
        }
    }

    /// Barely there: a thin tint and a stronger lens. What a control
    /// over a photograph wants.
    pub const fn clear() -> Glass {
        Glass {
            blur: Some(4.0),
            tint: Some(Color { r: 255, g: 255, b: 255, a: 26 }),
            refraction: Some((12.0, 28.0)),
            ..Glass::regular()
        }
    }

    /// A flat frosted pane: a heavy blur and NO lens. What a panel
    /// full of text wants — a bent rim under a paragraph is noise.
    pub const fn frosted() -> Glass {
        Glass {
            blur: Some(24.0),
            tint: Some(Color { r: 255, g: 255, b: 255, a: 61 }),
            refraction: Some((0.0, 0.0)),
            ..Glass::regular()
        }
    }

    pub const fn blur(mut self, sigma: Px) -> Glass {
        self.blur = Some(sigma);
        self
    }

    pub const fn tint(mut self, color: Color) -> Glass {
        self.tint = Some(color);
        self
    }

    /// How far inward the rim bends the backdrop, and by how much.
    pub const fn refraction(mut self, band: Px, amount: Px) -> Glass {
        self.refraction = Some((band, amount));
        self
    }

    /// The per-channel spread of the rim displacement. `0.0` has no
    /// fringe.
    pub const fn chromatic(mut self, amount: f64) -> Glass {
        self.chromatic = Some(amount);
        self
    }

    pub const fn highlight(mut self, color: Color, band: Px, intensity: f64) -> Glass {
        self.highlight = Some((color, band, intensity));
        self
    }

    pub const fn saturation(mut self, saturation: f64) -> Glass {
        self.saturation = Some(saturation);
        self
    }

    pub const fn brightness(mut self, brightness: f64) -> Glass {
        self.brightness = Some(brightness);
        self
    }

    pub const fn sheen(mut self, sheen: f64) -> Glass {
        self.sheen = Some(sheen);
        self
    }

    /// A pool of light in the pane: where, how wide as a fraction of
    /// the pane's smaller side, and how bright.
    pub const fn spot(mut self, center: UnitPoint, radius: f64, alpha: f64) -> Glass {
        self.spot = Some((center, radius, alpha));
        self
    }

    /// Merge of two glass modifiers on the same view: the knob already
    /// set (CLOSEST to the view) wins, and the outer one only fills
    /// what is missing — the same rule [`VisualProps::or`] follows.
    pub fn or(self, outer: Glass) -> Glass {
        Glass {
            blur: self.blur.or(outer.blur),
            tint: self.tint.or(outer.tint),
            refraction: self.refraction.or(outer.refraction),
            chromatic: self.chromatic.or(outer.chromatic),
            highlight: self.highlight.or(outer.highlight),
            saturation: self.saturation.or(outer.saturation),
            brightness: self.brightness.or(outer.brightness),
            sheen: self.sheen.or(outer.sheen),
            spot: self.spot.or(outer.spot),
        }
    }

    /// The knobs this material actually named, in a fixed order — what
    /// the scene print shows. A view that only asked for the material
    /// names nothing.
    pub fn knobs(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(blur) = self.blur {
            parts.push(format!("blur: {blur}"));
        }
        if let Some(tint) = self.tint {
            parts.push(format!("tint: {tint}"));
        }
        if let Some((band, amount)) = self.refraction {
            parts.push(format!("refraction: {band}/{amount}"));
        }
        if let Some(chromatic) = self.chromatic {
            parts.push(format!("chromatic: {chromatic}"));
        }
        if let Some((color, band, intensity)) = self.highlight {
            parts.push(format!("highlight: {color} {band}/{intensity}"));
        }
        if let Some(saturation) = self.saturation {
            parts.push(format!("saturation: {saturation}"));
        }
        if let Some(brightness) = self.brightness {
            parts.push(format!("brightness: {brightness}"));
        }
        if let Some(sheen) = self.sheen {
            parts.push(format!("sheen: {sheen}"));
        }
        if let Some((center, radius, alpha)) = self.spot {
            parts.push(format!("spot: {} {} {radius}/{alpha}", center.x, center.y));
        }
        parts.join(", ")
    }

    /// The material in px against the box that carries it — the form
    /// every rasterizer consumes (the CPU resolves in f64; the shaders
    /// only evaluate).
    pub fn resolve(self, rect: Rect) -> GlassPaint {
        let (refraction_band, refraction_amount) = self.refraction.unwrap_or(Glass::TUNED_REFRACTION);
        let (highlight, highlight_band, highlight_intensity) =
            self.highlight.unwrap_or(Glass::TUNED_HIGHLIGHT);
        let smaller = rect.size.width.min(rect.size.height);
        let (spot_center, spot_radius, spot_alpha) = match self.spot {
            Some((center, radius, alpha)) => (center.resolve(rect), radius * smaller, alpha),
            None => (rect.origin, 0.0, 0.0),
        };
        GlassPaint {
            blur: self.blur.unwrap_or(Glass::TUNED_BLUR).max(0.0),
            tint: self.tint.unwrap_or(Glass::TUNED_TINT),
            refraction_band: refraction_band.max(0.0),
            refraction_amount,
            chromatic: self.chromatic.unwrap_or(0.0).max(0.0),
            highlight,
            highlight_band: highlight_band.max(0.0),
            highlight_intensity: highlight_intensity.max(0.0),
            saturation: self.saturation.unwrap_or(Glass::TUNED_SATURATION).max(0.0),
            brightness: self.brightness.unwrap_or(1.0).max(0.0),
            sheen: self.sheen.unwrap_or(0.0).clamp(0.0, 1.0),
            spot_center,
            spot_radius,
            spot_alpha: spot_alpha.clamp(0.0, 1.0),
        }
    }
}

/// A [`Glass`] with every number resolved in logical px — what a draw
/// command carries.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GlassPaint {
    pub blur: Px,
    pub tint: Color,
    pub refraction_band: Px,
    pub refraction_amount: Px,
    pub chromatic: f64,
    pub highlight: Color,
    pub highlight_band: Px,
    pub highlight_intensity: f64,
    pub saturation: f64,
    pub brightness: f64,
    pub sheen: f64,
    pub spot_center: Point,
    pub spot_radius: Px,
    pub spot_alpha: f64,
}

impl GlassPaint {
    /// The same material in device pixels — the rasterizers work there.
    pub fn scaled(self, factor: f64) -> GlassPaint {
        GlassPaint {
            blur: self.blur * factor,
            refraction_band: self.refraction_band * factor,
            refraction_amount: self.refraction_amount * factor,
            highlight_band: self.highlight_band * factor,
            spot_center: Point {
                x: self.spot_center.x * factor,
                y: self.spot_center.y * factor,
            },
            spot_radius: self.spot_radius * factor,
            ..self
        }
    }

    /// Shifted into another surface's coordinates.
    pub(crate) fn shifted(self, dx: Px, dy: Px) -> GlassPaint {
        GlassPaint {
            spot_center: Point { x: self.spot_center.x + dx, y: self.spot_center.y + dy },
            ..self
        }
    }

    /// How far outside the pane the material reads, in the same units
    /// as its own numbers — a SUPERSET, which is what a damage rect
    /// always wants. The lens pulls from `refraction_amount` away and
    /// the fringe spreads that; the blur reaches about four sigma, and
    /// never less than the pyramid's own floor.
    pub fn reach(&self) -> Px {
        let fringe = self.refraction_amount.abs() * (1.0 + self.chromatic.max(0.0));
        self.blur.max(3.0) * 4.0 + fringe
    }
}

/// VISUAL properties of a scene node — paint only, by construction: no
/// field here changes measure (the "hover does not touch layout" LAW,
/// guaranteed by the type). In Dom mode this becomes the element's CSS;
/// in Gpu, draw commands — the semantics never die before the backend
/// gets to choose.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct VisualProps {
    pub background: Option<Color>,
    /// A ramp painted OVER the flat background and under the child —
    /// the two compose (`.background_color(base).background_gradient(…)`).
    pub gradient: Option<Gradient>,
    /// The liquid-glass material behind the box: it reads what is
    /// already painted, blurs it, bends it at the rim and lays a tint
    /// over the result. Paint only, like everything here — a pane
    /// measures exactly like the box it dresses.
    pub glass: Option<Glass>,
    /// Inherited downward: the text below paints with the current foreground.
    pub foreground: Option<Color>,
    pub border: Option<(Color, Px)>,
    pub corner_radius: Option<Corners>,
    /// Alternate background under hover/pressed of the ancestor
    /// `Interactive` — in Dom, `:hover`/`:active`. (Generalizing the
    /// state to the whole `VisualProps` waits for the theme port.)
    pub background_hovered: Option<Color>,
    pub background_pressed: Option<Color>,
    /// The same two states for the INK the text below inherits — a
    /// faint glyph that brightens under the pointer. Paint only, like
    /// its background twins: the measure never hears about it.
    pub foreground_hovered: Option<Color>,
    pub foreground_pressed: Option<Color>,
    /// Inherited font patch — the EXCEPTION to the "props is paint only"
    /// rule: font changes measure, so it goes down the `LayoutEnv`
    /// already at measure time (hover state is still forbidden to touch
    /// it).
    pub font: FontPatch,
    /// Inherited line height, the same exception as `font`: it sets the
    /// box a paragraph measures, so it travels at measure time. `None`
    /// keeps the face's own box (`ascent + descent`); a value steps the
    /// lines by it and centres the glyphs in the taller box — the CSS
    /// half-leading.
    pub line_height: Option<Px>,
    /// A soft halo behind the view: `(radius, color)`. The falloff is
    /// quadratic; the halo paints OUTSIDE the shape and follows the
    /// corner radius — including the notch behind a rounded corner,
    /// which belongs to the shadow, not to the backdrop.
    pub shadow: Option<(Px, Color)>,
    /// `.clipped()` — the subtree cannot paint outside this box, and
    /// the cut FOLLOWS `.corner_radius(…)` when there is one. Paint
    /// only, like everything here: the measure never hears about it.
    pub clip: bool,
    /// `.opacity(…)` — everything the subtree paints fades by this
    /// factor, `0..1`. Paint only, like the rest: a box at zero still
    /// measures, still lays out and still clicks.
    ///
    /// In the pixel pipelines the fade lands on each COMMAND, not on a
    /// layer: two children of one faded box that overlap show through
    /// each other. That is the price of a compositor with no offscreen
    /// pass, and it is invisible in the case the modifier exists for —
    /// a crossfade between two glyphs that share a slot.
    pub opacity: Option<f64>,
    /// The same fade under hover and pressed, so a mark can appear
    /// under the pointer without the scene changing what it CONTAINS
    /// (the LAW: hover swaps paint, never content).
    pub opacity_hovered: Option<f64>,
    pub opacity_pressed: Option<f64>,
    /// `.group_hovered()` — this box's hover and pressed paint follows
    /// the nearest `.hover_group()` above it, not the interactive
    /// target it belongs to.
    pub from_group: bool,
}

impl VisualProps {
    /// Merge of modifiers stacked on the same view: what is already set
    /// (CLOSEST to the view) wins; the outer one only fills what is
    /// missing.
    pub fn or(self, outer: VisualProps) -> VisualProps {
        VisualProps {
            background: self.background.or(outer.background),
            gradient: self.gradient.or(outer.gradient),
            // knob by knob, so a chain of `.backdrop_*()` builds ONE
            // material instead of the innermost one winning whole
            glass: match (self.glass, outer.glass) {
                (Some(inner), Some(outer)) => Some(inner.or(outer)),
                (inner, outer) => inner.or(outer),
            },
            clip: self.clip || outer.clip,
            foreground: self.foreground.or(outer.foreground),
            border: self.border.or(outer.border),
            corner_radius: self.corner_radius.or(outer.corner_radius),
            background_hovered: self.background_hovered.or(outer.background_hovered),
            background_pressed: self.background_pressed.or(outer.background_pressed),
            foreground_hovered: self.foreground_hovered.or(outer.foreground_hovered),
            foreground_pressed: self.foreground_pressed.or(outer.foreground_pressed),
            font: self.font.or(outer.font),
            line_height: self.line_height.or(outer.line_height),
            shadow: self.shadow.or(outer.shadow),
            opacity: self.opacity.or(outer.opacity),
            opacity_hovered: self.opacity_hovered.or(outer.opacity_hovered),
            opacity_pressed: self.opacity_pressed.or(outer.opacity_pressed),
            from_group: self.from_group || outer.from_group,
        }
    }
}

/// Interaction state of a frame — resolved BEFORE layout and stamped into
/// the expansion (the LAW: hover swaps paint, never measure). The
/// `Runtime` owns it; it lives here because it is scene vocabulary
/// (paths + point).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Interaction {
    pub pointer: Option<Point>,
    pub hovered: Option<String>,
    pub pressed: Option<String>,
    /// The split whose divider is being dragged — moves route to its
    /// retained position writer instead of hover until the release.
    pub split_drag: Option<String>,
    /// The scrollbar thumb being dragged: the region's path, whether it
    /// is the horizontal one, and WHERE inside the thumb the press
    /// landed — without that offset the thumb jumps to the pointer on
    /// the first move.
    pub thumb_drag: Option<ThumbDrag>,
    /// The app's box that owns the pointer: a press inside it keeps
    /// every move until the release, even outside the frame (dragging
    /// a selection out of the box and back is one gesture).
    pub element_grab: Option<String>,
    /// The field whose selection is being swept: the press dropped the
    /// anchor, every move until the release carries the caret away
    /// from it, and the release changes nothing.
    pub field_drag: Option<String>,
    /// The tooltip the runtime decided to SHOW — resolved before
    /// layout like everything here (the delay is the shell's clock,
    /// never the scene's). The placement turns it into an overlay.
    pub tooltip: Option<(Arc<str>, Side, Rect)>,
    /// The open context menu — the runtime's, resolved before layout.
    pub menu: Option<MenuOpen>,
    /// The live drag — the runtime's, resolved before layout.
    pub drag: Option<DragLive>,
}

/// A scrollbar thumb under the pointer, mid-drag.
#[derive(Clone, Debug, PartialEq)]
pub struct ThumbDrag {
    pub path: String,
    pub horizontal: bool,
    /// Distance from the thumb's leading edge to the press.
    pub grab: Px,
}

/// A draw command — the output of the placement pass, in paint order
/// (whoever comes later paints on top; `Layered` counts on that).
/// It is the rasterizer's interface and, later on, any backend's.
#[derive(Clone, PartialEq, Debug)]
pub enum DrawCommand {
    /// `corner_radius: Corners::ZERO` = plain rectangle (the usual
    /// straight path). Four numbers, so a band of a bigger figure can
    /// round only the corners that end it.
    FillRect { rect: Rect, color: Color, corner_radius: Corners },
    /// A two-stop ramp inside the rounded rect — the same shape a
    /// `FillRect` covers, with the color resolved per pixel.
    Gradient { rect: Rect, paint: GradientPaint, corner_radius: Corners },
    /// The liquid-glass pane: it READS the pixels the commands before
    /// it left behind, blurs them, bends them through the rounded lens
    /// and lays the tint over the result. The only command that reads
    /// what it paints on, which is why it must keep its place in paint
    /// order — a pane moved earlier shows a scene that did not exist
    /// yet.
    Backdrop { rect: Rect, glass: GlassPaint, corner_radius: Corners },
    /// A soft halo OUTSIDE the rounded rect: alpha falls off
    /// quadratically from the edge over `radius` px. `corner_radius`
    /// makes the halo follow the corners — including the little notch
    /// BEHIND a rounded corner, which belongs to the shadow, not to the
    /// backdrop.
    Shadow { rect: Rect, radius: Px, color: Color, corner_radius: Corners },
    /// A border painted INWARD from the edge, `width` logical px —
    /// it follows `corner_radius` around the corners (an anti-aliased
    /// ring; `0.0` = the four straight bars).
    StrokeRect { rect: Rect, color: Color, width: Px, corner_radius: Corners },
    /// One already-wrapped line of text. `origin` is the TOP-left of the
    /// line box (the engine converts to baseline internally); `font` is
    /// the effective inherited font. The painted span is
    /// `content[range.0..range.1]` — a SLICE of the node's whole content
    /// (the `Rc` clones cheap; no String is born per line on the hot
    /// path).
    TextLine {
        origin: Point,
        content: Arc<str>,
        range: (usize, usize),
        color: Color,
        font: FontSpec,
    },
    /// One image. `rect` is the destination in logical px — the backend
    /// asks the frame's [`ImageEngine`] for pixels at the rect's
    /// PHYSICAL size, so every pipeline composites the same bytes. The
    /// command carries identity, never pixels (equality is the source
    /// key — cheap for the damage diff).
    Image { rect: Rect, source: ImageSource },
    /// From here to the paired [`DrawCommand::PopClip`], every draw is
    /// cut to this box — the rect INTERSECTS whatever clip is already
    /// open (each consumer keeps that stack), and `corner_radius` bends
    /// the cut around the corners (`0.0` = the straight rectangle this
    /// command was born as). The rect arrives in the pushing node's OWN
    /// coordinates: a curve has no corner left after somebody else's
    /// intersection, so the composition lives where the stacks live.
    PushClip { rect: Rect, corner_radius: Corners },
    PopClip,
}

/// The draw list of one frame.
#[derive(Clone, Default, Debug)]
pub struct DisplayList {
    commands: Vec<DrawCommand>,
}

impl DisplayList {
    pub(crate) fn push(&mut self, command: DrawCommand) {
        self.commands.push(command);
    }

    pub(crate) fn extend(&mut self, other: DisplayList) {
        self.commands.extend(other.commands);
    }

    pub fn iter(&self) -> impl Iterator<Item = &DrawCommand> {
        self.commands.iter()
    }

    pub fn as_slice(&self) -> &[DrawCommand] {
        &self.commands
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Fades everything pushed since `from` — what `.opacity(…)`
    /// leaves behind in the pixel pipelines. The multiply lands on the
    /// ALPHA of each command's own paint (an image takes a veil over
    /// its source), so a nested fade multiplies, exactly as a stack of
    /// layers would.
    pub(crate) fn fade_from(&mut self, from: usize, opacity: f64) {
        let scale = |alpha: u8| ((alpha as f64) * opacity).round().clamp(0.0, 255.0) as u8;
        let fade = |color: &mut Color| color.a = scale(color.a);
        for command in &mut self.commands[from..] {
            match command {
                DrawCommand::FillRect { color, .. }
                | DrawCommand::StrokeRect { color, .. }
                | DrawCommand::Shadow { color, .. }
                | DrawCommand::TextLine { color, .. } => fade(color),
                // the pane fades by its TINT and its lights: what it
                // samples is the scene, and the scene is already there
                DrawCommand::Backdrop { glass, .. } => {
                    fade(&mut glass.tint);
                    fade(&mut glass.highlight);
                    glass.sheen *= opacity;
                    glass.spot_alpha *= opacity;
                }
                DrawCommand::Gradient { paint, .. } => match paint {
                    GradientPaint::Radial { inner, outer, .. } => {
                        fade(inner);
                        fade(outer);
                    }
                    GradientPaint::Linear { from, to, .. } => {
                        fade(from);
                        fade(to);
                    }
                },
                DrawCommand::Image { source, .. } => *source = source.faded(opacity),
                DrawCommand::PushClip { .. } | DrawCommand::PopClip => {}
            }
        }
    }

    /// A translated copy of `commands[range]` — what a second surface
    /// (a popover's child panel, a canvas island) re-presents in its
    /// own coordinates. Clip pairs inside an overlay slice are
    /// balanced by construction: the subtree closes what it opens.
    pub fn translated_slice(&self, range: (usize, usize), dx: Px, dy: Px) -> DisplayList {
        let shift_rect = |rect: Rect| Rect {
            origin: Point { x: rect.origin.x + dx, y: rect.origin.y + dy },
            size: rect.size,
        };
        let commands = self
            .commands
            .get(range.0..range.1)
            .unwrap_or_default()
            .iter()
            .cloned()
            .map(|command| match command {
                DrawCommand::FillRect { rect, color, corner_radius } => {
                    DrawCommand::FillRect { rect: shift_rect(rect), color, corner_radius }
                }
                DrawCommand::StrokeRect { rect, color, width, corner_radius } => {
                    DrawCommand::StrokeRect { rect: shift_rect(rect), color, width, corner_radius }
                }
                DrawCommand::Backdrop { rect, glass, corner_radius } => DrawCommand::Backdrop {
                    rect: shift_rect(rect),
                    glass: glass.shifted(dx, dy),
                    corner_radius,
                },
                DrawCommand::Gradient { rect, paint, corner_radius } => DrawCommand::Gradient {
                    rect: shift_rect(rect),
                    paint: paint.shifted(dx, dy),
                    corner_radius,
                },
                DrawCommand::Shadow { rect, radius, color, corner_radius } => {
                    DrawCommand::Shadow { rect: shift_rect(rect), radius, color, corner_radius }
                }
                DrawCommand::TextLine { origin, content, range, color, font } => {
                    DrawCommand::TextLine {
                        origin: Point { x: origin.x + dx, y: origin.y + dy },
                        content,
                        range,
                        color,
                        font,
                    }
                }
                DrawCommand::Image { rect, source } => {
                    DrawCommand::Image { rect: shift_rect(rect), source }
                }
                DrawCommand::PushClip { rect, corner_radius } => {
                    DrawCommand::PushClip { rect: shift_rect(rect), corner_radius: corner_radius }
                }
                DrawCommand::PopClip => DrawCommand::PopClip,
            })
            .collect();
        DisplayList { commands }
    }

    /// A copy without the commands of `slices` — how a presenter leaves
    /// a live box to the box's own surface. Each slice is a balanced
    /// `start..end` run (the box closes the clip it opens), so the
    /// remainder stays balanced too.
    pub fn without_slices(&self, slices: &[(usize, usize)]) -> DisplayList {
        let keep = |index: usize| {
            !slices
                .iter()
                .any(|(start, end)| index >= *start && index < *end)
        };
        DisplayList {
            commands: self
                .commands
                .iter()
                .enumerate()
                .filter(|(index, _)| keep(*index))
                .map(|(_, command)| command.clone())
                .collect(),
        }
    }
}

/// How a split's seam and its floors are measured.
///
/// The unit rides the BINDING's type, never a builder call: a
/// `Binding<f64>` is points and a [`Binding<Fraction>`] is a share, so
/// a seam and its floors can never disagree about what they mean.
///
/// [`Binding<Fraction>`]: Fraction
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeamUnit {
    /// Points: lane A is exactly `at` wide, and everything the window
    /// gains goes to lane B.
    Points,
    /// A share of the room the two lanes have, `0..1`: both keep their
    /// SLICE when the window changes size, which is what a tree of
    /// panes wants.
    Fraction,
}

/// A seam measured as a share of its container, `0..1`.
///
/// `hsplit(state.share.binding(), a, b)` with a `State<Fraction>` is
/// the whole ceremony: the floors become shares too (a tenth of the
/// pair by default), and a resize moves both lanes together.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug, Default)]
pub struct Fraction(pub f64);

impl Fraction {
    pub fn amount(self) -> f64 {
        self.0
    }
}

/// The outputs of the placement pass: frames by identity (tests), the
/// draw list (rasterizer/backends) and the interaction targets (in paint
/// order — hit-testing scans back to front, the top one wins).
/// The directions a scroll region travels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollAxes {
    Vertical,
    Horizontal,
    Both,
}

impl ScrollAxes {
    pub fn vertical(self) -> bool {
        matches!(self, ScrollAxes::Vertical | ScrollAxes::Both)
    }

    pub fn horizontal(self) -> bool {
        matches!(self, ScrollAxes::Horizontal | ScrollAxes::Both)
    }
}

/// Where a modal layer drew its line: how long every list the pointer
/// consults was when the layer went up. Everything recorded from those
/// marks on is ABOVE the modal and still answers; what came before is
/// what the modal covers, and it is out of reach until the modal
/// closes.
///
/// ONE line across ALL of them, because a layer that captures the
/// wheel but not the right press is not a modal — it is a modal with
/// holes, and the holes are where the bugs live.
///
/// It is written on ENTRY to the layer, so the LAST write is the
/// topmost one — nested sheets and sibling sheets both resolve to the
/// one on top without a stack to keep.
#[derive(Clone, Copy, Debug)]
pub struct ModalFloor {
    pub hits: usize,
    pub scrolls: usize,
    pub tooltips: usize,
    pub menus: usize,
    pub drag_sources: usize,
    pub drops: usize,
}

/// A placed scroll region — the wheel's map, in PAINT order (last =
/// topmost), the same convention the overlays, the tooltips and the
/// pointer's own hits already keep. A child paints over its parent and
/// a layer over what it covers, so ONE walk back answers both
/// "innermost" and "on top".
#[derive(Clone, Debug)]
pub struct ScrollRegion {
    pub path: String,
    pub frame: Rect,
    pub content: Size,
    /// The declared scroll target (`.scroll_target(id)`), if any.
    pub target: Option<String>,
    /// A region under an animation scope reveals its target through
    /// this spring instead of snapping.
    pub anim: Option<crate::anim::Spring>,
    /// A virtualized region's uniform row extent (measured) — the
    /// runtime snapshots it for the next body's window math.
    pub row_extent: Option<Px>,
    /// A variable-height region's prefix-sum offsets (`offsets[i]` =
    /// row `i`'s start; last entry = total) — snapshot material too.
    pub row_offsets: Option<Rc<Vec<Px>>>,
}

/// One placed overlay: its identity, the anchor it hangs from, the
/// resolved frame, and its slice `[start, end)` of the display list —
/// a shell that wants the popover on its own surface (the mac child
/// panel) re-presents exactly that slice, translated.
#[derive(Clone, Debug)]
pub struct OverlayPlacement {
    pub path: String,
    pub anchor: Rect,
    pub frame: Rect,
    pub display: (usize, usize),
    /// The anchor still intersects its clip — a popover whose anchor
    /// scrolled away dismisses on the follow-up.
    pub anchor_visible: bool,
}

/// One `.tooltip(…)` region of the placed scene — the anchor the
/// runtime watches and the side the bubble prefers.
#[derive(Clone, Debug)]
pub struct TooltipRegion {
    pub text: Arc<str>,
    pub side: Side,
    pub rect: Rect,
}

/// One `.context_menu(…)` region of the placed scene — the items a
/// right press inside `rect` offers.
#[derive(Clone, Debug)]
pub struct MenuRegion {
    pub items: std::rc::Rc<[crate::views::MenuItem]>,
    pub rect: Rect,
}

/// The closure that builds a payload at lift — a cheap handle that
/// keeps the tree `Debug` (the pattern `Custom` set).
#[derive(Clone)]
pub struct DragBuilder(pub std::rc::Rc<dyn Fn() -> crate::views::DragPayload>);

impl std::fmt::Debug for DragBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("on_drag")
    }
}

/// The closure a drop lands in — same story.
#[derive(Clone)]
pub struct DropAction(pub std::rc::Rc<dyn Fn(&dyn std::any::Any, DropPoint)>);

impl std::fmt::Debug for DropAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("on_drop")
    }
}

/// The closure a live drag reports its position to — the app's own
/// preview (a veil over the quadrant it would split into, a marker
/// between two chips). `None` means the drag left, landed or died.
#[derive(Clone)]
pub struct DragOverAction(pub std::rc::Rc<dyn Fn(Option<DropPoint>)>);

impl std::fmt::Debug for DragOverAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("preview")
    }
}

/// One `.on_drag(…)` region of the placed scene.
#[derive(Clone)]
pub struct DragSourceRegion {
    pub payload: DragBuilder,
    pub rect: Rect,
}

impl std::fmt::Debug for DragSourceRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DragSourceRegion({:?})", self.rect)
    }
}

/// Where a drag sits inside a drop target — the answer to "the cursor
/// is HERE over this box", which is what turns one drop into a move,
/// a split toward an edge or an insertion before a chip.
///
/// The point is in the target's OWN coordinates (its top-left is zero)
/// against its OWN size — never the visible slice. A target half
/// scrolled out of view still answers honestly: its quadrants do not
/// move because part of it is off screen.
///
/// Values outside the box are legal on purpose (a pointer dragged past
/// an edge): every consumer clamps, so the type carries no invariant.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DropPoint {
    /// The pointer in the target's own coordinates.
    pub local: Point,
    /// The target's own box size — what the fraction divides by.
    pub size: Size,
}

impl DropPoint {
    /// The pointer as a FRACTION of the box: `0.0` at the origin edge,
    /// `1.0` at the far edge, each axis on its own. A quadrant, a half
    /// or an insertion index is decided from this and nothing else.
    pub fn fraction(&self) -> (Px, Px) {
        let axis = |value: Px, extent: Px| if extent > 0.0 { value / extent } else { 0.0 };
        (axis(self.local.x, self.size.width), axis(self.local.y, self.size.height))
    }
}

/// One `.on_drop(…)` region of the placed scene.
#[derive(Clone)]
pub struct DropRegion {
    pub accepts: std::any::TypeId,
    pub action: DropAction,
    pub over: Option<DragOverAction>,
    /// The VISIBLE slice — what a pointer must be inside to land here
    /// (a target scrolled half away takes a drop only on the half you
    /// can see, exactly like a hit).
    pub rect: Rect,
    /// The target's OWN box, whole. The fraction divides by this, so a
    /// clipped target never lies about where the hand is.
    pub frame: Rect,
}

impl std::fmt::Debug for DropRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DropRegion({:?})", self.rect)
    }
}

/// A live drag as the STAMP carries it — data only, the value stays
/// with the runtime and lands on the drop.
#[derive(Clone, PartialEq, Debug)]
pub struct DragLive {
    /// The label the cursor wears.
    pub label: Arc<str>,
    /// Where the pointer is.
    pub at: Point,
    /// The compatible target under the pointer, by its region RECT —
    /// geometry needs no identity, and the ring compares its own.
    pub over: Option<Rect>,
}

/// The open context menu as the STAMP carries it — data only, the
/// actions stay with the runtime and fire by index.
#[derive(Clone, PartialEq, Debug)]
pub struct MenuOpen {
    /// Where the right press landed — the panel hangs off this point.
    pub at: Point,
    /// The labels in order; `None` is a divider.
    pub entries: Vec<Option<Arc<str>>>,
    /// The row under the pointer — the runtime's, never CSS.
    pub hovered: Option<usize>,
}

/// An overlay waiting for the deferred pass (the anchor placed, the
/// popover not yet).
#[derive(Debug)]
struct QueuedOverlay {
    path: String,
    side: Side,
    node: Rc<LayoutNode>,
    anchor: Rect,
    anchor_visible: bool,
}

/// A placed text field: geometry + EFFECTIVE font at that point of the
/// scene — click-to-position and IME sync measure through here.
#[derive(Clone, Debug)]
pub struct FieldPlacement {
    pub path: String,
    pub frame: Rect,
    /// One line, or many — the runtime asks before it hands the field
    /// a break or a vertical arrow.
    pub multiline: bool,
    /// One visual line's height: the step between wrapped lines, and
    /// what a vertical arrow travels.
    pub line_height: Px,
    /// The box the run may occupy — the frame minus the field's own
    /// padding. The runtime keeps the caret inside THIS; the padding
    /// itself stays layout's business.
    pub run: Rect,
    pub text_origin: Point,
    pub font: FontSpec,
    /// The field asked for focus on first appearance.
    pub auto_focus: bool,
}

/// Which visual line owns a byte: the FIRST whose end reaches it, so a
/// caret at a break belongs to the line it was typed on and not to the
/// one after. Never empty — a field always has at least one line.
pub fn line_of(lines: &[(usize, usize)], byte: usize) -> usize {
    lines
        .iter()
        .position(|&(_, end)| byte <= end)
        .unwrap_or(lines.len().saturating_sub(1))
}

/// A placed split — the geometry the runtime needs to route a divider
/// drag back into layout coordinates (mirror of [`FieldPlacement`]).
#[derive(Clone, Debug)]
pub struct SplitPlacement {
    pub path: String,
    pub frame: Rect,
    pub axis: Axis,
    pub unit: SeamUnit,
    /// What the two lanes share — the frame's main extent minus the
    /// divider. A fractional drag divides by THIS, never by the frame.
    pub room: Px,
    pub min_a: Px,
    pub min_b: Px,
}

/// A placed escape hatch — what the runtime needs to route an event
/// into the app's own coordinates (mirror of [`FieldPlacement`]). The
/// element rides along: a skipped subtree keeps answering, because the
/// retained tree kept it.
#[derive(Clone, Debug)]
pub struct CustomPlacement {
    pub path: String,
    pub frame: Rect,
    /// What the clip stack lets through, in the box's LOCAL
    /// coordinates — the viewport a box reads to page, and the same
    /// rect its paint was given.
    pub visible: Rect,
    /// The scroll region the box sits inside, if any: where a reveal
    /// the box asks for is spent.
    pub region: Option<String>,
    /// The font the box inherited — the metrics an event resolves with.
    pub font: FontSpec,
    /// The foreground the box inherited — a live repaint hands the
    /// paint the same ink the place did.
    pub ink: Color,
    pub element: crate::custom::Custom,
    /// The loop the box paints by, when a `.looping(...)` holds it —
    /// the runtime repaints this box alone on each step of the clock.
    pub live: Option<crate::anim::Loop>,
    /// The box's own commands in the frame's display list
    /// (`start..end`) — what a live repaint replaces, and what the GPU
    /// presenter routes to the box's own layer.
    pub slice: (usize, usize),
}

/// The grip band's thickness over a split divider, in points.
pub const SPLIT_GRIP: Px = 6.0;

#[derive(Default, Debug)]
pub struct Placement {
    pub frames: Frames,
    pub display: DisplayList,
    /// A Dom frame with no live island skips the display list — the
    /// clip stack still runs, only the command collection sleeps.
    skip_display: bool,
    /// A canvas island placed this pass. When collection was OFF, the
    /// runtime re-runs the pass collected — an island's birth costs
    /// one extra walk; a steady frame costs none.
    saw_island: bool,
    pub hits: Vec<(String, Rect)>,
    pub scrolls: Vec<ScrollRegion>,
    pub fields: Vec<FieldPlacement>,
    pub splits: Vec<SplitPlacement>,
    /// The app's own boxes, in paint order — where an event goes when
    /// the hit-test lands on one.
    pub customs: Vec<CustomPlacement>,
    /// Stack of the inherited foreground — the top colors the text.
    foreground: Vec<Color>,
    /// Stack of the nearest `Interactive`'s `(hovered, pressed)` — the
    /// `Styled` picks its background by it.
    pointer: Vec<(bool, bool)>,
    /// The same, but only for targets that DECLARED themselves a group
    /// (`.hover_group()`): a descendant marked `.group_hovered()` reads
    /// this stack instead, so the pointer over a chip can light the
    /// mark inside it.
    groups: Vec<(bool, bool)>,
    /// Stack of the current clip (intersections in logical coordinates) —
    /// whoever records a hit consults it; the raster redoes the cut in
    /// physical px.
    clip: Vec<Rect>,
    /// Stack of scroll CONTENT origins — animated origins anchor here,
    /// so scrolling moves content 1:1 and never bends a spring.
    anchors: Vec<Point>,
    /// Stack of the enclosing scroll region paths — a virtual window
    /// that misses reports against the innermost one.
    region_stack: Vec<String>,
    /// Regions whose virtual window failed to cover the visible band —
    /// the runtime invalidates their boundary for a follow-up pass.
    pub misses: Vec<String>,
    /// The line the topmost modal layer drew, if any — `None` is a
    /// scene with nothing capturing, and it costs nothing.
    pub modal_floor: Option<ModalFloor>,
    /// Overlays queued by `Anchored` during the walk — drained AFTER
    /// the root places (an empty scene never allocates).
    overlay_queue: Vec<QueuedOverlay>,
    /// The placed overlays, in paint order (last = topmost).
    pub overlays: Vec<OverlayPlacement>,
    /// Window-drag regions (clipped) — where a press with no
    /// interactive target drags the window on the desktop shell.
    pub drag_regions: Vec<Rect>,
    /// The window's own buttons on a scene-drawn bar, in paint order.
    pub control_regions: Vec<(WindowControl, Rect)>,
    /// Tooltip regions, OUTER before INNER and siblings in paint order
    /// — what the runtime's hover consults, walking back so the
    /// innermost of the topmost answers. Never a hit: a tooltip
    /// explains, it does not intercept.
    pub tooltips: Vec<TooltipRegion>,
    /// Context-menu regions, in the same order — what a right press
    /// consults.
    pub menus: Vec<MenuRegion>,
    /// Drag sources, in the same order — what a press arms.
    pub drag_sources: Vec<DragSourceRegion>,
    /// Drop targets, in the same order — what a live drag consults, by
    /// geometry, through every hover gate.
    ///
    /// The order is the one [`Placement::hits`] keeps and the only one
    /// that answers BOTH questions a pointer asks: an ancestor is
    /// recorded before the subtree it holds, so the reverse walk finds
    /// the innermost target; siblings keep paint order, so the one
    /// drawn later still wins where they overlap.
    pub drops: Vec<DropRegion>,
    /// The Dom capture, when that mode is on ([`layout_dom`]) — the
    /// placement braços feed it the SEMANTIC scene while they walk.
    /// `None` costs one branch per hook and nothing else.
    pub(crate) dom: Option<crate::dom::DomCapture>,
}

impl Placement {
    /// A placement seeded with an inherited ink — an island placed
    /// LOCALLY still paints with the foreground its subtree sits in.
    pub(crate) fn with_ink(ink: Color) -> Placement {
        Placement { foreground: vec![ink], ..Placement::default() }
    }

    /// [`Placement::with_ink`] with the Dom capture riding — the
    /// `.layout(Exact)` door: a LOCAL absolute lowering of one
    /// subtree, spliced back into the flow by the caller.
    pub(crate) fn with_capture(size: Size, ink: Color) -> Placement {
        Placement {
            foreground: vec![ink],
            dom: Some(crate::dom::DomCapture::new(size)),
            ..Placement::default()
        }
    }

    /// Takes the capture out — the caller splices its scene.
    pub(crate) fn take_capture(&mut self) -> crate::dom::DomCapture {
        self.dom.take().expect("the capture was riding")
    }

    /// A draw command joins the display list — unless this pass skips
    /// collection (a Dom frame with no live island: nothing consumes
    /// the list, so nothing pays for it).
    #[inline]
    fn draw(&mut self, command: DrawCommand) {
        if !self.skip_display {
            self.display.push(command);
        }
    }

    /// The command carries this node's OWN box; the stack keeps the
    /// intersection, because a hit consults the stack. Snapping and
    /// intersecting commute (round is monotone), so the consumers'
    /// own stacks land on the same integers the old pre-intersected
    /// command did — byte for byte.
    fn push_clip(&mut self, rect: Rect, corner_radius: impl Into<Corners>) {
        let clipped = match self.clip.last() {
            Some(top) => rect
                .intersection(*top)
                .unwrap_or(Rect { origin: rect.origin, size: Size::default() }),
            None => rect,
        };
        self.draw(DrawCommand::PushClip { rect, corner_radius: corner_radius.into() });
        self.clip.push(clipped);
    }

    fn pop_clip(&mut self) {
        self.draw(DrawCommand::PopClip);
        self.clip.pop();
    }

    fn current_clip(&self) -> Option<Rect> {
        self.clip.last().copied()
    }
}

impl Rect {
    pub fn contains(&self, x: Px, y: Px) -> bool {
        x >= self.origin.x
            && y >= self.origin.y
            && x < self.origin.x + self.size.width
            && y < self.origin.y + self.size.height
    }

    /// `None` = empty intersection.
    pub fn intersection(&self, other: Rect) -> Option<Rect> {
        let x0 = self.origin.x.max(other.origin.x);
        let y0 = self.origin.y.max(other.origin.y);
        let x1 = (self.origin.x + self.size.width).min(other.origin.x + other.size.width);
        let y1 = (self.origin.y + self.size.height).min(other.origin.y + other.size.height);
        (x1 > x0 && y1 > y0).then(|| Rect {
            origin: Point { x: x0, y: y0 },
            size: Size { width: x1 - x0, height: y1 - y0 },
        })
    }
}

/// The topmost target under the point — the key of the registered action.
pub fn hit_test(hits: &[(String, Rect)], x: Px, y: Px) -> Option<&str> {
    hits.iter()
        .rev()
        .find(|(_, rect)| rect.contains(x, y))
        .map(|(path, _)| path.as_str())
}

/// The absolute frames the placement pass produces, addressable by the
/// identity path of the boundaries.
#[derive(Default, Debug)]
pub struct Frames {
    entries: Vec<(String, Rect)>,
}

impl Frames {
    fn record(&mut self, path: &str, frame: Rect) {
        self.entries.push((path.to_string(), frame));
    }

    /// The exact frame for the path (the first one, if repeated).
    pub fn get(&self, path: &str) -> Option<Rect> {
        self.entries
            .iter()
            .find(|(entry, _)| entry == path)
            .map(|(_, frame)| *frame)
    }

    /// The first frame whose path ends with the suffix — a short address
    /// for tests (`"CountryCell"` instead of the whole path).
    pub fn find(&self, suffix: &str) -> Option<Rect> {
        self.entries
            .iter()
            .find(|(entry, _)| entry.ends_with(suffix))
            .map(|(_, frame)| *frame)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Rect)> {
        self.entries.iter().map(|(path, frame)| (path.as_str(), *frame))
    }
}

/// The result of the full pass: the size the root answered, the frames,
/// the draw list and the interaction targets.
#[derive(Debug)]
pub struct LayoutResult {
    pub size: Size,
    pub frames: Frames,
    pub display: DisplayList,
    pub hits: Vec<(String, Rect)>,
    pub scrolls: Vec<ScrollRegion>,
    pub fields: Vec<FieldPlacement>,
    pub splits: Vec<SplitPlacement>,
    /// The app's own boxes — the addresses an event resolves against.
    pub customs: Vec<CustomPlacement>,
    /// Virtual windows that failed to cover the visible band this
    /// frame — the runtime re-materializes them in a follow-up pass.
    pub misses: Vec<String>,
    /// A canvas island placed while display collection was off — the
    /// runtime re-runs the pass collected.
    pub(crate) saw_island: bool,
    /// The placed popovers, in paint order (last = topmost) — each one
    /// a suffix slice of `display`.
    pub overlays: Vec<OverlayPlacement>,
    /// The line the topmost modal layer drew — the pointer and the
    /// wheel do not look under it.
    pub modal_floor: Option<ModalFloor>,
    /// Window-drag regions — a press here with no interactive target
    /// drags the window on the desktop shell.
    pub drag_regions: Vec<Rect>,
    /// The window's own buttons on a scene-drawn bar, in paint order.
    pub control_regions: Vec<(WindowControl, Rect)>,
    /// Tooltip regions, OUTER before INNER and siblings in paint order
    /// — what the runtime's hover consults, walking back so the
    /// innermost of the topmost answers. Never a hit: a tooltip
    /// explains, it does not intercept.
    pub tooltips: Vec<TooltipRegion>,
    /// Context-menu regions, in the same order — what a right press
    /// consults.
    pub menus: Vec<MenuRegion>,
    /// Drag sources, in the same order — what a press arms.
    pub drag_sources: Vec<DragSourceRegion>,
    /// Drop targets, in the same order — what a live drag consults, by
    /// geometry, through every hover gate.
    ///
    /// The order is the one [`Placement::hits`] keeps and the only one
    /// that answers BOTH questions a pointer asks: an ancestor is
    /// recorded before the subtree it holds, so the reverse walk finds
    /// the innermost target; siblings keep paint order, so the one
    /// drawn later still wins where they overlap.
    pub drops: Vec<DropRegion>,
}

/// Runs both phases from the root with the default environment — the
/// deterministic [`PixelFont`], a fresh cache, no scroll offsets (tests
/// and direct use; the `Runtime` builds the real env in
/// [`layout_with`]).
///
/// [`PixelFont`]: crate::text_engine::PixelFont
pub fn layout(root: &LayoutNode, proposal: Proposal) -> LayoutResult {
    let engine = PixelFont;
    let images = RawImages::default();
    let cache = MeasureCache::default();
    let offsets = HashMap::default();
    let interaction = Interaction::default();
    let carets = HashMap::default();
    layout_with(
        root,
        proposal,
        LayoutEnv {
            text: &engine,
            images: &images,
            cache: &cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
            line_height: None,
            stamp: FrameStamp::idle(&interaction, &carets),
            animator: None,
            anim: None,
            live: None,
            overlay_bounds: None,
            scale: 1.0,
        },
    )
}

/// Runs both phases with the frame's environment.
pub fn layout_with(root: &LayoutNode, proposal: Proposal, env: LayoutEnv) -> LayoutResult {
    let (size, fit) = root.measure(proposal, env);
    let mut out = Placement::default();
    root.place(Rect { origin: Point::default(), size }, fit, env, &mut out);
    // popovers place AFTER the root: painted on top, hit first, free
    // of every scroll clip. Their default container is the WINDOW (the
    // proposal), never the root's answer — a small scene must not
    // shrink the room a popover positions in.
    place_overlays(window_bounds(proposal, size), env, &mut out);
    LayoutResult {
        size,
        frames: out.frames,
        display: out.display,
        hits: out.hits,
        scrolls: out.scrolls,
        fields: out.fields,
        splits: out.splits,
        customs: out.customs,
        misses: out.misses,
        saw_island: out.saw_island,
        overlays: out.overlays,
        modal_floor: out.modal_floor,
        drag_regions: out.drag_regions,
        control_regions: out.control_regions,
        tooltips: out.tooltips,
        menus: out.menus,
        drag_sources: out.drag_sources,
        drops: out.drops,
    }
}

/// Runs both phases WITH the Dom capture on: the same walk, plus the
/// semantic scene collected on the way ([`crate::dom`]). The regular
/// [`layout_with`] never pays for the sink.
pub fn layout_dom(
    root: &LayoutNode,
    proposal: Proposal,
    env: LayoutEnv,
    collect_display: bool,
) -> (LayoutResult, crate::dom::DomNode) {
    let (size, fit) = root.measure(proposal, env);
    let mut out = Placement {
        dom: Some(crate::dom::DomCapture::new(size)),
        skip_display: !collect_display,
        ..Placement::default()
    };
    root.place(Rect { origin: Point::default(), size }, fit, env, &mut out);
    // the capture is still open at the root here, so every popover
    // mounts as the root's LAST child — outside every scroll element,
    // stacked on top by document order: the portal, by construction
    place_overlays(window_bounds(proposal, size), env, &mut out);
    let scene = out.dom.take().expect("the capture stays for the whole walk").finish();
    (
        LayoutResult {
            size,
            frames: out.frames,
            display: out.display,
            hits: out.hits,
            scrolls: out.scrolls,
            fields: out.fields,
            splits: out.splits,
            customs: out.customs,
            misses: out.misses,
            saw_island: out.saw_island,
            overlays: out.overlays,
            modal_floor: out.modal_floor,
            drag_regions: out.drag_regions,
            control_regions: out.control_regions,
            tooltips: out.tooltips,
            menus: out.menus,
            drag_sources: out.drag_sources,
            drops: out.drops,
        },
        scene,
    )
}

/// The image's answer to a proposal. Not decoded yet (`None`
/// dimensions) answers zero on every path — the layout reflows when the
/// platform reports in.
fn image_size(
    intrinsic: Option<(u32, u32)>,
    resizable: bool,
    fit: Option<ContentMode>,
    proposal: Proposal,
) -> Size {
    let Some((width, height)) = intrinsic else {
        return Size::default();
    };
    let (width, height) = (width as Px, height as Px);
    if !resizable {
        // 1 pixel = 1 point in v1 — the browser's own default contract
        return Size { width, height };
    }
    match fit {
        // contain: the largest rect with the intrinsic ratio inside the
        // proposed box; one open axis derives from the other
        Some(ContentMode::Fit) => {
            let scale = match (proposal.width, proposal.height) {
                (Some(pw), Some(ph)) => (pw / width).min(ph / height),
                (Some(pw), None) => pw / width,
                (None, Some(ph)) => ph / height,
                (None, None) => 1.0,
            }
            .max(0.0);
            Size { width: width * scale, height: height * scale }
        }
        // cover (`Fill`) and plain stretch both answer the box EXACTLY;
        // an open axis falls back to the intrinsic length
        Some(ContentMode::Fill) | None => Size {
            width: proposal.width.unwrap_or(width).max(0.0),
            height: proposal.height.unwrap_or(height).max(0.0),
        },
    }
}

/// The window rect overlays position against when no explicit bounds
/// are set: the PROPOSAL when the axis was proposed, the root's answer
/// where it was open.
fn window_bounds(proposal: Proposal, size: Size) -> Rect {
    Rect {
        origin: Point::default(),
        size: Size {
            width: proposal.width.unwrap_or(size.width),
            height: proposal.height.unwrap_or(size.height),
        },
    }
}

/// The gap between an anchor and its popover, logical px — fixed in
/// v1 (a knob waits for a real need).
const OVERLAY_GAP: Px = 6.0;

/// How many queued overlays one pass resolves — nested popovers queue
/// while their parent places; the cap is the livelock guard.
const OVERLAY_CAP: usize = 8;

/// The popover frame for one anchor: the preferred side first, the
/// FLIP when it has no room, the roomier side when neither fits — and
/// a final clamp into the container on both axes. The size never
/// shrinks here (the measure already capped it at the container).
fn anchored_frame(anchor: Rect, side: Side, size: Size, container: Rect) -> Rect {
    let origin_for = |side: Side| -> Point {
        match side {
            Side::Bottom => Point {
                x: anchor.origin.x + (anchor.size.width - size.width) / 2.0,
                y: anchor.origin.y + anchor.size.height + OVERLAY_GAP,
            },
            Side::Top => Point {
                x: anchor.origin.x + (anchor.size.width - size.width) / 2.0,
                y: anchor.origin.y - OVERLAY_GAP - size.height,
            },
            Side::Trailing => Point {
                x: anchor.origin.x + anchor.size.width + OVERLAY_GAP,
                y: anchor.origin.y + (anchor.size.height - size.height) / 2.0,
            },
            Side::Leading => Point {
                x: anchor.origin.x - OVERLAY_GAP - size.width,
                y: anchor.origin.y + (anchor.size.height - size.height) / 2.0,
            },
        }
    };
    // room on the MAIN axis only — the cross axis always clamps
    let fits = |side: Side| -> bool {
        let origin = origin_for(side);
        match side {
            Side::Top | Side::Bottom => {
                origin.y >= container.origin.y
                    && origin.y + size.height <= container.origin.y + container.size.height
            }
            Side::Leading | Side::Trailing => {
                origin.x >= container.origin.x
                    && origin.x + size.width <= container.origin.x + container.size.width
            }
        }
    };
    let chosen = if fits(side) {
        side
    } else if fits(side.opposite()) {
        side.opposite()
    } else {
        // neither fits whole: the roomier side keeps more visible
        let room = |side: Side| -> Px {
            match side {
                Side::Bottom => {
                    container.origin.y + container.size.height
                        - (anchor.origin.y + anchor.size.height)
                }
                Side::Top => anchor.origin.y - container.origin.y,
                Side::Trailing => {
                    container.origin.x + container.size.width
                        - (anchor.origin.x + anchor.size.width)
                }
                Side::Leading => anchor.origin.x - container.origin.x,
            }
        };
        if room(side) >= room(side.opposite()) { side } else { side.opposite() }
    };
    let origin = origin_for(chosen);
    let clamp = |value: Px, low: Px, high: Px| value.min(high).max(low);
    Rect {
        origin: Point {
            x: clamp(
                origin.x,
                container.origin.x,
                container.origin.x + (container.size.width - size.width).max(0.0),
            ),
            y: clamp(
                origin.y,
                container.origin.y,
                container.origin.y + (container.size.height - size.height).max(0.0),
            ),
        },
        size,
    }
}

/// Drains the overlay queue AFTER the root placed: every popover
/// measures against the container, resolves its side and lands at the
/// END of the display list — on top by paint order, hit-priority by
/// position, outside every clip (the stack is empty here), and inside
/// the still-open Dom root (the portal). A popover opened inside a
/// popover queues during its parent's place and drains in the same
/// loop.
/// The path every shell recognizes as the tooltip's — the mac child
/// panel pools by it; the dismissal doors leave it alone (a tooltip
/// never eats a click, hover-out is its whole life).
pub const TOOLTIP_PATH: &str = "bunny.tooltip";

/// The context menu's overlay path — pooled like a popover's panel,
/// but the RUNTIME owns its doors, not the reconciler.
pub const MENU_PATH: &str = "bunny.menu";

/// The drag label's overlay path — the chip that follows the cursor.
pub const DRAG_LABEL_PATH: &str = "bunny.drag";

/// The menu's shared geometry — the builder below and the runtime's
/// row mapping must walk the SAME numbers, so they live in one place.
pub(crate) const MENU_ROW_H: Px = 24.0;
pub(crate) const MENU_DIVIDER_H: Px = 9.0;
pub(crate) const MENU_PAD_V: Px = 5.0;
pub(crate) const MENU_PAD_H: Px = 10.0;
pub(crate) const MENU_MIN_W: Px = 160.0;

/// The row index under a point inside an open menu's frame — `None`
/// on padding, a divider, or outside. The runtime maps a press or a
/// move through this; the panel painted THESE rows, so the two agree.
pub(crate) fn menu_row_at(
    frame: Rect,
    entries: &[Option<Arc<str>>],
    x: Px,
    y: Px,
) -> Option<usize> {
    if !frame.contains(x, y) {
        return None;
    }
    let mut top = frame.origin.y + MENU_PAD_V;
    for (index, entry) in entries.iter().enumerate() {
        let height = if entry.is_some() { MENU_ROW_H } else { MENU_DIVIDER_H };
        if y >= top && y < top + height {
            return entry.as_ref().map(|_| index);
        }
        top += height;
    }
    None
}

/// The menu panel: a themed card of rows. The hovered row is the
/// STAMP's (the runtime tracks it, so the pixel modes highlight), and
/// each row also declares its CSS hover — the element mode gets the
/// same highlight with zero patches, its own way.
fn menu_node(open: &MenuOpen, env: LayoutEnv) -> LayoutNode {
    let theme = crate::theme::current();
    let mut rows: Vec<LayoutNode> = Vec::with_capacity(open.entries.len());
    for (index, entry) in open.entries.iter().enumerate() {
        match entry {
            Some(label) => {
                let hovered = open.hovered == Some(index);
                rows.push(LayoutNode::Styled {
                    props: Box::new(VisualProps {
                        background: hovered.then_some(theme.accent),
                        background_hovered: Some(theme.accent),
                        foreground: if hovered {
                            Some(Color::WHITE)
                        } else {
                            Some(theme.fg)
                        },
                        foreground_hovered: Some(Color::WHITE),
                        corner_radius: Some(Corners::all(4.0)),
                        ..VisualProps::default()
                    }),
                    child: Box::new(LayoutNode::Frame {
                        width: None,
                        height: Some(MENU_ROW_H),
                        child: Box::new(LayoutNode::Stack {
                            axis: Axis::Horizontal,
                            spacing: 0.0,
                            align: CrossAlign::Center,
                            children: vec![
                                LayoutNode::Padding {
                                    edges: Edges {
                                        top: 0.0,
                                        bottom: 0.0,
                                        leading: MENU_PAD_H,
                                        trailing: MENU_PAD_H,
                                    },
                                    child: Box::new(LayoutNode::Text {
                                        content: label.clone(),
                                        highlights: None,
                                        truncation: None,
                                    }),
                                },
                                LayoutNode::Spacer,
                            ],
                        }),
                    }),
                });
            }
            None => {
                // the quiet line between groups: a hairline, inset
                rows.push(LayoutNode::Frame {
                    width: None,
                    height: Some(MENU_DIVIDER_H),
                    child: Box::new(LayoutNode::Padding {
                        edges: Edges {
                            top: 4.0,
                            bottom: 4.0,
                            leading: MENU_PAD_H,
                            trailing: MENU_PAD_H,
                        },
                        child: Box::new(LayoutNode::Styled {
                            props: Box::new(VisualProps {
                                background: Some(theme.border),
                                ..VisualProps::default()
                            }),
                            child: Box::new(LayoutNode::Fill),
                        }),
                    }),
                });
            }
        }
    }
    let _ = env;
    LayoutNode::Styled {
        props: Box::new(VisualProps {
            background: Some(theme.panel),
            border: Some((theme.border, 1.0)),
            corner_radius: Some(Corners::all(7.0)),
            shadow: Some((18.0, Color { r: 0, g: 0, b: 0, a: 80 })),
            clip: true,
            font: FontPatch { size: Some(13.0), ..FontPatch::default() },
            ..VisualProps::default()
        }),
        child: Box::new(LayoutNode::Padding {
            edges: Edges {
                top: MENU_PAD_V,
                bottom: MENU_PAD_V,
                leading: 0.0,
                trailing: 0.0,
            },
            child: Box::new(LayoutNode::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: CrossAlign::Start,
                children: rows,
            }),
        }),
    }
}

/// The menu frame: the panel hangs down-right of the press, flips up
/// or left when the container has no room, and clamps like everything
/// anchored.
fn menu_frame(at: Point, size: Size, container: Rect) -> Rect {
    let mut x = at.x;
    let mut y = at.y;
    if y + size.height > container.origin.y + container.size.height {
        y = at.y - size.height;
    }
    if x + size.width > container.origin.x + container.size.width {
        x = at.x - size.width;
    }
    let clamp = |value: Px, low: Px, high: Px| value.min(high).max(low);
    Rect {
        origin: Point {
            x: clamp(
                x,
                container.origin.x,
                container.origin.x + (container.size.width - size.width).max(0.0),
            ),
            y: clamp(
                y,
                container.origin.y,
                container.origin.y + (container.size.height - size.height).max(0.0),
            ),
        },
        size,
    }
}

/// The framework-drawn bubble: a small inverted label. The theme's
/// ink becomes the ground and the canvas becomes the text — legible
/// on both themes without a token of its own.
fn tooltip_node(text: Arc<str>) -> LayoutNode {
    let theme = crate::theme::current();
    LayoutNode::Styled {
        props: Box::new(VisualProps {
            background: Some(Color { a: 242, ..theme.fg }),
            foreground: Some(theme.canvas),
            corner_radius: Some(Corners::all(5.0)),
            shadow: Some((10.0, Color { r: 0, g: 0, b: 0, a: 90 })),
            font: FontPatch { size: Some(11.0), ..FontPatch::default() },
            ..VisualProps::default()
        }),
        child: Box::new(LayoutNode::Padding {
            edges: Edges { top: 3.0, trailing: 7.0, bottom: 4.0, leading: 7.0 },
            child: Box::new(LayoutNode::Text {
                content: text,
                highlights: None,
                truncation: None,
            }),
        }),
    }
}

fn place_overlays(viewport: Rect, env: LayoutEnv, out: &mut Placement) {
    let container = env.overlay_bounds.unwrap_or(viewport);
    let mut placed = 0;
    while !out.overlay_queue.is_empty() && placed < OVERLAY_CAP {
        placed += 1;
        let queued = out.overlay_queue.remove(0);
        let proposal = Proposal {
            width: Some(container.size.width),
            height: Some(container.size.height),
        };
        let (size, fit) = queued.node.measure(proposal, env);
        let frame = anchored_frame(queued.anchor, queued.side, size, container);
        let start = out.display.len();
        queued.node.place(frame, fit, env, out);
        let end = out.display.len();
        out.overlays.push(OverlayPlacement {
            path: queued.path,
            anchor: queued.anchor,
            frame,
            display: (start, end),
            anchor_visible: queued.anchor_visible,
        });
    }
    // the menu lands above every popover — the runtime opened it, the
    // runtime will close it, and its rows highlight from the stamp
    if let Some(open) = env.stamp.interaction.menu.clone() {
        let node = menu_node(&open, env);
        // natural width, floored at the house minimum; the height is
        // the rows' own
        let (natural, _) = node.measure(
            Proposal { width: None, height: Some(container.size.height) },
            env,
        );
        let size = Size {
            width: natural.width.max(MENU_MIN_W).min(container.size.width),
            height: natural.height.min(container.size.height),
        };
        let (_, fit) = node.measure(
            Proposal { width: Some(size.width), height: Some(size.height) },
            env,
        );
        let frame = menu_frame(open.at, size, container);
        let start = out.display.len();
        node.place(frame, fit, env, out);
        let end = out.display.len();
        out.overlays.push(OverlayPlacement {
            path: MENU_PATH.to_string(),
            anchor: Rect { origin: open.at, size: Size::default() },
            frame,
            display: (start, end),
            anchor_visible: true,
        });
    }
    // the drag label rides the cursor — the same bubble the tooltip
    // wears, hung off the pointer, above everything (on the desktop it
    // leaves the window with the pointer)
    if let Some(live) = env.stamp.interaction.drag.clone() {
        let node = tooltip_node(live.label);
        let proposal = Proposal {
            width: Some(container.size.width),
            height: Some(container.size.height),
        };
        let (size, fit) = node.measure(proposal, env);
        let at = Point { x: live.at.x + 14.0, y: live.at.y + 16.0 };
        let clamp = |value: Px, low: Px, high: Px| value.min(high).max(low);
        let frame = Rect {
            origin: Point {
                x: clamp(
                    at.x,
                    container.origin.x,
                    container.origin.x + (container.size.width - size.width).max(0.0),
                ),
                y: clamp(
                    at.y,
                    container.origin.y,
                    container.origin.y + (container.size.height - size.height).max(0.0),
                ),
            },
            size,
        };
        let start = out.display.len();
        node.place(frame, fit, env, out);
        let end = out.display.len();
        out.overlays.push(OverlayPlacement {
            path: DRAG_LABEL_PATH.to_string(),
            anchor: Rect { origin: live.at, size: Size::default() },
            frame,
            display: (start, end),
            anchor_visible: true,
        });
    }
    // the tooltip lands LAST — above every popover, outside every
    // clip, and on the desktop it leaves the window like they do
    if let Some((text, side, anchor)) = env.stamp.interaction.tooltip.clone() {
        let node = tooltip_node(text);
        let proposal = Proposal {
            width: Some(container.size.width),
            height: Some(container.size.height),
        };
        let (size, fit) = node.measure(proposal, env);
        let frame = anchored_frame(anchor, side, size, container);
        let start = out.display.len();
        node.place(frame, fit, env, out);
        let end = out.display.len();
        out.overlays.push(OverlayPlacement {
            path: TOOLTIP_PATH.to_string(),
            anchor,
            frame,
            display: (start, end),
            anchor_visible: true,
        });
    }
}

/// The cover rect: the smallest rect with the intrinsic ratio that
/// fills the frame completely, centered. `None` = nothing to size
/// against (undecoded, zero anywhere).
fn cover_rect(frame: Rect, intrinsic: Option<(u32, u32)>) -> Option<Rect> {
    let (width, height) = intrinsic?;
    let (width, height) = (width as Px, height as Px);
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        return None;
    }
    let scale = (frame.size.width / width).max(frame.size.height / height);
    let size = Size { width: width * scale, height: height * scale };
    Some(Rect {
        origin: Point {
            x: frame.origin.x + (frame.size.width - size.width) / 2.0,
            y: frame.origin.y + (frame.size.height - size.height) / 2.0,
        },
        size,
    })
}

impl LayoutNode {
    /// Flexible = wants the leftover space on the axis (the basis of
    /// stack distribution). Explicit priority, never a side effect of
    /// overflow.
    pub(crate) fn is_flexible(&self, axis: Axis, enclosing_main: Option<Axis>) -> bool {
        match self {
            // A spacer is flexible only on the MAIN axis of the stack that
            // holds it, never across it — so a bar of them never makes its
            // row take the leftover HEIGHT. With no stack in reach (a
            // spacer measured on its own) it keeps the old bi-axial answer.
            LayoutNode::Spacer => enclosing_main.map_or(true, |main| axis == main),
            // a fill FILLS the offer on both axes — legitimately bi-axial
            LayoutNode::Fill => true,
            // a split FILLS the offer on both axes — its whole job is
            // dividing the room it was given
            LayoutNode::Split { .. } => true,
            LayoutNode::Scroll { axes, .. } => match axis {
                Axis::Vertical => axes.vertical(),
                Axis::Horizontal => axes.horizontal(),
            },
            // a field takes the offered width (like the real TextField),
            // and a many-line one takes the offered HEIGHT: `.frame_height`
            // then sizes the BOX, instead of centring one line in a hole
            LayoutNode::Field { multiline, .. } => axis == Axis::Horizontal || *multiline,
            LayoutNode::MaxFrame { max_width, max_height, child, .. } => match axis {
                Axis::Horizontal => {
                    max_width.is_infinite() || child.is_flexible(axis, enclosing_main)
                }
                Axis::Vertical => {
                    max_height.is_infinite() || child.is_flexible(axis, enclosing_main)
                }
            },
            LayoutNode::Frame { width, height, child } => match axis {
                Axis::Horizontal => width.is_none() && child.is_flexible(axis, enclosing_main),
                Axis::Vertical => height.is_none() && child.is_flexible(axis, enclosing_main),
            },
            // the layer is invisible to the question: a rule wide
            // enough to cross the box must never make the box flexible
            LayoutNode::Overlay { child, .. } => child.is_flexible(axis, enclosing_main),
            LayoutNode::Padding { child, .. }
            | LayoutNode::Interactive { child, .. }
            | LayoutNode::HoverGroup { child, .. }
            | LayoutNode::Styled { child, .. }
            | LayoutNode::Animated { child, .. }
            | LayoutNode::Island { child, .. }
            | LayoutNode::Live { child, .. }
            | LayoutNode::Anchored { child, .. }
            | LayoutNode::DragRegion { child }
            | LayoutNode::ControlRegion { child, .. }
            | LayoutNode::Tooltip { child, .. }
            | LayoutNode::ContextSource { child, .. }
            | LayoutNode::DragSource { child, .. }
            | LayoutNode::DropTarget { child, .. }
            | LayoutNode::Hinted { child, .. } => child.is_flexible(axis, enclosing_main),
            // a stack that HOLDS something flexible is itself flexible
            // (a panel with a scroll inside wants the leftover space —
            // nesting it must not freeze it at its natural extent). It
            // also RENAMES the main axis: its children answer against its
            // own axis, not the grandparent's.
            LayoutNode::Stack { children, axis: main, .. } => {
                children.iter().any(|child| child.is_flexible(axis, Some(*main)))
            }
            // a layer pile has no main axis of its own — the question
            // passes through it unchanged
            LayoutNode::Layered { children, .. } => {
                children.iter().any(|child| child.is_flexible(axis, enclosing_main))
            }
            LayoutNode::Boundary { children, .. } => {
                children.len() == 1 && children[0].is_flexible(axis, enclosing_main)
            }
            // the app answers for its own box, per axis (the default is
            // yes on both, the same answer a Rectangle gives)
            LayoutNode::Custom { element, .. } => element.element().flexible(axis),
            // skipped boundary: the flexibility is the retained tree's
            LayoutNode::BoundaryRef { path } => crate::reconciler::with_retained_layout(
                path,
                |layout| {
                    layout.map(|node| node.is_flexible(axis, enclosing_main)).unwrap_or(false)
                },
            ),
            _ => false,
        }
    }

    /// The FIRST baseline of this subtree, in the node's own
    /// coordinates — text answers with its ascent, wrappers forward
    /// (padding adds its top inset, styled swaps the font first, the
    /// way measure does), and a subtree with no text answers `None`:
    /// the caller then uses the bottom edge (the rule for baselineless
    /// boxes). Only the baseline alignment walks this; everyone else
    /// pays nothing.
    fn first_baseline(&self, env: LayoutEnv) -> Option<Px> {
        match self {
            LayoutNode::Text { content, .. } => {
                Some(env.cache.get_or_measure(content, &env.font, env.text).ascent)
            }
            LayoutNode::Field { .. } => {
                let metrics = env.cache.get_or_measure("0", &env.font, env.text);
                Some(FIELD_PAD_V + metrics.ascent)
            }
            LayoutNode::Styled { props, child } => {
                let env = LayoutEnv {
                    font: props.font.apply_over(env.font),
                    line_height: props.line_height.or(env.line_height),
                    ..env
                };
                child.first_baseline(env)
            }
            LayoutNode::Overlay { child, .. } => child.first_baseline(env),
            LayoutNode::Animated { child, .. }
            | LayoutNode::Island { child, .. }
            | LayoutNode::Live { child, .. }
            | LayoutNode::Interactive { child, .. }
            | LayoutNode::HoverGroup { child, .. }
            | LayoutNode::Anchored { child, .. }
            | LayoutNode::DragRegion { child }
            | LayoutNode::ControlRegion { child, .. }
            | LayoutNode::Tooltip { child, .. }
            | LayoutNode::ContextSource { child, .. }
            | LayoutNode::DragSource { child, .. }
            | LayoutNode::DropTarget { child, .. }
            | LayoutNode::Hinted { child, .. }
            | LayoutNode::Frame { child, .. } => child.first_baseline(env),
            // lane A leads the seam — its text sets the shared line
            LayoutNode::Split { children, .. } => {
                children.first().and_then(|child| child.first_baseline(env))
            }
            LayoutNode::Padding { edges, child } => {
                child.first_baseline(env).map(|baseline| baseline + edges.top)
            }
            LayoutNode::Stack { children, .. } => {
                children.first().and_then(|child| child.first_baseline(env))
            }
            LayoutNode::Boundary { children, .. } => {
                children.first().and_then(|child| child.first_baseline(env))
            }
            LayoutNode::BoundaryRef { path } => crate::reconciler::with_retained_layout(
                path,
                |layout| layout.and_then(|node| node.first_baseline(env)),
            ),
            _ => None,
        }
    }

    pub(crate) fn measure(&self, proposal: Proposal, env: LayoutEnv) -> (Size, Fit) {
        match self {
            LayoutNode::Text { content, truncation, .. } => {
                let metrics = env.cache.get_or_measure(content, &env.font, env.text);
                let natural = metrics.width;
                // the line box a paragraph steps by: the face's own box,
                // or the inherited `.line_height(…)` when one is set. With
                // none, it is exactly the old `ascent + descent`.
                let advance = env.line_height.unwrap_or(metrics.height());
                // REAL word wrapping, with the engine's measurements —
                // the width goes into the cache key (probe mode);
                // truncation turns wrapping off: one line, always
                let size = match proposal.width {
                    Some(width) if width > 0.0 && width < natural => {
                        if truncation.is_some() {
                            Size { width, height: advance }
                        } else {
                            let lines =
                                env.cache.get_or_break(content, &env.font, width, env.text);
                            Size { width, height: lines.len() as Px * advance }
                        }
                    }
                    _ => Size { width: natural, height: advance },
                };
                (size, Fit::Leaf)
            }

            LayoutNode::Spacer | LayoutNode::Fill => {
                let size = Size {
                    width: proposal.width.unwrap_or(0.0),
                    height: proposal.height.unwrap_or(0.0),
                };
                (size, Fit::Leaf)
            }

            LayoutNode::Field { content, placeholder, multiline, .. } => {
                let sample: &str = if content.is_empty() { placeholder } else { content };
                let metrics = env.cache.get_or_measure(sample, &env.font, env.text);
                let natural = metrics.width + 2.0 * FIELD_PAD_H;
                let width = proposal.width.unwrap_or(natural);
                let height = match multiline {
                    // the box the parent gives, and the text wraps
                    // INSIDE it: a long line never widens the column,
                    // and a tall text never grows the box — it scrolls
                    true => proposal.height.unwrap_or_else(|| {
                        let inner = (width - 2.0 * FIELD_PAD_H).max(1.0);
                        let lines = env.cache.get_or_break(sample, &env.font, inner, env.text);
                        lines.len() as Px * metrics.height() + 2.0 * FIELD_PAD_V
                    }),
                    false => metrics.height() + 2.0 * FIELD_PAD_V,
                };
                (Size { width, height }, Fit::Leaf)
            }

            LayoutNode::Leaf { size } => (*size, Fit::Leaf),

            // the escape hatch measures itself, with the frame's text
            // metrics in hand
            LayoutNode::Custom { element, .. } => {
                let metrics = crate::custom::Metrics::new(env.text, env.cache, env.font);
                (element.element().measure(proposal, &metrics), Fit::Leaf)
            }

            LayoutNode::Image { source, resizable, fit } => {
                let size = match source {
                    // the print-parity stub keeps the old rigid box
                    None => Size { width: 40.0, height: 40.0 },
                    Some(source) => {
                        image_size(intrinsic_of(&*env.images, source), *resizable, *fit, proposal)
                    }
                };
                (size, Fit::Leaf)
            }

            LayoutNode::Icon { resizable, .. } => {
                // a synchronous square — no decode, no reflow, ever
                let side = crate::icon::natural_size(&env.font) as u32;
                (image_size(Some((side, side)), *resizable, None, proposal), Fit::Leaf)
            }

            // the anchor's geometry IS the node's — the overlay never
            // participates in the measure
            LayoutNode::Anchored { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::DragRegion { child } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::ControlRegion { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Tooltip { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::ContextSource { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::DragSource { child, .. } | LayoutNode::DropTarget { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Hinted { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::BoundaryHint { .. } => (Size::default(), Fit::Leaf),

            LayoutNode::ExactLayout { child } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Stack { axis, spacing, children, .. } => {
                measure_stack(*axis, *spacing, children, proposal, env)
            }

            LayoutNode::VirtualStack { row_extent, count, children, heights } => {
                let child_proposal = Proposal { width: proposal.width, height: None };
                let measured: Vec<(usize, Size, Fit)> = children
                    .iter()
                    .map(|(index, child)| {
                        let (size, fit) = child.measure(child_proposal, env);
                        (*index, size, fit)
                    })
                    .collect();
                let width = proposal.width.unwrap_or_else(|| {
                    measured
                        .iter()
                        .fold(0.0_f64, |acc, (_, size, _)| acc.max(size.width))
                });
                match heights {
                    // variable heights: the closure is the authority —
                    // offsets are prefix sums, the total is honest to
                    // every row that does not exist
                    Some(heights) => {
                        let mut offsets = Vec::with_capacity(*count + 1);
                        let mut total: Px = 0.0;
                        offsets.push(0.0);
                        for index in 0..*count {
                            total += (heights.0)(index).max(0.0);
                            offsets.push(total);
                        }
                        (
                            Size { width, height: total },
                            Fit::Virtual {
                                row_extent: 0.0,
                                children: measured,
                                offsets: Some(Rc::new(offsets)),
                            },
                        )
                    }
                    None => {
                        // the first measured row is the authoritative
                        // extent — the node's field only seeded the
                        // body's window math
                        let row = measured
                            .first()
                            .map(|(_, size, _)| size.height)
                            .filter(|height| *height > 0.0)
                            .unwrap_or(*row_extent)
                            .max(0.0);
                        (
                            Size { width, height: row * *count as Px },
                            Fit::Virtual {
                                row_extent: row,
                                children: measured,
                                offsets: None,
                            },
                        )
                    }
                }
            }

            LayoutNode::Overlay { layer, child, .. } => {
                // the base answers ALONE — the layer never enters a max
                let (size, base_fit) = child.measure(proposal, env);
                // and the layer negotiates against the RESOLVED box, so
                // a `Fill` inside it takes exactly what the base took
                let (layer_size, layer_fit) = layer.measure(Proposal::exact(size), env);
                (size, Fit::Children(vec![(size, base_fit), (layer_size, layer_fit)]))
            }

            LayoutNode::Layered { children, .. } => {
                let mut measured: Vec<(Size, Fit)> =
                    children.iter().map(|child| child.measure(proposal, env)).collect();
                let mut size = measured.iter().fold(Size::default(), |acc, (size, _)| Size {
                    width: acc.width.max(size.width),
                    height: acc.height.max(size.height),
                });
                // The stack's last pass, on both axes at once: a layer too
                // big to shrink decides the box, and a layer that CAN take
                // that box was measured against a smaller one. Centring it
                // there would walk a full-bleed wash off the corner it was
                // painted for. Asked again, once.
                for (index, child) in children.iter().enumerate() {
                    let (answered, _) = &measured[index];
                    let wider = size.width - answered.width > SETTLED
                        && child.is_flexible(Axis::Horizontal, None);
                    let taller = size.height - answered.height > SETTLED
                        && child.is_flexible(Axis::Vertical, None);
                    if !wider && !taller {
                        continue;
                    }
                    let asked = Proposal {
                        width: Some(if wider { size.width } else { answered.width }),
                        height: Some(if taller { size.height } else { answered.height }),
                    };
                    measured[index] = child.measure(asked, env);
                    size.width = size.width.max(measured[index].0.width);
                    size.height = size.height.max(measured[index].0.height);
                }
                (size, Fit::Children(measured))
            }

            LayoutNode::Padding { edges, child } => {
                let inset = |length: Option<Px>, total: Px| {
                    length.map(|length| (length - total).max(0.0))
                };
                let (child_size, fit) = child.measure(
                    Proposal {
                        width: inset(proposal.width, edges.horizontal()),
                        height: inset(proposal.height, edges.vertical()),
                    },
                    env,
                );
                let size = Size {
                    width: child_size.width + edges.horizontal(),
                    height: child_size.height + edges.vertical(),
                };
                (size, Fit::Wrapped(child_size, Box::new(fit)))
            }

            LayoutNode::Frame { width, height, child } => {
                let (child_size, fit) = child.measure(
                    Proposal {
                        width: width.or(proposal.width),
                        height: height.or(proposal.height),
                    },
                    env,
                );
                let size = Size {
                    width: width.unwrap_or(child_size.width),
                    height: height.unwrap_or(child_size.height),
                };
                (size, Fit::Wrapped(child_size, Box::new(fit)))
            }

            LayoutNode::MaxFrame { max_width, max_height, child, .. } => {
                let cap = |proposed: Option<Px>, max: Px| match proposed {
                    Some(length) => Some(length.min(max)),
                    None if max.is_finite() => Some(max),
                    None => None,
                };
                let (child_size, fit) = child.measure(
                    Proposal {
                        width: cap(proposal.width, *max_width),
                        height: cap(proposal.height, *max_height),
                    },
                    env,
                );
                // ∞ = fill what was proposed; finite = a ceiling over the child
                let resolve = |proposed: Option<Px>, max: Px, child_len: Px| {
                    if max.is_infinite() {
                        proposed.unwrap_or(child_len)
                    } else {
                        child_len.min(max)
                    }
                };
                let size = Size {
                    width: resolve(proposal.width, *max_width, child_size.width),
                    height: resolve(proposal.height, *max_height, child_size.height),
                };
                (size, Fit::Wrapped(child_size, Box::new(fit)))
            }

            LayoutNode::Scroll { axes, child, .. } => {
                // the scrolling axes stay OPEN — the content takes its
                // natural extent there and the region travels through it
                let (content, fit) = child.measure(
                    Proposal {
                        width: if axes.horizontal() { None } else { proposal.width },
                        height: if axes.vertical() { None } else { proposal.height },
                    },
                    env,
                );
                let size = Size {
                    width: proposal.width.unwrap_or(content.width),
                    height: proposal.height.unwrap_or(content.height),
                };
                (size, Fit::ScrollContent(content, Box::new(fit)))
            }

            LayoutNode::Split { axis, unit, at, min_a, min_b, children, .. } => {
                measure_split(*axis, *unit, *at, *min_a, *min_b, children, proposal, env)
            }

            LayoutNode::Interactive { child, .. }
            | LayoutNode::HoverGroup { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Styled { props, child } => {
                // the inherited font swaps HERE, at measure time — the
                // sanctioned VisualProps exception (font changes measure)
                let env = LayoutEnv {
                    font: props.font.apply_over(env.font),
                    line_height: props.line_height.or(env.line_height),
                    ..env
                };
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            // the animation scope never touches geometry — by type
            LayoutNode::Animated { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            // the island claims a renderer, never a pixel of geometry
            LayoutNode::Island { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            // the loop claims a clock, never a pixel of geometry — the
            // measure of a looping box is phase-blind by contract
            LayoutNode::Live { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Boundary { children, .. } => {
                // a boundary with one child is transparent; with several,
                // the children stack vertically (the implicit TupleView)
                if children.len() == 1 {
                    let (size, fit) = children[0].measure(proposal, env);
                    (size, Fit::Children(vec![(size, fit)]))
                } else {
                    let (size, fit) =
                        measure_stack(Axis::Vertical, 0.0, children, proposal, env);
                    (size, fit)
                }
            }

            // skipped boundary: measures the RETAINED tree in its place —
            // no copy stitched anywhere (the frame's layout reads the
            // retention)
            LayoutNode::BoundaryRef { path } => {
                crate::reconciler::with_retained_layout(path, |layout| match layout {
                    Some(node) => node.measure(proposal, env),
                    None => {
                        debug_assert!(false, "layout reference without retention: {path}");
                        (Size::default(), Fit::Leaf)
                    }
                })
            }
        }
    }

    pub(crate) fn place(&self, frame: Rect, fit: Fit, env: LayoutEnv, out: &mut Placement) {
        match (self, fit) {
            // visual leaves: the draw list is born here
            (LayoutNode::Text { content, highlights, truncation }, Fit::Leaf) => {
                let color = out.foreground.last().copied().unwrap_or_else(|| crate::theme::current().fg);
                if let Some(dom) = out.dom.as_mut() {
                    // the WHOLE content, unwrapped: the browser re-breaks
                    // lines inside the same box with the same measures
                    dom.leaf(
                        crate::dom::DomKind::Text(crate::dom::DomText {
                            content: content.clone(),
                            color,
                            // the capture decides: it knows whether a
                            // hover ink is open above this leaf
                            inherits_ink: false,
                            font: env.font,
                            line_height: env.line_height,
                            highlights: highlights
                                .as_ref()
                                .map(|h| (Rc::clone(&h.ranges), h.color)),
                            truncation: *truncation,
                        }),
                        frame,
                    );
                }
                place_text(
                    content,
                    highlights.as_ref(),
                    *truncation,
                    frame,
                    color,
                    env,
                    out,
                );
            }

            (LayoutNode::Fill, Fit::Leaf) => {
                if let Some(dom) = out.dom.as_mut() {
                    dom.open(crate::dom::DomKind::Box, frame, frame.origin);
                    dom.set_background(Color::FILL);
                    dom.close();
                }
                out.draw(DrawCommand::FillRect {
                    rect: frame,
                    color: Color::FILL,
                    corner_radius: Corners::ZERO,
                });
            }

            (
                LayoutNode::Field { path, content, placeholder, multiline, auto_focus },
                Fit::Leaf,
            ) => {
                let multiline = *multiline;
                if out.dom.is_some() {
                    // the browser's input owns focus, caret and
                    // composition — the record carries what to SHOW,
                    // and the THEME chrome rides the node's style (the
                    // same tokens the pixel paint below reads)
                    let theme = crate::theme::current();
                    let color = out
                        .foreground
                        .last()
                        .copied()
                        .unwrap_or(theme.fg);
                    if let Some(dom) = out.dom.as_mut() {
                        dom.leaf_styled(
                            crate::dom::DomKind::Field(crate::dom::DomField {
                                path: path.clone(),
                                content: content.clone(),
                                placeholder: placeholder.clone(),
                                font: env.font,
                                color,
                                multiline,
                            }),
                            frame,
                            crate::dom::DomStyle {
                                background: Some(theme.field),
                                border: Some((theme.field_border, 1.0)),
                                corner_radius: Some(Corners::all(FIELD_RADIUS)),
                                focus_border: Some(theme.focus),
                                placeholder_color: Some(theme.placeholder),
                                ..crate::dom::DomStyle::default()
                            },
                        );
                    }
                }
                // focus/caret/selection read from the env's STAMP, clamped
                // to the current content (the app may have swapped the
                // string outside the editor) — the tree never carried any
                // of this
                let focused = env.stamp.focus == Some(path.as_str());
                let state =
                    env.stamp.carets.get(path.as_str()).copied().unwrap_or_default();
                let clamp = |index: usize| crate::text_input::clamp_index(content, index);
                let caret = (focused && env.stamp.caret_visible).then(|| clamp(state.caret));
                let selection = focused
                    .then(|| state.selection())
                    .flatten()
                    .map(|(start, end)| (clamp(start), clamp(end)))
                    .filter(|(start, end)| start < end);
                let marked = focused
                    .then_some(state.marked)
                    .flatten()
                    .map(|(start, end)| (clamp(start), clamp(end)))
                    .filter(|(start, end)| start < end);
                // field chrome: tokens read at PLACEMENT — a retheme
                // repaints without re-running a single body
                let theme = crate::theme::current();
                out.draw(DrawCommand::FillRect {
                    rect: frame,
                    color: theme.field,
                    corner_radius: Corners::all(FIELD_RADIUS),
                });
                // the placeholder walks the SAME path as the real text:
                // same origin, same font, same breaks — only the ink drops
                let sample: &Arc<str> = if content.is_empty() { placeholder } else { content };
                // a many-line field asks a PROBE for the line height
                // instead of measuring a whole note as one line; a
                // one-line field measures what it draws, as it always did
                let metrics = env.cache.get_or_measure(
                    if multiline { "0" } else { sample },
                    &env.font,
                    env.text,
                );
                let line_h = metrics.height();
                let inner = (frame.size.width - 2.0 * FIELD_PAD_H).max(0.0);
                let inner_h = (frame.size.height - 2.0 * FIELD_PAD_V).max(0.0);
                // one shape, one loop: the one-line field is the field
                // whose list of lines has one entry
                let single = [(0usize, sample.len())];
                let broken;
                let lines: &[(usize, usize)] = if multiline {
                    broken = env.cache.get_or_break(sample, &env.font, inner, env.text);
                    &broken
                } else {
                    &single
                };
                // the run scrolls UNDER the box to keep the caret in
                // sight. The offset is engine state written by the
                // runtime from the caret — the same map a scroll box
                // reads, keyed by the field's own path — and it clamps
                // against THIS frame: a field that widens never leaves
                // a gap after the last glyph. A wrapped field has
                // nothing to give sideways, so it only ever rolls down
                let overflow_x =
                    if multiline { 0.0 } else { (metrics.width - inner).max(0.0) };
                let overflow_y = (lines.len() as Px * line_h - inner_h).max(0.0);
                let scroll = env.scroll_offsets.get(path).copied().unwrap_or_default();
                let offset = Point {
                    x: scroll.x.clamp(0.0, overflow_x),
                    y: scroll.y.clamp(0.0, overflow_y),
                };
                let text_origin = Point {
                    x: frame.origin.x + FIELD_PAD_H - offset.x,
                    y: frame.origin.y + FIELD_PAD_V - offset.y,
                };
                // everything the field writes is cut by its own box:
                // a string longer than the field stops at the border
                // instead of painting over the neighbour
                out.push_clip(frame, FIELD_RADIUS);
                let width_of = |from: usize, to: usize| {
                    env.cache.get_or_measure(&sample[from..to], &env.font, env.text).width
                };
                let color = if content.is_empty() {
                    theme.placeholder
                } else {
                    out.foreground.last().copied().unwrap_or(theme.fg)
                };
                for (index, &(start, end)) in lines.iter().enumerate() {
                    let y = text_origin.y + index as Px * line_h;
                    // a note of a thousand lines pays for the ones that
                    // show — the same discipline the row window keeps
                    if y + line_h <= frame.origin.y
                        || y >= frame.origin.y + frame.size.height
                    {
                        continue;
                    }
                    // selection behind the text
                    if let Some((from, to)) = selection {
                        let head = from.clamp(start, end);
                        let tail = to.clamp(start, end);
                        // a break INSIDE the selection reads as a sliver
                        // at the end of its line — without it a selected
                        // empty line would show nothing at all
                        let over = to > end && from <= end;
                        if head < tail || over {
                            let x0 = text_origin.x + width_of(start, head);
                            let x1 = text_origin.x
                                + width_of(start, tail)
                                + if over { line_h / 2.0 } else { 0.0 };
                            out.draw(DrawCommand::FillRect {
                                rect: Rect {
                                    origin: Point { x: x0, y },
                                    size: Size { width: x1 - x0, height: line_h },
                                },
                                color: theme.selection,
                                corner_radius: Corners::ZERO,
                            });
                        }
                    }
                    if start < end {
                        out.draw(DrawCommand::TextLine {
                            origin: Point { x: text_origin.x, y },
                            content: sample.clone(),
                            range: (start, end),
                            color,
                            font: env.font,
                        });
                    }
                    // the live composition gets the IME underline (the
                    // caret's ink — the composition's visual pair)
                    if let Some((from, to)) = marked {
                        let head = from.clamp(start, end);
                        let tail = to.clamp(start, end);
                        if head < tail {
                            let x0 = text_origin.x + width_of(start, head);
                            let x1 = text_origin.x + width_of(start, tail);
                            out.draw(DrawCommand::FillRect {
                                rect: Rect {
                                    origin: Point { x: x0, y: y + line_h - 1.0 },
                                    size: Size { width: x1 - x0, height: 1.0 },
                                },
                                color: theme.caret,
                                corner_radius: Corners::ZERO,
                            });
                        }
                    }
                }
                // caret on top (the blink alternates via the stamp).
                // It belongs to the EARLIER line at a break, so End on a
                // wrapped line shows it where the typing is
                if let Some(caret) = caret {
                    let index = line_of(lines, caret);
                    let (start, _) = lines[index];
                    out.draw(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point {
                                x: text_origin.x + width_of(start, caret.max(start)),
                                y: text_origin.y + index as Px * line_h,
                            },
                            size: Size { width: FIELD_CARET_W, height: line_h },
                        },
                        color: theme.caret,
                        corner_radius: Corners::all(FIELD_CARET_W / 2.0),
                    });
                }
                out.pop_clip();
                out.draw(DrawCommand::StrokeRect {
                    rect: frame,
                    color: if focused { theme.focus } else { theme.field_border },
                    width: 1.0,
                    corner_radius: Corners::all(FIELD_RADIUS),
                });
                // the field is a pointer target (clicking focuses) —
                // clipped like any hit
                let visible = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(visible) = visible {
                    out.hits.push((path.clone(), visible));
                }
                out.fields.push(FieldPlacement {
                    path: path.clone(),
                    frame,
                    run: Rect {
                        origin: Point {
                            x: frame.origin.x + FIELD_PAD_H,
                            y: frame.origin.y + FIELD_PAD_V,
                        },
                        size: Size { width: inner, height: inner_h },
                    },
                    text_origin,
                    font: env.font,
                    line_height: line_h,
                    multiline,
                    auto_focus: *auto_focus,
                });
            }

            (LayoutNode::Leaf { .. }, Fit::Leaf) => {
                if let Some(dom) = out.dom.as_mut() {
                    dom.open(crate::dom::DomKind::Box, frame, frame.origin);
                    dom.set_border(Color::OUTLINE, 1.0);
                    dom.close();
                }
                out.draw(DrawCommand::StrokeRect {
                    rect: frame,
                    color: Color::OUTLINE,
                    width: 1.0,
                    corner_radius: Corners::ZERO,
                });
            }

            (LayoutNode::Custom { path, element }, Fit::Leaf) => {
                // what the clip lets through, in the box's own
                // coordinates — the paint and the events read the SAME
                // window, and a box inside a scroll learns its viewport
                // from it. No clip above means all of the box shows;
                // a clip that misses the box means none of it does.
                let window = out.current_clip().map_or(
                    Rect { origin: Point::ZERO, size: frame.size },
                    |clip| {
                        clip.intersection(frame).map_or(
                            Rect { origin: Point::ZERO, size: Size::default() },
                            |clip| Rect {
                                origin: Point {
                                    x: clip.origin.x - frame.origin.x,
                                    y: clip.origin.y - frame.origin.y,
                                },
                                size: clip.size,
                            },
                        )
                    },
                );
                // the phase the paint reads: the loop the nearest
                // `.looping(...)` opened, resolved against the box's
                // own clock. A box outside a loop paints phase zero; a
                // box without identity holds the still frame.
                let phase = match env.live {
                    Some(spec) if !path.is_empty() => match env.animator {
                        Some(animator) => {
                            animator.borrow_mut().resolve_phase(path, spec)
                        }
                        None => spec.quantise(spec.still),
                    },
                    Some(spec) => spec.quantise(spec.still),
                    None => 0.0,
                };
                // the box answers for the whole frame: a hit here never
                // falls through to what is painted underneath, and the
                // event finds the element by this path
                let placed = if path.is_empty() {
                    None
                } else {
                    let visible = out.current_clip().map_or(Some(frame), |clip| {
                        frame.intersection(clip)
                    });
                    visible.map(|visible| {
                        out.hits.push((path.clone(), visible));
                        out.customs.push(CustomPlacement {
                            path: path.clone(),
                            frame,
                            visible: window,
                            region: out.region_stack.last().cloned(),
                            font: env.font,
                            ink: out.foreground.last().copied().unwrap_or(Color::BLACK),
                            element: element.clone(),
                            live: env.live,
                            slice: (0, 0),
                        });
                        out.customs.len() - 1
                    })
                };
                // what the app paints is PIXELS: on the element lowering
                // the box becomes a canvas island, and the island slices
                // exactly the commands between here and the close
                let start = out.display.len();
                out.saw_island = true;
                if let Some(dom) = out.dom.as_mut() {
                    // the island covers what is VISIBLE, never the whole
                    // box: a box that declared four thousand points of
                    // content would otherwise mint a canvas that tall,
                    // and the paint inside it is one screen anyway
                    dom.open_canvas(
                        Rect {
                            origin: Point {
                                x: frame.origin.x + window.origin.x,
                                y: frame.origin.y + window.origin.y,
                            },
                            size: window.size,
                        },
                        start,
                    );
                }
                // the box cannot paint outside itself — the clip is the
                // framework's, never the app's promise
                out.push_clip(frame, 0.0);
                let focused = env.stamp.focus == Some(path.as_str()) && !path.is_empty();
                let ctx = crate::custom::PaintCtx {
                    frame,
                    visible: window,
                    metrics: crate::custom::Metrics::new(env.text, env.cache, env.font),
                    focused,
                    caret_visible: focused && env.stamp.caret_visible,
                    phase,
                    scale: env.scale,
                };
                let ink = out.foreground.last().copied().unwrap_or(Color::BLACK);
                let mut painter =
                    crate::custom::Painter::new(&mut out.display, frame.origin, env.font, ink);
                element.element().paint(&ctx, &mut painter);
                out.pop_clip();
                let end = out.display.len();
                if let Some(dom) = out.dom.as_mut() {
                    dom.close_canvas(end);
                }
                // the retained placement remembers which commands are
                // the box's — a live box repaints exactly this slice
                if let Some(index) = placed {
                    out.customs[index].slice = (start, end);
                }
            }

            (LayoutNode::Anchored { path, side, overlay, child }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
                // the REAL anchor: un-shift the in-flight animation
                // (the popover never chases a sliding row — the same
                // contract as the retained boundary frames)
                let shift = env.anim.map(|scope| scope.shift).unwrap_or((0.0, 0.0));
                let anchor = Rect {
                    origin: Point {
                        x: frame.origin.x - shift.0,
                        y: frame.origin.y - shift.1,
                    },
                    size: frame.size,
                };
                let anchor_visible = out
                    .current_clip()
                    .is_none_or(|clip| anchor.intersection(clip).is_some());
                out.overlay_queue.push(QueuedOverlay {
                    path: path.clone(),
                    side: *side,
                    node: Rc::clone(overlay),
                    anchor,
                    anchor_visible,
                });
            }

            (LayoutNode::Hinted { child, .. }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
            }

            (LayoutNode::BoundaryHint { .. }, Fit::Leaf) => {}

            (LayoutNode::ExactLayout { child }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
            }

            (LayoutNode::DragRegion { child }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
                // clipped like a hit: what is not visible cannot drag
                let region = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(region) = region {
                    out.drag_regions.push(region);
                }
            }

            (LayoutNode::ControlRegion { control, child }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
                // clipped like a hit: what is not visible is no button
                let region = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(region) = region {
                    out.control_regions.push((*control, region));
                }
            }

            (LayoutNode::Tooltip { text, side, child }, Fit::Wrapped(_, fit)) => {
                if let Some(dom) = out.dom.as_mut() {
                    // in element mode the browser owns the wait and the
                    // bubble — the text lands as a data attribute on
                    // the child's own element
                    dom.arm_tooltip(text.clone());
                }
                // recorded BEFORE the child, so the vector runs outer
                // to inner and the reverse walk finds the INNERMOST
                // explanation first — a tooltip on a chip inside a card
                // is the chip's. Clipped like a hit: what is not visible
                // explains nothing.
                if let Some(rect) = clip_of(out, frame) {
                    out.tooltips.push(TooltipRegion { text: text.clone(), side: *side, rect });
                }
                child.place(frame, *fit, env, out);
            }

            (LayoutNode::ContextSource { items, child }, Fit::Wrapped(_, fit)) => {
                // outer before inner: the innermost menu wins the press
                if let Some(rect) = clip_of(out, frame) {
                    out.menus.push(MenuRegion { items: items.clone(), rect });
                }
                child.place(frame, *fit, env, out);
            }

            (LayoutNode::DragSource { payload, child }, Fit::Wrapped(_, fit)) => {
                // outer before inner: what the hand lifts is the
                // innermost thing under it, never the card around it
                if let Some(rect) = clip_of(out, frame) {
                    out.drag_sources.push(DragSourceRegion { payload: payload.clone(), rect });
                }
                child.place(frame, *fit, env, out);
            }

            (LayoutNode::DropTarget { accepts, action, over, child }, Fit::Wrapped(_, fit)) => {
                let region = clip_of(out, frame);
                // the ROUTE is recorded before the descent, so the
                // vector runs outer to inner and the runtime's reverse
                // walk answers with the innermost target — a chip
                // inside a pane takes its own drop. Siblings are
                // untouched: the one placed later still enters later.
                if let Some(rect) = region {
                    out.drops.push(DropRegion {
                        accepts: *accepts,
                        action: action.clone(),
                        over: over.clone(),
                        rect,
                        frame,
                    });
                }
                child.place(frame, *fit, env, out);
                // the RING is paint, and paint runs the other way: it
                // has to cover the child in the draw list and be the
                // later sibling in the element tree. Routing order and
                // paint order are deliberately opposite here.
                if let Some(rect) = region {
                    // a compatible drag over THIS box: the framework
                    // rings it — the drop focus every platform draws.
                    // A box that paints its OWN preview gets no ring:
                    // one affordance per target, and the app's wins.
                    let ringed = over.is_none()
                        && env
                            .stamp
                            .interaction
                            .drag
                            .as_ref()
                            .is_some_and(|live| live.over == Some(rect));
                    if ringed {
                        let accent = crate::theme::current().accent;
                        out.draw(DrawCommand::StrokeRect {
                            rect: frame,
                            color: accent,
                            width: 2.0,
                            corner_radius: Corners::all(6.0),
                        });
                        // element mode never reads the draw list, so the
                        // ring must be an ELEMENT there — a box with a
                        // border, born with the drag and dying with it
                        if let Some(dom) = out.dom.as_mut() {
                            dom.leaf_styled(
                                crate::dom::DomKind::Box,
                                frame,
                                crate::dom::DomStyle {
                                    border: Some((accent, 2.0)),
                                    corner_radius: Some(Corners::all(6.0)),
                                    ..crate::dom::DomStyle::default()
                                },
                            );
                        }
                    }
                }
            }

            (LayoutNode::Image { source, fit, .. }, Fit::Leaf) => match source {
                // the print-parity stub draws the SAME outline box the
                // old Leaf did — goldens hold by construction
                None => {
                    if let Some(dom) = out.dom.as_mut() {
                        dom.open(crate::dom::DomKind::Box, frame, frame.origin);
                        dom.set_border(Color::OUTLINE, 1.0);
                        dom.close();
                    }
                    out.draw(DrawCommand::StrokeRect {
                        rect: frame,
                        color: Color::OUTLINE,
                        width: 1.0,
                        corner_radius: Corners::ZERO,
                    });
                }
                Some(source) => {
                    let cover = *fit == Some(ContentMode::Fill);
                    if let Some(dom) = out.dom.as_mut() {
                        dom.leaf(
                            crate::dom::DomKind::Image(crate::dom::DomImage {
                                key: source.key(),
                                cover,
                            }),
                            frame,
                        );
                    }
                    if cover {
                        // Fill spills over the frame on one axis — the
                        // clip is built in, never a separate modifier
                        if let Some(rect) = cover_rect(frame, intrinsic_of(&*env.images, source)) {
                            out.push_clip(frame, 0.0);
                            out.draw(DrawCommand::Image {
                                rect,
                                source: source.clone(),
                            });
                            out.pop_clip();
                        }
                    } else if frame.size.width > 0.0 && frame.size.height > 0.0 {
                        out.draw(DrawCommand::Image {
                            rect: frame,
                            source: source.clone(),
                        });
                    }
                }
            },

            (LayoutNode::Icon { symbol, .. }, Fit::Leaf) => {
                // the ink is the INHERITED one, the same line Text
                // reads — .foreground_color and the hover/press inks
                // reach a glyph with zero new API
                let color = out
                    .foreground
                    .last()
                    .copied()
                    .unwrap_or_else(|| crate::theme::current().fg);
                if let Some(dom) = out.dom.as_mut() {
                    // the capture re-stamps the ink from its own stack
                    dom.leaf(
                        crate::dom::DomKind::Icon(crate::dom::DomIcon {
                            key: symbol.key,
                            symbol: *symbol,
                            color,
                            inherits_ink: false,
                        }),
                        frame,
                    );
                }
                let side = frame.size.width.min(frame.size.height);
                if side > 0.0 {
                    let rect = Rect {
                        origin: Point {
                            x: frame.origin.x + (frame.size.width - side) / 2.0,
                            y: frame.origin.y + (frame.size.height - side) / 2.0,
                        },
                        size: Size { width: side, height: side },
                    };
                    out.draw(DrawCommand::Image {
                        rect,
                        source: ImageSource::symbol(*symbol, color),
                    });
                }
            }

            (LayoutNode::Stack { axis, spacing, align, children }, Fit::Children(fits)) => {
                place_stack(*axis, *spacing, *align, children, frame, fits, env, out);
            }

            (
                LayoutNode::Split { path, axis, unit, min_a, min_b, children, .. },
                Fit::Children(fits),
            ) => {
                // the seam's metrics, read before the fits move into
                // the placing loop
                let lane_main = |index: usize| {
                    fits.get(index).map_or(0.0, |(size, _): &(Size, Fit)| match axis {
                        Axis::Horizontal => size.width,
                        Axis::Vertical => size.height,
                    })
                };
                let seam_center = lane_main(0) + lane_main(1) / 2.0;
                // what the lanes share: a fractional drag divides by
                // THIS, never by the frame (the divider is not room)
                let room = (lane_main(0) + lane_main(2)).max(0.0);
                // lanes in measure order: A, divider, B — each filling
                // the frame's cross extent
                let mut cursor = 0.0;
                for (child, (size, fit)) in children.iter().zip(fits) {
                    let (origin, lane) = match axis {
                        Axis::Horizontal => (
                            Point { x: frame.origin.x + cursor, y: frame.origin.y },
                            Size { width: size.width, height: frame.size.height },
                        ),
                        Axis::Vertical => (
                            Point { x: frame.origin.x, y: frame.origin.y + cursor },
                            Size { width: frame.size.width, height: size.height },
                        ),
                    };
                    child.place(Rect { origin, size: lane }, fit, env, out);
                    cursor += match axis {
                        Axis::Horizontal => size.width,
                        Axis::Vertical => size.height,
                    };
                }
                // the grip band, centered on the divider — pushed AFTER
                // the lanes so the reverse hit walk finds it first, and
                // clipped like every hit (an off-screen seam cannot drag)
                let seam = {
                    let center = seam_center;
                    match axis {
                        Axis::Horizontal => Rect {
                            origin: Point {
                                x: frame.origin.x + center - SPLIT_GRIP / 2.0,
                                y: frame.origin.y,
                            },
                            size: Size { width: SPLIT_GRIP, height: frame.size.height },
                        },
                        Axis::Vertical => Rect {
                            origin: Point {
                                x: frame.origin.x,
                                y: frame.origin.y + center - SPLIT_GRIP / 2.0,
                            },
                            size: Size { width: frame.size.width, height: SPLIT_GRIP },
                        },
                    }
                };
                let visible = match out.current_clip() {
                    Some(clip) => seam.intersection(clip),
                    None => Some(seam),
                };
                if let Some(visible) = visible {
                    out.hits.push((format!("{path}/#split"), visible));
                }
                out.splits.push(SplitPlacement {
                    path: path.clone(),
                    frame,
                    axis: *axis,
                    unit: *unit,
                    room,
                    min_a: *min_a,
                    min_b: *min_b,
                });
            }

            (LayoutNode::Overlay { at, behind, layer, child }, Fit::Children(fits)) => {
                let mut fits = fits;
                let (base_size, base_fit) = fits.remove(0);
                let (layer_size, layer_fit) = fits.remove(0);
                // a layer that FILLED the measured box follows the real
                // frame instead: the parent may have handed the base
                // more room than it asked for, and a rule that crossed
                // the box must still cross it
                let stretch = |axis_layer: Px, axis_base: Px, axis_frame: Px| {
                    if axis_layer >= axis_base { axis_frame } else { axis_layer }
                };
                let size = Size {
                    width: stretch(layer_size.width, base_size.width, frame.size.width),
                    height: stretch(layer_size.height, base_size.height, frame.size.height),
                };
                let origin = Point {
                    x: frame.origin.x + (frame.size.width - size.width) * at.x,
                    y: frame.origin.y + (frame.size.height - size.height) * at.y,
                };
                let layer_frame = Rect { origin, size };
                // paint order IS the declaration: behind goes first.
                // In element mode the tree order is the paint order, so
                // the same two calls do the whole job there too — and
                // the layer scope tells the glue that what it paints
                // lets the pointer through
                let paint_layer = |out: &mut Placement| {
                    if let Some(dom) = out.dom.as_mut() {
                        dom.enter_layer();
                    }
                    layer.place(layer_frame, layer_fit, env, out);
                    if let Some(dom) = out.dom.as_mut() {
                        dom.leave_layer();
                    }
                };
                if *behind {
                    paint_layer(out);
                    child.place(frame, base_fit, env, out);
                } else {
                    child.place(frame, base_fit, env, out);
                    paint_layer(out);
                }
            }

            (LayoutNode::Layered { align, modal, children }, Fit::Children(fits)) => {
                for (index, (child, (size, fit))) in children.iter().zip(fits).enumerate() {
                    // a modal pile draws its line here: everything
                    // recorded from now on is ABOVE, and the walk back
                    // stops at this mark instead of reaching under it
                    if *modal && index > 0 {
                        out.modal_floor = Some(ModalFloor {
                            hits: out.hits.len(),
                            scrolls: out.scrolls.len(),
                            tooltips: out.tooltips.len(),
                            menus: out.menus.len(),
                            drag_sources: out.drag_sources.len(),
                            drops: out.drops.len(),
                        });
                    }
                    // the alignment edge is horizontal — a 2pt accent bar
                    // hugs the leading side and still centers vertically
                    let origin = Point {
                        x: frame.origin.x
                            + match align {
                                CrossAlign::Start | CrossAlign::Baseline => 0.0,
                                CrossAlign::Center => (frame.size.width - size.width) / 2.0,
                                CrossAlign::End => frame.size.width - size.width,
                            },
                        y: frame.origin.y + (frame.size.height - size.height) / 2.0,
                    };
                    child.place(Rect { origin, size }, fit, env, out);
                }
            }

            (LayoutNode::Padding { edges, child }, Fit::Wrapped(child_size, fit)) => {
                let origin = Point {
                    x: frame.origin.x + edges.leading,
                    y: frame.origin.y + edges.top,
                };
                child.place(Rect { origin, size: child_size }, *fit, env, out);
            }

            (LayoutNode::Frame { child, .. }, Fit::Wrapped(child_size, fit)) => {
                let origin = Point {
                    x: frame.origin.x + (frame.size.width - child_size.width) / 2.0,
                    y: frame.origin.y + (frame.size.height - child_size.height) / 2.0,
                };
                child.place(Rect { origin, size: child_size }, *fit, env, out);
            }

            (LayoutNode::MaxFrame { align, child, .. }, Fit::Wrapped(child_size, fit)) => {
                let x = frame.origin.x
                    + match align {
                        CrossAlign::Start | CrossAlign::Baseline => 0.0,
                        CrossAlign::Center => (frame.size.width - child_size.width) / 2.0,
                        CrossAlign::End => frame.size.width - child_size.width,
                    };
                let y = frame.origin.y + (frame.size.height - child_size.height) / 2.0;
                child.place(Rect { origin: Point { x, y }, size: child_size }, *fit, env, out);
            }

            (
                LayoutNode::Scroll { path, target, axes, child },
                Fit::ScrollContent(content, fit),
            ) => {
                // per-axis travel over SNAPPED values: "scrollable by
                // 0.000001px" does not exist here by construction
                let max_x = (content.width.round() - frame.size.width.round()).max(0.0);
                let max_y = (content.height.round() - frame.size.height.round()).max(0.0);
                let raw = path
                    .as_deref()
                    .and_then(|path| env.scroll_offsets.get(path))
                    .copied()
                    .unwrap_or_default();
                // content that shrank re-clamps here — the retained
                // offset never leaves the region in no man's land
                let offset =
                    Point { x: raw.x.clamp(0.0, max_x), y: raw.y.clamp(0.0, max_y) };
                out.push_clip(frame, 0.0);
                let content_origin = Point {
                    x: frame.origin.x - offset.x,
                    y: frame.origin.y - offset.y,
                };
                // animated origins below anchor to the content box —
                // scrolling moves them 1:1, never through a spring
                out.anchors.push(content_origin);
                // a virtual child reports misses against THIS region,
                // and its measured geometry is snapshot material
                let (row_extent, row_offsets) = match fit.as_ref() {
                    Fit::Virtual { row_extent, offsets, .. } => {
                        (Some(*row_extent), offsets.clone())
                    }
                    _ => (None, None),
                };
                if let Some(path) = path {
                    out.region_stack.push(path.clone());
                    // BEFORE the child, which puts the regions in the
                    // paint order every other list here already keeps:
                    // a child paints over its parent and a later layer
                    // over the one it covers, so walking back finds the
                    // innermost region of the topmost layer in one pass
                    out.scrolls.push(ScrollRegion {
                        path: path.clone(),
                        frame,
                        content,
                        target: target.clone(),
                        // a region inside an animation scope reveals its
                        // target through the spring instead of snapping
                        anim: env.anim.map(|scope| scope.spec),
                        row_extent,
                        row_offsets,
                    });
                }
                if let Some(dom) = out.dom.as_mut() {
                    // viewport element + a content element sized to the
                    // FULL extent (the browser scrolls through it; a
                    // virtual list keeps the geometry honest the same
                    // way). the offset lives on the scroll node — the
                    // content and its children never move
                    dom.open(
                        crate::dom::DomKind::Scroll {
                            path: path.clone(),
                            offset: (offset.x, offset.y),
                            // the ABSOLUTE capture keeps reveal in the
                            // engine (SetScroll from measured frames) —
                            // the record stays silent here
                            target: None,
                        },
                        frame,
                        frame.origin,
                    );
                    dom.open(
                        crate::dom::DomKind::Content,
                        Rect { origin: frame.origin, size: content },
                        content_origin,
                    );
                }
                child.place(
                    Rect { origin: content_origin, size: content },
                    *fit,
                    env,
                    out,
                );
                if let Some(dom) = out.dom.as_mut() {
                    dom.close();
                    dom.close();
                }
                if path.is_some() {
                    out.region_stack.pop();
                }
                out.anchors.pop();
                if max_y > 0.0 && axes.vertical() {
                    draw_scrollbar(
                        path.as_deref(),
                        frame,
                        content.height,
                        offset.y,
                        max_y,
                        out,
                    );
                }
                if max_x > 0.0 && axes.horizontal() {
                    draw_scrollbar_h(
                        path.as_deref(),
                        frame,
                        content.width,
                        offset.x,
                        max_x,
                        out,
                    );
                }
                out.pop_clip();
            }

            (LayoutNode::Styled { props, child }, Fit::Wrapped(_, fit)) => {
                // the nearest styled EATS the color scope: its colors
                // move, deeper styled nodes paint plain (no shared-key
                // thrash between siblings of one scope)
                let colors = env.anim.filter(|scope| scope.colors);
                let env = LayoutEnv {
                    font: props.font.apply_over(env.font),
                    line_height: props.line_height.or(env.line_height),
                    anim: env.anim.map(|scope| AnimScope { colors: false, ..scope }),
                    ..env
                };
                // the animator paints the value in flight, never the
                // target — it seeds, retargets and snaps behind this
                let animated = |slot, color| match (colors, env.animator) {
                    (Some(scope), Some(animator)) => animator
                        .borrow_mut()
                        .resolve_color(scope.key, scope.spec, slot, color),
                    _ => color,
                };
                if let Some(dom) = out.dom.as_mut() {
                    // base + hover + pressed side by side, never the
                    // pointer-resolved paint: the scene stays pointer-
                    // invariant and a hover frame diffs to zero patches
                    // (the capture keeps the base ink of its own)
                    dom.open_styled(props, frame);
                }
                // what the fade covers starts HERE: the box's own halo,
                // background and border fade with the subtree, the way
                // a layer would take them all at once
                let fade_from = out.display.len();
                // the halo goes first — everything else paints over it
                if let Some((radius, color)) = props.shadow {
                    out.draw(DrawCommand::Shadow {
                        rect: frame,
                        radius,
                        color: animated(crate::anim::Channel::Shadow, color),
                        corner_radius: props.corner_radius.unwrap_or_default(),
                    });
                }
                // the pane comes after the halo and before everything
                // else: it reads the scene BEHIND the box, and the
                // box's own paint must stay sharp on top of it. The
                // halo is under it and therefore inside it — hang the
                // halo on a wrapper to keep it out
                if let Some(glass) = props.glass {
                    out.draw(DrawCommand::Backdrop {
                        rect: frame,
                        glass: glass.resolve(frame),
                        corner_radius: props.corner_radius.unwrap_or_default(),
                    });
                }
                // a box that follows a GROUP paints by the ancestor's
                // pointer, not by the target it belongs to — the mark
                // inside a chip lights when the CHIP is hovered
                let stack = if props.from_group { &out.groups } else { &out.pointer };
                let (hovered, pressed) = stack.last().copied().unwrap_or((false, false));
                // pressed > hovered > normal; a state without its own
                // background falls back to the base one — a button with
                // no hover defined does not flicker
                let background = if pressed {
                    props.background_pressed.or(props.background)
                } else if hovered {
                    props.background_hovered.or(props.background)
                } else {
                    props.background
                };
                if let Some(color) = background {
                    out.draw(DrawCommand::FillRect {
                        rect: frame,
                        color: animated(crate::anim::Channel::Background, color),
                        corner_radius: props.corner_radius.unwrap_or_default(),
                    });
                }
                // the ramp paints OVER the flat color and under the
                // child: the two compose, and the geometry resolves to
                // px here — the shaders only evaluate
                if let Some(gradient) = props.gradient {
                    out.draw(DrawCommand::Gradient {
                        rect: frame,
                        paint: gradient.resolve(frame),
                        corner_radius: props.corner_radius.unwrap_or_default(),
                    });
                }
                // the ink walks the same ladder as the background: a
                // state with no ink of its own falls back to the base
                let ink = if pressed {
                    props.foreground_pressed.or(props.foreground)
                } else if hovered {
                    props.foreground_hovered.or(props.foreground)
                } else {
                    props.foreground
                };
                if let Some(color) = ink {
                    let color = animated(crate::anim::Channel::Foreground, color);
                    out.foreground.push(color);
                }
                // the background IS the shape, so it paints before the
                // cut; the border paints after, over the cut child — a
                // ring blended once, never twice
                if props.clip {
                    out.push_clip(frame, props.corner_radius.unwrap_or_default());
                }
                child.place(frame, *fit, env, out);
                if props.clip {
                    out.pop_clip();
                }
                if ink.is_some() {
                    out.foreground.pop();
                }
                if let Some((color, width)) = props.border {
                    out.draw(DrawCommand::StrokeRect {
                        rect: frame,
                        color: animated(crate::anim::Channel::Border, color),
                        width,
                        corner_radius: props.corner_radius.unwrap_or_default(),
                    });
                }
                // the veil closes the box. In ELEMENT mode the browser
                // owns it (a real layer, on the element itself), so the
                // commands stay untouched — an island under a faded box
                // must not fade twice
                if out.dom.is_none() {
                    let opacity = if pressed {
                        props.opacity_pressed.or(props.opacity)
                    } else if hovered {
                        props.opacity_hovered.or(props.opacity)
                    } else {
                        props.opacity
                    };
                    if let Some(opacity) = opacity.filter(|value| *value < 1.0) {
                        out.display.fade_from(fade_from, opacity.max(0.0));
                    }
                }
                if let Some(dom) = out.dom.as_mut() {
                    dom.close();
                }
            }

            (LayoutNode::Animated { key, spec, child }, Fit::Wrapped(_, fit)) => {
                // a keyless scope (built outside a pass) stays inert
                let Some(key) = key else {
                    child.place(frame, *fit, env, out);
                    return;
                };
                // the node flies its OWN origin, anchored to the scroll
                // content box — scrolling moves content 1:1 and never
                // bends the spring. row keys carry the row identity
                // ("scope/[id]"), so sibling rows fly independently.
                let painted = match env.animator {
                    Some(animator) => {
                        let anchor = out.anchors.last().copied().unwrap_or_default();
                        let rel =
                            (frame.origin.x - anchor.x, frame.origin.y - anchor.y);
                        let (x, y) = animator
                            .borrow_mut()
                            .resolve_origin(key.as_ref(), *spec, rel);
                        Point { x: anchor.x + x, y: anchor.y + y }
                    }
                    None => frame.origin,
                };
                let shift = (painted.x - frame.origin.x, painted.y - frame.origin.y);
                let env = LayoutEnv {
                    anim: Some(AnimScope {
                        key: key.as_ref(),
                        spec: *spec,
                        colors: true,
                        shift,
                    }),
                    ..env
                };
                if let Some(dom) = out.dom.as_mut() {
                    // in Dom the browser animates: the spec lowers to a
                    // CSS transition on the next element opened below
                    dom.arm_transition(spec.response, spec.damping);
                }
                child.place(
                    Rect { origin: painted, size: frame.size },
                    *fit,
                    env,
                    out,
                );
                if let Some(dom) = out.dom.as_mut() {
                    dom.disarm();
                }
            }

            (LayoutNode::Live { spec, child }, Fit::Wrapped(_, fit)) => {
                // the clock opens for the subtree: the custom boxes
                // below resolve their phase against it at paint time
                let env = LayoutEnv { live: Some(*spec), ..env };
                child.place(frame, *fit, env, out);
            }

            (LayoutNode::Island { child, .. }, Fit::Wrapped(_, fit)) => {
                // Dom mode: a canvas element in the flow, filled with
                // the subtree's OWN draw commands (the display range
                // between open and close). Pixel targets place through:
                // everything is the pixel pipeline there already.
                if out.dom.is_some() {
                    out.saw_island = true;
                    let start = out.display.len();
                    if let Some(dom) = out.dom.as_mut() {
                        dom.open_canvas(frame, start);
                    }
                    child.place(frame, *fit, env, out);
                    let end = out.display.len();
                    if let Some(dom) = out.dom.as_mut() {
                        dom.close_canvas(end);
                    }
                } else {
                    child.place(frame, *fit, env, out);
                }
            }

            (LayoutNode::HoverGroup { path, child }, Fit::Wrapped(_, fit)) => {
                // a descendant's path CONTAINS the group's, so the walk
                // is a prefix test — and the state survives the pointer
                // moving onto a target inside the group
                let under = |target: &Option<String>| {
                    target.as_deref().is_some_and(|target| {
                        target == path
                            || target.strip_prefix(path.as_str()).is_some_and(|rest| {
                                rest.starts_with('/')
                            })
                    })
                };
                let hovered = under(&env.stamp.interaction.hovered);
                let pressed = hovered && under(&env.stamp.interaction.pressed);
                out.groups.push((hovered, pressed));
                if let Some(dom) = out.dom.as_mut() {
                    // element mode gets a box of its OWN: the glue hangs
                    // the descendants' state rules off its selector, and
                    // an ancestor is the one thing a selector can name
                    dom.open_group(group_key(path), frame);
                }
                child.place(frame, *fit, env, out);
                if let Some(dom) = out.dom.as_mut() {
                    dom.close_group();
                }
                out.groups.pop();
            }

            (LayoutNode::Interactive { path, child }, Fit::Wrapped(size, fit)) => {
                let _ = size;
                // outside the viewport the hit does NOT exist; a
                // half-visible row clicks only on its visible part (the
                // recorded rect is the intersection with the current clip)
                let visible = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(visible) = visible {
                    out.hits.push((path.clone(), visible));
                }
                // hover/pressed from the env's STAMP; VISUAL pressed only
                // with the pointer inside the target (AppKit semantics:
                // dragging out releases, coming back re-arms)
                let hovered =
                    env.stamp.interaction.hovered.as_deref() == Some(path.as_str());
                let pressed = hovered
                    && env.stamp.interaction.pressed.as_deref() == Some(path.as_str());
                out.pointer.push((hovered, pressed));
                if let Some(dom) = out.dom.as_mut() {
                    // the styled below takes the path: the glue posts
                    // clicks with it and scopes `:hover` to the element
                    dom.arm_interactive(path);
                }
                child.place(frame, *fit, env, out);
                if let Some(dom) = out.dom.as_mut() {
                    dom.disarm();
                }
                out.pointer.pop();
            }

            (
                LayoutNode::VirtualStack { count, children, .. },
                Fit::Virtual { row_extent, children: fits, offsets },
            ) => {
                let start_of = |index: usize| -> Px {
                    match &offsets {
                        Some(offsets) => offsets.get(index).copied().unwrap_or(0.0),
                        None => index as Px * row_extent,
                    }
                };
                let mut materialized: Vec<usize> = Vec::with_capacity(fits.len());
                for ((index, child), (fit_index, size, fit)) in children.iter().zip(fits) {
                    debug_assert_eq!(*index, fit_index, "window and fit walk in step");
                    materialized.push(*index);
                    let origin = Point {
                        x: frame.origin.x,
                        y: frame.origin.y + start_of(*index),
                    };
                    child.place(Rect { origin, size }, fit, env, out);
                }
                // the window miss, both directions: a VISIBLE row that
                // does not exist (the wheel outran the buffer), or a
                // window much FATTER than the band needs (the geometry-
                // blind first frame). either way the runtime invalidates
                // the enclosing boundary and the body re-materializes
                // with fresh geometry — the capped loop guards us, and
                // the 2× slack keeps the two window formulas from ever
                // arguing (no thrash)
                let geometry_known =
                    row_extent > 0.0 || offsets.as_ref().is_some_and(|o| o.len() == count + 1);
                if *count > 0
                    && geometry_known
                    && let Some(clip) = out.current_clip()
                {
                    let clip_top = clip.origin.y - frame.origin.y;
                    let clip_bottom = clip_top + clip.size.height;
                    let (top, bottom) = match &offsets {
                        Some(offsets) => {
                            // the row containing clip_top, and one past
                            // the last row starting before clip_bottom
                            let top = offsets
                                .partition_point(|start| *start <= clip_top)
                                .saturating_sub(1);
                            let bottom = offsets
                                .partition_point(|start| *start < clip_bottom)
                                .min(*count);
                            (top, bottom)
                        }
                        None => (
                            (clip_top / row_extent).floor().max(0.0) as usize,
                            ((clip_bottom / row_extent).ceil() as usize).min(*count),
                        ),
                    };
                    let uncovered =
                        (top..bottom).any(|index| !materialized.contains(&index));
                    let rows_in_view = bottom.saturating_sub(top).max(1);
                    let fat = materialized.len() > (3 * rows_in_view + 4) * 2;
                    if (uncovered || fat)
                        && let Some(path) = out.region_stack.last()
                    {
                        out.misses.push(path.clone());
                    }
                }
            }

            (LayoutNode::Boundary { path, children }, Fit::Children(fits)) => {
                // the recorded frame is the REAL target: a flight in
                // progress above un-shifts here, so scroll-to and tests
                // never chase a moving row. crossing the boundary also
                // closes the scope (`.animated` styles its own view,
                // never a child component's).
                let real = match env.anim {
                    Some(scope) => Rect {
                        origin: Point {
                            x: frame.origin.x - scope.shift.0,
                            y: frame.origin.y - scope.shift.1,
                        },
                        size: frame.size,
                    },
                    None => frame,
                };
                out.frames.record(path, real);
                if let Some(dom) = out.dom.as_mut() {
                    // the diff matches this node by identity path; the
                    // recorded frame is the REAL one and the children
                    // measure from the same origin, so a flight above
                    // never bends the captured interior
                    dom.open(
                        crate::dom::DomKind::Group { path: std::rc::Rc::from(path.as_str()) },
                        real,
                        real.origin,
                    );
                }
                let env = LayoutEnv { anim: None, ..env };
                if children.len() == 1 {
                    let mut fits = fits;
                    let (size, fit) = fits.remove(0);
                    let _ = size;
                    children[0].place(frame, fit, env, out);
                } else {
                    place_stack(
                        Axis::Vertical,
                        0.0,
                        CrossAlign::Start,
                        children,
                        frame,
                        fits,
                        env,
                        out,
                    );
                }
                if let Some(dom) = out.dom.as_mut() {
                    dom.close();
                }
            }

            // skipped boundary: places the RETAINED tree in its place
            // (measure's pair — both phases resolve the SAME retention,
            // the Fit mirrors by construction)
            (LayoutNode::BoundaryRef { path }, fit) => {
                crate::reconciler::with_retained_layout(path, |layout| {
                    if let Some(node) = layout {
                        node.place(frame, fit, env, out);
                    }
                });
            }

            (_, Fit::Leaf) => {}

            // the enum can represent the wrong pair; wiring a per-node
            // associated Fit makes that unrepresentable — here it is our
            // own bug
            _ => unreachable!("fit of one node used on another"),
        }
    }
}

/// The split's lane math: the divider child answers its own natural
/// thickness, lane A gets `at` clamped between the minimums, lane B the
/// rest. Unbounded on the main axis (a natural pass) the lanes answer
/// their naturals — the clamp only means something against a real offer.
/// A hover group's anchor for the element lowering: the path, hashed.
/// The browser needs a name to hang a selector on, never the path
/// itself — the same reason an image identity crosses as a number.
pub(crate) fn group_key(path: &str) -> u64 {
    let mut hasher = motor::hash::FxHasher::default();
    std::hash::Hasher::write(&mut hasher, path.as_bytes());
    std::hash::Hasher::finish(&hasher)
}

/// Lane A's extent in POINTS, whatever unit the seam speaks. `room` is
/// what the two lanes share — the frame's main extent minus the
/// divider — so a fraction of one half is exactly the other half, and
/// nested splits add up.
///
/// The clamp is the seam's ONLY guard: a binding may hold anything
/// (a restored window, a hand-typed number, a drag in flight), and
/// what reaches the lanes always fits between the floors.
pub(crate) fn resolve_seam(unit: SeamUnit, at: Px, min_a: Px, min_b: Px, room: Px) -> Px {
    let room = room.max(0.0);
    let (want, floor, ceiling) = match unit {
        SeamUnit::Points => (at, min_a, room - min_b),
        SeamUnit::Fraction => (at * room, min_a * room, room - min_b * room),
    };
    want.clamp(floor, ceiling.max(floor))
}

#[allow(clippy::too_many_arguments)]
fn measure_split(
    axis: Axis,
    unit: SeamUnit,
    at: Px,
    min_a: Px,
    min_b: Px,
    children: &[LayoutNode],
    proposal: Proposal,
    env: LayoutEnv,
) -> (Size, Fit) {
    let lane = |main: Option<Px>| match axis {
        Axis::Horizontal => Proposal { width: main, height: proposal.height },
        Axis::Vertical => Proposal { width: proposal.width, height: main },
    };
    let main = |size: &Size| match axis {
        Axis::Horizontal => size.width,
        Axis::Vertical => size.height,
    };
    let cross = |size: &Size| match axis {
        Axis::Horizontal => size.height,
        Axis::Vertical => size.width,
    };
    let proposed_main = match axis {
        Axis::Horizontal => proposal.width,
        Axis::Vertical => proposal.height,
    };

    let (divider_size, divider_fit) = children[1].measure(lane(None), env);
    let thickness = main(&divider_size);

    let (a_main, b_main) = match proposed_main {
        Some(total) => {
            let a = resolve_seam(unit, at, min_a, min_b, total - thickness);
            (Some(a), Some((total - a - thickness).max(0.0)))
        }
        None => (None, None),
    };
    let (a_size, a_fit) = children[0].measure(lane(a_main), env);
    let (b_size, b_fit) = children[2].measure(lane(b_main), env);

    // the Fit carries the LANES, not the children's answers: a rigid
    // child (a text) answers its natural, but its lane is still the
    // lane — the child sits inside it, the seam does not chase content
    let a_lane = a_main.unwrap_or_else(|| main(&a_size));
    let b_lane = b_main.unwrap_or_else(|| main(&b_size));
    let cross_extent = match axis {
        Axis::Horizontal => proposal.height,
        Axis::Vertical => proposal.width,
    }
    .unwrap_or_else(|| cross(&a_size).max(cross(&divider_size)).max(cross(&b_size)));
    let lane_size = |lane_main: Px| match axis {
        Axis::Horizontal => Size { width: lane_main, height: cross_extent },
        Axis::Vertical => Size { width: cross_extent, height: lane_main },
    };

    let main_sum = a_lane + thickness + b_lane;
    let size = match axis {
        Axis::Horizontal => Size {
            width: proposed_main.unwrap_or(main_sum),
            height: cross_extent,
        },
        Axis::Vertical => Size {
            width: cross_extent,
            height: proposed_main.unwrap_or(main_sum),
        },
    };
    (
        size,
        Fit::Children(vec![
            (lane_size(a_lane), a_fit),
            (lane_size(thickness), divider_fit),
            (lane_size(b_lane), b_fit),
        ]),
    )
}

/// A difference within half a device pixel is arithmetic, not appetite:
/// it must neither spin another round of a stack's waterfall nor send a
/// child back to be measured a second time.
const SETTLED: Px = 0.5;

/// The stack algorithm: measures everyone ONCE with no restriction on the
/// main axis (naturals + who is flexible), splits the leftover among the
/// flexibles and re-measures only those. Shrinking never happens behind
/// the scenes: rigid keeps its natural size.
///
/// The split is a waterfall, not one equal cut. A flexible child can be
/// bounded (a `frame_max` title bar over a spacer): offered its equal
/// share, it takes less. What it leaves is not lost — the pool re-splits
/// among the still-hungry and offers again. Every round retires at least
/// one child, so the loop is bounded by the child count; shares within a
/// round are equal, so no child's position buys it space.
///
/// A last pass hands the row's OWN thickness back. A child too big to
/// shrink can make the row thicker than the box that holds it, and
/// whoever CAN take that thickness was measured against the smaller
/// number — so they are asked again, once. A rigid child keeps its size
/// and is aligned instead, which is what alignment is for.
fn measure_stack(
    axis: Axis,
    spacing: Px,
    children: &[LayoutNode],
    proposal: Proposal,
    env: LayoutEnv,
) -> (Size, Fit) {
    let cross_proposal = |main: Option<Px>| match axis {
        Axis::Vertical => Proposal { width: proposal.width, height: main },
        Axis::Horizontal => Proposal { width: main, height: proposal.height },
    };
    let main = |size: &Size| match axis {
        Axis::Vertical => size.height,
        Axis::Horizontal => size.width,
    };
    let cross = |size: &Size| match axis {
        Axis::Vertical => size.width,
        Axis::Horizontal => size.height,
    };
    let proposed_main = match axis {
        Axis::Vertical => proposal.height,
        Axis::Horizontal => proposal.width,
    };

    // phase 1: naturals (unrestricted proposal on the main axis)
    let mut measured: Vec<(Size, Fit)> = children
        .iter()
        .map(|child| child.measure(cross_proposal(None), env))
        .collect();

    let spacing_total = spacing * children.len().saturating_sub(1) as Px;
    let flexible: Vec<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.is_flexible(axis, Some(axis)))
        .map(|(index, _)| index)
        .collect();

    // phase 2: only the flexibles re-measure — the waterfall. Offer equal
    // shares; a child that takes less than its offer is satisfied and its
    // leftover re-splits among the rest, until a round leaves nothing.
    if let Some(total) = proposed_main
        && !flexible.is_empty()
    {
        let rigid: Px = measured
            .iter()
            .enumerate()
            .filter(|(index, _)| !flexible.contains(index))
            .map(|(_, (size, _))| main(size))
            .sum();
        let mut budget = (total - rigid - spacing_total).max(0.0);
        let mut pool = flexible;
        loop {
            let share = budget / pool.len() as Px;
            for &index in &pool {
                measured[index] = children[index].measure(cross_proposal(Some(share)), env);
            }
            let (under, full): (Vec<usize>, Vec<usize>) = pool
                .into_iter()
                .partition(|&index| main(&measured[index].0) < share - SETTLED);
            if under.is_empty() || full.is_empty() {
                break;
            }
            budget -= under.iter().map(|&index| main(&measured[index].0)).sum::<Px>();
            budget = budget.max(0.0);
            pool = full;
        }
    }

    // phase 3: the row's OWN thickness is the truth, not the box it
    // started from. A rail of rigid icons makes the row taller than the
    // window holding it, and a child measured against the smaller number
    // would then be CENTRED in a row it could have filled — which is what
    // makes a squashed window read as broken instead of as clipped: the
    // body walks away from the bar above it and the gap it opens belongs
    // to nobody. A rigid child keeps its size and its alignment, which is
    // what alignment is for; a child that says it takes the room is asked
    // again, ONCE, with the room the row really has.
    let cross_axis = match axis {
        Axis::Vertical => Axis::Horizontal,
        Axis::Horizontal => Axis::Vertical,
    };
    let mut cross_max: Px = measured
        .iter()
        .map(|(size, _)| cross(size))
        .fold(0.0, Px::max);
    for (index, child) in children.iter().enumerate() {
        if cross(&measured[index].0) >= cross_max - SETTLED
            || !child.is_flexible(cross_axis, Some(axis))
        {
            continue;
        }
        // the main axis is settled — only the thickness is news
        let settled = main(&measured[index].0);
        measured[index] = child.measure(
            match axis {
                Axis::Vertical => Proposal { width: Some(cross_max), height: Some(settled) },
                Axis::Horizontal => Proposal { width: Some(settled), height: Some(cross_max) },
            },
            env,
        );
        cross_max = cross_max.max(cross(&measured[index].0));
    }

    let main_sum: Px = measured.iter().map(|(size, _)| main(size)).sum::<Px>() + spacing_total;

    let size = match axis {
        Axis::Vertical => Size { width: cross_max, height: main_sum },
        Axis::Horizontal => Size { width: main_sum, height: cross_max },
    };
    (size, Fit::Children(measured))
}

/// Paints a placed text: single line, word-wrapped, or truncated with an
/// ellipsis — always through the SAME caches as measurement.
fn place_text(
    content: &Arc<str>,
    highlights: Option<&TextHighlight>,
    truncation: Option<Truncation>,
    frame: Rect,
    base_color: Color,
    env: LayoutEnv,
    out: &mut Placement,
) {
    if content.is_empty() {
        return;
    }
    let metrics = env.cache.get_or_measure(content, &env.font, env.text);
    let line_h = metrics.height();
    // the step between lines, and the half-leading that centres the glyph
    // box inside a taller one — the baseline stays ascent-based, so the
    // engine's own raster is unchanged. With no `.line_height(…)` the
    // advance IS the face's box and the leading is zero: byte-identical.
    let advance = env.line_height.unwrap_or(line_h);
    let leading = (advance - line_h) / 2.0;
    let top = frame.origin.y + leading;

    if metrics.width <= frame.size.width {
        let origin = Point { x: frame.origin.x, y: top };
        emit_text_runs(content, (0, content.len()), highlights, origin, base_color, env, out);
        return;
    }
    if let Some(mode) = truncation {
        // highlight does not survive the ellipsis (the original's ranges
        // do not map onto the composed text) — honest v1, noted
        let composed: Arc<str> = Arc::from(truncate_to_width(content, mode, frame.size.width, env));
        let length = composed.len();
        out.draw(DrawCommand::TextLine {
            origin: Point { x: frame.origin.x, y: top },
            content: composed,
            range: (0, length),
            color: base_color,
            font: env.font,
        });
        return;
    }
    let lines = env.cache.get_or_break(content, &env.font, frame.size.width, env.text);
    for (line_index, (start, end)) in lines.iter().enumerate() {
        emit_text_runs(
            content,
            (*start, *end),
            highlights,
            Point { x: frame.origin.x, y: top + line_index as Px * advance },
            base_color,
            env,
            out,
        );
    }
}

/// Emits the `TextLine`s of ONE line: whole in the base color, or sliced
/// into segments when there is a highlight — each segment at its measured
/// prefix position (kerning between segments is approximate; real
/// shaping by runs arrives with the attributed text system).
fn emit_text_runs(
    content: &Arc<str>,
    line: (usize, usize),
    highlights: Option<&TextHighlight>,
    origin: Point,
    base_color: Color,
    env: LayoutEnv,
    out: &mut Placement,
) {
    let (line_start, line_end) = line;
    let whole = || DrawCommand::TextLine {
        origin,
        content: content.clone(),
        range: (line_start, line_end),
        color: base_color,
        font: env.font,
    };
    let Some(highlight) = highlights else {
        out.draw(whole());
        return;
    };

    // segments covering the whole line, alternating base/highlighted
    let clamp = |index: usize| crate::text_input::clamp_index(content, index);
    let mut segments: Vec<(usize, usize, bool)> = Vec::new();
    let mut cursor = line_start;
    for &(start, end) in highlight.ranges.iter() {
        let start = clamp(start).clamp(cursor, line_end);
        let end = clamp(end).min(line_end);
        if end <= start {
            continue;
        }
        if start > cursor {
            segments.push((cursor, start, false));
        }
        segments.push((start, end, true));
        cursor = end;
    }
    if cursor < line_end {
        segments.push((cursor, line_end, false));
    }
    if segments.len() == 1 && !segments[0].2 {
        out.draw(whole());
        return;
    }

    for (start, end, hot) in segments {
        let offset = if start == line_start {
            0.0
        } else {
            env.cache.get_or_measure(&content[line_start..start], &env.font, env.text).width
        };
        out.draw(DrawCommand::TextLine {
            origin: Point { x: origin.x + offset, y: origin.y },
            content: content.clone(),
            range: (start, end),
            color: if hot { highlight.color } else { base_color },
            font: env.font,
        });
    }
}

const ELLIPSIS: &str = "…";

/// Composes the ellipsis version that fits the width — the most content
/// possible, measured for real (every candidate goes through the cache).
fn truncate_to_width(content: &str, mode: Truncation, width: Px, env: LayoutEnv) -> String {
    let fits = |candidate: &str| {
        env.cache.get_or_measure(candidate, &env.font, env.text).width <= width
    };
    match mode {
        Truncation::End => {
            let mut best = ELLIPSIS.to_string();
            for (boundary, _) in content.char_indices().skip(1) {
                let candidate = format!("{}{ELLIPSIS}", &content[..boundary]);
                if fits(&candidate) {
                    best = candidate;
                } else {
                    break;
                }
            }
            best
        }
        Truncation::Start => {
            let mut best = ELLIPSIS.to_string();
            let starts: Vec<usize> = content.char_indices().map(|(index, _)| index).collect();
            for &start in starts.iter().rev() {
                if start == 0 {
                    break; // the whole content did not fit back there
                }
                let candidate = format!("{ELLIPSIS}{}", &content[start..]);
                if fits(&candidate) {
                    best = candidate;
                } else {
                    break;
                }
            }
            best
        }
        Truncation::Middle => {
            let next = |index: usize| crate::text_input::boundary_after(content, index);
            let previous = |index: usize| crate::text_input::boundary_before(content, index);
            let mut head = 0usize;
            let mut tail = content.len();
            loop {
                let mut grew = false;
                let head_next = next(head);
                if head_next <= tail
                    && fits(&format!("{}{ELLIPSIS}{}", &content[..head_next], &content[tail..]))
                {
                    head = head_next;
                    grew = true;
                }
                let tail_previous = previous(tail);
                if tail_previous >= head
                    && fits(&format!(
                        "{}{ELLIPSIS}{}",
                        &content[..head],
                        &content[tail_previous..]
                    ))
                {
                    tail = tail_previous;
                    grew = true;
                }
                if !grew {
                    break;
                }
            }
            format!("{}{ELLIPSIS}{}", &content[..head], &content[tail..])
        }
    }
}

const SCROLLBAR_W: Px = 4.0;
/// How far the thumb sits from the region's edges — and, with the
/// length below, the whole geometry of the track the runtime reads
/// back when a hand drags the thumb.
pub(crate) const SCROLLBAR_INSET: Px = 6.0;
pub(crate) const SCROLLBAR_MIN: Px = 24.0;
/// The band the pointer grabs the thumb by. The painted thumb is four
/// points wide — too thin to aim at — so the target is wider than the
/// paint, the same trick the split's grip plays over its hairline.
pub const SCROLLBAR_GRAB: Px = 12.0;

const FIELD_PAD_H: Px = 8.0;
const FIELD_PAD_V: Px = 5.0;
pub(crate) const FIELD_RADIUS: Px = 5.0;
const FIELD_CARET_W: Px = 1.5;

/// The region's thumb — draw-only at this stage (drag arrives with
/// pointer capture): 4px wide at 6px from the right edge, track with
/// inset 6, floor of 24, proportional to the viewport — and it only
/// exists when there is overflow (short content never gets a bar).
fn draw_scrollbar(
    path: Option<&str>,
    frame: Rect,
    content_h: Px,
    offset_y: Px,
    max_y: Px,
    out: &mut Placement,
) {
    let track = frame.size.height - 2.0 * SCROLLBAR_INSET;
    if track <= 0.0 {
        return;
    }
    let thumb_h = ((frame.size.height / content_h) * track).max(SCROLLBAR_MIN).min(track);
    let travel = track - thumb_h;
    let thumb_y = frame.origin.y + SCROLLBAR_INSET + travel * (offset_y / max_y);
    let thumb_x = frame.origin.x + frame.size.width - SCROLLBAR_INSET - SCROLLBAR_W;
    out.draw(DrawCommand::FillRect {
        rect: Rect {
            origin: Point { x: thumb_x, y: thumb_y },
            size: Size { width: SCROLLBAR_W, height: thumb_h },
        },
        color: crate::theme::current().scrollbar,
        corner_radius: Corners::all(SCROLLBAR_W / 2.0),
    });
    // the grab band, over the paint and pushed AFTER the content: the
    // reverse hit walk finds it first, and the thumb is draggable
    // wherever the pointer can plausibly aim at it
    if let Some(path) = path {
        let band = Rect {
            origin: Point {
                x: (thumb_x + SCROLLBAR_W / 2.0 - SCROLLBAR_GRAB / 2.0).max(frame.origin.x),
                y: thumb_y,
            },
            size: Size { width: SCROLLBAR_GRAB, height: thumb_h },
        };
        if let Some(visible) = clip_of(out, band) {
            out.hits.push((format!("{path}/#thumb-v"), visible));
        }
    }
}

/// What the current clip lets through of a rect — the door every
/// geometry-routed region goes through. What is not visible cannot be
/// grabbed, explained, dropped on or dragged from.
fn clip_of(out: &Placement, rect: Rect) -> Option<Rect> {
    match out.current_clip() {
        Some(clip) => rect.intersection(clip),
        None => Some(rect),
    }
}

/// The vertical thumb, turned on its side.
fn draw_scrollbar_h(
    path: Option<&str>,
    frame: Rect,
    content_w: Px,
    offset_x: Px,
    max_x: Px,
    out: &mut Placement,
) {
    let track = frame.size.width - 2.0 * SCROLLBAR_INSET;
    if track <= 0.0 {
        return;
    }
    let thumb_w = ((frame.size.width / content_w) * track).max(SCROLLBAR_MIN).min(track);
    let travel = track - thumb_w;
    let thumb_x = frame.origin.x + SCROLLBAR_INSET + travel * (offset_x / max_x);
    let thumb_y = frame.origin.y + frame.size.height - SCROLLBAR_INSET - SCROLLBAR_W;
    out.draw(DrawCommand::FillRect {
        rect: Rect {
            origin: Point { x: thumb_x, y: thumb_y },
            size: Size { width: thumb_w, height: SCROLLBAR_W },
        },
        color: crate::theme::current().scrollbar,
        corner_radius: Corners::all(SCROLLBAR_W / 2.0),
    });
    if let Some(path) = path {
        let band = Rect {
            origin: Point {
                x: thumb_x,
                y: (thumb_y + SCROLLBAR_W / 2.0 - SCROLLBAR_GRAB / 2.0).max(frame.origin.y),
            },
            size: Size { width: thumb_w, height: SCROLLBAR_GRAB },
        };
        if let Some(visible) = clip_of(out, band) {
            out.hits.push((format!("{path}/#thumb-h"), visible));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn place_stack(
    axis: Axis,
    spacing: Px,
    align: CrossAlign,
    children: &[LayoutNode],
    frame: Rect,
    fits: Vec<(Size, Fit)>,
    env: LayoutEnv,
    out: &mut Placement,
) {
    let mut cursor = match axis {
        Axis::Vertical => frame.origin.y,
        Axis::Horizontal => frame.origin.x,
    };
    // baseline alignment: every child sits on the SHARED first
    // baseline — text by its ascent, a baselineless box by its bottom
    // edge (its baseline IS its bottom, the SwiftUI rule). Computed
    // only when asked; the other alignments pay nothing.
    let baselines: Option<Vec<Px>> = (align == CrossAlign::Baseline
        && axis == Axis::Horizontal)
        .then(|| {
            children
                .iter()
                .zip(&fits)
                .map(|(child, (size, _))| {
                    child.first_baseline(env).unwrap_or(size.height)
                })
                .collect()
        });
    let shared = baselines
        .as_ref()
        .map(|baselines| baselines.iter().fold(0.0_f64, |acc, b| acc.max(*b)));
    for (index, (child, (size, fit))) in children.iter().zip(fits).enumerate() {
        let cross_offset = |extent: Px, len: Px| match align {
            CrossAlign::Start => 0.0,
            CrossAlign::Center => (extent - len) / 2.0,
            CrossAlign::End => extent - len,
            CrossAlign::Baseline => match (&baselines, shared) {
                (Some(baselines), Some(shared)) => shared - baselines[index],
                // a vertical stack has no shared baseline: start
                _ => 0.0,
            },
        };
        let origin = match axis {
            Axis::Vertical => Point {
                x: frame.origin.x + cross_offset(frame.size.width, size.width),
                y: cursor,
            },
            Axis::Horizontal => Point {
                x: cursor,
                y: frame.origin.y + cross_offset(frame.size.height, size.height),
            },
        };
        child.place(Rect { origin, size }, fit, env, out);
        cursor += match axis {
            Axis::Vertical => size.height,
            Axis::Horizontal => size.width,
        } + spacing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(chars: usize) -> LayoutNode {
        LayoutNode::Text { content: Arc::from("x".repeat(chars)), highlights: None, truncation: None }
    }

    fn boundary(path: &str, child: LayoutNode) -> LayoutNode {
        LayoutNode::Boundary { path: path.to_string(), children: vec![child] }
    }

    #[test]
    fn vstack_distributes_the_remainder_to_the_spacer() {
        let root = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                boundary("top", text(10)),
                boundary("gap", LayoutNode::Spacer),
                boundary("bottom", text(5)),
            ],
        };
        let result = layout(&root, Proposal { width: Some(200.0), height: Some(100.0) });

        assert_eq!(result.size.height, 100.0);
        assert_eq!(result.frames.get("top").unwrap().origin.y, 0.0);
        assert_eq!(result.frames.get("gap").unwrap().size.height, 68.0);
        assert_eq!(result.frames.get("bottom").unwrap().origin.y, 84.0);
    }

    /// A box shorter than the rail it holds: ten rigid icons make the row
    /// 280 tall while the box offers 174. The row answers 280, which is
    /// honest — it cannot shrink an icon. What it must NOT do is leave the
    /// body at the 174 it was asked for and CENTRE it inside the 280 it
    /// became: the body walks away from the bar above it, the gap it opens
    /// belongs to nobody, and a squashed window reads as broken instead of
    /// as clipped. Whoever can take the row's own height is asked again.
    #[test]
    fn a_squashed_row_hands_its_own_height_to_whoever_can_fill_it() {
        let rail = boundary(
            "rail",
            LayoutNode::Frame {
                width: Some(48.0),
                height: Some(280.0),
                child: Box::new(LayoutNode::Spacer),
            },
        );
        // the body is a `Fill` — a bare spacer is flexible on the row's
        // MAIN axis only, so filling the cross thickness is a fill's job
        let row = LayoutNode::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: CrossAlign::Center,
            children: vec![rail, boundary("body", LayoutNode::Fill)],
        };
        let result = layout(&row, Proposal { width: Some(1280.0), height: Some(174.0) });

        assert_eq!(result.size.height, 280.0, "the row is as tall as the rail it cannot shrink");
        let body = result.frames.get("body").unwrap();
        assert_eq!(body.size.height, 280.0, "the body takes the row it is in");
        assert_eq!(body.origin.y, 0.0, "and starts where the row starts");
    }

    /// The same rule on the other axis, which is where it reads worst. A
    /// body too WIDE to shrink makes the frame wider than the window, and
    /// a title bar centred inside that frame walks to the right — away
    /// from the corner the window counts from, and away from the buttons
    /// the system draws there. The bar takes the frame's width instead
    /// and loses its right end to the window edge, which is what being
    /// clipped means.
    #[test]
    fn a_squashed_column_keeps_its_bar_on_the_edge_the_window_counts_from() {
        // the bar fills the frame's width with a `Fill`: a bare spacer is
        // flexible on the column's MAIN axis (vertical) only, and here the
        // bar has to take the CROSS extent the too-wide body forced
        let bar = boundary(
            "bar",
            LayoutNode::Frame {
                width: None,
                height: Some(40.0),
                child: Box::new(LayoutNode::Fill),
            },
        );
        let body = boundary(
            "body",
            LayoutNode::Frame {
                width: Some(948.0),
                height: None,
                child: Box::new(LayoutNode::Spacer),
            },
        );
        let column = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Center,
            children: vec![bar, body],
        };
        let result = layout(&column, Proposal { width: Some(500.0), height: Some(800.0) });

        assert_eq!(result.size.width, 948.0, "the frame is as wide as the body it cannot shrink");
        let bar = result.frames.get("bar").unwrap();
        assert_eq!(bar.origin.x, 0.0, "the bar stays on the leading edge");
        assert_eq!(bar.size.width, 948.0, "and takes the frame's own width");
    }

    /// A pile obeys the same rule, on both axes at once. The ambience
    /// under a frame taller than the window answered the window, because
    /// that is what it was asked; the frame could not shrink and answered
    /// more. Centring the wash inside the pile slides it off the corner it
    /// was painted for — it takes the box the layers ended up needing.
    #[test]
    fn a_layer_that_can_fill_the_pile_is_not_centred_in_it() {
        let pile = LayoutNode::Layered {
            modal: false,
            align: CrossAlign::Center,
            children: vec![
                boundary("wash", LayoutNode::Spacer),
                boundary(
                    "frame",
                    LayoutNode::Frame {
                        width: None,
                        height: Some(372.0),
                        child: Box::new(LayoutNode::Spacer),
                    },
                ),
            ],
        };
        let result = layout(&pile, Proposal { width: Some(1280.0), height: Some(240.0) });

        assert_eq!(result.size.height, 372.0, "the pile is as tall as the frame in it");
        let wash = result.frames.get("wash").unwrap();
        assert_eq!(wash.size.height, 372.0, "the wash takes the pile");
        assert_eq!(wash.origin.y, 0.0, "and starts where the pile starts");
    }

    #[test]
    fn the_waterfall_hands_a_capped_flexible_leftover_to_the_hungry() {
        // The workbench frame in miniature: a title bar and a footer are
        // flexible (a spacer lives in each) but bounded, the body is not.
        // One equal cut would hand every child 800/3 and lose what the
        // bars decline; the waterfall re-offers it to the body.
        let capped = |name: &str, cap: f64| {
            boundary(
                name,
                LayoutNode::MaxFrame {
                    max_width: f64::INFINITY,
                    max_height: cap,
                    align: CrossAlign::Start,
                    child: Box::new(LayoutNode::Spacer),
                },
            )
        };
        let root = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                capped("bar", 40.0),
                boundary("body", LayoutNode::Spacer),
                capped("foot", 26.0),
            ],
        };
        let result = layout(&root, Proposal { width: Some(1280.0), height: Some(800.0) });

        assert_eq!(result.size.height, 800.0);
        assert_eq!(result.frames.get("bar").unwrap().size.height, 40.0);
        assert_eq!(result.frames.get("body").unwrap().size.height, 734.0);
        assert_eq!(result.frames.get("foot").unwrap().origin.y, 774.0);
    }

    #[test]
    fn the_waterfall_settles_a_side_panel_against_a_hungry_editor() {
        // The other axis of the same screen: a 260-capped panel, a 1px
        // divider, an editor that wants the rest. Equal thirds would give
        // the editor 399; the waterfall gives it everything the panel and
        // the divider do not use.
        let root = LayoutNode::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                boundary(
                    "panel",
                    LayoutNode::MaxFrame {
                        max_width: 260.0,
                        max_height: f64::INFINITY,
                        align: CrossAlign::Start,
                        child: Box::new(LayoutNode::Spacer),
                    },
                ),
                boundary(
                    "line",
                    LayoutNode::Frame {
                        width: Some(1.0),
                        height: None,
                        child: Box::new(LayoutNode::Spacer),
                    },
                ),
                boundary("editor", LayoutNode::Spacer),
            ],
        };
        let result = layout(&root, Proposal { width: Some(1200.0), height: Some(700.0) });

        assert_eq!(result.frames.get("panel").unwrap().size.width, 260.0);
        assert_eq!(result.frames.get("line").unwrap().size.width, 1.0);
        assert_eq!(result.frames.get("editor").unwrap().size.width, 939.0);
        assert_eq!(result.frames.get("editor").unwrap().origin.x, 261.0);
    }

    #[test]
    fn a_layer_is_invisible_to_flexibility() {
        // the same tree twice: as a Layered, a flexible child makes the
        // whole thing flexible; as an Overlay, the layer is not asked
        let rule = LayoutNode::Spacer;
        let chip = LayoutNode::Leaf { size: Size { width: 60.0, height: 20.0 } };
        let layered = LayoutNode::Layered {
            modal: false,
            align: CrossAlign::Center,
            children: vec![chip.clone(), rule.clone()],
        };
        assert!(
            layered.is_flexible(Axis::Horizontal, None),
            "a layered pile takes its children's flexibility"
        );
        let overlaid = LayoutNode::Overlay {
            at: UnitPoint::BOTTOM,
            behind: false,
            layer: Box::new(rule),
            child: Box::new(chip),
        };
        assert!(
            !overlaid.is_flexible(Axis::Horizontal, None),
            "an overlay answers for the BASE alone — the whole cure of the pain"
        );
    }

    #[test]
    fn a_spacer_is_flexible_only_along_its_stacks_main_axis() {
        // the cure of the title-bar pain: a bare spacer in a row pushes
        // sideways and never lets the row eat the leftover HEIGHT
        let row = |child: LayoutNode| LayoutNode::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: CrossAlign::Center,
            children: vec![
                LayoutNode::Leaf { size: Size { width: 40.0, height: 20.0 } },
                child,
            ],
        };
        let bare = row(LayoutNode::Spacer);
        assert!(bare.is_flexible(Axis::Horizontal, None), "the row spreads sideways");
        assert!(
            !bare.is_flexible(Axis::Vertical, None),
            "and never takes the leftover height — what page::push worked around"
        );
        // the same holds through a Frame that pins the other axis, the
        // way `spacer().frame_height(0.0)` used to have to
        let framed = row(LayoutNode::Frame {
            width: None,
            height: Some(0.0),
            child: Box::new(LayoutNode::Spacer),
        });
        assert!(!framed.is_flexible(Axis::Vertical, None));
        assert!(framed.is_flexible(Axis::Horizontal, None));
        // a column turns the axes over
        let column = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Center,
            children: vec![LayoutNode::Spacer],
        };
        assert!(column.is_flexible(Axis::Vertical, None));
        assert!(!column.is_flexible(Axis::Horizontal, None));
        // a spacer measured on its own — no stack in reach — keeps the
        // old bi-axial answer
        assert!(LayoutNode::Spacer.is_flexible(Axis::Horizontal, None));
        assert!(LayoutNode::Spacer.is_flexible(Axis::Vertical, None));
    }

    #[test]
    fn a_split_lays_exact_lanes_and_offers_a_grip() {
        let split = |at: f64| LayoutNode::Split {
            path: "seam".into(),
            axis: Axis::Horizontal,
            unit: SeamUnit::Points,
            at,
            min_a: 100.0,
            min_b: 100.0,
            children: vec![
                boundary("a", LayoutNode::Spacer),
                LayoutNode::Frame {
                    width: Some(1.0),
                    height: None,
                    child: Box::new(LayoutNode::Spacer),
                },
                boundary("b", LayoutNode::Spacer),
            ],
        };
        let result = layout(&split(260.0), Proposal { width: Some(1200.0), height: Some(700.0) });

        assert_eq!(result.frames.get("a").unwrap().size.width, 260.0);
        assert_eq!(result.frames.get("b").unwrap().size.width, 939.0);
        assert_eq!(result.frames.get("b").unwrap().origin.x, 261.0);
        // the grip band rides the seam, wider than the hairline, and is
        // the TOPMOST hit there
        let (path, grip) = result.hits.last().expect("the seam registers a grip").clone();
        assert_eq!(path, "seam/#split");
        assert_eq!(grip.size.width, SPLIT_GRIP);
        assert!(grip.origin.x < 260.5 && 260.5 < grip.origin.x + grip.size.width);
        // the placement carries the drag's geometry
        assert_eq!(result.splits.len(), 1);
        assert_eq!(result.splits[0].min_b, 100.0);

        // the clamps hold at both ends
        let low = layout(&split(0.0), Proposal { width: Some(1200.0), height: Some(700.0) });
        assert_eq!(low.frames.get("a").unwrap().size.width, 100.0);
        let high = layout(&split(9999.0), Proposal { width: Some(1200.0), height: Some(700.0) });
        assert_eq!(high.frames.get("a").unwrap().size.width, 1099.0);
    }

    #[test]
    fn a_split_stacks_its_lanes_when_the_axis_says_so() {
        // the same seam, turned: a panel with a list on top and a
        // history below, and the grip drags up and down
        let split = LayoutNode::Split {
            path: "seam".into(),
            axis: Axis::Vertical,
            unit: SeamUnit::Points,
            at: 300.0,
            min_a: 80.0,
            min_b: 80.0,
            children: vec![
                boundary("top", LayoutNode::Spacer),
                LayoutNode::Frame {
                    width: None,
                    height: Some(1.0),
                    child: Box::new(LayoutNode::Spacer),
                },
                boundary("bottom", LayoutNode::Spacer),
            ],
        };
        let result = layout(&split, Proposal { width: Some(400.0), height: Some(700.0) });

        assert_eq!(result.frames.get("top").unwrap().size.height, 300.0);
        assert_eq!(result.frames.get("bottom").unwrap().size.height, 399.0);
        assert_eq!(result.frames.get("bottom").unwrap().origin.y, 301.0);
        // both lanes take the full width — the seam only cuts one axis
        assert_eq!(result.frames.get("top").unwrap().size.width, 400.0);
        // the grip band rides the seam horizontally now
        let (path, grip) = result.hits.last().expect("the seam registers a grip").clone();
        assert_eq!(path, "seam/#split");
        assert_eq!(grip.size.height, SPLIT_GRIP);
        assert!(grip.origin.y < 300.5 && 300.5 < grip.origin.y + grip.size.height);
    }

    #[test]
    fn a_single_axis_frame_pins_one_axis_and_asks_the_other() {
        // `.frame(height: 22)` on a row: the height is EXACT — not a
        // ceiling — while the width still follows the proposal through.
        let root = LayoutNode::Frame {
            width: None,
            height: Some(22.0),
            child: Box::new(boundary("row", LayoutNode::Spacer)),
        };
        let result = layout(&root, Proposal { width: Some(300.0), height: Some(600.0) });

        assert_eq!(result.size.height, 22.0);
        assert_eq!(result.size.width, 300.0);
        assert_eq!(result.frames.get("row").unwrap().size.height, 22.0);
    }

    #[test]
    fn scroll_region_never_propagates_the_content_minimum() {
        // The classic flexbox pain, dead by construction: header + scroll
        // of giant content in a 300 viewport — the header stays natural,
        // the region answers what was left and the content overflows ON
        // THE INSIDE. No min_h(0), no magic overflow.
        let root = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                boundary("header", text(10)),
                boundary(
                    "region",
                    LayoutNode::Scroll {
                        axes: crate::layout::ScrollAxes::Vertical,
                        path: None,
                        target: None,
                        child: Box::new(boundary("content", text(1000))),
                    },
                ),
            ],
        };
        let result = layout(&root, Proposal { width: Some(400.0), height: Some(300.0) });

        let header = result.frames.get("header").unwrap();
        let region = result.frames.get("region").unwrap();
        let content = result.frames.get("content").unwrap();

        assert_eq!(header.size.height, LINE_H);
        assert_eq!(region.size.height, 300.0 - LINE_H, "the region takes what was left");
        assert!(
            content.size.height > region.size.height,
            "the content overflows on the inside — that is what scrolls: {} > {}",
            content.size.height,
            region.size.height
        );
    }

    #[test]
    fn padding_shrinks_the_proposal_and_grows_the_answer() {
        let root = LayoutNode::Padding {
            edges: Edges::uniform(10.0),
            child: Box::new(boundary("inner", text(5))),
        };
        let result = layout(&root, Proposal::unspecified());

        assert_eq!(result.size, Size { width: 60.0, height: 36.0 });
        let inner = result.frames.get("inner").unwrap();
        assert_eq!(inner.origin, Point { x: 10.0, y: 10.0 });
    }

    #[test]
    fn frame_overrides_only_its_axes() {
        let root = LayoutNode::Frame {
            width: Some(100.0),
            height: None,
            child: Box::new(text(5)),
        };
        let result = layout(&root, Proposal::unspecified());
        assert_eq!(result.size, Size { width: 100.0, height: LINE_H });
    }

    #[test]
    fn a_gradient_resolves_against_the_box_it_paints() {
        // the declaration is proportional; what the rasterizers get is
        // px, resolved once, here
        let frame = Rect {
            origin: Point { x: 10.0, y: 20.0 },
            size: Size { width: 100.0, height: 40.0 },
        };
        let inner = Color::hex(0xFF0000);
        let outer = Color::hex(0x0000FF);
        match Gradient::radial(inner, outer).resolve(frame) {
            GradientPaint::Radial { center, start, end, .. } => {
                assert_eq!(center, Point { x: 60.0, y: 40.0 }, "the centre of the box");
                assert_eq!(start, 0.0);
                // the default reach is the farthest corner
                assert!((end - 50.0_f64.hypot(20.0)).abs() < 0.001, "reach {end}");
            }
            other => panic!("a radial gradient stays radial: {other:?}"),
        }
        // a corner centre reaches the OPPOSITE corner
        let corner = Gradient::radial(inner, outer).center(UnitPoint::TOP_LEADING);
        match corner.resolve(frame) {
            GradientPaint::Radial { center, end, .. } => {
                assert_eq!(center, frame.origin);
                assert!((end - 100.0_f64.hypot(40.0)).abs() < 0.001);
            }
            other => panic!("{other:?}"),
        }
        match Gradient::linear(inner, outer).resolve(frame) {
            GradientPaint::Linear { start, end, .. } => {
                assert_eq!(start, Point { x: 60.0, y: 20.0 });
                assert_eq!(end, Point { x: 60.0, y: 60.0 });
            }
            other => panic!("{other:?}"),
        }
    }

    /// Measures a node with the tests' default environment (PixelFont).
    fn measure_with_defaults(node: &LayoutNode, proposal: Proposal) -> Size {
        let engine = PixelFont;
        let images = RawImages::default();
        let cache = MeasureCache::default();
        let offsets = HashMap::default();
        let interaction = Interaction::default();
        let carets = HashMap::default();
        let env = LayoutEnv {
            text: &engine,
            images: &images,
            cache: &cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
            line_height: None,
            stamp: FrameStamp::idle(&interaction, &carets),
            animator: None,
            anim: None,
            live: None,
            overlay_bounds: None,
            scale: 1.0,
        };
        node.measure(proposal, env).0
    }

    /// Full layout with a pointer stamped into the env — how tests drive
    /// hover/pressed now that frame state has left the tree.
    fn layout_with_pointer(
        root: &LayoutNode,
        proposal: Proposal,
        interaction: &Interaction,
    ) -> LayoutResult {
        let engine = PixelFont;
        let images = RawImages::default();
        let cache = MeasureCache::default();
        let offsets = HashMap::default();
        let carets = HashMap::default();
        layout_with(
            root,
            proposal,
            LayoutEnv {
                text: &engine,
                images: &images,
                cache: &cache,
                scroll_offsets: &offsets,
                font: FontSpec::DEFAULT,
                line_height: None,
                stamp: FrameStamp::idle(interaction, &carets),
                animator: None,
                anim: None,
                live: None,
                overlay_bounds: None,
                scale: 1.0,
            },
        )
    }

    #[test]
    fn max_frame_fills_when_infinite_and_caps_when_finite() {
        let fill = LayoutNode::MaxFrame {
            max_width: f64::INFINITY,
            max_height: 60.0,
            align: CrossAlign::Start,
            child: Box::new(text(5)),
        };
        let size =
            measure_with_defaults(&fill, Proposal { width: Some(300.0), height: Some(500.0) });
        assert_eq!(size, Size { width: 300.0, height: LINE_H });
    }

    #[test]
    fn text_wraps_against_the_proposed_width() {
        let size =
            measure_with_defaults(&text(100), Proposal { width: Some(100.0), height: None });
        // 100 chars with no space in 100px: hard-break by whole CHAR —
        // 12 per line (96px ≤ 100) → 9 lines (real wrapping does not
        // split characters the way the old average math did)
        assert_eq!(size, Size { width: 100.0, height: 144.0 });
    }

    #[test]
    fn words_wrap_at_spaces_never_mid_word() {
        let node = LayoutNode::Text { content: Arc::from("aa bb cc"), highlights: None, truncation: None };
        let result = layout(&node, Proposal { width: Some(40.0), height: None });

        // "aa bb" (40px) fits; "cc" goes down whole — never an orphan "c"
        assert_eq!(result.size.height, 2.0 * LINE_H);
        let lines: Vec<String> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, range, .. } => Some(content[range.0..range.1].to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec!["aa bb ".to_string(), "cc".to_string()]);
    }

    #[test]
    fn highlight_splits_the_line_into_colored_runs() {
        let hot = Color::hex(0xFF0000);
        let node = LayoutNode::Text {
            content: Arc::from("abcdef"),
            highlights: Some(TextHighlight { ranges: Rc::new(vec![(2, 4)]), color: hot }),
            truncation: None,
        };
        let result = layout(&node, Proposal::unspecified());

        let runs: Vec<(String, Color, Px)> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, range, color, origin, .. } => {
                    Some((content[range.0..range.1].to_string(), *color, origin.x))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            runs,
            vec![
                ("ab".to_string(), Color::BLACK, 0.0),
                ("cd".to_string(), hot, 16.0),
                ("ef".to_string(), Color::BLACK, 32.0),
            ],
            "segments at the measured prefix positions"
        );
    }

    #[test]
    fn highlight_survives_the_word_wrap() {
        let hot = Color::hex(0xFF0000);
        // "aa bb cc" at 40px breaks into "aa bb " + "cc"; the ranges
        // cover the "bb" (line 1) and the "cc" (line 2)
        let node = LayoutNode::Text {
            content: Arc::from("aa bb cc"),
            highlights: Some(TextHighlight {
                ranges: Rc::new(vec![(3, 5), (6, 8)]),
                color: hot,
            }),
            truncation: None,
        };
        let result = layout(&node, Proposal::exact(Size { width: 40.0, height: 100.0 }));

        let hot_runs: Vec<(String, Px, Px)> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, range, color, origin, .. } if *color == hot => {
                    Some((content[range.0..range.1].to_string(), origin.x, origin.y))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            hot_runs,
            vec![
                ("bb".to_string(), 24.0, 0.0),
                ("cc".to_string(), 0.0, LINE_H),
            ],
            "each line crops the ranges that intersect it"
        );
    }

    #[test]
    fn truncation_places_the_ellipsis_where_asked() {
        let truncated = |mode: Truncation| {
            let node = LayoutNode::Text {
                content: Arc::from("abcdefgh"),
                highlights: None,
                truncation: Some(mode),
            };
            let result = layout(&node, Proposal { width: Some(40.0), height: None });
            assert_eq!(result.size.height, LINE_H, "truncation never wraps a line");
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::TextLine { content, range, .. } => Some(content[range.0..range.1].to_string()),
                    _ => None,
                })
                .unwrap()
        };
        // PixelFont: 8px per char, "…" too
        assert_eq!(truncated(Truncation::End), "abcd…");
        assert_eq!(truncated(Truncation::Start), "…efgh");
        assert_eq!(truncated(Truncation::Middle), "ab…gh");
    }

    #[test]
    fn a_word_longer_than_the_line_hard_breaks() {
        let node = LayoutNode::Text { content: Arc::from("aaaaaaaaaa"), highlights: None, truncation: None };
        let result = layout(&node, Proposal { width: Some(40.0), height: None });

        // 10 chars of 8px in 40px: 5 per line
        assert_eq!(result.size.height, 2.0 * LINE_H);
        let lines: Vec<String> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, range, .. } => Some(content[range.0..range.1].to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec!["aaaaa".to_string(), "aaaaa".to_string()]);
    }

    fn styled(props: VisualProps, child: LayoutNode) -> LayoutNode {
        LayoutNode::Styled { props: Box::new(props), child: Box::new(child) }
    }

    fn rows(count: usize) -> LayoutNode {
        LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: (0..count)
                .map(|index| boundary(&format!("row{index}"), text(4)))
                .collect(),
        }
    }

    #[test]
    fn scroll_offset_moves_content_under_the_clip() {
        let engine = PixelFont;
        let images = RawImages::default();
        let cache = MeasureCache::default();
        let mut offsets = HashMap::default();
        offsets.insert("list".to_string(), Point { x: 0.0, y: 40.0 });
        let interaction = Interaction::default();
        let carets = HashMap::default();
        let env = LayoutEnv {
            text: &engine,
            images: &images,
            cache: &cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
            line_height: None,
            stamp: FrameStamp::idle(&interaction, &carets),
            animator: None,
            anim: None,
            live: None,
            overlay_bounds: None,
            scale: 1.0,
        };

        let root = LayoutNode::Scroll {
            axes: crate::layout::ScrollAxes::Vertical,
            path: Some("list".to_string()),
            target: None,
            child: Box::new(rows(10)),
        };
        let result = layout_with(
            &root,
            Proposal::exact(Size { width: 100.0, height: 100.0 }),
            env,
        );

        assert_eq!(result.scrolls.len(), 1);
        assert_eq!(result.scrolls[0].content.height, 160.0, "the real content stays in the region");
        assert_eq!(
            result.frames.get("row0").unwrap().origin.y,
            -40.0,
            "offset 40 pushes row 0 up above the viewport"
        );
        assert!(
            result.display.iter().any(|command| matches!(
                command,
                DrawCommand::PushClip { rect, .. } if rect.size.height == 100.0
            )),
            "the region clips at its own frame"
        );
    }

    #[test]
    fn hits_outside_the_viewport_do_not_exist() {
        let interactive = |path: &str| LayoutNode::Interactive {
            path: path.to_string(),
            child: Box::new(text(4)),
        };
        let root = LayoutNode::Scroll {
            axes: crate::layout::ScrollAxes::Vertical,
            path: Some("list".to_string()),
            target: None,
            child: Box::new(LayoutNode::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: CrossAlign::Start,
                children: vec![
                    interactive("inside"),  // y [0, 16)
                    interactive("half"),    // y [16, 32) — the viewport cuts at 24
                    interactive("outside"), // y [32, 48) — invisible
                ],
            }),
        };
        let result = layout(&root, Proposal::exact(Size { width: 100.0, height: 24.0 }));

        assert!(result.hits.iter().any(|(path, _)| path == "inside"));
        let half = result
            .hits
            .iter()
            .find(|(path, _)| path == "half")
            .map(|(_, rect)| *rect)
            .expect("the half-visible one exists");
        assert_eq!(half.size.height, 8.0, "the hit is only the visible part");
        assert!(
            !result.hits.iter().any(|(path, _)| path == "outside"),
            "outside the viewport the hit does NOT exist"
        );
    }

    #[test]
    fn scrollbar_appears_only_with_overflow() {
        let scroll = |count: usize| LayoutNode::Scroll {
            axes: crate::layout::ScrollAxes::Vertical,
            path: Some("list".to_string()),
            target: None,
            child: Box::new(rows(count)),
        };
        let viewport = Proposal::exact(Size { width: 100.0, height: 100.0 });
        let thumb_of = |result: &LayoutResult| {
            result.display.iter().find_map(|command| match command {
                DrawCommand::FillRect { rect, color, .. } if *color == Color::SCROLLBAR => {
                    Some(*rect)
                }
                _ => None,
            })
        };

        let fits = layout(&scroll(2), viewport);
        assert!(thumb_of(&fits).is_none(), "short content never gets a bar");

        let over = layout(&scroll(10), viewport);
        let thumb = thumb_of(&over).expect("overflow gets a thumb");
        // track 88 (inset 6 on both sides), proportional 100/160
        assert_eq!(thumb.size.height, (100.0 / 160.0_f64 * 88.0).max(24.0));
        assert_eq!(thumb.size.width, 4.0);
        assert_eq!(thumb.origin.x, 100.0 - 6.0 - 4.0);
    }

    #[test]
    fn styled_paints_background_behind_and_border_on_top() {
        let root = styled(
            VisualProps {
                background: Some(Color::hex(0x112233)),
                border: Some((Color::hex(0x445566), 2.0)),
                corner_radius: Some(Corners::all(4.0)),
                ..VisualProps::default()
            },
            text(3),
        );
        let result = layout(&root, Proposal::unspecified());

        let commands: Vec<_> = result.display.iter().collect();
        assert_eq!(commands.len(), 3, "background, text, border — in this order");
        assert!(matches!(
            commands[0],
            DrawCommand::FillRect { color, corner_radius, .. }
                if *color == Color::hex(0x112233) && *corner_radius == Corners::all(4.0)
        ));
        assert!(matches!(commands[1], DrawCommand::TextLine { .. }));
        assert!(matches!(
            commands[2],
            DrawCommand::StrokeRect { color, width, .. }
                if *color == Color::hex(0x445566) && *width == 2.0
        ));
    }

    #[test]
    fn styled_never_changes_measurement() {
        // the LAW at node level: VisualProps is pure paint
        let plain = layout(&text(7), Proposal::unspecified());
        let dressed = layout(
            &styled(
                VisualProps {
                    background: Some(Color::BLACK),
                    border: Some((Color::WHITE, 3.0)),
                    corner_radius: Some(Corners::all(8.0)),
                    ..VisualProps::default()
                },
                text(7),
            ),
            Proposal::unspecified(),
        );
        assert_eq!(plain.size, dressed.size);
    }

    #[test]
    fn foreground_inherits_and_the_nearest_wins() {
        let outer = Color::hex(0x00AA00);
        let inner = Color::hex(0xAA0000);
        let root = styled(
            VisualProps { foreground: Some(outer), ..VisualProps::default() },
            LayoutNode::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: CrossAlign::Start,
                children: vec![
                    text(3),
                    styled(
                        VisualProps { foreground: Some(inner), ..VisualProps::default() },
                        text(3),
                    ),
                ],
            },
        );
        let result = layout(&root, Proposal::unspecified());

        let colors: Vec<Color> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(colors, vec![outer, inner]);
    }

    #[test]
    fn hovered_swaps_paint_but_never_frames() {
        // hover lives in the ENV, never in the node: the SAME tree with
        // different stamps must give identical frames (the LAW, now by
        // type)
        let node = LayoutNode::Interactive {
            path: "button".to_string(),
            child: Box::new(styled(
                VisualProps {
                    background: Some(Color::hex(0x111111)),
                    background_hovered: Some(Color::hex(0x222222)),
                    ..VisualProps::default()
                },
                boundary("label", text(4)),
            )),
        };
        let idle = Interaction::default();
        let hovering =
            Interaction { hovered: Some("button".to_string()), ..Interaction::default() };
        let cold = layout_with_pointer(&node, Proposal::unspecified(), &idle);
        let hot = layout_with_pointer(&node, Proposal::unspecified(), &hovering);

        assert_eq!(cold.size, hot.size);
        assert_eq!(
            cold.frames.get("label"),
            hot.frames.get("label"),
            "the LAW: hover never touches a frame"
        );
        let background = |result: &LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { color, .. } => Some(*color),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(background(&cold), Color::hex(0x111111));
        assert_eq!(background(&hot), Color::hex(0x222222));
    }

    #[test]
    fn pressed_beats_hovered() {
        let root = LayoutNode::Interactive {
            path: "button".to_string(),
            child: Box::new(styled(
                VisualProps {
                    background: Some(Color::hex(0x111111)),
                    background_hovered: Some(Color::hex(0x222222)),
                    background_pressed: Some(Color::hex(0x333333)),
                    ..VisualProps::default()
                },
                text(2),
            )),
        };
        let pressing = Interaction {
            hovered: Some("button".to_string()),
            pressed: Some("button".to_string()),
            ..Interaction::default()
        };
        let result = layout_with_pointer(&root, Proposal::unspecified(), &pressing);

        let background = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
            .unwrap();
        assert_eq!(background, Color::hex(0x333333));
    }

    #[test]
    fn an_elliptical_ramp_reaches_both_radii() {
        let wash = Gradient::radial(Color::hex(0xFF0000), Color::hex(0x0000FF))
            .radius(392.0, 560.0)
            .aspect(260.0 / 560.0);
        let frame = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size { width: 1120.0, height: 520.0 },
        };
        let paint = wash.resolve(frame);
        // the far color lands at BOTH ends of the ellipse: 560 across,
        // 260 down — and the 70% stop rides both axes
        let far = Color::hex(0x0000FF);
        assert_eq!(paint.at(Point { x: 560.0 + 560.0, y: 260.0 }), far);
        assert_eq!(paint.at(Point { x: 560.0, y: 260.0 + 260.0 }), far);
        let near = Color::hex(0xFF0000);
        assert_eq!(paint.at(Point { x: 560.0 + 391.0, y: 260.0 }), near, "inside the stop");
        assert_eq!(paint.at(Point { x: 560.0, y: 260.0 + 181.0 }), near, "70% of 260 is 182");
        // aspect 1 is the circle it always was
        let circle = Gradient::radial(near, far).radius(0.0, 100.0).resolve(frame);
        match circle {
            GradientPaint::Radial { aspect, .. } => assert_eq!(aspect, 1.0),
            other => panic!("a radial, not {other:?}"),
        }
    }

    const CHECK_PATH: &[crate::icon::Verb] = &[
        crate::icon::Verb::Move(4.0, 12.0),
        crate::icon::Verb::Line(10.0, 18.0),
        crate::icon::Verb::Line(20.0, 6.0),
    ];
    const CHECK_GLYPH: crate::icon::Glyph = crate::icon::Glyph {
        draws: &[crate::icon::Draw {
            paint: crate::icon::Paint::Stroke { width: 2.0 },
            path: CHECK_PATH, tint: None,
        }],
    };
    const CHECK: crate::icon::Symbol = crate::icon::Symbol::new("test.check", &CHECK_GLYPH);

    fn icon_node(resizable: bool) -> LayoutNode {
        LayoutNode::Icon { symbol: CHECK, resizable }
    }

    #[test]
    fn an_icon_measures_off_the_inherited_font() {
        // 13pt body × 1.25, rounded to the whole point: sixteen — the
        // number the house wrote by hand before the symbol existed
        assert_eq!(
            layout(&icon_node(false), Proposal::unspecified()).size,
            Size { width: 16.0, height: 16.0 }
        );
        // a font patch on the way down moves it, like a character
        let big = styled(
            VisualProps {
                font: FontPatch { size: Some(20.0), ..FontPatch::default() },
                ..VisualProps::default()
            },
            icon_node(false),
        );
        assert_eq!(
            layout(&big, Proposal::unspecified()).size,
            Size { width: 25.0, height: 25.0 }
        );
        // rigid: a proposal cannot squeeze a glyph
        let squeezed = layout(
            &icon_node(false),
            Proposal { width: Some(100.0), height: Some(100.0) },
        );
        assert_eq!(squeezed.size, Size { width: 16.0, height: 16.0 });
    }

    #[test]
    fn a_resizable_icon_answers_the_frame() {
        // the file-icon idiom: .resizable().frame(w, h) is an exact box
        let node = LayoutNode::Frame {
            width: Some(24.0),
            height: Some(24.0),
            child: Box::new(icon_node(true)),
        };
        assert_eq!(
            layout(&node, Proposal::unspecified()).size,
            Size { width: 24.0, height: 24.0 }
        );
    }

    #[test]
    fn an_icon_takes_the_inherited_ink_in_its_key() {
        let ink = Color::hex(0x336699);
        let root = styled(
            VisualProps { foreground: Some(ink), ..VisualProps::default() },
            icon_node(false),
        );
        let result = layout(&root, Proposal::unspecified());
        let sources: Vec<ImageSource> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Image { source, .. } => Some(source.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(sources.len(), 1);
        // the tint rides the identity — the damage diff sees an ink
        // flip without ever looking at pixels
        assert_eq!(sources[0].key(), ImageSource::symbol(CHECK, ink).key());
        assert_ne!(sources[0].key(), ImageSource::symbol(CHECK, Color::hex(0x000000)).key());
    }

    #[test]
    fn an_icon_paints_the_largest_centred_square() {
        let node = LayoutNode::Frame {
            width: Some(40.0),
            height: Some(20.0),
            child: Box::new(icon_node(true)),
        };
        let result = layout(&node, Proposal::unspecified());
        let rect = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("the glyph paints");
        // the browser's centred meet, in our own paint
        assert_eq!(
            rect,
            Rect {
                origin: Point { x: 10.0, y: 0.0 },
                size: Size { width: 20.0, height: 20.0 }
            }
        );
    }

    #[test]
    fn clipped_reads_the_radius_already_on_the_box() {
        // .corner_radius + .clipped fuse into ONE node — the cut takes
        // the radius without being handed it, in either order
        let dressed = styled(
            VisualProps { clip: true, corner_radius: Some(Corners::all(6.0)), ..VisualProps::default() },
            text(3),
        );
        let result = layout(&dressed, Proposal::unspecified());
        let cut = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::PushClip { rect, corner_radius } => Some((*rect, *corner_radius)),
                _ => None,
            })
            .expect("the cut is pushed");
        assert_eq!(cut.1, Corners::all(6.0));
        let pops = result
            .display
            .iter()
            .filter(|command| matches!(command, DrawCommand::PopClip))
            .count();
        assert_eq!(pops, 1, "balanced by construction");
        // without a radius: the plain rect cut of always
        let plain = styled(VisualProps { clip: true, ..VisualProps::default() }, text(3));
        let result = layout(&plain, Proposal::unspecified());
        assert!(result.display.iter().any(|command| matches!(
            command,
            DrawCommand::PushClip { corner_radius, .. } if corner_radius.is_zero()
        )));
    }

    /// The pain the front came to kill, end to end: a bordered rounded
    /// island whose child paints its own background — the child's
    /// corner dies at the curve, the border paints OVER the cut child.
    #[cfg(feature = "canvas")]
    #[test]
    fn a_box_finally_holds_its_children() {
        let island = styled(
            VisualProps {
                background: Some(Color::hex(0xF0F0F0)),
                border: Some((Color::BLACK, 1.0)),
                corner_radius: Some(Corners::all(6.0)),
                clip: true,
                ..VisualProps::default()
            },
            styled(
                VisualProps { background: Some(Color::hex(0xAA2211)), ..VisualProps::default() },
                text(3),
            ),
        );
        let result = layout(&island, Proposal { width: Some(40.0), height: Some(24.0) });
        // paint order: island fill, PUSH, child fill, POP, border
        let kinds: Vec<&str> = result
            .display
            .iter()
            .map(|command| match command {
                DrawCommand::FillRect { .. } => "fill",
                DrawCommand::PushClip { .. } => "push",
                DrawCommand::PopClip => "pop",
                DrawCommand::StrokeRect { .. } => "stroke",
                _ => "other",
            })
            .filter(|kind| *kind != "other")
            .collect();
        assert_eq!(kinds, vec!["fill", "push", "fill", "pop", "stroke"]);
        // and the pixels agree: the child corner is CUT, the island
        // border survives on top
        let bitmap = crate::raster::rasterize(&result.display, 40, 24, Color::WHITE);
        let white = 0xFFFF_FFFF_u32;
        assert_eq!(bitmap.pixel(0, 0), Some(white), "the notch stays canvas");
        let child = 0xAA22_11FF_u32;
        assert_eq!(bitmap.pixel(20, 12), Some(child), "the child body paints");
    }
}
