//! The built-in views, written the Rust way — generic end to end.
//!
//! | Swift                            | bunny_ui                          |
//! |----------------------------------|-----------------------------------|
//! | `Text("x")`                      | [`text("x")`]                     |
//! | `VStack { }`                     | [`vstack((…))`]                   |
//! | `VStack(alignment: .leading) { }`| [`vstack((…)).alignment(Leading)`]|
//! | `List(items, id: \.id) { }`      | [`list(items, id, row)`]          |
//! | `Section(header: …) { }`         | [`section(header, (…))`]          |
//! | `NavigationStack(path: $p) { }`  | [`navigation_stack(path.binding(), (…))`] |
//! | `Button(action:) { Text(…) }`    | [`button(label, action)`]         |
//! | `ForEach(items, id: \.id) { }`   | [`for_each(items, id, row)`]      |
//! | `EmptyView`                      | [`empty()`]                       |
//!
//! Children are tuples (the implicit `TupleView` of a `@ViewBuilder`
//! block), not `Vec<AnyView>` — arity at compile time, zero
//! erasure. Where Swift prints an explicit `TupleView` node, the port
//! calls [`tuple`]. Configuration goes *after* the children, in methods
//! (`.alignment(…)`, `.spacing(…)`) — the default vanishes from the
//! callsite, like Swift's omitted argument. Signature convention: content
//! first, behavior (closures) last.

use std::collections::HashSet;
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context};
use motor::view::RenderNode;
use motor::views::NavigationPath;

use crate::layout::{
    Axis, CrossAlign, Edges, Fraction, LayoutNode, SeamUnit, Size as LayoutSize, VisualProps,
};
use crate::state_ext::BindingExt;
use crate::view::{NodeList, Single, View, render_line};

/// Several layout nodes becoming ONE (composite labels, sections, explicit
/// tuples): one child passes straight through; several stack vertically.
pub(crate) fn wrap_layout(children: Vec<LayoutNode>) -> LayoutNode {
    let mut children = children;
    if children.len() == 1 {
        children.remove(0)
    } else {
        LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children,
        }
    }
}

// MARK: - Leaves

/// `Text("…")` — `Rc<str>` for cheap clones (views are values).
#[derive(Clone)]
pub struct Text(pub Arc<str>);

impl View for Text {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(if crate::view::print_enabled() {
            format!("Text({:?})", self.0)
        } else {
            String::new()
        }));
        out.push_layout(LayoutNode::Text {
            content: self.0.clone(),
            highlights: None,
            truncation: None,
        });
    }
}

/// `Text` takes anything that becomes an `Rc<str>`: a literal or a
/// `String` pay ONE allocation here, and an `Rc<str>` handed in (a row
/// model that shares its strings) pays NOTHING — the body of a list
/// clones pointers, not bytes.
pub fn text(string: impl Into<Arc<str>>) -> Text {
    Text(string.into())
}

// The button chrome geometry (Role/Size come later; the future
// reference is height 26/34/44 with text 11/13/15). The COLORS come
// from the theme, read at render — retheme rebuilds the scene.
const BUTTON_RADIUS: f64 = 6.0;
const BUTTON_PAD_H: f64 = 14.0;
const BUTTON_PAD_V: f64 = 6.0;

/// `Button(action:) { label }` — the action lives in an `F: Fn()` field,
/// called statically (there is no `Rc<dyn Fn()>` here).
#[derive(Clone)]
pub struct Button<L, F> {
    label: L,
    action: F,
}

impl<L, F> View for Button<L, F>
where
    L: View,
    F: Fn() + Clone + 'static,
{
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mut label = NodeList::new();
        self.label.render_into(ctx, &mut label);
        let (prints, layouts) = label.into_parts();
        out.push(RenderNode::branch("Button", prints));

        // the default chrome lives in the SCENE (the print stays as it was):
        // background with corners + built-in padding, hover/pressed states
        // included — the hit-rect becomes the whole chrome, not just the label
        let theme = crate::theme::current();
        let chrome = LayoutNode::Styled {
            props: Box::new(VisualProps {
                background: Some(theme.control),
                background_hovered: Some(theme.control_hovered),
                background_pressed: Some(theme.control_pressed),
                corner_radius: Some(BUTTON_RADIUS),
                ..VisualProps::default()
            }),
            child: Box::new(LayoutNode::Padding {
                edges: Edges {
                    top: BUTTON_PAD_V,
                    bottom: BUTTON_PAD_V,
                    leading: BUTTON_PAD_H,
                    trailing: BUTTON_PAD_H,
                },
                child: Box::new(wrap_layout(layouts)),
            }),
        };

        // inside a pass, the button is an interaction target: the frame joins
        // the hit-test under the identity path, and the action stays registered
        // in the reconciler (retained like the effects — skipped view, live button)
        match motor::identity::cursor_scope() {
            Some(path) => {
                let action = self.action.clone();
                crate::reconciler::attribute_action(path.clone(), Rc::new(move || action()));
                out.push_layout(LayoutNode::Interactive {
                    path,
                    child: Box::new(chrome),
                });
            }
            None => out.push_layout(chrome),
        }
    }
}

impl<L, F> Button<L, F>
where
    F: Fn() + Clone + 'static,
{
    /// Pressing the button, for the headless demo.
    pub fn tap(&self) {
        (self.action)();
    }
}

/// `Button(action:) { label }` — the label first (it is what you read), the
/// action last (long closures format better at the end, and it is the
/// convention of the whole API: content before, behavior after).
pub fn button<L, F>(label: L, action: F) -> Button<L, F>
where
    L: View,
    F: Fn() + Clone + 'static,
{
    Button { label, action }
}

/// `TextField("Placeholder", text: $binding)` — a ONE-line field. The app
/// owns the STRING (the binding); the framework owns caret, selection and
/// focus (by structural identity, like scroll). Editing arrives through a
/// closure retained in the reconciler — a skipped view's field stays editable.
#[derive(Clone)]
pub struct TextField {
    placeholder: Arc<str>,
    text: Binding<String>,
}

