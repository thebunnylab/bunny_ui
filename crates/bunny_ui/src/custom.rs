//! The escape hatch: a box the app paints itself.
//!
//! Every other view in the framework is semantic — the scene says what a
//! thing IS and each target lowers it. Some content has no UI vocabulary
//! at all: a code editor, a terminal grid, a waveform, a chart. Those get
//! a box of their own:
//!
//! ```ignore
//! // just paint (the Canvas of SwiftUI)
//! canvas(|ctx, p| p.fill(Rect { origin: Point::ZERO, size: ctx.size() }, ink))
//!
//! // the full element: it measures, paints and (later) listens
//! custom(CodeSurface { document, state })
//! ```
//!
//! The app paints with the SAME vocabulary the built-ins emit — rects,
//! text lines, images, ramps, glyphs, paths and clips — so nothing
//! forks: the desktop rasterizes it on the GPU, the web canvas mode
//! rasterizes it on the CPU, the web element mode turns the box into a
//! canvas island, and the damage diff keeps working because the
//! commands compare by value.
//!
//! **When NOT to use it.** The hatch is for content outside the
//! vocabulary, never a shortcut around a missing modifier. A rounded
//! background, a hover state or a gradient belongs in the framework —
//! ask for it and the built-ins grow. What you paint here is invisible
//! to the element lowering (it becomes pixels), so a screen that could
//! have been views loses the browser's text selection, its accessibility
//! and its zero-patch hover.

use std::rc::Rc;
use std::sync::Arc;

use motor::state::Context;
use motor::view::RenderNode;

use crate::image_engine::ImageSource;
use crate::layout::{
    Color, Corners, DisplayList, DrawCommand, LayoutNode, Point, Proposal, Px, Rect, Size,
};
use crate::text_engine::{FontSpec, LineMetrics, MeasureCache, TextEngine};
use crate::view::{NodeList, Single, View};

/// What the app implements to own a box.
///
/// Only [`paint`] is required: the default measure fills the proposal
/// (the box behaves like a `Rectangle`) and the default name is what the
/// printed tree shows.
///
/// The element is a SNAPSHOT of what the body captured when it rendered.
/// A retained subtree keeps painting from the element it was built with
/// — the same contract the retained click actions have: state changes,
/// the body runs again, a new element arrives.
///
/// [`paint`]: CustomElement::paint
pub trait CustomElement: 'static {
    /// Paints the box, in LOCAL coordinates: the origin is the box's own
    /// top-left corner. Everything outside the box is clipped away by
    /// construction — and `ctx.visible` says which part of it the clip
    /// stack lets through, so a long document paints one screen.
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter);

    /// The answer to the parent's proposal. The default takes what was
    /// proposed (and zero on an axis the parent left open).
    ///
    /// An axis the parent left OPEN is a question: *how much do you
    /// hold?* Inside a scroll region, both are the same question — and
    /// a box that answers the extent of its CONTENT gets the whole
    /// region for free: the thumb (draggable), the wheel, the travel,
    /// the clamps, and [`CustomElement::reveal`]. It never has to paint
    /// more than a screen, because `ctx.visible` says which screen; on
    /// the element lowering the island is that screen too, never the
    /// content.
    ///
    /// ```ignore
    /// fn measure(&self, proposal: Proposal, metrics: &Metrics) -> Size {
    ///     Size {
    ///         width: proposal.width.unwrap_or(0.0),
    ///         // the document, not the window
    ///         height: self.lines as f64 * metrics.line_height(),
    ///     }
    /// }
    /// ```
    fn measure(&self, proposal: Proposal, metrics: &Metrics) -> Size {
        let _ = metrics;
        Size {
            width: proposal.width.unwrap_or(0.0),
            height: proposal.height.unwrap_or(0.0),
        }
    }

    /// One event, in LOCAL coordinates. The default ignores everything:
    /// a box that only paints answers nothing, and what it ignores goes
    /// back to the scene (an ignored wheel scrolls the region around
    /// it).
    ///
    /// A press on the box takes the POINTER until the release: the
    /// moves keep arriving even when the pointer leaves the frame,
    /// which is what dragging a selection needs.
    fn event(&self, event: &ElementEvent, ctx: &EventCtx) -> Response {
        let _ = (event, ctx);
        Response::ignored()
    }

    /// Does the box want the leftover space of the stack that holds it?
    /// The default is yes — the same answer a `Rectangle` gives. A
    /// `.frame(…)` around it always wins.
    fn flexible(&self) -> bool {
        true
    }

    /// Does the box take the keyboard? A `true` here makes a click
    /// FOCUS it: the strokes, the typed text and the composition
    /// arrive as events, and the caret blink runs for it.
    fn accepts_keys(&self) -> bool {
        false
    }

    /// What the platform's input system sees while the box is focused —
    /// the text around the caret, the selection, the live composition
    /// and the caret rect in LOCAL coordinates. `None` = no composition
    /// is offered (the box still types).
    ///
    /// A document answers with the CONTEXT it wants the input system to
    /// know, not with its whole content: the current line is a good
    /// answer, as long as the indices below agree with it.
    fn ime(&self, metrics: &Metrics) -> Option<ImeContext> {
        let _ = metrics;
        None
    }

    /// The UTF-16 index under a LOCAL point — the input system asks
    /// this to look a word up under the mouse.
    fn ime_index_at(&self, local: Point, metrics: &Metrics) -> Option<usize> {
        let _ = (local, metrics);
        None
    }

    /// The caret-shaped rect at a UTF-16 index of [`CustomElement::ime`],
    /// in LOCAL coordinates — where the candidate window lands.
    fn ime_rect_for(&self, utf16: usize, metrics: &Metrics) -> Option<Rect> {
        let _ = (utf16, metrics);
        None
    }

    /// A rect, in LOCAL coordinates, that the box wants BROUGHT INTO
    /// VIEW — the caret of a document, the selected cell of a grid.
    /// The enclosing scroll region travels the shortest distance that
    /// shows it, exactly as `.scroll_target(id)` does for a row, and
    /// only when the answer CHANGES: the wheel is never fought.
    ///
    /// It is the other half of the contract a scrolling box signs. On
    /// an OPEN axis, [`CustomElement::measure`] answers the extent of
    /// the CONTENT (not of the window), and the framework's region then
    /// owns the whole affair — the thumb, the wheel, the travel, and
    /// this. The box keeps painting one screen: `ctx.visible` says
    /// which one.
    fn reveal(&self) -> Option<Rect> {
        None
    }

    /// The name in the printed tree: `Custom(editor)`.
    fn name(&self) -> &str {
        "custom"
    }
}

/// The app's element inside the layout tree — a cheap handle (one `Rc`
/// clone) that keeps the tree `Debug` without asking the app for it.
#[derive(Clone)]
pub struct Custom(Rc<dyn CustomElement>);

impl Custom {
    pub fn new(element: impl CustomElement) -> Custom {
        Custom(Rc::new(element))
    }

