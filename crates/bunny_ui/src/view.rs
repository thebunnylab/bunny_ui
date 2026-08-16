//! `View` — a árvore tipada.
//!
//! Aqui não há `AnyView`: cada view é um valor genérico (`VStack<C>` com
//! filhos em tupla, `Button<L, F>` com a ação num campo), o dispatch é
//! estático de ponta a ponta e nenhuma `Rc` nasce durante o render. O
//! apagamento existe só nas bordas de dinamismo real — [`Erased`] (sheets,
//! `ViewModifier`), a fila de efeitos e os slots por site.
//!
//! `render_into` appende nós numa [`NodeList`] (uma tupla appende vários,
//! `EmptyView`/`None` nenhum) — é o achatamento que o `@ViewBuilder` do
//! SwiftUI faz no codegen. A `NodeList` é opaca de propósito: o formato de
//! saída é detalhe do motor, e vai mudar quando ele deixar de ser headless.
//!
//! [`Erased`]: crate::erased::Erased

use motor::state::Context;
use motor::view::RenderNode;

/// Quantos nós uma view appende — parte do tipo, para o compilador barrar
/// o que não faz sentido. Modificar `(a, b).padding()` decoraria só o `b`;
/// com `Arity` isso nem compila: [`ViewExt`] exige `Arity = Single`.
/// (Para agrupar de verdade, [`tuple`] imprime o próprio nó — e aí aceita
/// modifier.)
///
/// [`ViewExt`]: crate::ext::ViewExt
/// [`tuple`]: crate::views::tuple
pub struct Single;

/// O outro lado de [`Single`]: zero-ou-vários nós (tuplas, `Option`).
pub struct Many;

/// SwiftUI's `View`: a renderable blueprint. Built-ins implement
/// `render_into` directly; user views get it for free from
/// [`Component::body`].
///
/// Não implemente `View` à mão: o contrato de render é interno e vai
/// trocar junto com o motor. Views próprias implementam [`Component`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a View",
    label = "a view is expected here",
    note = "implement `Component` (the `fn body`) for your own views — never `View` directly",
    note = "put many children in one tuple: `vstack((a, b, c))` — a tuple holds up to 12; nest tuples for more"
)]
pub trait View: Clone + 'static {
    /// [`Single`] quando a view appende exatamente um nó; [`Many`] quando
    /// pode appender zero ou vários. Modifiers só existem para `Single`.
    type Arity;

    /// Appends this view's nodes to `out` (a tuple appends several,
    /// `EmptyView` and `None` append nothing that prints).
    #[doc(hidden)]
    fn render_into(&self, ctx: &Context, out: &mut NodeList);
}

/// Atalho para "view de exatamente um nó" em assinaturas de helpers:
/// `-> impl UnaryView`. Um `-> impl View` esconderia a aridade (opacos só
/// revelam o que a assinatura promete) e o callsite não conseguiria nem
/// aplicar modifier nem entrar num braço de `OneOf`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` can render many nodes — exactly one is required here",
    note = "to decorate a group, wrap it with `tuple(…)` — the wrapper has its own node"
)]
pub trait UnaryView: View<Arity = Single> {}

impl<V: View<Arity = Single>> UnaryView for V {}

/// A saída do render — opaca fora do crate (os mutadores são `pub(crate)`):
/// implementar `View` por fora até compila, mas não produz nó nenhum, que é
/// o jeito educado de dizer "implemente `Component`".
///
/// Carrega as DUAS saídas do único body-eval por pass: a árvore impressa
/// (`RenderNode`) e a árvore de layout ([`LayoutNode`]) — avaliar o body
/// duas vezes duplicaria âncoras de identidade, então print e layout saem
/// juntos.
///
/// [`LayoutNode`]: crate::layout::LayoutNode
#[derive(Default)]
pub struct NodeList {
    nodes: Vec<RenderNode>,
    layout: Vec<crate::layout::LayoutNode>,
}