impl View for TextField {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        let value = self.text.wrappedValue();
        out.push(RenderNode::leaf(if crate::view::print_enabled() {
            format!("TextField({:?}, text: {:?})", self.placeholder, value)
        } else {
            String::new()
        }));
        match motor::identity::cursor_scope() {
            Some(path) => {
                let binding = self.text.clone();
                crate::reconciler::attribute_editor(
                    path.clone(),
                    Rc::new(move |command, state| {
                        let mut value = binding.wrappedValue();
                        let original = value.clone();
                        let output = crate::text_input::apply(&mut value, state, command);
                        // the set dirties whoever READS — only when the text
                        // actually changed (Read/Copy must not invalidate the world)
                        if value != original {
                            binding.set(value);
                        }
                        output
                    }),
                );
                out.push_layout(LayoutNode::Field {
                    path,
                    content: Arc::from(value),
                    placeholder: self.placeholder.clone(),
                    auto_focus: false,
                });
            }
            // outside a pass (decorative use): the value becomes plain text
            None => out.push_layout(LayoutNode::Text {
                content: if value.is_empty() {
                    self.placeholder.clone()
                } else {
                    Arc::from(value)
                },
                highlights: None,
                truncation: None,
            }),
        }
    }
}

/// What a seam can be measured in. The unit rides the BINDING's type,
/// so a seam and its floors can never disagree: `Binding<f64>` is
/// points, `Binding<Fraction>` is a share of the container.
///
/// The trait is closed in practice — the two implementations below are
/// the two units the framework knows.
pub trait SeamValue: Clone + 'static {
    /// Which arithmetic the layout runs.
    const UNIT: SeamUnit;
    /// The floor each lane gets when the app names none: a hundred
    /// points, or a tenth of the pair.
    const FLOOR: f64;
    /// The number, for the layout node and the printed tree.
    fn amount(&self) -> f64;
    /// The number, on its way back from a drag.
    fn of(amount: f64) -> Self;
}

impl SeamValue for f64 {
    const UNIT: SeamUnit = SeamUnit::Points;
    const FLOOR: f64 = 100.0;

    fn amount(&self) -> f64 {
        *self
    }

    fn of(amount: f64) -> f64 {
        amount
    }
}

impl SeamValue for Fraction {
    const UNIT: SeamUnit = SeamUnit::Fraction;
    const FLOOR: f64 = 0.1;

    fn amount(&self) -> f64 {
        self.0
    }

    fn of(amount: f64) -> Fraction {
        Fraction(amount)
    }
}

/// A two-lane split with the seam as APP state: the binding's value
/// renders in as lane A's extent, a drag on the divider writes back —
/// clamped between the lanes' floors. The framework owns the grip (a 6pt
/// band over the 1pt hairline) and the drag loop; the app owns where the
/// seam rests, so persisting or animating it is ordinary state.
///
/// A `Binding<f64>` puts the seam in POINTS: lane A is exactly that
/// wide and everything the window gains goes to lane B. A
/// [`Binding<Fraction>`] puts it in SHARES: both lanes keep their
/// slice through a resize, and the floors become shares too — which is
/// what a tree of panes needs, where a boundary is a proportion and
/// not a pixel.
///
/// [`Binding<Fraction>`]: Fraction
#[derive(Clone)]
pub struct Split<T: 'static, A, B> {
    at: Binding<T>,
    axis: Axis,
    min_a: f64,
    min_b: f64,
    a: A,
    b: B,
}

/// `hsplit(at, leading, trailing)` — the two-lane split. Floors default
/// to a hundred points each (a tenth of the pair, for a fractional
/// seam); tune them with [`Split::min_sizes`]. The vertical twin is
/// [`vsplit`].
pub fn hsplit<T, A, B>(at: Binding<T>, a: A, b: B) -> Split<T, A, B>
where
    T: SeamValue,
    A: View<Arity = Single>,
    B: View<Arity = Single>,
{
    Split { at, axis: Axis::Horizontal, min_a: T::FLOOR, min_b: T::FLOOR, a, b }
}

/// `vsplit(at, top, bottom)` — the same seam, stacked. `at` is the TOP
/// lane's height, and the grip drags up and down.
pub fn vsplit<T, A, B>(at: Binding<T>, a: A, b: B) -> Split<T, A, B>
where
    T: SeamValue,
    A: View<Arity = Single>,
    B: View<Arity = Single>,
{
    Split { at, axis: Axis::Vertical, min_a: T::FLOOR, min_b: T::FLOOR, a, b }
}

impl<T, A, B> Split<T, A, B> {
    /// The lanes' floors, in the SEAM's own unit — points for a
    /// `f64` seam, shares for a `Fraction` one. The drag clamps
    /// against them, and so does every measure.
    pub fn min_sizes(mut self, min_a: f64, min_b: f64) -> Self {
        self.min_a = min_a;
        self.min_b = min_b;
        self
    }
}

impl<T, A, B> View for Split<T, A, B>
where
    T: SeamValue,
    A: View<Arity = Single>,
    B: View<Arity = Single>,
{
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let at = self.at.wrappedValue().amount();
        let mut nodes = NodeList::new();
        // the divider is an ORDINARY child (a themed 1pt strut): it
        // measures, paints and lowers like anything else on every target
        let strut = match self.axis {
            Axis::Horizontal => crate::ext::ViewExt::frame_width(spacer(), 1.0),
            Axis::Vertical => crate::ext::ViewExt::frame_height(spacer(), 1.0),
        };
        let divider =
            crate::ext::ViewExt::background_color(strut, crate::theme::divider());
        (self.a.clone(), divider, self.b.clone()).render_into(ctx, &mut nodes);
        let (prints, layouts) = nodes.into_parts();
        out.push(RenderNode::branch(
            if crate::view::print_enabled() {
                // the printed tree says which unit the seam speaks, so
                // a snapshot can never read a share as a number of points
                let seam = match T::UNIT {
                    SeamUnit::Points => format!("at: {at}"),
                    SeamUnit::Fraction => format!("share: {at}"),
                };
                match self.axis {
                    Axis::Horizontal => format!("HSplitView({seam})"),
                    Axis::Vertical => format!("VSplitView({seam})"),
                }
            } else {
                String::new()
            },
            prints,
        ));
        match motor::identity::cursor_scope() {
            Some(path) => {
                let binding = self.at.clone();
                crate::reconciler::attribute_split(
                    path.clone(),
                    Rc::new(move |new_at| {
                        // the clamp can pin the seam: a repeated value
                        // must not dirty the world
                        if binding.wrappedValue().amount() != new_at {
                            binding.set(T::of(new_at));
                        }
                    }),
                );
                out.push_layout(LayoutNode::Split {
                    path,
                    axis: self.axis,
                    unit: T::UNIT,
                    at,
                    min_a: self.min_a,
                    min_b: self.min_b,
                    children: layouts,
                });
            }
            // outside a pass (decorative use): the lanes become a stack
            None => out.push_layout(LayoutNode::Stack {
                axis: self.axis,
                spacing: 0.0,
                align: CrossAlign::Start,
                children: layouts,
            }),
        }
    }
}

