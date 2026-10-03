//! The reconciler — the tree retained by identity.
//!
//! Each view boundary (`Component`) leaves an [`Entry`] here: the
//! view's VALUE (re-runnable, erased behind an `Erased`), the `Context`
//! it rendered with (ancestor environment already applied), the printed
//! output, and the effects the body registered.
//!
//! At render, the boundary decides: clean and retained → SKIP the body
//! and emit a reference (the final assembly expands from the cache);
//! dirty, new, or inside a body that re-ran (the parent built new
//! values — the config may have changed) → run and re-retain. A dirty
//! view behind a skipped parent re-runs ISOLATED from the retained
//! value, with the cursor re-seeded on the path.
//!
//! Effects of skipped views keep pumping: each pass's queue is
//! reassembled from the retention (they are the live subscription).
//! `onAppear` of a skipped view does NOT fire — which brings the fake
//! closer to the real semantics (appear is mount, not frame).
//!
//! The output references boundaries by a marker on the line itself
//! (`\u{1}path\u{1}suffixes…`) — internal to the opaque [`NodeList`];
//! expansion resolves recursively against the retention, applies the
//! accumulated modifier suffixes, and re-appends extra children (the
//! `Sheet` node).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use motor::hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::rc::Rc;

use motor::state::{Context, EffectFn};
use motor::view::RenderNode;

use crate::erased::Erased;
use crate::layout::LayoutNode;
use crate::text_input::{CaretState, EditCommand};

/// An interactive action registered during render: (target path, what
/// the click fires).
/// What a click hands the app: the platform's own count for the press
/// that armed it — 1, then 2 on the double, 3 on the triple. The
/// framework holds no clock; it carries what the shell counted.
pub(crate) type ClickAction = Rc<dyn Fn(u8)>;

pub(crate) type ActionEntry = (String, ClickAction);

/// What a `.on_copy` answers when ⌘C reaches it: the text of what the
/// view has selected, or `None` when nothing is. Retained like the
/// actions — a skipped view's table still copies.
pub(crate) type CopyFn = Rc<dyn Fn() -> Option<String>>;

pub(crate) type CopyEntry = (String, CopyFn);

/// A text field's editor: applies a command to the (binding, caret)
/// pair and returns the output of `Read`/`Copy`/`Cut`. Retained like
/// the actions — a skipped view's field still edits.
type EditFn = Rc<dyn Fn(EditCommand, &mut CaretState) -> Option<String>>;
pub(crate) type FieldKeyFn = Rc<dyn Fn(&crate::action::Stroke, &mut CaretState) -> bool>;
#[derive(Clone)]
pub(crate) struct EditorFn {
    pub submit_on_enter: bool,
    /// After its submit runs, the field hands a borrowed keyboard back to
    /// its lender (`TextField::yield_on_submit`).
    pub yields_on_submit: bool,
    /// While it reads true the field declines the bare vertical arrows
    /// and the bare Enter (`TextField::nav_intercept`).
    pub nav_intercept: Option<motor::state::Binding<bool>>,
    /// Where a pasted picture goes (`TextField::on_paste_image`).
    pub paste_image: Option<Rc<dyn Fn(crate::clipboard::ClipboardImage)>>,
    pub command: EditFn,
    pub key: Option<FieldKeyFn>,
    pub policy: Option<Rc<dyn crate::text_input::EditingStrategy>>,
    /// The app's word for the keyboard reaching or leaving the field
    /// (`TextField::on_focus`).
    pub focus: Option<Rc<dyn Fn(bool)>>,
    /// The keys a software keyboard lays out for the field
    /// (`TextField::keyboard_type`).
    pub keyboard: crate::text_input::KeyboardType,
}
pub(crate) type EditorEntry = (String, EditorFn);

/// A split divider's position writer: the drag hands it the new lane-A
/// extent in layout points and it reaches the app's binding. Retained
/// like the actions — a skipped view's divider still drags.
pub(crate) type SplitFn = Rc<dyn Fn(crate::layout::Px)>;
pub(crate) type SplitEntry = (String, SplitFn);
/// A scroll-offset writer registered at render: where the region
/// landed goes back to the binding the app is holding it in.
pub(crate) type ScrollFn = Rc<dyn Fn(crate::layout::Point)>;
pub(crate) type ScrollEntry = (String, ScrollFn);
/// A measurement probe registered at render: the size a view resolved
/// to goes back to the app that asked for it.
pub(crate) type MeasureFn = Rc<dyn Fn(crate::layout::Size)>;
pub(crate) type MeasureEntry = (String, MeasureFn);

