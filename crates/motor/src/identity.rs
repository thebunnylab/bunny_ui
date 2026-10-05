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
use std::borrow::Cow;
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
    /// The cursor's path, joined with `/`, maintained incrementally by
    /// push/truncate — reading the scope is one clone, never a walk. It
    /// is the only spelling of the cursor: a step writes its segment
    /// here and nowhere else, so a row's key costs no string of its own.
    joined: String,
    /// Saved lengths of `joined`, one per open frame — the truncation
    /// points of the drops, and where each segment starts: the segments
    /// read back from them exactly, a key with a `/` of its own included.
    joined_lens: Vec<usize>,
    /// Only the view wrappers — the target of read-tracking. Each one is a
    /// LENGTH of `joined`: the path of an open view is a prefix of the
    /// cursor's, so a view that opens costs a number, not a copy.
    views: Vec<usize>,
    /// Beside each open view, the shared copy of its path, once its body
    /// is known to run ([`begin_view_reads`]): the one copy the retention
    /// keys the boundary by, which the bindings the body makes are filed
    /// under instead of a copy of their own.
    view_keys: Vec<Option<Rc<str>>>,
    /// The buffer a key derived from the cursor is spelled in before it
    /// becomes shared — one allocation per key, never one per step.
    key_scratch: String,
    /// The pass being run, counted. An owner's record carries the last pass
    /// its scope was entered in: that is the alive mark, and it costs a
    /// lookup where a set of every path of the pass cost a copy of each.
    pass_no: u64,
    /// Boundaries the reconciler skipped this pass (clean cache): their
    /// subtree counts as alive in the sweep.
    skipped: HashSet<Rc<str>>,
    /// Did any owner stand when the pass began? The skips are asked only
    /// for an owner the pass did not touch ([`protected_by_skip`]), and
    /// an owner born during the pass is touched by its birth — so a pass
    /// that began with none never asks, and keeps no skips.
    skips_asked: bool,
    /// Boundaries whose body RAN this pass: inside them the sweep follows
    /// the normal rule (what did not show up, died).
    reran: HashSet<Rc<str>>,
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
    /// From `begin_pass` to `consume_dirty`: whether a write is landing
    /// inside a pass, where it may reach a view whose body already ran.
    serving: bool,
    /// The dirt this pass took to serve ([`take_dirty_under`]): a write
    /// during the pass to a view in here that has not run yet is read
    /// when it runs, and dirties nothing.
    served: HashSet<String>,
    /// Views a write reached during this pass AFTER their body ran in it
    /// (T2-BUNNY-277). Their dirt is not the dirt the pass served, so the
    /// pass's end leaves it for the next one.
    missed: HashSet<String>,
    /// Effect slots by (site, scope) — the retention behind `on_change`/`on_receive`.
    effect_cells: HashMap<(Site, String), Rc<dyn std::any::Any>>,
    next_store_id: u64,
    /// The binding being read right now: while one is open, a read
    /// belongs to it and not to the view whose body may be running.
    binding_scope: Option<Rc<str>>,
    /// A probe is open ([`begin_probe`]): with no binding open inside
    /// it, a read is the probe's.
    probing: bool,
    /// What the last probe read, kept aside until a key is minted for
    /// it — or not, when it read nothing.
    probe_reads: Vec<DepKey>,
    /// A write reached something the open probe had already read.
    probe_written: bool,
    /// binding → dependencies read in its LAST evaluation — nearly
    /// always one, held inline ([`Few`]).
    binding_reads: HashMap<Rc<str>, Few<DepKey>>,
    /// inverted index: dependency → the bindings that read it — a row's
    /// own state is read by the row's own binding alone.
    binding_readers: HashMap<DepKey, Few<Rc<str>>>,
    /// The bindings a write reached since the last frame took them.
    dirty_bindings: HashSet<Rc<str>>,
    /// view → the bindings its body made. A body that re-runs makes them
    /// again; one that dies takes them along. Keyed by the view's shared
    /// path: a row that mounts files its bindings under the copy its
    /// boundary already holds, and one that leaves frees no string.
    view_bindings: HashMap<Rc<str>, BindingKeys>,
    /// Views that left the tree, by the shared path their bindings stand
    /// under: the bindings stay there until a write asks for them or the
    /// idle takes them apart ([`file_retired`]). A view made at the same
    /// path afterwards files under a path of its own.
    leaving: Vec<Rc<str>>,
    /// Views that left, each with the bindings its body made, taken out
    /// from under it: the half of their teardown that waits for an idle
    /// moment ([`collect_retired`]).
    retired: Vec<(Rc<str>, BindingKeys)>,
    /// The bindings of the retired views, by the IDENTITY of their key —
    /// the copy the binding was made with. A write that reaches one marks
    /// nothing. A body that makes the key again spells a copy of its own,
    /// which none of them is.
    retired_bindings: HashSet<usize>,
    /// The pump is asking the effects: a read now is an effect's.
    effects_open: bool,
    /// What the effects read, over the rounds of one settle — a write to
    /// one of these is a reason to ask them again.
    effect_readers: HashSet<DepKey>,
    /// A box is painting: a read now is the box's, and a write to what it
    /// read repaints the box, not the scene.
    painting: bool,
    /// The painting box's path, held in a warm buffer (no string per
    /// paint) and spelled as an `Rc<str>` only when a read is filed.
    painting_path: String,
    painting_rc: Option<Rc<str>>,
    /// Who paints what: the boxes that read each dependency.
    paint_readers: HashMap<DepKey, Few<Rc<str>>>,
    /// The boxes a write reached since the last frame took them.
    dirty_paints: HashSet<Rc<str>>,
}

impl Registry {
    /// The cursor's segments, read back from the joined path: each open
    /// frame saved where the path stood before it, and a separator
    /// follows a non-empty path — so every cut is exact.
    fn segments(&self) -> impl Iterator<Item = &str> + '_ {
        let lens = &self.joined_lens;
        (0..lens.len()).map(move |at| {
            let start = lens[at] + usize::from(lens[at] > 0);
            let end = lens.get(at + 1).copied().unwrap_or(self.joined.len());
            &self.joined[start..end]
        })
    }
}

/// A set that nearly always holds one member, held inline until a
/// second one arrives.
///
/// A binding reads one value as a rule — a row's label, its selection
/// flag — and that value is read by that binding alone. A hash set of
/// one is an allocation all the same, made on the first insert: a row
/// with two bindings made four of them when it mounted, and freed four
/// when it left. An empty set has no form here — the entry that would
/// hold it is taken out of its map instead ([`Few::remove`]).
#[derive(Debug)]
enum Few<T> {
    One(T),
    Many(HashSet<T>),
}

impl<T: Eq + std::hash::Hash> Few<T> {
    /// Adds a member; one already there is not added twice.
    fn insert(&mut self, member: T) {
        match self {
            Few::One(one) if *one == member => {}
            Few::One(_) => {
                let Few::One(one) = std::mem::replace(self, Few::Many(HashSet::default())) else {
                    unreachable!("the arm above matched one member")
                };
                if let Few::Many(set) = self {
                    set.insert(one);
                    set.insert(member);
                }
            }
            Few::Many(set) => {
                set.insert(member);
            }
        }
    }

    /// Takes a member out, and says whether the set is EMPTY now — the
    /// holder then takes the whole entry out, since an empty set has no
    /// form of its own.
    fn remove<Q>(&mut self, member: &Q) -> bool
    where
        T: std::borrow::Borrow<Q>,
        Q: Eq + std::hash::Hash + ?Sized,
    {
        match self {
            Few::One(one) => <T as std::borrow::Borrow<Q>>::borrow(one) == member,
            Few::Many(set) => {
                set.remove(member);
                set.is_empty()
            }
        }
    }

    fn len(&self) -> usize {
        match self {
            Few::One(_) => 1,
            Few::Many(set) => set.len(),
        }
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        let (one, many) = match self {
            Few::One(one) => (Some(one), None),
            Few::Many(set) => (None, Some(set.iter())),
        };
        one.into_iter().chain(many.into_iter().flatten())
    }

    /// The members, by value.
    fn into_members(self) -> impl Iterator<Item = T> {
        let (one, many) = match self {
            Few::One(one) => (Some(one), None),
            Few::Many(set) => (None, Some(set.into_iter())),
        };
        one.into_iter().chain(many.into_iter().flatten())
    }
}

/// Files `member` in the set `map` keeps at `key`: a set of one the
/// first time, inline.
fn file_few<K: Eq + std::hash::Hash, T: Eq + std::hash::Hash>(map: &mut HashMap<K, Few<T>>, key: K, member: T) {
    match map.entry(key) {
        std::collections::hash_map::Entry::Occupied(mut set) => set.get_mut().insert(member),
        std::collections::hash_map::Entry::Vacant(slot) => {
            slot.insert(Few::One(member));
        }
    }
}