pub fn text_field(placeholder: impl Into<String>, text: Binding<String>) -> TextField {
    TextField { placeholder: Arc::from(placeholder.into()), text }
}

/// `ProgressView()`
#[derive(Clone, Copy, Default)]
pub struct ProgressView;

impl View for ProgressView {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf("ProgressView"));
        out.push_layout(LayoutNode::Leaf {
            size: LayoutSize { width: 20.0, height: 20.0 },
        });
    }
}

pub fn progress_view() -> ProgressView {
    ProgressView
}

/// `Spacer()`
#[derive(Clone, Copy, Default)]
pub struct Spacer;

impl View for Spacer {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf("Spacer"));
        out.push_layout(LayoutNode::Spacer);
    }
}

pub fn spacer() -> Spacer {
    Spacer
}

/// `Rectangle()`
#[derive(Clone, Copy, Default)]
pub struct Rectangle;

impl View for Rectangle {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf("Rectangle"));
        out.push_layout(LayoutNode::Fill);
    }
}

pub fn rectangle() -> Rectangle {
    Rectangle
}

/// `EmptyView` — renders nothing.
#[derive(Clone, Copy, Default)]
pub struct EmptyView;

impl View for EmptyView {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(""));
    }
}

pub fn empty() -> EmptyView {
    EmptyView
}

/// `Image(uiImage: …)` — holds the formatted description, like the engine.
/// The layout node carries NO source: it measures the classic rigid
/// 40×40 and paints the outline box (print parity holds byte for byte).
#[derive(Clone)]
pub struct ImageUiImage(pub String);

impl View for ImageUiImage {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(format!("Image ({})", self.0)));
        out.push_layout(LayoutNode::Image { source: None, resizable: false, fit: None });
    }
}

pub fn image_ui<T: Debug>(image: T) -> ImageUiImage {
    ImageUiImage(format!("{image:?}"))
}

/// An image with real pixels. The platform decodes and resamples; the
/// layout owns geometry. Draws at the intrinsic size (1 pixel = 1
/// point) until `.resizable()` lets it negotiate — then
/// `.aspect_ratio(ContentMode::Fit)` contains and `Fill` covers with a
/// built-in clip.
///
/// ```ignore
/// image(ImageSource::from_bytes(LOGO)).resizable().aspect_ratio(ContentMode::Fit)
/// image(file_icon(path)).resizable().frame(16.0, 16.0)
/// ```
#[derive(Clone)]
pub struct Image(pub crate::image_engine::ImageSource);

impl View for Image {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(format!("Image ({:?})", self.0)));
        out.push_layout(LayoutNode::Image {
            source: Some(self.0.clone()),
            resizable: false,
            fit: None,
        });
    }
}

pub fn image(source: crate::image_engine::ImageSource) -> Image {
    Image(source)
}

/// A vector glyph. Sizes with the INHERITED font, the way a character
/// would, and takes the inherited ink — `.foreground_color` and the
/// hover/press inks reach it with no icon-specific API. For an exact
/// box, the file-icon idiom: `.resizable().frame(w, h)`.
///
/// ```ignore
/// icon(symbol::CHEVRON_RIGHT)
/// icon(symbol::SEARCH).font(Font::Title)
/// icon(symbol::FOLDER).resizable().frame(24.0, 24.0)
/// ```
#[derive(Clone)]
pub struct Icon(pub crate::icon::Symbol);

impl View for Icon {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(format!("Icon ({})", self.0.name)));
        out.push_layout(LayoutNode::Icon { symbol: self.0, resizable: false });
    }
}

use crate::ext::ViewExt as _;

/// One column of a [`table`]: its header and its width in points.
#[derive(Clone)]
pub struct TableColumn {
    pub title: Arc<str>,
    pub width: f64,
}

/// A table column. Width is points; the header paints it bold over a
/// hairline.
pub fn column(title: impl Into<Arc<str>>, width: f64) -> TableColumn {
    TableColumn { title: title.into(), width }
}

/// The row band of a [`table`] — one shared height keeps the virtual
/// window honest and the sheet scannable.
const TABLE_ROW_H: f64 = 26.0;
const TABLE_CELL_PAD: f64 = 8.0;

/// A table: fixed columns across, virtual rows down, and BOTH axes
/// scroll — the header stays put vertically and slides with its
/// columns horizontally, because it lives inside the sideways region
/// and outside the vertical one. Rows come by INDEX (ten thousand
/// exist, a screenful materializes); `cell(row, column)` builds one
/// cell, clipped to its lane.
///
/// ```ignore
/// table(
///     vec![column("Name", 220.0), column("Kind", 90.0), column("Size", 80.0)],
///     files.len(),
///     |row| files[row].id.clone(),
///     move |row, col| text(files[row].field(col)),
/// )
/// ```
///
/// What v1 leaves out, on purpose: the COLUMNS all materialize (the
/// rows are the axis that reaches ten thousand; a sheet of very many
/// columns can wait), and sorting is the app's — reorder your rows,
/// the table draws what it is given.
pub fn table<I, F, R>(
    columns: Vec<TableColumn>,
    count: usize,
    id: I,
    cell: F,
) -> impl View<Arity = crate::view::Single>
where
    I: Fn(usize) -> String + Clone + 'static,
    F: Fn(usize, usize) -> R + Clone + 'static,
    R: View<Arity = crate::view::Single>,
{
    let theme = crate::theme::current();
    let widths: Vec<f64> = columns.iter().map(|column| column.width).collect();
    let header = for_each(
        columns,
        |column| column.title.to_string(),
        |column| {
            text(column.title.clone())
                .bold()
                .padding_edge(motor::views::Edge::Leading, TABLE_CELL_PAD)
                .frame(column.width, TABLE_ROW_H)
        },
    )
    .horizontal();
    let body = {
        let widths = widths.clone();
        virtual_list(count, id, move |row| {
            let cell = cell.clone();
            let widths = widths.clone();
            for_each(
                widths.iter().copied().enumerate().collect::<Vec<_>>(),
                |(col, _)| col.to_string(),
                move |(col, width)| {
                    cell(row, *col)
                        .padding_edge(motor::views::Edge::Leading, TABLE_CELL_PAD)
                        .frame(*width, TABLE_ROW_H)
                        .clipped()
                },
            )
            .horizontal()
        })
    };
    scroll(
        crate::vstack!(
            header
                .background_color(theme.panel)
                .border(theme.border, 1.0),
            body,
        ),
    )
    .horizontal()
}

