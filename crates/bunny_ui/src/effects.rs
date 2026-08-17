//! Effects — the registry collected on render and drained by the pump.
//!
//! The motor's `ctx.effects` is `pub(crate)`; this layer keeps its own
//! thread-local registry with the same semantics (the "main actor" is
//! single-threaded by design). The effect builders carry the motor's
//! exact logic, with `on_receive`'s per-site retention.
//!
//! The [`Site`] arrives ready from `ViewExt` — the `#[track_caller]`
//! callsite on the common path, an explicit name in the `_keyed` variants.

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

/// An effect registered during render goes to the reconciler: it enters
/// the entry of the boundary under construction (and re-pumps from it
/// while the view stays mounted, running or skipped), or the root's
/// region.
pub(crate) fn push(effect: EffectFn) {
    crate::reconciler::attribute_effect(effect);
}

/// The pass's queue, reassembled by the runtime from the retention.
pub(crate) fn set_queue(effects: Vec<EffectFn>) {
    EFFECTS.with(|queue| *queue.borrow_mut() = effects);
}

pub(crate) fn take() -> Vec<EffectFn> {
    EFFECTS.with(|effects| std::mem::take(&mut *effects.borrow_mut()))
}

/// `.onChange(of:initial:)` — the per-(site, identity) slot learns the
/// value and only fires when it moves. The slot resolves at CONSTRUCTION
/// (the identity cursor only exists during render); the pump already
/// receives the cell.
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

/// `.onReceive(publisher)` — with per-(site, identity) retention: the
/// first publisher lives in a slot and the ones recreated by each
/// `body()` are ignored (the subscription's dedup is the retained one's
/// shared `last`). Without it, every re-render would create a zeroed-cell
/// publisher that would deliver the current value again — the pump would
/// report a change every cycle and `render_stable` would exit by
/// exhaustion.
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