/// A key context declared at render: (the declaring view's path, the
/// name, whether it counts only while the keyboard is inside that view).
/// Retained like the handlers — a skipped view's context stays declared.
pub(crate) type ContextEntry = (String, &'static str, bool);

/// The mark `.leaves_keyboard()` declares. It rides the key-context table
/// because that table is the framework's retained record of subtree marks
/// — a skipped view keeps its mark, an unmounted one drops it — and no
/// binding ever names it: the `bunny.` prefix is the house's.
pub(crate) const KEYBOARD_NEUTRAL: &str = "bunny.keyboard-neutral";

/// A NAMED action handler registered at render: (registration path,
/// id, what runs). Retained like the actions — a skipped view's
/// handler lives.
pub(crate) type HandlerFn = Rc<dyn Fn()>;
pub(crate) type HandlerEntry = (String, crate::action::ActionId, HandlerFn);

/// A webview's app-side hooks, registered at render: the navigation
/// and message writers, and the handle's command queue. Retained like
/// the scroll writers — a skipped body's page keeps reporting, and
/// its handle keeps commanding.
#[derive(Clone)]
pub(crate) struct WebviewHooks {
    /// A link in a document was activated — fires with its url; the
    /// document itself never moves.
    pub linked: Option<WebviewReport>,
    /// An editable document changed — fires with the body's html.
    pub changed: Option<WebviewReport>,
    /// A paste the app owns — fires with the clipboard's html and text.
    pub pasted: Option<WebviewPaste>,
    /// The page moved — fires with the committed url.
    pub navigated: Option<WebviewReport>,
    /// The page did NOT move — fires with the url and the reason.
    pub failed: Option<WebviewFailure>,
    /// The page posted — fires with the string it sent.
    pub posted: Option<WebviewReport>,
    /// The page's console spoke — fires with `"level: what it said"`.
    pub console: Option<WebviewReport>,
    /// A request of the page's completed — `"METHOD url status"`.
    pub requested: Option<WebviewReport>,
    /// The handle's queue — drained by the runtime, spent by the shell.
    pub commands: Option<crate::host::CommandQueue>,
}

/// A page report's writer — a navigation or a posted message, handed
/// to the app as the string it is.
pub(crate) type WebviewReport = Rc<dyn Fn(&str)>;
/// A refused load's writer — the url it tried, and why it never
/// arrived. Two strings because a failure that hides either half
/// tells the app nothing it can act on.
pub(crate) type WebviewFailure = Rc<dyn Fn(&str, &str)>;
/// A paste's writer — the clipboard's html and its text, both handed
/// over because the app decides which it wants.
pub(crate) type WebviewPaste = Rc<dyn Fn(&str, &str)>;
pub(crate) type WebviewEntry = (String, WebviewHooks);

pub(crate) struct Entry {
    pub value: Erased,
    pub ctx: Context,
    pub node: RenderNode,
    /// The body's layout tree — retained along with the print (the two
    /// outputs of the same body-eval) — and the measures of it that were
    /// kept, behind the SLOT a `BoundaryRef` holds: the layout reaches
    /// both without asking the retention for a path. The slot outlives
    /// the entry it was made for: a body that re-runs fills the same one,
    /// so a parent that did NOT re-run still refers to the tree of today.
    pub slot: Rc<Slot>,
    pub effects: Vec<EffectFn>,
    /// The body's interactive actions — retained like the effects: a
    /// skipped view's button stays clickable.
    pub actions: Vec<ActionEntry>,
    /// The body's `.on_copy` answers — same retention.
    pub copies: Vec<CopyEntry>,
    /// The body's field editors — same retention.
    pub editors: Vec<EditorEntry>,
    /// The body's split-position writers — same retention.
    pub splits: Vec<SplitEntry>,
    /// The body's scroll-offset writers — same retention.
    pub scrolls: Vec<ScrollEntry>,
    /// The body's measurement probes — same retention.
    pub measures: Vec<MeasureEntry>,
    /// The body's webview hooks — same retention.
    pub webviews: Vec<WebviewEntry>,
    /// The paths of the app's own boxes (`custom(…)`) — the map that
    /// says a focused escape hatch is still on screen.
    /// `(path, does it take the keyboard)` — the second half is
    /// what keeps a re-point from handing the keyboard to a box
    /// that answers nothing.
    pub customs: Vec<(String, bool)>,
    /// The body's named-action handlers — same retention.
    pub handlers: Vec<HandlerEntry>,
    /// Key contexts declared in the body (`.key_context(name)`) — a
    /// context is ACTIVE while a view declaring it stays mounted, or
    /// (`.key_context_focused(name)`) while the keyboard is inside it.
    pub contexts: Vec<ContextEntry>,
    /// The PARENT's path segments, packed — the cursor seed for an isolated
    /// re-run.
    pub parent_segments: motor::identity::PathSeed,
}

#[derive(Default)]
struct BuildingFrame {
    path: String,
    /// The frame of a keyed list: the rows under it are kept by key.
    list: bool,
    effects: Vec<EffectFn>,
    actions: Vec<ActionEntry>,
    copies: Vec<CopyEntry>,
    editors: Vec<EditorEntry>,
    splits: Vec<SplitEntry>,
    scrolls: Vec<ScrollEntry>,
    measures: Vec<MeasureEntry>,
    webviews: Vec<WebviewEntry>,
    customs: Vec<(String, bool)>,
    handlers: Vec<HandlerEntry>,
    contexts: Vec<ContextEntry>,
}

#[derive(Default)]
struct PassState {
    active: bool,
    /// Snapshot of the dirty set at pass start — decides who re-runs.
    dirty: HashSet<String>,
    /// Stack of entries being built (the top collects effects and actions).
    building: Vec<BuildingFrame>,
    /// Effects from the root region (outside any boundary) — they
    /// re-run on every walk.
    root_effects: Vec<EffectFn>,
    root_actions: Vec<ActionEntry>,
    root_copies: Vec<CopyEntry>,
    root_editors: Vec<EditorEntry>,
    root_splits: Vec<SplitEntry>,
    root_scrolls: Vec<ScrollEntry>,
    root_measures: Vec<MeasureEntry>,
    root_webviews: Vec<WebviewEntry>,
    root_customs: Vec<(String, bool)>,
    root_handlers: Vec<HandlerEntry>,
    root_contexts: Vec<ContextEntry>,
    /// Instrumentation: bodies that ran in this pass.
    body_runs: Vec<String>,
    /// Boundaries SKIPPED in this pass — a skipped one's subtree
    /// survives the entry sweep (the walk stayed out on purpose).
    skipped: Vec<String>,
}

thread_local! {
    static RETAINED: RefCell<BTreeMap<String, Entry>> = const { RefCell::new(BTreeMap::new()) };
    static PASS: RefCell<PassState> = RefCell::new(PassState::default());
    static LAST_BODY_RUNS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static FRAME_BODY_RUNS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static LIVE: RefCell<Live> = RefCell::new(Live::default());
}

// MARK: - The live registry

/// The tables the input doors read, kept TRUE at every change of the
/// retention instead of rebuilt from it.
///
/// A registration enters the retention when a body closes its entry
/// and leaves it when the entry falls — and those are the only two
/// moments these tables move. A body that runs in a list of a thousand
/// used to make every pass walk the thousand entries ten times over to
/// rebuild what one entry had changed; now the entry's own keys go in
/// and out, and a pass that ran one body pays for one body.
///
/// Paths are unique across every scene on the thread, so the maps hold
/// all scenes at once. A door that LOOKS UP a path reads them directly;
/// a door that ENUMERATES (an input by its name, the handles with
/// commands) keeps to the scene the tables answer for.
///
/// Two tables are DERIVED, not kept: the named-action handlers (the
/// deepest registration wins) and the key contexts (sorted outermost
/// first). They rebuild only when an entry that carries one is indexed
/// or dropped — a generation number per kind says when — and from the
/// entries that carry one, never from the whole retention.
#[derive(Default)]
struct Live {
    actions: HashMap<String, ClickAction>,
    copies: HashMap<String, CopyFn>,
    editors: HashMap<String, EditorFn>,
    splits: HashMap<String, SplitFn>,
    scrolls: HashMap<String, ScrollFn>,
    measures: HashMap<String, MeasureFn>,
    webviews: HashMap<String, WebviewHooks>,
    /// The app's boxes on screen — paths only.
    customs: HashSet<String>,
    /// The subset that answers `accepts_keys` — who may HOLD the
    /// keyboard, as opposed to who is merely on screen.
    keyed_customs: HashSet<String>,
    /// How many action keys are `.on_hover` registrations — a scene with
    /// none pays nothing for the hover road.
    hover_keys: usize,
    /// The entries that carry handlers, contexts and effects: the three
    /// derived products rebuild from these alone.
    handler_entries: HashSet<String>,
    context_entries: HashSet<String>,
    effect_entries: HashSet<String>,
    /// Moves when an entry carrying a handler is indexed or dropped.
    handler_gen: u64,
    /// The same, for key contexts.
    context_gen: u64,
    /// The same, for effects.
    effect_gen: u64,
    /// The keys the ROOT REGION of the last assembly put into the maps.
    /// Those registrations live for one pass: the next assembly takes
    /// them out before it puts the new region's in.
    root_keys: RootKeys,
    /// Entries with no retained boundary above them — the ones only the
    /// root region mounts, and so the only ones the root region can
    /// unmount. The sweep reads this instead of searching for them.
    top_level: HashSet<String>,
}

#[derive(Default)]
struct RootKeys {
    actions: Vec<String>,
    copies: Vec<String>,
    editors: Vec<String>,
    splits: Vec<String>,
    scrolls: Vec<String>,
    measures: Vec<String>,
    webviews: Vec<String>,
    customs: Vec<String>,
}

impl Live {
    fn insert_action(&mut self, key: String, action: ClickAction) {
        let hover = key.ends_with(HOVER_KEY);
        if self.actions.insert(key, action).is_none() && hover {
            self.hover_keys += 1;
        }
    }

    fn remove_action(&mut self, key: &str) {
        if self.actions.remove(key).is_some() && key.ends_with(HOVER_KEY) {
            self.hover_keys -= 1;
        }
    }

    /// Puts one closed entry's registrations into the tables.
    fn index(&mut self, path: &str, entry: &Entry) {
        for (key, action) in &entry.actions {
            self.insert_action(key.clone(), Rc::clone(action));
        }
        for (key, copy) in &entry.copies {
            self.copies.insert(key.clone(), Rc::clone(copy));
        }
        for (key, editor) in &entry.editors {
            self.editors.insert(key.clone(), editor.clone());
        }
        for (key, split) in &entry.splits {
            self.splits.insert(key.clone(), Rc::clone(split));
        }
        for (key, scroll) in &entry.scrolls {
            self.scrolls.insert(key.clone(), Rc::clone(scroll));
        }
        for (key, measure) in &entry.measures {
            self.measures.insert(key.clone(), Rc::clone(measure));
        }
        for (key, hooks) in &entry.webviews {
            self.webviews.insert(key.clone(), hooks.clone());
        }
        for (key, accepts_keys) in &entry.customs {
            self.customs.insert(key.clone());
            if *accepts_keys {
                self.keyed_customs.insert(key.clone());
            }
        }
        if !entry.handlers.is_empty() && self.handler_entries.insert(path.to_string()) {
            self.handler_gen += 1;
        }
        if !entry.contexts.is_empty() && self.context_entries.insert(path.to_string()) {
            self.context_gen += 1;
        }
        if !entry.effects.is_empty() && self.effect_entries.insert(path.to_string()) {
            self.effect_gen += 1;
        }
    }

    /// Takes one dropped entry's registrations out. A key belongs to the
    /// one boundary that renders its position, so nothing else can hold
    /// the same key while this entry does.
    fn unindex(&mut self, path: &str, entry: &Entry) {
        for (key, _) in &entry.actions {
            self.remove_action(key);
        }
        for (key, _) in &entry.copies {
            self.copies.remove(key);
        }
        for (key, _) in &entry.editors {
            self.editors.remove(key);
        }
        for (key, _) in &entry.splits {
            self.splits.remove(key);
        }
        for (key, _) in &entry.scrolls {
            self.scrolls.remove(key);
        }
        for (key, _) in &entry.measures {
            self.measures.remove(key);
        }
        for (key, _) in &entry.webviews {
            self.webviews.remove(key);
        }
        for (key, _) in &entry.customs {
            self.customs.remove(key);
            self.keyed_customs.remove(key);
        }
        if self.handler_entries.remove(path) {
            self.handler_gen += 1;
        }
        if self.context_entries.remove(path) {
            self.context_gen += 1;
        }
        if self.effect_entries.remove(path) {
            self.effect_gen += 1;
        }
        self.top_level.remove(path);
    }

    /// Takes the last root region's registrations back out.
    fn drop_root_region(&mut self) {
        let keys = std::mem::take(&mut self.root_keys);
        for key in &keys.actions {
            self.remove_action(key);
        }
        for key in &keys.copies {
            self.copies.remove(key);
        }
        for key in &keys.editors {
            self.editors.remove(key);
        }
        for key in &keys.splits {
            self.splits.remove(key);
        }
        for key in &keys.scrolls {
            self.scrolls.remove(key);
        }
        for key in &keys.measures {
            self.measures.remove(key);
        }
        for key in &keys.webviews {
            self.webviews.remove(key);
        }
        for key in &keys.customs {
            self.customs.remove(key);
            self.keyed_customs.remove(key);
        }
    }
}

/// Is a `/`-separated path's proper prefix `at` a cut between two
/// segments? Every `/` is — a row key may hold a `/` of its own, which
/// makes a cut that names no boundary, and a lookup that finds nothing
/// there is harmless.
fn cuts(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(at, _)| &path[..at])
}

