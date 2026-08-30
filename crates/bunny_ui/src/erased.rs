//! The erased boundary — where the dynamism is real.
//!
//! `Erased` is this layer's `AnyView`, but it exists in exactly three
//! places: the content of a `.sheet` (a closure that mounts the view
//! only when presented — `Fn() -> impl View` cannot be written), the
//! `content:` of a [`CustomModifier`], and the band a
//! [`table`](crate::views::table) hands its row wrapper — a closure the
//! app writes and the table stores, which is the same shape again. All
//! the rest of the tree is typed.

use std::rc::Rc;

use motor::state::Context;

use crate::view::{NodeList, Single, View};

pub(crate) trait ErasedDyn {
    fn render_into_dyn(&self, ctx: &Context, out: &mut NodeList);
}

impl<V: View> ErasedDyn for V {
    fn render_into_dyn(&self, ctx: &Context, out: &mut NodeList) {
        self.render_into(ctx, out);
    }
}

/// Type erasure over the typed tree — one `Rc` per boundary, not per
/// node. Only erases one-node views (`Arity = Single`): erasure cannot
/// hide arity, or `.blur(…)` on an erased `content:` would go back to
/// decorating the wrong node.
#[derive(Clone)]
pub struct Erased(Rc<dyn ErasedDyn>);

impl Erased {
    pub fn new<V: View<Arity = Single>>(view: V) -> Self {
        Erased(Rc::new(view))
    }
}

impl View for Erased {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        self.0.render_into_dyn(ctx, out);
    }
}

/// Erases a view to cross a dynamic boundary (the `AnyView::new` that
/// looks like an ordinary constructor — how sheet closures end).
pub fn erased<V: View<Arity = Single>>(view: V) -> Erased {
    Erased::new(view)
}

/// The same, from a BORROW — the door the render walk uses.
///
/// The copy is made INSIDE this frame, which opens and closes on its
/// own. A clone spelt at the call site would instead reserve its slot
/// in the caller's frame, and the caller here is one level of the
/// render recursion: a debug build keeps every local's slot for the
/// whole function, so that copy would be stack every descendant pays.
#[inline(never)]
pub fn erased_from<V: View<Arity = Single> + Clone>(view: &V) -> Erased {
    Erased(Rc::new(view.clone()))
}

/// `ViewModifier` / `EnvironmentalModifier` — Swift's
/// `func body(content:)`, re-applied on every render (it is what makes
/// `RootViewAppearance`'s `blur(radius:)` recompute when `isActive`
/// moves).
pub trait CustomModifier {
    fn name(&self) -> String;
    fn apply(&self, ctx: &Context, content: Erased) -> Erased;
}
