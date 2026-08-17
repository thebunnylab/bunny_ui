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
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use motor::state::{Context, EffectFn};
use motor::view::RenderNode;

use crate::erased::Erased;
use crate::layout::LayoutNode;
use crate::text_input::{CaretState, EditCommand};

/// An interactive action registered during render: (target path, what
/// the click fires).
pub(crate) type ActionEntry = (String, Rc<dyn Fn()>);

/// A text field's editor: applies a command to the (binding, caret)
/// pair and returns the output of `Read`/`Copy`/`Cut`. Retained like
/// the actions — a skipped view's field still edits.
pub(crate) type EditorFn = Rc<dyn Fn(EditCommand, &mut CaretState) -> Option<String>>;
pub(crate) type EditorEntry = (String, EditorFn);

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
    /// The body's named-action handlers — same retention.
    pub handlers: Vec<HandlerEntry>,
    /// The PARENT's path segments — the cursor seed for an isolated re-run.
    pub parent_segments: Vec<String>,
}

#[derive(Default)]
struct BuildingFrame {
    path: String,
    effects: Vec<EffectFn>,
    actions: Vec<ActionEntry>,
    editors: Vec<EditorEntry>,
    handlers: Vec<HandlerEntry>,
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
    root_handlers: Vec<HandlerEntry>,
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
    let (effects, actions, editors, handlers) = PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        match pass.building.pop() {
            Some(frame) => {
                debug_assert_eq!(frame.path, path, "entries close in the order they open");
                (frame.effects, frame.actions, frame.editors, frame.handlers)
            }
            None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
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
                handlers,
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
pub(crate) fn attribute_action(path: String, action: Rc<dyn Fn()>) {
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

/// A named-action handler registered during render — same attribution
/// as the actions: entry being built, or the root region.
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
        RefCell::new(HashMap::new());
}

/// Reassembles the handler map from the retention under the root + the
/// root region. Precedence: the DEEPEST path wins (innermost in the
/// tree); a depth tie → the last one mounted (deterministic from the
/// retention order, documented as NON-contractual — the semantic
/// tiebreak arrives with key contexts).
pub(crate) fn assemble_handlers(root: &str) {
    let mut map: HashMap<crate::action::ActionId, (usize, HandlerFn)> = HashMap::new();
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
    path == ancestor || path.starts_with(&format!("{ancestor}/"))
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
    /// The live click map: target path → action. Reassembled on every
    /// pass, like the effect queue.
    static ACTIONS: RefCell<HashMap<String, Rc<dyn Fn()>>> = RefCell::new(HashMap::new());
}

/// Reassembles the click map from the retention under the root (a
/// skipped view's button stays clickable) + the root region.
pub(crate) fn assemble_actions(root: &str) {
    let mut map: HashMap<String, Rc<dyn Fn()>> = HashMap::new();
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
pub(crate) fn run_action(path: &str) -> bool {
    let action = ACTIONS.with(|actions| actions.borrow().get(path).cloned());
    match action {
        Some(action) => {
            action();
            true
        }
        None => false,
    }
}

thread_local! {
    /// The live field-editor map — reassembled per pass, like the
    /// actions.
    static EDITORS: RefCell<HashMap<String, EditorFn>> = RefCell::new(HashMap::new());
}

/// Reassembles the editor map from retention under the root + root region.
pub(crate) fn assemble_editors(root: &str) {
    let mut map: HashMap<String, EditorFn> = HashMap::new();
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

/// Is the target a text field? (decides if a click FOCUSES instead of acting)
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

pub(crate) fn end_pass() {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        pass.active = false;
        let runs = std::mem::take(&mut pass.body_runs);
        LAST_BODY_RUNS.with(|last| *last.borrow_mut() = runs);
    });
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