impl NodeList {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, node: RenderNode) {
        self.nodes.push(node);
    }

    pub(crate) fn push_layout(&mut self, node: crate::layout::LayoutNode) {
        self.layout.push(node);
    }

    /// Embrulha o último nó de layout (o da view Single que acabou de
    /// render) — o caminho dos modifiers de layout (`.padding()`, frames).
    pub(crate) fn wrap_last_layout(
        &mut self,
        wrap: impl FnOnce(crate::layout::LayoutNode) -> crate::layout::LayoutNode,
    ) {
        if let Some(last) = self.layout.pop() {
            self.layout.push(wrap(last));
        }
    }

    /// Uma fronteira retida: entra como referência (a linha marcada no
    /// print, o nó-referência no layout) e a montagem final expande contra
    /// o reconciler.
    pub(crate) fn push_view_ref(&mut self, path: &str) {
        self.nodes.push(RenderNode::leaf(crate::reconciler::ref_line(path)));
        self.layout
            .push(crate::layout::LayoutNode::BoundaryRef { path: path.to_string() });
    }

    pub(crate) fn last_mut(&mut self) -> Option<&mut RenderNode> {
        self.nodes.last_mut()
    }

    pub(crate) fn extend(&mut self, other: NodeList) {
        self.nodes.extend(other.nodes);
        self.layout.extend(other.layout);
    }

    pub(crate) fn into_nodes(self) -> Vec<RenderNode> {
        self.nodes
    }

    pub(crate) fn into_parts(self) -> (Vec<RenderNode>, Vec<crate::layout::LayoutNode>) {
        (self.nodes, self.layout)
    }

    pub(crate) fn take_layout(&mut self) -> Vec<crate::layout::LayoutNode> {
        std::mem::take(&mut self.layout)
    }

    pub(crate) fn nodes(&self) -> &[RenderNode] {
        &self.nodes
    }
}

/// The conformance every `struct X: View` writes by hand.
///
/// (`var body: some View` → `fn body(self, ctx: &Context) -> impl View` —
/// return-position `impl Trait` in trait, estável desde o Rust 1.75. O tipo
/// concreto da árvore inteira fica conhecido em compile time.)
///
/// O body recebe `self` POR VALOR de propósito: closures capturam os
/// campos que usam (captura disjunta do Rust 2021) — `move ||
/// self.count.add(1)` funciona direto, sem o `let this = *self` que a
/// forma `&self` obrigava. Views são valores baratos (`State` é Copy); o
/// runtime clona antes de chamar.
pub trait Component: Clone + 'static {
    fn body(self, ctx: &Context) -> impl View;
}

impl<T: Component> View for T {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        // O frame cobre a construção do body E a descida: todo `State::new`
        // disparado pelos construtores dos filhos ancora nesta identidade.
        let _frame = motor::identity::enter_view(short_type_name::<T>());

        // Sem pass ativo (render fora do Runtime): caminho direto, sem
        // retenção — o comportamento pré-reconciler.
        let Some(path) = motor::identity::current_view_path() else {
            let mut body = NodeList::new();
            self.clone().body(ctx).render_into(ctx, &mut body);
            let (print_children, layout_children) = body.into_parts();
            out.push(RenderNode::branch(short_type_name::<T>(), print_children));
            out.push_layout(crate::layout::LayoutNode::Boundary {
                path: short_type_name::<T>(),
                children: layout_children,
            });
            return;
        };

        // Fronteira limpa e retida, fora de qualquer body re-rodando: o
        // body NÃO roda — sai uma referência e o cache responde por ela.
        if let crate::reconciler::Decision::Skip = crate::reconciler::decide(&path) {
            motor::identity::mark_skipped(&path);
            out.push_view_ref(&path);
            return;
        }

        // O body vai rodar: as leituras antigas desta view caem (o conjunto
        // novo é o que este body registrar) e os efeitos que ele empurrar
        // pertencem à entry nova.
        motor::identity::mark_reran(&path);
        motor::identity::begin_view_reads(&path);
        crate::reconciler::begin_entry(&path);
        let mut body = NodeList::new();
        self.clone().body(ctx).render_into(ctx, &mut body);
        let (print_children, layout_children) = body.into_parts();
        let node = RenderNode::branch(short_type_name::<T>(), print_children);
        let boundary = crate::layout::LayoutNode::Boundary {
            path: path.clone(),
            children: layout_children,
        };
        crate::reconciler::finish_entry(
            &path,
            crate::erased::erased(self.clone()),
            ctx.clone(),
            node,
            boundary,
        );
        out.push_view_ref(&path);
    }
}