    pub fn element(&self) -> &dyn CustomElement {
        &*self.0
    }
}

impl std::fmt::Debug for Custom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "custom({})", self.0.name())
    }
}

// MARK: - What the app reads while it measures and paints

/// Text measurement for the app's own layout — the frame's engine, with
/// the same cache the built-ins use (a repeated line costs a lookup).
#[derive(Clone, Copy)]
pub struct Metrics<'a> {
    text: &'a dyn TextEngine,
    cache: &'a MeasureCache,
    /// The font this box inherited from the scope above it.
    pub font: FontSpec,
}

impl<'a> Metrics<'a> {
    pub(crate) fn new(
        text: &'a dyn TextEngine,
        cache: &'a MeasureCache,
        font: FontSpec,
    ) -> Metrics<'a> {
        Metrics { text, cache, font }
    }

    /// One line measured with the inherited font.
    pub fn line(&self, text: &str) -> LineMetrics {
        self.cache.get_or_measure(text, &self.font, self.text)
    }

    /// One line measured with another font (a bold keyword, a small
    /// gutter number).
    pub fn line_in(&self, text: &str, font: &FontSpec) -> LineMetrics {
        self.cache.get_or_measure(text, font, self.text)
    }

    /// The width of one line with the inherited font.
    pub fn width(&self, text: &str) -> Px {
        self.line(text).width
    }

    /// The height of one line of the inherited font — the row step of a
    /// document.
    pub fn line_height(&self) -> Px {
        self.line("0").height()
    }
}

/// The box's state while it paints.
pub struct PaintCtx<'a> {
    /// The box, in LAYOUT coordinates — its size is what measure
    /// answered, its origin is where the parent put it.
    pub frame: Rect,
    /// What the clip stack lets through, in LOCAL coordinates: paint
    /// this and nothing else. An empty rect means the box is off screen.
    pub visible: Rect,
    /// Text measurement, cached.
    pub metrics: Metrics<'a>,
    /// Does the box hold the keyboard right now?
    pub focused: bool,
    /// The blink phase the caret follows — the box paints its own
    /// caret, the runtime only says when it shows.
    pub caret_visible: bool,
    /// Where the enclosing `.looping(...)` clock is in its cycle
    /// (0..1), snapped onto the loop's step grid. Zero outside a loop.
    /// The paint must be a pure function of it — the geometry the
    /// measure answered never depends on the phase.
    pub phase: f64,
    /// How many PHYSICAL pixels one point covers on this screen — `2.0`
    /// on a retina display, `1.0` everywhere the shell says nothing.
    ///
    /// A box that draws parts which TOUCH needs it. Two neighbor bands
    /// share one edge: if that edge falls in the middle of a pixel,
    /// both sides cover half of it and a translucent color blends
    /// twice — a darker thread down the seam. [`Self::snap`] puts the
    /// edge on a whole pixel and the thread goes away.
    pub scale: Px,
}

impl PaintCtx<'_> {
    /// The box's size — the usual start of a paint.
    pub fn size(&self) -> Size {
        self.frame.size
    }

    /// The whole box in LOCAL coordinates — what a background fills.
    pub fn bounds(&self) -> Rect {
        Rect { origin: Point::ZERO, size: self.frame.size }
    }

    /// Moves one length onto the screen's pixel grid — the cure for the
    /// seam between two parts that touch.
    ///
    /// ```ignore
    /// let split = ctx.snap(row.y + row.height);
    /// painter.fill(Rect::new(x, top, w, split - top), tint);
    /// painter.fill(Rect::new(x, split, w, bottom - split), tint);
    /// ```
    ///
    /// Both bands now end and start on the SAME whole pixel, so the
    /// tint covers it once.
    pub fn snap(&self, value: Px) -> Px {
        (value * self.scale).round() / self.scale
    }

    /// [`Self::snap`] on the four edges of a box — origin and far edge,
    /// never origin and size (a snapped size added to a snapped origin
    /// lands off the grid again).
    pub fn snap_rect(&self, rect: Rect) -> Rect {
        let x = self.snap(rect.origin.x);
        let y = self.snap(rect.origin.y);
        Rect {
            origin: Point { x, y },
            size: Size {
                width: self.snap(rect.origin.x + rect.size.width) - x,
                height: self.snap(rect.origin.y + rect.size.height) - y,
            },
        }
    }
}

/// The paint vocabulary, in LOCAL coordinates.
///
/// Every call becomes one of the same draw commands the built-in views
/// emit, so the app inherits the whole pipeline: anti-aliased corners,
/// the glyph atlas, the damage diff, the GPU and CPU parity.
pub struct Painter<'a> {
    display: &'a mut DisplayList,
    origin: Point,
    font: FontSpec,
    ink: Color,
}

