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

// MARK: - Rendering a component without stacking its payload
//
// A body takes `self` BY VALUE, so the payload is copied once per
// level. That is the contract and it is fine. What is NOT fine is the
// copy being made in the frame of the RECURSION: a debug build gives
// every local its own slot for the whole function, live or not, so a
// copy named in `render_into` is stack that every descendant of that
// view pays for. A shell with a hundred `State`s and a tree twenty
// deep turns a few kilobytes into megabytes, and the thread runs out
// before a screen is even mounted.
//
// The cure is boring and it is all here: anything holding a copy of
// the payload gets its OWN frame, one that pops before the descent
// begins or opens after it ends. The payload is still copied — it is
// just never copied ON TOP of itself.

/// Runs a component's body from a BORROW.
///
/// The copy the body consumes lives in THIS frame, which pops the
/// moment the body hands its view back — before a single child
/// renders. Written at the call site instead, the same copy would
/// reserve its slot in the caller's frame for the whole subtree.
#[inline(never)]
fn run_body<'a, T: Component>(view: &T, ctx: &'a Context) -> impl View + use<'a, T> {
    view.clone().body(ctx)
}

/// Files the finished body under its identity.
///
/// Everything here runs AFTER the descent and needs a slot for nothing
/// during it — including the second copy of the payload, the one the
/// retention keeps so a skipped view can answer from cache.
#[inline(never)]
fn retain_entry<T: Component>(view: &T, ctx: &Context, path: &str, body: NodeList) {
    let (print_children, layout_children) = body.into_parts();
    crate::reconciler::finish_entry(
        path,
        crate::erased::erased_from(view),
        ctx.clone(),
        RenderNode::branch(short_type_name::<T>(), print_children),
        crate::layout::LayoutNode::Boundary {
            path: path.to_string(),
            children: layout_children,
        },
    );
}