/// Does the path have a retained boundary, or one being built, above
/// it? A top-level entry has neither: only the root region mounts it,
/// so only the root region can unmount it.
fn is_top_level(path: &str, building: &[BuildingFrame]) -> bool {
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        !cuts(path).any(|prefix| {
            retained.contains_key(prefix) || building.iter().any(|frame| frame.path == prefix)
        })
    })
}

/// A boundary's place in the retention, as a node of the layout tree
/// holds it ([`LayoutNode::BoundaryRef`]).
///
/// The retention is keyed by path, and a path is long: every boundary of
/// a frame — every row of every list — was looked up twice in a tree of
/// strings, and the compare of those strings stood second in the profile
/// of a placement. A reference holds its boundary's slot instead. The slot
/// is as old as the PATH, not as the entry: a re-run fills it again
/// ([`finish_entry`]), and an entry that leaves the retention empties it.
///
/// Public in name only — a layout node mentions it, and a layout node is
/// public. An app has no door to one and nothing to do with one.
pub struct Slot {
    held: RefCell<Option<Rc<Held>>>,
}

/// What a slot holds while its boundary is retained.
struct Held {
    layout: LayoutNode,
    /// The measures of that tree that were kept: the question, and the
    /// size and fit it answered. A few, because one tree is asked a few
    /// questions in a frame (a stack measures a flexible child twice).
    /// A re-run replaces the whole `Held`, and the list with it; a body
    /// that re-runs BELOW clears it ([`finish_entry`]).
    measures_kept: RefCell<Vec<KeptMeasure>>,
    /// Is the tree only paint? Asked when the boundary sits far off the
    /// glass, answered once for this tree ([`crate::layout::Quiet`]).
    quiet: std::cell::OnceCell<crate::layout::Quiet>,
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.held.borrow().is_some() { "Slot(held)" } else { "Slot(empty)" })
    }
}

impl Slot {
    fn empty() -> Rc<Slot> {
        Rc::new(Slot { held: RefCell::new(None) })
    }

    fn held(&self) -> Option<Rc<Held>> {
        self.held.borrow().clone()
    }

    /// Is the boundary's tree quiet NOW ([`crate::layout::Quiet`])? A
    /// boundary that left the retention places nothing, which is quiet.
    pub(crate) fn quiet_now(&self) -> bool {
        self.held().is_none_or(|held| {
            held.quiet
                .get_or_init(|| crate::layout::Quiet::of_all(std::slice::from_ref(&held.layout)))
                .holds()
        })
    }

    /// The boundary's layout tree, borrowed in place — measure and place
    /// resolve a `BoundaryRef` through here, WITHOUT stitching an expanded
    /// copy. `None` = the boundary left the retention.
    pub(crate) fn with_layout<R>(&self, reader: impl FnOnce(Option<&LayoutNode>) -> R) -> R {
        let held = self.held();
        reader(held.as_deref().map(|held| &held.layout))
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        // the entry left the retention: what refers to it finds nothing.
        // A re-run takes the old entry out BEFORE it fills the slot again
        // ([`finish_entry`]), so this never empties a fresh one.
        self.slot.held.replace(None);
    }
}

/// The slot of the boundary at `path` — what a new `BoundaryRef` holds.
/// A path nothing retains answers an empty slot: the reference measures
/// zero and places nothing, as it always did.
pub(crate) fn slot_of(path: &str) -> Rc<Slot> {
    RETAINED.with(|retained| {
        retained.borrow().get(path).map_or_else(Slot::empty, |entry| Rc::clone(&entry.slot))
    })
}

/// One kept answer of a retained tree's measure.
pub(crate) struct KeptMeasure {
    key: crate::layout::MeasureKey,
    size: crate::layout::Size,
    fit: Rc<crate::layout::Fit>,
}

/// How many questions one tree keeps the answer to.
const KEPT_MEASURES: usize = 4;

/// The measure of a retained boundary, kept from frame to frame.
///
/// `measure` runs on a miss, and its answer is kept — unless something
/// inside said it is not a function of the key (the poison count moved).
pub(crate) fn measure_retained(
    slot: &Slot,
    path: &str,
    key: crate::layout::MeasureKey,
    measure: impl FnOnce(&LayoutNode) -> (crate::layout::Size, crate::layout::Fit),
) -> (crate::layout::Size, crate::layout::Fit) {
    use crate::layout::Fit;

    let Some(held) = slot.held() else {
        debug_assert!(false, "layout reference without retention: {path}");
        return (crate::layout::Size::default(), Fit::Leaf);
    };
    let kept = held
        .measures_kept
        .borrow()
        .iter()
        .find(|kept| kept.key == key)
        .map(|kept| (kept.size, Rc::clone(&kept.fit)));
    if let Some((size, fit)) = kept {
        crate::stats::note_measure_kept(true);
        if crate::paranoid::on(crate::paranoid::MEMO) {
            let (fresh_size, fresh_fit) = measure(&held.layout);
            assert!(
                fresh_size == size && fresh_fit.same_as(&fit),
                "a kept measure of `{path}` is stale: kept {size:?}, fresh {fresh_size:?}"
            );
        }
        return (size, Fit::Shared(fit));
    }
    crate::stats::note_measure_kept(false);
    let poison = crate::layout::measure_poison();
    let (size, fit) = measure(&held.layout);
    if crate::layout::measure_poison() != poison {
        // something below answers by its own rules: ask again next frame
        return (size, fit);
    }
    let fit = Rc::new(fit);
    let mut kept = held.measures_kept.borrow_mut();
    if kept.len() == KEPT_MEASURES {
        kept.remove(0);
    }
    kept.push(KeptMeasure { key, size, fit: Rc::clone(&fit) });
    (size, Fit::Shared(fit))
}

/// A body re-ran at `path`: the boundaries above it may size themselves
/// by it, so what they kept is stale. Ancestors are prefixes of the path
/// at a `/`. An id can hold a `/` of its own, so a prefix may name no
/// entry — and then there is nothing to clear.
fn clear_measures_above(path: &str) {
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        for (at, _) in path.match_indices('/') {
            if let Some(held) = retained.get(&path[..at]).and_then(|entry| entry.slot.held()) {
                held.measures_kept.borrow_mut().clear();
            }
        }
    });
}

/// Is the boundary retained? (The guard for the `Runtime` stable frame.)
pub(crate) fn is_retained(path: &str) -> bool {
    RETAINED.with(|retained| retained.borrow().contains_key(path))
}

/// Records that the current frame was served WITHOUT a pass (stable
/// root synthesized) — the observable `body_runs` contract holds: this
/// frame ran zero bodies.
pub(crate) fn note_stable_frame() {
    LAST_BODY_RUNS.with(|last| last.borrow_mut().clear());
}

const REF_MARK: char = '\u{1}';

/// The reserved suffix a view's `.on_hover` is registered under, beside
/// the click's own key at the same path. A hit test never sees it: the
/// hit list carries the bare path, and only the runtime spells this
/// one out.
pub const HOVER_KEY: &str = "#hover";

pub(crate) fn begin_pass(dirty: HashSet<String>) {
    PASS.with(|pass| {
        *pass.borrow_mut() = PassState {
            active: true,
            dirty,
            ..PassState::default()
        };
    });
}

pub(crate) enum Decision {
    Skip,
    Render,
}

/// A boundary reached in the walk: skip if it is clean, retained, and
/// no body above it ran in this pass (a parent that ran built new
/// values — the config may have changed without going through `State`).
///
/// One parent is the exception: a KEYED LIST that re-ran. Its rows are
/// kept by key — a row renders once per key, and what the row shows
/// moves through the row's own reads — so under it a retained clean
/// row is skipped like any clean boundary.
pub(crate) fn decide(path: &str) -> Decision {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if !pass.active {
            return Decision::Render;
        }
        let inside_rerun = !pass.building.is_empty();
        let under_list = pass.building.last().is_some_and(|frame| frame.list);
        let retained = RETAINED.with(|retained| retained.borrow().contains_key(path));
        if (!inside_rerun || under_list) && retained && !pass.dirty.contains(path) {
            pass.skipped.push(path.to_string());
            Decision::Skip
        } else {
            Decision::Render
        }
    })
}

pub(crate) fn begin_entry(path: &str, list: bool) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        pass.body_runs.push(path.to_string());
        pass.building.push(BuildingFrame { path: path.to_string(), list, ..Default::default() });
    });
}

