//! As views built-in, escritas do jeito Rust — genéricas de ponta a ponta.
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
//! Os filhos são tuplas (o `TupleView` implícito de um bloco
//! `@ViewBuilder`), não `Vec<AnyView>` — aridade em compile time, zero
//! apagamento. Onde o Swift imprime um nó `TupleView` explícito, o port
//! chama [`tuple`]. Configuração fica *depois* dos filhos, em métodos
//! (`.alignment(…)`, `.spacing(…)`) — o default some do callsite, como o
//! argumento omitido do Swift. Convenção de assinatura: conteúdo primeiro,
//! comportamento (closures) por último.

use std::collections::HashSet;
use std::fmt::Debug;
use std::rc::Rc;

use motor::state::{Binding, Context};
use motor::view::RenderNode;
use motor::views::NavigationPath;

use crate::layout::{Axis, Color, CrossAlign, Edges, LayoutNode, Size as LayoutSize, VisualProps};
use crate::state_ext::BindingExt;
use crate::view::{NodeList, Single, View, render_line};

/// Vários nós de layout virando UM (labels compostos, seções, tuplas
/// explícitas): um filho passa direto; vários empilham na vertical.
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

// MARK: - Folhas

/// `Text("…")` — `Rc<str>` para clonar barato (views são valores).
#[derive(Clone)]
pub struct Text(pub Rc<str>);

impl View for Text {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(format!("Text({:?})", self.0)));
        out.push_layout(LayoutNode::Text { content: self.0.clone() });
    }
}

pub fn text(string: impl Into<String>) -> Text {
    Text(Rc::from(string.into()))
}

// O tema-de-um-lápis do chrome de botão (Role/Size chegam com o port do
// tema; a referência futura é altura 26/34/44 com texto 11/13/15).
const BUTTON_BG: Color = Color::hex(0xDDE1E9);
const BUTTON_BG_HOVERED: Color = Color::hex(0xE7EAF1);
const BUTTON_BG_PRESSED: Color = Color::hex(0xC7CCD8);
const BUTTON_RADIUS: f64 = 6.0;
const BUTTON_PAD_H: f64 = 14.0;
const BUTTON_PAD_V: f64 = 6.0;