impl<'a> Painter<'a> {
    pub(crate) fn new(
        display: &'a mut DisplayList,
        origin: Point,
        font: FontSpec,
        ink: Color,
    ) -> Painter<'a> {
        Painter { display, origin, font, ink }
    }

    fn shift(&self, rect: Rect) -> Rect {
        Rect { origin: self.at(rect.origin), size: rect.size }
    }

    fn at(&self, point: Point) -> Point {
        Point { x: point.x + self.origin.x, y: point.y + self.origin.y }
    }

    /// The font the box inherited — the default of the text calls.
    pub fn font(&self) -> FontSpec {
        self.font
    }

    /// The foreground the box inherited: `.foreground_color(…)` above it
    /// reaches the app's own text, the way it reaches a `text(…)`.
    pub fn ink(&self) -> Color {
        self.ink
    }

    /// A plain rectangle.
    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.fill_rounded(rect, color, 0.0);
    }

    /// A rectangle with rounded corners (anti-aliased, like every
    /// background in the framework).
    ///
    /// One number rounds all four; a [`Corners`] rounds the ones it
    /// names — `Corners::top(4.0)` for the first band of a figure that
    /// continues below.
    pub fn fill_rounded(
        &mut self,
        rect: Rect,
        color: Color,
        corner_radius: impl Into<Corners>,
    ) {
        self.display.push(DrawCommand::FillRect {
            rect: self.shift(rect),
            color,
            corner_radius: corner_radius.into(),
        });
    }

    /// A border painted INWARD from the edge.
    pub fn stroke(
        &mut self,
        rect: Rect,
        color: Color,
        width: Px,
        corner_radius: impl Into<Corners>,
    ) {
        self.display.push(DrawCommand::StrokeRect {
            rect: self.shift(rect),
            color,
            width,
            corner_radius: corner_radius.into(),
        });
    }

    /// A soft halo outside the rect — the quadratic falloff of
    /// `.shadow()`.
    pub fn shadow(
        &mut self,
        rect: Rect,
        radius: Px,
        color: Color,
        corner_radius: impl Into<Corners>,
    ) {
        self.display.push(DrawCommand::Shadow {
            rect: self.shift(rect),
            radius,
            color,
            corner_radius: corner_radius.into(),
        });
    }

    /// One line of text with the inherited font. `origin` is the TOP-left
    /// of the line box; the app breaks its own lines (this paints what it
    /// gets, with no wrapping).
    pub fn text(&mut self, origin: Point, content: impl Into<Arc<str>>, color: Color) {
        let font = self.font;
        self.text_in(origin, content, color, font);
    }

    /// [`Painter::text`] with another font.
    pub fn text_in(
        &mut self,
        origin: Point,
        content: impl Into<Arc<str>>,
        color: Color,
        font: FontSpec,
    ) {
        let content = content.into();
        let range = (0, content.len());
        self.text_slice(origin, content, range, color, font);
    }

    /// A SLICE of one string — how a syntax-colored line paints without
    /// allocating a `String` per token: the whole line arrives once and
    /// every token names its byte range.
    pub fn text_slice(
        &mut self,
        origin: Point,
        content: Arc<str>,
        range: (usize, usize),
        color: Color,
        font: FontSpec,
    ) {
        if range.0 >= range.1 {
            return;
        }
        self.display.push(DrawCommand::TextLine {
            origin: self.at(origin),
            content,
            range,
            color,
            font,
        });
    }

    /// One image, at the platform's own decode of the destination size.
    pub fn image(&mut self, rect: Rect, source: ImageSource) {
        self.display.push(DrawCommand::Image { rect: self.shift(rect), source });
    }

    /// A two-stop ramp inside the rounded rect — the same value
    /// `.background_gradient(…)` takes, resolved against THIS rect.
    /// The declaration is proportional, so the ramp an app paints in
    /// its box matches the one the framework paints on a background.
    pub fn gradient(&mut self, rect: Rect, gradient: crate::layout::Gradient, corner_radius: impl Into<Corners>) {
        let shifted = self.shift(rect);
        self.display.push(DrawCommand::Gradient {
            rect: shifted,
            paint: gradient.resolve(shifted),
            corner_radius: corner_radius.into(),
        });
    }

    /// One vector glyph, tinted, on the largest CENTRED square of
    /// `rect` — the same bytes the built-in `icon(…)` paints. Pass
    /// `painter.ink()` to take the inherited ink.
    pub fn icon(&mut self, rect: Rect, symbol: crate::icon::Symbol, color: Color) {
        let side = rect.size.width.min(rect.size.height);
        if side <= 0.0 {
            return;
        }
        let square = Rect {
            origin: Point {
                x: rect.origin.x + (rect.size.width - side) / 2.0,
                y: rect.origin.y + (rect.size.height - side) / 2.0,
            },
            size: Size { width: side, height: side },
        };
        self.display.push(DrawCommand::Image {
            rect: self.shift(square),
            source: ImageSource::symbol(symbol, color),
        });
    }

    /// A path the app builds THIS frame, in LOCAL points — the door
    /// for geometry that comes from data and can never be a table: the
    /// squiggle under a misspelled word, the curved lane of a commit
    /// graph, the connector of a diagram, the line of a sparkline.
    ///
    /// The verbs are the ones a glyph carries ([`Verb::Move`],
    /// `Line`, `Quad`, `Cubic`, `Close`) and the paint is the same
    /// [`Paint`]: a fill under one of the two winding rules, or a pen
    /// of a given width with ROUND caps and joins. The house
    /// rasterizes it, so the CPU compositor, the GPU atlas and the
    /// browser canvas consume literally the same pixels.
    ///
    /// The ink is a `Color` or a [`Gradient`] — the same ramp a box
    /// paints behind itself, declared in the PATH's own proportions, so
    /// it survives every size the mark is drawn at. The house resolves
    /// and samples it while it rasterizes, which is why a ramped path
    /// asks nothing new of any rendering:
    ///
    /// ```ignore
    /// painter.path(&verbs, Paint::Stroke { width: 3.0 }, Color::WHITE);
    /// painter.path(
    ///     &verbs,
    ///     Paint::Fill(Rule::NonZero),
    ///     Gradient::linear(dawn, dusk)
    ///         .direction(UnitPoint::TOP_LEADING, UnitPoint::BOTTOM_TRAILING),
    /// );
    /// ```
    ///
    /// The cost is a raster: a table that changes every frame pays one
    /// every frame (and, on the GPU, one upload). The ink is part of
    /// the identity, so a mark repainted through another ramp is
    /// another tile. Build paths from data that moves at human speed,
    /// and a warm cache answers.
    ///
    /// [`Verb::Move`]: crate::icon::Verb::Move
    /// [`Paint`]: crate::icon::Paint
    /// [`Gradient`]: crate::layout::Gradient
    pub fn path(
        &mut self,
        verbs: &[crate::icon::Verb],
        paint: crate::icon::Paint,
        ink: impl Into<crate::icon::Ink>,
    ) {
        let Some((min_x, min_y, max_x, max_y)) = crate::icon::bounds(verbs) else {
            return;
        };
        // the box grows by half a pen (the ink rides OUTSIDE the
        // contour) plus one point for the anti-aliased edge
        let pad = match paint {
            crate::icon::Paint::Stroke { width } => width as f64 / 2.0 + 1.0,
            crate::icon::Paint::Fill(_) => 1.0,
        };
        let origin = Point { x: min_x - pad, y: min_y - pad };
        let size = Size {
            width: (max_x - min_x) + 2.0 * pad,
            height: (max_y - min_y) + 2.0 * pad,
        };
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let local = crate::icon::shifted(verbs, -origin.x as f32, -origin.y as f32);
        let source = ImageSource::path(
            local,
            paint,
            ink.into(),
            (size.width as f32, size.height as f32),
        );
        self.display.push(DrawCommand::Image { rect: self.shift(Rect { origin, size }), source });
    }

    /// Everything `body` paints is cut to `rect` — balanced by
    /// construction, so a clip can never leak out of the box.
    pub fn clipped(&mut self, rect: Rect, body: impl FnOnce(&mut Painter)) {
        self.clipped_rounded(rect, 0.0, body);
    }

    /// [`Painter::clipped`] with a corner — the same cut `.clipped()`
    /// gives a box, for an app that draws its own.
    pub fn clipped_rounded(
        &mut self,
        rect: Rect,
        corner_radius: impl Into<Corners>,
        body: impl FnOnce(&mut Painter),
    ) {
        self.display.push(DrawCommand::PushClip {
            rect: self.shift(rect),
            corner_radius: corner_radius.into(),
        });
        body(self);
        self.display.push(DrawCommand::PopClip);
    }
}

// MARK: - What reaches the box