pub(crate) fn finish_entry(
    path: &str,
    value: Erased,
    ctx: Context,
    node: RenderNode,
    layout: LayoutNode,
) {
    let (effects, actions, copies, editors, splits, scrolls, measures, webviews, customs, handlers, contexts) =
        PASS.with(|pass| {
            let mut pass = pass.borrow_mut();
            match pass.building.pop() {
                Some(frame) => {
                    debug_assert_eq!(frame.path, path, "entries close in the order they open");
                    (
                        frame.effects,
                        frame.actions,
                        frame.copies,
                        frame.editors,
                        frame.splits,
                        frame.scrolls,
                        frame.measures,
                        frame.webviews,
                        frame.customs,
                        frame.handlers,
                        frame.contexts,
                    )
                }
                None => (
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                ),
            }
        });
    let parent_segments = motor::identity::parent_seed();
    // a top-level entry is known by what stands above it — and the
    // boundaries still building have no entry yet, so they are asked too
    let top_level = PASS.with(|pass| is_top_level(path, &pass.borrow().building));
    RETAINED.with(|retained| {
        let mut retained = retained.borrow_mut();
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            // the slot is as old as the path: the entry of the last run
            // goes FIRST (its registrations leave the tables, its drop
            // empties the slot), then the slot is filled again, so a
            // parent that did not re-run refers to the tree of today
            let slot = match retained.remove(path) {
                Some(old) => {
                    live.unindex(path, &old);
                    Rc::clone(&old.slot)
                }
                None => Slot::empty(),
            };
            slot.held.replace(Some(Rc::new(Held {
                layout,
                measures_kept: RefCell::new(Vec::new()),
                quiet: std::cell::OnceCell::new(),
            })));
            let entry = Entry {
                value,
                ctx,
                node,
                slot,
                effects,
                actions,
                copies,
                editors,
                splits,
                scrolls,
                measures,
                webviews,
                customs,
                handlers,
                contexts,
                parent_segments,
            };
            // a body ran and its registrations are new closures: they
            // replace the old ones in the tables the doors read, now
            live.index(path, &entry);
            crate::stats::note_entry_indexed();
            if top_level {
                live.top_level.insert(path.to_string());
            }
            retained.insert(path.to_string(), entry);
        });
    });
    // …and so is every measure kept ABOVE it. The outermost re-run of a
    // pass does this once: the boundaries between it and the ones it
    // re-ran below are new entries themselves.
    let outermost = PASS.with(|pass| pass.borrow().building.is_empty());
    if outermost {
        clear_measures_above(path);
    }
}

/// An effect registered during render: goes to the entry being built,
/// or to the root region when no boundary is open.
pub(crate) fn attribute_effect(effect: EffectFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.effects.push(effect);
        } else {
            pass.root_effects.push(effect);
        }
    });
}

/// An interactive action registered during render — same attribution.
pub(crate) fn attribute_action(path: String, action: ClickAction) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.actions.push((path, action));
        } else {
            pass.root_actions.push((path, action));
        }
    });
}

/// A `.on_copy` answer registered during render — same attribution.
pub(crate) fn attribute_copy(path: String, copy: CopyFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.copies.push((path, copy));
        } else {
            pass.root_copies.push((path, copy));
        }
    });
}

/// A field editor registered during render — same attribution.
pub(crate) fn attribute_editor(path: String, editor: EditorFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.editors.push((path, editor));
        } else {
            pass.root_editors.push((path, editor));
        }
    });
}

/// A split-position writer registered during render — same attribution
/// as the editors: entry being built, or the root region.
pub(crate) fn attribute_split(path: String, split: SplitFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.splits.push((path, split));
        } else {
            pass.root_splits.push((path, split));
        }
    });
}

/// A scroll-offset writer registered during render — the split's twin,
/// and the same attribution.
pub(crate) fn attribute_scroll(path: String, scroll: ScrollFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.scrolls.push((path, scroll));
        } else {
            pass.root_scrolls.push((path, scroll));
        }
    });
}

/// A measurement probe registered during render — the scroll writer's
/// twin, and the same attribution.
pub(crate) fn attribute_measure(path: String, measure: MeasureFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.measures.push((path, measure));
        } else {
            pass.root_measures.push((path, measure));
        }
    });
}

/// A webview's hooks registered during render — the scroll writer's
/// attribution, for the page's own reports and the handle's commands.
pub(crate) fn attribute_webview(path: String, hooks: WebviewHooks) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.webviews.push((path, hooks));
        } else {
            pass.root_webviews.push((path, hooks));
        }
    });
}

/// The app's own box, registered during render — same attribution. The
/// path alone is the record: it says the box is on screen this pass,
/// which is how a focused escape hatch keeps the keyboard.
pub(crate) fn attribute_custom(path: String, accepts_keys: bool) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.customs.push((path, accepts_keys));
        } else {
            pass.root_customs.push((path, accepts_keys));
        }
    });
}

/// A key context declared during render at `path` — active while its
/// view stays mounted, or, `focused`, only while the keyboard is inside
/// that view (retained like the handlers; the sweep deactivates it).
pub(crate) fn attribute_context(path: String, name: &'static str, focused: bool) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.contexts.push((path, name, focused));
        } else {
            pass.root_contexts.push((path, name, focused));
        }
    });
}

thread_local! {
    /// The contexts a mounted view declares — active for as long as it
    /// is mounted, whoever holds the keyboard.
    static ACTIVE_CONTEXTS: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::default());
    /// Every live declaration, in the order a stack reads them —
    /// outermost first — with the view it hangs on and whether it
    /// waits for the keyboard to be inside that view.
    static DECLARED_CONTEXTS: RefCell<Vec<ContextEntry>> = const { RefCell::new(Vec::new()) };
}

/// The retained entries under `root` that carry the kind of registration
/// `carriers` names, in path order — what a derived table rebuilds from.
fn carriers_under(root: &str, carriers: impl FnOnce(&Live) -> Vec<String>) -> Vec<String> {
    let mut paths = LIVE.with(|live| carriers(&live.borrow()));
    paths.retain(|path| covers(root, path));
    paths.sort();
    paths
}

/// Rebuilds the active contexts from the entries that declare one —
/// the twin of the handler assembly.
pub(crate) fn assemble_contexts(root: &str) {
    let mut declared: Vec<ContextEntry> = Vec::new();
    let carriers = carriers_under(root, |live| live.context_entries.iter().cloned().collect());
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        for path in &carriers {
            if let Some(entry) = retained.get(path) {
                declared.extend(entry.contexts.iter().cloned());
            }
        }
    });
    PASS.with(|pass| {
        declared.extend(std::mem::take(&mut pass.borrow_mut().root_contexts));
    });
    // outermost first: a view nearer the root holds the ones below it,
    // and among equals the tree's own order stands
    declared.sort_by(|(a, ..), (b, ..)| {
        let depth = |path: &str| path.split('/').count();
        depth(a).cmp(&depth(b)).then_with(|| a.cmp(b))
    });
    let mounted: HashSet<&'static str> =
        declared.iter().filter(|(_, _, focused)| !focused).map(|(_, name, _)| *name).collect();
    ACTIVE_CONTEXTS.with(|contexts| *contexts.borrow_mut() = mounted);
    DECLARED_CONTEXTS.with(|contexts| *contexts.borrow_mut() = declared);
}

/// Does the declaration count for a keyboard held at `focus`? A context
/// that waits for the keyboard counts only when the focused field or
/// box sits inside the view that declares it.
fn declaration_counts((path, _, focused): &ContextEntry, focus: Option<&str>) -> bool {
    !focused || focus.is_some_and(|focus| covers(path, focus))
}

/// Is the context active for a keyboard held at `focus` — declared by a
/// mounted view, or by a view the focused field or box sits inside?
pub(crate) fn context_active(name: &str, focus: Option<&str>) -> bool {
    ACTIVE_CONTEXTS.with(|contexts| contexts.borrow().contains(name))
        || DECLARED_CONTEXTS.with(|contexts| {
            contexts
                .borrow()
                .iter()
                .any(|entry| entry.1 == name && entry.2 && declaration_counts(entry, focus))
        })
}

/// Is `path` inside a view that leaves the keyboard where it is
/// (`.leaves_keyboard()`)? A click there moves no focus.
pub(crate) fn leaves_keyboard(path: &str) -> bool {
    DECLARED_CONTEXTS.with(|contexts| {
        contexts.borrow().iter().any(|(declarer, name, _)| *name == KEYBOARD_NEUTRAL && covers(declarer, path))
    })
}

/// Does the view at exactly `path` declare `name` this pass? An open
/// alert is known by this: its sub-root declares the reserved context
/// for as long as it is mounted, a skipped pass included (the
/// declaration is retained like a handler).
pub(crate) fn declares(path: &str, name: &str) -> bool {
    DECLARED_CONTEXTS.with(|contexts| {
        contexts.borrow().iter().any(|(declarer, declared, _)| *declared == name && declarer == path)
    })
}

/// The contexts active for a keyboard held at `focus`, outermost first,
/// each named once — the stack a key-context debugger shows.
pub(crate) fn active_contexts(focus: Option<&str>) -> Vec<&'static str> {
    DECLARED_CONTEXTS.with(|contexts| {
        let mut stack: Vec<&'static str> = Vec::new();
        for entry in contexts.borrow().iter() {
            // a mark, not a context a key could be read in
            if entry.1 == KEYBOARD_NEUTRAL {
                continue;
            }
            if declaration_counts(entry, focus) && !stack.contains(&entry.1) {
                stack.push(entry.1);
            }
        }
        stack
    })
}