/// A scroll region of one's own — a vertical viewport by default. The
/// lists already scroll themselves; this is for everything else that
/// overflows: `.horizontal()` goes sideways (an editor without wrap, a
/// terminal line), `.both_axes()` travels freely (a spreadsheet).
#[derive(Clone)]
pub struct ScrollView<C> {
    content: C,
    axes: crate::layout::ScrollAxes,
}

impl<C> ScrollView<C> {
    /// Sideways only.
    pub fn horizontal(mut self) -> Self {
        self.axes = crate::layout::ScrollAxes::Horizontal;
        self
    }

    /// Both ways.
    pub fn both_axes(mut self) -> Self {
        self.axes = crate::layout::ScrollAxes::Both;
        self
    }
}

impl<C: View> View for ScrollView<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mut inner = NodeList::new();
        self.content.render_into(ctx, &mut inner);
        let (prints, layouts) = inner.into_parts();
        out.push(RenderNode::branch(
            if crate::view::print_enabled() { "ScrollView".to_string() } else { String::new() },
            prints,
        ));
        out.push_layout(LayoutNode::Scroll {
            path: motor::identity::cursor_scope(),
            target: None,
            axes: self.axes,
            child: Box::new(wrap_layout(layouts)),
        });
    }
}

pub fn scroll<C: View>(content: C) -> ScrollView<C> {
    ScrollView { content, axes: crate::layout::ScrollAxes::Vertical }
}

/// What a drag carries: the typed value, erased for the wire between
/// source and target, and the label the cursor wears on the way.
#[derive(Clone)]
pub struct DragPayload {
    pub value: Rc<dyn std::any::Any>,
    pub label: Arc<str>,
}

impl std::fmt::Debug for DragPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DragPayload({})", self.label)
    }
}

/// A typed drag: the value any `.on_drop(|v: &T| …)` of the same type
/// accepts, and the label that follows the pointer.
pub fn drag<T: 'static>(value: T, label: impl Into<Arc<str>>) -> DragPayload {
    DragPayload { value: Rc::new(value), label: label.into() }
}

/// One entry of a context menu: a labelled action, or the quiet line
/// between groups.
#[derive(Clone)]
pub enum MenuItem {
    Action { label: std::sync::Arc<str>, action: std::rc::Rc<dyn Fn()> },
    Divider,
}

impl std::fmt::Debug for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MenuItem::Action { label, .. } => write!(f, "MenuItem({label})"),
            MenuItem::Divider => f.write_str("MenuItem(—)"),
        }
    }
}

/// A labelled menu entry. The action runs when the row is picked —
/// the menu closes first, so state written here lays out clean.
pub fn menu_item(label: impl Into<std::sync::Arc<str>>, action: impl Fn() + 'static) -> MenuItem {
    MenuItem::Action { label: label.into(), action: std::rc::Rc::new(action) }
}

/// The line between groups.
pub fn menu_divider() -> MenuItem {
    MenuItem::Divider
}

pub fn icon(symbol: crate::icon::Symbol) -> Icon {
    Icon(symbol)
}

// MARK: - Containers

pub use motor::views::Alignment;

/// Cross-axis alignment of a `VStack` — only what makes sense
/// for columns. (`vstack` with `.bottom` is not a representable state.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HorizontalAlignment {
    Leading,
    Center,
    Trailing,
}

impl HorizontalAlignment {
    fn print(&self) -> &'static str {
        match self {
            HorizontalAlignment::Leading => ".leading",
            HorizontalAlignment::Center => ".center",
            HorizontalAlignment::Trailing => ".trailing",
        }
    }

    fn cross(&self) -> CrossAlign {
        match self {
            HorizontalAlignment::Leading => CrossAlign::Start,
            HorizontalAlignment::Center => CrossAlign::Center,
            HorizontalAlignment::Trailing => CrossAlign::End,
        }
    }
}

/// Cross-axis alignment of an `HStack`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerticalAlignment {
    Top,
    Center,
    Bottom,
    /// Text sits on a shared first baseline; a box with no text below
    /// aligns by its bottom edge.
    Baseline,
}

impl VerticalAlignment {
    fn print(&self) -> &'static str {
        match self {
            VerticalAlignment::Top => ".top",
            VerticalAlignment::Center => ".center",
            VerticalAlignment::Bottom => ".bottom",
            VerticalAlignment::Baseline => ".firstTextBaseline",
        }
    }

    fn cross(&self) -> CrossAlign {
        match self {
            VerticalAlignment::Top => CrossAlign::Start,
            VerticalAlignment::Center => CrossAlign::Center,
            VerticalAlignment::Bottom => CrossAlign::End,
            VerticalAlignment::Baseline => CrossAlign::Baseline,
        }
    }
}

fn stack_line(kind: &str, alignment: &str, spacing: Option<f64>) -> String {
    match spacing {
        Some(spacing) => format!("{kind} (alignment: {alignment}, spacing: {spacing})"),
        None => format!("{kind} (alignment: {alignment})"),
    }
}

/// The full `Alignment` in layout terms. A ZStack aligns on both axes;
/// what the enum names is the HORIZONTAL edge (`.leading` is leading
/// and centered, as in SwiftUI).
pub(crate) fn cross_align(alignment: Alignment) -> CrossAlign {
    match alignment {
        Alignment::Leading => CrossAlign::Start,
        Alignment::Center => CrossAlign::Center,
        Alignment::Trailing => CrossAlign::End,
    }
}

