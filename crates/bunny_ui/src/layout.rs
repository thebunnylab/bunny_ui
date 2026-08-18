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
    /// The pass's frame state — consulted BY PATH during placement.
    pub stamp: FrameStamp<'a>,
    /// The frame's animator — `None` in bare layouts (tests, direct
    /// [`layout`]): animated props then paint their targets.
    pub animator: Option<&'a std::cell::RefCell<crate::anim::Animator>>,
    /// The animation scope opened by the nearest `Animated` ancestor.
    pub anim: Option<AnimScope<'a>>,
    /// Where overlays may live. `None` = the pass's viewport (web,
    /// headless); the mac shell sets the SCREEN in layout coordinates —
    /// a popover then overflows the window by plain geometry, and the
    /// whole policy stays testable headless.
    pub overlay_bounds: Option<Rect>,
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
    Layered { align: CrossAlign, children: Vec<LayoutNode> },
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
    /// ONE-line text field — semantic end to end (in Dom it becomes an
    /// `<input>`; in Gpu, chrome + text + caret + selection from here).
    /// Focus, caret, selection and IME composition do NOT live here:
    /// placement consults the env's [`FrameStamp`] by `path` — the tree
    /// never carries frame state.
    Field {
        path: String,
        content: Arc<str>,
        placeholder: Arc<str>,
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
    /// Reference to a retained boundary (skipped by the reconciler);
    /// measure and place resolve ON-THE-FLY against the retention — the
    /// frame's tree is never stitched into a copy.
    BoundaryRef { path: String },
    /// `.rendering(Gpu)`: this subtree insists on the pixel pipeline.
    /// Transparent to geometry everywhere; in Dom mode it becomes a
    /// CANVAS ISLAND — an element our layout positions, filled with the
    /// subtree's own draw commands. On pixel targets it dissolves:
    /// everything is the pixel pipeline there already.
    Island { child: Box<LayoutNode> },
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
    DropTarget { accepts: std::any::TypeId, action: DropAction, child: Box<LayoutNode> },
    /// The escape hatch (`custom(…)` / `canvas(…)`): a box the APP
    /// measures and paints, in the same command vocabulary the built-ins
    /// emit. `path` is its identity — the address of the events it
    /// answers. On the element lowering it becomes a canvas island by
    /// construction: what the app paints is PIXELS, never elements.
    Custom { path: String, element: crate::custom::Custom },
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
    /// Inherited downward: the text below paints with the current foreground.
    pub foreground: Option<Color>,
    pub border: Option<(Color, Px)>,
    pub corner_radius: Option<Px>,
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
    /// A soft halo behind the view: `(radius, color)`. The falloff is
    /// quadratic; the halo paints OUTSIDE the shape and follows the
    /// corner radius — including the notch behind a rounded corner,
    /// which belongs to the shadow, not to the backdrop.
    pub shadow: Option<(Px, Color)>,
    /// `.clipped()` — the subtree cannot paint outside this box, and
    /// the cut FOLLOWS `.corner_radius(…)` when there is one. Paint
    /// only, like everything here: the measure never hears about it.
    pub clip: bool,
}

