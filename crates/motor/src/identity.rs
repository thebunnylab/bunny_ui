//! Structural identity + runtime ownership of state.
//!
//! The render cursor keeps the path down to the current point of the tree —
//! view wrapper (`CountriesList`), tuple position (`#0`), conditional arm
//! (`@First`), row key (`[USA]`), sheet content (`sheet`). That path is the
//! structural identity: it is where state anchors, it is what state dies
//! by, and it is what the reconciler uses to decide which body re-runs.
//!
//! Roles, one arena:
//!
//! - **Anchor**: `State::new` INSIDE a render pass does not allocate
//!   blindly — it asks here for (construction scope, type, seq). If the
//!   identity already owns the slot, the new handle points to it and the
//!   initial value is discarded (the initial only seeds the first mount,
//!   like Swift's `@State`). Outside render, app scope: allocate once and
//!   live forever — the case of the roots the app holds.
//! - **Owner**: every identity touched in a pass stays alive; at the end of
//!   the pass, identities under the same root that did not show up are
//!   swept — slots freed (generation advances: a stale handle fails loudly
//!   instead of reading a recycled slot), anchors and effect slots removed.
//!   Subtrees the reconciler SKIPPED (clean cache) count as alive without
//!   being visited.
//! - **Read graph**: `get()` during render records "this view read this
//!   dependency" — `State` (slot) or `Store` (id). A view's read set
//!   persists until ITS next re-render (skipped views do not lose
//!   dependencies). `set()`/`send()` marks dirty exactly who read.
//!
//! Known limit (documented, not accidental): anchors are born in the
//! CONSTRUCTION scope. Row and sheet closures run during render — with the
//! cursor already inside the key — so row state follows the item. But arms
//! of one same `body` build everything in the same scope: two arms that
//! built `State` of the SAME type would collide on the anchor. The real
//! engine, with per-view field metadata, gets this right; the fake picks
//! the simple, verifiable rule.

use std::any::TypeId;
use std::cell::RefCell;
use crate::hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::rc::Rc;

use crate::runtime::Site;

/// An observable dependency: a `State` (by the slot's global id, never
/// recycled) or a whole `Store` (object granularity, like an
/// ObservableObject).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DepKey {
    State(u64),
    Store(u64),
}

#[derive(Default)]
struct Registry {
    pass_active: bool,
    /// First segment pushed in the pass — defines the swept root.
    pass_root: Option<String>,
    path: Vec<String>,
    /// The path pre-joined with `/`, maintained incrementally by
    /// push/truncate — reading the scope is one clone, never a walk.
    joined: String,
    /// Saved lengths of `joined`, one per open frame — the truncation
    /// points of the drops.
    joined_lens: Vec<usize>,
    /// Only the view wrappers — the target of read-tracking.
    views: Vec<String>,
    touched: HashSet<String>,
    /// Boundaries the reconciler skipped this pass (clean cache): their
    /// subtree counts as alive in the sweep.
    skipped: HashSet<String>,
    /// Boundaries whose body RAN this pass: inside them the sweep follows
    /// the normal rule (what did not show up, died).
    reran: HashSet<String>,
    /// Identity → resources that die with it.
    owners: HashMap<String, OwnerRecord>,
    /// (scope, type, seq) → (index in the type's arena, generation, dep-id).
    anchors: HashMap<AnchorKey, (usize, u32, u64)>,
    /// Per-pass counters: how many `State::new` of each type each scope has done.
    seqs: HashMap<(String, TypeId), u32>,
    /// view → dependencies read in its LAST body (persists across passes).
    reads_by_view: HashMap<String, HashSet<DepKey>>,
    /// inverted index: dependency → reader views.
    readers: HashMap<DepKey, HashSet<String>>,
    dirty: HashSet<String>,
    /// Effect slots by (site, scope) — the retention behind `on_change`/`on_receive`.
    effect_cells: HashMap<(Site, String), Rc<dyn std::any::Any>>,
    next_store_id: u64,
}

type AnchorKey = (String, TypeId, u32);