fn render_stack<C: View>(
    children: &C,
    ctx: &Context,
    out: &mut NodeList,
    kind: &str,
    alignment: &str,
    spacing: Option<f64>,
    axis: Option<Axis>,
    align: CrossAlign,
) {
    let mut nodes = NodeList::new();
    children.render_into(ctx, &mut nodes);
    let (prints, layouts) = nodes.into_parts();
    out.push(RenderNode::branch(
        if crate::view::print_enabled() {
            stack_line(kind, alignment, spacing)
        } else {
            String::new()
        },
        prints,
    ));
    out.push_layout(match axis {
        Some(axis) => LayoutNode::Stack {
            axis,
            spacing: spacing.unwrap_or(0.0),
            align,
            children: layouts,
        },
        // ZStack: all children in the same frame
        None => LayoutNode::Layered { align, children: layouts },
    });
}

/// `VStack { … }` — children first; alignment and spacing in methods,
/// with Swift's defaults (center, automatic spacing).
#[derive(Clone)]
pub struct VStack<C> {
    alignment: HorizontalAlignment,
    spacing: Option<f64>,
    children: C,
}

impl<C: View> View for VStack<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        render_stack(
            &self.children,
            ctx,
            out,
            "VStack",
            self.alignment.print(),
            self.spacing,
            Some(Axis::Vertical),
            self.alignment.cross(),
        );
    }
}

impl<C> VStack<C> {
    /// `VStack(alignment: .leading)`
    pub fn alignment(mut self, alignment: HorizontalAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// `VStack(spacing: 8)`
    pub fn spacing(mut self, spacing: f64) -> Self {
        self.spacing = Some(spacing);
        self
    }
}

/// `VStack { … }`
pub fn vstack<C: View>(children: C) -> VStack<C> {
    VStack {
        alignment: HorizontalAlignment::Center,
        spacing: None,
        children,
    }
}

/// `HStack { … }`
#[derive(Clone)]
pub struct HStack<C> {
    alignment: VerticalAlignment,
    spacing: Option<f64>,
    children: C,
}

impl<C: View> View for HStack<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        render_stack(
            &self.children,
            ctx,
            out,
            "HStack",
            self.alignment.print(),
            self.spacing,
            Some(Axis::Horizontal),
            self.alignment.cross(),
        );
    }
}

impl<C> HStack<C> {
    /// `HStack(alignment: .top)`
    pub fn alignment(mut self, alignment: VerticalAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// `HStack(spacing: 8)`
    pub fn spacing(mut self, spacing: f64) -> Self {
        self.spacing = Some(spacing);
        self
    }
}

/// `HStack { … }`
pub fn hstack<C: View>(children: C) -> HStack<C> {
    HStack {
        alignment: VerticalAlignment::Center,
        spacing: None,
        children,
    }
}

/// `ZStack { … }` — depth aligns on both axes, so here it is the
/// full `Alignment`.
#[derive(Clone)]
pub struct ZStack<C> {
    alignment: Alignment,
    children: C,
}

impl<C: View> View for ZStack<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let alignment = self.alignment.to_string();
        render_stack(
            &self.children,
            ctx,
            out,
            "ZStack",
            &alignment,
            None,
            None,
            cross_align(self.alignment),
        );
    }
}

impl<C> ZStack<C> {
    /// `ZStack(alignment: .leading)` — the children hug that horizontal
    /// edge and stay centered vertically.
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }
}

/// `ZStack { … }`
pub fn zstack<C: View>(children: C) -> ZStack<C> {
    ZStack {
        alignment: Alignment::Center,
        children,
    }
}

/// Alternative spelling, associated namespace: `Stack::vertical((…))`.
///
/// Under side-by-side evaluation with `vstack((…))` — same view underneath,
/// only the name changes. One of the two becomes canonical before release;
/// the loser leaves.
pub struct Stack;

impl Stack {
    /// `vstack((…))` by another name.
    pub fn vertical<C: View>(children: C) -> VStack<C> {
        vstack(children)
    }

    /// `hstack((…))` by another name.
    pub fn horizontal<C: View>(children: C) -> HStack<C> {
        hstack(children)
    }

    /// `zstack((…))` by another name.
    pub fn layered<C: View>(children: C) -> ZStack<C> {
        zstack(children)
    }
}

/// `TupleView(…)` — the container that PRINTS its own node (the implicit
/// block of a `@ViewBuilder` with several views; raw tuples flatten into the
/// parent's children, no node of their own). Having its own node, it accepts
/// modifiers — it is the `Group` for decorating several at once.
#[derive(Clone)]
pub struct TupleView<C> {
    children: C,
}

impl<C: View> View for TupleView<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mut children = NodeList::new();
        self.children.render_into(ctx, &mut children);
        let (prints, layouts) = children.into_parts();
        out.push(RenderNode::branch("TupleView", prints));
        out.push_layout(wrap_layout(layouts));
    }
}

pub fn tuple<C: View>(children: C) -> TupleView<C> {
    TupleView { children }
}

/// `List(collection, id: \.keyPath) { item in … }`
#[derive(Clone)]
pub struct List<T, I, F> {
    items: Vec<T>,
    id: I,
    row: F,
}

impl<T, I, F, R> View for List<T, I, F>
where
    T: Clone + 'static,
    I: Fn(&T) -> String + Clone + 'static,
    F: Fn(&T) -> R + Clone + 'static,
    R: View,
{
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        debug_assert_unique_ids("list", self.items.iter().map(&self.id));
        let scope = motor::identity::cursor_scope();
        let mut row_layouts = Vec::new();
        let rows = self
            .items
            .iter()
            .map(|item| {
                let id = (self.id)(item);
                // one bracketed key per row, built once — the identity
                // frame and the boundary path share its bytes (the byte
                // shapes "[id]" and "scope/[id]" are a public contract)
                let mut key = String::with_capacity(id.len() + 2);
                key.push('[');
                key.push_str(&id);
                key.push(']');
                // the row is addressable GEOMETRY: a boundary node under
                // the row's identity records its frame — tests reach it,
                // and `.scroll_target(id)` finds the rect to reveal
                let path = scope.as_ref().map(|scope| {
                    let mut path = String::with_capacity(scope.len() + key.len() + 1);
                    path.push_str(scope);
                    path.push('/');
                    path.push_str(&key);
                    path
                });
                // The item's key is the row's identity: the closure runs with
                // the cursor already inside it, so state built here follows the
                // item — reordering does not shuffle, removing unmounts.
                let _frame = motor::identity::enter(key);
                let mut row = NodeList::new();
                (self.row)(item).render_into(ctx, &mut row);
                let (prints, layouts) = row.into_parts();
                let row_layout = match path {
                    Some(path) => LayoutNode::Boundary {
                        path,
                        children: vec![wrap_layout(layouts)],
                    },
                    None => wrap_layout(layouts),
                };
                row_layouts.push(row_layout);
                RenderNode::branch(
                    if crate::view::print_enabled() {
                        format!("Row (id: {id})")
                    } else {
                        String::new()
                    },
                    prints,
                )
            })
            .collect();
        out.push(RenderNode::branch(
            if crate::view::print_enabled() {
                format!("List ({})", self.items.len())
            } else {
                String::new()
            },
            rows,
        ));
        // List is a scroll region by nature: the rows stack and the
        // overflow stays inside; structural identity addresses the
        // retained offset (a remounted list restores the position)
        out.push_layout(LayoutNode::Scroll {
            target: None,
            axes: crate::layout::ScrollAxes::Vertical,
            path: motor::identity::cursor_scope(),
            child: Box::new(LayoutNode::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: CrossAlign::Start,
                children: row_layouts,
            }),
        });
    }
}

