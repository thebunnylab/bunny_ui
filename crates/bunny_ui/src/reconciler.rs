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

use std::cell::RefCell;
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

/// A text field's editor: applies a command to the (binding, caret)
/// pair and returns the output of `Read`/`Copy`/`Cut`. Retained like
/// the actions — a skipped view's field still edits.
pub(crate) type EditorFn = Rc<dyn Fn(EditCommand, &mut CaretState) -> Option<String>>;
pub(crate) type EditorEntry = (String, EditorFn);

/// A split divider's position writer: the drag hands it the new lane-A
/// extent in layout points and it reaches the app's binding. Retained
/// like the actions — a skipped view's divider still drags.
pub(crate) type SplitFn = Rc<dyn Fn(crate::layout::Px)>;
pub(crate) type SplitEntry = (String, SplitFn);

/// A NAMED action handler registered at render: (registration path,
/// id, what runs). Retained like the actions — a skipped view's
/// handler lives.
pub(crate) type HandlerFn = Rc<dyn Fn()>;
pub(crate) type HandlerEntry = (String, crate::action::ActionId, HandlerFn);

pub(crate) struct Entry {
    pub value: Erased,
    pub ctx: Context,
    pub node: RenderNode,
    /// The body's layout tree — retained along with the print (the two
    /// outputs of the same body-eval).
    pub layout: LayoutNode,
    pub effects: Vec<EffectFn>,
    /// The body's interactive actions — retained like the effects: a
    /// skipped view's button stays clickable.
    pub actions: Vec<ActionEntry>,
    /// The body's field editors — same retention.
    pub editors: Vec<EditorEntry>,
    /// The body's split-position writers — same retention.
    pub splits: Vec<SplitEntry>,
    /// The paths of the app's own boxes (`custom(…)`) — the map that
    /// says a focused escape hatch is still on screen.
    /// `(path, does it take the keyboard)` — the second half is
    /// what keeps a re-point from handing the keyboard to a box
    /// that answers nothing.
    pub customs: Vec<(String, bool)>,
    /// The body's named-action handlers — same retention.
    pub handlers: Vec<HandlerEntry>,
    /// Key contexts declared in the body (`.key_context(name)`) — a
    /// context is ACTIVE while a view declaring it stays mounted.
    pub contexts: Vec<&'static str>,
    /// The PARENT's path segments — the cursor seed for an isolated re-run.
    pub parent_segments: Vec<String>,
}

#[derive(Default)]
struct BuildingFrame {
    path: String,
    effects: Vec<EffectFn>,
    actions: Vec<ActionEntry>,
    editors: Vec<EditorEntry>,
    splits: Vec<SplitEntry>,
    customs: Vec<(String, bool)>,
    handlers: Vec<HandlerEntry>,
    contexts: Vec<&'static str>,
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
    root_editors: Vec<EditorEntry>,
    root_splits: Vec<SplitEntry>,
    root_customs: Vec<(String, bool)>,
    root_handlers: Vec<HandlerEntry>,
    root_contexts: Vec<&'static str>,
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
}

/// A boundary's retained layout tree, borrowed in place — measure and
/// place resolve `BoundaryRef` through here, WITHOUT stitching an
/// expanded copy. Borrows nest (ref inside ref = shared borrows of the
/// same RefCell); no body runs during layout, so no mutable re-borrow
/// is possible.
pub(crate) fn with_retained_layout<R>(
    path: &str,
    reader: impl FnOnce(Option<&LayoutNode>) -> R,
) -> R {
    RETAINED.with(|retained| {
        let retained = retained.borrow();
        reader(retained.get(path).map(|entry| &entry.layout))
    })
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
pub(crate) fn decide(path: &str) -> Decision {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if !pass.active {
            return Decision::Render;
        }
        let inside_rerun = !pass.building.is_empty();
        let retained = RETAINED.with(|retained| retained.borrow().contains_key(path));
        if !inside_rerun && retained && !pass.dirty.contains(path) {
            pass.skipped.push(path.to_string());
            Decision::Skip
        } else {
            Decision::Render
        }
    })
}

pub(crate) fn begin_entry(path: &str) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        pass.body_runs.push(path.to_string());
        pass.building.push(BuildingFrame { path: path.to_string(), ..Default::default() });
    });
}

pub(crate) fn finish_entry(
    path: &str,
    value: Erased,
    ctx: Context,
    node: RenderNode,
    layout: LayoutNode,
) {
    let (effects, actions, editors, splits, customs, handlers, contexts) = PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        match pass.building.pop() {
            Some(frame) => {
                debug_assert_eq!(frame.path, path, "entries close in the order they open");
                (
                    frame.effects,
                    frame.actions,
                    frame.editors,
                    frame.splits,
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
            ),
        }
    });
    let parent_segments = motor::identity::current_path_segments()
        .split_last()
        .map(|(_, parents)| parents.to_vec())
        .unwrap_or_default();
    RETAINED.with(|retained| {
        retained.borrow_mut().insert(
            path.to_string(),
            Entry {
                value,
                ctx,
                node,
                layout,
                effects,
                actions,
                editors,
                splits,
                customs,
                handlers,
                contexts,
                parent_segments,
            },
        );
    });
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