#[derive(Default)]
struct OwnerRecord {
    /// (type, index in the type's arena) — the sweep frees through the
    /// arena registry without knowing the type statically.
    slots: Vec<(TypeId, usize)>,
    anchors: Vec<AnchorKey>,
    effect_sites: Vec<(Site, String)>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Scope of the `State`s created outside any pass (app roots).
const APP_SCOPE: &str = "@app";

/// Reads outside any view wrapper (the root region — free fns, custom
/// modifiers at the top). That region re-runs on every pass, so its
/// dependencies reset on each begin.
const ROOT_READER: &str = "@root";

// MARK: - Pass

/// Opens a render pass: resets anchor counters, the alive marks and the
/// reads of the root region (which always re-runs). The reads of retained
/// views STAY — a skipped view does not lose dependencies. Called by the
/// typed layer's `Runtime` — the mirrored engine never opens a pass, so it
/// keeps the old semantics intact.
pub fn begin_pass() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.pass_active = true;
        registry.pass_root = None;
        registry.path.clear();
        registry.joined.clear();
        registry.joined_lens.clear();
        registry.views.clear();
        registry.touched.clear();
        registry.skipped.clear();
        registry.reran.clear();
        registry.seqs.clear();
        clear_view_reads(&mut registry, ROOT_READER);
    });
}

/// Closes the pass and sweeps. An owner dies if: it sits under this pass's
/// root, it was not touched, and the nearest retained boundary above it was
/// NOT skipped (longest prefix wins: under a skip the subtree lives; under
/// a body that ran, the normal rule applies). Returns the dead paths so the
/// reconciler can drop the matching retained entries.
pub fn end_pass() -> Vec<String> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.pass_active = false;
        // the root stays readable until the next begin_pass (the runtime
        // consults it to scope dirty state and effects)
        let Some(root) = registry.pass_root.clone() else {
            return Vec::new();
        };
        let prefix = format!("{root}/");
        let under_root =
            |owner: &str| owner == root || owner.starts_with(&prefix);
        let dead: Vec<String> = registry
            .owners
            .keys()
            .filter(|owner| {
                under_root(owner)
                    && !registry.touched.contains(*owner)
                    && !protected_by_skip(&registry, owner)
            })
            .cloned()
            .collect();
        for owner in &dead {
            let Some(record) = registry.owners.remove(owner) else {
                continue;
            };
            for (type_id, index) in record.slots {
                crate::state::free_slot(type_id, index);
            }
            for key in record.anchors {
                registry.anchors.remove(&key);
            }
            for site in record.effect_sites {
                registry.effect_cells.remove(&site);
            }
            clear_view_reads(&mut registry, owner);
            registry.dirty.remove(owner);
        }
        dead
    })
}

/// Longest prefix among skipped and re-run boundaries decides: skipped
/// protects, re-run (or none) lets the normal rule apply.
fn protected_by_skip(registry: &Registry, owner: &str) -> bool {
    let mut best_len = 0usize;
    let mut best_is_skip = false;
    let covers = |candidate: &str| {
        owner == candidate || owner.starts_with(&format!("{candidate}/"))
    };
    for skip in &registry.skipped {
        if covers(skip) && skip.len() > best_len {
            best_len = skip.len();
            best_is_skip = true;
        }
    }
    for rerun in &registry.reran {
        if covers(rerun) && rerun.len() > best_len {
            best_len = rerun.len();
            best_is_skip = false;
        }
    }
    best_is_skip
}

/// The reconciler reports: this boundary was skipped (clean cache) — its
/// subtree counts as alive.
pub fn mark_skipped(path: &str) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().skipped.insert(path.to_string());
    });
}

/// The reconciler reports: this boundary's body ran this pass.
pub fn mark_reran(path: &str) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().reran.insert(path.to_string());
    });
}

/// Views dirtied by writes since the last drain — the fine-grained
/// invalidation, exposed for the stability loop and for the tests.
pub fn take_dirty() -> Vec<String> {
    REGISTRY.with(|registry| {
        let mut dirty: Vec<String> = registry.borrow_mut().dirty.drain().collect();
        dirty.sort();
        dirty
    })
}

/// Drains only this root's dirt (plus the root region, which any pass
/// consumes). Dirt from ANOTHER root stays queued for that root's render.
pub fn take_dirty_matching(root: &str) -> Vec<String> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let prefix = format!("{root}/");
        let mut matching: Vec<String> = registry
            .dirty
            .iter()
            .filter(|path| *path == ROOT_READER || *path == root || path.starts_with(&prefix))
            .cloned()
            .collect();
        for path in &matching {
            registry.dirty.remove(path);
        }
        matching.sort();
        matching
    })
}