/// The bindings one body made, in the order it made them: two for a
/// row of a table (its class and its label), seldom more. Up to two
/// are held inline, in the width of two keys; a list is made only past
/// that. The list a body's first binding started was an allocation per
/// row that mounted, and its second binding grew it.
enum BindingKeys {
    One(Rc<str>),
    Two(Rc<str>, Rc<str>),
    More(Vec<Rc<str>>),
}

impl BindingKeys {
    fn push(&mut self, key: Rc<str>) {
        let held = std::mem::replace(self, BindingKeys::More(Vec::new()));
        *self = match held {
            BindingKeys::One(first) => BindingKeys::Two(first, key),
            BindingKeys::Two(first, second) => BindingKeys::More(vec![first, second, key]),
            BindingKeys::More(mut keys) => {
                keys.push(key);
                BindingKeys::More(keys)
            }
        };
    }

    fn len(&self) -> usize {
        match self {
            BindingKeys::One(_) => 1,
            BindingKeys::Two(..) => 2,
            BindingKeys::More(keys) => keys.len(),
        }
    }

    fn iter(&self) -> impl Iterator<Item = &Rc<str>> {
        let (pair, more): ([Option<&Rc<str>>; 2], &[Rc<str>]) = match self {
            BindingKeys::One(first) => ([Some(first), None], &[]),
            BindingKeys::Two(first, second) => ([Some(first), Some(second)], &[]),
            BindingKeys::More(keys) => ([None, None], keys),
        };
        pair.into_iter().flatten().chain(more.iter())
    }

    fn into_keys(self) -> impl Iterator<Item = Rc<str>> {
        let (pair, more) = match self {
            BindingKeys::One(first) => ([Some(first), None], Vec::new()),
            BindingKeys::Two(first, second) => ([Some(first), Some(second)], Vec::new()),
            BindingKeys::More(keys) => ([None, None], keys),
        };
        pair.into_iter().flatten().chain(more)
    }
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
        registry.serving = true;
        registry.missed.clear();
        registry.pass_root = None;
        registry.joined.clear();
        registry.joined_lens.clear();
        registry.views.clear();
        registry.view_keys.clear();
        registry.pass_no += 1;
        registry.skipped.clear();
        registry.skips_asked = !registry.owners.is_empty();
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
        // the pass is over: a write from here on is nobody's own
        registry.serving = false;
        registry.served.clear();
        // the root stays readable until the next begin_pass (the runtime
        // consults it to scope dirty state and effects)
        let Some(root) = registry.pass_root.clone() else {
            return Vec::new();
        };
        let below_root = |owner: &str| {
            owner == root || owner.strip_prefix(root.as_str()).is_some_and(|rest| rest.starts_with('/'))
        };
        let pass_no = registry.pass_no;
        let dead: Vec<String> = registry
            .owners
            .iter()
            .filter(|(owner, record)| {
                below_root(owner)
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

/// The nearest skipped or re-run boundary at or above the owner
/// decides: skipped protects, re-run (or none) lets the normal rule
/// apply. The owner's own path is asked first, then each cut above it —
/// a lookup per level, whatever the size of the two sets.
fn protected_by_skip(registry: &Registry, owner: &str) -> bool {
    let mut end = owner.len();
    loop {
        let candidate = &owner[..end];
        if registry.skipped.contains(candidate) {
            return true;
        }
        if registry.reran.contains(candidate) {
            return false;
        }
        match candidate.rfind('/') {
            Some(cut) => end = cut,
            None => return false,
        }
    }
}

/// A boundary the walk keeps without stepping into it — a row a keyed
/// list kept by its key, whose closure did not run. The owners of the two
/// scopes the steps to it would have entered, the row's key scope
/// ([`enter_key`]) and the boundary's own ([`enter_view`]), stay alive as
/// those steps would have kept them — the state the row's closure made
/// lives at the first — and the boundary counts as skipped
/// ([`mark_skipped`]).
pub fn keep_unentered(key_scope: &str, boundary: &Rc<str>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.pass_active {
            let pass_no = registry.pass_no;
            for scope in [key_scope, &**boundary] {
                if let Some(record) = registry.owners.get_mut(scope) {
                    record.touched = pass_no;
                }
            }
        }
        if registry.skips_asked {
            registry.skipped.insert(Rc::clone(boundary));
        }
    });
}

/// The reconciler reports: this boundary was skipped (clean cache) — its
/// subtree counts as alive.
pub fn mark_skipped(path: &Rc<str>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        // a list of a thousand kept rows is a thousand skips: kept only
        // when an owner from before the pass may ask for them
        if registry.skips_asked {
            registry.skipped.insert(Rc::clone(path));
        }
    });
}

/// The reconciler reports: this boundary's body ran this pass.
pub fn mark_reran(path: &Rc<str>) {
    REGISTRY.with(|registry| {
        registry.borrow_mut().reran.insert(Rc::clone(path));
    });
}

/// The current view's path, borrowed from the cursor for the length
/// of `read` — a decision that needs no copy of it. `read` must not
/// touch the registry itself.
pub fn with_current_view_path<R>(read: impl FnOnce(Option<&str>) -> R) -> R {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        read(registry.views.last().map(|len| &registry.joined[..*len]))
    })
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

/// Moves this root's dirt out of the registry (plus the root region's,
/// which any pass consumes) — the set that decides the pass, taken
/// instead of copied. Writes DURING the pass land in the emptied set and
/// survive into the next cycle; dirt from ANOTHER root stays queued for
/// that root's render.
pub fn take_dirty_under(root: &str) -> HashSet<String> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.dirty.is_empty() {
            return HashSet::default();
        }
        let prefix = format!("{root}/");
        let (taken, kept): (HashSet<String>, HashSet<String>) =
            std::mem::take(&mut registry.dirty).into_iter().partition(|path| {
                *path == ROOT_READER || *path == root || path.starts_with(&prefix)
            });
        registry.dirty = kept;
        registry.served = taken.clone();
        taken
    })
}

/// Marks the view at `path` dirty from OUTSIDE the read-tracking — the
/// runtime's hook for follow-up passes (a virtualized window that must
/// re-materialize after its offset moved). The next pass re-runs the
/// view like any dirty one; consumption stays with the pass.
pub fn invalidate(path: &str) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        note_missed(&mut registry, std::iter::once(path));
        registry.dirty.insert(path.to_string());
    });
}

/// Is there pending dirt for this root? Peeks without draining — the
/// stability condition uses this; who CONSUMES dirt is the render pass
/// (snapshot + consume), never the loop.
pub fn has_dirty_matching(root: &str) -> bool {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        // most frames ask with nothing dirty: no walk, no string
        if registry.dirty.is_empty() {
            return false;
        }
        registry.dirty.iter().any(|path| under_root(path, root))
    })
}

/// Is `path` the root itself, below it, or the root region — the three
/// readers a pass over `root` serves? No string is made: the root is cut
/// off the path and the slash checked where it stands.
fn under_root(path: &str, root: &str) -> bool {
    path == ROOT_READER
        || path == root
        || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

/// End of the pass: consumes from the registry the dirt this pass served —
/// the intersection of the snapshot with the root (and the root region).
/// What came from writes during render stays; what belongs to another root
/// stays; and a view the snapshot held that a write reached after its body
/// ran stays too — the pass ran it, but before the write (T2-BUNNY-277).
pub fn consume_dirty(root: &str, snapshot: &HashSet<String>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = &mut *registry;
        for path in snapshot {
            if under_root(path, root) && !registry.missed.contains(path)
            {
                registry.dirty.remove(path);
            }
        }
        registry.serving = false;
        registry.missed.clear();
    });
}

/// Records which of `readers` a write during the pass reached too late:
/// their body already ran in it, and their frame is closed. A reader whose
/// frame is still OPEN is the writer itself, or an ancestor of it, mid-body —
/// sending it round again for its own write would never let the frame rest,
/// since a write always notifies.
/// Is this reader's dirt served by the pass under way? The pass took
/// its dirt, and either its frame is still open — the write is its own,
/// and sending it round again would never let the frame rest — or the
/// pass has not reached it yet and it reads the new value when it runs.
/// A reader the pass did not take keeps the write for the next one.
fn served_by_this_pass(registry: &Registry, reader: &str) -> bool {
    if !registry.served.contains(reader) {
        return false;
    }
    let open = registry.views.iter().any(|len| &registry.joined[..*len] == reader);
    open || !registry.reran.contains(reader)
}

fn note_missed<'a>(registry: &mut Registry, readers: impl Iterator<Item = &'a str>) {
    if !registry.serving {
        return;
    }
    let open = |reader: &str| registry.views.iter().any(|len| &registry.joined[..*len] == reader);
    let late: Vec<String> = readers
        .filter(|reader| registry.reran.contains(*reader) && !open(reader))
        .map(str::to_string)
        .collect();
    registry.missed.extend(late);
}

/// The first segment pushed in the current pass (or in the last one closed).
pub fn current_pass_root() -> Option<String> {
    REGISTRY.with(|registry| registry.borrow().pass_root.clone())
}