pub(crate) fn attribute_handler(path: String, id: crate::action::ActionId, handler: HandlerFn) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.handlers.push((path, id, handler));
        } else {
            pass.root_handlers.push((path, id, handler));
        }
    });
}

thread_local! {
    /// The live handler map: id → (registration depth, handler).
    /// Reassembled per pass, like actions and editors — the map is a
    /// stamp of the pass, never retained interaction state.
    static HANDLERS: RefCell<HashMap<crate::action::ActionId, (usize, HandlerFn)>> =
        RefCell::new(HashMap::default());
}

/// Reassembles the handler map from the entries under the root that
/// carry one + the root region. Precedence: the DEEPEST path wins
/// (innermost in the tree); a depth tie → the last one mounted
/// (deterministic from the path order, documented as NON-contractual —
/// the semantic tiebreak arrives with key contexts).
pub(crate) fn assemble_handlers(root: &str) {
    let mut map: HashMap<crate::action::ActionId, (usize, HandlerFn)> = HashMap::default();
    let place = |map: &mut HashMap<crate::action::ActionId, (usize, HandlerFn)>,
                     path: &str,
                     id: crate::action::ActionId,
                     handler: HandlerFn| {
        let depth = path.split('/').count();
        match map.get(&id) {
            Some((existing, _)) if *existing > depth => {}
            _ => {
                map.insert(id, (depth, handler));
            }
        }
    };
    let carriers = carriers_under(root, |live| live.handler_entries.iter().cloned().collect());
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        for path in &carriers {
            if let Some(entry) = retained.get(path) {
                for (key, id, handler) in &entry.handlers {
                    place(&mut map, key, *id, handler.clone());
                }
            }
        }
    });
    PASS.with(|pass| {
        for (key, id, handler) in std::mem::take(&mut pass.borrow_mut().root_handlers) {
            place(&mut map, &key, id, handler);
        }
    });
    HANDLERS.with(|handlers| *handlers.borrow_mut() = map);
}

/// Is a handler for the id mounted in the tree? The question a menu asks
/// before it draws an item enabled — the same table [`run_handler`] reads,
/// read without running anything.
pub(crate) fn has_handler(id: crate::action::ActionId) -> bool {
    HANDLERS.with(|handlers| handlers.borrow().contains_key(&id))
}

/// Runs the innermost handler for the id. `false` = nobody registered —
/// the key is NOT consumed (it continues to the field/input system).
pub(crate) fn run_handler(id: crate::action::ActionId) -> bool {
    let handler = HANDLERS
        .with(|handlers| handlers.borrow().get(&id).map(|(_, handler)| handler.clone()));
    match handler {
        Some(handler) => {
            // outside the borrow: the handler can write state freely
            handler();
            true
        }
        None => false,
    }
}

/// Dirty views the walk did not reach (skipped parent): re-runs each
/// one from the retained value, with the cursor seeded on the parent's
/// path — ancestors first, because a parent's re-run covers the
/// descendants.
pub(crate) fn run_isolated(root: &str) {
    let mut pending: Vec<String> = PASS.with(|pass| {
        let pass = pass.borrow();
        pass.dirty
            .iter()
            .filter(|path| {
                covers(root, path) && !pass.body_runs.iter().any(|ran| covers(ran, path))
            })
            .cloned()
            .collect()
    });
    pending.sort_by_key(|path| path.len());

    for path in pending {
        let already_ran = PASS.with(|pass| {
            pass.borrow().body_runs.iter().any(|ran| covers(ran, &path))
        });
        if already_ran {
            continue;
        }
        let Some((value, ctx, parents)) = RETAINED.with(|retained| {
            retained.borrow().get(&path).map(|entry| {
                (entry.value.clone(), entry.ctx.clone(), entry.parent_segments.clone())
            })
        }) else {
            continue; // dirty but never mounted (or already swept): nothing to re-run
        };
        let _frames = motor::identity::seed_from(&parents);
        let mut scratch = crate::view::NodeList::new();
        use crate::view::View;
        // the retained value re-renders through the blanket's normal
        // path: the boundary is in the dirty snapshot, so it runs and
        // re-retains
        value.render_into(&ctx, &mut scratch);
    }
}

fn covers(ancestor: &str, path: &str) -> bool {
    // byte compare, no allocation: this runs once per retained entry
    // per assembly walk — a format! here taxed every pass
    path.len() >= ancestor.len()
        && path.as_bytes().starts_with(ancestor.as_bytes())
        && (path.len() == ancestor.len() || path.as_bytes()[ancestor.len()] == b'/')
}

/// The pass's effect queue: the root region + every retained entry
/// under the current root that carries one (skipped or not — a retained
/// effect is a live subscription), in path order.
pub(crate) fn assemble_effects(root: &str) -> Vec<EffectFn> {
    let mut queue = PASS.with(|pass| std::mem::take(&mut pass.borrow_mut().root_effects));
    let carriers = carriers_under(root, |live| live.effect_entries.iter().cloned().collect());
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        for path in &carriers {
            if let Some(entry) = retained.get(path) {
                queue.extend(entry.effects.iter().cloned());
            }
        }
    });
    queue
}

/// Is the path inside the scene the tables answer for? A door that
/// enumerates a table asks this, so a thread with two windows never
/// answers one window's question with the other's registrations. A
/// door that looks one path up never needs to: paths are unique.
fn in_scene(path: &str) -> bool {
    ASSEMBLED_ROOT.with(|root| root.borrow().as_deref().is_none_or(|root| covers(root, path)))
}

/// Does any view on screen want to hear the pointer arrive? One read,
/// so the hover road costs nothing at all in a scene that never asked.
pub(crate) fn hover_watched() -> bool {
    LIVE.with(|live| live.borrow().hover_keys > 0)
}

/// Fires the target's action (the key comes from the hit-test).
/// `false` = target not registered (the identity died between frame and
/// click — harmless).
pub(crate) fn run_action(path: &str, clicks: u8) -> bool {
    let action = LIVE.with(|live| live.borrow().actions.get(path).cloned());
    match action {
        Some(action) => {
            action(clicks);
            true
        }
        None => false,
    }
}

/// The view that answers a copy for a press at `path`: the path itself
/// or its nearest ancestor that registered `.on_copy`. A click on a row
/// hands the keyboard to the table the row belongs to.
pub(crate) fn copy_owner(path: &str) -> Option<String> {
    LIVE.with(|live| {
        live.borrow()
            .copies
            .keys()
            .filter(|owner| covers(owner, path))
            .max_by_key(|owner| owner.len())
            .cloned()
    })
}

/// Does the view at `path` answer a copy? A focus that lands on one
/// holds the keyboard with no caret.
pub(crate) fn answers_copy(path: &str) -> bool {
    LIVE.with(|live| live.borrow().copies.contains_key(path))
}

/// What the `.on_copy` at `path` answers now — `None` when nothing is
/// registered there, `Some(None)` when it is but nothing is selected.
pub(crate) fn run_copy(path: &str) -> Option<Option<String>> {
    // outside the borrow: the answer reads the app's state, which may
    // read the runtime back
    let copy = LIVE.with(|live| live.borrow().copies.get(path).cloned())?;
    Some(copy())
}

/// Hands one page report to its retained writer — cloned out of the
/// borrow before it runs, like every other door. `false` = nothing
/// listening at the path.
fn run_webview_report(
    path: &str,
    pick: impl Fn(&WebviewHooks) -> Option<WebviewReport>,
    line: &str,
) -> bool {
    let report = LIVE.with(|live| live.borrow().webviews.get(path).and_then(|hooks| pick(hooks)));
    match report {
        Some(report) => {
            report(line);
            true
        }
        None => false,
    }
}

/// An activated link, to the document's `on_link`.
pub(crate) fn run_webview_linked(path: &str, url: &str) -> bool {
    run_webview_report(path, |hooks| hooks.linked.clone(), url)
}

/// A changed body, to the document's `on_html_change`.
pub(crate) fn run_webview_changed(path: &str, html: &str) -> bool {
    run_webview_report(path, |hooks| hooks.changed.clone(), html)
}

/// A paste the app owns, to the document's `on_paste`.
pub(crate) fn run_webview_pasted(path: &str, html: &str, text: &str) -> bool {
    let report =
        LIVE.with(|live| live.borrow().webviews.get(path).and_then(|hooks| hooks.pasted.clone()));
    match report {
        Some(report) => {
            report(html, text);
            true
        }
        None => false,
    }
}

/// A committed navigation, to the page's `on_navigate`.
pub(crate) fn run_webview_navigated(path: &str, url: &str) -> bool {
    run_webview_report(path, |hooks| hooks.navigated.clone(), url)
}

