//! The built-in views (`Text`, `VStack`, `List`, …) plus the fake
//! `NavigationPath` and SwiftData's `Query`.

use crate::state::Context;
use crate::view::{AnyView, RenderNode, View};
use std::cell::RefCell;
use std::fmt::Display;
use std::rc::Rc;

// MARK: - Style enums

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    LargeTitle,
    Title,
    Headline,
    Subheadline,
    Body,
    Callout,
    Footnote,
    Caption,
    Caption2,
}

impl Display for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Font::LargeTitle => "largeTitle",
            Font::Title => "title",
            Font::Headline => "headline",
            Font::Subheadline => "subheadline",
            Font::Body => "body",
            Font::Callout => "callout",
            Font::Footnote => "footnote",
            Font::Caption => "caption",
            Font::Caption2 => "caption2",
        };
        write!(f, ".{name}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Leading,
    Center,
    Trailing,
}

impl Display for Alignment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Alignment::Leading => "leading",
            Alignment::Center => "center",
            Alignment::Trailing => "trailing",
        };
        write!(f, ".{name}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlignment {
    Leading,
    Center,
    Trailing,
}

impl Display for TextAlignment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            TextAlignment::Leading => "leading",
            TextAlignment::Center => "center",
            TextAlignment::Trailing => "trailing",
        };
        write!(f, ".{name}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Leading,
    Trailing,
}

impl Display for Edge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Edge::Top => "top",
            Edge::Bottom => "bottom",
            Edge::Leading => "leading",
            Edge::Trailing => "trailing",
        };
        write!(f, ".{name}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentMode {
    Fit,
    Fill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStyle {
    Grouped,
}

impl Display for ListStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ListStyle::Grouped => "grouped",
        };
        write!(f, ".{name}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressViewStyle {
    Circular,
}

impl Display for ProgressViewStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ProgressViewStyle::Circular => "circular",
        };
        write!(f, ".{name}")
    }
}

// MARK: - Leaf / container views

#[derive(Clone)]
pub struct TextView {
    text: String,
}

impl View for TextView {
    fn render(&self, _ctx: &Context) -> RenderNode {
        RenderNode::leaf(format!("Text({:?})", self.text))
    }
}

/// `Text("…")`
pub fn Text(text: impl Into<String>) -> AnyView {
    AnyView::new(TextView { text: text.into() })
}

/// `Text(verbatim:)` — same thing here.
pub fn TextVerbatim(text: impl Into<String>) -> AnyView {
    Text(text)
}

#[derive(Clone)]
pub struct VStackView {
    kind: &'static str,
    alignment: Alignment,
    children: Vec<AnyView>,
}

impl View for VStackView {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch(
            format!("{} (alignment: {})", self.kind, self.alignment),
            self.children.iter().map(|child| child.render(ctx)).collect(),
        )
    }
}

pub fn VStack(alignment: Alignment, children: Vec<AnyView>) -> AnyView {
    AnyView::new(VStackView { kind: "VStack", alignment, children })
}

pub fn HStack(alignment: Alignment, children: Vec<AnyView>) -> AnyView {
    AnyView::new(VStackView { kind: "HStack", alignment, children })
}

pub fn ZStack(alignment: Alignment, children: Vec<AnyView>) -> AnyView {
    AnyView::new(VStackView { kind: "ZStack", alignment, children })
}

#[derive(Clone)]
pub struct ListView<T> {
    items: Vec<T>,
    id: Rc<dyn Fn(&T) -> String>,
    row: Rc<dyn Fn(&T) -> AnyView>,
}

impl<T: Clone + 'static> View for ListView<T> {
    fn render(&self, ctx: &Context) -> RenderNode {
        let rows = self
            .items
            .iter()
            .map(|item| {
                let id = (self.id)(item);
                RenderNode::branch(
                    format!("Row (id: {id})"),
                    vec![(self.row)(item).render(ctx)],
                )
            })
            .collect();
        RenderNode::branch(format!("List ({})", self.items.len()), rows)
    }
}

/// `List(collection, id: \.keyPath) { item in … }`
pub fn List<T: Clone + 'static>(
    items: Vec<T>,
    id: Rc<dyn Fn(&T) -> String>,
    row: Rc<dyn Fn(&T) -> AnyView>,
) -> AnyView {
    AnyView::new(ListView { items, id, row })
}

/// `List { Section { … } }`
pub fn ListContent(children: Vec<AnyView>) -> AnyView {
    AnyView::new(SectionView { header: None, children, kind: "List" })
}