pub fn list<T, I, F, R>(items: Vec<T>, id: I, row: F) -> List<T, I, F>
where
    T: Clone + 'static,
    I: Fn(&T) -> String + Clone + 'static,
    F: Fn(&T) -> R + Clone + 'static,
    R: View,
{
    List { items, id, row }
}

/// Rows materialized on the first frame, before any geometry is known —
/// generous enough to cover any plausible viewport at uniform heights;
/// the window-miss pass corrects the rare shortfall.
const FIRST_WINDOW: usize = 256;

/// A virtualized list: `count` rows of ONE uniform height, and only the
/// visible window (plus one viewport of buffer on each side) exists.
/// Closures take the row INDEX — no collection is cloned into the view.
///
/// LAZY semantics, named and deliberate: a row outside the window is
/// not entered, so its identity dies in the normal sweep and its state
/// is born again when it scrolls back in (`onAppear` fires again) —
/// the industry contract for virtualized rows. State that must survive
/// scrolling lives ABOVE the list. The dense [`list`] keeps full
/// retention; choose by need.
#[derive(Clone)]
pub struct VirtualList<I, F> {
    count: usize,
    id: I,
    row: F,
    reveal: Option<usize>,
    heights: Option<std::rc::Rc<dyn Fn(usize) -> f64>>,
}

impl<I, F> VirtualList<I, F> {
    /// Scrolls just enough to show this row INDEX, materializing it
    /// even when it sits far outside the window (the pin) — the
    /// virtualized sibling of `.scroll_target(id)`, by index because
    /// the list never walks all ids. The wheel stays sovereign in
    /// between; under `.animated`, the reveal flies.
    pub fn reveal(mut self, index: usize) -> Self {
        self.reveal = Some(index);
        self
    }

    /// Every row's own height, by index — the closure is the
    /// AUTHORITY: offsets, the scrollbar and the window math all come
    /// from it (a row that measures taller than its slot overlaps the
    /// next). Keep it cheap and deterministic; it runs once per row
    /// per layout. Without it, rows stay uniform at the measured
    /// height of the first one.
    pub fn row_height_with(mut self, height: impl Fn(usize) -> f64 + 'static) -> Self {
        self.heights = Some(std::rc::Rc::new(height));
        self
    }
}

