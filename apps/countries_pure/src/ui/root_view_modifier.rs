//
//  RootViewModifier.swift — RootViewAppearance
//
//  `ViewModifier` → `CustomModifier`: the `apply` re-runs on every render,
//  which is what makes the `blur(radius:)` recompute when `isActive` moves
//  (in a plain modifier chain the detail would be baked into the tree build).
//
//  Note: bunny_ui's `on_receive` retains the subscription per site, so the
//  blur really does drop to zero when `sceneDidBecomeActive` sets
//  `system.isActive` — without the retention, the publisher recreated on
//  every render would deliver the current value again and the delivery
//  would never stabilize.
//
//  The `content:` arrives erased (`Erased`) — it is the dynamic boundary of
//  `body(content:)`; the wrap it returns goes back to being typed until the
//  next boundary.
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
    /// `RootViewAppearance()` — the `@Environment(\.injected)` arrives here
    /// as a parameter (the modifier lives outside the subtree that `.inject`
    /// waters).
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
