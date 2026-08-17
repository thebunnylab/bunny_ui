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

use crate::layout::Px;
use motor::hash::FxHashMap;

/// What the body needs for the window math, all from the LAST frame.
#[derive(Clone, Default)]
pub(crate) struct RegionSnapshot {
    pub offset_y: Px,
    pub viewport: Px,
    pub row_extent: Px,
    /// The scroll target the runtime already APPLIED for this region —
    /// a reveal equal to it is settled history, not a pending jump,
    /// and must not fight the wheel for the window.
    pub applied: Option<String>,
}

thread_local! {
    static SNAPSHOT: RefCell<FxHashMap<String, RegionSnapshot>> =
        RefCell::new(FxHashMap::default());
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