impl<I, F, R> View for VirtualList<I, F>
where
    I: Fn(usize) -> String + Clone + 'static,
    F: Fn(usize) -> R + Clone + 'static,
    R: View,
{
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let scope = motor::identity::cursor_scope();
        // the window comes from LAST frame's retained geometry (offset,
        // viewport, measured row height) — one frame of lag masked by
        // the buffer; a miss re-runs this body in the same frame
        let snapshot = crate::viewport::region(scope.as_deref());
        // last frame's offsets, when they still describe THIS count —
        // a count that changed falls back and heals by miss
        let offsets = snapshot.as_ref().and_then(|snap| {
            snap.offsets
                .as_ref()
                .filter(|offsets| offsets.len() == self.count + 1)
                .cloned()
        });
        let (first, last) = match (&snapshot, &offsets) {
            // variable heights: the band in px, rows by binary search,
            // one viewport of buffer on each side
            (Some(snap), Some(offsets)) if self.count > 0 => {
                let total = *offsets.last().expect("offsets carry the total");
                let offset = snap.offset_y.clamp(0.0, (total - snap.viewport).max(0.0));
                let first_edge = (offset - snap.viewport).max(0.0);
                let last_edge = offset + 2.0 * snap.viewport;
                let first = offsets
                    .partition_point(|start| *start <= first_edge)
                    .saturating_sub(1)
                    .min(self.count - 1);
                let last = offsets
                    .partition_point(|start| *start < last_edge)
                    .saturating_sub(1)
                    .min(self.count - 1);
                (first, last)
            }
            (Some(snap), None) if snap.row_extent > 0.0 && self.count > 0 => {
                let rows_in_view =
                    (snap.viewport / snap.row_extent).ceil().max(1.0) as usize + 1;
                // the retained offset clamps HERE the way place will
                // clamp it — a count that just shrank must not leave
                // the window math pointing at rows that no longer exist
                let travel = (snap.row_extent * self.count as f64 - snap.viewport)
                    .max(0.0);
                let offset = snap.offset_y.clamp(0.0, travel);
                let top = (offset / snap.row_extent).floor().max(0.0) as usize;
                let first = top.saturating_sub(rows_in_view).min(self.count - 1);
                let last = (top + 2 * rows_in_view).min(self.count - 1);
                (first, last)
            }
            _ => (0, self.count.min(FIRST_WINDOW).saturating_sub(1)),
        };
        // an EMPTY list has no window: `first..=last` is (0, 0) there,
        // and asking for row zero is asking the app for a row it does
        // not have — the shape of every list that waits for data
        if self.count > 0 {
            debug_assert_unique_ids("virtual_list", (first..=last).map(&self.id));
        }
        // the pin: a PENDING reveal exists even far outside the window —
        // WITH its own buffered band, so when the follow-up scrolls to
        // it the fresh viewport is already covered (one body, no extra
        // invalidation round). A reveal the runtime already APPLIED is
        // settled history: it never replaces the window — the wheel
        // that scrolled away stays sovereign.
        let reveal = self.reveal.filter(|index| self.count > 0 && *index < self.count);
        let reveal_id = reveal.map(|index| (self.id)(index));
        let jump_pending = match (&reveal_id, &snapshot) {
            (Some(id), Some(snap)) => snap.applied.as_deref() != Some(id.as_str()),
            (Some(_), None) => true,
            _ => false,
        };
        let pin = reveal
            .filter(|_| jump_pending)
            .filter(|index| *index < first || *index > last);
        let pin_band = pin.map(|index| match (&snapshot, &offsets) {
            // variable heights: the buffer is a viewport of px around
            // the pinned row's own edges
            (Some(snap), Some(offsets)) => {
                let first_edge = (offsets[index] - snap.viewport).max(0.0);
                let last_edge = offsets[index + 1] + snap.viewport;
                let first = offsets
                    .partition_point(|start| *start <= first_edge)
                    .saturating_sub(1)
                    .min(self.count - 1);
                let last = offsets
                    .partition_point(|start| *start < last_edge)
                    .saturating_sub(1)
                    .min(self.count - 1);
                (first, last)
            }
            (Some(snap), None) if snap.row_extent > 0.0 => {
                let buffer =
                    (snap.viewport / snap.row_extent).ceil().max(1.0) as usize + 1;
                (index.saturating_sub(buffer), (index + buffer).min(self.count - 1))
            }
            _ => (index.saturating_sub(1), (index + 1).min(self.count - 1)),
        });
        // a far pin REPLACES the stale window: the offset is about to
        // leave it anyway, so materializing it would be pure waste —
        // the frame in flight is never presented (the follow-up layout
        // is), and the fresh viewport lands covered
        let (first, last) = match pin_band {
            Some(band) => band,
            None => (first, last),
        };

        let mut children = Vec::new();
        let mut prints = Vec::new();
        if self.count > 0 {
            for index in first..=last {
                let id = (self.id)(index);
                // the same byte contract as the dense list: "[id]" is
                // the identity frame, "scope/[id]" the boundary path
                let mut key = String::with_capacity(id.len() + 2);
                key.push('[');
                key.push_str(&id);
                key.push(']');
                let path = scope.as_ref().map(|scope| {
                    let mut path = String::with_capacity(scope.len() + key.len() + 1);
                    path.push_str(scope);
                    path.push('/');
                    path.push_str(&key);
                    path
                });
                let _frame = motor::identity::enter(key);
                let mut row = NodeList::new();
                (self.row)(index).render_into(ctx, &mut row);
                let (row_prints, layouts) = row.into_parts();
                let row_layout = match path {
                    Some(path) => LayoutNode::Boundary {
                        path,
                        children: vec![wrap_layout(layouts)],
                    },
                    None => wrap_layout(layouts),
                };
                children.push((index, row_layout));
                prints.push(RenderNode::branch(
                    if crate::view::print_enabled() {
                        format!("Row (id: {id})")
                    } else {
                        String::new()
                    },
                    row_prints,
                ));
            }
        }
        out.push(RenderNode::branch(
            if crate::view::print_enabled() {
                format!("VirtualList ({} of {})", children.len(), self.count)
            } else {
                String::new()
            },
            prints,
        ));
        out.push_layout(LayoutNode::Scroll {
            target: reveal_id,
            axes: crate::layout::ScrollAxes::Vertical,
            path: motor::identity::cursor_scope(),
            child: Box::new(LayoutNode::VirtualStack {
                row_extent: snapshot.map(|snap| snap.row_extent).unwrap_or(0.0),
                count: self.count,
                children,
                heights: self.heights.clone().map(crate::layout::RowHeights),
            }),
        });
    }
}

/// The virtualized sibling of [`list`]: `virtual_list(count, id, row)`
/// with closures by INDEX. See [`VirtualList`] for the lazy contract.
pub fn virtual_list<I, F, R>(count: usize, id: I, row: F) -> VirtualList<I, F>
where
    I: Fn(usize) -> String + Clone + 'static,
    F: Fn(usize) -> R + Clone + 'static,
    R: View,
{
    VirtualList { count, id, row, reveal: None, heights: None }
}

/// `ForEach(collection, id: \.keyPath) { item in … }` — the `id` is the
/// identity contract: it is what will let state and animation follow the item
/// (reorder, insert in the middle) instead of the position. The headless
/// runtime only enforces the verifiable part today: unique ids.
///
/// The collection lays itself out: a column by default,
/// [`horizontal`](ForEach::horizontal) for a strip of tabs or chips,
/// with the [`spacing`](ForEach::spacing) of a stack. Cross alignment
/// follows the axis — leading down a column, centered along a row.
#[derive(Clone)]
pub struct ForEach<T, I, F> {
    items: Vec<T>,
    id: I,
    row: F,
    axis: Axis,
    spacing: Option<f64>,
}

impl<T, I, F, R> View for ForEach<T, I, F>
where
    T: Clone + 'static,
    I: Fn(&T) -> String + Clone + 'static,
    F: Fn(&T) -> R + Clone + 'static,
    R: View,
{
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        debug_assert_unique_ids("for_each", self.items.iter().map(&self.id));
        let mut rows = NodeList::new();
        for item in &self.items {
            let _frame = motor::identity::enter(format!("[{}]", (self.id)(item)));
            (self.row)(item).render_into(ctx, &mut rows);
        }
        let (prints, layouts) = rows.into_parts();
        out.push(RenderNode::branch(
            for_each_line(self.items.len(), self.axis, self.spacing),
            prints,
        ));
        out.push_layout(LayoutNode::Stack {
            axis: self.axis,
            spacing: self.spacing.unwrap_or(0.0),
            // a column reads leading, a strip reads centered — the same
            // defaults the stacks carry
            align: match self.axis {
                Axis::Vertical => CrossAlign::Start,
                Axis::Horizontal => CrossAlign::Center,
            },
            children: layouts,
        });
    }
}

impl<T, I, F> ForEach<T, I, F> {
    /// The items sit side by side instead of stacking down.
    pub fn horizontal(mut self) -> Self {
        self.axis = Axis::Horizontal;
        self
    }

    /// The default, spelled out: the items stack down.
    pub fn vertical(mut self) -> Self {
        self.axis = Axis::Vertical;
        self
    }