/// The cursor's segments right now.
pub fn current_path_segments() -> Vec<String> {
    REGISTRY.with(|registry| registry.borrow().segments().map(str::to_string).collect())
}

/// The cursor's PARENT segments, as the points the path they were taken
/// at is cut: what a retained entry keeps to seed an isolated re-run
/// ([`seed_from`]), beside that path — which it keeps anyway, as its key.
///
/// Every boundary of a mount keeps one, and the path above a row is a
/// dozen segments deep. The segments were copied out of the path (end to
/// end, with where each one stops: two allocations a boundary), but the
/// path holds them already: only the cuts are kept, as short numbers
/// held inline up to a depth few trees reach. A deeper path, or one
/// longer than a short number counts, keeps its cuts on the heap. The
/// cuts are kept and not found again: a segment may hold a `/` of its
/// own (a row's key is the app's string), so the path cannot be split
/// back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSeed {
    ends: SeedEnds,
}

/// Where each parent segment ends in the path, the separator after it
/// not counted.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SeedEnds {
    Inline { count: u8, ends: [u16; SEED_INLINE] },
    Spilled(Box<[u32]>),
}

/// How many parent segments a seed holds without an allocation.
const SEED_INLINE: usize = 23;

impl Default for PathSeed {
    fn default() -> Self {
        PathSeed { ends: SeedEnds::Inline { count: 0, ends: [0; SEED_INLINE] } }
    }
}

impl PathSeed {
    /// The seed of the parents that end at `ends`.
    fn from_ends(ends: &[usize]) -> PathSeed {
        let short = ends.len() <= SEED_INLINE && ends.iter().all(|end| *end <= u16::MAX as usize);
        let ends = if short {
            let mut inline = [0u16; SEED_INLINE];
            for (slot, end) in inline.iter_mut().zip(ends) {
                *slot = *end as u16;
            }
            SeedEnds::Inline { count: ends.len() as u8, ends: inline }
        } else {
            SeedEnds::Spilled(ends.iter().map(|end| *end as u32).collect())
        };
        PathSeed { ends }
    }

    fn ends(&self) -> impl Iterator<Item = usize> + '_ {
        let (inline, spilled): (&[u16], &[u32]) = match &self.ends {
            SeedEnds::Inline { count, ends } => (&ends[..*count as usize], &[]),
            SeedEnds::Spilled(ends) => (&[], ends),
        };
        inline.iter().map(|end| *end as usize).chain(spilled.iter().map(|end| *end as usize))
    }

    /// Does the seed hold its cuts without an allocation?
    #[cfg(test)]
    fn is_inline(&self) -> bool {
        matches!(self.ends, SeedEnds::Inline { .. })
    }

    /// The parent segments, read back from `path` — the path the seed
    /// was taken at. A separator follows a non-empty path, so a segment
    /// starts one byte past the end of the last, or at the start.
    fn segments<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let mut from = 0usize;
        self.ends().map(move |end| {
            let segment = &path[from..end];
            from = end + usize::from(end > 0);
            segment
        })
    }
}

/// [`PathSeed`] of the cursor right now: every segment but the last, cut
/// from the cursor's path where the frames above it cut it.
pub fn parent_seed() -> PathSeed {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        // each frame saved where the path stood before it: the frame
        // after a parent saved where that parent ends
        let lens = &registry.joined_lens;
        debug_assert!(lens.first().is_none_or(|first| *first == 0), "the cursor starts at an empty path");
        PathSeed::from_ends(lens.get(1..).unwrap_or(&[]))
    })
}

/// The cursor's full path right now (`None` outside a pass) — the key
/// interactive nodes register their actions under. One clone of the
/// incrementally maintained path: no join walk, ever.
/// Diagnostics: the sizes of the register's tables — owners, views
/// with reads, dependencies with readers, views with bindings, bindings
/// with reads, dirty views, dirty bindings.
pub fn registry_counts() -> [usize; 7] {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        [
            registry.owners.len(),
            registry.reads_by_view.len(),
            registry.readers.len(),
            // the views that left still stand there until their bindings
            // are taken out: not views that keep any
            registry.view_bindings.len()
                - registry.leaving.iter().filter(|view| standing_bindings(&registry.view_bindings, view).is_some()).count(),
            registry.binding_reads.len(),
            registry.dirty.len(),
            registry.dirty_bindings.len(),
        ]
    })
}

/// Diagnostics, the tables [`registry_counts`] leaves out: the anchors,
/// the effect cells, the views leaving, the bindings retired, the
/// anchor sequence counters.
pub fn registry_more_counts() -> [usize; 8] {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        [
            registry.anchors.len(),
            registry.effect_cells.len(),
            registry.leaving.len(),
            registry.retired.len(),
            registry.seqs.len(),
            registry.effect_readers.len(),
            registry.paint_readers.len(),
            registry.dirty_paints.len(),
        ]
    })
}

pub fn cursor_scope() -> Option<String> {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        (registry.pass_active && !registry.joined.is_empty()).then(|| registry.joined.clone())
    })
}

/// [`cursor_scope`] as a shared string: the one copy an action's key,
/// its layout node and the element's own path all hold.
pub fn cursor_scope_rc() -> Option<Rc<str>> {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        (registry.pass_active && !registry.joined.is_empty()).then(|| Rc::from(registry.joined.as_str()))
    })
}

/// `{cursor scope}/{suffix}` as a shared string — the key a node's own
/// reading stands under. Spelled once into the register's buffer and
/// shared from there: one allocation, where a scope copy, a format
/// and a share were three.
pub fn cursor_key(suffix: &str) -> Option<Rc<str>> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active || registry.joined.is_empty() {
            return None;
        }
        let registry = &mut *registry;
        registry.key_scratch.clear();
        registry.key_scratch.push_str(&registry.joined);
        registry.key_scratch.push('/');
        registry.key_scratch.push_str(suffix);
        Some(Rc::from(registry.key_scratch.as_str()))
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
            // the joined path steps back by truncation — the bytes of
            // the parent are still in place, untouched
            let depth = registry.joined_lens.pop().unwrap_or(0);
            registry.joined.truncate(depth);
            if self.pops_view {
                registry.views.pop();
                registry.view_keys.pop();
            }
        });
    }
}

/// One step down: `spell` writes the segment at the end of the joined
/// path — a word as it is, a row's key inside its brackets.
fn push(spell: impl FnOnce(&mut String), is_view: bool) -> Frame {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if !registry.pass_active {
            return Frame { pops_view: false, active: false };
        }
        // the joined path grows in place: the segment is written now and
        // truncated on the frame's drop — the per-step full-path JOIN
        // died here
        let depth = registry.joined.len();
        registry.joined_lens.push(depth);
        if !registry.joined.is_empty() {
            registry.joined.push('/');
        }
        let start = registry.joined.len();
        spell(&mut registry.joined);
        if registry.pass_root.is_none() {
            registry.pass_root = Some(registry.joined[start..].to_string());
        }
        // the alive mark: an identity that owns something and was entered
        // this pass stays. One that owns nothing has no record to mark —
        // and nothing to sweep
        let registry = &mut *registry;
        if let Some(record) = registry.owners.get_mut(registry.joined.as_str()) {
            record.touched = registry.pass_no;
        }
        if is_view {
            registry.views.push(registry.joined.len());
            registry.view_keys.push(None);
        }
        Frame { pops_view: is_view, active: true }
    })
}

/// Steps down one structural level: tuple position (`#0`), arm (`@First`),
/// row key (`[USA]`), sheet content (`sheet`). A static word is borrowed
/// and costs nothing; a `String` is written into the path and let go —
/// the path is the one place a segment lives.
pub fn enter(segment: impl Into<Cow<'static, str>>) -> Frame {
    let segment = segment.into();
    push(|joined| joined.push_str(&segment), false)
}

/// Steps down into a list's row: the app's key inside brackets
/// (`[USA]`), written straight into the cursor's path. The segment is
/// never a string of its own — bracketing the app's string in place
/// grew it, a reallocation for every row each time a list ran.
pub fn enter_key(key: &str) -> Frame {
    push(
        |joined| {
            joined.push('[');
            joined.push_str(key);
            joined.push(']');
        },
        false,
    )
}

/// Steps down into a view's wrapper (`Component`) — besides the path, it
/// enters the view stack that read-tracking uses as its target.
pub fn enter_view(name: impl Into<Cow<'static, str>>) -> Frame {
    let name = name.into();
    push(|joined| joined.push_str(&name), true)
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
    segments.iter().map(|segment| push(|joined| joined.push_str(segment), false)).collect()
}

/// [`seed`], from the cuts a retained entry keeps and the path they cut
/// — the entry's own, which they were taken at.
pub fn seed_from(path: &str, parents: &PathSeed) -> Vec<Frame> {
    parents.segments(path).map(|segment| push(|joined| joined.push_str(segment), false)).collect()
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
/// set is whatever the body records now. `view` is the innermost open
/// view's path, shared: the bindings its body makes are filed under it.
pub fn begin_view_reads(view: &Rc<str>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        clear_view_reads(&mut registry, view);
        let registry = &mut *registry;
        if let (Some(len), Some(key)) = (registry.views.last(), registry.view_keys.last_mut()) {
            debug_assert_eq!(&registry.joined[..*len], &**view, "the view whose body runs is the innermost one open");
            *key = Some(Rc::clone(view));
        }
    });
}