/// Copy of the dirty set right now — the snapshot that decides the pass,
/// without draining (writes DURING the pass must survive into the next
/// cycle).
pub fn dirty_snapshot() -> HashSet<String> {
    REGISTRY.with(|registry| registry.borrow().dirty.clone())
}

/// Marks the view at `path` dirty from OUTSIDE the read-tracking — the
/// runtime's hook for follow-up passes (a virtualized window that must
/// re-materialize after its offset moved). The next pass re-runs the
/// view like any dirty one; consumption stays with the pass.
pub fn invalidate(path: &str) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().dirty.insert(path.to_string());
    });
}

/// Is there pending dirt for this root? Peeks without draining — the
/// stability condition uses this; who CONSUMES dirt is the render pass
/// (snapshot + consume), never the loop.
pub fn has_dirty_matching(root: &str) -> bool {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let prefix = format!("{root}/");
        registry
            .dirty
            .iter()
            .any(|path| path == ROOT_READER || path == root || path.starts_with(&prefix))
    })
}

/// End of the pass: consumes from the registry the dirt this pass served —
/// the intersection of the snapshot with the root (and the root region).
/// What came from writes during render stays; what belongs to another root
/// stays.
pub fn consume_dirty(root: &str, snapshot: &HashSet<String>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let prefix = format!("{root}/");
        for path in snapshot {
            if path == ROOT_READER || path == root || path.starts_with(&prefix) {
                registry.dirty.remove(path);
            }
        }
    });
}

/// The first segment pushed in the current pass (or in the last one closed).
pub fn current_pass_root() -> Option<String> {
    REGISTRY.with(|registry| registry.borrow().pass_root.clone())
}

/// The cursor's segments right now — the retained entry stores the parent
/// path to seed isolated re-runs.
pub fn current_path_segments() -> Vec<String> {
    REGISTRY.with(|registry| registry.borrow().path.clone())
}

/// The cursor's full path right now (`None` outside a pass) — the key
/// interactive nodes register their actions under. One clone of the
/// incrementally maintained path: no join walk, ever.
pub fn cursor_scope() -> Option<String> {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        (registry.pass_active && !registry.joined.is_empty()).then(|| registry.joined.clone())
    })
}

// MARK: - Cursor

/// One step of the cursor — released on drop, so the path survives early
/// returns and debug_assert panics.
pub struct Frame {
    pops_view: bool,
    active: bool,
}

impl Drop for Frame {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.path.pop();
            // the joined path steps back by truncation — the bytes of
            // the parent are still in place, untouched
            let depth = registry.joined_lens.pop().unwrap_or(0);
            registry.joined.truncate(depth);
            if self.pops_view {
                registry.views.pop();
            }
        });
    }
}

fn push(segment: String, is_view: bool) -> Frame {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active {
            return Frame { pops_view: false, active: false };
        }
        if registry.pass_root.is_none() {
            registry.pass_root = Some(segment.clone());
        }
        // the joined path grows in place: push_str now, truncate on the
        // frame's drop — the per-step full-path JOIN died here
        let depth = registry.joined.len();
        registry.joined_lens.push(depth);
        if !registry.joined.is_empty() {
            registry.joined.push('/');
        }
        registry.joined.push_str(&segment);
        registry.path.push(segment);
        let scope = registry.joined.clone();
        if is_view {
            registry.touched.insert(scope.clone());
            registry.views.push(scope);
        } else {
            registry.touched.insert(scope);
        }
        Frame { pops_view: is_view, active: true }
    })
}

/// Steps down one structural level: tuple position (`#0`), arm (`@First`),
/// row key (`[USA]`), sheet content (`sheet`).
pub fn enter(segment: impl Into<String>) -> Frame {
    push(segment.into(), false)
}

/// Steps down into a view's wrapper (`Component`) — besides the path, it
/// enters the view stack that read-tracking uses as its target.
pub fn enter_view(name: impl Into<String>) -> Frame {
    push(name.into(), true)
}

/// The path of the innermost view being rendered — the reconciler's key.
pub fn current_view_path() -> Option<String> {
    REGISTRY.with(|registry| registry.borrow().views.last().cloned())
}

