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
    /// Only the view wrappers — the target of read-tracking. Each one is a
    /// LENGTH of `joined`: the path of an open view is a prefix of the
    /// cursor's, so a view that opens costs a number, not a copy.
    views: Vec<usize>,
    /// The pass being run, counted. An owner's record carries the last pass
    /// its scope was entered in: that is the alive mark, and it costs a
    /// lookup where a set of every path of the pass cost a copy of each.
    pass_no: u64,
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
    /// The last pass this owner's scope was entered in ([`Registry::pass_no`]).
    touched: u64,
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
        registry.pass_no += 1;
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
        let pass_no = registry.pass_no;
        let dead: Vec<String> = registry
            .owners
            .iter()
            .filter(|(owner, record)| {
                under_root(owner)
                    && record.touched != pass_no
                    && !protected_by_skip(&registry, owner)
            })
            .map(|(owner, _)| owner.clone())
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
        // byte compare, no allocation — this closure runs per skipped
        // and re-run boundary for every owner the sweep audits
        owner.len() >= candidate.len()
            && owner.as_bytes().starts_with(candidate.as_bytes())
            && (owner.len() == candidate.len() || owner.as_bytes()[candidate.len()] == b'/')
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

/// The cursor's segments right now.
pub fn current_path_segments() -> Vec<String> {
    REGISTRY.with(|registry| registry.borrow().path.clone())
}

/// The cursor's PARENT segments, packed: what a retained entry keeps to
/// seed an isolated re-run ([`seed_from`]).
///
/// Every boundary of a mount keeps one, and the path above a row is a dozen
/// segments deep: a `Vec<String>` of them was a dozen allocations for each
/// boundary, made twice. Packed, it is the segments end to end and where
/// each one stops — two allocations, whatever the depth. The cut points are
/// kept and not found again: a segment may hold a `/` of its own (a row's
/// key is the app's string), so the joined path cannot be split back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathSeed {
    text: String,
    ends: Vec<u32>,
}

impl PathSeed {
    fn segments(&self) -> impl Iterator<Item = &str> {
        let mut from = 0usize;
        self.ends.iter().map(move |end| {
            let segment = &self.text[from..*end as usize];
            from = *end as usize;
            segment
        })
    }
}

/// [`PathSeed`] of the cursor right now: every segment but the last.
pub fn parent_seed() -> PathSeed {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let parents = registry.path.split_last().map_or(&[][..], |(_, parents)| parents);
        let mut seed = PathSeed {
            text: String::with_capacity(parents.iter().map(String::len).sum()),
            ends: Vec::with_capacity(parents.len()),
        };
        for segment in parents {
            seed.text.push_str(segment);
            seed.ends.push(seed.text.len() as u32);
        }
        seed
    })
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

/// The NAMED projection of a path — the segments a person chose, in
/// order, with everything positional dropped.
///
/// `Bench/#0/[pane-1]/Pane/@First/[code]` becomes `[pane-1]/[code]`.
/// The kept segments are the ones an app spells: `.id(…)`, a row key,
/// and the two overlay scopes (`sheet`, `popover`) — which stay so a
/// name inside a sheet can never be confused with the same name on the
/// page behind it. Dropped: the component's type name, `#n` tuple
/// positions and `@Variant` branches, which all move when the tree
/// above changes shape.
///
/// It is a PROJECTION, never a second identity: nothing is minted, the
/// cursor is untouched, and the result is only as stable as the names
/// the app chose. An empty answer means the thing has no name at all.
pub fn named_chain(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for segment in path.split('/') {
        let named = segment.starts_with('[') || segment == "sheet" || segment == "popover";
        if !named {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(segment);
    }
    out
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
        // the alive mark: an identity that owns something and was entered
        // this pass stays. One that owns nothing has no record to mark —
        // and nothing to sweep
        let registry = &mut *registry;
        if let Some(record) = registry.owners.get_mut(registry.joined.as_str()) {
            record.touched = registry.pass_no;
        }
        if is_view {
            registry.views.push(registry.joined.len());
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
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        registry.views.last().map(|len| registry.joined[..*len].to_string())
    })
}

/// Re-seeds the cursor with the PARENT path of a retained boundary, so the
/// reconciler can re-run one body in isolation (dirty view behind a skipped
/// parent) with correct anchors and identities. The returned frames undo on
/// drop.
pub fn seed(segments: &[String]) -> Vec<Frame> {
    segments.iter().map(|segment| enter(segment.clone())).collect()
}

/// [`seed`], from the packed form a retained entry keeps.
pub fn seed_from(parents: &PathSeed) -> Vec<Frame> {
    parents.segments().map(enter).collect()
}

fn current_scope(registry: &Registry) -> String {
    if registry.pass_active && !registry.joined.is_empty() {
        registry.joined.clone()
    } else {
        APP_SCOPE.to_string()
    }
}