/// Clears a view's reads and its bindings' reads.
fn clear_view_reads(registry: &mut Registry, view: &str) {
    // the bindings the body made read for themselves, but they are the
    // body's: a re-run makes them again, a death takes them along
    if let Some(bindings) = registry.view_bindings.remove(view) {
        for binding in bindings.iter() {
            clear_binding_reads(registry, binding);
        }
    }
    clear_own_reads(registry, view);
}

/// The reads the view's body made itself (its bindings' aside).
fn clear_own_reads(registry: &mut Registry, view: &str) {
    let Some(keys) = registry.reads_by_view.remove(view) else {
        return;
    };
    for key in keys {
        // one probe for the dependency: its readers are found, thinned
        // and, when the view was the last of them, taken out in place
        if let std::collections::hash_map::Entry::Occupied(mut readers) = registry.readers.entry(key) {
            readers.get_mut().remove(view);
            if readers.get().is_empty() {
                readers.remove();
            }
        }
    }
}

/// A view left the tree — the twin of the owner sweep, for a view that
/// owns no state and so has no owner record.
///
/// What its body read falls now: no write can make it dirty again, and
/// it never runs again. Its bindings are RETIRED: from here a write that
/// reaches one of them marks nothing, and taking their reads apart waits
/// for an idle moment ([`collect_retired`]). A thousand rows that leave
/// a list are two thousand bindings, and unpicking each — the readers of
/// every dependency, the sets freed one by one — was the larger part of
/// the click that let them go. Even taking their list from under the
/// view and filing each one as retired hashed a path and two thousand
/// keys that no write may ever ask for: the view is only noted, by the
/// shared path its bindings stand under, and they are taken out and filed
/// when a write does ask ([`file_retired`]).
pub fn retire_view(view: &Rc<str>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = &mut *registry;
        clear_own_reads(registry, view);
        // most frames that let views go hold no dirt at all: an empty
        // set is not asked, and the path is not hashed for it
        if !registry.dirty.is_empty() {
            registry.dirty.remove(&**view);
        }
        // a write that reached its bindings before it left is not news
        // to anyone now
        if !registry.dirty_bindings.is_empty()
            && let Some(bindings) = standing_bindings(&registry.view_bindings, view)
        {
            for binding in bindings.iter() {
                registry.dirty_bindings.remove(binding);
            }
        }
        registry.leaving.push(Rc::clone(view));
    })
}

/// The bindings still filed under the very path a view that left shared
/// — none, when a view made at its path since filed its own there.
fn standing_bindings<'a>(view_bindings: &'a HashMap<Rc<str>, BindingKeys>, view: &Rc<str>) -> Option<&'a BindingKeys> {
    view_bindings
        .get_key_value(&**view)
        .filter(|(standing, _)| Rc::ptr_eq(standing, view))
        .map(|(_, bindings)| bindings)
}

/// The identity of a binding's key: the copy it was made with.
fn identity(key: &Rc<str>) -> usize {
    Rc::as_ptr(key) as *const u8 as usize
}

/// Takes the bindings of the views that left since the last filing from
/// under them, and files each in the set a write asks. The retired list
/// holds their keys, so no key filed by its identity is freed — or
/// spelled again at the same place — while it stands in the set. A view
/// made at the same path since cleared the ones that left as it began,
/// and filed its own under a path of its own: nothing is taken.
fn file_retired(
    leaving: &mut Vec<Rc<str>>,
    view_bindings: &mut HashMap<Rc<str>, BindingKeys>,
    retired: &mut Vec<(Rc<str>, BindingKeys)>,
    set: &mut HashSet<usize>,
) {
    for view in leaving.drain(..) {
        if let std::collections::hash_map::Entry::Occupied(standing) = view_bindings.entry(Rc::clone(&view))
            && Rc::ptr_eq(standing.key(), &view)
        {
            let bindings = standing.remove();
            set.extend(bindings.iter().map(identity));
            retired.push((view, bindings));
        }
    }
}

/// [`file_retired`] on the register's own lists.
fn file_all_retired(registry: &mut Registry) {
    file_retired(&mut registry.leaving, &mut registry.view_bindings, &mut registry.retired, &mut registry.retired_bindings);
}

/// Are a retired binding's reads still the ones standing under its key?
/// Not when a body made the key again, clearing them and reading under a
/// copy of its own. A key whose binding read nothing has nothing standing
/// either way.
fn reads_stand(registry: &Registry, binding: &Rc<str>) -> bool {
    registry.binding_reads.get_key_value(&**binding).is_none_or(|(standing, _)| Rc::ptr_eq(standing, binding))
}

/// Is a filed binding still retired — not taken back by name, and its
/// reads still its own?
fn still_retired(registry: &Registry, binding: &Rc<str>) -> bool {
    registry.retired_bindings.contains(&identity(binding)) && reads_stand(registry, binding)
}

/// Takes apart the read graph of the bindings retired since the last
/// call ([`retire_view`]) and returns their keys, for the caller to drop
/// them where else they live. A key a body made again in the meantime is
/// left alone: its reads are the new binding's now.
pub fn collect_retired() -> Vec<Rc<str>> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = &mut *registry;
        if registry.retired.is_empty() && registry.leaving.is_empty() {
            return Vec::new();
        }
        let mut torn = Vec::new();
        let mut tear = |registry: &mut Registry, binding: Rc<str>| {
            clear_binding_reads(registry, &binding);
            torn.push(binding);
        };
        // the ones a write asked for were filed, and one may have been
        // taken back by name since
        let mut retired = std::mem::take(&mut registry.retired);
        for (_, bindings) in retired.drain(..) {
            for binding in bindings.into_keys() {
                if still_retired(registry, &binding) {
                    tear(registry, binding);
                }
            }
        }
        // the ones no write asked for are taken from under their views
        // here, unfiled: taking one back by name files them all first
        let mut leaving = std::mem::take(&mut registry.leaving);
        for view in leaving.drain(..) {
            if let std::collections::hash_map::Entry::Occupied(standing) = registry.view_bindings.entry(Rc::clone(&view))
                && Rc::ptr_eq(standing.key(), &view)
            {
                for binding in standing.remove().into_keys() {
                    if reads_stand(registry, &binding) {
                        tear(registry, binding);
                    }
                }
            }
        }
        registry.retired_bindings.clear();
        // the lists keep their room for the next rows that leave
        registry.retired = retired;
        registry.leaving = leaving;
        torn
    })
}

/// Does a view that left wait for [`collect_retired`]?
pub fn retirement_pending() -> bool {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        !registry.leaving.is_empty() || !registry.retired.is_empty()
    })
}

/// Bindings retired and not yet taken apart — diagnostics.
pub fn retired_count() -> usize {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let leaving: usize = registry
            .leaving
            .iter()
            .filter_map(|view| standing_bindings(&registry.view_bindings, view))
            .map(|bindings| bindings.iter().filter(|binding| reads_stand(&registry, binding)).count())
            .sum();
        let filed: usize = registry
            .retired
            .iter()
            .map(|(_, bindings)| bindings.iter().filter(|binding| still_retired(&registry, binding)).count())
            .sum();
        leaving + filed
    })
}

// MARK: - Bindings

/// A binding's evaluation in progress: every read until the drop
/// belongs to the binding.
pub struct BindingScope {
    previous: Option<Rc<str>>,
}

impl Drop for BindingScope {
    fn drop(&mut self) {
        REGISTRY.with(|registry| registry.borrow_mut().binding_scope = self.previous.take());
    }
}

/// Opens the scope of a binding's evaluation: the binding's old reads
/// fall away, and until the scope drops every read is recorded under
/// `key` — inside a pass or out of one. `owner` names the view whose
/// body made the binding, said once at the first evaluation, so the
/// body's re-run or death takes the binding's reads along.
pub fn begin_binding(key: &Rc<str>, owner: Option<&str>) -> BindingScope {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        clear_binding_reads(&mut registry, key);
        if let Some(owner) = owner {
            // filed under a view, the binding is a body's new one
            unretire(&mut registry, key);
            match registry.view_bindings.get_mut(owner) {
                Some(bindings) => bindings.push(Rc::clone(key)),
                None => {
                    registry.view_bindings.insert(Rc::from(owner), BindingKeys::One(Rc::clone(key)));
                }
            }
        }
        BindingScope { previous: registry.binding_scope.replace(Rc::clone(key)) }
    })
}