/// A named-action handler registered during render — same attribution
/// as the actions: entry being built, or the root region.
/// A key context declared during render — active while its view stays
/// mounted (retained like the handlers; the sweep deactivates it).
pub(crate) fn attribute_context(name: &'static str) {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        if let Some(frame) = pass.building.last_mut() {
            frame.contexts.push(name);
        } else {
            pass.root_contexts.push(name);
        }
    });
}

thread_local! {
    static ACTIVE_CONTEXTS: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::default());
}

/// Rebuilds the active-context set from the retention (the live
/// declarations) — the twin of the handler assembly.
pub(crate) fn assemble_contexts(root: &str) {
    let mut active: HashSet<&'static str> = HashSet::default();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                active.extend(entry.contexts.iter().copied());
            }
        }
    });
    PASS.with(|pass| {
        active.extend(std::mem::take(&mut pass.borrow_mut().root_contexts));
    });
    ACTIVE_CONTEXTS.with(|contexts| *contexts.borrow_mut() = active);
}

/// Is the context declared by any mounted view?
pub(crate) fn context_active(name: &str) -> bool {
    ACTIVE_CONTEXTS.with(|contexts| contexts.borrow().contains(name))
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

/// Reassembles the handler map from the retention under the root + the
/// root region. Precedence: the DEEPEST path wins (innermost in the
/// tree); a depth tie → the last one mounted (deterministic from the
/// retention order, documented as NON-contractual — the semantic
/// tiebreak arrives with key contexts).
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
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
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
        let _frames = motor::identity::seed(&parents);
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

/// The pass's effect queue: the root region + the whole retention under
/// the current root (skipped or not — a retained effect is a live
/// subscription).
pub(crate) fn assemble_effects(root: &str) -> Vec<EffectFn> {
    let mut queue = PASS.with(|pass| std::mem::take(&mut pass.borrow_mut().root_effects));
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                queue.extend(entry.effects.iter().cloned());
            }
        }
    });
    queue
}

thread_local! {
    /// The live click map: target path → action, which takes the
    /// platform's click count. Reassembled on every pass, like the
    /// effect queue.
    static ACTIONS: RefCell<HashMap<String, ClickAction>> = RefCell::new(HashMap::default());
}

/// Reassembles the click map from the retention under the root (a
/// skipped view's button stays clickable) + the root region.
pub(crate) fn assemble_actions(root: &str) {
    let mut map: HashMap<String, ClickAction> = HashMap::default();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                for (key, action) in &entry.actions {
                    map.insert(key.clone(), action.clone());
                }
            }
        }
    });
    PASS.with(|pass| {
        for (key, action) in std::mem::take(&mut pass.borrow_mut().root_actions) {
            map.insert(key, action);
        }
    });
    ACTIONS.with(|actions| *actions.borrow_mut() = map);
}

/// Fires the target's action (the key comes from the hit-test).
/// `false` = target not registered (the identity died between frame and
/// click — harmless).
pub(crate) fn run_action(path: &str, clicks: u8) -> bool {
    let action = ACTIONS.with(|actions| actions.borrow().get(path).cloned());
    match action {
        Some(action) => {
            action(clicks);
            true
        }
        None => false,
    }
}

thread_local! {
    /// The live field-editor map — reassembled per pass, like the
    /// actions.
    static EDITORS: RefCell<HashMap<String, EditorFn>> = RefCell::new(HashMap::default());
    static SPLITS: RefCell<HashMap<String, SplitFn>> = RefCell::new(HashMap::default());
    /// The app's boxes on screen this pass — paths only.
    static CUSTOMS: RefCell<HashSet<String>> = RefCell::new(HashSet::default());
    /// The subset that answers `accepts_keys` — who may HOLD the
    /// keyboard, as opposed to who is merely on screen.
    static KEYED_CUSTOMS: RefCell<HashSet<String>> = RefCell::new(HashSet::default());
}

/// Reassembles the editor map from retention under the root + root region.
pub(crate) fn assemble_editors(root: &str) {
    let mut map: HashMap<String, EditorFn> = HashMap::default();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                for (key, editor) in &entry.editors {
                    map.insert(key.clone(), editor.clone());
                }
            }
        }
    });
    PASS.with(|pass| {
        for (key, editor) in std::mem::take(&mut pass.borrow_mut().root_editors) {
            map.insert(key, editor);
        }
    });
    EDITORS.with(|editors| *editors.borrow_mut() = map);
}

/// Reassembles the split map from retention — the editors' twin.
pub(crate) fn assemble_splits(root: &str) {
    let mut map: HashMap<String, SplitFn> = HashMap::default();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                for (key, split) in &entry.splits {
                    map.insert(key.clone(), split.clone());
                }
            }
        }
    });
    PASS.with(|pass| {
        for (key, split) in std::mem::take(&mut pass.borrow_mut().root_splits) {
            map.insert(key, split);
        }
    });
    SPLITS.with(|splits| *splits.borrow_mut() = map);
}