impl VisualProps {
    /// Merge of modifiers stacked on the same view: what is already set
    /// (CLOSEST to the view) wins; the outer one only fills what is
    /// missing.
    pub fn or(self, outer: VisualProps) -> VisualProps {
        VisualProps {
            background: self.background.or(outer.background),
            gradient: self.gradient.or(outer.gradient),
            clip: self.clip || outer.clip,
            foreground: self.foreground.or(outer.foreground),
            border: self.border.or(outer.border),
            corner_radius: self.corner_radius.or(outer.corner_radius),
            background_hovered: self.background_hovered.or(outer.background_hovered),
            background_pressed: self.background_pressed.or(outer.background_pressed),
            foreground_hovered: self.foreground_hovered.or(outer.foreground_hovered),
            foreground_pressed: self.foreground_pressed.or(outer.foreground_pressed),
            font: self.font.or(outer.font),
            shadow: self.shadow.or(outer.shadow),
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
    /// The app's box that owns the pointer: a press inside it keeps
    /// every move until the release, even outside the frame (dragging
    /// a selection out of the box and back is one gesture).
    pub element_grab: Option<String>,
    /// The tooltip the runtime decided to SHOW — resolved before
    /// layout like everything here (the delay is the shell's clock,
    /// never the scene's). The placement turns it into an overlay.
    pub tooltip: Option<(Arc<str>, Side, Rect)>,
    /// The open context menu — the runtime's, resolved before layout.
    pub menu: Option<MenuOpen>,
    /// The live drag — the runtime's, resolved before layout.
    pub drag: Option<DragLive>,
}

/// A draw command — the output of the placement pass, in paint order
/// (whoever comes later paints on top; `Layered` counts on that).
/// It is the rasterizer's interface and, later on, any backend's.
#[derive(Clone, PartialEq, Debug)]
pub enum DrawCommand {
    /// `corner_radius: 0.0` = plain rectangle (the usual straight path).
    FillRect { rect: Rect, color: Color, corner_radius: Px },
    /// A two-stop ramp inside the rounded rect — the same shape a
    /// `FillRect` covers, with the color resolved per pixel.
    Gradient { rect: Rect, paint: GradientPaint, corner_radius: Px },
    /// A soft halo OUTSIDE the rounded rect: alpha falls off
    /// quadratically from the edge over `radius` px. `corner_radius`
    /// makes the halo follow the corners — including the little notch
    /// BEHIND a rounded corner, which belongs to the shadow, not to the
    /// backdrop.
    Shadow { rect: Rect, radius: Px, color: Color, corner_radius: Px },
    /// A border painted INWARD from the edge, `width` logical px —
    /// it follows `corner_radius` around the corners (an anti-aliased
    /// ring; `0.0` = the four straight bars).
    StrokeRect { rect: Rect, color: Color, width: Px, corner_radius: Px },
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
    PushClip { rect: Rect, corner_radius: Px },
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
}

/// The outputs of the placement pass: frames by identity (tests), the
/// draw list (rasterizer/backends) and the interaction targets (in paint
/// order — hit-testing scans back to front, the top one wins).
/// A placed scroll region — the wheel's map. Regions enter
/// child-before-parent: the innermost one under the point decides first.
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
pub struct DropAction(pub std::rc::Rc<dyn Fn(&dyn std::any::Any)>);

impl std::fmt::Debug for DropAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("on_drop")
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

/// One `.on_drop(…)` region of the placed scene.
#[derive(Clone)]
pub struct DropRegion {
    pub accepts: std::any::TypeId,
    pub action: DropAction,
    pub rect: Rect,
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
    pub text_origin: Point,
    pub font: FontSpec,
    /// The field asked for focus on first appearance.
    pub auto_focus: bool,
}

/// A placed split — the geometry the runtime needs to route a divider
/// drag back into layout coordinates (mirror of [`FieldPlacement`]).
#[derive(Clone, Debug)]
pub struct SplitPlacement {
    pub path: String,
    pub frame: Rect,
    pub axis: Axis,
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
    /// The font the box inherited — the metrics an event resolves with.
    pub font: FontSpec,
    pub element: crate::custom::Custom,
}

/// The grip band's thickness over a split divider, in points.
pub const SPLIT_GRIP: Px = 6.0;

#[derive(Default, Debug)]
pub struct Placement {
    pub frames: Frames,
    pub display: DisplayList,
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
    /// Overlays queued by `Anchored` during the walk — drained AFTER
    /// the root places (an empty scene never allocates).
    overlay_queue: Vec<QueuedOverlay>,
    /// The placed overlays, in paint order (last = topmost).
    pub overlays: Vec<OverlayPlacement>,
    /// Window-drag regions (clipped) — where a press with no
    /// interactive target drags the window on the desktop shell.
    pub drag_regions: Vec<Rect>,
    /// Tooltip regions in paint order (last = topmost) — what the
    /// runtime's hover consults. Never a hit: a tooltip explains,
    /// it does not intercept.
    pub tooltips: Vec<TooltipRegion>,
    /// Context-menu regions in paint order (last = topmost) — what a
    /// right press consults.
    pub menus: Vec<MenuRegion>,
    /// Drag sources in paint order — what a press arms.
    pub drag_sources: Vec<DragSourceRegion>,
    /// Drop targets in paint order — what a live drag consults, by
    /// geometry, through every hover gate.
    pub drops: Vec<DropRegion>,
    /// The Dom capture, when that mode is on ([`layout_dom`]) — the
    /// placement braços feed it the SEMANTIC scene while they walk.
    /// `None` costs one branch per hook and nothing else.
    pub(crate) dom: Option<crate::dom::DomCapture>,
}

impl Placement {
    /// The command carries this node's OWN box; the stack keeps the
    /// intersection, because a hit consults the stack. Snapping and
    /// intersecting commute (round is monotone), so the consumers'
    /// own stacks land on the same integers the old pre-intersected
    /// command did — byte for byte.
    fn push_clip(&mut self, rect: Rect, corner_radius: Px) {
        let clipped = match self.clip.last() {
            Some(top) => rect
                .intersection(*top)
                .unwrap_or(Rect { origin: rect.origin, size: Size::default() }),
            None => rect,
        };
        self.display.push(DrawCommand::PushClip { rect, corner_radius });
        self.clip.push(clipped);
    }

    fn pop_clip(&mut self) {
        self.display.push(DrawCommand::PopClip);
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
    /// The placed popovers, in paint order (last = topmost) — each one
    /// a suffix slice of `display`.
    pub overlays: Vec<OverlayPlacement>,
    /// Window-drag regions — a press here with no interactive target
    /// drags the window on the desktop shell.
    pub drag_regions: Vec<Rect>,
    /// Tooltip regions in paint order (last = topmost) — what the
    /// runtime's hover consults. Never a hit: a tooltip explains,
    /// it does not intercept.
    pub tooltips: Vec<TooltipRegion>,
    /// Context-menu regions in paint order (last = topmost) — what a
    /// right press consults.
    pub menus: Vec<MenuRegion>,
    /// Drag sources in paint order — what a press arms.
    pub drag_sources: Vec<DragSourceRegion>,
    /// Drop targets in paint order — what a live drag consults, by
    /// geometry, through every hover gate.
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
            stamp: FrameStamp::idle(&interaction, &carets),
            animator: None,
            anim: None,
            overlay_bounds: None,
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
        overlays: out.overlays,
        drag_regions: out.drag_regions,
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
) -> (LayoutResult, crate::dom::DomNode) {
    let (size, fit) = root.measure(proposal, env);
    let mut out = Placement {
        dom: Some(crate::dom::DomCapture::new(size)),
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
            overlays: out.overlays,
            drag_regions: out.drag_regions,
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
                        corner_radius: Some(4.0),
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
            corner_radius: Some(7.0),
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
            corner_radius: Some(5.0),
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
    fn is_flexible(&self, axis: Axis) -> bool {
        match self {
            LayoutNode::Spacer | LayoutNode::Fill => true,
            // a split FILLS the offer on both axes — its whole job is
            // dividing the room it was given
            LayoutNode::Split { .. } => true,
            LayoutNode::Scroll { .. } => axis == Axis::Vertical,
            // a field takes the offered width (like the real TextField)
            LayoutNode::Field { .. } => axis == Axis::Horizontal,
            LayoutNode::MaxFrame { max_width, max_height, child, .. } => match axis {
                Axis::Horizontal => max_width.is_infinite() || child.is_flexible(axis),
                Axis::Vertical => max_height.is_infinite() || child.is_flexible(axis),
            },
            LayoutNode::Frame { width, height, child } => match axis {
                Axis::Horizontal => width.is_none() && child.is_flexible(axis),
                Axis::Vertical => height.is_none() && child.is_flexible(axis),
            },
            LayoutNode::Padding { child, .. }
            | LayoutNode::Interactive { child, .. }
            | LayoutNode::Styled { child, .. }
            | LayoutNode::Animated { child, .. }
            | LayoutNode::Island { child }
            | LayoutNode::Anchored { child, .. }
            | LayoutNode::DragRegion { child }
            | LayoutNode::Tooltip { child, .. }
            | LayoutNode::ContextSource { child, .. }
            | LayoutNode::DragSource { child, .. }
            | LayoutNode::DropTarget { child, .. } => child.is_flexible(axis),
            // a stack that HOLDS something flexible is itself flexible
            // (a panel with a scroll inside wants the leftover space —
            // nesting it must not freeze it at its natural extent)
            LayoutNode::Stack { children, .. } | LayoutNode::Layered { children, .. } => {
                children.iter().any(|child| child.is_flexible(axis))
            }
            LayoutNode::Boundary { children, .. } => {
                children.len() == 1 && children[0].is_flexible(axis)
            }
            // the app answers for its own box (the default is yes, the
            // same answer a Rectangle gives)
            LayoutNode::Custom { element, .. } => element.element().flexible(),
            // skipped boundary: the flexibility is the retained tree's
            LayoutNode::BoundaryRef { path } => crate::reconciler::with_retained_layout(
                path,
                |layout| layout.map(|node| node.is_flexible(axis)).unwrap_or(false),
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
                let env = LayoutEnv { font: props.font.apply_over(env.font), ..env };
                child.first_baseline(env)
            }
            LayoutNode::Animated { child, .. }
            | LayoutNode::Island { child }
            | LayoutNode::Interactive { child, .. }
            | LayoutNode::Anchored { child, .. }
            | LayoutNode::DragRegion { child }
            | LayoutNode::Tooltip { child, .. }
            | LayoutNode::ContextSource { child, .. }
            | LayoutNode::DragSource { child, .. }
            | LayoutNode::DropTarget { child, .. }
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
                let line_h = metrics.height();
                // REAL word wrapping, with the engine's measurements —
                // the width goes into the cache key (probe mode);
                // truncation turns wrapping off: one line, always
                let size = match proposal.width {
                    Some(width) if width > 0.0 && width < natural => {
                        if truncation.is_some() {
                            Size { width, height: line_h }
                        } else {
                            let lines =
                                env.cache.get_or_break(content, &env.font, width, env.text);
                            Size { width, height: lines.len() as Px * line_h }
                        }
                    }
                    _ => Size { width: natural, height: line_h },
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

            LayoutNode::Field { content, placeholder, .. } => {
                let sample: &str = if content.is_empty() { placeholder } else { content };
                let metrics = env.cache.get_or_measure(sample, &env.font, env.text);
                let natural = metrics.width + 2.0 * FIELD_PAD_H;
                let size = Size {
                    width: proposal.width.unwrap_or(natural),
                    height: metrics.height() + 2.0 * FIELD_PAD_V,
                };
                (size, Fit::Leaf)
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

            LayoutNode::Layered { children, .. } => {
                let measured: Vec<(Size, Fit)> =
                    children.iter().map(|child| child.measure(proposal, env)).collect();
                let size = measured.iter().fold(Size::default(), |acc, (size, _)| Size {
                    width: acc.width.max(size.width),
                    height: acc.height.max(size.height),
                });
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

            LayoutNode::Scroll { child, .. } => {
                let (content, fit) = child.measure(
                    Proposal {
                        width: proposal.width,
                        height: None,
                    },
                    env,
                );
                let size = Size {
                    width: proposal.width.unwrap_or(content.width),
                    height: proposal.height.unwrap_or(content.height),
                };
                (size, Fit::ScrollContent(content, Box::new(fit)))
            }

            LayoutNode::Split { axis, at, min_a, min_b, children, .. } => {
                measure_split(*axis, *at, *min_a, *min_b, children, proposal, env)
            }

            LayoutNode::Interactive { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            LayoutNode::Styled { props, child } => {
                // the inherited font swaps HERE, at measure time — the
                // sanctioned VisualProps exception (font changes measure)
                let env = LayoutEnv { font: props.font.apply_over(env.font), ..env };
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            // the animation scope never touches geometry — by type
            LayoutNode::Animated { child, .. } => {
                let (size, fit) = child.measure(proposal, env);
                (size, Fit::Wrapped(size, Box::new(fit)))
            }

            // the island claims a renderer, never a pixel of geometry
            LayoutNode::Island { child } => {
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
                out.display.push(DrawCommand::FillRect {
                    rect: frame,
                    color: Color::FILL,
                    corner_radius: 0.0,
                });
            }

            (LayoutNode::Field { path, content, placeholder, auto_focus }, Fit::Leaf) => {
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
                            }),
                            frame,
                            crate::dom::DomStyle {
                                background: Some(theme.field),
                                border: Some((theme.field_border, 1.0)),
                                corner_radius: Some(FIELD_RADIUS),
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
                out.display.push(DrawCommand::FillRect {
                    rect: frame,
                    color: theme.field,
                    corner_radius: FIELD_RADIUS,
                });
                let text_origin = Point {
                    x: frame.origin.x + FIELD_PAD_H,
                    y: frame.origin.y + FIELD_PAD_V,
                };
                let metrics = env.cache.get_or_measure(
                    if content.is_empty() { placeholder } else { content },
                    &env.font,
                    env.text,
                );
                let prefix_width = |end: usize| {
                    env.cache.get_or_measure(&content[..end], &env.font, env.text).width
                };
                // selection behind the text
                if let Some((start, end)) = selection {
                    let x0 = text_origin.x + prefix_width(start);
                    let x1 = text_origin.x + prefix_width(end);
                    out.display.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x: x0, y: text_origin.y },
                            size: Size { width: x1 - x0, height: metrics.height() },
                        },
                        color: theme.selection,
                        corner_radius: 0.0,
                    });
                }
                if content.is_empty() {
                    if !placeholder.is_empty() {
                        // the placeholder walks the SAME path as the real
                        // text: same origin, same font, only the color
                        // drops
                        out.display.push(DrawCommand::TextLine {
                            origin: text_origin,
                            content: placeholder.clone(),
                            range: (0, placeholder.len()),
                            color: theme.placeholder,
                            font: env.font,
                        });
                    }
                } else {
                    let color = out.foreground.last().copied().unwrap_or_else(|| crate::theme::current().fg);
                    out.display.push(DrawCommand::TextLine {
                        origin: text_origin,
                        content: content.clone(),
                        range: (0, content.len()),
                        color,
                        font: env.font,
                    });
                }
                // the live composition gets the IME underline (the
                // caret's ink — the composition's visual pair)
                if let Some((start, end)) = marked {
                    let x0 = text_origin.x + prefix_width(start);
                    let x1 = text_origin.x + prefix_width(end);
                    out.display.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x: x0, y: text_origin.y + metrics.height() - 1.0 },
                            size: Size { width: x1 - x0, height: 1.0 },
                        },
                        color: theme.caret,
                        corner_radius: 0.0,
                    });
                }
                // caret on top (the blink alternates via the stamp)
                if let Some(caret) = caret {
                    let x = text_origin.x + prefix_width(caret);
                    out.display.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x, y: text_origin.y },
                            size: Size { width: FIELD_CARET_W, height: metrics.height() },
                        },
                        color: theme.caret,
                        corner_radius: FIELD_CARET_W / 2.0,
                    });
                }
                out.display.push(DrawCommand::StrokeRect {
                    rect: frame,
                    color: if focused { theme.focus } else { theme.field_border },
                    width: 1.0,
                    corner_radius: FIELD_RADIUS,
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
                    text_origin,
                    font: env.font,
                    auto_focus: *auto_focus,
                });
            }

            (LayoutNode::Leaf { .. }, Fit::Leaf) => {
                if let Some(dom) = out.dom.as_mut() {
                    dom.open(crate::dom::DomKind::Box, frame, frame.origin);
                    dom.set_border(Color::OUTLINE, 1.0);
                    dom.close();
                }
                out.display.push(DrawCommand::StrokeRect {
                    rect: frame,
                    color: Color::OUTLINE,
                    width: 1.0,
                    corner_radius: 0.0,
                });
            }

            (LayoutNode::Custom { path, element }, Fit::Leaf) => {
                // the box answers for the whole frame: a hit here never
                // falls through to what is painted underneath, and the
                // event finds the element by this path
                if !path.is_empty() {
                    let visible = out.current_clip().map_or(Some(frame), |clip| {
                        frame.intersection(clip)
                    });
                    if let Some(visible) = visible {
                        out.hits.push((path.clone(), visible));
                        out.customs.push(CustomPlacement {
                            path: path.clone(),
                            frame,
                            font: env.font,
                            element: element.clone(),
                        });
                    }
                }
                // what the app paints is PIXELS: on the element lowering
                // the box becomes a canvas island, and the island slices
                // exactly the commands between here and the close
                let start = out.display.len();
                if let Some(dom) = out.dom.as_mut() {
                    dom.open_canvas(frame, start);
                }
                // the box cannot paint outside itself — the clip is the
                // framework's, never the app's promise
                out.push_clip(frame, 0.0);
                let visible = out
                    .current_clip()
                    .and_then(|clip| clip.intersection(frame))
                    .map_or(Rect { origin: Point::ZERO, size: Size::default() }, |clip| Rect {
                        origin: Point {
                            x: clip.origin.x - frame.origin.x,
                            y: clip.origin.y - frame.origin.y,
                        },
                        size: clip.size,
                    });
                let focused = env.stamp.focus == Some(path.as_str()) && !path.is_empty();
                let ctx = crate::custom::PaintCtx {
                    frame,
                    visible,
                    metrics: crate::custom::Metrics::new(env.text, env.cache, env.font),
                    focused,
                    caret_visible: focused && env.stamp.caret_visible,
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

            (LayoutNode::Tooltip { text, side, child }, Fit::Wrapped(_, fit)) => {
                if let Some(dom) = out.dom.as_mut() {
                    // in element mode the browser owns the wait and the
                    // bubble — the text lands as a data attribute on
                    // the child's own element
                    dom.arm_tooltip(text.clone());
                }
                child.place(frame, *fit, env, out);
                // clipped like a hit: what is not visible explains nothing
                let region = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(rect) = region {
                    out.tooltips.push(TooltipRegion { text: text.clone(), side: *side, rect });
                }
            }

            (LayoutNode::ContextSource { items, child }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
                let region = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(rect) = region {
                    out.menus.push(MenuRegion { items: items.clone(), rect });
                }
            }

            (LayoutNode::DragSource { payload, child }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
                let region = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(rect) = region {
                    out.drag_sources.push(DragSourceRegion { payload: payload.clone(), rect });
                }
            }

            (LayoutNode::DropTarget { accepts, action, child }, Fit::Wrapped(_, fit)) => {
                child.place(frame, *fit, env, out);
                let region = match out.current_clip() {
                    Some(clip) => frame.intersection(clip),
                    None => Some(frame),
                };
                if let Some(rect) = region {
                    // a compatible drag over THIS box: the framework
                    // rings it — the drop focus every platform draws
                    let ringed = env
                        .stamp
                        .interaction
                        .drag
                        .as_ref()
                        .is_some_and(|live| live.over == Some(rect));
                    if ringed {
                        out.display.push(DrawCommand::StrokeRect {
                            rect: frame,
                            color: crate::theme::current().accent,
                            width: 2.0,
                            corner_radius: 6.0,
                        });
                    }
                    out.drops.push(DropRegion {
                        accepts: *accepts,
                        action: action.clone(),
                        rect,
                    });
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
                    out.display.push(DrawCommand::StrokeRect {
                        rect: frame,
                        color: Color::OUTLINE,
                        width: 1.0,
                        corner_radius: 0.0,
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
                            out.display.push(DrawCommand::Image {
                                rect,
                                source: source.clone(),
                            });
                            out.pop_clip();
                        }
                    } else if frame.size.width > 0.0 && frame.size.height > 0.0 {
                        out.display.push(DrawCommand::Image {
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
                    out.display.push(DrawCommand::Image {
                        rect,
                        source: ImageSource::symbol(*symbol, color),
                    });
                }
            }

            (LayoutNode::Stack { axis, spacing, align, children }, Fit::Children(fits)) => {
                place_stack(*axis, *spacing, *align, children, frame, fits, env, out);
            }

            (
                LayoutNode::Split { path, axis, min_a, min_b, children, .. },
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
                    min_a: *min_a,
                    min_b: *min_b,
                });
            }

            (LayoutNode::Layered { align, children }, Fit::Children(fits)) => {
                for (child, (size, fit)) in children.iter().zip(fits) {
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

            (LayoutNode::Scroll { path, target, child }, Fit::ScrollContent(content, fit)) => {
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
                if max_y > 0.0 {
                    draw_scrollbar(frame, content.height, offset.y, max_y, out);
                }
                out.pop_clip();
                if let Some(path) = path {
                    // after the child: inner regions come EARLIER in the vec
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
            }

            (LayoutNode::Styled { props, child }, Fit::Wrapped(_, fit)) => {
                // the nearest styled EATS the color scope: its colors
                // move, deeper styled nodes paint plain (no shared-key
                // thrash between siblings of one scope)
                let colors = env.anim.filter(|scope| scope.colors);
                let env = LayoutEnv {
                    font: props.font.apply_over(env.font),
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
                // the halo goes first — everything else paints over it
                if let Some((radius, color)) = props.shadow {
                    out.display.push(DrawCommand::Shadow {
                        rect: frame,
                        radius,
                        color: animated(crate::anim::Channel::Shadow, color),
                        corner_radius: props.corner_radius.unwrap_or(0.0),
                    });
                }
                let (hovered, pressed) = out.pointer.last().copied().unwrap_or((false, false));
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
                    out.display.push(DrawCommand::FillRect {
                        rect: frame,
                        color: animated(crate::anim::Channel::Background, color),
                        corner_radius: props.corner_radius.unwrap_or(0.0),
                    });
                }
                // the ramp paints OVER the flat color and under the
                // child: the two compose, and the geometry resolves to
                // px here — the shaders only evaluate
                if let Some(gradient) = props.gradient {
                    out.display.push(DrawCommand::Gradient {
                        rect: frame,
                        paint: gradient.resolve(frame),
                        corner_radius: props.corner_radius.unwrap_or(0.0),
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
                    out.push_clip(frame, props.corner_radius.unwrap_or(0.0));
                }
                child.place(frame, *fit, env, out);
                if props.clip {
                    out.pop_clip();
                }
                if ink.is_some() {
                    out.foreground.pop();
                }
                if let Some((color, width)) = props.border {
                    out.display.push(DrawCommand::StrokeRect {
                        rect: frame,
                        color: animated(crate::anim::Channel::Border, color),
                        width,
                        corner_radius: props.corner_radius.unwrap_or(0.0),
                    });
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

            (LayoutNode::Island { child }, Fit::Wrapped(_, fit)) => {
                // Dom mode: a canvas element in the flow, filled with
                // the subtree's OWN draw commands (the display range
                // between open and close). Pixel targets place through:
                // everything is the pixel pipeline there already.
                if out.dom.is_some() {
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
                        crate::dom::DomKind::Group { path: path.clone() },
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
fn measure_split(
    axis: Axis,
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
            let a = at.clamp(min_a, (total - thickness - min_b).max(min_a));
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
        .filter(|(_, child)| child.is_flexible(axis))
        .map(|(index, _)| index)
        .collect();

    // phase 2: only the flexibles re-measure — the waterfall. Offer equal
    // shares; a child that takes less than its offer is satisfied and its
    // leftover re-splits among the rest, until a round leaves nothing.
    if let Some(total) = proposed_main
        && !flexible.is_empty()
    {
        // Under-consumption within half a device pixel is arithmetic, not
        // appetite — it must not spin another round.
        const SETTLED: Px = 0.5;
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

    let main_sum: Px = measured.iter().map(|(size, _)| main(size)).sum::<Px>() + spacing_total;
    let cross_max: Px = measured
        .iter()
        .map(|(size, _)| cross(size))
        .fold(0.0, Px::max);

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

    if metrics.width <= frame.size.width {
        emit_text_runs(content, (0, content.len()), highlights, frame.origin, base_color, env, out);
        return;
    }
    if let Some(mode) = truncation {
        // highlight does not survive the ellipsis (the original's ranges
        // do not map onto the composed text) — honest v1, noted
        let composed: Arc<str> = Arc::from(truncate_to_width(content, mode, frame.size.width, env));
        let length = composed.len();
        out.display.push(DrawCommand::TextLine {
            origin: frame.origin,
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
            Point { x: frame.origin.x, y: frame.origin.y + line_index as Px * line_h },
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
        out.display.push(whole());
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
        out.display.push(whole());
        return;
    }

    for (start, end, hot) in segments {
        let offset = if start == line_start {
            0.0
        } else {
            env.cache.get_or_measure(&content[line_start..start], &env.font, env.text).width
        };
        out.display.push(DrawCommand::TextLine {
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
const SCROLLBAR_INSET: Px = 6.0;
const SCROLLBAR_MIN: Px = 24.0;

const FIELD_PAD_H: Px = 8.0;
const FIELD_PAD_V: Px = 5.0;
const FIELD_RADIUS: Px = 5.0;
const FIELD_CARET_W: Px = 1.5;

/// The region's thumb — draw-only at this stage (drag arrives with
/// pointer capture): 4px wide at 6px from the right edge, track with
/// inset 6, floor of 24, proportional to the viewport — and it only
/// exists when there is overflow (short content never gets a bar).
fn draw_scrollbar(frame: Rect, content_h: Px, offset_y: Px, max_y: Px, out: &mut Placement) {
    let track = frame.size.height - 2.0 * SCROLLBAR_INSET;
    if track <= 0.0 {
        return;
    }
    let thumb_h = ((frame.size.height / content_h) * track).max(SCROLLBAR_MIN).min(track);
    let travel = track - thumb_h;
    let thumb_y = frame.origin.y + SCROLLBAR_INSET + travel * (offset_y / max_y);
    out.display.push(DrawCommand::FillRect {
        rect: Rect {
            origin: Point {
                x: frame.origin.x + frame.size.width - SCROLLBAR_INSET - SCROLLBAR_W,
                y: thumb_y,
            },
            size: Size { width: SCROLLBAR_W, height: thumb_h },
        },
        color: crate::theme::current().scrollbar,
        corner_radius: SCROLLBAR_W / 2.0,
    });
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
    fn a_split_lays_exact_lanes_and_offers_a_grip() {
        let split = |at: f64| LayoutNode::Split {
            path: "seam".into(),
            axis: Axis::Horizontal,
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
            stamp: FrameStamp::idle(&interaction, &carets),
            animator: None,
            anim: None,
            overlay_bounds: None,
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
                stamp: FrameStamp::idle(interaction, &carets),
                animator: None,
                anim: None,
                overlay_bounds: None,
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
            stamp: FrameStamp::idle(&interaction, &carets),
            animator: None,
            anim: None,
            overlay_bounds: None,
        };

        let root = LayoutNode::Scroll {
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
                corner_radius: Some(4.0),
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
                if *color == Color::hex(0x112233) && *corner_radius == 4.0
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
                    corner_radius: Some(8.0),
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
            VisualProps { clip: true, corner_radius: Some(6.0), ..VisualProps::default() },
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
        assert_eq!(cut.1, 6.0);
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
            DrawCommand::PushClip { corner_radius, .. } if *corner_radius == 0.0
        )));
    }

    /// The pain the front came to kill, end to end: a bordered rounded
    /// island whose child paints its own background — the child's
    /// corner dies at the curve, the border paints OVER the cut child.
    #[test]
    fn a_box_finally_holds_its_children() {
        let island = styled(
            VisualProps {
                background: Some(Color::hex(0xF0F0F0)),
                border: Some((Color::BLACK, 1.0)),
                corner_radius: Some(6.0),
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