/// SwiftUI's `_ConditionalContent` — o `if/else` de um `@ViewBuilder`.
/// Os dois ramos precisam da mesma aridade (senão `.padding()` no `Either`
/// significaria coisas diferentes por ramo). Para `match` com mais braços,
/// use os [`OneOf3`]…[`OneOf8`].
///
/// [`OneOf3`]: crate::one_of::OneOf3
/// [`OneOf8`]: crate::one_of::OneOf8
#[derive(Clone)]
pub enum Either<A, B> {
    First(A),
    Second(B),
}

impl<A: View, B: View<Arity = A::Arity>> View for Either<A, B> {
    type Arity = A::Arity;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        // O braço entra na identidade: trocar de ramo desmonta o que o
        // outro ramo montou (estado de views sob o braço morre junto).
        match self {
            Either::First(view) => {
                let _frame = motor::identity::enter("@First");
                view.render_into(ctx, out);
            }
            Either::Second(view) => {
                let _frame = motor::identity::enter("@Second");
                view.render_into(ctx, out);
            }
        }
    }
}

/// SwiftUI's optional-view handling: `country.flag.map { … }` renders the
/// content when the optional has a value, nothing when it doesn't — zero
/// nós, para não deslocar o conector `└─` do irmão anterior (o `Vec`
/// condicional do motor também não deixava placeholder).
impl<C: View> View for Option<C> {
    type Arity = Many;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        if let Some(view) = self {
            view.render_into(ctx, out);
        }
    }
}

/// SwiftUI's `TupleView` — the implicit container of a multi-statement
/// `@ViewBuilder` block, achatado nos filhos do pai.
macro_rules! tuple_view {
    ($($name:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($name: View),+> View for ($($name,)+) {
            type Arity = Many;

            fn render_into(&self, ctx: &Context, out: &mut NodeList) {
                let ($(ref $name,)+) = *self;
                // A posição na tupla é identidade estrutural: dois irmãos do
                // mesmo tipo não se confundem, e `Option` vazio não desloca
                // os índices (a estrutura é estática, não os nós emitidos).
                let mut position = 0usize;
                $(
                    {
                        let _frame = motor::identity::enter(format!("#{position}"));
                        $name.render_into(ctx, out);
                        position += 1;
                    }
                )+
                let _ = position;
            }
        }
    };
}

tuple_view!(A);
tuple_view!(A, B);
tuple_view!(A, B, C);
tuple_view!(A, B, C, D);
tuple_view!(A, B, C, D, E);
tuple_view!(A, B, C, D, E, F);
tuple_view!(A, B, C, D, E, F, G);
tuple_view!(A, B, C, D, E, F, G, H);
tuple_view!(A, B, C, D, E, F, G, H, I);
tuple_view!(A, B, C, D, E, F, G, H, I, J);
tuple_view!(A, B, C, D, E, F, G, H, I, J, K);
tuple_view!(A, B, C, D, E, F, G, H, I, J, K, L);

/// Renders a view to its single printed line (used by inert modifiers that
/// describe content they never mount: `.background { … }`,
/// `NavigationLink(destination:)`). Expande referências: uma fronteira
/// retida descreve pela linha do cache, não pelo marcador.
pub(crate) fn render_line(view: &impl View) -> String {
    let mut out = NodeList::new();
    view.render_into(&Context::default(), &mut out);
    out.nodes()
        .first()
        .map(|node| crate::reconciler::expand(node).line)
        .unwrap_or_default()
}

pub(crate) fn short_type_name<T: ?Sized>() -> String {
    let full = std::any::type_name::<T>();
    // genéricos: `path::DetailRow<bunny_ui::views::Text>` → `DetailRow`
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base).to_string()
}
