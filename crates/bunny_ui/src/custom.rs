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
//! text lines, images, clips — so nothing forks: the desktop rasterizes
//! it on the GPU, the web canvas mode rasterizes it on the CPU, the web
//! element mode turns the box into a canvas island, and the damage diff
//! keeps working because the commands compare by value.
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
use crate::layout::{Color, DisplayList, DrawCommand, LayoutNode, Point, Proposal, Px, Rect, Size};
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
    fn measure(&self, proposal: Proposal, metrics: &Metrics) -> Size {
        let _ = metrics;
        Size {
            width: proposal.width.unwrap_or(0.0),
            height: proposal.height.unwrap_or(0.0),
        }
    }

    /// Does the box want the leftover space of the stack that holds it?
    /// The default is yes — the same answer a `Rectangle` gives. A
    /// `.frame(…)` around it always wins.
    fn flexible(&self) -> bool {
        true
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
    pub fn fill_rounded(&mut self, rect: Rect, color: Color, corner_radius: Px) {
        self.display.push(DrawCommand::FillRect {
            rect: self.shift(rect),
            color,
            corner_radius,
        });
    }

    /// A border painted INWARD from the edge.
    pub fn stroke(&mut self, rect: Rect, color: Color, width: Px, corner_radius: Px) {
        self.display.push(DrawCommand::StrokeRect {
            rect: self.shift(rect),
            color,
            width,
            corner_radius,
        });
    }

    /// A soft halo outside the rect — the quadratic falloff of
    /// `.shadow()`.
    pub fn shadow(&mut self, rect: Rect, radius: Px, color: Color, corner_radius: Px) {
        self.display.push(DrawCommand::Shadow {
            rect: self.shift(rect),
            radius,
            color,
            corner_radius,
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

    /// Everything `body` paints is cut to `rect` — balanced by
    /// construction, so a clip can never leak out of the box.
    pub fn clipped(&mut self, rect: Rect, body: impl FnOnce(&mut Painter)) {
        self.display.push(DrawCommand::PushClip { rect: self.shift(rect) });
        body(self);
        self.display.push(DrawCommand::PopClip);
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
    use crate::view::Component;
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
                Some(DrawCommand::PushClip { rect }) if *rect == Rect {
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

    /// Lays a node out at an exact proposal.
    fn layout_root(root: &LayoutNode, width: Px, height: Px) -> crate::layout::LayoutResult {
        crate::layout::layout(root, Proposal { width: Some(width), height: Some(height) })
    }
}