/// A refused load, to the page's `on_navigate_failed` — the report
/// door with two words instead of one.
pub(crate) fn run_webview_failed(path: &str, url: &str, why: &str) -> bool {
    let report =
        LIVE.with(|live| live.borrow().webviews.get(path).and_then(|hooks| hooks.failed.clone()));
    match report {
        Some(report) => {
            report(url, why);
            true
        }
        None => false,
    }
}

/// What the page posted, to the page's `on_message`.
pub(crate) fn run_webview_posted(path: &str, body: &str) -> bool {
    run_webview_report(path, |hooks| hooks.posted.clone(), body)
}

/// A console line, to the page's `on_console`.
pub(crate) fn run_webview_console(path: &str, line: &str) -> bool {
    run_webview_report(path, |hooks| hooks.console.clone(), line)
}

/// A completed request, to the page's `on_request`.
pub(crate) fn run_webview_requested(path: &str, line: &str) -> bool {
    run_webview_report(path, |hooks| hooks.requested.clone(), line)
}

/// Does any handle in this scene hold a command the shell did not
/// spend yet? A peek: nothing is drained. A handle queues its commands
/// with no state write, so this is how a shell learns that a frame is
/// due for them.
pub(crate) fn has_webview_commands() -> bool {
    LIVE.with(|live| {
        live.borrow().webviews.iter().any(|(path, hooks)| {
            in_scene(path)
                && hooks.commands.as_ref().is_some_and(|queue| !queue.borrow().is_empty())
        })
    })
}

/// Drains every handle's queued commands in this scene, paired with
/// the path the handle is bound to — the runtime stamps eval tokens
/// and the shell spends the rest.
pub(crate) fn drain_webview_commands() -> Vec<(String, Vec<crate::host::WebviewCommand>)> {
    LIVE.with(|live| {
        live.borrow()
            .webviews
            .iter()
            .filter_map(|(path, hooks)| {
                if !in_scene(path) {
                    return None;
                }
                let queue = hooks.commands.as_ref()?;
                let commands = std::mem::take(&mut *queue.borrow_mut());
                if commands.is_empty() {
                    return None;
                }
                Some((path.clone(), commands))
            })
            .collect()
    })
}

/// Is the app's box at this path still on screen? (The focus of an
/// escape hatch lives or dies by this answer.)
pub(crate) fn has_custom(path: &str) -> bool {
    LIVE.with(|live| live.borrow().customs.contains(path))
}

/// Hands a dragged divider position to the split's retained writer.
/// `false` = no split registered at the path.
pub(crate) fn run_split(path: &str, at: crate::layout::Px) -> bool {
    let split = LIVE.with(|live| live.borrow().splits.get(path).cloned());
    match split {
        Some(split) => {
            split(at);
            true
        }
        None => false,
    }
}

/// Hands the offset a region LANDED on to its retained writer, so the
/// binding the app holds it in tells the truth. `false` = no writer at
/// the path — the region was never given a binding.
pub(crate) fn run_scroll(path: &str, offset: crate::layout::Point) -> bool {
    let scroll = LIVE.with(|live| live.borrow().scrolls.get(path).cloned());
    match scroll {
        Some(scroll) => {
            scroll(offset);
            true
        }
        None => false,
    }
}

/// Did any view ask to be measured? O(1), and it is what keeps a scene
/// with no probe from paying for the feature: the frame record holds
/// thousands of entries and walking it per layout to discover there is
/// nothing to report would be a real cost.
pub(crate) fn has_measures() -> bool {
    LIVE.with(|live| !live.borrow().measures.is_empty())
}

/// Hands a view's resolved size to the probe that asked for it.
/// `false` = no probe at the path.
pub(crate) fn run_measure(path: &str, size: crate::layout::Size) -> bool {
    let measure = LIVE.with(|live| live.borrow().measures.get(path).cloned());
    match measure {
        Some(measure) => {
            measure(size);
            true
        }
        None => false,
    }
}

/// The ONE live input of this scene whose named chain is `chain` — a
/// field's editor or a box that takes the keyboard. `None` when nothing
/// answers, and `None` when TWO do: an ambiguous name must never hand
/// the keyboard over on a guess.
///
/// `editors_only` narrows it to the fields, which is what a caret is
/// allowed to follow — a caret belongs to an editor, and a box owns
/// its own.
pub(crate) fn input_by_chain(chain: &str, editors_only: bool) -> Option<String> {
    if chain.is_empty() {
        return None;
    }
    let mut found: Option<String> = None;
    let mut walk = |path: &String| {
        if !in_scene(path) || motor::identity::named_chain(path) != chain {
            return false;
        }
        match &found {
            // two live inputs wear the same name: neither wins
            Some(seen) if seen != path => return true,
            Some(_) => {}
            None => found = Some(path.clone()),
        }
        false
    };
    let ambiguous = LIVE.with(|live| {
        let live = live.borrow();
        live.editors.keys().any(&mut walk)
            || (!editors_only && live.keyed_customs.iter().any(&mut walk))
    });
    if ambiguous { None } else { found }
}

pub(crate) fn has_editor(path: &str) -> bool {
    LIVE.with(|live| live.borrow().editors.contains_key(path))
}

/// The field's editor at `path`, cloned out of the borrow — the record
/// every field door reads.
fn editor_at(path: &str) -> Option<EditorFn> {
    LIVE.with(|live| live.borrow().editors.get(path).cloned())
}

/// Applies a command to the field — the retained closure is what
/// reaches the binding. Outer `None` = field not registered; the inner
/// `Option` is the command's output.
pub(crate) fn run_editor(
    path: &str,
    command: EditCommand,
    state: &mut CaretState,
) -> Option<Option<String>> {
    editor_at(path).map(|editor| (editor.command)(command, state))
}

thread_local! {
    /// Whose scene the DERIVED tables (the handlers, the key contexts)
    /// answer for.
    ///
    /// The retention is keyed by PATH and holds every scene at once; so
    /// do the path-keyed tables, and a path is unique, so a lookup never
    /// needs to know the scene. The two derived tables are flattened
    /// views of ONE root — on a thread with two windows, the window that
    /// rendered last would otherwise answer for the window the hand is
    /// actually in — so a runtime checks this before it reads them, and
    /// rebuilds its own if another scene left theirs standing.
    static ASSEMBLED_ROOT: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The generations the derived tables were built at — handlers,
    /// contexts — and whether a root region fed them. A root region is
    /// rebuilt by every pass (its closures are new each time), so tables
    /// that hold one never stay.
    static ASSEMBLED_AT: Cell<Option<(u64, u64, bool)>> = const { Cell::new(None) };
    /// The effect queue of the last FULL assembly, with the root and the
    /// effect generation it was built for. It has its own key: the
    /// derived tables are also rebuilt outside a pass, when a scene
    /// becomes current again, and that rebuild makes no queue.
    static ASSEMBLED_EFFECTS: RefCell<Option<(String, u64, Rc<[EffectFn]>)>> =
        const { RefCell::new(None) };
}

fn fingerprint_key(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = motor::hash::FxHasher::default();
    key.hash(&mut hasher);
    hasher.finish()
}

fn fingerprint_ptr<T: ?Sized>(shared: &Rc<T>) -> u64 {
    Rc::as_ptr(shared) as *const () as usize as u64
}

/// The fingerprint of the path-keyed tables: every key, and the identity
/// of every closure behind it. Order never matters inside a table: a sum
/// is the same in any order. `without_root` leaves the root region's
/// keys out — the retention never holds those, so a fingerprint built
/// from it must not see them either.
fn tables_fingerprint(live: &Live, without_root: bool) -> u64 {
    let skip = |keys: &[String], key: &str| without_root && keys.iter().any(|root| root == key);
    let mut total = 0u64;
    let mut mix = |part: u64| total = total.wrapping_mul(31).wrapping_add(part);
    mix(live.actions.iter().fold(0u64, |sum, (key, action)| {
        if skip(&live.root_keys.actions, key) { sum } else { sum.wrapping_add(fingerprint_key(key) ^ fingerprint_ptr(action)) }
    }));
    mix(live.copies.iter().fold(0u64, |sum, (key, copy)| {
        if skip(&live.root_keys.copies, key) { sum } else { sum.wrapping_add(fingerprint_key(key) ^ fingerprint_ptr(copy)) }
    }));
    mix(live.editors.iter().fold(0u64, |sum, (key, editor)| {
        if skip(&live.root_keys.editors, key) { sum } else { sum.wrapping_add(fingerprint_key(key) ^ fingerprint_ptr(&editor.command)) }
    }));
    mix(live.splits.iter().fold(0u64, |sum, (key, split)| {
        if skip(&live.root_keys.splits, key) { sum } else { sum.wrapping_add(fingerprint_key(key) ^ fingerprint_ptr(split)) }
    }));
    mix(live.scrolls.iter().fold(0u64, |sum, (key, scroll)| {
        if skip(&live.root_keys.scrolls, key) { sum } else { sum.wrapping_add(fingerprint_key(key) ^ fingerprint_ptr(scroll)) }
    }));
    mix(live.measures.iter().fold(0u64, |sum, (key, measure)| {
        if skip(&live.root_keys.measures, key) { sum } else { sum.wrapping_add(fingerprint_key(key) ^ fingerprint_ptr(measure)) }
    }));
    mix(live.webviews.keys().fold(0u64, |sum, key| {
        if skip(&live.root_keys.webviews, key) { sum } else { sum.wrapping_add(fingerprint_key(key)) }
    }));
    mix(live.customs.iter().fold(0u64, |sum, key| {
        if skip(&live.root_keys.customs, key) { sum } else { sum.wrapping_add(fingerprint_key(key)) }
    }));
    mix(live.keyed_customs.iter().fold(0u64, |sum, key| {
        if skip(&live.root_keys.customs, key) { sum } else { sum.wrapping_add(fingerprint_key(key)) }
    }));
    total
}

