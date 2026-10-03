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

pub(crate) type ActionEntry = (Rc<str>, ClickAction);

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
    /// The registrations few bodies make ([`Rare`]) — `None` for a body
    /// that made none of them, which is nearly every row of a list.
    pub rare: Option<Box<Rare>>,
    /// Where the PARENT's path segments end in the entry's own path —
    /// the cursor seed for an isolated re-run, read against the key.
    pub parent_segments: motor::identity::PathSeed,
    /// Did the entry close with no retained boundary above it? Then it
    /// stands in the live tables' top level, and it is the one place the
    /// entry's fall has to take it out of — asked here, never searched.
    pub top_level: bool,
    /// The last pass that met the entry, and how ([`Visit`]): the sweep
    /// reads who is alive off the entry it walks past, instead of
    /// hashing its path into the sets of the pass.
    visit: Cell<u64>,
}

/// The registrations a body seldom makes, kept apart from its entry.
///
/// An entry sits in the retention's tree by value, and the tree moves
/// its values: a row that leaves shifts the rows after it in its node,
/// and the node that runs short borrows from or merges with the next. A
/// row carried nine empty lists through every one of those moves; boxed
/// apart, they are one word, and a row that makes none of them makes no
/// box.
#[derive(Default)]
pub(crate) struct Rare {
    /// The body's `.on_copy` answers — same retention as the actions.
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
}

impl Rare {
    /// The lists a body closed with, boxed — or nothing, when it made
    /// none of them.
    #[allow(clippy::too_many_arguments)]
    fn boxed(
        copies: Vec<CopyEntry>,
        editors: Vec<EditorEntry>,
        splits: Vec<SplitEntry>,
        scrolls: Vec<ScrollEntry>,
        measures: Vec<MeasureEntry>,
        webviews: Vec<WebviewEntry>,
        customs: Vec<(String, bool)>,
        handlers: Vec<HandlerEntry>,
        contexts: Vec<ContextEntry>,
    ) -> Option<Box<Rare>> {
        let none = copies.is_empty()
            && editors.is_empty()
            && splits.is_empty()
            && scrolls.is_empty()
            && measures.is_empty()
            && webviews.is_empty()
            && customs.is_empty()
            && handlers.is_empty()
            && contexts.is_empty();
        (!none).then(|| {
            Box::new(Rare { copies, editors, splits, scrolls, measures, webviews, customs, handlers, contexts })
        })
    }
}

impl Entry {
    /// The body's named-action handlers.
    fn handlers(&self) -> &[HandlerEntry] {
        self.rare.as_ref().map_or(&[], |rare| &rare.handlers)
    }

    /// The key contexts the body declared.
    fn contexts(&self) -> &[ContextEntry] {
        self.rare.as_ref().map_or(&[], |rare| &rare.contexts)
    }
}

/// How a pass met an entry, stamped on the entry: its body RAN, or the
/// walk SKIPPED it on purpose (clean and retained). A stamp from an
/// earlier pass reads as neither.
#[derive(Clone, Copy)]
struct Visit {
    ran: u64,
    skipped: u64,
}

impl Visit {
    /// The stamps of the pass under way. A pass is numbered from one, so
    /// an entry that was never met (stamped zero) is met by no pass.
    fn now() -> Visit {
        let pass = PASS_NO.with(Cell::get);
        Visit { ran: pass * 2, skipped: pass * 2 + 1 }
    }
}