/// One event for the app's box, in LOCAL coordinates: the origin is the
/// box's own top-left corner.
///
/// The list grows as the shells learn to say more — match with a `_`
/// arm.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ElementEvent {
    /// The pointer moved. `pressed` = the box owns the drag (the press
    /// started here), so the point can be outside the frame.
    PointerMoved { at: Point, pressed: bool },
    /// `clicks` is the platform's own count — 2 selects a word, 3 a
    /// line, and the box never needs a clock of its own.
    PointerDown { at: Point, clicks: u8 },
    PointerUp { at: Point },
    /// The wheel turned over the box. Ignore it and the scroll region
    /// around the box takes the turn instead.
    Wheel { at: Point, dx: Px, dy: Px },
    /// The pointer left the box (or the window).
    PointerExited,
    /// A keystroke, while the box has focus — arrows, Enter, Tab and
    /// the shortcuts, exactly as the keymap spells them. What the box
    /// ignores goes on to the app's key bindings.
    Key(crate::action::KeyPattern),
    /// Text to insert: typing, a paste, or the commit of a
    /// composition. The framework opens no clipboard — the shell reads
    /// it and the text arrives here.
    Text(String),
    /// A live composition: the marked text replaces the previous mark
    /// (or the selection) and stays MARKED — underlined, not
    /// committed. `caret_utf16` is (location, length) INSIDE the marked
    /// text, the platform's own vocabulary.
    Marked { text: String, caret_utf16: (usize, usize) },
    /// The composition ended: what is marked stands as typed.
    Unmark,
    /// Answer with [`Response::text`] and the shell writes the
    /// clipboard.
    Copy,
    /// The same, and the box removes what it handed over.
    Cut,
    /// The box took the keyboard (or lost it).
    Focused(bool),
}

/// What the platform's input system reads from a focused box.
///
/// The indices are UTF-16 — the vocabulary of the input systems — and
/// they index [`ImeContext::text`], which is the editing CONTEXT the
/// box chose to expose.
#[derive(Clone, Debug, PartialEq)]
pub struct ImeContext {
    pub text: String,
    /// (location, length) of the selection, in UTF-16.
    pub selected: (usize, usize),
    /// The marked range while a composition is live.
    pub marked: Option<(usize, usize)>,
    /// The caret, in LOCAL coordinates.
    pub caret_rect: Rect,
}

/// What the app answers.
///
/// `handled` decides whether the event stops here; `text` is what a
/// copy or a cut hands back to the platform's clipboard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    pub handled: bool,
    pub text: Option<String>,
    /// Handled AND the gesture still rises: the enclosing pane's
    /// `.on_click` fires, a card can still select. The box keeps its
    /// caret; the ancestors keep their affordances.
    pub rises: bool,
}

impl Response {
    /// The box used the event: it stops here.
    pub fn handled() -> Response {
        Response { handled: true, text: None, rises: false }
    }

    /// The box used the event — and lets it RISE: the nearest
    /// interactive ancestor still arms and can fire on the release.
    /// A press that positions a caret while the pane takes focus is
    /// one click, not a stolen one.
    pub fn handled_rising() -> Response {
        Response { handled: true, text: None, rises: true }
    }

    /// The box did not use it: the scene takes over.
    pub fn ignored() -> Response {
        Response::default()
    }

    /// Handled, with text for the clipboard.
    pub fn text(text: impl Into<String>) -> Response {
        Response { handled: true, text: Some(text.into()), rises: false }
    }
}

/// What the app reads while it answers an event.
pub struct EventCtx<'a> {
    /// The box, in LAYOUT coordinates — its size is what measure
    /// answered.
    pub frame: Rect,
    /// What the clip stack lets through, in LOCAL coordinates — the
    /// SAME window the paint was given. A box that declared its
    /// content extent reads its viewport here: the origin is how far
    /// the region has travelled, the size is what fits, and a page key
    /// needs nothing else.
    pub visible: Rect,
    /// Text measurement, cached: how a click becomes a column.
    pub metrics: Metrics<'a>,
}

impl EventCtx<'_> {
    pub fn size(&self) -> Size {
        self.frame.size
    }
}

// MARK: - The two doors

/// The view a [`CustomElement`] enters the scene as.
#[derive(Clone)]
pub struct CustomView {
    element: Custom,
}

impl View for CustomView {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(if crate::view::print_enabled() {
            format!("Custom({})", self.element.element().name())
        } else {
            String::new()
        }));
        // outside a pass (a decorative render) there is no identity to
        // key events on — the box still paints, it just answers nothing
        let path = motor::identity::cursor_scope().unwrap_or_default();
        if !path.is_empty() {
            // the registration says "this box is on screen": a focused
            // hatch keeps the keyboard while it is, and loses it the
            // pass it leaves (the same truth the fields live by)
            crate::reconciler::attribute_custom(
                path.clone(),
                self.element.element().accepts_keys(),
            );
        }
        out.push_layout(LayoutNode::Custom { path, element: self.element.clone() });
    }
}

/// A box the app owns: it measures, paints and (with the trait's other
/// answers) listens.
///
/// ```ignore
/// custom(CodeSurface { document: doc.clone(), state })
/// ```
pub fn custom(element: impl CustomElement) -> CustomView {
    CustomView { element: Custom::new(element) }
}

/// A box the app only PAINTS — the short door, for a chart, a sparkline,
/// a badge no modifier can express. It fills what the parent proposes;
/// `.frame(…)` pins it.
///
/// ```ignore
/// canvas(|ctx, p| {
///     let size = ctx.size();
///     p.fill_rounded(Rect { origin: Point::ZERO, size }, ink, 6.0);
/// })
/// ```
pub fn canvas(paint: impl Fn(&PaintCtx, &mut Painter) + 'static) -> CustomView {
    custom(Painting(paint))
}

/// The closure adapter behind [`canvas`].
struct Painting<F>(F);