/// Reassembles the escape-hatch map from retention — the editors' twin
/// for the boxes the app paints.
pub(crate) fn assemble_customs(root: &str) {
    let mut set: HashSet<String> = HashSet::default();
    let mut keyed: HashSet<String> = HashSet::default();
    RETAINED.with(|retained| {
        for (path, entry) in retained.borrow().iter() {
            if covers(root, path) {
                for (path, accepts_keys) in &entry.customs {
                    set.insert(path.clone());
                    if *accepts_keys {
                        keyed.insert(path.clone());
                    }
                }
            }
        }
    });
    PASS.with(|pass| {
        for (path, accepts_keys) in std::mem::take(&mut pass.borrow_mut().root_customs) {
            if accepts_keys {
                keyed.insert(path.clone());
            }
            set.insert(path);
        }
    });
    CUSTOMS.with(|customs| *customs.borrow_mut() = set);
    KEYED_CUSTOMS.with(|keyed_customs| *keyed_customs.borrow_mut() = keyed);
}

/// Is the app's box at this path still on screen? (The focus of an
/// escape hatch lives or dies by this answer.)
pub(crate) fn has_custom(path: &str) -> bool {
    CUSTOMS.with(|customs| customs.borrow().contains(path))
}

/// Hands a dragged divider position to the split's retained writer.
/// `false` = no split registered at the path.
pub(crate) fn run_split(path: &str, at: crate::layout::Px) -> bool {
    let split = SPLITS.with(|splits| splits.borrow().get(path).cloned());
    match split {
        Some(split) => {
            split(at);
            true
        }
        None => false,
    }
}

/// Is the target a text field? (decides if a click FOCUSES instead of acting)
/// The ONE live input whose named chain is `chain` — a field's editor
/// or a box that takes the keyboard. `None` when nothing answers, and
/// `None` when TWO do: an ambiguous name must never hand the keyboard
/// over on a guess.
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
        if motor::identity::named_chain(path) != chain {
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
    let ambiguous = EDITORS.with(|editors| editors.borrow().keys().any(&mut walk))
        || (!editors_only
            && KEYED_CUSTOMS.with(|customs| customs.borrow().iter().any(&mut walk)));
    if ambiguous { None } else { found }
}

pub(crate) fn has_editor(path: &str) -> bool {
    EDITORS.with(|editors| editors.borrow().contains_key(path))
}

/// Applies a command to the field — the retained closure is what
/// reaches the binding. Outer `None` = field not registered; the inner
/// `Option` is the command's output.
pub(crate) fn run_editor(
    path: &str,
    command: EditCommand,
    state: &mut CaretState,
) -> Option<Option<String>> {
    let editor = EDITORS.with(|editors| editors.borrow().get(path).cloned());
    editor.map(|editor| editor(command, state))
}

/// Identities swept by `end_pass`: their entries fall with them.
pub(crate) fn forget(dead: &[String]) {
    RETAINED.with(|retained| {
        let mut retained = retained.borrow_mut();
        for path in dead {
            retained.remove(path);
        }
    });
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
pub(crate) fn sweep_stale(root: &str) {
    let (runs, skipped) = PASS.with(|pass| {
        let pass = pass.borrow();
        (
            pass.body_runs.iter().cloned().collect::<HashSet<String>>(),
            pass.skipped.clone(),
        )
    });
    RETAINED.with(|retained| {
        retained.borrow_mut().retain(|path, _| {
            if !covers(root, path) {
                return true; // another tree mounted on the same thread
            }
            runs.contains(path) || skipped.iter().any(|skip| covers(skip, path))
        });
    });
}

/// Drops the whole retention — the next pass runs every body (the
/// tests' `render_full`; the state in the identity arenas stays).
pub(crate) fn clear() {
    RETAINED.with(|retained| retained.borrow_mut().clear());
}

/// The world-reset twin of [`clear`]: the retention AND every per-pass
/// assembly falls. A newborn runtime starts from nothing — see
/// `motor::identity::reset_world` for the other half of the contract.
pub(crate) fn reset_world() {
    RETAINED.with(|retained| retained.borrow_mut().clear());
    PASS.with(|pass| *pass.borrow_mut() = PassState::default());
    LAST_BODY_RUNS.with(|last| last.borrow_mut().clear());
    FRAME_BODY_RUNS.with(|frame| frame.borrow_mut().clear());
    ACTIVE_CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
    HANDLERS.with(|handlers| handlers.borrow_mut().clear());
    ACTIONS.with(|actions| actions.borrow_mut().clear());
    EDITORS.with(|editors| editors.borrow_mut().clear());
    SPLITS.with(|splits| splits.borrow_mut().clear());
    CUSTOMS.with(|customs| customs.borrow_mut().clear());
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
        for child in &node.children {
            expanded.children.push(expand(child));
        }
        expanded
    } else {
        RenderNode {
            line: node.line.clone(),
            children: node.children.iter().map(expand).collect(),
        }
    }
}