#[derive(Clone)]
pub struct SectionView {
    header: Option<AnyView>,
    children: Vec<AnyView>,
    kind: &'static str,
}

impl View for SectionView {
    fn render(&self, ctx: &Context) -> RenderNode {
        let mut children = Vec::new();
        if let Some(header) = &self.header {
            children.push(RenderNode::branch("Header", vec![header.render(ctx)]));
        }
        children.extend(self.children.iter().map(|child| child.render(ctx)));
        RenderNode::branch(self.kind.to_string(), children)
    }
}

/// `Section(header: Text("…")) { … }`
pub fn Section(header: Option<AnyView>, children: Vec<AnyView>) -> AnyView {
    AnyView::new(SectionView { header, children, kind: "Section" })
}

#[derive(Clone)]
pub struct NavigationStackView {
    path: Option<crate::state::Binding<NavigationPath>>,
    children: Vec<AnyView>,
}

impl View for NavigationStackView {
    fn render(&self, ctx: &Context) -> RenderNode {
        let detail = match &self.path {
            Some(path) => format!(" (path: {})", path.wrappedValue().count()),
            None => String::new(),
        };
        RenderNode::branch(
            format!("NavigationStack{detail}"),
            self.children.iter().map(|child| child.render(ctx)).collect(),
        )
    }
}

/// `NavigationStack(path: $path) { … }`
pub fn NavigationStack(
    path: crate::state::Binding<NavigationPath>,
    children: Vec<AnyView>,
) -> AnyView {
    AnyView::new(NavigationStackView { path: Some(path), children })
}

/// `NavigationStack { … }`
pub fn NavigationStackContent(children: Vec<AnyView>) -> AnyView {
    AnyView::new(NavigationStackView { path: None, children })
}

#[derive(Clone)]
pub struct NavigationLinkView {
    detail: String,
    label: AnyView,
}

impl View for NavigationLinkView {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch(format!("NavigationLink → {}", self.detail), vec![self.label.render(ctx)])
    }
}

/// `NavigationLink(value: country) { … }`
pub fn NavigationLinkValue(value: impl std::fmt::Debug, label: AnyView) -> AnyView {
    AnyView::new(NavigationLinkView { detail: format!("{value:?}"), label })
}

/// `NavigationLink(destination: …) { label }`
pub fn NavigationLink(destination: AnyView, label: AnyView) -> AnyView {
    let destination = destination.render(&Context::default()).line;
    AnyView::new(NavigationLinkView { detail: destination, label })
}

#[derive(Clone)]
pub struct ButtonView {
    action: Rc<dyn Fn()>,
    label: AnyView,
}

impl View for ButtonView {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch("Button", vec![self.label.render(ctx)])
    }
}

impl ButtonView {
    /// Pressing the button, for the headless demo.
    pub fn tap(&self) {
        (self.action)();
    }
}

/// `Button(action:label:)`
pub fn Button(action: Rc<dyn Fn()>, label: AnyView) -> AnyView {
    AnyView::new(ButtonView { action, label })
}

#[derive(Clone)]
pub struct ImageView {
    detail: String,
}

impl View for ImageView {
    fn render(&self, _ctx: &Context) -> RenderNode {
        RenderNode::leaf(format!("Image ({})", self.detail))
    }
}

/// `Image(uiImage: image)`
pub fn ImageUiImage<T: std::fmt::Debug>(image: T) -> AnyView {
    AnyView::new(ImageView { detail: format!("{image:?}") })
}

#[derive(Clone)]
pub struct ProgressViewLeaf;

impl View for ProgressViewLeaf {
    fn render(&self, _ctx: &Context) -> RenderNode {
        RenderNode::leaf("ProgressView")
    }
}

/// `ProgressView()`
pub fn ProgressView() -> AnyView {
    AnyView::new(ProgressViewLeaf)
}

#[derive(Clone)]
pub struct SpacerLeaf;

impl View for SpacerLeaf {
    fn render(&self, _ctx: &Context) -> RenderNode {
        RenderNode::leaf("Spacer")
    }
}

/// `Spacer()`
pub fn Spacer() -> AnyView {
    AnyView::new(SpacerLeaf)
}

#[derive(Clone)]
pub struct RectangleLeaf;

impl View for RectangleLeaf {
    fn render(&self, _ctx: &Context) -> RenderNode {
        RenderNode::leaf("Rectangle")
    }
}

