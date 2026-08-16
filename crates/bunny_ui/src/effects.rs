//! Efeitos — o registro coletado no render e drenado pelo pump.
//!
//! O `ctx.effects` do motor é `pub(crate)`; esta camada mantém o próprio
//! registro thread-local com a mesma semântica (o "main actor" é
//! single-threaded por design). Os builders de efeito carregam a lógica
//! exata do motor, com a retenção por site do `on_receive`.
//!
//! O [`Site`] chega pronto da `ViewExt` — o callsite de `#[track_caller]`
//! no caminho comum, um nome explícito nas variantes `_keyed`.

use std::cell::RefCell;
use std::rc::Rc;

use motor::combine::AnyPublisher;
use motor::identity::scoped_effect_slot;
use motor::runtime::Site;
use motor::state::{Context, EffectFn};

thread_local! {
    static EFFECTS: RefCell<Vec<EffectFn>> = RefCell::new(Vec::new());
}

pub(crate) fn reset() {
    EFFECTS.with(|effects| effects.borrow_mut().clear());
}

/// Um efeito registrado durante o render vai para o reconciler: entra na
/// entry da fronteira em construção (e re-bombeia dela enquanto a view
/// estiver montada, rodando ou pulada), ou na região do root.
pub(crate) fn push(effect: EffectFn) {
    crate::reconciler::attribute_effect(effect);
}

/// A fila do pass, remontada pelo runtime a partir da retenção.
pub(crate) fn set_queue(effects: Vec<EffectFn>) {
    EFFECTS.with(|queue| *queue.borrow_mut() = effects);
}

pub(crate) fn take() -> Vec<EffectFn> {
    EFFECTS.with(|effects| std::mem::take(&mut *effects.borrow_mut()))
}

/// `.onChange(of:initial:)` — o slot por (site, identidade) aprende o valor
/// e só dispara quando ele anda. O slot resolve na CONSTRUÇÃO (o cursor de
/// identidade só existe durante o render); o pump já recebe a célula.
pub fn change_effect<V, OF, AC>(site: Site, of: OF, initial: bool, action: AC) -> EffectFn
where
    V: Clone + PartialEq + 'static,
    OF: Fn() -> V + 'static,
    AC: Fn(&V, &V) + 'static,
{
    let cell = scoped_effect_slot::<V>(site);
    Rc::new(move |_ctx: &Context| {
        let value = of();
        let mut previous = cell.borrow_mut();
        match previous.take() {
            None => {
                *previous = Some(value.clone());
                if initial {
                    let old = value.clone();
                    action(&old, &value);
                    true
                } else {
                    false
                }
            }
            Some(old) if old != value => {
                *previous = Some(value.clone());
                action(&old, &value);
                true
            }
            Some(old) => {
                *previous = Some(old);
                false
            }
        }
    })
}

/// `.onReceive(publisher)` — com retenção por (site, identidade): o
/// primeiro publisher vive num slot e os recriados por cada `body()` são
/// ignorados (a dedup da subscription é o `last` compartilhado do retido).
/// Sem isso, cada re-render criaria um publisher de célula zerada que
/// entregaria o valor atual de novo — o pump reportaria mudança a cada
/// ciclo e o `render_stable` sairia por exaustão.
pub fn receive_effect<V, AC>(site: Site, publisher: AnyPublisher<V>, action: AC) -> EffectFn
where
    V: Clone + PartialEq + 'static,
    AC: Fn(V) + 'static,
{
    let cell = scoped_effect_slot::<AnyPublisher<V>>(site);
    let retained = {
        let mut slot = cell.borrow_mut();
        let retained = slot.take().unwrap_or(publisher);
        *slot = Some(retained.clone());
        retained
    };
    Rc::new(move |_ctx: &Context| match retained.poll() {
        Some(value) => {
            action(value);
            true
        }
        None => false,
    })
}
