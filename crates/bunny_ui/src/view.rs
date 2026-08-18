//! `View` — the typed tree.
//!
//! There is no `AnyView` here: each view is a generic value (`VStack<C>` with
//! tuple children, `Button<L, F>` with the action in a field), dispatch is
//! static end to end and no `Rc` is born during render. Erasure
//! exists only at the borders of real dynamism — [`Erased`] (sheets,
//! `ViewModifier`), the effect queue and the per-site slots.
//!
//! `render_into` appends nodes to a [`NodeList`] (a tuple appends several,
//! `EmptyView`/`None` none) — the flattening SwiftUI's `@ViewBuilder`
//! does in codegen. The `NodeList` is opaque on purpose: the output
//! format is engine detail, and will change when it stops being headless.
//!
//! [`Erased`]: crate::erased::Erased

use motor::state::Context;
use motor::view::RenderNode;

/// How many nodes a view appends — part of the type, so the compiler blocks
/// what makes no sense. Modifying `(a, b).padding()` would decorate only `b`;
/// with `Arity` it does not even compile: [`ViewExt`] demands `Arity = Single`.
/// (To truly group, [`tuple`] prints its own node — and then accepts a
/// modifier.)
///
/// [`ViewExt`]: crate::ext::ViewExt
/// [`tuple`]: crate::views::tuple
pub struct Single;

/// The other side of [`Single`]: zero-or-several nodes (tuples, `Option`).
pub struct Many;

/// SwiftUI's `View`: a renderable blueprint. Built-ins implement
/// `render_into` directly; user views get it for free from
/// [`Component::body`].
///
/// Do not implement `View` by hand: the render contract is internal and will
/// change together with the engine. Your own views implement [`Component`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a View",
    label = "a view is expected here",
    note = "implement `Component` (the `fn body`) for your own views — never `View` directly",
    note = "put many children in one tuple: `vstack((a, b, c))` — a tuple holds up to 12; nest tuples for more"
)]
pub trait View: Clone + 'static {
    /// [`Single`] when the view appends exactly one node; [`Many`] when it
    /// can append zero or several. Modifiers only exist for `Single`.
    type Arity;

    /// Appends this view's nodes to `out` (a tuple appends several,
    /// `EmptyView` and `None` append nothing that prints).
    #[doc(hidden)]
    fn render_into(&self, ctx: &Context, out: &mut NodeList);
}

/// Shorthand for "view of exactly one node" in helper signatures:
/// `-> impl UnaryView`. A `-> impl View` would hide the arity (opaque types
/// only reveal what the signature promises) and the callsite could neither
/// apply a modifier nor enter a `OneOf` arm.
#[diagnostic::on_unimplemented(
    message = "`{Self}` can render many nodes — exactly one is required here",
    note = "to decorate a group, wrap it with `tuple(…)` — the wrapper has its own node"
)]
pub trait UnaryView: View<Arity = Single> {}

impl<V: View<Arity = Single>> UnaryView for V {}

/// The render output — opaque outside the crate (mutators are `pub(crate)`):
/// implementing `View` from outside even compiles, but produces no node at
/// all — the polite way of saying "implement `Component`".
///
/// Carries the TWO outputs of the single body-eval per pass: the printed
/// tree (`RenderNode`) and the layout tree ([`LayoutNode`]) — evaluating the
/// body twice would duplicate identity anchors, so print and layout come
/// out together.
///
/// [`LayoutNode`]: crate::layout::LayoutNode
#[derive(Default)]
pub struct NodeList {
    nodes: Vec<RenderNode>,
    layout: Vec<crate::layout::LayoutNode>,
}