impl<F: Fn(&PaintCtx, &mut Painter) + 'static> CustomElement for Painting<F> {
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        (self.0)(ctx, painter);
    }

    fn name(&self) -> &str {
        "canvas"
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::layout::{Axis, CrossAlign, DrawCommand, LayoutNode, Proposal};
    use crate::runtime::Runtime;
    use crate::text_input::EditCommand;
    use crate::state_ext::StateExt;
    use crate::view::{Component, Either};
    use motor::state::State;
    use motor::state::Context as ViewContext;

    /// A surface that fills itself and remembers what it was told.
    struct Bar {
        color: Color,
        seen: Rc<Cell<Option<(Rect, FontSpec, Color)>>>,
    }

    impl Bar {
        fn new(color: Color) -> (Bar, Rc<Cell<Option<(Rect, FontSpec, Color)>>>) {
            let seen = Rc::new(Cell::new(None));
            (Bar { color, seen: Rc::clone(&seen) }, seen)
        }
    }

    impl CustomElement for Bar {
        fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
            self.seen.set(Some((ctx.visible, painter.font(), painter.ink())));
            painter.fill(ctx.bounds(), self.color);
        }

        fn name(&self) -> &str {
            "bar"
        }
    }

    fn node(element: impl CustomElement) -> LayoutNode {
        LayoutNode::Custom { path: "surface".into(), element: Custom::new(element) }
    }

    fn fills(display: &crate::layout::DisplayList) -> Vec<Rect> {
        display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_surface_takes_the_room_it_is_offered() {
        let (bar, _) = Bar::new(Color::FILL);
        let result = layout_root(&node(bar), 200.0, 90.0);
        assert_eq!(result.size, Size { width: 200.0, height: 90.0 });
        assert_eq!(
            fills(&result.display),
            vec![Rect {
                origin: Point::ZERO,
                size: Size { width: 200.0, height: 90.0 }
            }],
            "the default measure answers the whole proposal"
        );
    }

    #[test]
    fn a_surface_measures_itself_when_it_wants_to() {
        // a document knows its own extent: lines times the row step
        struct Document;
        impl CustomElement for Document {
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
            fn measure(&self, proposal: Proposal, metrics: &Metrics) -> Size {
                Size {
                    width: proposal.width.unwrap_or(0.0),
                    height: metrics.line_height() * 12.0,
                }
            }
        }
        let result = layout_root(&node(Document), 300.0, 900.0);
        assert_eq!(result.size.height, crate::layout::LINE_H * 12.0);
    }

    #[test]
    fn the_paint_lands_where_the_parent_put_the_box() {
        // local coordinates go in, layout coordinates come out
        let (bar, _) = Bar::new(Color::FILL);
        let stack = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                LayoutNode::Frame {
                    width: None,
                    height: Some(30.0),
                    child: Box::new(LayoutNode::Spacer),
                },
                node(bar),
            ],
        };
        let result = layout_root(&stack, 120.0, 100.0);
        assert_eq!(fills(&result.display)[0].origin, Point { x: 0.0, y: 30.0 });
    }

    #[test]
    fn a_surface_cannot_paint_outside_its_box() {
        // whatever the app draws, the clip pair around it is the
        // framework's — an overflowing element damages nobody
        struct Overflow;
        impl CustomElement for Overflow {
            fn paint(&self, _ctx: &PaintCtx, painter: &mut Painter) {
                painter.fill(
                    Rect {
                        origin: Point { x: -50.0, y: -50.0 },
                        size: Size { width: 500.0, height: 500.0 },
                    },
                    Color::FILL,
                );
            }
        }
        let result = layout_root(&node(Overflow), 80.0, 40.0);
        let commands: Vec<_> = result.display.iter().cloned().collect();
        assert!(
            matches!(
                commands.first(),
                Some(DrawCommand::PushClip { rect, .. }) if *rect == Rect {
                    origin: Point::ZERO,
                    size: Size { width: 80.0, height: 40.0 },
                }
            ),
            "the box clips itself: {commands:?}"
        );
        assert!(
            matches!(commands.last(), Some(DrawCommand::PopClip)),
            "and closes what it opened: {commands:?}"
        );
    }

    #[test]
    fn the_visible_rect_is_what_the_clip_lets_through() {
        // a tall surface inside a scroll region hears how much of it
        // shows — the contract that keeps a long document cheap
        let (bar, seen) = Bar::new(Color::FILL);
        let scroll = LayoutNode::Scroll {
            axes: crate::layout::ScrollAxes::Vertical,
            path: None,
            target: None,
            child: Box::new(LayoutNode::Frame {
                width: None,
                height: Some(400.0),
                child: Box::new(node(bar)),
            }),
        };
        layout_root(&scroll, 100.0, 60.0);
        let (visible, _, _) = seen.get().expect("the surface painted");
        assert_eq!(visible.origin, Point::ZERO);
        assert_eq!(visible.size.height, 60.0, "one viewport of the 400");
    }

    #[test]
    fn the_box_inherits_the_ink_and_the_font() {
        // `.foreground_color` and `.font_size` above the box reach the
        // app's own text, exactly as they reach a text(…)
        #[derive(Clone)]
        struct Screen {
            seen: Rc<Cell<Option<(Rect, FontSpec, Color)>>>,
            color: Color,
        }
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                use crate::ext::ViewExt;
                custom(Bar { color: Color::FILL, seen: self.seen })
                    .foreground_color(self.color)
                    .font_size(9.0)
            }
        }
        let seen = Rc::new(Cell::new(None));
        let ink = Color::hex(0x3B82F6);
        let runtime = Runtime::new();
        let view = Screen { seen: Rc::clone(&seen), color: ink };
        runtime.layout(&view, Proposal { width: Some(50.0), height: Some(20.0) });
        let (_, font, painted) = seen.get().expect("the surface painted");
        assert_eq!(font.size, 9.0);
        assert_eq!(painted, ink);
    }

    #[test]
    fn the_printed_tree_names_the_element() {
        #[derive(Clone)]
        struct Screen;
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                canvas(|ctx, painter| painter.fill(ctx.bounds(), Color::FILL))
            }
        }
        let runtime = Runtime::new();
        assert!(
            runtime.render(&Screen).contains("Custom(canvas)"),
            "{}",
            runtime.render(&Screen)
        );
    }

    #[test]
    fn a_surface_lowers_to_one_canvas_island() {
        // the element mode cannot express what the app paints: the box
        // becomes a canvas, and the subtree under it is PIXELS
        #[derive(Clone)]
        struct Screen;
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                use crate::ext::ViewExt;
                canvas(|ctx, painter| painter.fill(ctx.bounds(), Color::hex(0x3B82F6)))
                    .frame(40.0, 20.0)
            }
        }
        let runtime = Runtime::new();
        let size = Size { width: 60.0, height: 40.0 };
        let mount = runtime.dom_frame(&Screen, size);
        let canvases = mount
            .iter()
            .filter(|patch| {
                matches!(
                    patch,
                    crate::dom::DomPatch::Create { kind: crate::dom::CreateKind::Canvas, .. }
                )
            })
            .count();
        assert_eq!(canvases, 1, "one surface, one canvas: {mount:?}");
        let islands = runtime.dom_islands(1);
        assert_eq!(islands.len(), 1);
        assert!(
            islands[0].rgba.chunks_exact(4).any(|pixel| pixel[3] > 0),
            "the island carries the app's ink"
        );
        // a second frame with nothing changed asks for no pixels
        assert!(runtime.dom_frame(&Screen, size).is_empty());
        assert!(runtime.dom_islands(1).is_empty(), "unchanged pixels never re-raster");
    }

    /// A surface that writes down every event it is told about.
    struct Recorder {
        log: Rc<std::cell::RefCell<Vec<ElementEvent>>>,
        takes_wheel: Rc<Cell<bool>>,
    }

    impl Recorder {
        fn new(log: &Rc<std::cell::RefCell<Vec<ElementEvent>>>) -> Recorder {
            Recorder { log: Rc::clone(log), takes_wheel: Rc::new(Cell::new(false)) }
        }
    }

    impl CustomElement for Recorder {
        fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}

        fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
            self.log.borrow_mut().push(event.clone());
            match event {
                ElementEvent::Wheel { .. } if !self.takes_wheel.get() => Response::ignored(),
                _ => Response::handled(),
            }
        }
    }

    #[test]
    fn a_press_hands_the_box_the_pointer_until_the_release() {
        // the drag leaves the frame and keeps arriving: selecting text
        // past the edge is ONE gesture
        #[derive(Clone)]
        struct Screen {
            log: Rc<std::cell::RefCell<Vec<ElementEvent>>>,
        }
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                use crate::ext::ViewExt;
                custom(Recorder::new(&self.log))
                    .frame(80.0, 40.0)
                    .padding_length(10.0)
            }
        }
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        let runtime = Runtime::new();
        let view = Screen { log: Rc::clone(&log) };
        runtime.layout(&view, Proposal { width: Some(200.0), height: Some(100.0) });

        assert!(runtime.pointer_pressed(20.0, 20.0), "the press lands in the box");
        runtime.pointer_moved(300.0, 300.0);
        assert_eq!(runtime.pointer_released(300.0, 300.0), None, "no action fires under it");
        let seen = log.borrow().clone();
        assert_eq!(
            seen,
            vec![
                ElementEvent::PointerDown { at: Point { x: 10.0, y: 10.0 }, clicks: 1 },
                ElementEvent::PointerMoved {
                    at: Point { x: 290.0, y: 290.0 },
                    pressed: true
                },
                ElementEvent::PointerUp { at: Point { x: 290.0, y: 290.0 } },
            ],
            "local coordinates, and the drag survives leaving the box"
        );
    }

    #[test]
    fn a_press_on_the_box_never_reaches_what_is_under_it() {
        #[derive(Clone)]
        struct Screen {
            fired: Rc<Cell<bool>>,
        }
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                let fired = Rc::clone(&self.fired);
                crate::zstack!(
                    crate::views::button(crate::views::text("under"), move || fired.set(true)),
                    canvas(|ctx, painter| painter.fill(ctx.bounds(), Color::FILL)),
                )
            }
        }
        let fired = Rc::new(Cell::new(false));
        let runtime = Runtime::new();
        let view = Screen { fired: Rc::clone(&fired) };
        runtime.layout(&view, Proposal { width: Some(120.0), height: Some(60.0) });
        runtime.pointer_pressed(60.0, 30.0);
        runtime.pointer_released(60.0, 30.0);
        assert!(!fired.get(), "the box owns its own frame");
    }

    #[test]
    fn an_ignored_wheel_falls_through_to_the_region() {
        #[derive(Clone)]
        struct Scrolled {
            log: Rc<std::cell::RefCell<Vec<ElementEvent>>>,
            takes_wheel: Rc<Cell<bool>>,
        }
        impl Component for Scrolled {
            fn body(self, _ctx: &ViewContext) -> impl View {
                use crate::ext::ViewExt;
                let (log, takes_wheel) = (self.log, self.takes_wheel);
                crate::views::list(
                    vec![0usize],
                    |row| row.to_string(),
                    move |_| {
                        custom(Recorder {
                            log: Rc::clone(&log),
                            takes_wheel: Rc::clone(&takes_wheel),
                        })
                        .frame(60.0, 400.0)
                    },
                )
            }
        }
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        let takes_wheel = Rc::new(Cell::new(false));
        let runtime = Runtime::new();
        let view = Scrolled { log: Rc::clone(&log), takes_wheel: Rc::clone(&takes_wheel) };
        runtime.layout(&view, Proposal { width: Some(60.0), height: Some(80.0) });

        // the box ignores the wheel: the list around it scrolls
        assert!(runtime.wheel(30.0, 40.0, 0.0, -20.0), "the region took the turn");
        assert!(matches!(log.borrow().last(), Some(ElementEvent::Wheel { .. })));
        assert_eq!(runtime.scroll_offset("Scrolled").y, 20.0);

        // the box that takes it stops it there
        takes_wheel.set(true);
        assert!(runtime.wheel(30.0, 40.0, 0.0, -20.0));
        assert_eq!(
            runtime.scroll_offset("Scrolled").y,
            20.0,
            "what the box takes never reaches the region"
        );
    }

    #[test]
    fn a_free_move_reaches_the_box_unpressed() {
        #[derive(Clone)]
        struct Screen {
            log: Rc<std::cell::RefCell<Vec<ElementEvent>>>,
        }
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                custom(Recorder::new(&self.log))
            }
        }
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        let runtime = Runtime::new();
        let view = Screen { log: Rc::clone(&log) };
        runtime.layout(&view, Proposal { width: Some(40.0), height: Some(40.0) });
        runtime.pointer_moved(12.0, 8.0);
        assert_eq!(
            log.borrow().last(),
            Some(&ElementEvent::PointerMoved { at: Point { x: 12.0, y: 8.0 }, pressed: false })
        );
        runtime.pointer_exited();
        assert_eq!(log.borrow().last(), Some(&ElementEvent::PointerExited));
    }

    // MARK: - The keyboard

    /// A one-line editor built from the app's own parts — enough to
    /// prove the keyboard, the clipboard and the input system reach a
    /// box the framework knows nothing about.
    #[derive(Clone, Default)]
    struct MiniEditor {
        text: Rc<std::cell::RefCell<String>>,
        marked: Rc<Cell<Option<(usize, usize)>>>,
        focus_log: Rc<std::cell::RefCell<Vec<bool>>>,
        caret_shown: Rc<Cell<bool>>,
    }

    /// One monospaced column of the deterministic test font.
    const COLUMN: Px = 8.0;

    impl CustomElement for MiniEditor {
        fn paint(&self, ctx: &PaintCtx, _painter: &mut Painter) {
            self.caret_shown.set(ctx.caret_visible);
        }

        fn accepts_keys(&self) -> bool {
            true
        }

        fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
            match event {
                ElementEvent::Text(text) => {
                    self.marked.set(None);
                    self.text.borrow_mut().push_str(text);
                    Response::handled()
                }
                ElementEvent::Marked { text, .. } => {
                    let start = self.text.borrow().len();
                    self.text.borrow_mut().push_str(text);
                    self.marked.set(Some((start, start + text.len())));
                    Response::handled()
                }
                ElementEvent::Unmark => {
                    self.marked.set(None);
                    Response::handled()
                }
                ElementEvent::Copy => Response::text(self.text.borrow().clone()),
                ElementEvent::Cut => {
                    let text = self.text.replace(String::new());
                    Response::text(text)
                }
                ElementEvent::Key(pattern)
                    if pattern.key == crate::action::Key::Backspace =>
                {
                    self.text.borrow_mut().pop();
                    Response::handled()
                }
                ElementEvent::Focused(on) => {
                    self.focus_log.borrow_mut().push(*on);
                    Response::handled()
                }
                _ => Response::ignored(),
            }
        }

        fn ime(&self, _metrics: &Metrics) -> Option<ImeContext> {
            let text = self.text.borrow().clone();
            let caret = text.chars().count();
            Some(ImeContext {
                selected: (caret, 0),
                marked: self.marked.get().map(|(start, end)| (start, end - start)),
                caret_rect: Rect {
                    origin: Point { x: caret as Px * COLUMN, y: 0.0 },
                    size: Size { width: 1.5, height: 16.0 },
                },
                text,
            })
        }

        fn ime_index_at(&self, local: Point, _metrics: &Metrics) -> Option<usize> {
            Some((local.x / COLUMN).round().max(0.0) as usize)
        }

        fn ime_rect_for(&self, utf16: usize, _metrics: &Metrics) -> Option<Rect> {
            Some(Rect {
                origin: Point { x: utf16 as Px * COLUMN, y: 0.0 },
                size: Size { width: 1.5, height: 16.0 },
            })
        }
    }

    #[derive(Clone)]
    struct Editing {
        editor: MiniEditor,
    }

    impl Component for Editing {
        fn body(self, _ctx: &ViewContext) -> impl View {
            use crate::ext::ViewExt;
            crate::vstack!(
                crate::views::text("above"),
                custom(self.editor).frame(200.0, 40.0),
            )
        }
    }

    /// A focused editor at (0, 16) — the click that focused it included.
    fn focused_editor() -> (Runtime, MiniEditor) {
        let editor = MiniEditor::default();
        let runtime = Runtime::new();
        let view = Editing { editor: editor.clone() };
        runtime.layout(&view, Proposal { width: Some(200.0), height: Some(80.0) });
        runtime.pointer_pressed(10.0, 30.0);
        runtime.pointer_released(10.0, 30.0);
        (runtime, editor)
    }

    #[test]
    fn the_keyboard_reaches_the_box_that_asked_for_it() {
        let (runtime, editor) = focused_editor();
        assert_eq!(runtime.focused().as_deref(), Some("Editing/#1"));
        assert_eq!(editor.focus_log.borrow().as_slice(), &[true]);

        // typing, pasting and the commit of a composition are all Text
        assert!(runtime.key(EditCommand::Insert("let ".into())).applied);
        assert!(runtime.key(EditCommand::Insert("x".into())).applied);
        assert_eq!(&*editor.text.borrow(), "let x");

        // the strokes come through the gate's door
        assert!(
            runtime
                .key_stroke(&crate::action::KeyPattern::key(crate::action::Key::Backspace))
                .handled
        );
        assert_eq!(&*editor.text.borrow(), "let ");
    }

    #[test]
    fn one_key_has_one_door() {
        // what the box took at the gate never arrives again as an edit,
        // and what the box ignores is the keymap's
        let (runtime, editor) = focused_editor();
        assert!(!runtime.key(EditCommand::Backspace).applied, "the gate owns the strokes");
        assert!(!runtime.key(EditCommand::Left(false)).applied);
        assert!(editor.text.borrow().is_empty());

        const NEXT: crate::action::ActionId = crate::action::ActionId("test.next");
        let pattern = crate::action::KeyPattern::key(crate::action::Key::Down);
        runtime.bind(pattern, NEXT);
        assert!(!runtime.key_stroke(&pattern).handled, "the editor ignores Down");
        assert_eq!(runtime.match_key(&pattern), Some(NEXT), "so the keymap answers");
    }

    #[test]
    fn the_box_answers_the_input_system() {
        let (runtime, editor) = focused_editor();
        runtime.key(EditCommand::Insert("ab".into()));

        // a live composition, and the marked range comes back
        assert!(
            runtime
                .key(EditCommand::SetMarked {
                    text: "に".into(),
                    caret_utf16: (0, 1)
                })
                .applied
        );
        let snapshot = runtime.ime_snapshot().expect("the box answers");
        assert_eq!(snapshot.text, "abに");
        assert!(snapshot.marked.is_some(), "the composition is live");
        // the caret rect crossed from the box's coordinates into the
        // scene's: the box sits under one line of text
        assert_eq!(snapshot.caret_rect.origin, Point { x: 3.0 * COLUMN, y: crate::layout::LINE_H });
        assert!(runtime.key(EditCommand::Unmark).applied);
        assert!(runtime.ime_snapshot().unwrap().marked.is_none());

        // the questions the input system asks about geometry
        assert_eq!(runtime.ime_index_at(2.0 * COLUMN, 20.0), Some(2));
        assert_eq!(runtime.ime_index_at(2.0 * COLUMN, 400.0), None, "outside the box");
        assert_eq!(
            runtime.ime_rect_for(1).map(|rect| rect.origin),
            Some(Point { x: COLUMN, y: crate::layout::LINE_H })
        );
        assert_eq!(&*editor.text.borrow(), "abに");
    }

    #[test]
    fn a_copy_hands_the_selection_to_the_shell() {
        let (runtime, _) = focused_editor();
        runtime.key(EditCommand::Insert("copy me".into()));
        assert_eq!(runtime.key(EditCommand::Copy).output.as_deref(), Some("copy me"));
        assert_eq!(runtime.key(EditCommand::Cut).output.as_deref(), Some("copy me"));
        assert_eq!(runtime.key(EditCommand::Copy).output.as_deref(), Some(""));
    }

    #[test]
    fn the_caret_blinks_for_the_box_and_the_focus_leaves_with_a_click() {
        let editor = MiniEditor::default();
        let runtime = Runtime::new();
        let view = Editing { editor: editor.clone() };
        let proposal = Proposal { width: Some(200.0), height: Some(80.0) };
        runtime.layout(&view, proposal);
        runtime.pointer_pressed(10.0, 30.0);
        runtime.pointer_released(10.0, 30.0);

        assert!(runtime.blink(), "a focused box blinks");
        runtime.layout(&view, proposal);
        assert!(!editor.caret_shown.get(), "the box paints the phase it was told");
        assert!(runtime.blink());
        runtime.layout(&view, proposal);
        assert!(editor.caret_shown.get());

        // a click above the box (on plain text) drops the keyboard
        runtime.pointer_pressed(10.0, 5.0);
        runtime.pointer_released(10.0, 5.0);
        assert_eq!(runtime.focused(), None);
        assert_eq!(editor.focus_log.borrow().as_slice(), &[true, false]);
    }

    #[test]
    fn a_box_that_does_not_ask_never_takes_the_keyboard() {
        #[derive(Clone)]
        struct Screen {
            log: Rc<std::cell::RefCell<Vec<ElementEvent>>>,
        }
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                custom(Recorder::new(&self.log))
            }
        }
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        let runtime = Runtime::new();
        let view = Screen { log: Rc::clone(&log) };
        runtime.layout(&view, Proposal { width: Some(40.0), height: Some(40.0) });
        runtime.pointer_pressed(10.0, 10.0);
        runtime.pointer_released(10.0, 10.0);
        assert_eq!(runtime.focused(), None);
        assert!(
            !runtime
                .key_stroke(&crate::action::KeyPattern::key(crate::action::Key::Enter))
                .handled,
            "no focus, no keys"
        );
    }

    #[test]
    fn the_keyboard_survives_the_frames_that_follow_the_click() {
        // the shells settle after every event, and the settle releases
        // the input of whatever left the scene. A box the app paints
        // registers no field editor, so the sweep used to take its
        // keyboard on the very next frame — it now says it is here.
        let (runtime, editor) = focused_editor();
        let view = Editing { editor: editor.clone() };
        runtime.settle(&view);
        runtime.layout(&view, Proposal { width: Some(200.0), height: Some(80.0) });
        assert_eq!(runtime.focused().as_deref(), Some("Editing/#1"));
        assert!(runtime.key(EditCommand::Insert("still here".into())).applied);
        assert_eq!(&*editor.text.borrow(), "still here");
    }

    #[test]
    fn a_box_that_leaves_the_scene_gives_the_keyboard_back() {
        #[derive(Clone)]
        struct Screen {
            editor: MiniEditor,
            gone: State<bool>,
        }
        impl Component for Screen {
            fn body(self, _ctx: &ViewContext) -> impl View {
                use crate::ext::ViewExt;
                if self.gone.get() {
                    Either::Second(crate::views::text("the box left"))
                } else {
                    Either::First(custom(self.editor).frame(200.0, 40.0))
                }
            }
        }
        let runtime = Runtime::new();
        let view = Screen { editor: MiniEditor::default(), gone: State::new(false) };
        let proposal = Proposal { width: Some(200.0), height: Some(80.0) };
        runtime.layout(&view, proposal);
        runtime.pointer_pressed(10.0, 10.0);
        runtime.pointer_released(10.0, 10.0);
        assert!(runtime.focused().is_some());

        view.gone.set(true);
        runtime.settle(&view);
        assert_eq!(runtime.focused(), None, "the keyboard goes with the box");
    }

    /// Lays a node out at an exact proposal.
    fn layout_root(root: &LayoutNode, width: Px, height: Px) -> crate::layout::LayoutResult {
        crate::layout::layout(root, Proposal { width: Some(width), height: Some(height) })
    }

    /// The painter's ramp resolves against the SHIFTED rect: what an
    /// app declares proportionally lands where the box actually is.
    #[test]
    fn the_painter_ramp_lands_in_layout_coords() {
        struct Ramp;
        impl CustomElement for Ramp {
            fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
                painter.gradient(
                    ctx.bounds(),
                    crate::layout::Gradient::linear(Color::hex(0x000000), Color::hex(0xFFFFFF)),
                    0.0,
                );
            }
            fn name(&self) -> &str {
                "ramp"
            }
        }
        let result = crate::layout::layout(
            &crate::layout::LayoutNode::Padding {
                edges: crate::layout::Edges::uniform(10.0),
                child: Box::new(node(Ramp)),
            },
            Proposal { width: Some(120.0), height: Some(60.0) },
        );
        let (rect, paint) = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::Gradient { rect, paint, .. } => Some((*rect, *paint)),
                _ => None,
            })
            .expect("the ramp painted");
        assert_eq!(rect.origin.x, 10.0);
        assert_eq!(rect.origin.y, 10.0);
        // the line runs down the box: the resolved start sits on the
        // box's top edge, in LAYOUT coordinates
        match paint {
            crate::layout::GradientPaint::Linear { start, end, .. } => {
                assert_eq!(start.y, 10.0);
                assert_eq!(end.y, 50.0);
            }
            other => panic!("a linear ramp, not {other:?}"),
        }
    }

    /// The painter's glyph is the same bytes the built-in paints —
    /// proven by the source key alone.
    #[test]
    fn the_painter_glyph_carries_the_tinted_key() {
        const DOT_PATH: &[crate::icon::Verb] = &[
            crate::icon::Verb::Move(4.0, 12.0),
            crate::icon::Verb::Line(20.0, 12.0),
        ];
        const DOT_GLYPH: crate::icon::Glyph = crate::icon::Glyph {
            draws: &[crate::icon::Draw {
                paint: crate::icon::Paint::Stroke { width: 2.0 },
                path: DOT_PATH, tint: None,
            }],
        };
        const DOT: crate::icon::Symbol = crate::icon::Symbol::new("test.dot", &DOT_GLYPH);
        struct Badge;
        impl CustomElement for Badge {
            fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
                let ink = painter.ink();
                painter.icon(ctx.bounds(), DOT, ink);
            }
            fn name(&self) -> &str {
                "badge"
            }
        }
        let result = crate::layout::layout(
            &node(Badge),
            Proposal { width: Some(40.0), height: Some(20.0) },
        );
        let source = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::Image { source, .. } => Some(source.clone()),
                _ => None,
            })
            .expect("the glyph painted");
        let ink = crate::theme::current().fg;
        assert_eq!(
            source.key(),
            crate::image_engine::ImageSource::symbol(DOT, ink).key()
        );
    }

    // MARK: - The traced path (dor 28)

    /// A box that draws ONE curve the app assembled.
    struct Squiggle {
        verbs: Vec<crate::icon::Verb>,
        paint: crate::icon::Paint,
    }

    impl CustomElement for Squiggle {
        fn paint(&self, _ctx: &PaintCtx, painter: &mut Painter) {
            painter.path(&self.verbs, self.paint, Color::BLACK);
        }
    }

    fn images(display: &crate::layout::DisplayList) -> Vec<(Rect, crate::image_engine::ImageSource)> {
        display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Image { rect, source } => Some((*rect, source.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_traced_path_lands_in_the_boxs_own_coordinates() {
        use crate::icon::{Paint, Verb};
        // a pen of 2 across a line from (10,20) to (60,20): the box is
        // the geometry plus half a pen plus the anti-aliased point
        let element = Squiggle {
            verbs: vec![Verb::Move(10.0, 20.0), Verb::Line(60.0, 20.0)],
            paint: Paint::Stroke { width: 2.0 },
        };
        let result = layout_root(&node(element), 200.0, 90.0);
        let drawn = images(&result.display);
        assert_eq!(drawn.len(), 1);
        let (rect, _) = drawn[0];
        assert_eq!(rect.origin, Point { x: 8.0, y: 18.0 });
        assert_eq!(rect.size, Size { width: 54.0, height: 4.0 });
    }

    #[test]
    fn an_empty_table_paints_nothing() {
        use crate::icon::{Paint, Rule};
        let element = Squiggle { verbs: Vec::new(), paint: Paint::Fill(Rule::NonZero) };
        let result = layout_root(&node(element), 60.0, 40.0);
        assert!(images(&result.display).is_empty());
    }

    #[test]
    fn a_traced_path_survives_the_damage_diff() {
        use crate::icon::{Paint, Verb};
        // the identity compares by VALUE: the same table twice is the
        // same image, one moved point is a different one
        let table = |y: f32| vec![Verb::Move(4.0, y), Verb::Quad(20.0, 0.0, 36.0, y)];
        let pen = Paint::Stroke { width: 1.5 };
        let draw = |y: f32| {
            let element = Squiggle { verbs: table(y), paint: pen };
            images(&layout_root(&node(element), 60.0, 40.0).display)
        };
        assert_eq!(draw(20.0), draw(20.0));
        assert_ne!(draw(20.0), draw(21.0));
    }
}
