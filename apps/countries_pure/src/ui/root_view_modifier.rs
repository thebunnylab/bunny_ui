//
//  RootViewModifier.swift — RootViewAppearance
//
//  `ViewModifier` → `CustomModifier`: o `apply` re-roda a cada render, que é
//  o que faz o `blur(radius:)` recomputar quando o `isActive` anda (numa
//  cadeia de modifiers comum o detalhe seria assado no build da árvore).
//
//  Nota: o `on_receive` do bunny_ui retém a subscription por site, então o
//  blur de verdade cai pra zero quando `sceneDidBecomeActive` seta
//  `system.isActive` — sem a retenção, o publisher recriado a cada render
//  entregaria o valor atual de novo e a entrega nunca estabilizaria.
//
//  O `content:` chega apagado (`Erased`) — é a borda dinâmica do
//  `body(content:)`; o wrap que ele devolve volta a ser tipado até a
//  próxima borda.
//

use countries_core::DependencyInjection::DIContainer::DIContainer;
use bunny_ui::prelude::*;

/// `struct RootViewAppearance: ViewModifier`
#[derive(Clone)]
pub struct RootViewAppearance {
    is_active: State<bool>,
    state_update: AnyPublisher<bool>,
}

impl RootViewAppearance {
    /// `RootViewAppearance()` — o `@Environment(\.injected)` chega aqui como
    /// parâmetro (o modifier vive fora da subárvore que o `.inject` rega).
    pub fn new(injected: &DIContainer) -> Self {
        Self {
            is_active: State::new(false),
            state_update: injected.appState.updates(|state| state.system.isActive),
        }
    }
}

impl CustomModifier for RootViewAppearance {
    fn name(&self) -> String {
        "RootViewAppearance".into()
    }

    /// `body(content:)`
    fn apply(&self, _ctx: &Context, content: Erased) -> Erased {
        let is_active = self.is_active;
        erased(
            content
                .blur(if self.is_active.get() { 0.0 } else { 10.0 })
                .ignores_safe_area()
                .on_receive(self.state_update.clone(), move |active| {
                    is_active.set(active)
                }),
        )
    }
}
