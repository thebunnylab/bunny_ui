//! Effects — the registry collected on render and drained by the pump.
//!
//! The motor's `ctx.effects` is `pub(crate)`; this layer keeps its own
//! thread-local registry with the same semantics (the "main actor" is
//! single-threaded by design). The effect builders carry the motor's
//! exact logic, with `on_receive`'s per-site retention.
//!
//! The [`Site`] arrives ready from `ViewExt` — the `#[track_caller]`
//! callsite on the common path, an explicit name in the `_keyed` variants.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use motor::combine::AnyPublisher;
use motor::identity::scoped_effect_slot;
use motor::runtime::Site;
use motor::state::{Context, EffectFn};

thread_local! {
    static EFFECTS: RefCell<Vec<EffectFn>> = RefCell::new(Vec::new());
    /// Every `.task` slot ever opened on this thread, weakly: the
    /// identity owns the cell, this list only watches it.
    static TASK_CELLS: RefCell<Vec<Weak<RefCell<Option<TaskSlot>>>>> =
        RefCell::new(Vec::new());
    /// The pass counter the sweep compares against.
    static GENERATION: Cell<u64> = const { Cell::new(1) };
    /// Did THIS pass assemble its queue? A pass with no root declares
    /// nothing, and sweeping on it would cancel every live task.
    static DECLARED: Cell<bool> = const { Cell::new(false) };
}

/// What a `.task` keeps in its identity slot: which id is running, its
/// handle, and the pass that last DECLARED it.
type TaskSlot = (Option<String>, motor::task::Spawned, u64);

pub(crate) fn reset() {
    EFFECTS.with(|effects| effects.borrow_mut().clear());
    DECLARED.with(|declared| declared.set(false));
}

/// An effect registered during render goes to the reconciler: it enters
/// the entry of the boundary under construction (and re-pumps from it
/// while the view stays mounted, running or skipped), or the root's
/// region.
pub(crate) fn push(effect: EffectFn) {
    crate::reconciler::attribute_effect(effect);
}

/// The pass's queue, reassembled by the runtime from the retention.
/// Assembling it IS the declaration: what is not in here — a branch
/// that closed, a row that left — no longer belongs to the scene.
pub(crate) fn set_queue(effects: Vec<EffectFn>) {
    EFFECTS.with(|queue| *queue.borrow_mut() = effects);
    DECLARED.with(|declared| declared.set(true));
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

/// `.task { await … }` — the future starts when the view appears and
/// dies WITH it. The handle lives in the per-(site, identity) slot, and
/// the identity sweep drops that slot when the view leaves the tree;
/// dropping the handle cancels the task, so no sweep of our own exists.
///
/// With an `id`, an id that moved cancels what runs and starts fresh
/// (SwiftUI's `.task(id:)`). The factory is `Fn` because of that
/// restart: what the future needs to own, it creates inside itself.
pub fn task_effect<F, Fut>(site: Site, id: Option<String>, start: F) -> EffectFn
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let cell = scoped_effect_slot::<TaskSlot>(site);
    watch_task(&cell);
    Rc::new(move |_ctx: &Context| {
        let generation = GENERATION.with(Cell::get);
        let running = matches!(
            cell.borrow().as_ref(),
            Some((current, _, _)) if *current == id
        );
        if running {
            // a re-render never restarts what is already running — it
            // only says the task is still part of the scene
            if let Some(slot) = cell.borrow_mut().as_mut() {
                slot.2 = generation;
            }
            return false;
        }
        let started = (id.clone(), motor::task::spawn(start()), generation);
        // out of the cell before it drops: cancelling runs the future's
        // own Drop, which must not find this slot borrowed
        let previous = cell.borrow_mut().replace(started);
        drop(previous);
        // nothing observable changed YET — the frame loop keeps going
        // because the queue now has something ready to poll
        false
    })
}

/// Starts watching a task slot (once per cell — the modifier is rebuilt
/// on every render and hands back the same one).
fn watch_task(cell: &Rc<RefCell<Option<TaskSlot>>>) {
    TASK_CELLS.with(|cells| {
        let mut cells = cells.borrow_mut();
        let known = cells
            .iter()
            .any(|weak| weak.upgrade().is_some_and(|other| Rc::ptr_eq(&other, cell)));
        if !known {
            cells.push(Rc::downgrade(cell));
        }
    });
}

/// Cancels the tasks the pass did NOT declare. A view that left the
/// tree stops pushing its effect, so its slot keeps an older pass
/// number — and a subprocess never outlives the panel that asked for
/// it. A skipped subtree still declares (the reconciler reassembles its
/// effects from the retention), so skipping never cancels anything.
pub(crate) fn sweep_tasks() {
    if !DECLARED.with(Cell::get) {
        // no queue was assembled: this pass declared nothing, and
        // "nothing declared" is not the same as "everything died"
        return;
    }
    let generation = GENERATION.with(Cell::get);
    TASK_CELLS.with(|cells| {
        let mut cells = cells.borrow_mut();
        cells.retain(|weak| {
            // the cell is gone = the identity swept it, and the handle
            // it held cancelled on the way out
            let Some(cell) = weak.upgrade() else { return false };
            let stale = matches!(
                cell.borrow().as_ref(),
                Some((_, _, seen)) if *seen != generation
            );
            if stale {
                let previous = cell.borrow_mut().take();
                drop(previous);
            }
            true
        });
    });
    GENERATION.with(|counter| counter.set(generation + 1));
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