/// The same tail, for a render with no pass around it: nothing is
/// retained, so the two lists go straight out.
///
/// It earns its own frame for a second reason. A debug frame reserves
/// room for the locals of every BRANCH, taken or not, so this one was
/// costing the retained path too — on every level.
#[inline(never)]
fn close_loose<T: Component>(body: NodeList, out: &mut NodeList) {
    let (print_children, layout_children) = body.into_parts();
    out.push(RenderNode::branch(short_type_name::<T>(), print_children));
    out.push_layout(crate::layout::LayoutNode::Boundary {
        path: short_type_name::<T>(),
        children: layout_children,
    });
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
            run_body(self, ctx).render_into(ctx, &mut body);
            close_loose::<T>(body, out);
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
        run_body(self, ctx).render_into(ctx, &mut body);
        retain_entry(self, ctx, &path, body);
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

/// A `Vec` of views flattens into its parent the way a tuple does — the
/// answer to a list whose length the source cannot spell, and to the
/// twelfth tuple running out.
///
/// ```ignore
/// vstack(rows.into_iter().map(card).collect::<Vec<_>>())
/// ```
///
/// Identity is POSITIONAL, exactly as in a tuple: item `i` owns `#i`.
/// That is the honest reading of a bare `Vec` — it carries no names —
/// but it means an insertion in the middle re-associates every state
/// below it with a different item. Where the items have identities of
/// their own (rows of a table, a list that reorders), use
/// [`crate::views::for_each`] and its `id` closure: that is what keeps a
/// row's state, its animation and its caret with the row.
impl<C: View> View for Vec<C> {
    type Arity = Many;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        for (position, view) in self.iter().enumerate() {
            let _frame = motor::identity::enter(format!("#{position}"));
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use motor::state::State;

    use crate::state_ext::StateExt as _;

    use crate::layout::{Proposal, Size};
    use crate::runtime::Runtime;
    use crate::views::{text, vstack};

    use super::*;

    thread_local! {
        static BASE: Cell<usize> = const { Cell::new(0) };
        static DEEPEST: Cell<usize> = const { Cell::new(usize::MAX) };
    }

    /// Where this frame sits. A LOCAL, kept opaque — `&0u8` is a
    /// promoted constant and lives in the binary, never on the stack.
    #[inline(never)]
    fn here() -> usize {
        let anchor = 0u8;
        std::hint::black_box(&anchor) as *const u8 as usize
    }

    #[inline(never)]
    fn mark() {
        let now = here();
        DEEPEST.with(|deepest| deepest.set(deepest.get().min(now)));
    }

    #[derive(Clone, Copy)]
    struct Payload<const N: usize> {
        states: [State<u64>; N],
    }

    impl<const N: usize> Payload<N> {
        fn new() -> Payload<N> {
            Payload { states: [(); N].map(|_| State::new(0u64)) }
        }

        /// A read, so the payload is not weight the compiler may drop.
        fn sum(&self) -> u64 {
            self.states.iter().map(|state| state.get()).sum()
        }
    }

    /// One level of a tree that carries its whole payload BY VALUE —
    /// the shape an app's shell has when every screen hangs off it.
    #[derive(Clone, Copy)]
    struct Level<const N: usize> {
        payload: Payload<N>,
        left: usize,
    }

    impl<const N: usize> Component for Level<N> {
        fn body(self, _ctx: &Context) -> impl View {
            mark();
            let deeper =
                (self.left > 0).then(|| Level { payload: self.payload, left: self.left - 1 });
            vstack((text(format!("level {} of {}", self.left, self.payload.sum())), deeper))
        }
    }

    /// Stack bytes ONE level of the recursion costs, for a payload of
    /// `N` states. A stack overflow aborts the process, so the number
    /// is the high-water mark and never "did it survive".
    fn per_level<const N: usize>(depth: usize) -> usize {
        // its own thread: a generous stack, and the identity arenas are
        // thread-local, so a measurement never reads another's world
        std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                let runtime = Runtime::new();
                let root = Level { payload: Payload::<N>::new(), left: depth };
                let base = here();
                BASE.with(|cell| cell.set(base));
                DEEPEST.with(|cell| cell.set(base));
                let _ = runtime
                    .settled_layout(&root, Proposal::exact(Size { width: 900.0, height: 700.0 }));
                let deepest = DEEPEST.with(|cell| cell.get());
                (base - deepest) / depth
            })
            .expect("a thread for the probe")
            .join()
            .expect("the probe finished")
    }

    /// How many times a component's payload is re-materialized per
    /// level of the render recursion.
    ///
    /// A body takes `self` BY VALUE, so ONE copy per level is the
    /// contract: the view a body returns holds the children it built,
    /// and those children carry what they were given. Every copy above
    /// that is accident, and accident is what puts a ceiling on how
    /// deep a tree with a fat shell may go — a debug frame reserves
    /// room for every local it declares, live or not, so anything the
    /// recursive frame names is stack that every descendant pays for.
    ///
    /// Measured as a DIFFERENCE between two payload sizes, which
    /// cancels the framework's own fixed cost per level and most of
    /// what the machine and the compiler contribute. The bound sits
    /// between what this pass costs (2.1) and what a single copy
    /// coming back costs (3.2), so it catches one — and it is not a
    /// number to tune: a release build elides the copies and answers
    /// near zero, which passes.
    #[test]
    fn a_payload_is_not_re_materialized_all_the_way_down() {
        const DEPTH: usize = 40;
        let slim = per_level::<50>(DEPTH);
        let fat = per_level::<150>(DEPTH);
        let added = size_of::<Payload<150>>() - size_of::<Payload<50>>();
        let copies = fat.saturating_sub(slim) as f64 / added as f64;
        assert!(
            copies <= 3.0,
            "a payload is copied {copies:.2}× per level \
             (slim {slim} B, fat {fat} B, {added} B of payload apart) — \
             a copy of `self` is being named in the recursive frame again",
        );
    }
}