/// `Rectangle().hidden()` (the QueryView shield)
pub fn Rectangle() -> AnyView {
    AnyView::new(RectangleLeaf)
}

#[derive(Clone)]
pub struct EmptyViewLeaf;

impl View for EmptyViewLeaf {
    fn render(&self, _ctx: &Context) -> RenderNode {
        RenderNode::leaf("")
    }
}

/// `EmptyView` — renders nothing.
pub fn EmptyView() -> AnyView {
    AnyView::new(EmptyViewLeaf)
}

#[derive(Clone)]
pub struct TupleView {
    children: Vec<AnyView>,
}

impl View for TupleView {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch(
            "TupleView",
            self.children.iter().map(|child| child.render(ctx)).collect(),
        )
    }
}

/// The implicit container of a multi-statement ViewBuilder block.
pub fn TupleView(children: Vec<AnyView>) -> AnyView {
    AnyView::new(TupleView { children })
}

#[derive(Clone)]
pub struct ForEachView<T> {
    items: Vec<T>,
    row: Rc<dyn Fn(&T) -> AnyView>,
}

impl<T: Clone + 'static> View for ForEachView<T> {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch(
            format!("ForEach ({})", self.items.len()),
            self.items.iter().map(|item| (self.row)(item).render(ctx)).collect(),
        )
    }
}

/// `ForEach(collection) { item in … }`
pub fn ForEach<T: Clone + 'static>(items: Vec<T>, row: Rc<dyn Fn(&T) -> AnyView>) -> AnyView {
    AnyView::new(ForEachView { items, row })
}

#[derive(Clone)]
pub struct WindowGroupView {
    children: Vec<AnyView>,
}

impl View for WindowGroupView {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch("WindowGroup", self.children.iter().map(|c| c.render(ctx)).collect())
    }
}

/// `WindowGroup { … }` (Scene level)
pub fn WindowGroup(children: Vec<AnyView>) -> AnyView {
    AnyView::new(WindowGroupView { children })
}

#[derive(Clone)]
pub struct ToolbarItemView {
    content: AnyView,
}

impl View for ToolbarItemView {
    fn render(&self, ctx: &Context) -> RenderNode {
        RenderNode::branch("ToolbarItem", vec![self.content.render(ctx)])
    }
}

/// `ToolbarItem { … }`
pub fn ToolbarItem(content: AnyView) -> AnyView {
    AnyView::new(ToolbarItemView { content })
}

// MARK: - NavigationPath

/// `NavigationPath` — remembers debug descriptions of pushed values.
#[derive(Clone, Default)]
pub struct NavigationPath {
    entries: Rc<RefCell<Vec<String>>>,
}

impl NavigationPath {
    /// `NavigationPath()`
    pub fn new() -> Self {
        Self::default()
    }

    /// `path.isEmpty`
    pub fn isEmpty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    /// snake_case alias — the idiomatic side calls through here
    pub fn is_empty(&self) -> bool {
        self.isEmpty()
    }

    /// `path.count`
    pub fn count(&self) -> usize {
        self.entries.borrow().len()
    }

    /// `path.append(value)`
    pub fn append(&self, item: impl std::fmt::Debug) {
        self.entries.borrow_mut().push(format!("{item:?}"));
    }

    /// `path = NavigationPath()` (pop to root)
    pub fn removeAll(&self) {
        self.entries.borrow_mut().clear();
    }
}

impl PartialEq for NavigationPath {
    fn eq(&self, other: &Self) -> bool {
        *self.entries.borrow() == *other.entries.borrow()
    }
}

impl std::fmt::Debug for NavigationPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.entries.borrow().iter()).finish()
    }
}

// MARK: - Query (SwiftData)

/// `Query<T, [T]>` — filter + sort built by `.query(searchText:results:)`.
pub struct Query<T: 'static> {
    pub filter: Rc<dyn Fn(&T) -> bool>,
    pub sortKey: Rc<dyn Fn(&T) -> String>,
}

impl<T: 'static> Clone for Query<T> {
    fn clone(&self) -> Self {
        Query { filter: self.filter.clone(), sortKey: self.sortKey.clone() }
    }
}

impl<T: 'static> Query<T> {
    /// `Query(filter:sort:)`
    pub fn new(filter: Rc<dyn Fn(&T) -> bool>, sortKey: Rc<dyn Fn(&T) -> String>) -> Self {
        Query { filter, sortKey }
    }
}