/// Re-seeds the cursor with the PARENT path of a retained boundary, so the
/// reconciler can re-run one body in isolation (dirty view behind a skipped
/// parent) with correct anchors and identities. The returned frames undo on
/// drop.
pub fn seed(segments: &[String]) -> Vec<Frame> {
    segments.iter().map(|segment| enter(segment.clone())).collect()
}

fn current_scope(registry: &Registry) -> String {
    if registry.pass_active && !registry.joined.is_empty() {
        registry.joined.clone()
    } else {
        APP_SCOPE.to_string()
    }
}

// MARK: - State anchors

/// What `State::new` gets back when declaring state.
pub(crate) enum Claim {
    /// The identity already owns this state: reuse the slot, discard the initial.
    Existing { index: usize, generation: u32, dep: u64 },
    /// First mount (or app scope): allocate and register with the token.
    Fresh(AnchorToken),
}

pub(crate) struct AnchorToken {
    key: Option<AnchorKey>,
}

pub(crate) fn claim_anchor(type_id: TypeId) -> Claim {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active {
            return Claim::Fresh(AnchorToken { key: None });
        }
        let scope = current_scope(&registry);
        let seq_key = (scope.clone(), type_id);
        let seq = *registry
            .seqs
            .entry(seq_key)
            .and_modify(|seq| *seq += 1)
            .or_insert(0);
        let key = (scope, type_id, seq);
        match registry.anchors.get(&key) {
            Some(&(index, generation, dep)) => Claim::Existing { index, generation, dep },
            None => Claim::Fresh(AnchorToken { key: Some(key) }),
        }
    })
}

pub(crate) fn fulfill_anchor(token: AnchorToken, index: usize, generation: u32, dep: u64) {
    let Some(key) = token.key else {
        return; // app scope: no anchor, no owner, lives forever
    };
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.anchors.insert(key.clone(), (index, generation, dep));
        let owner = registry.owners.entry(key.0.clone()).or_default();
        owner.slots.push((key.1, index));
        owner.anchors.push(key);
    });
}

// MARK: - Read graph

/// This view's body is about to (re)run: its old reads fall away — the new
/// set is whatever the body records now.
pub fn begin_view_reads(view: &str) {
    REGISTRY.with(|registry| {
        clear_view_reads(&mut registry.borrow_mut(), view);
    });
}

fn clear_view_reads(registry: &mut Registry, view: &str) {
    let Some(keys) = registry.reads_by_view.remove(view) else {
        return;
    };
    for key in keys {
        if let Some(readers) = registry.readers.get_mut(&key) {
            readers.remove(view);
            if readers.is_empty() {
                registry.readers.remove(&key);
            }
        }
    }
}

pub(crate) fn record_read(key: DepKey) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active {
            return;
        }
        let view = registry
            .views
            .last()
            .cloned()
            .unwrap_or_else(|| ROOT_READER.to_string());
        registry.reads_by_view.entry(view.clone()).or_default().insert(key);
        registry.readers.entry(key).or_default().insert(view);
    });
}

pub(crate) fn record_write(key: DepKey) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(readers) = registry.readers.get(&key).cloned() else {
            return;
        };
        registry.dirty.extend(readers);
    });
}

pub(crate) fn next_store_id() -> u64 {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.next_store_id += 1;
        registry.next_store_id
    })
}

// MARK: - Per-identity effect slots

/// The `on_change`/`on_receive` slot, keyed by (site, current scope): two
/// instances of the same view at the same callsite get separate slots, and
/// the slot dies with the identity. Outside a pass it falls back to the app
/// scope — the global behavior from before.
pub fn scoped_effect_slot<V: 'static>(site: impl Into<Site>) -> Rc<RefCell<Option<V>>> {
    let site = site.into();
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let scope = current_scope(&registry);
        let key = (site, scope.clone());
        if let Some(any) = registry.effect_cells.get(&key).cloned()
            && let Ok(cell) = any.downcast::<RefCell<Option<V>>>()
        {
            return cell;
        }
        let cell: Rc<RefCell<Option<V>>> = Rc::new(RefCell::new(None));
        registry.effect_cells.insert(key.clone(), cell.clone());
        if scope != APP_SCOPE {
            registry.owners.entry(scope).or_default().effect_sites.push(key);
        }
        cell
    })
}