/// [`begin_binding`] under the view whose body is running now, read
/// off the register itself — the owner's path is a prefix of the
/// cursor, so naming it costs no copy. Outside a view (a read after
/// the pass) the binding has no owner, as before.
pub fn begin_binding_under_view(key: &Rc<str>) -> BindingScope {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = &mut *registry;
        file_under_view(registry, key);
        BindingScope { previous: registry.binding_scope.replace(Rc::clone(key)) }
    })
}

/// A new binding at `key`, made by the body that is running: what it
/// read before is gone, and it is filed under the view.
fn file_under_view(registry: &mut Registry, key: &Rc<str>) {
    // a binding made under a view is a body's new one, its key a copy
    // just spelled: never one that waits retired, whose reads it
    // clears here — and the idle, finding them gone, leaves it alone
    clear_binding_reads(registry, key);
    if let Some(len) = registry.views.last() {
        let owner = &registry.joined[..*len];
        match registry.view_bindings.get_mut(owner) {
            Some(bindings) => bindings.push(Rc::clone(key)),
            None => {
                // the body's own boundary handed its shared path over
                // when it began; a view with no retention spells one
                let owner = match registry.view_keys.last() {
                    Some(Some(shared)) => Rc::clone(shared),
                    _ => Rc::from(owner),
                };
                registry.view_bindings.insert(owner, BindingKeys::One(Rc::clone(key)));
            }
        }
    }
}

/// A node's first reading, before the node is a binding: open while the
/// scope lives ([`begin_probe`]).
pub struct ProbeScope {
    previous: Option<Rc<str>>,
    was_probing: bool,
}

impl Drop for ProbeScope {
    fn drop(&mut self) {
        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.binding_scope = self.previous.take();
            registry.probing = self.was_probing;
        });
    }
}

/// Opens a PROBE: a node's first reading, before the node is a binding.
///
/// A node that reads for itself takes a key, a binding object and a
/// place under the body that made it — and a reading that turns out to
/// read nothing threw all three away, since nothing can ever move it.
/// The probe reads first and decides after: every read until the scope
/// drops is kept aside, in a list of the probe's own, and the node then
/// either files them under the key it mints ([`file_probe_under_view`])
/// or, having read nothing, mints nothing at all
/// ([`probe_read_anything`]). A binding read while the probe is open
/// reads for itself, as it would inside any binding.
pub fn begin_probe() -> ProbeScope {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.probe_reads.clear();
        registry.probe_written = false;
        ProbeScope {
            previous: registry.binding_scope.take(),
            was_probing: std::mem::replace(&mut registry.probing, true),
        }
    })
}

/// Did the last probe read anything?
pub fn probe_read_anything() -> bool {
    REGISTRY.with(|registry| !registry.borrow().probe_reads.is_empty())
}

/// Files the last probe's reads under `key`, the binding minted for
/// them, and the binding under the view whose body is running — what
/// [`begin_binding_under_view`] and the reads themselves file when the
/// key is there before the reading.
pub fn file_probe_under_view(key: &Rc<str>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = &mut *registry;
        file_under_view(registry, key);
        let reads = std::mem::take(&mut registry.probe_reads);
        for dep in &reads {
            file_few(&mut registry.binding_reads, Rc::clone(key), *dep);
            file_few(&mut registry.binding_readers, *dep, Rc::clone(key));
        }
        // the list keeps its room for the next probe
        registry.probe_reads = reads;
        registry.probe_reads.clear();
        // a write that reached what the probe had read, while it read,
        // reaches the binding now
        if std::mem::take(&mut registry.probe_written) {
            registry.dirty_bindings.insert(Rc::clone(key));
        }
    })
}

/// Lets the last probe's reads go: the reading was not kept as a binding
/// (no key to keep it under — a render with no pass around it).
pub fn forget_probe() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.probe_reads.clear();
        registry.probe_written = false;
    });
}

/// The key is a live binding's again ([`retire_view`]).
fn unretire(registry: &mut Registry, key: &Rc<str>) {
    file_all_retired(registry);
    if !registry.retired_bindings.is_empty() {
        registry.retired_bindings.remove(&identity(key));
    }
}

fn clear_binding_reads(registry: &mut Registry, key: &str) {
    // a binding is cleared at each of its evaluations and at its death,
    // and between two frames the dirty set is nearly always empty: the
    // key is hashed for it only when there is something to find
    if !registry.dirty_bindings.is_empty() {
        registry.dirty_bindings.remove(key);
    }
    let Some(deps) = registry.binding_reads.remove(key) else {
        return;
    };
    for dep in deps.into_members() {
        if let std::collections::hash_map::Entry::Occupied(mut readers) = registry.binding_readers.entry(dep) {
            // the binding was the dependency's last reader: the entry goes
            if readers.get_mut().remove(key) {
                readers.remove();
            }
        }
    }
}

/// How many dependencies the binding read in its last evaluation. None
/// makes it a constant: nothing can ever move it.
pub fn binding_read_count(key: &str) -> usize {
    REGISTRY.with(|registry| registry.borrow().binding_reads.get(key).map_or(0, Few::len))
}

/// Did a write reach a binding since the last frame took the dirty ones?
pub fn has_dirty_bindings() -> bool {
    REGISTRY.with(|registry| !registry.borrow().dirty_bindings.is_empty())
}

/// The bindings a write reached, taken: the frame marks them stale and
/// the element lowering patches them by key.
pub fn take_dirty_bindings() -> Vec<Rc<str>> {
    REGISTRY.with(|registry| {
        let mut dirty: Vec<Rc<str>> = registry.borrow_mut().dirty_bindings.drain().collect();
        dirty.sort();
        dirty
    })
}

pub(crate) fn record_read(key: DepKey) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if let Some(binding) = registry.binding_scope.clone() {
            // a binding reads for itself — at a placement or at a
            // measure as much as inside a body
            file_few(&mut registry.binding_reads, Rc::clone(&binding), key);
            file_few(&mut registry.binding_readers, key, binding);
            return;
        }
        if registry.probing {
            // a first reading with no key yet: kept aside until one is
            // minted for it
            registry.probe_reads.push(key);
            return;
        }
        if registry.painting {
            // a box reads while it paints: a write to what it read repaints
            // the box, not the scene
            let registry = &mut *registry;
            if registry.painting_rc.is_none() {
                registry.painting_rc = Some(Rc::from(registry.painting_path.as_str()));
            }
            let path = registry.painting_rc.clone().expect("spelled just above");
            file_few(&mut registry.paint_readers, key, path);
            return;
        }
        if registry.effects_open {
            // an effect reads while the pump asks it: a write to what it
            // read is a reason to ask again
            registry.effect_readers.insert(key);
            return;
        }
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
    /// Counts the writes that REACHED the scene: a view, a binding, an
    /// effect, a probe read what was written — or a send came from outside
    /// the register's sight. A write nobody read moves it not at all.
    static SCENE_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The writes that reached nothing — a poller landing the same answer,
    /// a value nobody shows. Diagnostics: a tape prints it.
    static UNREACHED_WRITES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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

/// A number that moves each time a write REACHES the scene: a view read
/// what was written (it is dirty now), a binding did, an effect did on
/// the last pump, a probe did — or a send came from outside the
/// register's sight ([`note_external_write`]). A write nobody read does
/// not move it: the same number after a turn of tasks means no body, no
/// binding and no effect has anything new to say. A box's paint that read
/// the value is told apart ([`take_dirty_paints`]).
pub fn scene_epoch() -> u64 {
    SCENE_EPOCH.with(std::cell::Cell::get)
}

/// Diagnostics: the writes since launch that reached nothing at all.
pub fn unreached_writes() -> usize {
    UNREACHED_WRITES.with(|count| count.get() as usize)
}

/// A write the register did not see — a publisher's send, a value kept
/// outside `State` and `Store` that an effect polls for. It moves both
/// epochs, so the pump that would notice it runs.
pub fn note_external_write() {
    WRITE_EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
    SCENE_EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
}

/// The pump opens the effects' reading: what they read while open is
/// filed as theirs. `fresh` forgets the last settle's filing first — the
/// first pump of a settle says so, and the rounds that follow add to it.
pub fn begin_effects(fresh: bool) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.effects_open = true;
        if fresh {
            registry.effect_readers.clear();
        }
    });
}

pub fn end_effects() {
    REGISTRY.with(|registry| registry.borrow_mut().effects_open = false);
}

/// A box begins to paint: what it reads until [`end_paint`] is the box's.
/// The path is copied into a warm buffer — no string is made for a box
/// that reads nothing.
pub fn begin_paint(path: &str) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.painting = true;
        registry.painting_path.clear();
        registry.painting_path.push_str(path);
        registry.painting_rc = None;
    });
}

pub fn end_paint() {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.painting = false;
        registry.painting_rc = None;
    });
}

/// Did a write reach a box's paint since the last frame took them?
pub fn has_dirty_paints() -> bool {
    REGISTRY.with(|registry| !registry.borrow().dirty_paints.is_empty())
}

/// The boxes a write reached since the last call — a frame that painted
/// them all takes them; a shell that repaints boxes alone takes them to
/// know which.
pub fn take_dirty_paints() -> Vec<Rc<str>> {
    REGISTRY.with(|registry| registry.borrow_mut().dirty_paints.drain().collect())
}