thread_local! {
    /// The printed tree turns on and off per pass: printing is for people
    /// (tests, the incremental-vs-full oracle); the FRAME path
    /// (settle/layout/paint) turns it off and the lines are not even
    /// formatted — per-node suffix `format!` was real frame cost.
    static PRINT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Does the current pass build the printed tree? Hot `format!` sites
/// check before formatting.
pub(crate) fn print_enabled() -> bool {
    PRINT.with(|print| print.get())
}

pub(crate) fn set_print(enabled: bool) {
    PRINT.with(|print| print.set(enabled));
}

/// A box of nothing: zero on both axes, no paint, no hit — what a
/// modifier wraps when the view below it has no geometry at all.
fn nothing_node() -> crate::layout::LayoutNode {
    crate::layout::LayoutNode::Stack {
        axis: crate::layout::Axis::Vertical,
        spacing: 0.0,
        align: crate::layout::CrossAlign::Start,
        children: Vec::new(),
    }
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

    /// Wraps the last layout node (the one from the Single view that just
    /// rendered) — the path of the layout modifiers (`.padding()`, frames).
    /// How many layout nodes are in hand — the MARK a modifier takes
    /// before its base renders, so it can tell what the base added
    /// from what a SIBLING left behind.
    pub(crate) fn layout_mark(&self) -> usize {
        self.layout.len()
    }

    /// Wraps what the base contributed since `mark`.
    ///
    /// The base is a Single view, so it left one node — or NONE at all
    /// (`empty()` has no geometry). The old reading was "wrap the last
    /// node", which in a stack reached PAST the base and stole the
    /// previous sibling's box. Now: nothing added means the modifier
    /// wraps a BOX OF NOTHING, which is how
    /// `empty().frame(w, h).background_color(c)` becomes a real painted
    /// box — SwiftUI's own answer, where the frame IS a view and
    /// `EmptyView` merely fills none of it. Bare `empty()` still
    /// contributes nothing to a stack: only a modifier mints the box.
    pub(crate) fn wrap_layout_from(
        &mut self,
        mark: usize,
        wrap: impl FnOnce(crate::layout::LayoutNode) -> crate::layout::LayoutNode,
    ) {
        let base = if self.layout.len() > mark {
            self.layout.remove(mark)
        } else {
            nothing_node()
        };
        self.layout.insert(mark, wrap(base));
    }

    /// A retained boundary: enters as a reference (the marked line in the
    /// print, the reference node in the layout) and the final assembly
    /// expands against the reconciler.
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
/// return-position `impl Trait` in trait, stable since Rust 1.75. The
/// concrete type of the whole tree is known at compile time.)
///
/// The body takes `self` BY VALUE on purpose: closures capture the
/// fields they use (Rust 2021 disjoint capture) — `move ||
/// self.count.add(1)` just works, without the `let this = *self` the
/// `&self` form demanded. Views are cheap values (`State` is Copy); the
/// runtime clones before calling.
pub trait Component: Clone + 'static {
    fn body(self, ctx: &Context) -> impl View;
}

impl<T: Component> View for T {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        // The frame covers the body construction AND the descent: every
        // `State::new` fired by the children's constructors anchors to this identity.
        let _frame = motor::identity::enter_view(short_type_name::<T>());

        // No active pass (render outside the Runtime): direct path, no
        // retention — the pre-reconciler behavior.
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

        // A clean, retained boundary, outside any re-running body: the
        // body does NOT run — a reference goes out and the cache answers for it.
        if let crate::reconciler::Decision::Skip = crate::reconciler::decide(&path) {
            motor::identity::mark_skipped(&path);
            out.push_view_ref(&path);
            return;
        }

        // The body will run: this view's old reads drop (the new set is
        // whatever this body registers) and the effects it pushes
        // belong to the new entry.
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

/// SwiftUI's `_ConditionalContent` — the `if/else` of a `@ViewBuilder`.
/// Both branches need the same arity (otherwise `.padding()` on the `Either`
/// would mean different things per branch). For `match` with more arms,
/// use [`OneOf3`]…[`OneOf8`].
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
        // The arm joins the identity: switching branches unmounts what the
        // other branch mounted (state of views under the arm dies with it).
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
/// nodes, so the previous sibling's `└─` connector does not shift (the
/// engine's conditional `Vec` left no placeholder either).
impl<C: View> View for Option<C> {
    type Arity = Many;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        if let Some(view) = self {
            view.render_into(ctx, out);
        }
    }
}

/// SwiftUI's `TupleView` — the implicit container of a multi-statement
/// `@ViewBuilder` block, flattened into the parent's children.
macro_rules! tuple_view {
    ($($name:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($name: View),+> View for ($($name,)+) {
            type Arity = Many;

            fn render_into(&self, ctx: &Context, out: &mut NodeList) {
                let ($(ref $name,)+) = *self;
                // The tuple position is structural identity: two siblings of
                // the same type do not get confused, and an empty `Option` does
                // not shift the indices (the structure is static, not the emitted nodes).
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
/// `NavigationLink(destination:)`). Expands references: a retained
/// boundary describes through the cache line, not the marker.
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
    // generics: `path::DetailRow<bunny_ui::views::Text>` → `DetailRow`
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base).to_string()
}