/// A number that moves when any table the doors read moves: every key,
/// and the identity of every closure behind it. It is the paranoid
/// check's question, and nothing else asks it.
pub(crate) fn input_fingerprint() -> u64 {
    let mut total = LIVE.with(|live| tables_fingerprint(&live.borrow(), false));
    let mut mix = |part: u64| total = total.wrapping_mul(31).wrapping_add(part);
    mix(HANDLERS.with(|map| {
        map.borrow().values().fold(0u64, |sum, (depth, handler)| {
            sum.wrapping_add((*depth as u64).wrapping_mul(0x9E37_79B9) ^ fingerprint_ptr(handler))
        })
    }));
    mix(ACTIVE_CONTEXTS.with(|set| set.borrow().iter().fold(0u64, |sum, key| sum.wrapping_add(fingerprint_key(key)))));
    mix(DECLARED_CONTEXTS.with(|list| {
        list.borrow().iter().fold(0u64, |sum, (path, name, focused)| {
            sum.wrapping_add(fingerprint_key(path) ^ fingerprint_key(name) ^ u64::from(*focused))
        })
    }));
    total
}

/// The paranoid oracle of the live tables: built again from the whole
/// retention, do they hold the same keys over the same closures? The
/// root region is left out of both sides.
pub(crate) fn live_tables_match_retention() -> bool {
    let mut scratch = Live::default();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            scratch.index(path, entry);
        }
    });
    LIVE.with(|live| tables_fingerprint(&live.borrow(), true) == tables_fingerprint(&scratch, false))
}

/// Is the root region of THIS pass empty? Registrations made outside
/// every boundary live there, and only for one pass.
fn root_region_is_empty() -> bool {
    PASS.with(|pass| {
        let pass = pass.borrow();
        pass.root_effects.is_empty()
            && pass.root_actions.is_empty()
            && pass.root_copies.is_empty()
            && pass.root_editors.is_empty()
            && pass.root_splits.is_empty()
            && pass.root_scrolls.is_empty()
            && pass.root_measures.is_empty()
            && pass.root_webviews.is_empty()
            && pass.root_customs.is_empty()
            && pass.root_handlers.is_empty()
            && pass.root_contexts.is_empty()
    })
}

fn derived_generations() -> (u64, u64) {
    LIVE.with(|live| {
        let live = live.borrow();
        (live.handler_gen, live.context_gen)
    })
}

/// Do the derived tables still answer for `root`? They do when they
/// were built for it, no entry carrying a handler or a context moved
/// since, and no root region fed them then or wants to feed them now.
pub(crate) fn assembly_is_current(root: &str) -> bool {
    let same_root = ASSEMBLED_ROOT.with(|slot| slot.borrow().as_deref() == Some(root));
    let (handlers, contexts) = derived_generations();
    same_root
        && ASSEMBLED_AT.with(Cell::get) == Some((handlers, contexts, false))
        && root_region_is_empty()
}

/// The effect queue this root's last full assembly built — `None` when
/// an entry carrying an effect moved since, when another scene
/// assembled after it, or when a root region wants to feed the queue
/// now. The caller then assembles a new one.
pub(crate) fn assembled_effects(root: &str) -> Option<Rc<[EffectFn]>> {
    if !root_region_is_empty() {
        return None;
    }
    let generation = LIVE.with(|live| live.borrow().effect_gen);
    ASSEMBLED_EFFECTS.with(|slot| match slot.borrow().as_ref() {
        Some((kept_root, kept_at, queue)) if kept_root == root && *kept_at == generation => {
            Some(Rc::clone(queue))
        }
        _ => None,
    })
}

/// Keeps the queue a full assembly built, for the passes that change
/// nothing. A queue a root region fed is never kept: the region's
/// closures are new on every pass.
pub(crate) fn keep_assembled_effects(root: &str, queue: &Rc<[EffectFn]>, had_root_region: bool) {
    let generation = LIVE.with(|live| live.borrow().effect_gen);
    ASSEMBLED_EFFECTS.with(|slot| {
        *slot.borrow_mut() =
            (!had_root_region).then(|| (root.to_string(), generation, Rc::clone(queue)));
    });
}

/// Was a root region waiting when this pass reached its assembly?
pub(crate) fn pass_has_root_region() -> bool {
    !root_region_is_empty()
}

/// Whose scene the derived tables answer for right now.
pub(crate) fn assembled_root() -> Option<String> {
    ASSEMBLED_ROOT.with(|root| root.borrow().clone())
}

/// Records that the derived tables now answer for `root` — the runtime
/// calls this as the last step of assembling them.
pub(crate) fn set_assembled_root(root: &str, had_root_region: bool) {
    ASSEMBLED_ROOT.with(|slot| *slot.borrow_mut() = Some(root.to_string()));
    let (handlers, contexts) = derived_generations();
    ASSEMBLED_AT.with(|at| at.set(Some((handlers, contexts, had_root_region))));
}

/// The root region's registrations — made outside every boundary —
/// enter the tables for ONE pass: the last region's leave first, this
/// pass's go in, and their keys are kept so the next assembly can take
/// them out again.
pub(crate) fn refresh_root_region() {
    let (actions, copies, editors, splits, scrolls, measures, webviews, customs) =
        PASS.with(|pass| {
            let mut pass = pass.borrow_mut();
            (
                std::mem::take(&mut pass.root_actions),
                std::mem::take(&mut pass.root_copies),
                std::mem::take(&mut pass.root_editors),
                std::mem::take(&mut pass.root_splits),
                std::mem::take(&mut pass.root_scrolls),
                std::mem::take(&mut pass.root_measures),
                std::mem::take(&mut pass.root_webviews),
                std::mem::take(&mut pass.root_customs),
            )
        });
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.drop_root_region();
        let mut keys = RootKeys::default();
        for (key, action) in actions {
            keys.actions.push(key.clone());
            live.insert_action(key, action);
        }
        for (key, copy) in copies {
            keys.copies.push(key.clone());
            live.copies.insert(key, copy);
        }
        for (key, editor) in editors {
            keys.editors.push(key.clone());
            live.editors.insert(key, editor);
        }
        for (key, split) in splits {
            keys.splits.push(key.clone());
            live.splits.insert(key, split);
        }
        for (key, scroll) in scrolls {
            keys.scrolls.push(key.clone());
            live.scrolls.insert(key, scroll);
        }
        for (key, measure) in measures {
            keys.measures.push(key.clone());
            live.measures.insert(key, measure);
        }
        for (key, hooks) in webviews {
            keys.webviews.push(key.clone());
            live.webviews.insert(key, hooks);
        }
        for (key, accepts_keys) in customs {
            keys.customs.push(key.clone());
            if accepts_keys {
                live.keyed_customs.insert(key.clone());
            }
            live.customs.insert(key);
        }
        live.root_keys = keys;
    });
}

/// Removes the entries at `paths` from the retention, and their
/// registrations from the tables the doors read.
fn drop_entries(paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    RETAINED.with(|retained| {
        let mut retained = retained.borrow_mut();
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            for path in paths {
                if let Some(entry) = retained.remove(path) {
                    live.unindex(path, &entry);
                    // a view that left owes the read graph nothing more:
                    // its reads and its bindings' reads fall with it
                    motor::identity::forget_view_reads(path);
                }
            }
        });
    });
}

/// Drops every retained entry under `root` — the retention half of a
/// SCENE reset (`motor::identity::reset_scene` is the other half). The
/// other scenes on this thread keep theirs.
pub(crate) fn forget_under(root: &str) {
    let prefix = format!("{root}/");
    let doomed: Vec<String> = RETAINED.with(|retained| {
        retained
            .borrow()
            .keys()
            .filter(|path| *path == root || path.starts_with(&prefix))
            .cloned()
            .collect()
    });
    drop_entries(&doomed);
    ASSEMBLED_ROOT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_deref() == Some(root) {
            *slot = None;
        }
    });
}

/// Identities swept by `end_pass`: their entries fall with them.
pub(crate) fn forget(dead: &[String]) {
    drop_entries(dead);
}

/// The bodies that ran, with the ones under another run dropped: a run
/// covers its subtree, so the outermost runs name every subtree the
/// sweep must read.
fn outermost(runs: &HashSet<String>) -> Vec<&String> {
    let mut sorted: Vec<&String> = runs.iter().collect();
    sorted.sort_by_key(|run| run.len());
    let mut outer: Vec<&String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::default();
    for run in sorted {
        if cuts(run).any(|prefix| seen.contains(prefix)) {
            continue;
        }
        seen.insert(run.as_str());
        outer.push(run);
    }
    outer
}