/// `Button(action:) { label }` — a ação mora num campo `F: Fn()`, chamada
/// estaticamente (não há `Rc<dyn Fn()>` aqui).
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

        // o chrome default vive na CENA (o print fica como era): fundo com
        // cantos + padding embutido, estados de hover/pressed inclusos —
        // o hit-rect passa a ser o chrome inteiro, não só o label
        let chrome = LayoutNode::Styled {
            props: VisualProps {
                background: Some(BUTTON_BG),
                background_hovered: Some(BUTTON_BG_HOVERED),
                background_pressed: Some(BUTTON_BG_PRESSED),
                corner_radius: Some(BUTTON_RADIUS),
                ..VisualProps::default()
            },
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

        // dentro de um pass, o botão é um alvo de interação: o frame entra
        // no hit-test com o caminho de identidade, e a ação fica registrada
        // no reconciler (retida como os efeitos — view pulada, botão vivo)
        match motor::identity::cursor_scope() {
            Some(path) => {
                let action = self.action.clone();
                crate::reconciler::attribute_action(path.clone(), Rc::new(move || action()));
                out.push_layout(LayoutNode::Interactive {
                    path,
                    hovered: false,
                    pressed: false,
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

/// `Button(action:) { label }` — o label primeiro (é o que se lê), a ação
/// por último (closures longas formatam melhor no fim, e é a convenção de
/// toda a API: conteúdo antes, comportamento depois).
pub fn button<L, F>(label: L, action: F) -> Button<L, F>
where
    L: View,
    F: Fn() + Clone + 'static,
{
    Button { label, action }
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

/// `Image(uiImage: …)` — segura a descrição formatada, como o motor.
#[derive(Clone)]
pub struct ImageUiImage(pub String);

impl View for ImageUiImage {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        out.push(RenderNode::leaf(format!("Image ({})", self.0)));
        out.push_layout(LayoutNode::Leaf {
            size: LayoutSize { width: 40.0, height: 40.0 },
        });
    }
}

pub fn image_ui<T: Debug>(image: T) -> ImageUiImage {
    ImageUiImage(format!("{image:?}"))
}

// MARK: - Contêineres

pub use motor::views::Alignment;

/// Alinhamento no eixo transversal de um `VStack` — só o que faz sentido
/// para colunas. (`vstack` com `.bottom` não é um estado representável.)
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

/// Alinhamento no eixo transversal de um `HStack`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerticalAlignment {
    Top,
    Center,
    Bottom,
}

impl VerticalAlignment {
    fn print(&self) -> &'static str {
        match self {
            VerticalAlignment::Top => ".top",
            VerticalAlignment::Center => ".center",
            VerticalAlignment::Bottom => ".bottom",
        }
    }

    fn cross(&self) -> CrossAlign {
        match self {
            VerticalAlignment::Top => CrossAlign::Start,
            VerticalAlignment::Center => CrossAlign::Center,
            VerticalAlignment::Bottom => CrossAlign::End,
        }
    }
}

fn stack_line(kind: &str, alignment: &str, spacing: Option<f64>) -> String {
    match spacing {
        Some(spacing) => format!("{kind} (alignment: {alignment}, spacing: {spacing})"),
        None => format!("{kind} (alignment: {alignment})"),
    }
}

fn render_stack<C: View>(
    children: &C,
    ctx: &Context,
    out: &mut NodeList,
    kind: &str,
    alignment: &str,
    spacing: Option<f64>,
    layout_axis: Option<(Axis, CrossAlign)>,
) {
    let mut nodes = NodeList::new();
    children.render_into(ctx, &mut nodes);
    let (prints, layouts) = nodes.into_parts();
    out.push(RenderNode::branch(stack_line(kind, alignment, spacing), prints));
    out.push_layout(match layout_axis {
        Some((axis, align)) => LayoutNode::Stack {
            axis,
            spacing: spacing.unwrap_or(0.0),
            align,
            children: layouts,
        },
        // ZStack: todos os filhos no mesmo frame
        None => LayoutNode::Layered { children: layouts },
    });
}

/// `VStack { … }` — filhos primeiro; alinhamento e spacing nos métodos,
/// com os defaults do Swift (center, spacing automático).
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
            Some((Axis::Vertical, self.alignment.cross())),
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
            Some((Axis::Horizontal, self.alignment.cross())),
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

/// `ZStack { … }` — profundidade alinha nos dois eixos, então aqui é o
/// `Alignment` completo.
#[derive(Clone)]
pub struct ZStack<C> {
    alignment: Alignment,
    children: C,
}

impl<C: View> View for ZStack<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let alignment = self.alignment.to_string();
        render_stack(&self.children, ctx, out, "ZStack", &alignment, None, None);
    }
}

impl<C> ZStack<C> {
    /// `ZStack(alignment: .leading)`
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

/// Grafia alternativa, namespace associado: `Stack::vertical((…))`.
///
/// Em avaliação lado a lado com `vstack((…))` — mesma view por baixo, só o
/// nome muda. Uma das duas vira a canônica antes do release; a perdedora
/// sai.
pub struct Stack;

impl Stack {
    /// `vstack((…))` por outro nome.
    pub fn vertical<C: View>(children: C) -> VStack<C> {
        vstack(children)
    }

    /// `hstack((…))` por outro nome.
    pub fn horizontal<C: View>(children: C) -> HStack<C> {
        hstack(children)
    }

    /// `zstack((…))` por outro nome.
    pub fn layered<C: View>(children: C) -> ZStack<C> {
        zstack(children)
    }
}

/// `TupleView(…)` — o contêiner que IMPRIME o próprio nó (o bloco implícito
/// de um `@ViewBuilder` com várias views; tuplas cruas achatam nos filhos
/// do pai, sem nó próprio). Por ter nó próprio, aceita modifiers — é o
/// `Group` de quem quer decorar vários de uma vez.
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
        let mut row_layouts = Vec::new();
        let rows = self
            .items
            .iter()
            .map(|item| {
                let id = (self.id)(item);
                // A chave do item é a identidade da row: a closure roda com o
                // cursor já dentro dela, então estado construído aqui segue o
                // item — reordenar não embaralha, remover desmonta.
                let _frame = motor::identity::enter(format!("[{id}]"));
                let mut row = NodeList::new();
                (self.row)(item).render_into(ctx, &mut row);
                let (prints, layouts) = row.into_parts();
                row_layouts.push(wrap_layout(layouts));
                RenderNode::branch(format!("Row (id: {id})"), prints)
            })
            .collect();
        out.push(RenderNode::branch(
            format!("List ({})", self.items.len()),
            rows,
        ));
        // List é uma região de rolagem por natureza: as rows empilham e o
        // excedente fica por dentro
        out.push_layout(LayoutNode::Scroll {
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

/// `ForEach(collection, id: \.keyPath) { item in … }` — o `id` é o contrato
/// de identidade: é ele que vai deixar estado e animação seguirem o item
/// (reordenar, inserir no meio) em vez da posição. O runtime headless só
/// cobra a parte verificável hoje: ids únicos.
#[derive(Clone)]
pub struct ForEach<T, I, F> {
    items: Vec<T>,
    id: I,
    row: F,
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
            format!("ForEach ({})", self.items.len()),
            prints,
        ));
        out.push_layout(LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: layouts,
        });
    }
}

pub fn for_each<T, I, F, R>(items: Vec<T>, id: I, row: F) -> ForEach<T, I, F>
where
    T: Clone + 'static,
    I: Fn(&T) -> String + Clone + 'static,
    F: Fn(&T) -> R + Clone + 'static,
    R: View,
{
    ForEach { items, id, row }
}

fn debug_assert_unique_ids(container: &str, ids: impl Iterator<Item = String>) {
    if cfg!(debug_assertions) {
        let mut seen = HashSet::new();
        for id in ids {
            assert!(
                seen.insert(id.clone()),
                "{container}: id duplicado {id:?} — identidade por item precisa ser única"
            );
        }
    }
}

/// `Section(header: …) { … }` — e o `List { Section { … } }` da view de
/// detalhes, via [`list_content`].
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
        // a List de sections (list_content) é região de rolagem; a Section
        // comum é só o empilhamento
        out.push_layout(if self.kind == "List" {
            LayoutNode::Scroll { child: Box::new(stacked) }
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

/// `Section { … }` — sem header.
pub fn section_plain<C: View>(children: C) -> Section<EmptyView, C> {
    Section {
        header: None,
        children,
        kind: "Section",
    }
}

/// `List { Section { … } }` — a List construída de sections, sem coleção.
pub fn list_content<C: View>(children: C) -> Section<EmptyView, C> {
    Section {
        header: None,
        children,
        kind: "List",
    }
}

// MARK: - Navegação

/// `NavigationStack(path: $path) { … }` (ou sem binding).
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

/// `NavigationStack { … }` (sem binding de path)
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

/// `NavigationLink(destination: …) { label }` — o destino nunca monta no
/// runtime fake, só se descreve.
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

/// `ToolbarItem { … }` — existe na API; o `.toolbar` do runtime fake é inert
/// e nunca o monta (paridade com o motor, que também descarta o conteúdo).
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

/// `WindowGroup { … }` (nível de Scene)
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