    /// `ForEach` inside a `VStack(spacing: 8)` — the gap between items.
    pub fn spacing(mut self, spacing: f64) -> Self {
        self.spacing = Some(spacing);
        self
    }
}

/// The default column prints as it always did; an axis or a spacing of
/// its own says so.
fn for_each_line(count: usize, axis: Axis, spacing: Option<f64>) -> String {
    let mut line = format!("ForEach ({count}");
    if let Axis::Horizontal = axis {
        line.push_str(", axis: .horizontal");
    }
    if let Some(spacing) = spacing {
        line.push_str(&format!(", spacing: {spacing}"));
    }
    line.push(')');
    line
}

pub fn for_each<T, I, F, R>(items: Vec<T>, id: I, row: F) -> ForEach<T, I, F>
where
    T: Clone + 'static,
    I: Fn(&T) -> String + Clone + 'static,
    F: Fn(&T) -> R + Clone + 'static,
    R: View,
{
    ForEach { items, id, row, axis: Axis::Vertical, spacing: None }
}

fn debug_assert_unique_ids(container: &str, ids: impl Iterator<Item = String>) {
    if cfg!(debug_assertions) {
        let mut seen = HashSet::new();
        for id in ids {
            assert!(
                seen.insert(id.clone()),
                "{container}: duplicate id {id:?} — per-item identity must be unique"
            );
        }
    }
}

/// `Section(header: …) { … }` — and the detail view's
/// `List { Section { … } }`, via [`list_content`].
#[derive(Clone)]
pub struct Section<H, C> {
    header: Option<H>,
    children: C,
    kind: &'static str,
}

impl<H: View, C: View> View for Section<H, C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mut children = NodeList::new();
        if let Some(header) = &self.header {
            let mut header_nodes = NodeList::new();
            header.render_into(ctx, &mut header_nodes);
            let (header_prints, header_layouts) = header_nodes.into_parts();
            children.push(RenderNode::branch("Header", header_prints));
            children.push_layout(wrap_layout(header_layouts));
        }
        self.children.render_into(ctx, &mut children);
        let (prints, layouts) = children.into_parts();
        out.push(RenderNode::branch(self.kind.to_string(), prints));
        let stacked = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: layouts,
        };
        // the List of sections (list_content) is a scroll region; the plain
        // Section is just the stacking
        out.push_layout(if self.kind == "List" {
            LayoutNode::Scroll {
                target: None,
                axes: crate::layout::ScrollAxes::Vertical,
                path: motor::identity::cursor_scope(),
                child: Box::new(stacked),
            }
        } else {
            stacked
        });
    }
}

/// `Section(header: …) { … }`
pub fn section<H: View, C: View>(header: H, children: C) -> Section<H, C> {
    Section {
        header: Some(header),
        children,
        kind: "Section",
    }
}

/// `Section { … }` — no header.
pub fn section_plain<C: View>(children: C) -> Section<EmptyView, C> {
    Section {
        header: None,
        children,
        kind: "Section",
    }
}

/// `List { Section { … } }` — the List built from sections, no collection.
pub fn list_content<C: View>(children: C) -> Section<EmptyView, C> {
    Section {
        header: None,
        children,
        kind: "List",
    }
}

// MARK: - Navigation

/// `NavigationStack(path: $path) { … }` (or without a binding).
#[derive(Clone)]
pub struct NavigationStack<C> {
    path: Option<Binding<NavigationPath>>,
    children: C,
}

impl<C: View> View for NavigationStack<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let detail = match &self.path {
            Some(path) => format!(" (path: {})", path.get().count()),
            None => String::new(),
        };
        let mut children = NodeList::new();
        self.children.render_into(ctx, &mut children);
        let (prints, layouts) = children.into_parts();
        out.push(RenderNode::branch(
            format!("NavigationStack{detail}"),
            prints,
        ));
        out.push_layout(wrap_layout(layouts));
    }
}

/// `NavigationStack(path: $path) { … }`
pub fn navigation_stack<C: View>(path: Binding<NavigationPath>, children: C) -> NavigationStack<C> {
    NavigationStack {
        path: Some(path),
        children,
    }
}

/// `NavigationStack { … }` (no path binding)
pub fn navigation_stack_content<C: View>(children: C) -> NavigationStack<C> {
    NavigationStack {
        path: None,
        children,
    }
}

/// `NavigationLink(destination:) { label }` / `NavigationLink(value:) { label }`
#[derive(Clone)]
pub struct NavigationLink<L> {
    detail: String,
    label: L,
}

impl<L: View> View for NavigationLink<L> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mut label = NodeList::new();
        self.label.render_into(ctx, &mut label);
        let (prints, layouts) = label.into_parts();
        out.push(RenderNode::branch(
            format!("NavigationLink → {}", self.detail),
            prints,
        ));
        out.push_layout(wrap_layout(layouts));
    }
}

/// `NavigationLink(destination: …) { label }` — the destination never mounts
/// in the fake runtime, it only describes itself.
pub fn navigation_link<D: View<Arity = Single>, L: View>(
    destination: D,
    label: L,
) -> NavigationLink<L> {
    NavigationLink {
        detail: render_line(&destination),
        label,
    }
}

/// `NavigationLink(value: country) { … }`
pub fn nav_link_value<V: Debug + 'static, L: View>(value: V, label: L) -> NavigationLink<L> {
    NavigationLink {
        detail: format!("{value:?}"),
        label,
    }
}

/// `ToolbarItem { … }` — exists in the API; the fake runtime's `.toolbar` is
/// inert and never mounts it (parity with the engine, which also drops the content).
#[derive(Clone)]
pub struct ToolbarItem;

impl View for ToolbarItem {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf("ToolbarItem"));
    }
}

pub fn toolbar_item<C: View>(_content: C) -> ToolbarItem {
    ToolbarItem
}

/// `WindowGroup { … }` (Scene level)
#[derive(Clone)]
pub struct WindowGroup<C> {
    children: C,
}

impl<C: View> View for WindowGroup<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mut children = NodeList::new();
        self.children.render_into(ctx, &mut children);
        let (prints, layouts) = children.into_parts();
        out.push(RenderNode::branch("WindowGroup", prints));
        out.push_layout(wrap_layout(layouts));
    }
}

pub fn window_group<C: View>(children: C) -> WindowGroup<C> {
    WindowGroup { children }
}