struct BuildingFrame {
    path: Rc<str>,
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

impl BuildingFrame {
    /// The frame of a body that begins, every list empty. Spelled out
    /// and not defaulted: a default frame is a default PATH too, and an
    /// empty shared string is an allocation of its own — one for every
    /// body that ran, thrown away the moment the real path took its
    /// place.
    fn new(path: Rc<str>, list: bool) -> BuildingFrame {
        BuildingFrame {
            path,
            list,
            effects: Vec::new(),
            actions: Vec::new(),
            copies: Vec::new(),
            editors: Vec::new(),
            splits: Vec::new(),
            scrolls: Vec::new(),
            measures: Vec::new(),
            webviews: Vec::new(),
            customs: Vec::new(),
            handlers: Vec::new(),
            contexts: Vec::new(),
        }
    }
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
    body_runs: Vec<Rc<str>>,
    /// The runs that began with no body open around them — the subtrees
    /// the entry sweep reads. Known when they begin, never searched for.
    outermost: Vec<Rc<str>>,
}

thread_local! {
    /// Every retained boundary by path. The tree holds each entry by its
    /// box: a node shifts, lends and merges its values at every row that
    /// mounts or leaves, and a value a word wide moves for nothing.
    static RETAINED: RefCell<BTreeMap<Rc<str>, Box<Entry>>> = const { RefCell::new(BTreeMap::new()) };
    static PASS: RefCell<PassState> = RefCell::new(PassState::default());
    /// The passes, counted: the number the entries are stamped with
    /// ([`Visit`]). It never goes back, so no stamp is ever met twice.
    static PASS_NO: Cell<u64> = const { Cell::new(0) };
    static LAST_BODY_RUNS: RefCell<Vec<Rc<str>>> = const { RefCell::new(Vec::new()) };
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
///
/// One table lags on purpose: the click keys of an entry that LEFT stay
/// until the idle takes them out ([`collect_garbage`]), because a
/// thousand rows that leave are two thousand keys hashed out of it in the
/// click that let them go. Until then each one still names its owner, and
/// a key whose owner left fires nothing ([`Live::click`]).
#[derive(Default)]
struct Live {
    actions: HashMap<Rc<str>, Registered>,
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
    /// How many click keys of entries that left still stand in the
    /// table, for the idle to take out — none, and the idle has nothing
    /// to look for.
    buried_actions: usize,
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

/// A click key's registration: what it fires, and whose it is.
struct Registered {
    action: ClickAction,
    /// The slot of the boundary whose body registered it — as old as the
    /// boundary's stay in the retention: a body that re-runs keeps it and
    /// replaces its registrations, one that leaves marks it left, and the
    /// boundary that mounts at the same path afterwards gets a slot of its
    /// own. `None` for the root region, whose keys leave at the next
    /// assembly.
    owner: Option<Rc<Slot>>,
}

impl Registered {
    /// Does the registration still belong to a retained entry?
    fn is_live(&self) -> bool {
        self.owner.as_ref().is_none_or(|slot| !slot.left.get())
    }
}

impl Live {
    fn insert_action(&mut self, key: Rc<str>, action: ClickAction, owner: Option<Rc<Slot>>) {
        let hover = key.ends_with(HOVER_KEY);
        if self.actions.insert(key, Registered { action, owner }).is_none() && hover {
            self.hover_keys += 1;
        }
    }

    fn remove_action(&mut self, key: &str) {
        if self.actions.remove(key).is_some() && key.ends_with(HOVER_KEY) {
            self.hover_keys -= 1;
        }
    }

    /// What a click at `key` fires — nothing when the key's owner left
    /// and the idle has not taken the key out yet. A press can still
    /// name it: the event was queued before the frame that let the row
    /// go, and the element it hit is gone from the page but not from
    /// the event.
    fn click(&self, key: &str) -> Option<ClickAction> {
        let registered = self.actions.get(key)?;
        registered.is_live().then(|| Rc::clone(&registered.action))
    }

    /// Puts one closed entry's registrations into the tables.
    fn index(&mut self, path: &str, entry: &Entry) {
        for (key, action) in &entry.actions {
            self.insert_action(key.clone(), Rc::clone(action), Some(Rc::clone(&entry.slot)));
        }
        if let Some(rare) = &entry.rare {
            for (key, copy) in &rare.copies {
                self.copies.insert(key.clone(), Rc::clone(copy));
            }
            for (key, editor) in &rare.editors {
                self.editors.insert(key.clone(), editor.clone());
            }
            for (key, split) in &rare.splits {
                self.splits.insert(key.clone(), Rc::clone(split));
            }
            for (key, scroll) in &rare.scrolls {
                self.scrolls.insert(key.clone(), Rc::clone(scroll));
            }
            for (key, measure) in &rare.measures {
                self.measures.insert(key.clone(), Rc::clone(measure));
            }
            for (key, hooks) in &rare.webviews {
                self.webviews.insert(key.clone(), hooks.clone());
            }
            for (key, accepts_keys) in &rare.customs {
                self.customs.insert(key.clone());
                if *accepts_keys {
                    self.keyed_customs.insert(key.clone());
                }
            }
        }
        if !entry.handlers().is_empty() && self.handler_entries.insert(path.to_string()) {
            self.handler_gen += 1;
        }
        if !entry.contexts().is_empty() && self.context_entries.insert(path.to_string()) {
            self.context_gen += 1;
        }
        if !entry.effects.is_empty() && self.effect_entries.insert(path.to_string()) {
            self.effect_gen += 1;
        }
    }

    /// Takes one dropped entry's registrations out. A key belongs to the
    /// one boundary that renders its position, so nothing else can hold
    /// the same key while this entry does.
    ///
    /// A path stands in the carrier sets only while its entry carries
    /// one — [`Live::index`] files it on that condition alone — so an
    /// entry that carries nothing asks no set: a thousand rows that leave
    /// a list hash their path for the tables they are in, and none other.
    fn unindex(&mut self, path: &str, entry: &Entry) {
        for (key, _) in &entry.actions {
            self.remove_action(key);
        }
        self.unindex_rest(path, entry);
    }

    /// [`Live::unindex`] for an entry that LEFT the retention: its slot
    /// is marked left, and its click keys stay until the idle takes them
    /// out ([`Live::take_buried`]), firing nothing meanwhile. A hover key
    /// leaves now — the count of them says whether the hover road runs.
    fn unindex_leaving(&mut self, path: &str, entry: &Entry) {
        entry.slot.left.set(true);
        for (key, _) in &entry.actions {
            if key.ends_with(HOVER_KEY) {
                self.remove_action(key);
            } else {
                self.buried_actions += 1;
            }
        }
        self.unindex_rest(path, entry);
    }

    /// Takes out the click keys the entries that left kept in the table —
    /// each one only while it is still the left entry's own: a boundary
    /// that mounted at the same path since then registered its own over
    /// it, from a slot of its own.
    fn take_buried(&mut self, graveyard: &[Box<Entry>]) {
        if self.buried_actions == 0 {
            return;
        }
        for entry in graveyard {
            for (key, action) in &entry.actions {
                if let std::collections::hash_map::Entry::Occupied(found) = self.actions.entry(Rc::clone(key))
                    && found.get().owner.as_ref().is_some_and(|owner| Rc::ptr_eq(owner, &entry.slot))
                    && Rc::ptr_eq(&found.get().action, action)
                {
                    found.remove();
                }
            }
        }
        self.buried_actions = 0;
    }

    /// The registrations of [`Live::unindex`] other than the click keys.
    fn unindex_rest(&mut self, path: &str, entry: &Entry) {
        if let Some(rare) = &entry.rare {
            for (key, _) in &rare.copies {
                self.copies.remove(key);
            }
            for (key, _) in &rare.editors {
                self.editors.remove(key);
            }
            for (key, _) in &rare.splits {
                self.splits.remove(key);
            }
            for (key, _) in &rare.scrolls {
                self.scrolls.remove(key);
            }
            for (key, _) in &rare.measures {
                self.measures.remove(key);
            }
            for (key, _) in &rare.webviews {
                self.webviews.remove(key);
            }
            for (key, _) in &rare.customs {
                self.customs.remove(key);
                self.keyed_customs.remove(key);
            }
        }
        if !entry.handlers().is_empty() && self.handler_entries.remove(path) {
            self.handler_gen += 1;
        }
        if !entry.contexts().is_empty() && self.context_entries.remove(path) {
            self.context_gen += 1;
        }
        if !entry.effects.is_empty() && self.effect_entries.remove(path) {
            self.effect_gen += 1;
        }
        if entry.top_level {
            self.top_level.remove(path);
        }
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

/// Does the path have no retained boundary above it? A top-level entry
/// has none: only the root region mounts it, so only the root region
/// can unmount it. Asked only with no body building around the path —
/// a body still open above it is a boundary above it, and the caller
/// knows that without a search ([`finish_entry`]).
fn is_top_level(path: &str) -> bool {
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        !cuts(path).any(|prefix| retained.contains_key(prefix))
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
    /// Did the entry it was filled for leave the retention? Set when it
    /// leaves, while its tree still waits for the idle, and never cleared:
    /// a boundary that mounts at the same path afterwards gets a slot of
    /// its own. The click keys the entry left behind ask it ([`Registered`]).
    left: Cell<bool>,
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
        Rc::new(Slot { held: RefCell::new(None), left: Cell::new(false) })
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
    let pass_no = PASS_NO.with(|count| {
        count.set(count.get() + 1);
        count.get()
    });
    // garbage that waited out its patience is freed now: no idle came
    let buried = BURIED_AT.with(Cell::get);
    let patience = if HOST_COLLECTS.with(Cell::get) { IDLE_HOST_PATIENCE } else { GARBAGE_PATIENCE };
    if buried != 0 && pass_no - buried >= patience {
        free_garbage();
    }
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
    decide_at(path).0
}

/// [`decide`], with the retained boundary's own path handed back when
/// there is one: the one copy of it the page holds, for the marks and
/// the reference to share instead of copying the path again.
pub(crate) fn decide_at(path: &str) -> (Decision, Option<(Rc<str>, Rc<Slot>)>) {
    PASS.with(|pass| {
        let pass = pass.borrow();
        RETAINED.with(|retained| {
            let retained = retained.borrow();
            // one lookup: the key and the slot the reference will point
            // at, and the entry the decision is stamped on
            let entry = retained.get_key_value(path);
            let found = entry.map(|(key, entry)| (Rc::clone(key), Rc::clone(&entry.slot)));
            if !pass.active {
                return (Decision::Render, found);
            }
            let inside_rerun = !pass.building.is_empty();
            let under_list = pass.building.last().is_some_and(|frame| frame.list);
            match entry {
                Some((_, entry)) if (!inside_rerun || under_list) && !pass.dirty.contains(path) => {
                    // the walk stays out on purpose, and the entry says
                    // so to the sweep: its subtree survives it
                    entry.visit.set(Visit::now().skipped);
                    (Decision::Skip, found)
                }
                _ => (Decision::Render, found),
            }
        })
    })
}

pub(crate) fn begin_entry(path: &Rc<str>, list: bool) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        pass.body_runs.push(Rc::clone(path));
        if pass.building.is_empty() {
            // no body open around it: a subtree the sweep will read
            pass.outermost.push(Rc::clone(path));
        }
        pass.building.push(BuildingFrame::new(Rc::clone(path), list));
    });
}

/// Files a body that ran under its path, and hands back the slot its
/// tree now fills — the one a reference to the boundary holds, so the
/// parent's list does not search the retention for it a second time.
///
/// `retained` is what the decision found: a boundary the retention did
/// not hold is a fresh mount, and there is no entry of a last run to
/// take out first. Nothing files one at the same path while the body
/// runs — every boundary below it has a longer path.
pub(crate) fn finish_entry(
    path: &Rc<str>,
    retained: bool,
    value: Erased,
    ctx: Context,
    node: RenderNode,
    layout: LayoutNode,
) -> Rc<Slot> {
    let (nested, (effects, actions, copies, editors, splits, scrolls, measures, webviews, customs, handlers, contexts)) =
        PASS.with(|pass| {
            let mut pass = pass.borrow_mut();
            let lists = match pass.building.pop() {
                Some(frame) => {
                    debug_assert_eq!(&*frame.path, &**path, "entries close in the order they open");
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
            };
            // a body still open around this one is a boundary above it:
            // every frame on the stack is an ancestor
            (!pass.building.is_empty(), lists)
        });
    let parent_segments = motor::identity::parent_seed();
    // a top-level entry is known by what stands above it: a body still
    // building around it says so at once, and only with none does the
    // retention have to be asked at each cut
    let top_level = !nested && is_top_level(path);
    let filled = RETAINED.with(|retention| {
        let mut retention = retention.borrow_mut();
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            // the slot is as old as the path: the entry of the last run
            // goes FIRST (its registrations leave the tables, its drop
            // empties the slot), then the slot is filled again, so a
            // parent that did not re-run refers to the tree of today
            let last = if retained { retention.remove(path) } else { None };
            let slot = match last {
                Some(old) => {
                    live.unindex(path, &old);
                    // the tree of the last run waits for the idle with the
                    // entries that left: a list that runs again replaces a
                    // node per row, and the frame must not pay their frees
                    if let Some(held) = old.slot.held.take() {
                        REPLACED.with(|replaced| replaced.borrow_mut().push(held));
                        note_buried();
                    }
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
                rare: Rare::boxed(copies, editors, splits, scrolls, measures, webviews, customs, handlers, contexts),
                parent_segments,
                top_level,
                visit: Cell::new(Visit::now().ran),
            };
            // a body ran and its registrations are new closures: they
            // replace the old ones in the tables the doors read, now
            live.index(path, &entry);
            crate::stats::note_entry_indexed();
            if top_level {
                live.top_level.insert(path.to_string());
            }
            let filled = Rc::clone(&entry.slot);
            if let Some(displaced) = retention.insert(Rc::clone(path), Box::new(entry)) {
                // a fresh mount displaces nothing, by construction; were
                // it ever to, the tables forget the keys of the entry it
                // displaced and learn the new one's again
                debug_assert!(false, "a boundary filed twice in one run: {path}");
                live.unindex(path, &displaced);
                if let Some(entry) = retention.get(&**path) {
                    live.index(path, entry);
                }
            }
            filled
        })
    });
    // …and so is every measure kept ABOVE it. The outermost re-run of a
    // pass does this once: the boundaries between it and the ones it
    // re-ran below are new entries themselves.
    if !nested {
        clear_measures_above(path);
    }
    filled
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
pub(crate) fn attribute_action(path: Rc<str>, action: ClickAction) {
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
            if let Some(entry) = retained.get(path.as_str()) {
                declared.extend(entry.contexts().iter().cloned());
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
            if let Some(entry) = retained.get(path.as_str()) {
                for (key, id, handler) in entry.handlers() {
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
    // shallower first, so an ancestor's run covers its dirty descendants —
    // then by path, so siblings of one depth run in ONE order. The set they
    // come from has none of its own, and a write one makes during the pass
    // reached the other before or after its body by the hash's whim
    // (T2-BUNNY-277).
    pending.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));

    for path in pending {
        let already_ran = PASS.with(|pass| {
            pass.borrow().body_runs.iter().any(|ran| covers(ran, &path))
        });
        if already_ran {
            continue;
        }
        let Some((value, ctx, parents)) = RETAINED.with(|retained| {
            retained.borrow().get(path.as_str()).map(|entry| {
                (entry.value.clone(), entry.ctx.clone(), entry.parent_segments.clone())
            })
        }) else {
            continue; // dirty but never mounted (or already swept): nothing to re-run
        };
        let _frames = motor::identity::seed_from(&path, &parents);
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
            if let Some(entry) = retained.get(path.as_str()) {
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
    let action = LIVE.with(|live| live.borrow().click(path));
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
    // a click key whose owner left waits for the idle: it is not live
    mix(live.actions.iter().fold(0u64, |sum, (key, registered)| {
        if skip(&live.root_keys.actions, key) || !registered.is_live() {
            sum
        } else {
            sum.wrapping_add(fingerprint_key(key) ^ fingerprint_ptr(&registered.action))
        }
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
            keys.actions.push(key.to_string());
            live.insert_action(key, action, None);
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
fn drop_entries<P: AsRef<str>>(paths: &[P]) {
    if paths.is_empty() {
        return;
    }
    RETAINED.with(|retained| {
        let mut retained = retained.borrow_mut();
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            GRAVEYARD.with(|graveyard| {
                let mut graveyard = graveyard.borrow_mut();
                let buried = graveyard.len();
                for path in paths {
                    if let Some((path, entry)) = retained.remove_entry(path.as_ref()) {
                        live.unindex_leaving(&path, &entry);
                        // a view that left owes the read graph nothing
                        // more: its reads fall with it, and its bindings
                        // hear no write from here. Unpicking their reads
                        // waits for the idle, with the entry's memory
                        motor::identity::retire_view(&path);
                        // the entry's memory — a layout tree, a value, the
                        // bindings' objects — is freed when the page is
                        // idle, not inside the frame that let it go
                        graveyard.push(entry);
                    }
                }
                if graveyard.len() > buried {
                    note_buried();
                }
            });
        });
    });
}

thread_local! {
    /// Entries that left the retention and wait for an idle moment to
    /// be freed: a thousand rows that leave a list are a thousand layout
    /// trees, and the frame that drops them must not pay their frees.
    static GRAVEYARD: RefCell<Vec<Box<Entry>>> = const { RefCell::new(Vec::new()) };
    /// The trees re-runs replaced, waiting for the same idle moment.
    static REPLACED: RefCell<Vec<Rc<Held>>> = const { RefCell::new(Vec::new()) };
    /// The pass that buried the oldest garbage still waiting; 0 when
    /// nothing waits.
    static BURIED_AT: Cell<u64> = const { Cell::new(0) };
    /// Has the host asked for the collection itself — a page that goes
    /// idle between two clicks? Then the valve waits for it as long as
    /// [`IDLE_HOST_PATIENCE`]. A newborn runtime has not been asked yet.
    static HOST_COLLECTS: Cell<bool> = const { Cell::new(false) };
}

/// How many passes garbage waits for an idle moment. A page goes idle
/// between two clicks and frees it there; a host that never does — a
/// shell that never asks for the collection — has it freed by the first
/// pass after this many, so what leaves is never kept for good.
const GARBAGE_PATIENCE: u64 = 64;

/// How many passes garbage waits on a host that has asked for the
/// collection. Such a host asks again within a second of any frame (the
/// page's idle callback has a one-second timeout), and a frame of its own
/// must never pay for the freeing: sixty-four passes are sixty-four frames
/// of a list that clicks fast, which may all fall inside one second. This
/// many passes are more than a quarter minute of frames at sixty a second
/// — no burst of clicks reaches it — and the valve stays the floor under a
/// host that stopped asking.
const IDLE_HOST_PATIENCE: u64 = 1024;

/// Garbage was buried in the pass under way: the oldest starts waiting.
fn note_buried() {
    BURIED_AT.with(|at| {
        if at.get() == 0 {
            at.set(PASS_NO.with(Cell::get).max(1));
        }
    });
}

/// Frees the entries that left since the last call, and the trees the
/// re-runs replaced; returns how many of both. The read graph of the
/// views that left goes first: the bindings they made are taken out of
/// the register and out of the live table. The host asks for this when
/// it is idle — and is known from then on as a host that does.
pub(crate) fn collect_garbage() -> usize {
    HOST_COLLECTS.with(|asked| asked.set(true));
    free_garbage()
}

/// [`collect_garbage`]'s work, for the host's idle and for the valve.
fn free_garbage() -> usize {
    BURIED_AT.with(|at| at.set(0));
    collect_retired_reads();
    let replaced = take_replaced();
    let count = replaced.len();
    drop(replaced);
    count
        + GRAVEYARD.with(|graveyard| {
            let mut graveyard = graveyard.borrow_mut();
            LIVE.with(|live| live.borrow_mut().take_buried(&graveyard));
            let count = graveyard.len();
            graveyard.clear();
            count
        })
}

/// The replaced trees, out of their list — to be dropped by the caller,
/// with no borrow of the list held while they fall.
fn take_replaced() -> Vec<Rc<Held>> {
    REPLACED.with(|replaced| std::mem::take(&mut *replaced.borrow_mut()))
}

/// The bindings retired since the last collection, taken apart: their
/// reads in the register ([`motor::identity::collect_retired`]), then
/// their keys in the live table — a key a body made again stays the new
/// binding's in both.
fn collect_retired_reads() {
    let torn = motor::identity::collect_retired();
    crate::bind::forget_live(&torn);
}

/// Diagnostics: entries waiting to be freed.
pub(crate) fn graveyard_len() -> usize {
    GRAVEYARD.with(|graveyard| graveyard.borrow().len())
}

/// Diagnostics: the trees re-runs replaced, waiting to be freed.
pub(crate) fn replaced_len() -> usize {
    REPLACED.with(|replaced| replaced.borrow().len())
}

/// Diagnostics: the click keys entries that left still keep in the live
/// table, for the idle to take out.
pub(crate) fn buried_actions() -> usize {
    LIVE.with(|live| live.borrow().buried_actions)
}

/// Is anything waiting for [`collect_garbage`] — an entry that left, a
/// tree a re-run replaced, or the bindings of a view that left, whose
/// reads still stand in the register?
pub(crate) fn garbage_pending() -> bool {
    graveyard_len() > 0 || replaced_len() > 0 || motor::identity::retirement_pending()
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
            .filter(|path| &***path == root || path.starts_with(&prefix))
            .map(|path| path.to_string())
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

/// Has the ordered walk passed the whole subtree of `shelter`, which
/// it met before `path`? The subtree `[shelter/, shelter0)` is one run
/// of the order, but it does not always follow its root at once: a
/// sibling that extends the name with a byte below `/` sorts between
/// them (`P/A` < `P/A!x` < `P/A/c`). Only a path at or past `shelter0`
/// has left it.
fn passed(path: &str, shelter: &str) -> bool {
    !path.starts_with(shelter) || path.as_bytes().get(shelter.len()).is_some_and(|byte| *byte >= b'0')
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
///
/// Who survives is read off the entries as the walk passes them: the
/// pass stamped each one it met ([`Visit`]), so a row costs a compare,
/// where it cost its path hashed into the sets of every run and every
/// skip of the pass.
pub(crate) fn sweep_stale(root: &str) {
    let mut outermost: Vec<Rc<str>> = PASS.with(|pass| std::mem::take(&mut pass.borrow_mut().outermost));
    // a run that began with nothing open is outermost by construction;
    // the few are checked against each other all the same
    outermost.sort_by_key(|run| run.len());
    let mut runs: Vec<Rc<str>> = Vec::with_capacity(outermost.len());
    for run in outermost {
        if !runs.iter().any(|outer| covers(outer, &run)) {
            runs.push(run);
        }
    }
    let visit = Visit::now();
    // a top-level entry the root region did not mount again fell, and
    // everything under it with it — unless the walk skipped a boundary
    // between the root and it. Asked first, while nothing has left
    let fallen: Vec<Rc<str>> = RETAINED.with(|retained| {
        let retained = retained.borrow();
        let alive_at_top = |path: &str| {
            let stamp = retained.get(path).map(|entry| entry.visit.get());
            if stamp == Some(visit.ran) || stamp == Some(visit.skipped) {
                return true;
            }
            let mut cut = path.len();
            while let Some(at) = path[..cut].rfind('/') {
                if at <= root.len() {
                    break;
                }
                if retained.get(&path[..at]).is_some_and(|entry| entry.visit.get() == visit.skipped) {
                    return true;
                }
                cut = at;
            }
            false
        };
        LIVE.with(|live| {
            live.borrow()
                .top_level
                .iter()
                .filter(|path| covers(root, path) && !alive_at_top(path))
                .map(|path| Rc::from(path.as_str()))
                .collect()
        })
    });
    RETAINED.with(|retained| {
        let mut retained = retained.borrow_mut();
        LIVE.with(|live| {
            let mut live = live.borrow_mut();
            GRAVEYARD.with(|graveyard| {
                let mut graveyard = graveyard.borrow_mut();
                let buried = graveyard.len();
                // the entry leaves the tables and the read graph as it
                // leaves the retention, and waits for the idle to be freed
                let mut fall = |path: &Rc<str>, entry: Box<Entry>| {
                    live.unindex_leaving(path, &entry);
                    motor::identity::retire_view(path);
                    graveyard.push(entry);
                };
                for run in &runs {
                    sweep_under(&mut retained, run, visit, &mut fall);
                }
                for top in &fallen {
                    sweep_under(&mut retained, top, visit, &mut fall);
                    if let Some((path, entry)) = retained.remove_entry(&**top) {
                        fall(&path, entry);
                    }
                }
                if graveyard.len() > buried {
                    note_buried();
                }
            });
        });
    });
}

/// Takes out of the retention every entry under `boundary` the pass did
/// not meet, and hands each to `fall`.
///
/// Alive under a boundary that ran: it ran itself, or it stands at or
/// under a boundary the walk skipped on purpose BELOW that run. A skipped
/// ancestor above the run says nothing about what is under the run: the
/// run rebuilt its subtree, and an entry it did not reach again has left
/// (a list under a clean page clears, and the page is skipped — its rows
/// must still go). The entries under one boundary come in one ordered
/// range — the subtree is contiguous because `/` sorts before every byte
/// a segment may start with after it — and a skipped entry comes before
/// its own subtree, so the walk carries the skips it is inside. An entry
/// that leaves is taken out where a walk stands: no search from the root
/// of the tree for each one.
fn sweep_under(
    retained: &mut BTreeMap<Rc<str>, Box<Entry>>,
    boundary: &str,
    visit: Visit,
    fall: &mut impl FnMut(&Rc<str>, Box<Entry>),
) {
    // who leaves, read off the stamps in a walk that only looks: the
    // skips it stands inside are borrowed, and a row that stays costs
    // nothing but its compare
    let mut leaving: Vec<Rc<str>> = Vec::new();
    let lo = format!("{boundary}/");
    let hi = format!("{boundary}0");
    let mut walked = 0;
    {
        let range = (std::ops::Bound::Included(lo.as_str()), std::ops::Bound::Excluded(hi.as_str()));
        let mut shelters: Vec<&str> = Vec::new();
        for (path, entry) in retained.range::<str, _>(range) {
            walked += 1;
            while shelters.last().is_some_and(|shelter| passed(path, shelter)) {
                shelters.pop();
            }
            let stamp = entry.visit.get();
            if stamp == visit.skipped {
                shelters.push(path);
            } else if stamp != visit.ran && !shelters.iter().any(|shelter| covers(shelter, path)) {
                leaving.push(Rc::clone(path));
            }
        }
    }
    // everything under the boundary leaves — a list that clears — and
    // it is most of the retention: the range is cut out of the tree at
    // its two ends, where taking the entries out one by one rebalanced
    // the tree at each of them. The cuts touch the entries of the
    // smaller side, and putting back what lies past the range costs no
    // more entries than leave
    if walked > 0 && leaving.len() == walked && walked * 2 >= retained.len() {
        let mut under = retained.split_off(lo.as_str());
        let mut after = under.split_off(hi.as_str());
        if after.len() > retained.len() {
            std::mem::swap(retained, &mut after);
        }
        retained.extend(after);
        for (path, entry) in under {
            fall(&path, entry);
        }
        return;
    }
    // …or taken out in a second walk, from the first that leaves to the
    // last, each where the walk stands: the list is in the order of the
    // walk, and holds the very keys it meets
    let (Some(first), Some(last)) = (leaving.first(), leaving.last()) else {
        return;
    };
    let range = (std::ops::Bound::Included(Rc::clone(first)), std::ops::Bound::Included(Rc::clone(last)));
    let mut next = 0;
    let taken = retained.extract_if(range, |path, _| {
        let leaves = leaving.get(next).is_some_and(|dead| Rc::ptr_eq(dead, path));
        next += usize::from(leaves);
        leaves
    });
    for (path, entry) in taken {
        fall(&path, entry);
    }
}

/// Drops the whole retention — the next pass runs every body (the
/// tests' `render_full`; the state in the identity arenas stays).
pub(crate) fn clear() {
    RETAINED.with(|retained| retained.borrow_mut().clear());
    LIVE.with(|live| *live.borrow_mut() = Live::default());
    collect_retired_reads();
    drop(take_replaced());
    GRAVEYARD.with(|graveyard| graveyard.borrow_mut().clear());
    BURIED_AT.with(|at| at.set(0));
    ASSEMBLED_AT.with(|at| at.set(None));
}

/// The world-reset twin of [`clear`]: the retention AND every table
/// falls. A newborn runtime starts from nothing — see
/// `motor::identity::reset_world` for the other half of the contract.
pub(crate) fn reset_world() {
    RETAINED.with(|retained| retained.borrow_mut().clear());
    collect_retired_reads();
    drop(take_replaced());
    GRAVEYARD.with(|graveyard| graveyard.borrow_mut().clear());
    BURIED_AT.with(|at| at.set(0));
    HOST_COLLECTS.with(|asked| asked.set(false));
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
        FRAME_BODY_RUNS.with(|frame| frame.borrow_mut().extend(runs.iter().map(|run| run.to_string())));
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
    LAST_BODY_RUNS.with(|last| last.borrow().iter().map(|run| run.to_string()).collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::ActionId;
    use crate::prelude::*;
    use crate::runtime::Runtime;

    /// The carrier sets and the top level, built again from the
    /// retention: who carries a handler, a key context, an effect, and
    /// who closed with nothing retained above it.
    fn carriers_match_retention() -> bool {
        RETAINED.with(|retained| {
            let retained = retained.borrow();
            let built = |carries: &dyn Fn(&Entry) -> bool| -> HashSet<String> {
                retained.iter().filter(|(_, entry)| carries(entry)).map(|(path, _)| path.to_string()).collect()
            };
            let handlers = built(&|entry| !entry.handlers().is_empty());
            let contexts = built(&|entry| !entry.contexts().is_empty());
            let effects = built(&|entry| !entry.effects.is_empty());
            let top_level = built(&|entry| entry.top_level);
            LIVE.with(|live| {
                let live = live.borrow();
                live.handler_entries == handlers
                    && live.context_entries == contexts
                    && live.effect_entries == effects
                    && live.top_level == top_level
            })
        })
    }

    /// An entry that falls takes its path out of the carrier sets and the
    /// top level by what it CARRIES, never by asking each set: the guard
    /// holds only while a path stands in a set exactly as long as its
    /// entry carries what the set is for. A carrier that re-runs and sheds
    /// its handler, one that takes it back, and one that unmounts — after
    /// each, the sets are what the retention says they are.
    #[test]
    fn the_carrier_sets_hold_exactly_the_entries_that_carry() {
        const POKE: ActionId = ActionId("test.carrier");

        #[derive(Clone, Copy)]
        struct Carrier {
            armed: State<bool>,
        }

        impl Component for Carrier {
            fn body(self, _ctx: &Context) -> impl View {
                if self.armed.get() {
                    Either::First(text("armed").on_action(POKE, || {}).key_context("carrier").on_appear(|| {}))
                } else {
                    Either::Second(text("plain"))
                }
            }
        }

        #[derive(Clone, Copy)]
        struct Holder {
            mounted: State<bool>,
            armed: State<bool>,
        }

        impl Component for Holder {
            fn body(self, _ctx: &Context) -> impl View {
                if self.mounted.get() {
                    Either::First(Carrier { armed: self.armed })
                } else {
                    Either::Second(text("closed"))
                }
            }
        }

        let holder = Holder { mounted: State::new(true), armed: State::new(true) };
        let runtime = Runtime::new();
        runtime.render_stable(&holder);
        let carried = |set: fn(&Live) -> usize| LIVE.with(|live| set(&live.borrow()));
        assert_eq!(carried(|live| live.handler_entries.len()), 1, "the carrier stands in the set");
        assert!(carriers_match_retention(), "mounted, armed");

        holder.armed.set(false);
        runtime.render_stable(&holder);
        assert_eq!(carried(|live| live.handler_entries.len()), 0, "it shed its handler");
        assert!(carriers_match_retention(), "re-ran and shed its handler, context and effect");

        holder.armed.set(true);
        runtime.render_stable(&holder);
        assert!(carriers_match_retention(), "re-ran and took them back");

        // the handler and the context are kept apart from the entry, boxed
        // only while the body makes them
        let boxed = || {
            RETAINED.with(|retained| {
                retained.borrow().iter().filter(|(_, entry)| entry.rare.is_some()).map(|(path, _)| path.to_string()).collect::<Vec<_>>()
            })
        };
        assert_eq!(boxed().len(), 1, "the armed carrier boxes its rare registrations: {:?}", boxed());
        holder.armed.set(false);
        runtime.render_stable(&holder);
        assert!(boxed().is_empty(), "a body that makes none keeps no box: {:?}", boxed());
        holder.armed.set(true);
        runtime.render_stable(&holder);

        holder.mounted.set(false);
        runtime.render_stable(&holder);
        assert_eq!(carried(|live| live.handler_entries.len()), 0, "the carrier left with its entry");
        assert!(carriers_match_retention(), "unmounted");
        assert_eq!(carried(|live| live.top_level.len()), 1, "the holder alone stands at the top");
    }

    /// A row of a list, its label bound.
    #[derive(Clone, Copy)]
    struct Line {
        id: usize,
        label: State<Rc<str>>,
    }

    impl Component for Line {
        fn body(self, _ctx: &Context) -> impl View {
            crate::text!(self.label)
        }
    }

    /// A page that is only its list: the list runs, the page is skipped.
    #[derive(Clone, Copy)]
    struct Lines {
        lines: State<Rc<Vec<Line>>>,
    }

    impl Component for Lines {
        fn body(self, _ctx: &Context) -> impl View {
            crate::views::for_each(self.lines, |line| line.id.to_string(), |line| *line)
        }
    }

    fn lines(ids: std::ops::RangeInclusive<usize>) -> Rc<Vec<Line>> {
        Rc::new(ids.map(|id| Line { id, label: State::new(Rc::from(format!("line {id}").as_str())) }).collect())
    }

    /// A body that runs again fills its slot with the tree of today at
    /// once — the page above it did not run, and reads the list through
    /// that slot — while the tree it replaced waits for the idle with the
    /// entries that left, and leaves with them.
    #[test]
    fn a_list_that_runs_again_leaves_the_tree_it_replaced_for_the_idle() {
        let replaced = || REPLACED.with(|replaced| replaced.borrow().len());
        let page = Lines { lines: State::new(lines(1..=3)) };
        let runtime = Runtime::new();
        let printed = runtime.render(&page);
        assert!(printed.contains("line 3"), "{printed}");
        let _ = collect_garbage();

        page.lines.set(lines(4..=5));
        let printed = runtime.render(&page);
        assert!(printed.contains("line 4") && printed.contains("line 5"), "the tree of today: {printed}");
        assert!(!printed.contains("line 3"), "and nothing of the last one: {printed}");
        assert_eq!(runtime.body_runs().len(), 3, "the list and its two new rows ran, the page did not: {:?}", runtime.body_runs());
        assert_eq!(replaced(), 1, "the list's last tree waits for the idle");
        assert_eq!(graveyard_len(), 3, "with the rows that left");

        assert_eq!(collect_garbage(), 4, "the idle frees the tree and the rows");
        assert_eq!(replaced(), 0);
        let printed = runtime.render(&page);
        assert!(printed.contains("line 4") && printed.contains("line 5"), "the slot still holds today's tree: {printed}");
    }

    /// A page frees what left when it goes idle. A host that never goes
    /// idle — a shell that never asks for the collection — must not keep
    /// it for good: garbage that waited out its patience is freed by the
    /// next pass, the read graph of its bindings with it. Until then it
    /// waits, so the frames of a page that does go idle still free
    /// nothing. A runtime born on a thread whose last host asked for the
    /// collection has not been asked itself.
    #[test]
    fn garbage_no_idle_came_for_is_freed_by_a_later_pass() {
        let earlier = Runtime::new();
        let _ = collect_garbage();
        drop(earlier);
        let three = (1..=3).map(|id| Line { id, label: State::new(Rc::from("line")) }).collect();
        let lines = State::new(Rc::new(three));
        let page = Lines { lines };
        let runtime = Runtime::new();
        runtime.render(&page);
        lines.set(Rc::new(Vec::new()));
        runtime.render(&page);
        assert_eq!(graveyard_len(), 3, "three lines wait for the idle");
        assert_eq!(motor::identity::retired_count(), 3, "and their bindings");

        for _ in 1..GARBAGE_PATIENCE {
            runtime.render(&page);
        }
        assert_eq!(graveyard_len(), 3, "within its patience the garbage waits");
        runtime.render(&page);
        assert_eq!(graveyard_len(), 0, "past it, the pass freed it");
        assert_eq!(motor::identity::retired_count(), 0, "read graph and all");
    }

    /// A row that answers a click — or, unarmed, shows the same words
    /// and answers nothing. The click counts into the row's own state.
    #[derive(Clone, Copy)]
    struct Pressable {
        id: usize,
        armed: bool,
        presses: State<usize>,
    }

    impl Component for Pressable {
        fn body(self, _ctx: &Context) -> impl View {
            let presses = self.presses;
            let words = format!("row {}", self.id);
            if self.armed {
                Either::First(text(words).on_click(move || presses.set(presses.get() + 1)))
            } else {
                Either::Second(text(words))
            }
        }
    }

    #[derive(Clone, Copy)]
    struct Pressables {
        rows: State<Rc<Vec<Pressable>>>,
    }

    impl Component for Pressables {
        fn body(self, _ctx: &Context) -> impl View {
            crate::views::for_each(self.rows, |row| row.id.to_string(), |row| *row)
        }
    }

    fn pressables(ids: &[usize]) -> Vec<Pressable> {
        ids.iter().map(|&id| Pressable { id, armed: true, presses: State::new(0) }).collect()
    }

    /// The click keys standing in the live table whose path holds `part`.
    fn click_keys(part: &str) -> Vec<String> {
        LIVE.with(|live| {
            live.borrow().actions.keys().filter(|key| key.contains(part)).map(|key| key.to_string()).collect()
        })
    }

    fn buried_actions() -> usize {
        LIVE.with(|live| live.borrow().buried_actions)
    }

    /// A row that leaves keeps its click key in the live table until the
    /// idle: a thousand rows hashed out of it were the larger part of the
    /// sweep. But a key whose owner left fires nothing — a press queued
    /// before the frame still names the element it hit, which is gone —
    /// and the tables say what the retention says all the while. The idle
    /// takes the key out; the rows that stayed answer throughout.
    #[test]
    fn a_row_that_left_fires_nothing_until_the_idle_takes_its_key_out() {
        let all = pressables(&[1, 2, 3]);
        let rows = State::new(Rc::new(all.clone()));
        let page = Pressables { rows };
        let runtime = Runtime::new();
        runtime.render(&page);
        let gone = click_keys("[2]");
        let kept = click_keys("[3]");
        assert_eq!((gone.len(), kept.len()), (1, 1), "one click per row: {gone:?} {kept:?}");
        assert!(run_action(&gone[0], 1), "row 2 answers while it stands");
        assert_eq!(all[1].presses.get(), 1);

        rows.set(Rc::new(vec![all[0], all[2]]));
        runtime.render(&page);
        assert_eq!(click_keys("[2]"), gone, "the key of the row that left waits for the idle");
        assert_eq!(buried_actions(), 1);
        assert!(!run_action(&gone[0], 1), "and fires nothing");
        assert_eq!(all[1].presses.get(), 1, "the row that left was not pressed");
        assert!(live_tables_match_retention(), "a waiting key is not a live one");
        assert!(run_action(&kept[0], 1), "a row that stayed answers");
        assert_eq!(all[2].presses.get(), 1);

        let _ = collect_garbage();
        assert!(click_keys("[2]").is_empty(), "the idle took the key out");
        assert_eq!(buried_actions(), 0);
        assert!(!run_action(&gone[0], 1));
        assert!(run_action(&kept[0], 1), "and left the rows that stayed alone");
        assert_eq!(all[2].presses.get(), 2);
    }

    /// A row that comes back before the idle registers its click again at
    /// the same key, over the one the row that left kept there: it answers
    /// with the new closure, and the idle that follows leaves it standing.
    /// One that comes back with nothing to click stands at the same path
    /// with a slot of its own — the left key is still not its, and fires
    /// nothing.
    #[test]
    fn a_row_that_comes_back_before_the_idle_answers_with_its_own_click() {
        let all = pressables(&[1, 2, 3]);
        let rows = State::new(Rc::new(all.clone()));
        let page = Pressables { rows };
        let runtime = Runtime::new();
        runtime.render(&page);
        let key = click_keys("[2]").pop().expect("row 2's click");

        rows.set(Rc::new(vec![all[0], all[2]]));
        runtime.render(&page);
        rows.set(Rc::new(all.clone()));
        runtime.render(&page);
        assert!(run_action(&key, 1), "the row that came back answers");
        assert_eq!(all[1].presses.get(), 1);
        let _ = collect_garbage();
        assert_eq!(click_keys("[2]"), [key.clone()], "the idle kept the new registration");
        assert!(run_action(&key, 1));
        assert_eq!(all[1].presses.get(), 2);

        // it leaves again, and comes back with nothing to click
        rows.set(Rc::new(vec![all[0], all[2]]));
        runtime.render(&page);
        let plain = Pressable { armed: false, ..all[1] };
        rows.set(Rc::new(vec![all[0], plain, all[2]]));
        runtime.render(&page);
        let back = retained_under("Pressables");
        assert!(back.iter().any(|path| path.contains("[2]")), "row 2 stands again: {back:?}");
        assert!(!run_action(&key, 1), "the path stands again, but the key is not the new row's");
        assert_eq!(all[1].presses.get(), 2);
        assert!(live_tables_match_retention());
        let _ = collect_garbage();
        assert!(click_keys("[2]").is_empty(), "the idle took the left key out");
    }

    /// A host that asks for the collection when it goes idle — the page —
    /// never has one of its frames pay for the freeing. Lists replaced,
    /// rows swapped and lists cleared, click after click, with the idle
    /// late by far more passes than a host that never asks is given: every
    /// frame keeps what it let go, and what earlier frames let go, for the
    /// idle. The idle then frees it all.
    #[test]
    fn a_host_that_collects_at_idle_never_frees_inside_a_frame() {
        let rows = State::new(lines(1..=10));
        let page = Lines { lines: rows };
        let runtime = Runtime::new();
        runtime.render(&page);
        let _ = collect_garbage();

        let idle_at = PASS_NO.with(Cell::get);
        let mut next = 11;
        let mut waiting = 0;
        for click in 0..GARBAGE_PATIENCE * 3 {
            match click % 3 {
                0 => {
                    rows.set(lines(next..=next + 9));
                    next += 10;
                }
                1 => {
                    let mut swapped = (*rows.get()).clone();
                    swapped.swap(0, 9);
                    rows.set(Rc::new(swapped));
                }
                _ => rows.set(Rc::new(Vec::new())),
            }
            runtime.render(&page);
            let now = graveyard_len() + REPLACED.with(|replaced| replaced.borrow().len());
            assert!(now > waiting, "click {click} freed what waited for the idle: {waiting} -> {now}");
            waiting = now;
        }
        let late_by = PASS_NO.with(Cell::get) - idle_at;
        assert!(late_by >= GARBAGE_PATIENCE * 3, "the idle was late by {late_by} passes");
        assert_eq!(collect_garbage(), waiting, "the idle frees it all");
        assert_eq!(graveyard_len(), 0);
    }

    /// A list that clears, most of the retention, leaves whole: its range
    /// is cut out of the tree at its two ends. What sorts before the list
    /// and what sorts after it — a header, a footer with boundaries of its
    /// own, more of them than stand before — stays retained, every entry,
    /// and every row leaves as a row taken out alone would: to the idle,
    /// out of the tables, its bindings retired.
    #[test]
    fn a_list_that_clears_leaves_whole_and_keeps_what_sorts_around_it() {
        #[derive(Clone, Copy)]
        struct Note(&'static str);

        impl Component for Note {
            fn body(self, _ctx: &Context) -> impl View {
                text(self.0)
            }
        }

        #[derive(Clone, Copy)]
        struct Footer;

        impl Component for Footer {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(Note("first"), Note("second"), Note("third"), Note("fourth"))
            }
        }

        #[derive(Clone, Copy)]
        struct Framed {
            lines: State<Rc<Vec<Line>>>,
        }

        impl Component for Framed {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(Note("head"), Lines { lines: self.lines }, Footer)
            }
        }

        let rows = State::new(lines(1..=12));
        let page = Framed { lines: rows };
        let runtime = Runtime::new();
        runtime.render(&page);
        let all = retained_under("Framed");
        let around: Vec<String> = all.iter().filter(|path| !path.contains("/[")).cloned().collect();
        assert_eq!(all.len() - around.len(), 12, "a row each: {all:?}");
        let _ = collect_garbage();

        rows.set(Rc::new(Vec::new()));
        let printed = runtime.render(&page);
        assert_eq!(retained_under("Framed"), around, "the rows left, and everything around them stayed");
        assert_eq!(graveyard_len(), 12, "to the idle");
        assert_eq!(motor::identity::retired_count(), 12, "their bindings retired");
        assert!(live_tables_match_retention() && carriers_match_retention());
        assert!(printed.contains("head") && printed.contains("third") && !printed.contains("line"), "{printed}");
        assert_eq!(collect_garbage(), 13, "the rows and the list's old tree");
    }

    /// The paths retained under a prefix, sorted.
    fn retained_under(prefix: &str) -> Vec<String> {
        RETAINED.with(|retained| {
            retained.borrow().keys().filter(|path| path.starts_with(prefix)).map(|path| path.to_string()).collect()
        })
    }

    /// The sweep reads who survives off the entries in path order, and a
    /// row the walk skipped shelters its subtree. The subtree is one run
    /// of the order, but not always right after its row: a row whose key
    /// extends the other's path with a byte that sorts below `/` falls in
    /// between (`…/[a]/Row` < `…/[a]/Row!]/Row` < `…/[a]/Row/Leaf`). Both
    /// rows are kept while the list re-runs around them, and neither one
    /// loses the boundary inside it.
    #[test]
    fn a_row_that_sorts_inside_another_rows_name_leaves_its_subtree_sheltered() {
        #[derive(Clone)]
        struct Leaf {
            word: Rc<str>,
        }

        impl Component for Leaf {
            fn body(self, _ctx: &Context) -> impl View {
                text(self.word.to_string())
            }
        }

        #[derive(Clone)]
        struct Row {
            key: Rc<str>,
        }

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                Leaf { word: Rc::clone(&self.key) }
            }
        }

        #[derive(Clone, Copy)]
        struct Page {
            keys: State<Rc<Vec<Rc<str>>>>,
        }

        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                crate::views::for_each(self.keys, |key| key.to_string(), |key| Row { key: Rc::clone(key) })
            }
        }

        let keys: State<Rc<Vec<Rc<str>>>> = State::new(Rc::new(vec![Rc::from("a"), Rc::from("a]/Row!")]));
        let page = Page { keys };
        let runtime = Runtime::new();
        runtime.render_stable(&page);
        let leaves = |all: &[String]| all.iter().filter(|path| path.ends_with("/Leaf")).cloned().collect::<Vec<_>>();
        let before = retained_under("Page");
        let sheltered = leaves(&before);
        assert_eq!(sheltered.len(), 2, "a leaf in each row: {before:?}");
        let inner = sheltered.iter().find(|path| path.contains("[a]/Row/")).expect("row a's leaf");
        let between = before.iter().find(|path| path.ends_with("[a]/Row!]/Row")).expect("the other row");
        assert!(
            inner.as_str() > between.as_str() && between.as_str() > inner.trim_end_matches("/Leaf"),
            "the other row sorts between row a and its leaf: {before:?}"
        );

        // the list re-runs around both rows: they are kept, and skipped
        keys.set(Rc::new(vec![Rc::from("a"), Rc::from("a]/Row!"), Rc::from("b")]));
        runtime.render_stable(&page);
        let after = retained_under("Page");
        for leaf in &sheltered {
            assert!(after.contains(leaf), "{leaf} survived the sweep: {after:?}");
        }
        assert_eq!(after.len(), before.len() + 2, "row b and its leaf joined: {after:?}");

        // and a row that leaves takes its leaf along
        keys.set(Rc::new(vec![Rc::from("a]/Row!"), Rc::from("b")]));
        runtime.render_stable(&page);
        let gone = retained_under("Page");
        assert!(!gone.contains(inner) && gone.len() == after.len() - 2, "row a and its leaf left: {gone:?}");
    }

    /// A frame's pass prints nothing, so the entries it files keep no
    /// name — and a print that comes after still names every boundary,
    /// because a retention built without print is built again before
    /// anything prints it.
    #[test]
    fn a_frame_files_no_name_and_a_print_after_it_names_every_boundary() {
        let page = Lines { lines: State::new(lines(1..=2)) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&page, crate::layout::Size { width: 400.0, height: 300.0 });
        let named: Vec<String> = RETAINED.with(|retained| {
            retained.borrow().iter().filter(|(_, entry)| !entry.node.line.is_empty()).map(|(path, _)| path.to_string()).collect()
        });
        assert!(named.is_empty(), "a frame names no boundary: {named:?}");
        let printed = runtime.render(&page);
        assert!(printed.starts_with("Lines"), "the page is named: {printed}");
        assert!(printed.matches("Line\n").count() == 2, "and so is each row: {printed}");
    }

    /// Every reference to a boundary, in every retained tree, with the
    /// slot it holds.
    fn references() -> Vec<(Rc<str>, Rc<Slot>)> {
        fn walk(node: &LayoutNode, out: &mut Vec<(Rc<str>, Rc<Slot>)>) {
            match node {
                LayoutNode::BoundaryRef { path, slot, .. } => out.push((Rc::clone(path), Rc::clone(slot))),
                LayoutNode::Stack { children, .. } | LayoutNode::Boundary { children, .. } => {
                    children.iter().for_each(|child| walk(child, out));
                }
                LayoutNode::Hinted { child, .. }
                | LayoutNode::Interactive { child, .. }
                | LayoutNode::Styled { child, .. } => walk(child, out),
                _ => {}
            }
        }
        let mut out = Vec::new();
        RETAINED.with(|retained| {
            for entry in retained.borrow().values() {
                entry.slot.with_layout(|tree| tree.into_iter().for_each(|tree| walk(tree, &mut out)));
            }
        });
        out
    }

    /// What a closing body knows without searching must be what a
    /// search would find. The reference a parent's list keeps holds the
    /// very slot its boundary's entry was filed with — on a fresh mount
    /// and on a re-run alike — and an entry stands at the top level
    /// exactly when no retained boundary sits above it.
    #[test]
    fn a_closing_body_knows_its_slot_and_its_level_without_a_search() {
        thread_local! {
            static TOGGLE_RUNS: Cell<usize> = const { Cell::new(0) };
        }

        #[derive(Clone, Copy)]
        struct Toggle {
            id: usize,
            on: State<bool>,
        }

        impl Component for Toggle {
            fn body(self, _ctx: &Context) -> impl View {
                TOGGLE_RUNS.with(|runs| runs.set(runs.get() + 1));
                if self.on.get() { Either::First(text("on")) } else { Either::Second(text("off")) }
            }
        }

        #[derive(Clone, Copy)]
        struct Toggles {
            toggles: State<Rc<Vec<Toggle>>>,
        }

        impl Component for Toggles {
            fn body(self, _ctx: &Context) -> impl View {
                crate::views::for_each(self.toggles, |toggle| toggle.id.to_string(), |toggle| toggle.element("tr"))
            }
        }

        let check = |when: &str| {
            let refs = references();
            assert!(!refs.is_empty(), "{when}: the list refers to its rows");
            RETAINED.with(|retained| {
                let retained = retained.borrow();
                for (path, slot) in &refs {
                    let entry = retained.get(&**path).unwrap_or_else(|| panic!("{when}: {path} is retained"));
                    assert!(Rc::ptr_eq(&entry.slot, slot), "{when}: the reference to {path} holds its entry's slot");
                }
                for (path, entry) in retained.iter() {
                    let above = cuts(path).any(|prefix| retained.contains_key(prefix));
                    assert_eq!(entry.top_level, !above, "{when}: {path} stands at the top exactly when nothing is above it");
                }
            });
        };

        let on = State::new(false);
        let toggles = State::new(Rc::new((1..=3).map(|id| Toggle { id, on }).collect::<Vec<_>>()));
        let page = Toggles { toggles };
        let runtime = Runtime::new();
        let size = crate::layout::Size { width: 400.0, height: 300.0 };
        let _ = runtime.dom_frame(&page, size);
        assert_eq!(TOGGLE_RUNS.with(Cell::get), 3, "three rows mounted");
        check("mounted");

        // the rows re-run on their own: their entries are replaced
        on.set(true);
        let _ = runtime.dom_frame(&page, size);
        assert_eq!(TOGGLE_RUNS.with(Cell::get), 6, "the three rows ran again");
        check("re-run");

        // the list re-runs: the kept rows are skipped, a new one mounts
        toggles.set(Rc::new((1..=4).map(|id| Toggle { id, on }).collect()));
        let _ = runtime.dom_frame(&page, size);
        assert_eq!(TOGGLE_RUNS.with(Cell::get), 7, "the new row alone ran");
        check("a row added");
    }
}
