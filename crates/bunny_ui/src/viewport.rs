//! The pre-pass snapshot a virtualized body reads: the RETAINED
//! geometry of each virtual scroll region — last frame's offset,
//! viewport and measured row extent, keyed by region path.
//!
//! The runtime publishes it right before every pass; a `virtual_list`
//! body asks for its own region by the cursor scope and computes the
//! window from it. One frame of lag by construction, masked by the
//! window's buffer — and when a wheel outruns the buffer, the place
//! phase reports a miss and the runtime re-runs the body with fresh
//! numbers in the same frame.

use std::cell::RefCell;
use std::rc::Rc;

use crate::layout::Px;
use motor::hash::FxHashMap;

/// What the body needs for the window math, all from the LAST frame.
#[derive(Clone, Default)]
pub(crate) struct RegionSnapshot {
    pub offset_y: Px,
    pub viewport: Px,
    pub row_extent: Px,
    /// Variable-height regions: last frame's prefix-sum offsets
    /// (`offsets[i]` = row `i`'s start; last entry = total). Valid for
    /// the window math only while `len == count + 1` — a count that
    /// changed falls back to the first-frame window and heals by miss.
    pub offsets: Option<Rc<Vec<Px>>>,
    /// The scroll target the runtime already APPLIED for this region —
    /// a reveal equal to it is settled history, not a pending jump,
    /// and must not fight the wheel for the window.
    pub applied: Option<String>,
}

thread_local! {
    static SNAPSHOT: RefCell<FxHashMap<String, RegionSnapshot>> =
        RefCell::new(FxHashMap::default());
    /// The rows of every list that measures its own, by region — kept
    /// across frames, which is the whole point: a row measured once is
    /// counted at what it really is from then on.
    static ROWS: RefCell<FxHashMap<String, Rc<crate::layout::RowCache>>> =
        RefCell::new(FxHashMap::default());
}

/// Drops every retained region — part of the newborn runtime's world
/// reset.
pub(crate) fn reset() {
    SNAPSHOT.with(|slot| slot.borrow_mut().clear());
    ROWS.with(|slot| slot.borrow_mut().clear());
}

/// The row cache of the list at `path`, made the first time it is asked.
pub(crate) fn row_cache(path: &str) -> Rc<crate::layout::RowCache> {
    ROWS.with(|slot| {
        Rc::clone(
            slot.borrow_mut()
                .entry(path.to_string())
                .or_insert_with(|| Rc::new(crate::layout::RowCache::new(path.to_string()))),
        )
    })
}

/// The row cache of the list at `path`, if it measures its own rows.
pub(crate) fn row_cache_if_any(path: &str) -> Option<Rc<crate::layout::RowCache>> {
    ROWS.with(|slot| slot.borrow().get(path).cloned())
}

/// Every list that measures its own rows, for the runtime's follow-ups.
pub(crate) fn row_caches() -> Vec<Rc<crate::layout::RowCache>> {
    ROWS.with(|slot| slot.borrow().values().cloned().collect())
}

/// Replaces the snapshot — the runtime calls this before each pass.
pub(crate) fn publish(regions: impl Iterator<Item = (String, RegionSnapshot)>) {
    SNAPSHOT.with(|slot| {
        let mut map = slot.borrow_mut();
        map.clear();
        map.extend(regions);
    });
}

/// The retained geometry of one region, if it virtualized last frame.
pub(crate) fn region(path: Option<&str>) -> Option<RegionSnapshot> {
    let path = path?;
    SNAPSHOT.with(|slot| slot.borrow().get(path).cloned())
}