pub(crate) fn record_write(key: DepKey) {
    WRITE_EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
    let reached = REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        // two fields of one registry: the readers are read, the dirty set
        // is written — and a reader that already ran this pass is noted
        let registry = &mut *registry;
        // who the write REACHES: a view, a binding, an effect, a probe —
        // the scene has something new; a box's paint alone — the box has
        let mut reached = false;
        // the open probe read it already: the binding it becomes hears it
        if registry.probing && registry.probe_reads.contains(&key) {
            registry.probe_written = true;
            reached = true;
        }
        if registry.effect_readers.contains(&key) {
            reached = true;
        }
        if let Some(boxes) = registry.paint_readers.get(&key) {
            let dirty = &mut registry.dirty_paints;
            for path in boxes.iter() {
                dirty.insert(Rc::clone(path));
            }
        }
        if let Some(readers) = registry.readers.get(&key).cloned() {
            reached = true;
            note_missed(registry, readers.iter().map(String::as_str));
            for reader in readers {
                // a write DURING a pass is served by it when the pass took
                // the reader's dirt and the reader's frame is still open
                // (its own write) or still to come — it reads the new value
                // then. Any other reader keeps the write for the next pass
                if registry.serving && served_by_this_pass(registry, &reader) {
                    continue;
                }
                registry.dirty.insert(reader);
            }
        }
        if let Some(bindings) = registry.binding_readers.get(&key) {
            reached = true;
            if registry.retired.is_empty() && registry.leaving.is_empty() {
                registry.dirty_bindings.extend(bindings.iter().cloned());
            } else {
                // a retired binding still stands in the readers until the
                // idle takes it apart — and a write reaches it no more
                file_retired(
                    &mut registry.leaving,
                    &mut registry.view_bindings,
                    &mut registry.retired,
                    &mut registry.retired_bindings,
                );
                let retired = &registry.retired_bindings;
                registry
                    .dirty_bindings
                    .extend(bindings.iter().filter(|binding| !retired.contains(&identity(binding))).cloned());
            }
        }
        reached
    });
    if reached {
        SCENE_EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
    } else {
        UNREACHED_WRITES.with(|count| count.set(count.get().wrapping_add(1)));
    }
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
    /// A seed re-enters the same segments from the path it was taken at —
    /// a row's key with a `/` of its own included, which is why the cut
    /// points are kept and the path is never split back — and holds its
    /// cuts inline, with no allocation.
    #[test]
    fn a_seed_re_enters_the_segments_it_was_cut_from() {
        use super::{begin_pass, current_path_segments, current_view_path, end_pass, enter, enter_view, parent_seed, seed_from};

        begin_pass();
        let (seed, path) = {
            let _root = enter("Root");
            let _row = enter("[a/b]");
            let _stack = enter("#0");
            let _leaf = enter_view("Leaf");
            (parent_seed(), current_view_path().expect("inside a view"))
        };
        let _ = end_pass();
        assert_eq!(path, "Root/[a/b]/#0/Leaf");
        assert!(seed.is_inline(), "a seed of three parents allocates nothing");
        assert_eq!(seed.segments(&path).collect::<Vec<_>>(), ["Root", "[a/b]", "#0"]);
        begin_pass();
        let frames = seed_from(&path, &seed);
        assert_eq!(current_path_segments(), ["Root", "[a/b]", "#0"]);
        drop(frames);
        let _ = end_pass();

        // an empty first segment adds no separator to the path: the
        // next segment starts where it ends, not one byte past it
        begin_pass();
        let (seed, path) = {
            let _scene = enter("");
            let _row = enter("[a/b]");
            let _leaf = enter_view("Leaf");
            (parent_seed(), current_view_path().expect("inside a view"))
        };
        let _ = end_pass();
        assert_eq!(path, "[a/b]/Leaf");
        assert_eq!(seed.segments(&path).collect::<Vec<_>>(), ["", "[a/b]"]);
    }

    /// A tree deeper than a seed holds inline, or a path longer than its
    /// short numbers count, keeps its cuts on the heap — and reads back
    /// the same segments all the same.
    #[test]
    fn a_seed_too_deep_or_too_long_for_inline_reads_back_the_same() {
        use super::{begin_pass, cursor_scope, end_pass, enter, parent_seed, Frame, SEED_INLINE};

        let walk = |words: &[String]| {
            begin_pass();
            let read = {
                let _frames: Vec<Frame> = words.iter().map(|word| enter(word.clone())).collect();
                let path = cursor_scope().expect("inside a pass");
                let seed = parent_seed();
                let segments = seed.segments(&path).map(str::to_string).collect::<Vec<_>>();
                (seed.is_inline(), segments)
            };
            let _ = end_pass();
            read
        };
        let deep: Vec<String> = (0..=SEED_INLINE + 1).map(|at| format!("#{at}")).collect();
        let (inline, segments) = walk(&deep);
        assert!(!inline, "deeper than the inline room");
        assert_eq!(segments, deep[..deep.len() - 1]);
        let long = vec!["Root".to_string(), "x".repeat(70_000), "[a/b]".to_string(), "Leaf".to_string()];
        let (inline, segments) = walk(&long);
        assert!(!inline, "longer than a short number counts");
        assert_eq!(segments, long[..long.len() - 1]);
    }

    /// A row's key is written into the path between its brackets, and
    /// the path is the only place the cursor's segments live: they read
    /// back from where each frame cut it — a key with a `/` of its own,
    /// an empty key and an empty word included — and the scope, the
    /// segments, the seed and the pass's root are what entering the
    /// bracketed string gave.
    #[test]
    fn a_key_written_into_the_path_reads_back_as_its_own_segment() {
        use super::{
            begin_pass, current_pass_root, current_path_segments, cursor_scope, end_pass, enter,
            enter_key, parent_seed,
        };

        let walk = |keyed: bool| {
            begin_pass();
            let read = {
                let _row = if keyed { enter_key("top") } else { enter("[top]") };
                let _empty = enter("");
                let _key = if keyed { enter_key("a/b") } else { enter("[a/b]") };
                let _blank = if keyed { enter_key("") } else { enter("[]") };
                let _leaf = enter("#0");
                let scope = cursor_scope();
                let seed = parent_seed();
                let parents = seed.segments(scope.as_deref().unwrap_or("")).map(str::to_string).collect::<Vec<_>>();
                (scope, current_path_segments(), parents)
            };
            let root = current_pass_root();
            // every frame dropped: the path is empty again
            assert_eq!(current_path_segments(), Vec::<String>::new());
            let _ = end_pass();
            (read, root)
        };
        let ((scope, segments, seed), root) = walk(true);
        assert_eq!(scope.as_deref(), Some("[top]//[a/b]/[]/#0"));
        assert_eq!(segments, ["[top]", "", "[a/b]", "[]", "#0"]);
        assert_eq!(seed, ["[top]", "", "[a/b]", "[]"]);
        assert_eq!(root.as_deref(), Some("[top]"));
        assert_eq!(walk(true), walk(false), "a key in place is the bracketed string, entered");
    }

    /// A body's bindings are filed under the view that made them, and the
    /// key is the very path its boundary shares — no copy is spelled for
    /// it, so a row that mounts allocates none and one that leaves frees
    /// none. A view that never handed its path over (no retention ran its
    /// body) still files them, under a copy of its own.
    #[test]
    fn a_view_files_its_bindings_under_the_path_its_boundary_shares() {
        use super::{begin_binding_under_view, begin_pass, begin_view_reads, cursor_key, end_pass, enter_view, REGISTRY};
        use std::rc::Rc;

        let filed_under = |view: &str| {
            REGISTRY.with(|registry| {
                registry.borrow().view_bindings.get_key_value(view).map(|(key, bindings)| (Rc::clone(key), bindings.len()))
            })
        };
        begin_pass();
        let shared: Rc<str> = Rc::from("Shared");
        {
            let _view = enter_view("Shared");
            begin_view_reads(&shared);
            let key = cursor_key("#text").expect("inside a pass");
            drop(begin_binding_under_view(&key));
            let again = cursor_key("#class").expect("inside a pass");
            drop(begin_binding_under_view(&again));
        }
        {
            let _view = enter_view("Spelled");
            let key = cursor_key("#text").expect("inside a pass");
            drop(begin_binding_under_view(&key));
        }
        let _ = end_pass();
        let (key, count) = filed_under("Shared").expect("the bindings are filed");
        assert!(Rc::ptr_eq(&key, &shared), "filed under the boundary's own path");
        assert_eq!(count, 2);
        let (spelled, count) = filed_under("Spelled").expect("filed without a shared path too");
        assert_eq!(&*spelled, "Spelled");
        assert_eq!(count, 1);
        super::reset_world();
    }

    /// A set of one is the member itself: a second insert of it adds
    /// nothing, a second member makes it a set, and taking the last
    /// member out says so — the holder takes the entry out, since an
    /// empty set has no form of its own. A member it does not hold
    /// leaves it as it was.
    #[test]
    fn a_set_of_one_is_its_member_until_a_second_arrives() {
        use super::Few;

        let mut few = Few::One(1u64);
        few.insert(1);
        assert!(matches!(few, Few::One(1)), "the same member, inserted again, is not a second");
        assert!(!few.remove(&2), "a member it does not hold leaves it as it was");
        assert!(matches!(few, Few::One(1)));
        few.insert(2);
        assert!(matches!(&few, Few::Many(set) if set.len() == 2), "a second member makes it a set");
        assert_eq!(few.iter().copied().collect::<std::collections::BTreeSet<_>>(), [1, 2].into());
        assert!(!few.remove(&1), "one member is left");
        assert!(few.remove(&2), "the last one out empties it");
        assert!(Few::One(7u64).remove(&7), "a set of one empties with its member");
    }

    /// A body's bindings are held in the order they were made: one and
    /// two in the width of two keys, a list from the third on — and every
    /// one of them is read back, by reference and by value.
    #[test]
    fn a_bodys_bindings_are_held_in_order_inline_up_to_two() {
        use super::BindingKeys;
        use std::rc::Rc;

        let keys: Vec<Rc<str>> = ["#0/#class", "#1/#text", "#2/#text"].into_iter().map(Rc::from).collect();
        let mut held = BindingKeys::One(Rc::clone(&keys[0]));
        held.push(Rc::clone(&keys[1]));
        assert!(matches!(held, BindingKeys::Two(..)), "two keys take no list");
        assert_eq!(held.iter().cloned().collect::<Vec<_>>(), keys[..2]);
        held.push(Rc::clone(&keys[2]));
        assert!(matches!(&held, BindingKeys::More(list) if list.len() == 3), "the third makes the list");
        assert_eq!(held.iter().cloned().collect::<Vec<_>>(), keys);
        assert_eq!(held.into_keys().collect::<Vec<_>>(), keys);
        assert!(std::mem::size_of::<BindingKeys>() <= 2 * std::mem::size_of::<Rc<str>>() + 8, "two keys wide");
    }

    /// The register of a row's bindings holds what a row has inline: the
    /// one value each binding reads, the one binding that reads it, the
    /// two bindings the row's body made. A write still reaches the
    /// binding, a second reader of the same value still joins it, and
    /// the body's re-run still takes every entry out.
    #[test]
    fn a_rows_bindings_are_filed_inline_and_unfiled_whole() {
        use super::{
            BindingKeys, DepKey, Few, REGISTRY, begin_binding_under_view, begin_pass, begin_view_reads, cursor_key,
            end_pass, enter_view, record_read, record_write, reset_world, take_dirty_bindings,
        };
        use std::rc::Rc;

        reset_world();
        let label = DepKey::State(1);
        let flag = DepKey::State(2);
        let row: Rc<str> = Rc::from("Row");
        begin_pass();
        let (text, class) = {
            let _view = enter_view("Row");
            begin_view_reads(&row);
            let text = cursor_key("#text").expect("inside a pass");
            let scope = begin_binding_under_view(&text);
            record_read(label);
            drop(scope);
            let class = cursor_key("#class").expect("inside a pass");
            let scope = begin_binding_under_view(&class);
            record_read(flag);
            drop(scope);
            (text, class)
        };
        let _ = end_pass();
        REGISTRY.with(|registry| {
            let registry = registry.borrow();
            assert!(matches!(registry.binding_reads.get(&text), Some(Few::One(dep)) if *dep == label));
            assert!(matches!(registry.binding_readers.get(&flag), Some(Few::One(key)) if *key == class));
            let filed = registry.view_bindings.get("Row").expect("the row's bindings are filed");
            assert!(
                matches!(filed, BindingKeys::Two(first, second) if *first == text && *second == class),
                "two bindings take no list, held in the order they were made"
            );
        });

        record_write(label);
        assert_eq!(take_dirty_bindings(), vec![Rc::clone(&text)], "a write reaches the binding that read it");

        // a second reader of the label joins the first
        let other: Rc<str> = Rc::from("Other/#text");
        drop(super::begin_binding(&other, None));
        {
            let scope = super::begin_binding(&other, None);
            record_read(label);
            drop(scope);
        }
        REGISTRY.with(|registry| {
            assert_eq!(registry.borrow().binding_readers.get(&label).map(Few::len), Some(2), "two readers now");
        });

        // the row's body runs again: its bindings' entries all leave
        begin_pass();
        {
            let _view = enter_view("Row");
            begin_view_reads(&row);
        }
        let _ = end_pass();
        REGISTRY.with(|registry| {
            let registry = registry.borrow();
            assert!(!registry.binding_reads.contains_key(&text) && !registry.binding_reads.contains_key(&class));
            assert!(!registry.binding_readers.contains_key(&flag), "the flag's one reader left with its entry");
            assert_eq!(registry.binding_readers.get(&label).map(Few::len), Some(1), "the other reader stays");
            assert!(!registry.view_bindings.contains_key("Row"));
        });
        reset_world();
    }

    /// A probe reads before the binding has a key: its reads are kept
    /// aside, out of the tables, and filed whole under the key minted
    /// for them — both directions — or let go when no key is minted. A
    /// probe that read nothing says so; a binding read inside a probe
    /// reads for itself; a write that reached what the probe had read
    /// reaches the binding it becomes.
    #[test]
    fn a_probe_files_its_reads_under_the_key_minted_for_them() {
        use super::{
            begin_binding, begin_pass, begin_probe, begin_view_reads, end_pass, enter_view, file_probe_under_view,
            forget_probe, probe_read_anything, record_read, record_write, registry_counts, reset_world,
            take_dirty_bindings, DepKey, Few, REGISTRY,
        };
        use std::rc::Rc;

        reset_world();
        let label = DepKey::State(1);
        let inner = DepKey::State(3);
        let row: Rc<str> = Rc::from("Row");
        let key: Rc<str> = Rc::from("Row/#text");
        let nested: Rc<str> = Rc::from("Elsewhere/#text");
        begin_pass();
        {
            let _view = enter_view("Row");
            begin_view_reads(&row);
            drop(begin_probe());
            assert!(!probe_read_anything(), "a probe that read nothing says so");
            let probe = begin_probe();
            record_read(label);
            assert_eq!(registry_counts()[4], 0, "nothing is filed while the probe reads");
            {
                let _binding = begin_binding(&nested, None);
                record_read(inner);
            }
            drop(probe);
            assert!(probe_read_anything());
            file_probe_under_view(&key);
        }
        let _ = end_pass();
        REGISTRY.with(|registry| {
            let registry = registry.borrow();
            assert!(matches!(registry.binding_reads.get(&key), Some(Few::One(dep)) if *dep == label));
            assert!(matches!(registry.binding_readers.get(&label), Some(Few::One(reader)) if *reader == key));
            assert!(matches!(registry.binding_readers.get(&inner), Some(Few::One(reader)) if *reader == nested), "the inner binding read for itself");
            assert_eq!(registry.view_bindings.get("Row").map(|bindings| bindings.len()), Some(1), "filed under the row");
        });
        record_write(label);
        assert_eq!(take_dirty_bindings(), vec![Rc::clone(&key)], "a write reaches the key");

        // a write during the reading, to what it read, reaches the key
        let probe = begin_probe();
        record_read(label);
        record_write(label);
        drop(probe);
        let _ = take_dirty_bindings();
        file_probe_under_view(&key);
        assert_eq!(take_dirty_bindings(), vec![Rc::clone(&key)], "the write during the reading");

        // a probe whose reading is not kept leaves nothing behind
        let probe = begin_probe();
        record_read(DepKey::State(2));
        drop(probe);
        forget_probe();
        assert!(!probe_read_anything());
        assert_eq!(registry_counts()[4], 2, "only the bindings' reads stand");
    }

    /// One pass: a view at `view`, its path shared the way a boundary
    /// shares it, whose body makes one binding that reads `count`. Returns
    /// the shared path and the binding's key.
    fn bind_under(view: &'static str, count: crate::state::State<u32>) -> (std::rc::Rc<str>, std::rc::Rc<str>) {
        use super::{begin_binding_under_view, begin_pass, begin_view_reads, cursor_key, end_pass, enter_view};
        let shared: std::rc::Rc<str> = std::rc::Rc::from(view);
        begin_pass();
        let key = {
            let _view = enter_view(view);
            begin_view_reads(&shared);
            let key = cursor_key("#text").expect("inside a pass");
            let _reading = begin_binding_under_view(&key);
            let _ = count.wrappedValue();
            key
        };
        let _ = end_pass();
        (shared, key)
    }

    fn filed_retired() -> usize {
        super::REGISTRY.with(|registry| registry.borrow().retired_bindings.len())
    }

    fn standing_under(view: &str) -> usize {
        super::REGISTRY.with(|registry| registry.borrow().view_bindings.get(view).map_or(0, super::BindingKeys::len))
    }

    /// A view that leaves is only noted: its bindings stay filed under it,
    /// none is filed as retired — a list that clears is two thousand of
    /// them, and no one asked in the click that let them go — and the
    /// register counts no view that keeps any. The first write that
    /// reaches a binding takes them out and files them, and reaches none
    /// of them: the row that left hears nothing. The idle takes their
    /// reads apart.
    #[test]
    fn a_view_that_leaves_files_its_bindings_only_when_a_write_asks() {
        use super::{binding_read_count, collect_retired, has_dirty_bindings, registry_counts, reset_world, retire_view, retired_count};
        reset_world();
        let count = crate::state::State::new(0u32);
        let (view, key) = bind_under("Row", count);
        assert_eq!(binding_read_count(&key), 1);

        retire_view(&view);
        assert_eq!((standing_under("Row"), filed_retired()), (1, 0), "the view left, and was only noted");
        assert_eq!(registry_counts()[3], 0, "no view keeps bindings");
        assert_eq!(retired_count(), 1, "its binding waits retired all the same");

        count.set(1);
        assert!(!has_dirty_bindings(), "the write reached no binding of the view that left");
        assert_eq!((standing_under("Row"), filed_retired()), (0, 1), "the write asked: taken out and filed");

        let torn = collect_retired();
        assert_eq!(torn.len(), 1);
        assert!(std::rc::Rc::ptr_eq(&torn[0], &key), "the idle took apart the very binding that left");
        assert_eq!(binding_read_count(&key), 0);
        assert_eq!((filed_retired(), retired_count()), (0, 0));

        // and with no write between, the idle takes them out itself
        let (view, key) = bind_under("Row", count);
        retire_view(&view);
        assert_eq!(collect_retired().len(), 1);
        assert_eq!((standing_under("Row"), binding_read_count(&key)), (0, 0));
        reset_world();
    }

    /// A body that makes a retired key again spells a copy of its own: a
    /// new binding, which the retired one never was. It files nothing as
    /// it is made, it hears the next write, and the idle that takes the
    /// retired binding apart leaves the new one's reads standing.
    #[test]
    fn a_key_made_again_is_a_new_binding_the_retired_one_never_was() {
        use super::{binding_read_count, collect_retired, reset_world, retire_view, retired_count, take_dirty_bindings};
        reset_world();
        let count = crate::state::State::new(0u32);
        let (view, left) = bind_under("Row", count);
        retire_view(&view);
        let (_, again) = bind_under("Row", count);
        assert_eq!(&*again, &*left, "the same key");
        assert!(!std::rc::Rc::ptr_eq(&again, &left), "spelled again");
        assert_eq!(filed_retired(), 0, "making it again filed nothing");
        assert_eq!(retired_count(), 0, "the key is a live binding's again");

        count.set(1);
        let dirty = take_dirty_bindings();
        assert_eq!(dirty.len(), 1, "the write reached the new binding: {dirty:?}");
        assert!(std::rc::Rc::ptr_eq(&dirty[0], &again));

        assert!(collect_retired().is_empty(), "nothing of the retired binding stood");
        assert_eq!(binding_read_count(&again), 1, "the new binding's reads stand");
        count.set(2);
        assert_eq!(take_dirty_bindings().len(), 1, "and it still hears writes");
        reset_world();
    }

    /// The door that files a binding under an owner it names takes a
    /// retired binding back — the very copy of the key, live again: a
    /// write reaches it, and the idle leaves its reads standing.
    #[test]
    fn a_binding_filed_again_by_name_is_taken_back() {
        use super::{begin_binding, binding_read_count, collect_retired, reset_world, retire_view, retired_count, take_dirty_bindings};
        reset_world();
        let count = crate::state::State::new(0u32);
        let (view, key) = bind_under("Row", count);
        retire_view(&view);
        {
            let _reading = begin_binding(&key, Some("Elsewhere"));
            let _ = count.wrappedValue();
        }
        assert_eq!(retired_count(), 0, "taken back");
        count.set(1);
        assert_eq!(take_dirty_bindings().len(), 1, "a write reaches it");
        assert!(collect_retired().is_empty(), "the idle leaves it alone");
        assert_eq!(binding_read_count(&key), 1);
        reset_world();
    }

    /// The skips of a pass are asked for one thing: an owner the pass did
    /// not touch, whose nearest skipped boundary shelters it. An owner born
    /// during the pass is touched by its birth, so a pass that began with no
    /// owner keeps none of its skips — a list of a thousand kept rows files
    /// nothing — and the owner it bears lives all the same. The next pass,
    /// which begins with that owner standing, keeps its skips again, and the
    /// skip shelters the owner it did not visit.
    #[test]
    fn a_pass_that_begins_with_no_owner_keeps_no_skips() {
        use super::{begin_pass, end_pass, enter, mark_skipped, reset_world, REGISTRY};
        use std::rc::Rc;

        reset_world();
        let skipped = || REGISTRY.with(|registry| registry.borrow().skipped.len());
        let owners = || REGISTRY.with(|registry| registry.borrow().owners.len());
        let kept: Rc<str> = Rc::from("Root/Kept");
        begin_pass();
        {
            let _root = enter("Root");
            mark_skipped(&kept);
            let _row = enter("Row");
            let _ = crate::state::State::new(0u8);
        }
        assert_eq!(skipped(), 0, "no owner stood when the pass began");
        assert!(end_pass().is_empty(), "the owner born in the pass lives");
        assert_eq!(owners(), 1);

        begin_pass();
        {
            let _root = enter("Root");
            mark_skipped(&Rc::from("Root/Row"));
        }
        assert_eq!(skipped(), 1, "an owner stood: the skips are kept");
        assert!(end_pass().is_empty(), "the skip sheltered the owner it did not visit");
        assert_eq!(owners(), 1);

        begin_pass();
        {
            let _root = enter("Root");
            mark_skipped(&kept);
        }
        assert_eq!(end_pass(), ["Root/Row"], "unsheltered and unvisited, it died");
        reset_world();
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

#[cfg(test)]
mod reach_tests {
    use super::*;
    use crate::state::State;

    #[test]
    fn a_write_nobody_read_moves_no_scene() {
        let state = State::new(1u32);
        let scene = scene_epoch();
        let writes = write_epoch();
        let unreached = unreached_writes();
        state.set(2);
        assert_eq!(scene_epoch(), scene, "nobody read it: the scene has nothing new");
        assert_ne!(write_epoch(), writes, "but it was a write");
        assert_eq!(unreached_writes(), unreached + 1);
    }

    #[test]
    fn a_write_an_effect_read_is_a_scene_write() {
        let state = State::new(1u32);
        begin_effects(true);
        let _ = state.wrappedValue();
        end_effects();
        let scene = scene_epoch();
        state.set(2);
        assert_ne!(scene_epoch(), scene, "the effect that read it is asked again");
    }

    #[test]
    fn a_fresh_pump_forgets_the_last_settles_readers() {
        let state = State::new(1u32);
        begin_effects(true);
        let _ = state.wrappedValue();
        end_effects();
        begin_effects(true);
        end_effects();
        let scene = scene_epoch();
        state.set(2);
        assert_eq!(scene_epoch(), scene, "no effect read it on the last settle");
    }

    #[test]
    fn a_write_a_paint_read_dirties_the_box_not_the_scene() {
        let state = State::new(1u32);
        begin_paint("page/box");
        let _ = state.wrappedValue();
        end_paint();
        let scene = scene_epoch();
        state.set(2);
        assert_eq!(scene_epoch(), scene, "the scene did not move");
        assert!(has_dirty_paints());
        let dirty = take_dirty_paints();
        assert_eq!(dirty.iter().map(|path| &**path).collect::<Vec<_>>(), ["page/box"]);
        assert!(!has_dirty_paints(), "taken once");
    }

    #[test]
    fn a_subjects_send_is_a_scene_write() {
        let scene = scene_epoch();
        note_external_write();
        assert_ne!(scene_epoch(), scene);
    }

    #[test]
    fn set_if_changed_writes_only_what_moved() {
        let state = State::new(1u32);
        begin_effects(true);
        let _ = state.wrappedValue();
        end_effects();
        let scene = scene_epoch();
        assert!(!state.set_if_changed(1), "the same value is no write");
        assert_eq!(scene_epoch(), scene);
        assert!(state.set_if_changed(2));
        assert_ne!(scene_epoch(), scene);
        assert_eq!(state.wrappedValue(), 2);
        let scene = scene_epoch();
        let doubled = state.update_if(|value| {
            if *value > 100 {
                (false, *value)
            } else {
                *value *= 2;
                (true, *value)
            }
        });
        assert_eq!(doubled, 4);
        assert_ne!(scene_epoch(), scene, "it changed, it wrote");
        let scene = scene_epoch();
        let kept = state.update_if(|value| (false, *value));
        assert_eq!(kept, 4);
        assert_eq!(scene_epoch(), scene, "it did not change, it did not write");
    }
}