/// Opens a new world on this thread: everything the identity owns by
/// PATH dies — the anchors, their state slots, the effect cells, the
/// whole read graph. State declared outside any pass (app scope) has
/// no owner and lives on untouched.
///
/// The runtime calls this when it is born. Path identity has no
/// meaning across two runtimes: a world that outlived its runtime
/// kept feeding the new one reads that pointed at the old state
/// slots, and invalidation died silently. Dropping the effect cells
/// also drops their task handles, so the old world's tasks cancel
/// with it.
pub fn reset_world() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let owners = std::mem::take(&mut registry.owners);
        for (_, record) in owners {
            for (type_id, index) in record.slots {
                crate::state::free_slot(type_id, index);
            }
        }
        let next_store_id = registry.next_store_id;
        *registry = Registry { next_store_id, ..Registry::default() };
    });
}

/// Opens a new world for ONE SCENE — the multi-window twin of
/// [`reset_world`].
///
/// A thread with several windows has several trees, each rooted at its
/// own first segment, and a newborn runtime there must not take the
/// others' worlds down with it: it drops only what lives under its own
/// root. The rule the whole-world reset states still holds inside that
/// subtree — path identity means nothing across two runtimes, so the
/// anchors, their state slots, the effect cells (and with them their
/// task handles) and the read graph under `root` all die here.
pub fn reset_scene(root: &str) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let prefix = format!("{root}/");
        let doomed: Vec<String> = registry
            .owners
            .keys()
            .filter(|owner| *owner == root || owner.starts_with(&prefix))
            .cloned()
            .collect();
        for owner in doomed {
            let Some(record) = registry.owners.remove(&owner) else {
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
            clear_view_reads(&mut registry, &owner);
            registry.dirty.remove(&owner);
        }
    });
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
        let pass_no = registry.pass_no;
        let owner = registry.owners.entry(key.0.clone()).or_default();
        // declared inside a scope this pass entered: alive
        owner.touched = pass_no;
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
        let view = match registry.views.last() {
            Some(len) => registry.joined[..*len].to_string(),
            None => ROOT_READER.to_string(),
        };
        registry.reads_by_view.entry(view.clone()).or_default().insert(key);
        registry.readers.entry(key).or_default().insert(view);
    });
}

thread_local! {
    /// Counts every write, read by a view or not.
    static WRITE_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A number that moves each time a `State` or a `Store` is written on
/// this thread — whether a view read it or not. A shell asks it after a
/// turn of tasks: the same number means no task wrote anything, so the
/// scene cannot have changed through a write. A write that no view read
/// still moves it, because an `on_change` or an `on_receive` may watch
/// the value, and those read outside a pass.
pub fn write_epoch() -> u64 {
    WRITE_EPOCH.with(std::cell::Cell::get)
}

pub(crate) fn record_write(key: DepKey) {
    WRITE_EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
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
            let pass_no = registry.pass_no;
            let owner = registry.owners.entry(scope).or_default();
            owner.touched = pass_no;
            owner.effect_sites.push(key);
        }
        cell
    })
}

#[cfg(test)]
mod tests {
    /// A packed seed re-enters the same segments — a row's key with a `/`
    /// of its own included, which is why the cut points are kept and the
    /// joined path is never split back.
    #[test]
    fn a_packed_seed_re_enters_the_segments_it_was_cut_from() {
        use super::{begin_pass, current_path_segments, end_pass, enter, enter_view, parent_seed, seed_from};

        begin_pass();
        let seed = {
            let _root = enter("Root");
            let _row = enter("[a/b]");
            let _stack = enter("#0");
            let _leaf = enter_view("Leaf");
            parent_seed()
        };
        let _ = end_pass();
        assert_eq!(seed.segments().collect::<Vec<_>>(), ["Root", "[a/b]", "#0"]);
        begin_pass();
        let frames = seed_from(&seed);
        assert_eq!(current_path_segments(), ["Root", "[a/b]", "#0"]);
        drop(frames);
        let _ = end_pass();
    }

    use super::named_chain;

    #[test]
    fn the_projection_keeps_the_names_and_drops_the_positions() {
        assert_eq!(
            named_chain("Bench/#0/[pane-1]/Pane/@First/[code]"),
            "[pane-1]/[code]"
        );
        // the shape of the tree above moved; the names did not
        assert_eq!(
            named_chain("Bench/@Second/[code]"),
            named_chain("Bench/@First/#0/[code]")
        );
    }

    #[test]
    fn an_overlay_scope_stays_in_the_chain() {
        // a name INSIDE a sheet is not the same name as the one on the
        // page behind it — the scope is part of what a person wrote
        assert_ne!(named_chain("App/popover/[query]"), named_chain("App/[query]"));
        assert_eq!(named_chain("App/#3/sheet/[query]"), "sheet/[query]");
    }

    #[test]
    fn a_path_with_no_name_projects_to_nothing() {
        assert_eq!(named_chain("Bench/@First/#0"), "");
        assert_eq!(named_chain(""), "");
    }
}
