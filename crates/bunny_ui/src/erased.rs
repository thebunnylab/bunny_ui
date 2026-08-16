//! A borda apagada — onde o dinamismo é de verdade.
//!
//! `Erased` é o `AnyView` desta camada, mas existe em exatamente dois
//! lugares: o conteúdo de uma `.sheet` (closure que monta view só quando
//! apresentada — `Fn() -> impl View` não se escreve) e o `content:` de um
//! [`CustomModifier`]. Todo o resto da árvore é tipada.

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

/// Type erasure sobre a árvore tipada — um `Rc` por borda, não por nó.
/// Só apaga views de um nó (`Arity = Single`): o apagamento não pode
/// esconder aridade, senão `.blur(…)` num `content:` apagado voltaria a
/// decorar o nó errado.
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

/// Apaga uma view para atravessar uma borda dinâmica (o `AnyView::new`
/// com cara de construtor comum — fim das closures de sheet).
pub fn erased<V: View<Arity = Single>>(view: V) -> Erased {
    Erased::new(view)
}

/// `ViewModifier` / `EnvironmentalModifier` — o `func body(content:)` do
/// Swift, re-aplicado a cada render (é o que faz o `blur(radius:)` do
/// `RootViewAppearance` recomputar quando o `isActive` anda).
pub trait CustomModifier {
    fn name(&self) -> String;
    fn apply(&self, ctx: &Context, content: Erased) -> Erased;
}