/// The TWIN of the identity sweep, for views with NO state of their
/// own: the motor's sweep only knows boundaries with slots/anchors
/// (owners); a stateless view that unmounts would leave its entry
/// retained — and with it ZOMBIE handlers/actions/editors answering
/// after the unmount. The rule: under the root, survivors are who
/// re-ran, who was skipped, or who lives under a SKIPPED boundary (the
/// walk stayed out of it on purpose). An unvisited descendant of a
/// parent that RE-RAN is dead — the parent revisited its living
/// children one by one.
///
/// Only two places can hold the dead: the subtrees of the bodies that
/// ran, and the top level, where the root region mounts and unmounts
/// on its own. The sweep reads those and nothing else — a pass that
/// ran one body in a list of a thousand reads that body's subtree.
pub(crate) fn sweep_stale(root: &str) {
    let (runs, skipped): (HashSet<String>, HashSet<String>) = PASS.with(|pass| {
        let pass = pass.borrow();
        (
            pass.body_runs.iter().cloned().collect(),
            pass.skipped.iter().cloned().collect(),
        )
    });
    // alive: it ran, or it stands at or under a boundary the walk
    // skipped on purpose
    let alive = |path: &str| {
        runs.contains(path) || skipped.contains(path) || cuts(path).any(|prefix| skipped.contains(prefix))
    };
    let mut dead: Vec<String> = Vec::new();
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        // the entries under one boundary, in one ordered range — the
        // subtree is contiguous because `/` sorts before every byte a
        // segment may start with after it
        let sweep_under = |boundary: &str, dead: &mut Vec<String>| {
            let lo = format!("{boundary}/");
            let hi = format!("{boundary}0");
            let range = (std::ops::Bound::Included(lo.as_str()), std::ops::Bound::Excluded(hi.as_str()));
            for (path, _) in retained.range::<str, _>(range) {
                if !alive(path) {
                    dead.push(path.clone());
                }
            }
        };
        for run in outermost(&runs) {
            sweep_under(run, &mut dead);
        }
        // a top-level entry the root region did not mount again fell,
        // and everything under it with it
        let fallen: Vec<String> = LIVE.with(|live| {
            live.borrow()
                .top_level
                .iter()
                .filter(|path| covers(root, path) && !alive(path))
                .cloned()
                .collect()
        });
        for path in fallen {
            sweep_under(&path, &mut dead);
            dead.push(path);
        }
    });
    drop_entries(&dead);
}

/// Drops the whole retention — the next pass runs every body (the
/// tests' `render_full`; the state in the identity arenas stays).
pub(crate) fn clear() {
    RETAINED.with(|retained| retained.borrow_mut().clear());
    LIVE.with(|live| *live.borrow_mut() = Live::default());
    ASSEMBLED_AT.with(|at| at.set(None));
}

/// The world-reset twin of [`clear`]: the retention AND every table
/// falls. A newborn runtime starts from nothing — see
/// `motor::identity::reset_world` for the other half of the contract.
pub(crate) fn reset_world() {
    RETAINED.with(|retained| retained.borrow_mut().clear());
    crate::layout::forget_pictures();
    LIVE.with(|live| *live.borrow_mut() = Live::default());
    ASSEMBLED_ROOT.with(|root| *root.borrow_mut() = None);
    ASSEMBLED_AT.with(|at| at.set(None));
    ASSEMBLED_EFFECTS.with(|slot| *slot.borrow_mut() = None);
    PASS.with(|pass| *pass.borrow_mut() = PassState::default());
    LAST_BODY_RUNS.with(|last| last.borrow_mut().clear());
    FRAME_BODY_RUNS.with(|frame| frame.borrow_mut().clear());
    ACTIVE_CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
    DECLARED_CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
    HANDLERS.with(|handlers| handlers.borrow_mut().clear());
}

pub(crate) fn end_pass() {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        pass.active = false;
        let runs = std::mem::take(&mut pass.body_runs);
        FRAME_BODY_RUNS.with(|frame| frame.borrow_mut().extend(runs.iter().cloned()));
        LAST_BODY_RUNS.with(|last| *last.borrow_mut() = runs);
    });
}

/// Every body that ran since the last drain — a FRAME may settle over
/// several passes, and the reuse decision needs all of them. The Dom
/// frame drains this once per event.
/// Diagnostics: how many boundaries the reconciler retains.
pub(crate) fn retained_len() -> usize {
    RETAINED.with(|retained| retained.borrow().len())
}

pub(crate) fn take_frame_runs() -> Vec<String> {
    FRAME_BODY_RUNS.with(|frame| std::mem::take(&mut *frame.borrow_mut()))
}

/// Instrumentation: the bodies that ran in the last pass (identity
/// paths) — the proof of incrementality in the tests.
pub(crate) fn last_body_runs() -> Vec<String> {
    LAST_BODY_RUNS.with(|last| last.borrow().clone())
}

// MARK: - References and expansion

pub(crate) fn ref_line(path: &str) -> String {
    format!("{REF_MARK}{path}{REF_MARK}")
}

fn parse_ref(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix(REF_MARK)?;
    let end = rest.find(REF_MARK)?;
    Some((&rest[..end], &rest[end + REF_MARK.len_utf8()..]))
}

/// Resolves references against the retention: expands the retained node
/// (recursive — the cache references too), re-applies the modifier
/// suffixes accumulated on the reference line, and re-appends extra
/// children (the `Sheet` node the modifier hangs on the boundary).
pub(crate) fn expand(node: &RenderNode) -> RenderNode {
    if let Some((path, suffix)) = parse_ref(&node.line) {
        let retained = RETAINED.with(|retained| {
            retained.borrow().get(path).map(|entry| entry.node.clone())
        });
        let Some(inner) = retained else {
            debug_assert!(false, "boundary reference without retention: {path}");
            return RenderNode::leaf("");
        };
        let mut expanded = expand(&inner);
        expanded.line.push_str(suffix);
        if let Some(read) = expanded.live.take() {
            // a live line keeps its suffixes: they ride the read
            let suffix = suffix.to_string();
            expanded.live = Some(Rc::new(move || format!("{}{suffix}", read())));
        }
        for child in &node.children {
            expanded.children.push(expand(child));
        }
        expanded
    } else {
        RenderNode {
            line: node.line.clone(),
            live: node.live.clone(),
            children: node.children.iter().map(expand).collect(),
        }
    }
}

/// Offer modal input through the same retained field identity as native edits.
pub(crate) fn field_key(
    path: &str,
    stroke: &crate::action::Stroke,
    state: &mut CaretState,
) -> bool {
    let editor = editor_at(path);
    editor.and_then(|editor| editor.key).is_some_and(|key| key(stroke, state))
}

/// The keys the field at `path` asks a software keyboard for — the
/// letters when nothing there named any.
pub(crate) fn field_keyboard(path: &str) -> crate::text_input::KeyboardType {
    editor_at(path).map(|editor| editor.keyboard).unwrap_or_default()
}

/// The field's own word for the keyboard (`TextField::on_focus`), if the
/// app gave it one.
pub(crate) fn field_focus_hook(path: &str) -> Option<Rc<dyn Fn(bool)>> {
    editor_at(path).and_then(|editor| editor.focus.clone())
}

pub(crate) fn field_policy(path: &str) -> Option<Rc<dyn crate::text_input::EditingStrategy>> {
    editor_at(path).and_then(|editor| editor.policy.clone())
}

pub(crate) fn field_caret_shape(path: &str) -> crate::text_input::CaretShape {
    field_policy(path).map_or(crate::text_input::CaretShape::Bar, |policy| {
        policy.caret_shape()
    })
}

pub(crate) fn field_takes_text(path: &str) -> bool {
    let editor = editor_at(path);
    editor.is_some_and(|editor| editor.policy.as_ref().is_none_or(|policy| policy.takes_text()))
}

/// Whether this retained field opts into chat-style Enter submission.
/// The door a pasted picture goes through, when the field opened one.
pub(crate) fn field_paste_image(path: &str) -> Option<Rc<dyn Fn(crate::clipboard::ClipboardImage)>> {
    editor_at(path).and_then(|editor| editor.paste_image.clone())
}

/// Does the field stand aside for the app's navigation right now? Read
/// at the stroke, from the binding the app holds.
pub(crate) fn field_intercepts_nav(path: &str) -> bool {
    let intercept = editor_at(path).and_then(|editor| editor.nav_intercept.clone());
    // out of the borrow: reading a binding may reach the app's state
    intercept.is_some_and(|binding| binding.wrappedValue())
}

pub(crate) fn field_yields_on_submit(path: &str) -> bool {
    editor_at(path).is_some_and(|editor| editor.yields_on_submit)
}

pub(crate) fn field_submits_on_enter(path: &str) -> bool {
    editor_at(path).is_some_and(|editor| editor.submit_on_enter)
}
