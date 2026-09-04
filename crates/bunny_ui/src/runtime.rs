//! `Runtime` — the fake main-thread of this layer: it renders the typed
//! tree, pumps effects, and settles (re-render until the printed tree
//! stops changing — the stand-in for the frame loop).
//!
//! Each `render` is an identity pass ([`motor::identity`]) driven by the
//! reconciler: clean, retained boundaries SKIP the body (the cache
//! answers), dirty ones re-run — even behind a skipped parent (isolated
//! re-run from the retained value). The effect queue is reassembled from
//! the retention, the sweep unmounts what left the tree, and the final
//! assembly expands the references. `set()` marks dirty whoever READ —
//! the fine invalidation joins the stability condition and is visible in
//! [`Runtime::take_dirty`] and [`Runtime::body_runs`].

use std::cell::{Cell, RefCell};
use motor::hash::FxHashMap as HashMap;
use std::rc::Rc;

use motor::state::{Context, EnvironmentValues};

use crate::action::{ActionId, KeyPattern, OVERLAY_CONTEXT, OVERLAY_DISMISS};
use crate::effects;
use crate::layout::{
    FieldPlacement, Interaction, LayoutEnv, OverlayPlacement, Point, Px, Rect, ScrollRegion,
};
use crate::reconciler;
use crate::image_engine::{ImageEngine, RawImages};
use crate::text_engine::{FontSpec, MeasureCache, PixelFont, TextEngine, caret_from_x};
use crate::text_input::{CaretState, EditCommand, word_around};
use crate::view::{NodeList, View};

/// The result of an edit command: `applied` = a focused field was there
/// to receive it (the shell repaints); `output` = the text that `Read`/
/// `Copy`/`Cut` extract (the bridge for the clipboard and for IME sync).
pub struct Edited {
    pub applied: bool,
    pub output: Option<String>,
}

/// What the platform asks the focused field (NSTextInputClient and
/// friends): text, selection, and composition in UTF-16 — its
/// vocabulary — and the caret rect in LAYOUT coordinates (the shell
/// converts to screen; it is where the IME candidate window lands).
pub struct ImeSnapshot {
    pub text: String,
    /// (location, length) in UTF-16.
    pub selected: (usize, usize),
    /// Marked range in UTF-16, if a composition is live.
    pub marked: Option<(usize, usize)>,
    pub caret_rect: Rect,
}

/// One live box whose picture changed on a clock step: where it sits
/// and the fresh pixels. The shell presents it on the box's OWN
/// surface — the window behind it never redraws.
pub struct LiveBlit {
    /// The box's identity — stable across steps; a shell keys the
    /// box's layer on it.
    pub path: String,
    /// The rect the pixels cover, in LAYOUT coordinates (the visible
    /// window of the box).
    pub frame: Rect,
    /// Physical pixel size of `rgba`.
    pub width: usize,
    pub height: usize,
    /// Straight (not premultiplied) RGBA rows, top-down.
    pub rgba: Vec<u8>,
}

/// What one live box left on its own surface: the picture it painted
/// and the PHYSICAL size those pixels were rasterized at. A step is
/// dropped only when both still hold — same picture, same size.
struct LiveCell {
    display: Vec<crate::layout::DrawCommand>,
    physical: (usize, usize),
}

/// The wait between a hover and its bubble — armed by the pointer,
/// aged by one shell tick, shown on the next. No clock in sight.
#[derive(Default)]
struct TooltipLife {
    pending: Option<crate::layout::TooltipRegion>,
    aged: bool,
}

/// One binding that takes a sequence of strokes. `context` is `None`
/// for the global layer — the same two shelves single strokes have.
struct Chord {
    strokes: Box<[KeyPattern]>,
    action: ActionId,
    context: Option<&'static str>,
}

pub struct Runtime {
    ctx: Context,
    /// The scene this runtime renders, when the thread has more than
    /// one — pushed as the FIRST identity segment of every pass, so two
    /// windows showing the same root view are two trees and not one.
    /// `None` is the single-window world every app and every probe has
    /// had, where the root view's own name is the root of the pass.
    scene: Option<Rc<str>>,
    /// The root of the last pass — scopes `take_dirty` so it does not
    /// drain dirt from another tree mounted on the same thread.
    last_root: RefCell<Option<String>>,
    /// The targets of the last layout, OUTER before INNER with siblings
    /// in paint order — the hit-test
    /// map for pointer events.
    last_hits: RefCell<Vec<(String, Rect)>>,
    /// Handlers the HOST mounted, outside any view. The tree's own
    /// handlers are the reconciler's and are rebuilt every pass; these
    /// stand until the host takes them down, and they are the OUTERMOST
    /// registration there is — any mounted view claiming the same id
    /// shadows them.
    hosted_handlers: RefCell<HashMap<ActionId, Rc<dyn Fn()>>>,
    /// Pointer state for the frame — resolved BEFORE layout (the LAW:
    /// hover swaps paint, never measurement) and stamped at expansion.
    interaction: RefCell<Interaction>,
    /// What the hand was holding on the last move. It sits OUTSIDE the
    /// interaction record on purpose: the framework spends none of it,
    /// so it is not scene vocabulary and must not enter the layout the
    /// way a hovered path does. It is remembered for one reason — a
    /// hover that re-resolves after a layout has no shell to ask.
    pointer_modifiers: std::cell::Cell<crate::action::Modifiers>,
    /// The text edge of the frame — PixelFont by default (headless,
    /// byte-stable); the shell installs the platform engine.
    text: Rc<dyn TextEngine>,
    /// The image edge — RawImages by default (the house raw format +
    /// deterministic file-icon checkers); the shell installs the
    /// platform decoder.
    images: Rc<dyn ImageEngine>,
    /// Double-buffered measure cache, swapped on every layout pass.
    cache: MeasureCache,
    /// Scroll offsets by identity — engine-owned (the premise's dual
    /// ownership: on the DOM the backend will own them and we will
    /// observe). DELIBERATELY not pruned when the identity goes away:
    /// a remounted list RESTORES the position, and the map is bounded
    /// by region SITES, never by rows. The other input maps (carets,
    /// targets, auto-focus) release on the sweep.
    scroll_offsets: RefCell<HashMap<String, Point>>,
    /// Scroll viewports the BROWSER reported (`bunny_dom_viewport`,
    /// from a ResizeObserver) — the flow frame's window math reads
    /// them; the pixel targets never fill them.
    dom_viewports: RefCell<HashMap<String, (f64, f64)>>,
    /// Browser-reported boxes by island path — a FLEXIBLE island
    /// measures against its real box, not against a guess.
    island_boxes: RefCell<HashMap<Rc<str>, (f64, f64)>>,
    /// The app's boxes inside each island, frames ISLAND-LOCAL — the
    /// canvas pointer door routes the browser's coordinates by them.
    dom_customs: RefCell<Vec<(Rc<str>, crate::layout::CustomPlacement)>>,
    /// The scroll regions of the last layout — the wheel map.
    last_scrolls: RefCell<Vec<ScrollRegion>>,
    /// The line the topmost modal layer drew this frame — nothing the
    /// scene placed under it answers the pointer or the wheel.
    last_modal_floor: std::cell::Cell<Option<crate::layout::ModalFloor>>,
    /// The focused field (identity path) — owner of the keyboard.
    focus: RefCell<Option<String>>,
    /// Caret + selection per field — they survive blur/refocus and
    /// remount (restored by identity, like scroll).
    carets: RefCell<HashMap<String, CaretState>>,
    /// Blink phase: the caret goes and comes back on the shell tick;
    /// typing or focusing returns it to solid (an idle caret blinks,
    /// an active one does not).
    caret_visible: Cell<bool>,
    /// The column a vertical walk keeps while it crosses short lines.
    /// Any other caret move clears it — the walk starts fresh from
    /// wherever the caret now stands.
    goal_column: Cell<Option<Px>>,
    /// The fields of the last layout (geometry + effective font) —
    /// click-to-position and IME sync measure through here.
    last_fields: RefCell<Vec<FieldPlacement>>,
    /// The splits of the last layout — a divider drag maps the pointer
    /// back to a lane extent through this geometry.
    last_splits: RefCell<Vec<crate::layout::SplitPlacement>>,
    /// The app's own boxes from the last layout — an event resolves its
    /// element and its local coordinates through here.
    last_customs: RefCell<Vec<crate::layout::CustomPlacement>>,
    /// The native hosts of the last layout — the boxes the shell
    /// mounts platform views over, each frame.
    last_hosts: RefCell<Vec<crate::layout::HostPlacement>>,
    /// Eval answers still owed: the app's `then`, keyed by the token
    /// the drain stamped on the op. The shell answers through
    /// [`Runtime::webview_eval_done`]; a swept page answers late or
    /// never, and the entry waits — bounded by evals in flight.
    webview_evals: RefCell<HashMap<u64, crate::host::EvalSink>>,
    /// Snapshot answers still owed — the eval ledger's twin.
    webview_snaps: RefCell<HashMap<u64, crate::host::SnapshotSink>>,
    /// The next eval or snapshot token — ONE counter for the whole
    /// runtime, so a late answer can never land on another question.
    webview_eval_next: Cell<u64>,
    /// What each live box painted on its last step, in LOCAL
    /// coordinates, and the PHYSICAL size it was rasterized at — a
    /// step that paints the same picture at the same size blits
    /// nothing. Swept against the live boxes still placed.
    ///
    /// The size belongs here as much as the picture. A live box owns a
    /// surface of its own, and a surface holds the pixels it was given:
    /// hand it a new frame without new pixels and it STRETCHES the old
    /// ones. A box whose picture does not depend on its width would do
    /// exactly that through a window resize.
    live_ledger: RefCell<motor::hash::FxHashMap<Rc<str>, LiveCell>>,
    /// The theme version the last pass saw — switching themes rebuilds
    /// the retention ONCE (tokens read in a body are baked into the
    /// scene).
    theme_version: Cell<u64>,
    /// The app keymap: key pattern → action. Runtime config (like the
    /// text engine), not retention — bind is a declaration of intent.
    keymap: RefCell<HashMap<KeyPattern, ActionId>>,
    /// Context-scoped bindings (`bind_in`): active only while a mounted
    /// view declares the context (`.key_context(name)`).
    scoped_keymap: RefCell<HashMap<&'static str, HashMap<KeyPattern, ActionId>>>,
    /// The bindings that take MORE than one stroke. A flat list, walked
    /// on a keystroke: a product carries dozens of these beside
    /// hundreds of single strokes, and comparing a two-element prefix
    /// costs nothing next to the frame it precedes.
    chords: RefCell<Vec<Chord>>,
    /// The strokes of a sequence still in the air. Empty means the
    /// keyboard is free.
    pending: RefCell<Vec<KeyPattern>>,
    /// A pending prefix that has seen one slow tick. The second one
    /// drops it — the tooltip's own idiom, and the reason `cmd-k` can
    /// never hold the keyboard for good.
    pending_aged: Cell<bool>,
    /// Who hears the sequence move — a which-key panel's door. Called
    /// with the strokes in the air after every change: a stroke that
    /// opened or lengthened a sequence, and the end of one, however it
    /// ended (an action, a dead end, Escape, the slow tick).
    chord_sink: RefCell<Option<Rc<dyn Fn(&[KeyPattern])>>>,
    /// Whether the sink has heard the sequence now in the air — so its
    /// end is announced exactly when its start was, and a plain stroke
    /// (pushed and resolved in one breath) says nothing at all.
    chord_announced: Cell<bool>,
    /// The size last HANDED to each measurement probe. A probe fires on
    /// change and only on change: a view at rest costs nothing, and a
    /// handler that writes state cannot spin against its own report.
    measures: RefCell<HashMap<String, crate::layout::Size>>,
    /// The offset last PUBLISHED to each region's binding. It is what
    /// tells a value the app WROTE apart from the value the region
    /// itself landed on and reported: only one of the two can differ
    /// from this, and whichever does is the one that moved.
    scroll_commands: RefCell<HashMap<String, Point>>,
    /// The last APPLIED `.scroll_target` per region — the follow fires
    /// only when the target changes; in between, the wheel is sovereign.
    scroll_targets: RefCell<HashMap<String, String>>,
    /// The last APPLIED reveal per app box — the same "only on change"
    /// door the targets above keep, so the wheel is never fought.
    element_reveals: RefCell<HashMap<String, crate::layout::Rect>>,
    /// Fields whose `.auto_focus()` already fired — first appearance
    /// only; a user blur is final.
    auto_focused: RefCell<std::collections::HashSet<String>>,
    /// The retained animations — springs keyed by identity, resolved
    /// at place through the env, advanced by the shell's tick.
    animator: RefCell<crate::anim::Animator>,
    /// The proposal of the last layout pass — a pass with a DIFFERENT
    /// one is a resize, and a resize snaps animated retargets (the
    /// window tracks the mouse; nothing wobbles after it).
    last_proposal: Cell<Option<crate::layout::Proposal>>,
    /// The popovers of the last layout, in paint order (last =
    /// topmost) — the outside-press dismissal and the shells' second
    /// surfaces read from here.
    last_overlays: RefCell<Vec<OverlayPlacement>>,
    /// The `.tooltip(…)` regions of the last layout, in paint order.
    last_tooltips: RefCell<Vec<crate::layout::TooltipRegion>>,
    /// The `.context_menu(…)` regions of the last layout.
    last_menus: RefCell<Vec<crate::layout::MenuRegion>>,
    /// The open menu's ACTIONS, by row index — the stamp carries only
    /// the labels; the closures stay here and fire on the pick.
    menu_items: RefCell<Option<std::rc::Rc<[crate::views::MenuItem]>>>,
    /// The `.on_drag(…)` regions of the last layout.
    last_drag_sources: RefCell<Vec<crate::layout::DragSourceRegion>>,
    /// The `.on_drop(…)` regions of the last layout.
    last_drops: RefCell<Vec<crate::layout::DropRegion>>,
    /// The rings the element tree currently shows — a drag moves
    /// between targets without running one body, so the reuse
    /// shortcut has to hear about it from here.
    last_drop_rings: RefCell<Vec<bool>>,
    /// The pressed-but-not-lifted drag: its builder and where the
    /// press landed. Past the threshold it becomes the live value.
    drag_armed: RefCell<Option<(crate::layout::DragBuilder, Point)>>,
    /// The click count of the press that ARMED the pressed target. The
    /// platform counts (AppKit's `clickCount`, the Win32 counter, the
    /// web shell's) — the framework holds no clock. One press arms at a
    /// time, so the number needs no key: it belongs to whatever
    /// `interaction.pressed` names, and the release takes it.
    pressed_clicks: Cell<u8>,
    /// The lifted drag's VALUE — the stamp carries only label and
    /// geometry; the typed value stays here and lands on the drop.
    drag_value: RefCell<Option<std::rc::Rc<dyn std::any::Any>>>,
    /// The target that last heard a preview, and the closure to tell
    /// when the drag leaves it. Exactly ONE box is ever previewing, so
    /// the leaving `None` has one address and can never be lost.
    drag_preview: RefCell<Option<(Rect, crate::layout::DragOverAction)>>,
    /// The tooltip's whole life — the runtime owns it, the scene only
    /// declares. CLOCKLESS: the delay is the shell's tick seen twice,
    /// so no Instant crosses into wasm and the tests drive it by hand.
    tooltip: RefCell<TooltipLife>,
    /// Window-drag regions of the last layout — the desktop shell's
    /// press gate consults them.
    last_drag_regions: RefCell<Vec<Rect>>,
    last_control_regions: RefCell<Vec<(crate::layout::WindowControl, Rect)>>,
    /// Where popovers may live, in layout coordinates. `None` = the
    /// viewport; the desktop shell sets the SCREEN — overflow becomes
    /// plain geometry.
    overlay_bounds: Cell<Option<Rect>>,
    /// Where the shell is holding each open DIALOG's window, by
    /// overlay path — a `.dialog(…)` presented as a real window
    /// reports the window's travels here and the next pass lays its
    /// content out inside them. Entries survive a close on purpose:
    /// reopening lands where the reader left the window (the session's
    /// memory — a fresh runtime starts centered again).
    dialog_frames: RefCell<HashMap<String, Rect>>,
    /// How many PHYSICAL pixels one layout point is worth on this
    /// screen. The shell installs it; everyone else keeps `1.0`.
    device_scale: Cell<Px>,
    /// The Dom mode's retained scene — [`Runtime::dom_frame`] diffs
    /// each new capture against it. Empty (and free) in every other
    /// mode.
    dom: RefCell<crate::dom::DomLowering>,
    /// Did the last pass see the root become ONE boundary
    /// (`Boundary`/ref)? Only then can the stable frame synthesize the
    /// reference without a pass — a boundary-less root comes fresh
    /// from the walk on every frame.
    root_is_boundary: Cell<bool>,
    /// The retention can hold entries WITHOUT print lines (built on the
    /// frame path, which does not format) — printing again rebuilds
    /// once, and the full == incremental oracle stays byte-for-byte.
    printless: Cell<bool>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    /// A runtime with the deterministic house font.
    ///
    /// A runtime opens its own world: retention and identity are
    /// thread state, and they reset when a runtime is born — a second
    /// runtime on the same thread starts from nothing instead of
    /// adopting the first one's retained bodies (whose recorded reads
    /// would point at dead state slots and kill invalidation
    /// silently). State declared OUTSIDE any pass — the app-scope
    /// pattern every harness uses — has no owner and survives the
    /// hand-over; state anchored inside the old world's bodies dies
    /// with it, and so do its tasks. One runtime at a time per thread
    /// is still the law: two runtimes ALTERNATING frames on one
    /// thread would fight over one world.
    pub fn new() -> Self {
        Self::with_parts(Context::default(), Rc::new(PixelFont))
    }

    pub fn with_environment(values: EnvironmentValues) -> Self {
        let mut ctx = Context::default();
        ctx.values = values;
        Self::with_parts(ctx, Rc::new(PixelFont))
    }

    /// A runtime that renders ONE NAMED SCENE of a thread that holds
    /// several — the window road.
    ///
    /// [`Runtime::new`] opens its world by closing every other: a second
    /// runtime on the thread inherits nothing, which is right when it
    /// REPLACES the first and fatal when it stands beside it. A named
    /// scene keeps the same promise inside its own subtree and leaves the
    /// neighbours alone: `name` becomes the first segment of every
    /// identity under it (`w1/Workbench/…`), so the sweep, the dirty set
    /// and the retention — all already scoped by root — separate on their
    /// own, and only this scene's world is reset when the runtime is born.
    ///
    /// The name must be unique on the thread for as long as the runtime
    /// lives; the shell mints `w0`, `w1`, … per window.
    pub fn scene(name: impl Into<Rc<str>>) -> Self {
        Self::scene_with_parts(name.into(), Context::default(), Rc::new(PixelFont))
    }

    /// Swaps the text engine (builder — composes with `with_environment`):
    /// `Runtime::new().text_engine(Rc::new(CoreTextEngine::new()))`.
    pub fn text_engine(mut self, engine: Rc<dyn TextEngine>) -> Self {
        self.text = engine;
        self
    }

    /// Swaps the image engine (builder, mirror of [`Self::text_engine`]).
    pub fn image_engine(mut self, engine: Rc<dyn ImageEngine>) -> Self {
        self.images = engine;
        self
    }

    /// The frame's image edge — the shell hands it to its presenters
    /// (the GPU atlas and the CPU surface resolve pixels through it).
    pub fn images(&self) -> Rc<dyn ImageEngine> {
        Rc::clone(&self.images)
    }

    /// Moves the keyboard to the field the app NAMED with `.id(…)`.
    ///
    /// A field's identity path is structural — the scene's prefix, then every
    /// wrapper down to it — so an app that wants to put the caret somewhere
    /// (a form's Tab walk, a screen that opens with one box already live)
    /// would have to spell a path it does not own. It owns the NAME, and this
    /// resolves it against the last layout's fields.
    ///
    /// `false` when nothing laid out under that name — before the first
    /// frame there are no fields to reach.
    pub fn focus_named(&self, id: &str) -> bool {
        let tail = format!("[{id}]");
        let path = self
            .last_fields
            .borrow()
            .iter()
            .find(|field| field.path.ends_with(&tail))
            .map(|field| field.path.clone());
        match path {
            Some(path) => {
                self.focus(&path);
                true
            }
            None => false,
        }
    }

    /// The identity path `rel` has inside this scene — what an app hands
    /// [`Runtime::focus`] when it wants a field by name and the window's
    /// own prefix is not its to guess.
    ///
    /// ```ignore
    /// runtime.focus(&runtime.scene_path("Gate/password"));
    /// ```
    pub fn scene_path(&self, rel: &str) -> String {
        match &self.scene {
            Some(name) => format!("{name}/{rel}"),
            None => rel.to_string(),
        }
    }

    /// Rebuilds everything a finished pass leaves standing: the tables the
    /// input doors read, and the effect queue.
    fn assemble_scene(&self, root: &str) {
        // The effect queue belongs to the PASS that produced it, and only to
        // it: `pump` TAKES the queue and runs what is in it, so refilling it
        // outside a pass re-arms every `.task` under the root — a fresh
        // thread each, on every refill. Which is why `enter_scene` rebuilds
        // the input tables and never this.
        effects::set_queue(reconciler::assemble_effects(root));
        self.assemble_input(root);
    }

    /// Rebuilds only the tables the input doors read.
    fn assemble_input(&self, root: &str) {
        reconciler::assemble_actions(root);
        reconciler::assemble_editors(root);
        reconciler::assemble_splits(root);
        reconciler::assemble_scrolls(root);
        reconciler::assemble_measures(root);
        reconciler::assemble_webviews(root);
        reconciler::assemble_customs(root);
        reconciler::assemble_handlers(root);
        reconciler::assemble_contexts(root);
        reconciler::set_assembled_root(root);
    }



    /// Makes THIS scene the one the thread's assembled tables answer for.
    ///
    /// A window's event arrives long after its frame, and on a thread
    /// with two windows the frame in between may have been the other
    /// one's — which would leave the click map, the handlers and the
    /// editors belonging to the window the hand is NOT in. The retention
    /// holds every scene at once, so the fix is to rebuild this root's
    /// view of it; a single-window app never pays, because the marker
    /// already names its root.
    fn enter_scene(&self) {
        let Some(root) = self.last_root.borrow().clone() else {
            return;
        };
        if reconciler::assembled_root().as_deref() == Some(root.as_str()) {
            return;
        }
        // The INPUT tables only. The effect queue is the pass's, not the
        // door's: refilling it here re-arms every task under this root, and a
        // thread with it — which two windows alternating frames turn into
        // thousands within seconds.
        self.assemble_input(&root);
    }

    /// The popovers of the last layout, in paint order — a shell that
    /// presents them on their own surfaces re-slices the display list
    /// by each placement.
    pub fn overlays(&self) -> Vec<OverlayPlacement> {
        self.last_overlays.borrow().clone()
    }

    /// Where popovers may live, in LAYOUT coordinates. The desktop
    /// shell sets the screen's visible frame (an origin left of or
    /// above the window is negative); everyone else leaves the
    /// default — the viewport.
    pub fn set_overlay_bounds(&self, bounds: Option<Rect>) {
        self.overlay_bounds.set(bounds);
    }

    /// Where the shell is holding an open dialog's WINDOW — content
    /// origin and size, in layout coordinates. The next pass lays the
    /// dialog's content out inside exactly this frame: the window
    /// drives, the content follows. The entry survives a close, so a
    /// reopen lands where the reader left it.
    pub fn set_dialog_frame(&self, path: &str, frame: Rect) {
        self.dialog_frames.borrow_mut().insert(path.to_string(), frame);
    }

    /// Runs ONE overlay's dismissal — the road a dialog window's own
    /// close button arrives by (the shell hears `windowShouldClose`,
    /// fires this, and the flipped binding is what closes the window).
    /// `true` = the overlay answered and the shell repaints.
    pub fn dismiss_overlay(&self, path: &str) -> bool {
        reconciler::run_action(&format!("{path}/#dismiss"), 1)
    }

    /// How many PHYSICAL pixels one layout point covers — what the
    /// shell reads from the screen (`2.0` on a retina display). It
    /// reaches the app through [`crate::custom::PaintCtx::scale`], so
    /// a box that draws parts which TOUCH can put the shared edge on
    /// a whole pixel. The default is `1.0`.
    pub fn set_device_scale(&self, scale: Px) {
        self.device_scale.set(scale.max(1.0));
    }

    /// The screen's scale, as the shell last told it.
    pub fn device_scale(&self) -> Px {
        self.device_scale.get()
    }

    /// Closes every open popover, outermost last — the app-switch
    /// behavior (the desktop shell calls it when the window resigns
    /// key). `true` = something closed and the shell repaints.
    pub fn dismiss_all_overlays(&self) -> bool {
        let paths: Vec<String> = self
            .last_overlays
            .borrow()
            .iter()
            .rev()
            // a dialog is a WINDOW, and windows survive an app switch
            // (its own popovers still close) — without this skip the
            // dialog would dismiss itself on the very key-swap that
            // opens it
            .filter(|overlay| {
                matches!(overlay.surface, crate::layout::OverlaySurface::Layer)
            })
            .map(|overlay| overlay.path.clone())
            .collect();
        let mut closed = false;
        for path in paths {
            closed |= reconciler::run_action(&format!("{path}/#dismiss"), 1);
        }
        // the app switched away: the explanation and the menu go too
        closed | self.clear_tooltip() | self.close_menu()
    }

    /// One beat of the shell's slow clock (the caret-blink timer, a
    /// browser timeout). The delay is this tick seen TWICE over an
    /// unmoved hover: the first beat ages the wait, the second shows.
    /// `true` = the bubble appeared — repaint.
    pub fn tooltip_tick(&self) -> bool {
        let mut life = self.tooltip.borrow_mut();
        let Some(pending) = life.pending.clone() else {
            return false;
        };
        if !life.aged {
            life.aged = true;
            return false;
        }
        life.pending = None;
        life.aged = false;
        drop(life);
        self.interaction.borrow_mut().tooltip =
            Some((pending.text, pending.side, pending.rect));
        true
    }

    /// Is a tooltip waiting on the clock? The web glue asks after a
    /// pointer event to arm its timeout chain; the mac shell rides the
    /// blink timer and never asks.
    pub fn tooltip_waiting(&self) -> bool {
        self.tooltip.borrow().pending.is_some()
    }

    /// Drops the bubble and the wait. `true` = a bubble was showing —
    /// repaint.
    fn clear_tooltip(&self) -> bool {
        let mut life = self.tooltip.borrow_mut();
        life.pending = None;
        life.aged = false;
        drop(life);
        self.interaction.borrow_mut().tooltip.take().is_some()
    }

    /// A right press (a two-finger tap, a long press): the topmost
    /// `.context_menu(…)` region under it opens its items at the
    /// pointer. A press outside every region closes whatever is open.
    /// `true` = repaint.
    pub fn context_click(&self, x: Px, y: Px) -> bool {
        self.enter_scene();
        let was_open = self.close_menu();
        let cleared = self.clear_tooltip();
        let menus = self.last_menus.borrow();
        let region = self
            .reachable(&menus, |floor| floor.menus)
            .iter()
            .rev()
            .find(|region| region.rect.contains(x, y))
            .cloned();
        let Some(region) = region else {
            return was_open || cleared;
        };
        // The app asked to hear this one: it gets the point and answers it
        // however it likes, and the runtime opens nothing. Whichever region is
        // INNER wins the press, which is the same precedence a menu of items
        // has — the two doors are one gesture with two answers.
        if let Some(handler) = region.on_click {
            handler.0(Point { x, y });
            return true;
        }
        let entries: Vec<Option<std::sync::Arc<str>>> = region
            .items
            .iter()
            .map(|item| match item {
                crate::views::MenuItem::Action { label, .. } => Some(label.clone()),
                crate::views::MenuItem::Divider => None,
            })
            .collect();
        *self.menu_items.borrow_mut() = Some(region.items);
        self.interaction.borrow_mut().menu = Some(crate::layout::MenuOpen {
            at: Point { x, y },
            entries,
            hovered: None,
        });
        true
    }

    /// Closes the open menu without firing anything. `true` = one was
    /// open — repaint.
    fn close_menu(&self) -> bool {
        self.menu_items.borrow_mut().take();
        self.interaction.borrow_mut().menu.take().is_some()
    }

    /// How far a pressed pointer travels before the press becomes a
    /// lift — under it, a click stays a click.
    const DRAG_THRESHOLD: Px = 4.0;

    /// The room the run keeps after the caret when it scrolls to the
    /// right edge — the caret's own width, so the bar stays whole.
    const CARET_ROOM: Px = 2.0;

    /// The compatible drop region under a point — the INNERMOST of the
    /// topmost, by GEOMETRY: a drag lands through every opaque hover
    /// gate, which is the transparent catcher the dock wanted.
    ///
    /// The reverse walk is load-bearing twice. The regions are recorded
    /// outer before inner, so walking back answers with the innermost
    /// accepting target; and overlays are drained AFTER the root, so
    /// walking back is also the only reason a target inside a popover
    /// beats the one on the page underneath it.
    fn drop_at(&self, x: Px, y: Px, value: &dyn std::any::Any) -> Option<crate::layout::DropRegion> {
        let drops = self.last_drops.borrow();
        self.reachable(&drops, |floor| floor.drops)
            .iter()
            .rev()
            .find(|region| region.rect.contains(x, y) && region.accepts == value.type_id())
            .cloned()
    }

    /// Where a point sits inside a target's OWN box — never the
    /// visible slice, so a half-scrolled target keeps honest quadrants.
    fn drop_point(region: &crate::layout::DropRegion, x: Px, y: Px) -> crate::layout::DropPoint {
        crate::layout::DropPoint {
            local: Point { x: x - region.frame.origin.x, y: y - region.frame.origin.y },
            size: region.frame.size,
        }
    }

    /// Tells the box under the drag where the hand is, and the box the
    /// drag just LEFT that it is over — one enter, one leave, in that
    /// order, so an app that writes state from both never sees two
    /// live previews. `true` = a closure ran (the state it wrote will
    /// repaint on its own).
    fn note_drag_preview(&self, region: Option<&crate::layout::DropRegion>, x: Px, y: Px) -> bool {
        let entering = region.and_then(|region| {
            region.over.as_ref().map(|over| (region.rect, over.clone()))
        });
        let leaving = {
            let current = self.drag_preview.borrow();
            match (&*current, &entering) {
                // the same box, still under the hand: only the point moved
                (Some((rect, _)), Some((next, _))) if rect == next => None,
                (Some((_, over)), _) => Some(over.clone()),
                (None, _) => None,
            }
        };
        let mut ran = false;
        if let Some(over) = leaving {
            (over.0)(None);
            ran = true;
        }
        if let Some((rect, over)) = entering {
            let at = region.map(|region| Self::drop_point(region, x, y));
            (over.0)(at);
            *self.drag_preview.borrow_mut() = Some((rect, over));
            ran = true;
        } else {
            *self.drag_preview.borrow_mut() = None;
        }
        ran
    }

    /// The drag ends: whoever was previewing hears `None`, once.
    fn clear_drag_preview(&self) -> bool {
        let previous = self.drag_preview.borrow_mut().take();
        match previous {
            Some((_, over)) => {
                (over.0)(None);
                true
            }
            None => false,
        }
    }

    /// A live drag follows the pointer: the label chip moves, the
    /// compatible target under it rings, the scene's hover stays
    /// quiet. `Some(repaint)` when a drag owns the move.
    fn note_drag_move(&self, x: Px, y: Px) -> Option<bool> {
        // past the threshold, the armed press lifts
        let lift = {
            let armed = self.drag_armed.borrow();
            armed.as_ref().and_then(|(builder, pressed_at)| {
                let far = (x - pressed_at.x).hypot(y - pressed_at.y) >= Self::DRAG_THRESHOLD;
                (far && self.drag_value.borrow().is_none()).then(|| builder.clone())
            })
        };
        if let Some(builder) = lift {
            let payload = (builder.0)();
            let region = self.drop_at(x, y, &*payload.value);
            let over = region.as_ref().map(|region| region.rect);
            self.note_drag_preview(region.as_ref(), x, y);
            *self.drag_value.borrow_mut() = Some(payload.value);
            let mut interaction = self.interaction.borrow_mut();
            // the click dies at the lift: nothing fires on the release
            interaction.pressed = None;
            interaction.hovered = None;
            interaction.pointer = Some(Point { x, y });
            interaction.drag = Some(crate::layout::DragLive {
                label: payload.label,
                at: Point { x, y },
                over,
            });
            drop(interaction);
            let _ = self.clear_tooltip();
            return Some(true);
        }
        let value = self.drag_value.borrow().clone()?;
        let region = self.drop_at(x, y, &*value);
        let over = region.as_ref().map(|region| region.rect);
        // the app hears the place first: the state it writes and the
        // stamp below land in the SAME frame, never a step apart
        let previewed = self.note_drag_preview(region.as_ref(), x, y);
        let mut interaction = self.interaction.borrow_mut();
        let live = interaction.drag.as_mut()?;
        let moved = live.at != (Point { x, y }) || live.over != over;
        live.at = Point { x, y };
        live.over = over;
        interaction.pointer = Some(Point { x, y });
        Some(moved || previewed)
    }

    /// Did the last press land on a drag source? The web's element
    /// mode asks right after a press, and opens its pointer-move door
    /// ONLY when the answer is yes — a hover with no button down can
    /// never reach the engine there, which is how that mode keeps its
    /// zero-patch hover by construction instead of by policy.
    pub fn drag_armed(&self) -> bool {
        self.drag_armed.borrow().is_some() || self.drag_value.borrow().is_some()
    }

    /// Ends the drag without landing it. `true` = one was live.
    fn cancel_drag(&self) -> bool {
        self.drag_armed.borrow_mut().take();
        self.drag_value.borrow_mut().take();
        let previewed = self.clear_drag_preview();
        self.interaction.borrow_mut().drag.take().is_some() || previewed
    }

    /// The row under a point of the OPEN menu — the same walk the
    /// panel painted, shared in the layout module.
    fn menu_row_at(&self, x: Px, y: Px) -> Option<(Option<usize>, bool)> {
        let interaction = self.interaction.borrow();
        let open = interaction.menu.as_ref()?;
        let frame = self
            .last_overlays
            .borrow()
            .iter()
            .find(|overlay| overlay.path == crate::layout::MENU_PATH)
            .map(|overlay| overlay.frame)?;
        let inside = frame.contains(x, y);
        Some((crate::layout::menu_row_at(frame, &open.entries, x, y), inside))
    }

    /// The nearest interactive hit under the point that is NOT the
    /// box's own — the ancestor a RISING press arms. Only the TOPMOST
    /// entry of the box's path is skipped: a `.on_click` wrapped
    /// straight around the box shares its identity scope, and that
    /// wrapper is exactly the affordance the rise exists to reach.
    fn hit_above(&self, own: &str, x: Px, y: Px) -> Option<String> {
        let hits = self.last_hits.borrow();
        let mut skipped_own = false;
        for (path, rect) in self.reachable(&hits, |floor| floor.hits).iter().rev() {
            if !rect.contains(x, y) {
                continue;
            }
            if !skipped_own && path == own {
                skipped_own = true;
                continue;
            }
            return Some(path.clone());
        }
        None
    }

    /// The topmost tooltip region under the pointer — paint order, so
    /// the last one wins, mirroring the hits.
    fn tooltip_at(&self, x: Px, y: Px) -> Option<crate::layout::TooltipRegion> {
        let tooltips = self.last_tooltips.borrow();
        self.reachable(&tooltips, |floor| floor.tooltips)
            .iter()
            .rev()
            .find(|region| region.rect.contains(x, y))
            .cloned()
    }

    /// Follows the hover: a region under the pointer arms the wait, a
    /// bare stretch clears it, and the shown bubble lives exactly as
    /// long as the pointer stays inside ITS anchor. `true` = repaint.
    fn note_tooltip_hover(&self, x: Px, y: Px) -> bool {
        let region = self.tooltip_at(x, y);
        let mut interaction = self.interaction.borrow_mut();
        if let Some((_, _, anchor)) = &interaction.tooltip {
            match &region {
                Some(hovered) if hovered.rect == *anchor => return false,
                _ => {
                    interaction.tooltip = None;
                    let mut life = self.tooltip.borrow_mut();
                    life.pending = region.filter(|_| interaction.pressed.is_none());
                    life.aged = false;
                    return true;
                }
            }
        }
        let pressed = interaction.pressed.is_some();
        drop(interaction);
        let mut life = self.tooltip.borrow_mut();
        match region {
            Some(region) if !pressed => {
                let same = life
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.rect == region.rect);
                if !same {
                    life.pending = Some(region);
                    life.aged = false;
                }
            }
            _ => {
                life.pending = None;
                life.aged = false;
            }
        }
        false
    }

    /// Should a press at this point drag the WINDOW? True inside a
    /// `.window_drag_region()` where no interactive target wins — a
    /// button on the scene's own title bar still clicks.
    pub fn window_drag_at(&self, x: Px, y: Px) -> bool {
        let blocked = {
            let hits = self.last_hits.borrow();
            crate::layout::hit_test(self.reachable(&hits, |floor| floor.hits), x, y).is_some()
        };
        if blocked {
            return false;
        }
        self.last_drag_regions.borrow().iter().any(|region| region.contains(x, y))
    }

    /// Which of the window's own buttons sits at this point, topmost
    /// first. Unlike the drag handle, the control WINS by design: it
    /// IS the button, and the platform (not the scene) activates it.
    pub fn window_control_at(&self, x: Px, y: Px) -> Option<crate::layout::WindowControl> {
        self.last_control_regions
            .borrow()
            .iter()
            .rev()
            .find(|(_, region)| region.contains(x, y))
            .map(|(control, _)| *control)
    }

    fn with_parts(ctx: Context, text: Rc<dyn TextEngine>) -> Self {
        // a runtime is born into its OWN world: whatever a previous
        // runtime retained on this thread dies here — path identity
        // means nothing across two runtimes, and stale reads pointing
        // at old state slots would kill invalidation silently. App
        // state declared outside any pass has no owner and survives.
        crate::reconciler::reset_world();
        crate::effects::reset();
        crate::viewport::reset();
        motor::identity::reset_world();
        Self::assembled(None, ctx, text)
    }

    /// [`Self::with_parts`] for a named scene: the same fresh world, cut
    /// to this scene's own subtree so the thread's other windows keep
    /// theirs.
    fn scene_with_parts(name: Rc<str>, ctx: Context, text: Rc<dyn TextEngine>) -> Self {
        crate::reconciler::forget_under(&name);
        motor::identity::reset_scene(&name);
        Self::assembled(Some(name), ctx, text)
    }

    fn assembled(scene: Option<Rc<str>>, ctx: Context, text: Rc<dyn TextEngine>) -> Self {
        let runtime = Runtime {
            ctx,
            scene,
            last_root: RefCell::new(None),
            last_hits: RefCell::new(Vec::new()),
            hosted_handlers: RefCell::new(HashMap::default()),
            interaction: RefCell::new(Interaction::default()),
            pointer_modifiers: std::cell::Cell::new(crate::action::Modifiers::NONE),
            text,
            images: Rc::new(RawImages::default()),
            cache: MeasureCache::default(),
            scroll_offsets: RefCell::new(HashMap::default()),
            dom_viewports: RefCell::new(HashMap::default()),
            island_boxes: RefCell::new(HashMap::default()),
            dom_customs: RefCell::new(Vec::new()),
            last_scrolls: RefCell::new(Vec::new()),
            last_modal_floor: std::cell::Cell::new(None),
            focus: RefCell::new(None),
            carets: RefCell::new(HashMap::default()),
            caret_visible: Cell::new(true),
            goal_column: Cell::new(None),
            last_fields: RefCell::new(Vec::new()),
            last_splits: RefCell::new(Vec::new()),
            last_customs: RefCell::new(Vec::new()),
            last_hosts: RefCell::new(Vec::new()),
            webview_evals: RefCell::new(HashMap::default()),
            webview_snaps: RefCell::new(HashMap::default()),
            webview_eval_next: Cell::new(0),
            live_ledger: RefCell::new(motor::hash::FxHashMap::default()),
            theme_version: Cell::new(crate::theme::version()),
            keymap: RefCell::new(HashMap::default()),
            scoped_keymap: RefCell::new(HashMap::default()),
            chords: RefCell::new(Vec::new()),
            pending: RefCell::new(Vec::new()),
            chord_sink: RefCell::new(None),
            chord_announced: Cell::new(false),
            pending_aged: Cell::new(false),
            measures: RefCell::new(HashMap::default()),
            scroll_commands: RefCell::new(HashMap::default()),
            scroll_targets: RefCell::new(HashMap::default()),
            element_reveals: RefCell::new(HashMap::default()),
            auto_focused: RefCell::new(std::collections::HashSet::default()),
            animator: RefCell::new(crate::anim::Animator::default()),
            last_proposal: Cell::new(None),
            last_overlays: RefCell::new(Vec::new()),
            last_tooltips: RefCell::new(Vec::new()),
            last_menus: RefCell::new(Vec::new()),
            menu_items: RefCell::new(None),
            last_drag_sources: RefCell::new(Vec::new()),
            last_drops: RefCell::new(Vec::new()),
            last_drop_rings: RefCell::new(Vec::new()),
            drag_armed: RefCell::new(None),
            pressed_clicks: Cell::new(1),
            drag_value: RefCell::new(None),
            drag_preview: RefCell::new(None),
            tooltip: RefCell::new(TooltipLife::default()),
            last_drag_regions: RefCell::new(Vec::new()),
            last_control_regions: RefCell::new(Vec::new()),
            overlay_bounds: Cell::new(None),
            dialog_frames: RefCell::new(HashMap::default()),
            device_scale: Cell::new(1.0),
            dom: RefCell::new(crate::dom::DomLowering::default()),
            root_is_boundary: Cell::new(false),
            printless: Cell::new(false),
        };
        // Escape closes the innermost popover — pre-bound in the
        // reserved context so apps never wire it (and never lose it)
        runtime
            .scoped_keymap
            .borrow_mut()
            .entry(OVERLAY_CONTEXT)
            .or_default()
            .insert(KeyPattern::key(crate::action::Key::Escape), OVERLAY_DISMISS);
        runtime
    }

    pub fn context(&self) -> Context {
        self.ctx.clone()
    }

    /// One incremental pass: walk with skips, isolated re-runs of dirty
    /// views the walk missed, effect-queue reassembly, and the sweep.
    /// Returns both outputs (print and layout) still holding references.
    fn render_pass(&self, root: &impl View) -> NodeList {
        // virtualized bodies read LAST frame's region geometry (offset
        // taken NOW — a wheel that just moved it must reach the window
        // math) — published fresh before every pass
        {
            let offsets = self.scroll_offsets.borrow();
            let applied = self.scroll_targets.borrow();
            let last_scrolls = self.last_scrolls.borrow();
            if last_scrolls.is_empty() {
                // the FLOW frame never lays out, so no measured region
                // exists — the browser's own reports stand in: offset
                // from the scroll events, viewport from the observer,
                // extents from the app (the window math's authority)
                let viewports = self.dom_viewports.borrow();
                crate::viewport::publish(offsets.keys().map(|path| {
                    (
                        path.clone(),
                        crate::viewport::RegionSnapshot {
                            offset_y: offsets.get(path).copied().unwrap_or_default().y,
                            viewport: viewports
                                .get(path)
                                .map(|box_| box_.1)
                                .unwrap_or(self.last_proposal.get().and_then(|p| p.height).unwrap_or(600.0)),
                            row_extent: 0.0,
                            offsets: None,
                            applied: applied.get(path).cloned(),
                        },
                    )
                }));
            } else {
                crate::viewport::publish(last_scrolls.iter().filter_map(|region| {
                    let row_extent = region.row_extent?;
                    Some((
                        region.path.clone(),
                        crate::viewport::RegionSnapshot {
                            offset_y: offsets
                                .get(&region.path)
                                .copied()
                                .unwrap_or_default()
                                .y,
                            viewport: region.frame.size.height,
                            row_extent,
                            offsets: region.row_offsets.clone(),
                            applied: applied.get(&region.path).cloned(),
                        },
                    ))
                }));
            }
        }
        // new theme = stale retention (bodies baked old tokens into
        // the scene): rebuild once and continue incremental
        let theme_version = crate::theme::version();
        if self.theme_version.get() != theme_version {
            self.theme_version.set(theme_version);
            reconciler::clear();
        }
        effects::reset();
        let snapshot = motor::identity::dirty_snapshot();
        reconciler::begin_pass(snapshot.clone());
        motor::identity::begin_pass();

        let mut nodes = NodeList::new();
        {
            // the scene's own segment goes down FIRST, so it is the root
            // the sweep, the dirty drain and the retention all scope by
            let _scene = self.scene.as_ref().map(|name| motor::identity::enter(&**name));
            root.render_into(&self.ctx, &mut nodes);
        }

        let pass_root = motor::identity::current_pass_root();
        if let Some(pass_root) = &pass_root {
            reconciler::run_isolated(pass_root);
        }

        let dead = motor::identity::end_pass();
        reconciler::forget(&dead);
        if let Some(pass_root) = &pass_root {
            // the twin of the sweep above, for views with no state of their own
            reconciler::sweep_stale(pass_root);
        }

        if let Some(pass_root) = &pass_root {
            self.assemble_scene(pass_root);
            // with the editors of THIS pass assembled, dead fields
            // release their carets, auto-focus memory and the focus
            self.release_dead_input();
            motor::identity::consume_dirty(pass_root, &snapshot);
            *self.last_root.borrow_mut() = Some(pass_root.clone());
        }
        reconciler::end_pass();
        nodes
    }

    /// Fires the interactive target's action (the key comes from the
    /// hit-test over `LayoutResult::hits`). `false` = target not
    /// registered.
    pub fn activate(&self, path: &str) -> bool {
        self.activate_clicks(path, 1)
    }

    /// The same fire, carrying the platform's click count — what a
    /// press that arrived through [`Runtime::pointer_clicked`] hands
    /// the app.
    pub fn activate_clicks(&self, path: &str, clicks: u8) -> bool {
        self.enter_scene();
        reconciler::run_action(path, clicks)
    }

    // MARK: - Pointer (resolved BEFORE layout — the LAW)

    /// Runs a pointer road and then tells the views the pointer left
    /// one and arrived at another.
    ///
    /// EVERY road that can move the hover comes through here, instead
    /// of each of the seven places that assign it — a law spread over
    /// seven sites is a law with six chances to be forgotten, and the
    /// last one that happened cost a whole round.
    ///
    /// It fires OUTSIDE every borrow this file holds: an app's closure
    /// may come straight back in through the front door, and a hover
    /// that fired mid-borrow would meet a runtime busy with itself.
    fn watching_hover<T>(&self, road: impl FnOnce() -> T) -> (T, bool) {
        // nobody asked: the road runs and not one path is copied
        if !reconciler::hover_watched() {
            return (road(), false);
        }
        let before = self.interaction.borrow().hovered.clone();
        let out = road();
        let after = self.interaction.borrow().hovered.clone();
        if before == after {
            return (out, false);
        }
        let told = |path: Option<&str>, inside: u8| match path {
            Some(path) => reconciler::run_action(
                &format!("{path}/{}", reconciler::HOVER_KEY),
                inside,
            ),
            None => false,
        };
        // left first, then arrived: a view that hands its state to the
        // next one must not be told it is gone AFTER the next one was
        // told it is here
        let left = told(before.as_deref(), 0);
        let arrived = told(after.as_deref(), 1);
        (out, left || arrived)
    }

    /// The target under the point, against the last layout's hits.
    fn hover_target(&self, x: Px, y: Px) -> Option<String> {
        let hits = self.last_hits.borrow();
        crate::layout::hit_test(self.reachable(&hits, |floor| floor.hits), x, y).map(str::to_string)
    }

    /// The tail of a placed list the pointer may still reach: a modal
    /// layer draws ONE line across every list, and nothing recorded
    /// under it answers — not where the layer paints and not beside
    /// it either, because a modal owns what it covers whole. `mark` is
    /// that layer's own count on THIS list.
    ///
    /// One line across ALL of them: a layer that eats the wheel but
    /// not the right press is a modal with holes, and the holes are
    /// where the bugs live.
    ///
    /// The line is drawn HERE and not inside `hit_test`, because the
    /// window's own buttons ask that function directly, and a modal
    /// must not swallow the traffic lights of the window it sits in.
    fn reachable<'a, T>(
        &self,
        items: &'a [T],
        mark: fn(&crate::layout::ModalFloor) -> usize,
    ) -> &'a [T] {
        let floor = self.last_modal_floor.get().map_or(0, |floor| mark(&floor));
        &items[floor.min(items.len())..]
    }

    /// Pointer moved. `true` = the visible state changed (the shell
    /// repaints). During a press, hover only re-resolves against the
    /// pressed target: dragging out drops the visual, coming back
    /// re-arms it (AppKit).
    ///
    /// The move carries what the hand is HOLDING, the same as the
    /// press. The framework spends none of it: a box under the pointer
    /// is the only one that knows whether a held command makes its
    /// content a door, and it wants to say so BEFORE the press.
    ///
    /// The runtime remembers them, so a hover that re-resolves after a
    /// layout — content sliding under a still hand — replays the move
    /// the way it really was.
    pub fn pointer_moved(&self, x: Px, y: Px, modifiers: impl Into<crate::action::Modifiers>) -> bool {
        self.enter_scene();
        let modifiers = modifiers.into();
        self.pointer_modifiers.set(modifiers);
        let (repaint, told) = self.watching_hover(|| self.pointer_moved_road(x, y, modifiers));
        repaint || told
    }

    fn pointer_moved_road(&self, x: Px, y: Px, modifiers: crate::action::Modifiers) -> bool {
        // a live divider drag owns the pointer: the move becomes a lane
        // extent, the retained writer reaches the binding, and the app's
        // state change re-lays the frame — hover stays untouched
        let dragging = self.interaction.borrow().split_drag.clone();
        if let Some(path) = dragging {
            self.interaction.borrow_mut().pointer = Some(Point { x, y });
            return self.drag_split(&path, x, y);
        }
        // a thumb under the hand owns the pointer the same way
        let thumb = self.interaction.borrow().thumb_drag.clone();
        if let Some(thumb) = thumb {
            self.interaction.borrow_mut().pointer = Some(Point { x, y });
            return self.drag_thumb(&thumb, x, y);
        }
        // a field under a sweeping hand owns the pointer the same way:
        // the anchor stays where the press dropped it and the caret
        // follows the x. Past the border the caret lands on whatever
        // that x names in TEXT space, so the run rolls with the hand —
        // a hand held perfectly still does not roll on, because no
        // clock lives in here
        let swept = self.interaction.borrow().field_drag.clone();
        if let Some(path) = swept {
            self.interaction.borrow_mut().pointer = Some(Point { x, y });
            return self.sweep_to(&path, x, y);
        }
        // a box that took the press owns every move until the release —
        // dragging a selection past the frame is one gesture, not two
        let grabbed = self.interaction.borrow().element_grab.clone();
        if let Some(path) = grabbed {
            self.interaction.borrow_mut().pointer = Some(Point { x, y });
            if let Some(placement) = self.custom_at(&path) {
                let at = Self::local(&placement, x, y);
                let event =
                    crate::custom::ElementEvent::PointerMoved { at, pressed: true, modifiers };
                return self.deliver(&placement, event).handled;
            }
            // the box left the scene mid-drag: the gesture ends with it
            self.interaction.borrow_mut().element_grab = None;
            return false;
        }
        // a live (or lifting) drag owns the move whole
        if let Some(repaint) = self.note_drag_move(x, y) {
            return repaint;
        }
        // an open menu sits above the scene: a move inside it moves
        // the row highlight and nothing underneath hears a thing
        if let Some(repaint) = self.note_menu_hover(x, y) {
            return repaint;
        }
        let target = self.hover_target(x, y);
        let mut interaction = self.interaction.borrow_mut();
        let hovered = match &interaction.pressed {
            Some(pressed) => target.filter(|candidate| candidate == pressed),
            None => target,
        };
        let changed = interaction.hovered != hovered;
        interaction.pointer = Some(Point { x, y });
        interaction.hovered = hovered.clone();
        drop(interaction);
        // a free move over a box still reaches it: a cursor over code
        // wants the column under it
        let over = hovered.as_deref().and_then(|path| self.custom_at(path));
        let used = match over {
            Some(placement) => {
                let at = Self::local(&placement, x, y);
                let event =
                    crate::custom::ElementEvent::PointerMoved { at, pressed: false, modifiers };
                self.deliver(&placement, event).handled
            }
            None => false,
        };
        // the tooltip's hover walks beside the interactive one and
        // never touches it — a region explains, it does not intercept
        let explained = self.note_tooltip_hover(x, y);
        changed || used || explained
    }

    /// The pointer over an OPEN menu: the row under it highlights and
    /// the scene's own hover goes quiet — the panel is above it all.
    /// `Some(repaint)` when the menu swallowed the move.
    fn note_menu_hover(&self, x: Px, y: Px) -> Option<bool> {
        let (row, inside) = self.menu_row_at(x, y)?;
        let mut interaction = self.interaction.borrow_mut();
        let open = interaction.menu.as_mut()?;
        if !inside {
            // outside the panel the scene hovers as ever (macOS keeps
            // the menu up until a press) — only the row quiets
            let changed = open.hovered.take().is_some();
            return if changed { Some(true) } else { None };
        }
        let changed = open.hovered != row;
        open.hovered = row;
        let hovered_scene = interaction.hovered.take().is_some();
        interaction.pointer = Some(Point { x, y });
        Some(changed || hovered_scene)
    }

    /// One divider move: clamp the pointer into the split's lane range
    /// and hand it to the retained writer. `true` = the position writer
    /// ran (the state write re-renders; a clamped no-move still repaints
    /// cheaply — the frame is stable and the pass settles at zero).
    fn drag_split(&self, path: &str, x: Px, y: Px) -> bool {
        let placement = self
            .last_splits
            .borrow()
            .iter()
            .find(|split| split.path == path)
            .cloned();
        let Some(split) = placement else {
            return false;
        };
        let (pointer_main, origin_main) = match split.axis {
            crate::layout::Axis::Horizontal => (x, split.frame.origin.x),
            crate::layout::Axis::Vertical => (y, split.frame.origin.y),
        };
        // the pointer names lane A's extent in POINTS; what the binding
        // holds is whatever unit the seam speaks, so the clamp runs in
        // that unit and the write-back is already in it
        //
        // …unless the seam names the TRAILING lane, in which case the
        // pointer still lands where it lands and the app is holding the
        // OTHER side of it: the reach is mirrored across the room, and
        // the floors swap with it.
        let reached = pointer_main - origin_main;
        let (reached, near, far) = if split.trailing {
            ((split.room - reached).max(0.0), split.min_b, split.min_a)
        } else {
            (reached, split.min_a, split.min_b)
        };
        let at = match split.unit {
            crate::layout::SeamUnit::Points => {
                reached.clamp(near, (split.room - far).max(near))
            }
            crate::layout::SeamUnit::Fraction => {
                if split.room <= 0.0 {
                    return false;
                }
                (reached / split.room).clamp(near, (1.0 - far).max(near))
            }
        };
        reconciler::run_split(path, at)
    }

    /// The thumb's geometry, in the axis it travels: `(track start,
    /// track length, thumb length, travel, max offset)`. The mirror of
    /// `draw_scrollbar` — one formula, written twice on purpose would
    /// be a bug waiting, so this reads the SAME constants.
    fn thumb_geometry(
        region: &crate::layout::ScrollRegion,
        horizontal: bool,
    ) -> Option<(Px, Px, Px, Px)> {
        let (extent, content) = match horizontal {
            true => (region.frame.size.width, region.content.width),
            false => (region.frame.size.height, region.content.height),
        };
        let max = (content.round() - extent.round()).max(0.0);
        if max <= 0.0 {
            return None;
        }
        let track = extent - 2.0 * crate::layout::SCROLLBAR_INSET;
        if track <= 0.0 {
            return None;
        }
        let thumb = ((extent / content) * track)
            .max(crate::layout::SCROLLBAR_MIN)
            .min(track);
        let start = match horizontal {
            true => region.frame.origin.x,
            false => region.frame.origin.y,
        } + crate::layout::SCROLLBAR_INSET;
        Some((start, thumb, (track - thumb).max(0.0), max))
    }

    fn region_at(&self, path: &str) -> Option<crate::layout::ScrollRegion> {
        self.last_scrolls.borrow().iter().find(|region| region.path == path).cloned()
    }

    /// A press on a thumb's grab band: which region, which axis, and
    /// how far into the thumb the pointer landed.
    fn grab_thumb(&self, target: &str, x: Px, y: Px) -> Option<crate::layout::ThumbDrag> {
        let (path, horizontal) = match target.strip_suffix("/#thumb-v") {
            Some(path) => (path, false),
            None => (target.strip_suffix("/#thumb-h")?, true),
        };
        let region = self.region_at(path)?;
        let (start, thumb, travel, max) = Self::thumb_geometry(&region, horizontal)?;
        let offset = self.scroll_offset(path);
        let along = match horizontal {
            true => offset.x,
            false => offset.y,
        };
        let head = start + travel * (along / max);
        let pointer = if horizontal { x } else { y };
        Some(crate::layout::ThumbDrag {
            path: path.to_string(),
            horizontal,
            grab: (pointer - head).clamp(0.0, thumb),
        })
    }

    /// The thumb travels with the hand: the band's head follows the
    /// pointer minus where it was grabbed, and the region's offset is
    /// that head read back through the track.
    fn drag_thumb(&self, drag: &crate::layout::ThumbDrag, x: Px, y: Px) -> bool {
        let Some(region) = self.region_at(&drag.path) else {
            return false;
        };
        let Some((start, _, travel, max)) = Self::thumb_geometry(&region, drag.horizontal)
        else {
            return false;
        };
        if travel <= 0.0 {
            return false;
        }
        let pointer = if drag.horizontal { x } else { y };
        let along = (((pointer - drag.grab) - start) / travel * max).clamp(0.0, max);
        let current = self.scroll_offset(&drag.path);
        let next = match drag.horizontal {
            true => Point { x: along, y: current.y },
            false => Point { x: current.x, y: along },
        };
        if next == current {
            return false;
        }
        // the wheel's own door: an offset written by hand cancels a
        // reveal in flight, or the spring would fight the hand
        self.animator.borrow_mut().cancel_scroll(&drag.path);
        self.set_scroll_offset(&drag.path, next);
        true
    }

    // MARK: - The app's own boxes (the escape hatch)

    /// The app's box registered at `path` in the last layout — the
    /// pixel pass's ledger first, then the flow's (island-local
    /// frames; keys and text carry no point, and the box's own world
    /// is exactly what the ctx should say).
    fn custom_at(&self, path: &str) -> Option<crate::layout::CustomPlacement> {
        self.last_customs
            .borrow()
            .iter()
            .find(|placement| placement.path == path)
            .cloned()
            .or_else(|| {
                self.dom_customs
                    .borrow()
                    .iter()
                    .find(|(_, placement)| placement.path == path)
                    .map(|(_, placement)| placement.clone())
            })
    }

    /// Hands one event to the app's box — the point arrives in the
    /// box's OWN coordinates, and the answer says whether the scene
    /// still gets a turn.
    fn deliver(
        &self,
        placement: &crate::layout::CustomPlacement,
        event: crate::custom::ElementEvent,
    ) -> crate::custom::Response {
        let ctx = crate::custom::EventCtx {
            frame: placement.frame,
            visible: placement.visible,
            metrics: crate::custom::Metrics::new(&*self.text, &self.cache, placement.font),
        };
        placement.element.element().event(&event, &ctx)
    }

    /// A rect of the box's own coordinates, in the scene's.
    fn to_layout(placement: &crate::layout::CustomPlacement, rect: Rect) -> Rect {
        Rect {
            origin: Point {
                x: rect.origin.x + placement.frame.origin.x,
                y: rect.origin.y + placement.frame.origin.y,
            },
            size: rect.size,
        }
    }

    /// A point in the box's own coordinates.
    fn local(placement: &crate::layout::CustomPlacement, x: Px, y: Px) -> Point {
        Point { x: x - placement.frame.origin.x, y: y - placement.frame.origin.y }
    }

    /// The app's box that holds the keyboard, if the focus is on one.
    fn focused_custom(&self) -> Option<crate::layout::CustomPlacement> {
        let path = self.focus.borrow().clone()?;
        self.custom_at(&path)
    }

    /// Hands the keyboard to the app's box (no caret state: the caret
    /// belongs to the app).
    fn focus_element(&self, path: &str) {
        if self.focus.borrow().as_deref() == Some(path) {
            return;
        }
        self.blur();
        self.caret_visible.set(true);
        *self.focus.borrow_mut() = Some(path.to_string());
        if let Some(placement) = self.custom_at(path) {
            self.deliver(&placement, crate::custom::ElementEvent::Focused(true));
            self.dirty_island_of(&placement.path);
        }
    }

    /// One keystroke offered to whoever holds the keyboard BEFORE the
    /// keymap: an editor owns its arrows, its Enter and its Tab while
    /// it has focus, and a field of many lines owns the `⌘↵` its
    /// `.on_submit` asked for. The answer's `text` is what a copy
    /// hands the platform's clipboard; `handled: false` sends the
    /// stroke on to the app's bindings.
    pub fn key_stroke(
        &self,
        stroke: impl Into<crate::action::Stroke>,
    ) -> crate::custom::Response {
        self.enter_scene();
        let stroke = stroke.into();
        let pattern = &stroke.pattern;
        // an open menu takes Escape before anyone — its owner is the
        // runtime, so no reconciler handler could; a live drag is next
        if *pattern == KeyPattern::key(crate::action::Key::Escape)
            && (self.close_menu() || self.cancel_drag())
        {
            return crate::custom::Response::handled();
        }
        // a field of MANY lines owns `⌘↵`: the bare break is its
        // newline, so its submit has to be the chord — and the app
        // only loses that chord where the field named a handler
        if *pattern == KeyPattern::command(crate::action::Key::Enter) {
            let focused = self.focus.borrow().clone();
            if let Some(path) = focused
                && self.field_at(&path).is_some_and(|field| field.multiline)
                && self.submit(&path).applied
            {
                return crate::custom::Response::handled();
            }
        }
        let Some(placement) = self.focused_custom() else {
            return crate::custom::Response::ignored();
        };
        let response = self.deliver(&placement, crate::custom::ElementEvent::Key(stroke));
        if response.handled {
            self.caret_visible.set(true);
            self.dirty_island_of(&placement.path);
        }
        response
    }

    /// Button down: ARMS pressed on the target under the point — no
    /// action fires here (up-inside is button semantics). `true` =
    /// repaint.
    pub fn pointer_pressed(&self, x: Px, y: Px) -> bool {
        self.enter_scene();
        self.pointer_clicked(x, y, 1, false)
    }

    /// [`Self::pointer_pressed`] with the platform's click count — the
    /// shells pass it through and the runtime never needs a clock.
    /// AppKit counts (`clickCount`); Win32 hands a message kind and its
    /// shell counts; the browser counts on `mousedown` and NOT on
    /// `pointerdown`, which reports `detail` zero, so the web shell
    /// counts too. A box hears the number in `PointerDown`, and a view
    /// through `.on_click_count`.
    ///
    /// The press carries what the hand was HOLDING, and all of it. The
    /// framework itself only ever reads the shift — over a field it
    /// extends the selection instead of replacing it — but the rest is
    /// not the framework's to spend: command and a click is a jump to a
    /// definition in one box and nothing in the next, and only the box
    /// under the pointer knows which.
    pub fn pointer_clicked(
        &self,
        x: Px,
        y: Px,
        clicks: u8,
        modifiers: impl Into<crate::action::Modifiers>,
    ) -> bool {
        self.enter_scene();
        let modifiers = modifiers.into();
        let (repaint, told) = self.watching_hover(|| self.pointer_clicked_road(x, y, clicks, modifiers));
        repaint || told
    }

    fn pointer_clicked_road(
        &self,
        x: Px,
        y: Px,
        clicks: u8,
        modifiers: crate::action::Modifiers,
    ) -> bool {
        let shift = modifiers.shift;
        // the hand left the keyboard: a sequence in the air goes with
        // it, the way it does in every editor that has chords
        self.cancel_chord();
        // an open menu owns the press whole: a row fires ON THE DOWN
        // (menu semantics, not button semantics) and a press outside
        // closes and consumes — AppKit's own manners
        if self.interaction.borrow().menu.is_some() {
            let picked = self.menu_row_at(x, y).and_then(|(row, _)| row);
            let action = picked.and_then(|index| {
                self.menu_items.borrow().as_ref().and_then(|items| {
                    match items.get(index) {
                        Some(crate::views::MenuItem::Action { action, .. }) => {
                            Some(action.clone())
                        }
                        _ => None,
                    }
                })
            });
            self.close_menu();
            if let Some(action) = action {
                // outside the borrows: the action writes state and the
                // next layout sees a world without the menu
                action();
            }
            return true;
        }
        // a press on a drag source ARMS the lift — and the press goes
        // on: a click that never moves stays a click
        let sources = self.last_drag_sources.borrow();
        let source = self
            .reachable(&sources, |floor| floor.drag_sources)
            .iter()
            .rev()
            .find(|region| region.rect.contains(x, y))
            .map(|region| region.payload.clone());
        *self.drag_armed.borrow_mut() =
            source.map(|payload| (payload, Point { x, y }));
        // a press ends any explanation, and never the other way round:
        // the tooltip is not a popover — it cannot eat a click
        let explained = self.clear_tooltip();
        // an open popover eats the press outside its frame: the
        // TOPMOST one closes and nothing underneath arms (the press
        // is consumed — AppKit semantics, no accidental activation)
        let outside = {
            let overlays = self.last_overlays.borrow();
            overlays
                .iter()
                .rev()
                .find(|top| {
                    top.path != crate::layout::TOOLTIP_PATH
                        && top.path != crate::layout::DRAG_LABEL_PATH
                        // a dialog WINDOW never dismisses from outside:
                        // outside is the inert parent, and a press
                        // there answers to the modal floor (nothing)
                        && matches!(top.surface, crate::layout::OverlaySurface::Layer)
                })
                .filter(|top| !top.frame.contains(x, y))
                .map(|top| top.path.clone())
        };
        if let Some(path) = outside {
            reconciler::run_action(&format!("{path}/#dismiss"), 1);
            return true;
        }
        // ONE write, above every branch that can arm a target — the
        // risen press of a box, the thumb, the seam and the ordinary
        // tail all take their count from here, and the release takes it
        self.pressed_clicks.set(clicks);
        let target = self.hover_target(x, y);
        // a press inside the app's box hands it the pointer: nothing
        // arms by default (a box has no up-inside action to mis-fire)
        // and the moves keep coming until the release — unless the box
        // answers that the gesture RISES, and then the nearest
        // interactive ancestor arms too, exactly as if the box were
        // glass for that one purpose
        if let Some(placement) = target.as_deref().and_then(|path| self.custom_at(path)) {
            {
                let mut interaction = self.interaction.borrow_mut();
                interaction.pointer = Some(Point { x, y });
                interaction.hovered = Some(placement.path.clone());
                interaction.pressed = None;
                interaction.element_grab = Some(placement.path.clone());
            }
            let at = Self::local(&placement, x, y);
            let response = self.deliver(
                &placement,
                crate::custom::ElementEvent::PointerDown { at, clicks, modifiers },
            );
            if response.rises {
                let above = self.hit_above(&placement.path, x, y);
                self.interaction.borrow_mut().pressed = above;
            }
            return true;
        }
        // a press on a thumb takes the pointer until the release: the
        // region travels with the hand, and nothing arms (a thumb has
        // no up-inside action to mis-fire)
        if let Some(drag) = target.as_deref().and_then(|t| self.grab_thumb(t, x, y)) {
            let mut interaction = self.interaction.borrow_mut();
            interaction.pointer = Some(Point { x, y });
            interaction.thumb_drag = Some(drag);
            interaction.pressed = None;
            return true;
        }
        // a press on a grip band starts the divider drag — nothing arms
        // (a divider has no up-inside action to mis-fire)
        if let Some(grip) = target.as_deref().and_then(|t| t.strip_suffix("/#split")) {
            let mut interaction = self.interaction.borrow_mut();
            interaction.pointer = Some(Point { x, y });
            interaction.split_drag = Some(grip.to_string());
            interaction.pressed = None;
            return true;
        }
        // a press on a field takes the keyboard AND opens a selection.
        // The field is the one target that acts on the DOWN: first
        // responder follows the press everywhere, and a sweep cannot
        // begin on an up. Nothing arms — a field has no up-inside
        // action to mis-fire — and every move comes here until the
        // release, so a sweep that leaves the box stays one gesture
        if let Some(path) = target.as_deref().filter(|path| reconciler::has_editor(path)) {
            let path = path.to_string();
            self.select_at(&path, x, y, clicks, shift);
            let mut interaction = self.interaction.borrow_mut();
            interaction.pointer = Some(Point { x, y });
            interaction.hovered = Some(path.clone());
            interaction.pressed = None;
            interaction.field_drag = Some(path);
            return true;
        }
        let mut interaction = self.interaction.borrow_mut();
        interaction.pointer = Some(Point { x, y });
        let changed = interaction.pressed != target || interaction.hovered != target;
        interaction.hovered = target.clone();
        interaction.pressed = target;
        changed || explained
    }

    /// Button up: fires the action IF released inside the pressed
    /// target — or FOCUSES, if the target is a text field. Releasing
    /// outside any field drops focus (first responder follows the
    /// click). Returns the fired/focused path; the pressed visual
    /// always clears.
    pub fn pointer_released(&self, x: Px, y: Px) -> Option<String> {
        self.enter_scene();
        self.watching_hover(|| self.pointer_released_road(x, y)).0
    }

    fn pointer_released_road(&self, x: Px, y: Px) -> Option<String> {
        // taken at the TOP so it cannot leak: every release resets it,
        // including the ones that end a drag, a grab, a seam or a thumb
        // and fire no action at all, so the next gesture starts at one
        let clicks = self.pressed_clicks.replace(1);
        // a live drag ends here: over a compatible target the value
        // lands (the drag clears FIRST — the action writes state into
        // a world without it); anywhere else it just goes home
        self.drag_armed.borrow_mut().take();
        if let Some(value) = self.drag_value.borrow_mut().take() {
            let target = self.drop_at(x, y, &*value);
            self.interaction.borrow_mut().drag = None;
            // the preview closes FIRST — the landing action writes its
            // state into a world with no drag and no leftover preview
            self.clear_drag_preview();
            if let Some(region) = target {
                let at = Self::drop_point(&region, x, y);
                (region.action.0)(&*value, at);
            }
            return None;
        }
        // the box that owns the pointer hears the release and the
        // gesture ends there: no action fires under it
        let grabbed = self.interaction.borrow().element_grab.clone();
        if let Some(path) = grabbed {
            let risen = {
                let mut interaction = self.interaction.borrow_mut();
                interaction.element_grab = None;
                interaction.pointer = Some(Point { x, y });
                interaction.pressed.take()
            };
            if let Some(placement) = self.custom_at(&path) {
                let at = Self::local(&placement, x, y);
                self.deliver(&placement, crate::custom::ElementEvent::PointerUp { at });
                // a box that takes the keyboard takes it on the click,
                // the way a field does — the caret is the app's, so
                // nothing here measures a column
                if placement.element.element().accepts_keys() {
                    self.focus_element(&placement.path);
                } else {
                    self.blur();
                }
            }
            // the RISEN press fires like any button: released inside
            // the same target — the pane focuses while the box keeps
            // its caret, one click doing both jobs
            if let Some(above) = risen {
                let inside = self
                    .last_hits
                    .borrow()
                    .iter()
                    .any(|(path, rect)| *path == above && rect.contains(x, y));
                if inside && self.activate_clicks(&above, clicks) {
                    return Some(above);
                }
            }
            return None;
        }
        // a field's sweep ends with the button and what it selected
        // STAYS — focus and caret both moved on the press, so there is
        // nothing left for the release to decide
        let swept = {
            let mut interaction = self.interaction.borrow_mut();
            let swept = interaction.field_drag.take();
            if swept.is_some() {
                interaction.pointer = Some(Point { x, y });
            }
            swept
        };
        if let Some(path) = swept {
            return Some(path);
        }
        // a divider or a thumb ends its drag on release — no action
        // fires, no focus moves
        if self.interaction.borrow().split_drag.is_some()
            || self.interaction.borrow().thumb_drag.is_some()
        {
            let mut interaction = self.interaction.borrow_mut();
            interaction.split_drag = None;
            interaction.thumb_drag = None;
            interaction.pointer = Some(Point { x, y });
            return None;
        }
        let target = self.hover_target(x, y);
        let fired = {
            let mut interaction = self.interaction.borrow_mut();
            let pressed = interaction.pressed.take();
            interaction.pointer = Some(Point { x, y });
            interaction.hovered = target.clone();
            match (pressed, target) {
                (Some(pressed), Some(target)) if pressed == target => Some(pressed),
                _ => None,
            }
        };
        // outside the borrow: the action can write state and re-enter here
        match fired {
            Some(path) if self.activate_clicks(&path, clicks) => {
                self.blur();
                Some(path)
            }
            _ => {
                self.blur();
                None
            }
        }
    }

    /// The seam the pointer is on, by the AXIS it resizes — `None` when
    /// the pointer is not on one. It answers for the grip under the
    /// hand and for a drag already under way, because a seam keeps the
    /// pointer while the hand runs ahead of it.
    ///
    /// The shell dresses the pointer from this: a seam between lanes
    /// side by side travels left and right; one between stacked lanes
    /// travels up and down. Without the axis a workbench wears the same
    /// arrow on every seam, and the cursor is the only thing that says
    /// which way a seam moves before the hand pulls it.
    /// What the pointer should look like where it is — the box under it
    /// answers, or `None` and the shell's own rule stands.
    ///
    /// Topmost first, because the boxes are placed in paint order and the one
    /// drawn last is the one the eye sees. The point reaches the box in ITS
    /// coordinates, with the viewport beside it: a surface whose regions move
    /// with the scroll (a pinned gutter) cannot answer from an x alone.
    pub fn hovered_cursor(&self) -> Option<crate::layout::Cursor> {
        let at = self.interaction.borrow().pointer?;
        let customs = self.last_customs.borrow();
        customs.iter().rev().find_map(|placement| {
            let local = crate::layout::Point {
                x: at.x - placement.frame.origin.x,
                y: at.y - placement.frame.origin.y,
            };
            let inside = local.x >= 0.0
                && local.y >= 0.0
                && local.x <= placement.frame.size.width
                && local.y <= placement.frame.size.height;
            inside.then(|| placement.element.element().cursor(local, placement.visible)).flatten()
        })
    }

    pub fn seam_axis(&self) -> Option<crate::layout::Axis> {
        let interaction = self.interaction.borrow();
        let path = match interaction.split_drag.as_deref() {
            Some(path) => path.to_string(),
            None => interaction
                .hovered
                .as_deref()
                .and_then(|target| target.strip_suffix("/#split"))?
                .to_string(),
        };
        drop(interaction);
        self.last_splits
            .borrow()
            .iter()
            .find(|split| split.path == path)
            .map(|split| split.axis)
    }

    /// The pointer left the window: clears hover (an in-flight press
    /// already had its visual dropped by the drag's `pointer_moved`).
    pub fn pointer_exited(&self) -> bool {
        self.enter_scene();
        let (repaint, told) = self.watching_hover(|| self.pointer_exited_road());
        repaint || told
    }

    fn pointer_exited_road(&self) -> bool {
        let explained = self.clear_tooltip();
        let _ = explained;
        let hovered = {
            let mut interaction = self.interaction.borrow_mut();
            let hovered = interaction.hovered.take();
            interaction.pointer = None;
            hovered
        };
        // the box under the pointer hears it leave (a hovered column,
        // a hovered row of the app's own drawing, goes quiet)
        if let Some(placement) = hovered.as_deref().and_then(|path| self.custom_at(path)) {
            self.deliver(&placement, crate::custom::ElementEvent::PointerExited);
        }
        hovered.is_some()
    }

    /// Snapshot of the pointer state — the shell cursor and the asserts.
    pub fn interaction(&self) -> Interaction {
        self.interaction.borrow().clone()
    }

    // MARK: - Scrolling (offset is ENGINE state: no view invalidates)

    /// Routes the wheel to the region that paints LAST among those
    /// under the point WITH travel on the delta's axis — the innermost
    /// one of the topmost layer, which is the pointer's own rule. A
    /// modal layer's line stops the walk: what it covers does not
    /// answer. AppKit convention: positive delta reveals content above
    /// — the offset shrinks. `true` = something changed and the shell
    /// repaints (no render: zero bodies).
    pub fn wheel(&self, x: Px, y: Px, dx: Px, dy: Px) -> bool {
        self.enter_scene();
        // the content is about to slide under a still pointer — the
        // explanation dies and so does the menu, rather than pointing
        // at the wrong row
        // the tooltip and the menu die here, and that DEATH is a
        // repaint: a wheel that moves nothing still took a bubble off
        // the screen, and a shell told "nothing happened" leaves it
        // painted over a scene that no longer explains it
        let explained = self.clear_tooltip() | self.close_menu();
        // the app's box gets the turn first: an editor scrolls itself.
        // What it ignores falls through to the region around it.
        let over = self
            .hover_target(x, y)
            .and_then(|path| self.custom_at(&path));
        if let Some(placement) = over {
            let at = Self::local(&placement, x, y);
            let event = crate::custom::ElementEvent::Wheel { at, dx, dy };
            if self.deliver(&placement, event).handled {
                return true;
            }
        }
        let scrolls = self.last_scrolls.borrow();
        let travel = |region: &ScrollRegion| {
            let max_x =
                (region.content.width.round() - region.frame.size.width.round()).max(0.0);
            let max_y =
                (region.content.height.round() - region.frame.size.height.round()).max(0.0);
            (max_x, max_y)
        };
        // each AXIS routes to the region that paints LAST among those
        // under the point that travel that way — the pointer's own
        // rule, walking the list back. A child paints over its parent
        // and a layer over what it covers, so one comparison answers
        // both "innermost" and "on top": a panel over a document takes
        // the wheel, and the document behind it stays where it is.
        //
        // Per AXIS, because a diagonal gesture over a table scrolls the
        // rows AND slides the columns, instead of losing half of itself
        // inside the inner list.
        // the same line the pointer stops at, on the regions' list:
        // what a modal covers is out of reach while it is up, which is
        // what makes a settings sheet a sheet and not a picture of one
        let reachable = self.reachable(&scrolls, |floor| floor.scrolls);
        let topmost = |axis: fn((Px, Px)) -> Px| {
            reachable.iter().rev().find(|region| {
                region.frame.contains(x, y) && axis(travel(region)) > 0.0
            })
        };
        let region_y = (dy != 0.0).then(|| topmost(|(_, y)| y)).flatten();
        let region_x = (dx != 0.0).then(|| topmost(|(x, _)| x)).flatten();
        if region_x.is_none() && region_y.is_none() {
            return explained;
        }
        let mut moved = false;
        let mut offsets = self.scroll_offsets.borrow_mut();
        let mut apply = |region: &ScrollRegion, dx: Px, dy: Px| {
            let (max_x, max_y) = travel(region);
            // the wheel is sovereign: a reveal in flight dies here
            self.animator.borrow_mut().cancel_scroll(&region.path);
            let current = offsets.get(&region.path).copied().unwrap_or_default();
            let next = Point {
                x: (current.x - dx).clamp(0.0, max_x),
                y: (current.y - dy).clamp(0.0, max_y),
            };
            if next != current {
                offsets.insert(region.path.clone(), next);
                moved = true;
            }
        };
        match (region_x, region_y) {
            (Some(rx), Some(ry)) if rx.path == ry.path => apply(rx, dx, dy),
            (rx, ry) => {
                if let Some(region) = ry {
                    apply(region, 0.0, dy);
                }
                if let Some(region) = rx {
                    apply(region, dx, 0.0);
                }
            }
        }
        moved || explained
    }

    /// Programmatic scrolling — the NEXT layout (same frame) already
    /// applies it, clamped at place.
    pub fn set_scroll_offset(&self, path: &str, offset: Point) {
        self.scroll_offsets.borrow_mut().insert(path.to_string(), offset);
    }

    pub fn scroll_offset(&self, path: &str) -> Point {
        self.scroll_offsets.borrow().get(path).copied().unwrap_or_default()
    }

    // MARK: - Named actions + keymap

    /// Binds a key pattern to an action — rebind overwrites. No
    /// defaults: the app declares the map (the field editing shortcuts
    /// stay with the shell). Modifier matching is EXACT.
    pub fn bind(&self, pattern: KeyPattern, action: ActionId) {
        self.keymap.borrow_mut().insert(pattern, action);
    }

    /// Binds a SEQUENCE of strokes — `cmd-k cmd-s`, the shape a
    /// product's keymaps carry. Press the first and nothing fires: the
    /// keyboard is held (the match answers [`KeyMatch::Pending`]) until
    /// the next stroke completes the chord or lets it go.
    ///
    /// A sequence beats a single stroke on the same first key, and it
    /// has to: a `cmd-k` that fired on its own could never be the start
    /// of anything. One stroke here is just a [`Runtime::bind`].
    ///
    /// [`KeyMatch::Pending`]: crate::action::KeyMatch::Pending
    pub fn bind_sequence(&self, strokes: &[KeyPattern], action: ActionId) {
        self.push_chord(strokes, action, None);
    }

    /// [`Runtime::bind_sequence`] inside a key context — the scoped twin.
    pub fn bind_sequence_in(
        &self,
        context: &'static str,
        strokes: &[KeyPattern],
        action: ActionId,
    ) {
        self.push_chord(strokes, action, Some(context));
    }

    fn push_chord(&self, strokes: &[KeyPattern], action: ActionId, context: Option<&'static str>) {
        debug_assert!(!strokes.is_empty(), "a chord is at least one stroke");
        if strokes.is_empty() {
            return;
        }
        // one stroke is the plain table's business, and putting it here
        // would hide it from `match_key`
        if strokes.len() == 1 {
            match context {
                Some(context) => self.bind_in(context, strokes[0], action),
                None => self.bind(strokes[0], action),
            }
            return;
        }
        let mut chords = self.chords.borrow_mut();
        // a rebind overwrites, exactly like the single-stroke table
        if let Some(chord) =
            chords.iter_mut().find(|c| *c.strokes == *strokes && c.context == context)
        {
            chord.action = action;
            return;
        }
        chords.push(Chord { strokes: strokes.into(), action, context });
    }

    /// Binds the pattern INSIDE a key context: the binding only answers
    /// while some mounted view declares `.key_context(context)`. Scoped
    /// bindings beat global ones on the same pattern.
    pub fn bind_in(&self, context: &'static str, pattern: KeyPattern, action: ActionId) {
        self.scoped_keymap
            .borrow_mut()
            .entry(context)
            .or_default()
            .insert(pattern, action);
    }

    /// Empties the app's key table, so a cascade can be RE-INSTALLED
    /// instead of piled onto. Overwriting a pattern is not enough: a
    /// binding the user DELETED from their keymap has nothing to
    /// overwrite it, and would outlive the edit that removed it.
    ///
    /// The house's own contexts stand — [`RESERVED_PREFIX`] is not the
    /// app's to empty, and a popover that could no longer be dismissed
    /// with Escape would be a strange price for reloading a keymap.
    ///
    /// [`RESERVED_PREFIX`]: crate::action::RESERVED_PREFIX
    pub fn clear_bindings(&self) {
        self.keymap.borrow_mut().clear();
        self.scoped_keymap
            .borrow_mut()
            .retain(|context, _| context.starts_with(crate::action::RESERVED_PREFIX));
        self.chords.borrow_mut().retain(|chord| {
            chord.context.is_some_and(|name| name.starts_with(crate::action::RESERVED_PREFIX))
        });
        self.cancel_chord();
    }

    /// Empties ONE context — the layer-sized twin, for a cascade that
    /// re-stacks a single scope. A reserved context is not the app's to
    /// empty and the call does nothing.
    pub fn clear_bindings_in(&self, context: &str) {
        debug_assert!(
            !context.starts_with(crate::action::RESERVED_PREFIX),
            "the `{}` prefix is the framework's own",
            crate::action::RESERVED_PREFIX,
        );
        if context.starts_with(crate::action::RESERVED_PREFIX) {
            return;
        }
        self.scoped_keymap.borrow_mut().remove(context);
        self.chords.borrow_mut().retain(|chord| chord.context != Some(context));
    }

    /// The binding for the pattern: ACTIVE scoped contexts first (a
    /// mounted `.key_context` turns its bindings on), the global map as
    /// the fallback.
    pub fn match_key(&self, pattern: &KeyPattern) -> Option<ActionId> {
        let scoped = self.scoped_keymap.borrow();
        // the reserved popover context wins DETERMINISTICALLY — the
        // map below iterates in arbitrary order, and an app binding
        // Escape in its own active context must not shadow the dismiss
        if reconciler::context_active(OVERLAY_CONTEXT)
            && let Some(action) =
                scoped.get(OVERLAY_CONTEXT).and_then(|map| map.get(pattern))
        {
            return Some(*action);
        }
        for (context, map) in scoped.iter() {
            if let Some(action) = map.get(pattern)
                && reconciler::context_active(context)
            {
                return Some(*action);
            }
        }
        drop(scoped);
        self.keymap.borrow().get(pattern).copied()
    }

    /// One stroke offered to the keymap, which may be MID-CHORD. This
    /// is the door a shell uses; [`Runtime::match_key`] stays the pure
    /// lookup of a single pattern.
    ///
    /// Three answers, and the middle one is why this exists: a stroke
    /// that opens a sequence fires nothing and is SPENT — it belongs to
    /// the chord, not to the field and not to the app.
    ///
    /// The prefix cannot hold the keyboard for good. Escape lets it go,
    /// a press of the pointer lets it go, a stroke that leads nowhere
    /// ends it, and the shell's slow clock ages it out through
    /// [`Runtime::chord_tick`].
    pub fn chord(&self, stroke: impl Into<crate::action::Stroke>) -> crate::action::KeyMatch {
        use crate::action::KeyMatch;
        self.enter_scene();
        let stroke = stroke.into();
        let pattern = &stroke.pattern;
        let held = !self.pending.borrow().is_empty();
        // the explicit way out, and it consumes: a chord abandoned with
        // Escape must not also close the app's palette behind it
        if held && *pattern == KeyPattern::key(crate::action::Key::Escape) {
            self.cancel_chord();
            return KeyMatch::Pending;
        }
        self.pending.borrow_mut().push(*pattern);
        self.pending_aged.set(false);
        let live = |context: &Option<&'static str>| {
            context.is_none_or(|name| reconciler::context_active(name))
        };
        let answer = {
            let chords = self.chords.borrow();
            let pending = self.pending.borrow();
            let exact = chords
                .iter()
                .find(|chord| live(&chord.context) && *chord.strokes == pending[..])
                .map(|chord| chord.action);
            let ahead = chords.iter().any(|chord| {
                live(&chord.context)
                    && chord.strokes.len() > pending.len()
                    && chord.strokes.starts_with(&pending)
            });
            match (exact, ahead) {
                (Some(action), _) => Some(KeyMatch::Action(action)),
                // a longer chord is still reachable: hold the keyboard
                (None, true) => None,
                (None, false) => Some(KeyMatch::None),
            }
        };
        let Some(answer) = answer else {
            // the sequence is in the air: a which-key panel hears it now
            self.announce_chord();
            return KeyMatch::Pending;
        };
        // whatever it was, the sequence is over
        let strokes = self.pending.borrow().len();
        self.cancel_chord();
        match answer {
            KeyMatch::None if strokes == 1 => {
                // a plain stroke that started nothing: the single-stroke
                // table answers — first for the KEY it is, and then for
                // the character it TYPED. A keymap written by a hand
                // spells `>` and `$` and `?`, never `shift-.`, and
                // which key makes a `?` is the layout's answer: the
                // second reading is the only one that can find it.
                //
                // The key's own spelling always wins, so an app that
                // wrote both gets the one it was more precise about.
                self.match_key(pattern)
                    .or_else(|| {
                        stroke.typed_pattern().and_then(|typed| self.match_key(&typed))
                    })
                    .map_or(KeyMatch::None, KeyMatch::Action)
            }
            // a sequence that dead-ended spends its last stroke: the
            // hand was mid-chord, and re-reading it as a fresh one is
            // how an editor fires something nobody asked for
            other => other,
        }
    }

    /// The strokes of a sequence still in the air — what a which-key
    /// panel draws. Empty means the keyboard is free.
    pub fn pending_chord(&self) -> Vec<KeyPattern> {
        self.pending.borrow().clone()
    }

    /// Drops a sequence in the air. `true` = one was held.
    pub fn cancel_chord(&self) -> bool {
        self.pending_aged.set(false);
        let held = {
            let mut pending = self.pending.borrow_mut();
            let held = !pending.is_empty();
            pending.clear();
            held
        };
        if self.chord_announced.replace(false) {
            self.announce_chord();
        }
        held
    }

    /// Installs who hears the sequence move: the sink is called with the
    /// strokes in the air after every change — the door a which-key panel
    /// reads through, since the app's bodies never see a stroke. One sink;
    /// installing another replaces it.
    pub fn observe_chord(&self, sink: impl Fn(&[KeyPattern]) + 'static) {
        *self.chord_sink.borrow_mut() = Some(Rc::new(sink));
    }

    fn announce_chord(&self) {
        let sink = self.chord_sink.borrow().clone();
        let Some(sink) = sink else { return };
        let pending = self.pending.borrow().clone();
        self.chord_announced.set(!pending.is_empty());
        sink(&pending);
    }

    /// The slow clock, aging a pending prefix: the SECOND tick drops
    /// it. Same shape as the tooltip's wait — a delay is this tick seen
    /// twice, and no clock ever enters the engine.
    ///
    /// `true` = something changed and the frame is worth painting (a
    /// which-key panel just went away).
    pub fn chord_tick(&self) -> bool {
        if self.pending.borrow().is_empty() {
            return false;
        }
        if !self.pending_aged.get() {
            self.pending_aged.set(true);
            return false;
        }
        self.cancel_chord()
    }

    /// Mounts an action's handler OUTSIDE the view tree — the door for
    /// a TABLE.
    ///
    /// `.on_action(id, f)` is a view, and a view is the right shape for
    /// the ten actions an app writes by hand. It is the wrong shape for
    /// ninety of them coming from a shared `const`: a table does not
    /// expand into a chain of modifiers, and the ones that could be
    /// written out would still be a screen of plumbing between the
    /// table and the tree.
    ///
    /// ```ignore
    /// for (id, verb) in VIM_VERBS {          // a const, shared with the
    ///     let doc = doc.clone();             // other renderer
    ///     runtime.on_action(*id, move || doc.run(*verb));
    /// }
    /// ```
    ///
    /// A host handler is the OUTERMOST there is: any mounted view that
    /// claims the same id shadows it, which is the same law the tree
    /// keeps among its own — the innermost wins. Registering an id
    /// twice replaces it, exactly as a rebind does.
    ///
    /// Unlike a view's, this handler does not die with a subtree. The
    /// host mounted it and the host takes it down —
    /// [`Runtime::clear_action_handlers`].
    pub fn on_action(&self, id: ActionId, handler: impl Fn() + 'static) {
        self.hosted_handlers.borrow_mut().insert(id, Rc::new(handler));
    }

    /// Takes the host's whole table down, so a cascade can be
    /// RE-INSTALLED instead of piled onto — the twin of
    /// [`Runtime::clear_bindings`], for the other end of the bridge.
    ///
    /// The tree's own handlers are untouched: they are not the host's
    /// to remove, and they come back with the next pass anyway.
    pub fn clear_action_handlers(&self) {
        self.hosted_handlers.borrow_mut().clear();
    }

    /// Fires the innermost live handler. `false` = nobody answered —
    /// the caller decides the fallback (the gate lets the key continue
    /// on to the field).
    ///
    /// The tree answers first, whatever depth it is at, and the host's
    /// table is the floor beneath it.
    pub fn dispatch_action(&self, id: ActionId) -> bool {
        self.enter_scene();
        if reconciler::run_handler(id) {
            return true;
        }
        // out of the borrow before it runs: a handler writes state, and
        // may mount another one
        let hosted = self.hosted_handlers.borrow().get(&id).cloned();
        match hosted {
            Some(handler) => {
                handler();
                true
            }
            None => false,
        }
    }

    // MARK: - Focus and keyboard (the focused field owns the keyboard)

    pub fn focused(&self) -> Option<String> {
        self.focus.borrow().clone()
    }

    /// Forgets what the live boxes left on their own surfaces — the
    /// next present re-seeds every one of them.
    ///
    /// A shell calls this when the surfaces THEMSELVES went away: a
    /// window that dissolves its layers while it changes size, a
    /// presenter that was rebuilt. Without it the ledger would answer
    /// "nothing changed" about a surface that no longer holds a
    /// picture, and the box would come back empty.
    pub fn forget_live_surfaces(&self) {
        self.live_ledger.borrow_mut().clear();
    }

    /// The viewport the last layout ran at — the world every retained
    /// frame was computed in.
    ///
    /// A shell that presents a box on its OWN surface flips into this
    /// height, never the one the view measures: mid-resize the two
    /// disagree, and a layer placed in the wrong world lands in the
    /// wrong place. `None` before the first layout.
    pub fn last_viewport(&self) -> Option<crate::layout::Size> {
        let proposal = self.last_proposal.get()?;
        Some(crate::layout::Size {
            width: proposal.width?,
            height: proposal.height?,
        })
    }

    /// Is the keyboard's owner taking TEXT right now? The key gate of
    /// every shell asks this before it lets a bare character through
    /// as typing: `false` and the stroke walks on to the box, then to
    /// the keymap.
    ///
    /// Nothing focused answers `false` — there is no one to type. A
    /// field always answers `true`. A box the app owns answers for
    /// itself through [`crate::custom::CustomElement::takes_text`],
    /// which is how a modal editor keeps its command mode.
    pub fn focus_takes_text(&self) -> bool {
        let Some(path) = self.focused() else {
            return false;
        };
        match self.custom_at(&path) {
            Some(placement) => placement.element.element().takes_text(),
            // a field types, always — the box that is not the app's is
            // the framework's own, and it has no mode
            None => true,
        }
    }

    /// Focuses a field. The caret goes to the END the first time (the
    /// stamp's clamp resolves the `usize::MAX`); refocusing restores
    /// the retained position.
    pub fn focus(&self, path: &str) {
        self.enter_scene();
        self.caret_visible.set(true);
        *self.focus.borrow_mut() = Some(path.to_string());
        self.carets
            .borrow_mut()
            .entry(path.to_string())
            .or_insert(CaretState { caret: usize::MAX, anchor: None, marked: None });
    }

    /// The press that opens a selection: focuses, puts the caret under
    /// the pointer's X (prefix measurement with the field's effective
    /// FONT, retained from the last layout) and DROPS THE ANCHOR there
    /// — the sweep that follows extends from it.
    ///
    /// The count picks the unit, the way every text field does it: two
    /// clicks take the word under the pointer, three take the line. And
    /// `shift` keeps the anchor where it already was, which is the
    /// keyboard's own extend done with the mouse.
    fn select_at(&self, path: &str, x: Px, y: Px, clicks: u8, shift: bool) {
        self.caret_visible.set(true);
        self.goal_column.set(None);
        let held = self.focus.borrow().as_deref() == Some(path);
        *self.focus.borrow_mut() = Some(path.to_string());
        let Some((text, caret, line)) = self.caret_under(path, x, y) else {
            self.carets
                .borrow_mut()
                .entry(path.to_string())
                .or_insert(CaretState { caret: usize::MAX, anchor: None, marked: None });
            return;
        };
        let previous = self.carets.borrow().get(path).copied().unwrap_or_default();
        let state = match clicks {
            // the third click takes the line under the hand — and a
            // one-line field IS the line
            3.. => CaretState { caret: line.1, anchor: Some(line.0), marked: None },
            2 => {
                let (start, end) = word_around(&text, caret);
                CaretState { caret: end, anchor: Some(start), marked: None }
            }
            // shift extends from where the selection already stood (or
            // from the caret, when there was no selection to extend)
            _ if shift && held => CaretState {
                caret,
                anchor: Some(previous.anchor.unwrap_or(previous.caret)),
                marked: None,
            },
            // the plain press collapses and arms: anchor == caret is no
            // selection at all, and the next move gives it width
            _ => CaretState { caret, anchor: Some(caret), marked: None },
        };
        self.carets.borrow_mut().insert(path.to_string(), state);
        self.reveal_caret(path);
    }

    /// A move with the button down: the caret walks to the pointer and
    /// the anchor stays put. `true` = the selection moved and the frame
    /// must repaint.
    fn sweep_to(&self, path: &str, x: Px, y: Px) -> bool {
        self.goal_column.set(None);
        let Some((_, caret, _)) = self.caret_under(path, x, y) else { return false };
        let mut state = self.carets.borrow().get(path).copied().unwrap_or_default();
        if state.caret == caret {
            return false;
        }
        // a sweep that begins before the first layout has no anchor to
        // sweep from — it drops one where it started
        state.anchor = Some(state.anchor.unwrap_or(state.caret));
        state.caret = caret;
        self.carets.borrow_mut().insert(path.to_string(), state);
        self.caret_visible.set(true);
        self.reveal_caret(path);
        true
    }

    /// What the pointer names inside a field: its text, the byte under
    /// the point, and the VISUAL LINE the point landed on — `None` when
    /// the path holds no editor. In a many-line field the Y picks the
    /// line and the X the byte inside it; a one-line field is one line,
    /// and the Y is nothing at all.
    ///
    /// The line comes from the pointer, never from the byte: a caret
    /// sitting exactly on a break belongs to the line it was typed on,
    /// and a third click there must still take the line under the hand.
    fn caret_under(&self, path: &str, x: Px, y: Px) -> Option<(String, usize, (usize, usize))> {
        let mut probe = CaretState::default();
        let text = reconciler::run_editor(path, EditCommand::Read, &mut probe)??;
        let placement = self.field_at(path);
        // a secret field draws bullets, and a bullet is not as wide as
        // the character it stands for: the pointer has to be resolved
        // against what the eye sees, then carried back to the string
        // the app holds. Same character, both ways.
        let secret = placement.as_ref().is_some_and(|field| field.secret);
        let shown =
            if secret { crate::text_input::masked(&text) } else { String::new() };
        let seen: &str = if secret { &shown } else { &text };
        let home = |index: usize| {
            if secret { crate::text_input::unmasked_index(&text, index) } else { index }
        };
        let whole = (0, text.len());
        let (caret, line) = match placement {
            Some(field) if field.multiline => {
                let lines = self.wrap(seen, &field);
                let row = ((y - field.text_origin.y) / field.line_height).floor();
                let row = (row.max(0.0) as usize).min(lines.len().saturating_sub(1));
                let (start, end) = lines[row];
                let caret = start
                    + caret_from_x(
                        &seen[start..end],
                        x - field.text_origin.x,
                        &field.font,
                        &*self.text,
                        &self.cache,
                    );
                (home(caret), (home(start), home(end)))
            }
            Some(field) => (
                home(caret_from_x(
                    seen,
                    x - field.text_origin.x,
                    &field.font,
                    &*self.text,
                    &self.cache,
                )),
                whole,
            ),
            // before the first layout there is no run to measure
            // against: the caret goes to the end, as it always did
            None => (text.len(), whole),
        };
        Some((text, caret, line))
    }

    /// The geometry the last layout recorded for a field.
    fn field_at(&self, path: &str) -> Option<crate::layout::FieldPlacement> {
        self.last_fields.borrow().iter().find(|field| field.path == path).cloned()
    }

    /// The field's visual lines — the SAME break the placement drew,
    /// because it is the same call against the same cache.
    fn wrap(
        &self,
        text: &str,
        field: &crate::layout::FieldPlacement,
    ) -> std::rc::Rc<Vec<(usize, usize)>> {
        self.cache.get_or_break(text, &field.font, field.run.size.width, &*self.text)
    }

    /// The run follows the caret: the field scrolls its own text so the
    /// caret never hides behind a border. The offset is DERIVED — from
    /// the caret, the string, and the geometry of the last layout — and
    /// it lands where a scroll box keeps its own, under the field's
    /// path. The app writes nothing and reads nothing.
    ///
    /// A one-line field rolls sideways; a wrapped one has nothing to
    /// give sideways and rolls DOWN, one visual line at a time.
    fn reveal_caret(&self, path: &str) {
        // before the first layout there is no box to scroll inside
        let Some(field) = self.field_at(path) else { return };
        let mut probe = CaretState::default();
        let Some(Some(text)) = reconciler::run_editor(path, EditCommand::Read, &mut probe)
        else {
            return;
        };
        let caret = self.carets.borrow().get(path).map(|state| state.caret).unwrap_or(0);
        let caret = crate::text_input::clamp_index(&text, caret);
        // what scrolls is what is drawn: bullets for a secret field
        let shown = if field.secret {
            crate::text_input::masked(&text)
        } else {
            String::new()
        };
        let (text, caret) = if field.secret {
            let caret = crate::text_input::masked_index(&text, caret);
            (shown.as_str(), caret)
        } else {
            (text.as_str(), caret)
        };
        let mut offset =
            self.scroll_offsets.borrow().get(path).copied().unwrap_or_default();
        // the caret leaving through one edge pulls the run back; through
        // the other it pushes — with the caret's own width of room, so
        // the bar itself is never half eaten by the border. And it never
        // rolls past the text: deleting the tail brings the run home
        // instead of leaving a gap after the last glyph
        let follow = |at: Px, extent: Px, room: Px, box_extent: Px, full: Px| {
            let mut at_offset = extent;
            if at < at_offset {
                at_offset = at;
            } else if at + room > at_offset + box_extent {
                at_offset = at + room - box_extent;
            }
            at_offset.clamp(0.0, (full - box_extent).max(0.0))
        };
        if field.multiline {
            let lines = self.wrap(text, &field);
            let row = crate::layout::line_of(&lines, caret);
            offset.x = 0.0;
            offset.y = follow(
                row as Px * field.line_height,
                offset.y,
                field.line_height,
                field.run.size.height,
                lines.len() as Px * field.line_height,
            );
        } else {
            let caret_x =
                self.cache.get_or_measure(&text[..caret], &field.font, &*self.text).width;
            let full = self.cache.get_or_measure(text, &field.font, &*self.text).width;
            offset.x = follow(
                caret_x,
                offset.x,
                Self::CARET_ROOM,
                field.run.size.width,
                full,
            );
        }
        self.scroll_offsets.borrow_mut().insert(path.to_string(), offset);
    }

    /// Enter, offered to the focused field. A many-line field takes it
    /// as a break; a one-line one declines, and the stroke goes on to
    /// the app's bindings — which is why `⌘↵` still commits.
    fn insert_break(&self, path: &str) -> Edited {
        match self.field_at(path).is_some_and(|field| field.multiline) {
            true => self.key(EditCommand::Insert("\n".into())),
            // a one-line field has no break to take, so the bare
            // stroke is its SUBMIT — and where the app named none it
            // still declines, the way it always did
            false => self.submit(path),
        }
    }

    /// The field's own key, offered to `.on_submit`. A field that
    /// named no handler declines and the stroke walks on to the app's
    /// keys, which is what the bare Enter did before the door existed.
    fn submit(&self, path: &str) -> Edited {
        let mut state = self.carets.borrow().get(path).copied().unwrap_or_default();
        // the empty answer is the field saying the stroke is spent
        let taken = matches!(
            reconciler::run_editor(path, EditCommand::Submit, &mut state),
            Some(Some(_))
        );
        Edited { applied: taken, output: None }
    }

    /// A vertical arrow, offered to the focused field. The caret walks
    /// ONE visual line and keeps the column it started from — off the
    /// top it goes home, off the bottom to the end, the way a text view
    /// does everywhere. A one-line field declines, so a list under a
    /// search box still navigates with the arrows.
    fn walk_line(&self, path: &str, down: bool, select: bool) -> Edited {
        let declined = Edited { applied: false, output: None };
        let Some(field) = self.field_at(path).filter(|field| field.multiline) else {
            return declined;
        };
        let mut probe = CaretState::default();
        let Some(Some(text)) = reconciler::run_editor(path, EditCommand::Read, &mut probe)
        else {
            return declined;
        };
        let lines = self.wrap(&text, &field);
        let mut state = self.carets.borrow().get(path).copied().unwrap_or_default();
        let caret = crate::text_input::clamp_index(&text, state.caret);
        let row = crate::layout::line_of(&lines, caret);
        // the column the walk keeps: taken from where the caret stands
        // and RETAINED, so crossing a short line does not lose it
        let column = self.goal_column.get().unwrap_or_else(|| {
            self.cache
                .get_or_measure(&text[lines[row].0..caret], &field.font, &*self.text)
                .width
        });
        let target = match (down, row) {
            (true, row) => row + 1,
            (false, 0) => usize::MAX,
            (false, row) => row - 1,
        };
        let landed = match lines.get(target) {
            Some(&(start, end)) => {
                start
                    + caret_from_x(
                        &text[start..end],
                        column,
                        &field.font,
                        &*self.text,
                        &self.cache,
                    )
            }
            // off the top or the bottom: the ends of the text
            None if down => text.len(),
            None => 0,
        };
        // the same rule the arrows already follow: shift arms the anchor
        // and keeps it, a bare walk drops the selection
        if select {
            state.anchor = Some(state.anchor.unwrap_or(caret));
        } else {
            state.anchor = None;
        }
        state.caret = landed;
        state.marked = None;
        self.carets.borrow_mut().insert(path.to_string(), state);
        self.caret_visible.set(true);
        self.reveal_caret(path);
        self.goal_column.set(Some(column));
        Edited { applied: true, output: None }
    }

    pub fn blur(&self) -> bool {
        let dropped = self.focus.borrow_mut().take();
        // the box hears the keyboard leave — a selection that only
        // means something while focused goes quiet
        if let Some(placement) = dropped.as_deref().and_then(|path| self.custom_at(path)) {
            self.deliver(&placement, crate::custom::ElementEvent::Focused(false));
            self.dirty_island_of(&placement.path);
        }
        dropped.is_some()
    }

    /// Half-period of the blink (the shell calls it on a timer):
    /// toggles caret visibility. `true` = a field is focused — repaint.
    pub fn blink(&self) -> bool {
        if self.focus.borrow().is_none() {
            self.caret_visible.set(true);
            return false;
        }
        self.caret_visible.set(!self.caret_visible.get());
        true
    }

    /// The IME snapshot of the focused field — `None` without focus.
    /// Indices already in UTF-16 at the framework edge; the caret rect
    /// comes from the geometry retained from the last layout.
    pub fn ime_snapshot(&self) -> Option<ImeSnapshot> {
        use crate::text_input::byte_to_utf16;

        let path = self.focus.borrow().clone()?;
        // the app's box answers for itself; only the caret rect
        // changes hands, from the box's coordinates into the scene's
        if let Some(placement) = self.custom_at(&path) {
            let metrics =
                crate::custom::Metrics::new(&*self.text, &self.cache, placement.font);
            let context = placement.element.element().ime(&metrics)?;
            return Some(ImeSnapshot {
                text: context.text,
                selected: context.selected,
                marked: context.marked,
                caret_rect: Self::to_layout(&placement, context.caret_rect),
            });
        }
        let mut probe = CaretState::default();
        let text = reconciler::run_editor(&path, EditCommand::Read, &mut probe)??;
        let state = self.carets.borrow().get(&path).copied().unwrap_or_default();

        let caret = crate::text_input::clamp_index(&text, state.caret);
        let (start, end) = state.selection().unwrap_or((caret, caret));
        let start_utf16 = byte_to_utf16(&text, start);
        let selected = (start_utf16, byte_to_utf16(&text, end) - start_utf16);
        let marked = state.marked.map(|(start, end)| {
            let start_utf16 = byte_to_utf16(&text, start);
            (start_utf16, byte_to_utf16(&text, end) - start_utf16)
        });

        let field = self
            .last_fields
            .borrow()
            .iter()
            .find(|field| field.path == path)
            .cloned()?;
        let metrics = self.cache.get_or_measure(&text, &field.font, &*self.text);
        let prefix = self.cache.get_or_measure(&text[..caret], &field.font, &*self.text).width;
        let caret_rect = Rect {
            origin: Point { x: field.text_origin.x + prefix, y: field.text_origin.y },
            size: crate::layout::Size { width: 1.5, height: metrics.height() },
        };

        Some(ImeSnapshot { text, selected, marked, caret_rect })
    }

    /// The UTF-16 index in the FOCUSED field at a layout point — `None`
    /// off the field or without focus. The input system's
    /// characterIndexForPoint (dictionary lookup by mouse) answers
    /// through this.
    pub fn ime_index_at(&self, x: Px, y: Px) -> Option<usize> {
        let path = self.focus.borrow().clone()?;
        if let Some(placement) = self.custom_at(&path) {
            if !placement.frame.contains(x, y) {
                return None;
            }
            let local = Self::local(&placement, x, y);
            let metrics =
                crate::custom::Metrics::new(&*self.text, &self.cache, placement.font);
            return placement.element.element().ime_index_at(local, &metrics);
        }
        let field = self
            .last_fields
            .borrow()
            .iter()
            .find(|field| field.path == path)
            .cloned()?;
        if !field.frame.contains(x, y) {
            return None;
        }
        let mut probe = CaretState::default();
        let text = reconciler::run_editor(&path, EditCommand::Read, &mut probe)??;
        let byte =
            caret_from_x(&text, x - field.text_origin.x, &field.font, &*self.text, &self.cache);
        Some(crate::text_input::byte_to_utf16(&text, byte))
    }

    /// The caret-shaped rect at a UTF-16 index of the focused field, in
    /// LAYOUT coordinates — the real answer for a ranged
    /// firstRectForCharacterRange (the candidate window placed at the
    /// COMPOSITION's start, not always at the caret).
    pub fn ime_rect_for(&self, utf16: usize) -> Option<Rect> {
        let path = self.focus.borrow().clone()?;
        if let Some(placement) = self.custom_at(&path) {
            let metrics =
                crate::custom::Metrics::new(&*self.text, &self.cache, placement.font);
            let rect = placement.element.element().ime_rect_for(utf16, &metrics)?;
            return Some(Self::to_layout(&placement, rect));
        }
        let field = self
            .last_fields
            .borrow()
            .iter()
            .find(|field| field.path == path)
            .cloned()?;
        let mut probe = CaretState::default();
        let text = reconciler::run_editor(&path, EditCommand::Read, &mut probe)??;
        let byte = crate::text_input::utf16_to_byte(&text, utf16);
        let metrics = self.cache.get_or_measure(&text, &field.font, &*self.text);
        let prefix = self.cache.get_or_measure(&text[..byte], &field.font, &*self.text).width;
        Some(Rect {
            origin: Point { x: field.text_origin.x + prefix, y: field.text_origin.y },
            size: crate::layout::Size { width: 1.5, height: metrics.height() },
        })
    }

    /// Dom mode's sync door: the BROWSER's input owns the editing there,
    /// and this mirrors its value back into the binding. Focus follows
    /// the input; a changed value replaces the whole content in one
    /// binding write; `caret_utf16` is the input's `selectionStart`
    /// (UTF-16 units, the browser's vocabulary). Never called during a
    /// live composition — the glue guards that boundary.
    pub fn sync_field(&self, path: &str, content: &str, caret_utf16: usize) -> bool {
        if !reconciler::has_editor(path) {
            return false;
        }
        if self.focus.borrow().as_deref() != Some(path) {
            self.focus(path);
        }
        let mut state = self.carets.borrow().get(path).copied().unwrap_or_default();
        let current = reconciler::run_editor(path, EditCommand::Read, &mut state)
            .flatten()
            .unwrap_or_default();
        if current != content {
            let _ = reconciler::run_editor(path, EditCommand::SelectAll, &mut state);
            let _ = reconciler::run_editor(
                path,
                EditCommand::Insert(content.to_string()),
                &mut state,
            );
        }
        state.caret = crate::text_input::utf16_to_byte(content, caret_utf16);
        state.anchor = None;
        state.marked = None;
        self.carets.borrow_mut().insert(path.to_string(), state);
        true
    }

    /// Applies an edit command to the focused field. The binding write
    /// already dirtied whoever reads; typing returns the caret to solid.
    pub fn key(&self, command: EditCommand) -> Edited {
        self.enter_scene();
        let Some(path) = self.focus.borrow().clone() else {
            return Edited { applied: false, output: None };
        };
        // a secret field is refused the two commands that would carry
        // what it holds out of it. A cut is refused WHOLE — it does not
        // take a copy and it does not delete, because half a cut is
        // worse than none. A paste is untouched: the secret is what
        // leaves the box, never what enters it.
        if matches!(command, EditCommand::Copy | EditCommand::Cut)
            && self.field_at(&path).is_some_and(|field| field.secret)
        {
            return Edited { applied: false, output: None };
        }
        // three commands a headless model cannot answer: they need the
        // wrap, and the wrap is geometry. A field that declines lets
        // the stroke through to the app, which is the whole point of
        // Enter and of the vertical arrows
        match command {
            EditCommand::Newline => return self.insert_break(&path),
            EditCommand::Up(select) => return self.walk_line(&path, false, select),
            EditCommand::Down(select) => return self.walk_line(&path, true, select),
            // any other command is a fresh start for the walk's column
            _ => self.goal_column.set(None),
        }
        // the app's box speaks its own vocabulary: what the shell means
        // by "insert this text" or "this is the marked text" arrives as
        // an event, and the box owns the rest (a document's caret is
        // not a field's)
        if let Some(placement) = self.custom_at(&path) {
            let event = match command {
                EditCommand::Insert(text) => crate::custom::ElementEvent::Text(text),
                EditCommand::SetMarked { text, caret_utf16 } => {
                    crate::custom::ElementEvent::Marked { text, caret_utf16 }
                }
                EditCommand::Unmark => crate::custom::ElementEvent::Unmark,
                EditCommand::Copy => crate::custom::ElementEvent::Copy,
                EditCommand::Cut => crate::custom::ElementEvent::Cut,
                // everything else is a stroke, and a stroke has one
                // door: the gate offered it before the keymap
                other => {
                    let _ = other;
                    return Edited { applied: false, output: None };
                }
            };
            let response = self.deliver(&placement, event);
            if response.handled {
                self.caret_visible.set(true);
                self.dirty_island_of(&placement.path);
            }
            return Edited { applied: response.handled, output: response.text };
        }
        let mut state = self.carets.borrow().get(&path).copied().unwrap_or_default();
        // outside the map borrow: the editor writes to the binding and
        // can re-enter the runtime
        match reconciler::run_editor(&path, command, &mut state) {
            Some(output) => {
                self.carets.borrow_mut().insert(path.clone(), state);
                self.caret_visible.set(true);
                self.reveal_caret(&path);
                Edited { applied: true, output }
            }
            None => Edited { applied: false, output: None },
        }
    }

    /// A full frame for the shell: settle, layout at the viewport,
    /// raster at the scale — the hits stay retained for the events. If
    /// content moved under a still pointer (an action inserted/removed),
    /// hover re-resolves against the new hits and runs ONE extra pass —
    /// interaction always resolved BEFORE the pass that paints it.
    #[cfg(feature = "canvas")]
    pub fn frame(
        &self,
        root: &impl View,
        size: crate::layout::Size,
        scale: usize,
        background: crate::layout::Color,
    ) -> crate::raster::Bitmap {
        let display = self.display_frame(root, size);
        crate::raster::rasterize_with(
            &display,
            (size.width.round() as usize) * scale,
            (size.height.round() as usize) * scale,
            scale,
            background,
            &*self.text,
            &*self.images,
        )
    }

    /// The frame up to the display list — settle, layout, and the
    /// bounded hover re-resolve (content may have moved under an idle
    /// pointer). The incremental-repaint path: the shell hands this to
    /// its retained [`Surface`] and blits only the damage.
    ///
    /// [`Surface`]: crate::raster::Surface
    pub fn display_frame(
        &self,
        root: &impl View,
        size: crate::layout::Size,
    ) -> crate::layout::DisplayList {
        self.settle(root);
        let mut result = self.layout(root, crate::layout::Proposal::exact(size));
        let pointer = self.interaction.borrow().pointer;
        if let Some(point) = pointer
            && self.pointer_moved(point.x, point.y, self.pointer_modifiers.get())
        {
            result = self.layout(root, crate::layout::Proposal::exact(size));
        }
        result.display
    }

    /// Advances the retained animations by `dt` seconds and says what
    /// moved: `scene` asks for a layout frame, `islands` only repaints
    /// the looping boxes. With nothing animating the call is free — the
    /// shell pauses its frame driver while both stay false.
    pub fn tick(&self, dt: f64) -> crate::anim::Ticked {
        // the engine's clock moves with the frames: a sleeping task
        // wakes here, and its waker asks the shell for a settled turn
        // (this path only repaints, and a task needs the bodies)
        motor::task::advance(dt);
        let (moved, offsets) = self.animator.borrow_mut().tick(dt);
        // scroll flights write their in-flight value back into the
        // offsets the place consumes; a settled flight delivered its
        // final (snapped) value and already left the animator
        for (path, (x, y)) in offsets {
            self.scroll_offsets
                .borrow_mut()
                .insert(path.as_ref().to_string(), Point { x, y });
        }
        moved
    }

    /// Does any animation still want a next frame? The shell syncs its
    /// frame driver (display link, rAF) with this after every present.
    pub fn wants_frame(&self) -> bool {
        // a sleeping task needs the clock to keep moving, and the
        // clock is the frame tick — the shell's driver stays awake
        self.animator.borrow().wants_frame() || motor::task::has_timers()
    }

    /// The frame rate the moment deserves. A shell with a slow timer
    /// serves a loop-only scene with one frame per step instead of a
    /// display-rate driver — the difference between a decoration and a
    /// busy app. Tasks in flight keep the display pace: their wakers
    /// ride the frame clock.
    pub fn frame_pace(&self) -> crate::anim::FramePace {
        let pace = self.animator.borrow().pace();
        if motor::task::has_timers() && pace != crate::anim::FramePace::Display {
            return crate::anim::FramePace::Display;
        }
        pace
    }

    /// Freezes (or resumes) the loop clocks — the shell calls it when
    /// the window leaves or reaches the front. A decoration animates
    /// for eyes that are on it; springs keep flying either way.
    pub fn set_loops_paused(&self, paused: bool) {
        self.animator.borrow_mut().set_loops_paused(paused);
    }

    /// Accessibility: on, every animation completes instantly. The
    /// shell mirrors the system setting; an app may also set it.
    pub fn set_reduce_motion(&self, on: bool) {
        self.animator.borrow_mut().set_reduce_motion(on);
    }

    /// The two halves of motion, set apart. A shell that delegates its
    /// SPRINGS to the platform — the browser, where an animation spec
    /// lowers to a CSS transition — silences ours and keeps the loop
    /// clocks, which nothing else drives. `set_reduce_motion` is still
    /// the coarse accessibility switch, and it silences both.
    pub fn set_motion(&self, springs_reduced: bool, loops_reduced: bool) {
        self.animator.borrow_mut().set_motion(springs_reduced, loops_reduced);
    }

    /// The frame a TICK drives: layout only — no settle, no effect
    /// pump. A tick moves animated values, never state, so the pass
    /// runs zero bodies on a stable tree; settle and effects stay on
    /// the real-event path (the documented contract). Hover still
    /// re-resolves: content slides under a still pointer while
    /// something animates.
    pub fn animation_frame(
        &self,
        root: &impl View,
        size: crate::layout::Size,
    ) -> crate::layout::DisplayList {
        let mut result = self.layout(root, crate::layout::Proposal::exact(size));
        let pointer = self.interaction.borrow().pointer;
        if let Some(point) = pointer
            && self.pointer_moved(point.x, point.y, self.pointer_modifiers.get())
        {
            result = self.layout(root, crate::layout::Proposal::exact(size));
        }
        result.display
    }

    /// The frame in DOM mode: settle, then the same convergence loop
    /// as [`Runtime::layout`] — with the capture riding EVERY round,
    /// so the settled round's scene is the one lowered and no second
    /// walk ever runs. The result is the patch list that brings the
    /// element tree up to date — empty when nothing observable
    /// changed (a caret blink).
    ///
    /// Two idle costs of the pixel path stay out on purpose: the
    /// engine never ticks springs here (animation specs lower into
    /// the patches as CSS transitions and the browser animates), and
    /// hover never re-resolves (the glue sends no pointer moves —
    /// `:hover` belongs to the browser, and the scene is pointer-
    /// invariant by construction).
    pub fn dom_frame(
        &self,
        root: &impl View,
        size: crate::layout::Size,
    ) -> Vec<crate::dom::DomPatch> {
        self.settle(root);
        // everything that ran while settling — the reuse decision's
        // whole evidence (a theme change already cleared retention,
        // which re-runs every body and empties no promise wrongly)
        let changed = reconciler::take_frame_runs();
        let retained_groups = self.dom.borrow().group_paths();
        // the tree, stable-root shortcut included — the flow twin of
        // the pixel path's pass assembly
        // a drag crossing targets runs no body at all, so the ring is
        // news the reuse shortcut can only hear from the interaction
        let rings = self.drop_rings();
        let rings_held = *self.last_drop_rings.borrow() == rings;
        *self.last_drop_rings.borrow_mut() = rings.clone();
        let stable_root = (self.root_is_boundary.get()
            && crate::theme::version() == self.theme_version.get()
            && rings_held
            && !self.has_pending_dirty())
        .then(|| self.last_root.borrow().clone())
        .flatten()
        .filter(|path| reconciler::is_retained(path));
        let tree = match stable_root {
            Some(path) => {
                reconciler::note_stable_frame();
                crate::layout::LayoutNode::BoundaryRef { path }
            }
            None => {
                let mut nodes = self.frame_pass(root);
                let mut roots = nodes.take_layout();
                self.root_is_boundary.set(matches!(
                    roots.as_slice(),
                    [crate::layout::LayoutNode::Boundary { .. }]
                        | [crate::layout::LayoutNode::BoundaryRef { .. }]
                ));
                if roots.len() == 1 {
                    roots.remove(0)
                } else {
                    crate::layout::LayoutNode::Stack {
                        axis: crate::layout::Axis::Vertical,
                        spacing: 0.0,
                        align: crate::layout::CrossAlign::Start,
                        children: roots,
                    }
                }
            }
        };
        // the island door: only an island's own subtree may measure
        // and place, locally — the flow walk itself never does
        let interaction = self.interaction.borrow().clone();
        let focus = self.focus.borrow().clone();
        let carets = self.carets.borrow();
        let stamp = crate::layout::FrameStamp {
            interaction: &interaction,
            focus: focus.as_deref(),
            carets: &carets,
            caret_visible: self.caret_visible.get(),
        };
        self.cache.begin_frame();
        self.last_proposal.set(Some(crate::layout::Proposal::exact(size)));
        let offsets = self.scroll_offsets.borrow();
        let dialogs = self.dialog_frames.borrow();
        let env = LayoutEnv {
            text: &*self.text,
            images: &*self.images,
            cache: &self.cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
            line_height: None,
            text_align: None,
            stamp,
            animator: Some(&self.animator),
            live: None,
            scale: self.device_scale.get(),
            anim: None,
            overlay_bounds: self.overlay_bounds.get(),
            dialog_frames: Some(&dialogs),
        };
        let no_promises = std::collections::HashSet::new();
        let boxes = self.island_boxes.borrow();
        let flow = crate::dom_flow::FlowEnv {
            scroll_offsets: &*offsets,
            size: (size.width, size.height),
            layout: Some(env),
            changed: &changed,
            retained_groups: match rings_held {
                true => &retained_groups,
                // the ring moved: no boundary may promise this frame,
                // or the walk never reaches the target that wears it
                false => &no_promises,
            },
            island_boxes: &boxes,
            drop_rings: &rings,
        };
        let output = crate::stats::time(crate::stats::Stage::Capture, || {
            crate::dom_flow::lower(&tree, &flow)
        });
        drop(boxes);
        self.seed_island_boxes(&output.scene);
        *self.dom_customs.borrow_mut() = output.customs.clone();
        drop(offsets);
        drop(carets);
        // the first field that asks for focus takes it — once
        for (path, wants) in &output.fields {
            if *wants
                && !self.auto_focused.borrow().contains(path)
            {
                self.auto_focused.borrow_mut().insert(path.clone());
                if self.focus.borrow().is_none() {
                    self.focus(path);
                }
            }
        }
        self.dom.borrow_mut().lower(&output.scene, &output.display)
    }

    /// Hydration's engine half: run the same frame the build ran and
    /// ADOPT its scene as the retained truth — the browser already
    /// holds these elements, ids assigned by the same pre-order. The
    /// next [`Runtime::dom_frame`] diffs against a page that is
    /// already true and says nothing.
    pub fn dom_adopt(&self, root: &impl View, size: crate::layout::Size) {
        self.settle(root);
        let _ = reconciler::take_frame_runs();
        let retained_groups = self.dom.borrow().group_paths();
        // a drag crossing targets runs no body at all, so the ring is
        // news the reuse shortcut can only hear from the interaction
        let rings = self.drop_rings();
        let rings_held = *self.last_drop_rings.borrow() == rings;
        *self.last_drop_rings.borrow_mut() = rings.clone();
        let stable_root = (self.root_is_boundary.get()
            && crate::theme::version() == self.theme_version.get()
            && rings_held
            && !self.has_pending_dirty())
        .then(|| self.last_root.borrow().clone())
        .flatten()
        .filter(|path| reconciler::is_retained(path));
        let tree = match stable_root {
            Some(path) => {
                reconciler::note_stable_frame();
                crate::layout::LayoutNode::BoundaryRef { path }
            }
            None => {
                let mut nodes = self.frame_pass(root);
                let mut roots = nodes.take_layout();
                self.root_is_boundary.set(matches!(
                    roots.as_slice(),
                    [crate::layout::LayoutNode::Boundary { .. }]
                        | [crate::layout::LayoutNode::BoundaryRef { .. }]
                ));
                if roots.len() == 1 {
                    roots.remove(0)
                } else {
                    crate::layout::LayoutNode::Stack {
                        axis: crate::layout::Axis::Vertical,
                        spacing: 0.0,
                        align: crate::layout::CrossAlign::Start,
                        children: roots,
                    }
                }
            }
        };
        let interaction = self.interaction.borrow().clone();
        let focus = self.focus.borrow().clone();
        let carets = self.carets.borrow();
        let stamp = crate::layout::FrameStamp {
            interaction: &interaction,
            focus: focus.as_deref(),
            carets: &carets,
            caret_visible: self.caret_visible.get(),
        };
        self.cache.begin_frame();
        self.last_proposal.set(Some(crate::layout::Proposal::exact(size)));
        let offsets = self.scroll_offsets.borrow();
        let dialogs = self.dialog_frames.borrow();
        let env = LayoutEnv {
            text: &*self.text,
            images: &*self.images,
            cache: &self.cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
            line_height: None,
            text_align: None,
            stamp,
            animator: Some(&self.animator),
            live: None,
            scale: self.device_scale.get(),
            anim: None,
            overlay_bounds: self.overlay_bounds.get(),
            dialog_frames: Some(&dialogs),
        };
        let changed: Vec<String> = Vec::new();
        let no_promises = std::collections::HashSet::new();
        let boxes = self.island_boxes.borrow();
        let flow = crate::dom_flow::FlowEnv {
            scroll_offsets: &*offsets,
            size: (size.width, size.height),
            layout: Some(env),
            changed: &changed,
            retained_groups: match rings_held {
                true => &retained_groups,
                // the ring moved: no boundary may promise this frame,
                // or the walk never reaches the target that wears it
                false => &no_promises,
            },
            island_boxes: &boxes,
            drop_rings: &rings,
        };
        let output = crate::dom_flow::lower(&tree, &flow);
        drop(boxes);
        self.seed_island_boxes(&output.scene);
        *self.dom_customs.borrow_mut() = output.customs.clone();
        drop(offsets);
        drop(carets);
        self.dom.borrow_mut().adopt(&output.scene, &output.display);
    }

    /// A click resolved by the BROWSER: the glue walked up from the
    /// event target to the nearest `[data-path]` and hands the path
    /// straight to the action door — no engine hit test, no geometry.
    pub fn dom_action(&self, path: &str, clicks: u8) -> bool {
        reconciler::run_action(path, clicks)
    }

    /// The canvas islands whose CONTENT changed since the last call —
    /// each one's display list and its physical size, with no pixels.
    /// A tier that paints its own islands takes this road;
    /// [`Runtime::dom_islands`] is the CPU twin and is built on it.
    ///
    /// The dirty marks are CONSUMED, so a frame calls one of the two
    /// and never both.
    #[cfg(feature = "canvas")]
    pub fn dom_island_lists(&self, scale: usize) -> Vec<crate::dom::IslandList> {
        self.dom
            .borrow_mut()
            .take_dirty_islands()
            .into_iter()
            .map(|(id, width, height, commands)| {
                let mut display = crate::layout::DisplayList::default();
                for command in commands {
                    display.push(command);
                }
                let physical = (
                    ((width.round() as usize) * scale).max(1),
                    ((height.round() as usize) * scale).max(1),
                );
                crate::dom::IslandList { id, width: physical.0, height: physical.1, display }
            })
            .collect()
    }

    /// The canvas islands whose pixels changed since the last call —
    /// rasterized at `scale` and ready to blit. Empty when the scene
    /// has no islands or nothing inside one moved.
    #[cfg(feature = "canvas")]
    pub fn dom_islands(&self, scale: usize) -> Vec<crate::dom::IslandFrame> {
        self.dom_island_lists(scale)
            .into_iter()
            .map(|island| {
                let bitmap = crate::raster::rasterize_with(
                    &island.display,
                    island.width,
                    island.height,
                    scale,
                    crate::layout::Color::rgba(0, 0, 0, 0),
                    &*self.text,
                    &*self.images,
                );
                crate::dom::IslandFrame {
                    id: island.id,
                    width: island.width,
                    height: island.height,
                    // the island cleared to NOTHING, so the blend left
                    // the colour multiplied by its own coverage —
                    // `putImageData` reads straight and would multiply
                    // it a second time
                    rgba: crate::raster::unpremultiplied(&bitmap.to_rgba_bytes()),
                }
            })
            .collect()
    }

    /// Which drop targets a live drag rings, in walk order — the flow
    /// lowering holds no geometry, so the comparison happens here,
    /// against the regions the last layout recorded. A target that
    /// paints its OWN preview takes no ring: one affordance per
    /// target, and the app's wins.
    fn drop_rings(&self) -> Vec<bool> {
        let over = self.interaction.borrow().drag.as_ref().and_then(|live| live.over);
        let Some(over) = over else { return Vec::new() };
        self.last_drops
            .borrow()
            .iter()
            .map(|region| region.over.is_none() && region.rect == over)
            .collect()
    }

    /// The scroll region path behind a Dom element id — the glue's
    /// scroll observer reports by id, the runtime scrolls by path.
    pub fn dom_scroll_path(&self, id: u32) -> Option<String> {
        self.dom.borrow().scroll_path(id)
    }

    /// The live boxes whose picture changed on the latest clock steps —
    /// repainted at their new phase and rasterized at `scale`, ready to
    /// blit. The scene is NEVER touched: no body runs, no layout runs,
    /// the retained display list stays byte-identical. A step whose
    /// paint lands on the same commands is dropped by the ledger.
    ///
    /// The shell calls this when a tick reports `islands` and presents
    /// each blit on the box's own surface (a layer on macOS, the
    /// island canvas on the web) — the window behind it never redraws.
    #[cfg(feature = "canvas")]
    pub fn live_islands(&self, scale: usize) -> Vec<LiveBlit> {
        let dirty = self.animator.borrow_mut().take_dirty_loops();
        if dirty.is_empty() {
            return Vec::new();
        }
        self.live_repaint(scale, &dirty)
    }

    /// Every live box, repainted at its current phase — the presenter
    /// calls it on an ordinary frame to seed (or refresh) the boxes'
    /// own surfaces. The ledger stays in charge: a box whose picture
    /// did not change costs one paint pass and NO raster, so an app
    /// that wakes often (a file watch, a poll) never pays the mark
    /// again for a frame that left it alone.
    #[cfg(feature = "canvas")]
    pub fn live_islands_all(&self, scale: usize) -> Vec<LiveBlit> {
        let paths: Vec<Rc<str>> = self
            .last_customs
            .borrow()
            .iter()
            .filter(|placement| placement.live.is_some())
            .map(|placement| Rc::from(placement.path.as_str()))
            .collect();
        if paths.is_empty() {
            return Vec::new();
        }
        self.live_repaint(scale, &paths)
    }

    /// Where every live box sits right now — the presenter re-places
    /// the boxes' surfaces on an ordinary frame (a moved bar carries
    /// its mark along) without repainting a pixel.
    #[cfg(feature = "canvas")]
    pub fn live_frames(&self) -> Vec<(String, crate::layout::Rect)> {
        self.last_customs
            .borrow()
            .iter()
            .filter(|placement| placement.live.is_some())
            .map(|placement| {
                (
                    placement.path.clone(),
                    crate::layout::Rect {
                        origin: crate::layout::Point {
                            x: placement.frame.origin.x + placement.visible.origin.x,
                            y: placement.frame.origin.y + placement.visible.origin.y,
                        },
                        size: placement.visible.size,
                    },
                )
            })
            .collect()
    }

    /// The display-list ranges owned by the live boxes of the last
    /// layout — what a GPU presenter carves out of the scene (each box
    /// presents on its own layer instead).
    #[cfg(feature = "canvas")]
    pub fn live_slices(&self) -> Vec<(usize, usize)> {
        self.last_customs
            .borrow()
            .iter()
            .filter(|placement| placement.live.is_some())
            .map(|placement| placement.slice)
            .collect()
    }

    /// The identities of the live boxes still placed — the presenter
    /// sweeps dead layers against this list.
    #[cfg(feature = "canvas")]
    pub fn live_paths(&self) -> Vec<String> {
        self.last_customs
            .borrow()
            .iter()
            .filter(|placement| placement.live.is_some())
            .map(|placement| placement.path.clone())
            .collect()
    }

    /// The native hosts placed by the last layout — the shell mounts,
    /// places and sweeps the platform views by these boxes, each
    /// frame.
    pub fn hosts(&self) -> Vec<crate::layout::HostPlacement> {
        self.last_hosts.borrow().clone()
    }

    /// The display-list ranges that painted ABOVE each host — the
    /// scene interleaved with the islands. Each entry names the host
    /// the range covers: the commands from that host's mark to the
    /// next host's (the tail is capped at the first overlay, which
    /// presents on its own surface already). A shell carves these out
    /// of the window's present and composites each on a surface of
    /// its own, between the platform views — paint order stays the
    /// truth, island or no island. A layout with nothing painted
    /// after its hosts answers nothing here, and costs nothing.
    pub fn host_segments(&self, display_len: usize) -> Vec<(String, (usize, usize))> {
        let hosts = self.last_hosts.borrow();
        if hosts.is_empty() {
            return Vec::new();
        }
        let cap = self
            .last_overlays
            .borrow()
            .first()
            .map_or(display_len, |overlay| overlay.display.0)
            .min(display_len);
        let mut segments = Vec::new();
        for (index, host) in hosts.iter().enumerate() {
            let end = hosts.get(index + 1).map_or(cap, |next| next.mark).min(cap);
            if host.mark < end {
                segments.push((host.path.clone(), (host.mark, end)));
            }
        }
        segments
    }

    /// The shell reports: the engine committed a navigation. Routed to
    /// the page's retained `on_navigate`; `false` = nothing listening.
    pub fn webview_navigated(&self, path: &str, url: &str) -> bool {
        reconciler::run_webview_navigated(path, url)
    }

    /// The shell reports: the engine REFUSED a load — a dead host, a
    /// bad certificate, a server that is down. Routed to the page's
    /// retained `on_navigate_failed`, with the url it tried and the
    /// engine's own words; `false` = nothing listening.
    pub fn webview_navigate_failed(&self, path: &str, url: &str, why: &str) -> bool {
        reconciler::run_webview_failed(path, url, why)
    }

    /// The shell reports: the page posted on the bus
    /// (`window.bunny.post(…)`). Routed to the retained `on_message`;
    /// `false` = nothing listening.
    pub fn webview_posted(&self, path: &str, body: &str) -> bool {
        reconciler::run_webview_posted(path, body)
    }

    /// The shell reports: the page's console spoke. Routed to the
    /// retained `on_console`; `false` = nothing listening.
    pub fn webview_console(&self, path: &str, line: &str) -> bool {
        reconciler::run_webview_console(path, line)
    }

    /// The shell reports: a request of the page's completed. Routed to
    /// the retained `on_request`; `false` = nothing listening.
    pub fn webview_requested(&self, path: &str, line: &str) -> bool {
        reconciler::run_webview_requested(path, line)
    }

    /// Drains what the app queued on its webview handles, addressed by
    /// the path each handle is bound to — the shell spends these on
    /// the mounted engines, once per frame. An Eval keeps its `then`
    /// here, keyed by the stamped token, until
    /// [`Runtime::webview_eval_done`] answers it.
    pub fn webview_commands(&self) -> Vec<crate::host::WebviewOp> {
        use crate::host::{WebviewCommand, WebviewOp};
        let mut ops = Vec::new();
        for (path, commands) in reconciler::drain_webview_commands() {
            for command in commands {
                ops.push(match command {
                    WebviewCommand::Navigate(url) => {
                        WebviewOp::Navigate { path: path.clone(), url }
                    }
                    WebviewCommand::Back => WebviewOp::Back { path: path.clone() },
                    WebviewCommand::Forward => WebviewOp::Forward { path: path.clone() },
                    WebviewCommand::Eval { js, then } => {
                        let token = self.webview_eval_next.get();
                        self.webview_eval_next.set(token + 1);
                        self.webview_evals.borrow_mut().insert(token, then);
                        WebviewOp::Eval { path: path.clone(), token, js }
                    }
                    WebviewCommand::Snapshot { then } => {
                        let token = self.webview_eval_next.get();
                        self.webview_eval_next.set(token + 1);
                        self.webview_snaps.borrow_mut().insert(token, then);
                        WebviewOp::Snapshot { path: path.clone(), token }
                    }
                    // no token: nothing comes back from a hand
                    WebviewCommand::Input(event) => {
                        WebviewOp::Input { path: path.clone(), event }
                    }
                });
            }
        }
        ops
    }

    /// The shell's answer to an Eval op — fires the app's `then`,
    /// outside the borrow (the callback writes state). An unknown
    /// token answers `false` and nothing runs: the question was
    /// already answered, or never asked.
    pub fn webview_eval_done(&self, token: u64, result: crate::host::EvalResult) -> bool {
        let then = self.webview_evals.borrow_mut().remove(&token);
        match then {
            Some(then) => {
                then(result);
                true
            }
            None => false,
        }
    }

    /// The shell's answer to a Snapshot op — the eval door's twin.
    pub fn webview_snapshot_done(
        &self,
        token: u64,
        result: crate::host::SnapshotResult,
    ) -> bool {
        let then = self.webview_snaps.borrow_mut().remove(&token);
        match then {
            Some(then) => {
                then(result);
                true
            }
            None => false,
        }
    }

    #[cfg(feature = "canvas")]
    fn live_repaint(&self, scale: usize, dirty: &[Rc<str>]) -> Vec<LiveBlit> {
        let customs = self.last_customs.borrow();
        let mut ledger = self.live_ledger.borrow_mut();
        // the ledger lives exactly as long as the placed live boxes
        ledger.retain(|path, _| {
            customs
                .iter()
                .any(|placement| placement.live.is_some() && *placement.path == **path)
        });
        let focus = self.focus.borrow().clone();
        let mut blits = Vec::new();
        for path in dirty {
            let Some(placement) = customs
                .iter()
                .find(|placement| placement.live.is_some() && *placement.path == **path)
            else {
                continue;
            };
            let spec = placement.live.expect("filtered on live above");
            let phase = self.animator.borrow_mut().resolve_phase(&placement.path, spec);
            // repaint in LOCAL coordinates: the painter's origin undoes
            // the visible window, so the pixels cover exactly what the
            // screen shows of the box
            let mut display = crate::layout::DisplayList::default();
            let focused = focus.as_deref() == Some(placement.path.as_str());
            let ctx = crate::custom::PaintCtx {
                frame: placement.frame,
                visible: placement.visible,
                metrics: crate::custom::Metrics::new(&*self.text, &self.cache, placement.font),
                focused,
                caret_visible: focused && self.caret_visible.get(),
                phase,
                scale: self.device_scale.get(),
            };
            let origin = crate::layout::Point {
                x: -placement.visible.origin.x,
                y: -placement.visible.origin.y,
            };
            let mut painter = crate::custom::Painter::new(
                &mut display,
                origin,
                placement.font,
                placement.ink,
            );
            placement.element.element().paint(&ctx, &mut painter);
            let physical = (
                ((placement.visible.size.width.round() as usize) * scale).max(1),
                ((placement.visible.size.height.round() as usize) * scale).max(1),
            );
            // the SIZE counts as much as the picture: a box that grew
            // without changing what it draws still owes its surface new
            // pixels, or the surface stretches the old ones
            if ledger
                .get(path)
                .is_some_and(|last| last.display == display.as_slice() && last.physical == physical)
            {
                continue;
            }
            ledger.insert(
                Rc::clone(path),
                LiveCell { display: display.as_slice().to_vec(), physical },
            );
            let bitmap = crate::raster::rasterize_with(
                &display,
                physical.0,
                physical.1,
                scale,
                crate::layout::Color::rgba(0, 0, 0, 0),
                &*self.text,
                &*self.images,
            );
            blits.push(LiveBlit {
                path: placement.path.clone(),
                frame: crate::layout::Rect {
                    origin: crate::layout::Point {
                        x: placement.frame.origin.x + placement.visible.origin.x,
                        y: placement.frame.origin.y + placement.visible.origin.y,
                    },
                    size: placement.visible.size,
                },
                width: physical.0,
                height: physical.1,
                rgba: bitmap.to_rgba_bytes(),
            });
        }
        blits
    }

    /// The browser scrolled: the offset lands in the engine AND in
    /// the retained scene, so the next diff meets its own echo and
    /// emits nothing — the browser already moved.
    pub fn dom_scrolled(&self, id: u32, x: f64, y: f64) {
        let Some(path) = self.dom_scroll_path(id) else {
            return;
        };
        let landed = crate::layout::Point { x, y };
        self.set_scroll_offset(&path, landed);
        self.dom.borrow_mut().note_scroll(id, x, y);
        // a region the app holds in a binding hears where the browser
        // put it — the same report the wheel makes on the pixel path
        if reconciler::run_scroll(&path, landed) {
            self.scroll_commands.borrow_mut().insert(path.clone(), landed);
        }
        // the region's body IS the window function here — the fresh
        // offset re-runs it (nearest retained boundary up the path)
        let mut probe = path.as_str();
        loop {
            if reconciler::is_retained(probe) {
                motor::identity::invalidate(probe);
                break;
            }
            match probe.rfind('/') {
                Some(cut) => probe = &probe[..cut],
                None => break,
            }
        }
    }

    /// The browser reported a scroll element's box (a ResizeObserver
    /// fired) — the flow frame's window math reads it next pass.
    pub fn set_dom_viewport(&self, id: u32, width: f64, height: f64) {
        let Some(path) = self.dom_scroll_path(id) else {
            return;
        };
        self.dom_viewports.borrow_mut().insert(path, (width, height));
    }

    /// The browser reported a canvas island's box (its resize
    /// observer fired). News re-runs the island's body so the next
    /// frame measures against the REAL box; an echo of what the
    /// engine already said returns false and costs nothing.
    pub fn dom_island_box(&self, id: u32, width: f64, height: f64) -> bool {
        let Some(path) = self.dom.borrow().island_path(id) else {
            return false;
        };
        if let Some((w, h)) = self.island_boxes.borrow().get(path.as_ref()) {
            if (w - width).abs() < 0.5 && (h - height).abs() < 0.5 {
                return false;
            }
        }
        self.island_boxes.borrow_mut().insert(Rc::clone(&path), (width, height));
        // the island lives under a body — the fresh box re-runs it
        // (nearest retained boundary up the path, the scroll's walk)
        let mut probe: &str = path.as_ref();
        loop {
            if reconciler::is_retained(probe) {
                motor::identity::invalidate(probe);
                break;
            }
            match probe.rfind('/') {
                Some(cut) => probe = &probe[..cut],
                None => break,
            }
        }
        true
    }

    /// A pointer event ON a canvas island, in the canvas's own
    /// coordinates (`kind`: 0 down, 1 move, 2 up). The box under the
    /// point hears it; a press GRABS the box until the release, the
    /// way the desktop's pointer does; the release hands the keyboard
    /// to a box that takes keys — first responder follows the click.
    pub fn dom_island_pointer(
        &self,
        id: u32,
        kind: u32,
        x: f64,
        y: f64,
        modifiers: impl Into<crate::action::Modifiers>,
    ) -> bool {
        let modifiers = modifiers.into();
        let Some(island) = self.dom.borrow().island_path(id) else {
            return false;
        };
        let grabbed = self.interaction.borrow().element_grab.clone();
        let placement = match (kind, grabbed) {
            // the grabbed box hears every move and the release,
            // wherever the pointer went — dragging needs it
            (1 | 2, Some(path)) => self
                .dom_customs
                .borrow()
                .iter()
                .find(|(_, custom)| custom.path == path)
                .map(|(_, custom)| custom.clone()),
            _ => self
                .dom_customs
                .borrow()
                .iter()
                .rev()
                .find(|(home, custom)| {
                    home.as_ref() == island.as_ref() && custom.frame.contains(x, y)
                })
                .map(|(_, custom)| custom.clone()),
        };
        let Some(placement) = placement else {
            return false;
        };
        let at = Self::local(&placement, x, y);
        match kind {
            0 => {
                self.interaction.borrow_mut().element_grab =
                    Some(placement.path.clone());
                self.deliver(
                    &placement,
                    crate::custom::ElementEvent::PointerDown { at, clicks: 1, modifiers },
                );
            }
            1 => {
                let pressed = self.interaction.borrow().element_grab.is_some();
                self.deliver(
                    &placement,
                    crate::custom::ElementEvent::PointerMoved { at, pressed, modifiers },
                );
            }
            2 => {
                self.interaction.borrow_mut().element_grab = None;
                self.deliver(&placement, crate::custom::ElementEvent::PointerUp { at });
                if placement.element.element().accepts_keys() {
                    self.focus_element(&placement.path);
                } else {
                    self.blur();
                }
            }
            _ => return false,
        }
        self.dirty_island_of(&placement.path);
        true
    }

    /// The paint of an app's box reads state OUTSIDE any body — no
    /// boundary hears its changes. After an event reaches a box that
    /// lives in an island, the island's body re-runs so the pixels
    /// can follow; identical paint output still blits nothing (the
    /// island ledger compares commands). A box from the pixel pass
    /// is not in this ledger — the call is a no-op there.
    fn dirty_island_of(&self, custom_path: &str) {
        let island = self
            .dom_customs
            .borrow()
            .iter()
            .find(|(_, custom)| custom.path == custom_path)
            .map(|(island, _)| Rc::clone(island));
        let Some(island) = island else {
            return;
        };
        let mut probe: &str = island.as_ref();
        loop {
            if reconciler::is_retained(probe) {
                motor::identity::invalidate(probe);
                break;
            }
            match probe.rfind('/') {
                Some(cut) => probe = &probe[..cut],
                None => break,
            }
        }
    }

    /// Every island the scene holds seeds its measured box once — so
    /// the observer's FIRST report (which only echoes the mount) does
    /// not buy a frame. Later reports that disagree are real news.
    fn seed_island_boxes(&self, scene: &crate::dom::DomNode) {
        fn walk(node: &crate::dom::DomNode, boxes: &mut HashMap<Rc<str>, (f64, f64)>) {
            if let crate::dom::DomKind::Canvas { path: Some(path), .. } = &node.kind {
                if let Some(layout) = &node.layout {
                    if let (Some(w), Some(h)) = (layout.width, layout.height) {
                        boxes.entry(Rc::clone(path)).or_insert((w, h));
                    }
                }
            }
            for child in &node.children {
                walk(child, boxes);
            }
        }
        walk(scene, &mut self.island_boxes.borrow_mut());
    }

    /// The text engine of this runtime — the shell pairs it with its
    /// retained paint surface.
    pub fn text(&self) -> Rc<dyn TextEngine> {
        Rc::clone(&self.text)
    }

    /// The font families this runtime's engine can shape — sorted, and
    /// without the system's own face, which every scene already has.
    /// A scene offering the reader a choice of face fills its list
    /// from here and writes the answer with `.font_family(…)`.
    ///
    /// The roster is the PLATFORM's, so it is read once and not on
    /// every body: the headless engine answers nothing, and a browser
    /// answers only what the page is allowed to see.
    pub fn font_families(&self) -> Vec<std::sync::Arc<str>> {
        self.text.families()
    }

    pub fn render(&self, root: &impl View) -> String {
        // retention built without print has no line to expand —
        // rebuild once and go back to normal incremental
        if self.printless.get() {
            reconciler::clear();
            self.printless.set(false);
        }
        crate::view::set_print(true);
        self.render_pass(root)
            .into_nodes()
            .iter()
            .map(|node| reconciler::expand(node).print())
            .collect()
    }

    /// Applies the reveals the app's own boxes ask for: a box inside a
    /// region answers a LOCAL rect and the region travels the shortest
    /// distance that shows it — the same arithmetic `.scroll_target`
    /// runs for a row, on both axes.
    ///
    /// Only a CHANGED answer moves anything: a caret that stayed put
    /// while the hand turned the wheel must not be dragged back.
    fn apply_element_reveals(&self, result: &crate::layout::LayoutResult) -> bool {
        let mut moved = false;
        for placement in &result.customs {
            let Some(path) = &placement.region else { continue };
            let Some(local) = placement.element.element().reveal() else { continue };
            // the memory is the LOCAL rect: the box's frame travels
            // with the region, so a layout rect would look new after
            // every wheel turn and the reveal would fight the hand
            if self.element_reveals.borrow().get(&placement.path) == Some(&local) {
                continue;
            }
            self.element_reveals.borrow_mut().insert(placement.path.clone(), local);
            let wanted = crate::layout::Rect {
                origin: Point {
                    x: placement.frame.origin.x + local.origin.x,
                    y: placement.frame.origin.y + local.origin.y,
                },
                size: local.size,
            };
            let Some(region) = result.scrolls.iter().find(|region| &region.path == path)
            else {
                continue;
            };
            moved |= self.reveal_in(region, wanted);
        }
        moved
    }

    /// Moves ONE region the shortest way that shows `wanted`, on both
    /// axes. `true` = an offset changed on the spot (an animated region
    /// flies there instead, and answers `false`).
    fn reveal_in(
        &self,
        region: &crate::layout::ScrollRegion,
        wanted: crate::layout::Rect,
    ) -> bool {
        // the shortest travel that shows an edge: nothing when it
        // already fits, the near edge when it sits before the window,
        // the far edge when it sits after
        let shift = |low: Px, high: Px, window_low: Px, window_high: Px| -> Px {
            if low < window_low {
                low - window_low
            } else if high > window_high {
                (high - window_high).min(low - window_low)
            } else {
                0.0
            }
        };
        let dx = shift(
            wanted.origin.x,
            wanted.origin.x + wanted.size.width,
            region.frame.origin.x,
            region.frame.origin.x + region.frame.size.width,
        );
        let dy = shift(
            wanted.origin.y,
            wanted.origin.y + wanted.size.height,
            region.frame.origin.y,
            region.frame.origin.y + region.frame.size.height,
        );
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        let current = self.scroll_offset(&region.path);
        let travel_x =
            (region.content.width.round() - region.frame.size.width.round()).max(0.0);
        let travel_y =
            (region.content.height.round() - region.frame.size.height.round()).max(0.0);
        let next = Point {
            x: (current.x + dx).clamp(0.0, travel_x),
            y: (current.y + dy).clamp(0.0, travel_y),
        };
        if next == current {
            return false;
        }
        let mut animator = self.animator.borrow_mut();
        match region.anim.filter(|_| !animator.reduce_motion()) {
            Some(spring) => {
                animator.animate_scroll(
                    &region.path,
                    (current.x, current.y),
                    (next.x, next.y),
                    spring,
                );
                false
            }
            None => {
                drop(animator);
                self.set_scroll_offset(&region.path, next);
                true
            }
        }
    }

    /// Applies `.scroll_target(id)` requests: for each region whose
    /// declared target CHANGED since the last application, scrolls just
    /// enough to reveal the row (top-aligns when above, bottom-aligns
    /// when below). Returns whether any offset moved. A target whose row
    /// is not mounted this frame (filtered out) stays pending — it
    /// applies when the row appears.
    fn apply_scroll_targets(&self, result: &crate::layout::LayoutResult) -> bool {
        let mut moved = false;
        for region in &result.scrolls {
            let Some(target) = &region.target else { continue };
            if self.scroll_targets.borrow().get(&region.path) == Some(target) {
                continue;
            }
            let key = format!("{}/[{}]", region.path, target);
            let Some(row) = result.frames.get(&key) else { continue };
            self.scroll_targets.borrow_mut().insert(region.path.clone(), target.clone());
            let region_top = region.frame.origin.y;
            let region_bottom = region_top + region.frame.size.height;
            let row_top = row.origin.y;
            let row_bottom = row_top + row.size.height;
            let delta = if row_top < region_top {
                row_top - region_top
            } else if row_bottom > region_bottom {
                row_bottom - region_bottom
            } else {
                0.0
            };
            if delta != 0.0 {
                let current = self.scroll_offset(&region.path);
                let travel =
                    (region.content.height.round() - region.frame.size.height.round()).max(0.0);
                let next = (current.y + delta).clamp(0.0, travel);
                if next != current.y {
                    let mut animator = self.animator.borrow_mut();
                    match region.anim.filter(|_| !animator.reduce_motion()) {
                        // an animated region REVEALS: the offset flies
                        // there over the next ticks (the wheel cancels)
                        Some(spring) => animator.animate_scroll(
                            &region.path,
                            (current.x, current.y),
                            (current.x, next),
                            spring,
                        ),
                        None => {
                            drop(animator);
                            self.set_scroll_offset(&region.path, Point { x: current.x, y: next });
                            moved = true;
                        }
                    }
                }
            }
        }
        moved
    }

    /// Hands every probe the size its view resolved to, when that size
    /// CHANGED since the last time it heard.
    ///
    /// `true` = a probe fired, so the caller relayouts — which is what
    /// puts the report and the reaction to it in the SAME frame. A body
    /// that turns a measured height into a frame gets to run before
    /// anything is painted, instead of showing one frame at the wrong
    /// size and correcting it on the next.
    ///
    /// A probe whose view left the tree keeps no entry: the ledger is
    /// swept against the frames this layout actually recorded, so a
    /// view that comes back reports again rather than staying silent
    /// on a stale match.
    fn apply_measures(&self, result: &crate::layout::LayoutResult) -> bool {
        // the cheap question first: a scene that never asked to be
        // measured must not pay a walk of the frame record to find out
        if !reconciler::has_measures() && self.measures.borrow().is_empty() {
            return false;
        }
        let mut fired = false;
        let mut seen: Vec<String> = Vec::new();
        for (path, rect) in result.frames.measured() {
            seen.push(path.to_string());
            let last = self.measures.borrow().get(path).copied();
            if last == Some(rect.size) {
                continue;
            }
            self.measures.borrow_mut().insert(path.to_string(), rect.size);
            // outside the borrow: the handler writes state, and the
            // write is the whole point
            fired |= reconciler::run_measure(path, rect.size);
        }
        self.measures.borrow_mut().retain(|path, _| seen.iter().any(|kept| kept == path));
        fired
    }

    /// Settles each region that holds its offset in a BINDING, both
    /// ways, in one pass.
    ///
    /// One value is compared against what was last published to the
    /// binding, and that is the whole rule: whichever side differs from
    /// it is the side that moved.
    ///
    /// - The app wrote: the region goes there, clamped to the travel it
    ///   actually has, and the clamped value goes back — the app's
    ///   state and the region never disagree about where it is.
    /// - Nobody wrote: the wheel, a thumb or a reveal may have moved
    ///   the region, and the binding is told where it landed.
    ///
    /// `true` = an offset moved and the caller relayouts.
    fn apply_scroll_offsets(&self, result: &crate::layout::LayoutResult) -> bool {
        let mut moved = false;
        for region in &result.scrolls {
            let Some(commanded) = region.commanded else { continue };
            let travel = |content: Px, extent: Px| (content.round() - extent.round()).max(0.0);
            let max_x = travel(region.content.width, region.frame.size.width);
            let max_y = travel(region.content.height, region.frame.size.height);
            let published = self.scroll_commands.borrow().get(&region.path).copied();
            let current = self.scroll_offset(&region.path);
            if published == Some(commanded) {
                // the app is holding a stale reading: the region moved
                // under it, and the binding hears where it landed
                if current != commanded {
                    self.scroll_commands.borrow_mut().insert(region.path.clone(), current);
                    reconciler::run_scroll(&region.path, current);
                }
                continue;
            }
            // the app WROTE. A write is sovereign over anything still
            // in flight, exactly as the wheel is
            self.animator.borrow_mut().cancel_scroll(&region.path);
            let wanted = Point {
                x: commanded.x.clamp(0.0, max_x),
                y: commanded.y.clamp(0.0, max_y),
            };
            self.scroll_commands.borrow_mut().insert(region.path.clone(), wanted);
            if wanted != current {
                self.set_scroll_offset(&region.path, wanted);
                moved = true;
            }
            // a value past the end comes home already clamped
            if wanted != commanded {
                reconciler::run_scroll(&region.path, wanted);
            }
        }
        moved
    }

    /// The FRAME-path pass: identical to the print one, but the printed
    /// tree's lines are not even formatted (printing is for people;
    /// frames are for pixels).
    fn frame_pass(&self, root: &impl View) -> NodeList {
        crate::stats::note_body_pass();
        crate::view::set_print(false);
        self.printless.set(true);
        let nodes = self.render_pass(root);
        crate::view::set_print(true);
        nodes
    }

    /// Settle, then lay out — the headless twin of
    /// [`Runtime::display_frame`], for a probe or a test that wants the
    /// frames instead of the pixels.
    ///
    /// [`Runtime::layout`] alone does NOT run effects or tasks: it lays
    /// out what the last pass built. A screen whose data arrives through
    /// `.task` stays empty forever under a bare `layout` loop, which is
    /// exactly what a headless probe writes first.
    pub fn settled_layout(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
    ) -> crate::layout::LayoutResult {
        self.settle(root);
        self.layout(root, proposal)
    }

    /// Layout of the current frame: runs one pass (incremental — stable
    /// tree = zero bodies), expands the retained layout tree, and
    /// answers the proposal with the frames by identity.
    ///
    /// It lays out; it does not settle. Effects and tasks belong to
    /// [`Runtime::settle`] — see [`Runtime::settled_layout`] for the
    /// pair a probe wants.
    pub fn layout(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
    ) -> crate::layout::LayoutResult {
        let mut result = self.layout_once(root, proposal);
        // follow-ups, capped: a scroll target that CHANGED moves its
        // region's offset; a field's first `.auto_focus()` takes the
        // keyboard; a virtualized window that missed re-materializes
        // (its boundary re-runs on the next round). The cap is the
        // livelock guard — a broken row extent degrades to a blank
        // strip for a frame, never a hang. The wheel and a user blur
        // are never fought.
        for _ in 0..2 {
            let moved = self.apply_scroll_targets(&result)
                | self.apply_element_reveals(&result)
                | self.apply_scroll_offsets(&result)
                | self.apply_measures(&result);
            let focused = self.apply_auto_focus(&result);
            // a miss measured on the round a target just moved is
            // spurious — it audited the PRE-jump offset; the relayout
            // below re-audits against the real one
            let missed = if moved {
                false
            } else {
                self.invalidate_window_misses(&result)
            };
            // a popover whose anchor scrolled out of view closes here
            // — one dismissal, one relayout, converged
            let orphaned = self.dismiss_orphaned_overlays(&result);
            if !moved && !focused && !missed && !orphaned {
                break;
            }
            result = self.layout_once(root, proposal);
        }
        result
    }

    /// Closes every popover whose anchor no longer intersects its clip
    /// (the row scrolled away). `true` = something closed and the
    /// caller relayouts.
    fn dismiss_orphaned_overlays(&self, result: &crate::layout::LayoutResult) -> bool {
        let mut closed = false;
        for overlay in &result.overlays {
            if !overlay.anchor_visible {
                closed |= reconciler::run_action(&format!("{}/#dismiss", overlay.path), 1);
            }
        }
        closed
    }

    /// A virtual window that failed to cover the visible band asks its
    /// nearest RETAINED boundary to re-run — walking prefixes of the
    /// region path finds it. `true` = something was invalidated and the
    /// next layout embeds a fresh pass.
    fn invalidate_window_misses(&self, result: &crate::layout::LayoutResult) -> bool {
        let mut any = false;
        for path in &result.misses {
            let mut probe = path.as_str();
            loop {
                if reconciler::is_retained(probe) {
                    motor::identity::invalidate(probe);
                    any = true;
                    break;
                }
                match probe.rfind('/') {
                    Some(cut) => probe = &probe[..cut],
                    None => break,
                }
            }
        }
        any
    }

    /// A dead field releases its input state. The EDITOR map is the
    /// truth: fields re-register on every pass they exist, so a caret,
    /// an auto-focus memory or the focus itself whose path has no
    /// editor belongs to an unmounted field. (The identity sweeps
    /// cannot see these — a field row owns no state and is no
    /// component boundary.) A remounted field's `.auto_focus()` fires
    /// again: the identity is genuinely new, deliberately.
    fn release_dead_input(&self) {
        // FIRST the migration, then the sweep: an input that moved must
        // take its caret and its once-per-identity memory with it, and
        // the retains below would have thrown both away
        self.follow_named_inputs();
        self.carets.borrow_mut().retain(|path, _| reconciler::has_editor(path));
        self.auto_focused.borrow_mut().retain(|key| {
            // a custom box's once-per-beat memory is keyed `path#beat`:
            // keep it while the BOX lives — sweeping it would re-arm the
            // beat every pass and the box would steal focus forever
            match key.rsplit_once('#') {
                Some((path, beat)) if beat.bytes().all(|b| b.is_ascii_digit()) => {
                    reconciler::has_custom(path)
                }
                _ => reconciler::has_editor(key),
            }
        });
        // the app's own box counts as a live input too: it registers
        // itself every pass it renders, exactly like a field's editor
        let focus_died = self
            .focus
            .borrow()
            .as_deref()
            .is_some_and(|path| !reconciler::has_editor(path) && !reconciler::has_custom(path));
        if focus_died {
            *self.focus.borrow_mut() = None;
        }
    }

    /// An input whose PATH died but whose NAME is still on screen moves
    /// house: the keyboard, the caret and the auto-focus memory follow
    /// it to wherever the tree put it.
    ///
    /// This is what makes a caret survive `⌘\`. A path carries
    /// positions — `#0`, `@First` — and wrapping a pane in a split
    /// shifts every one of them below it; the names an app wrote with
    /// `.id(…)` do not move. So when the held path is gone, the named
    /// projection of it is asked for the ONE live input that still
    /// wears that name, and the hold re-points there, in the same pass,
    /// before the frame is stamped. The box hears no focus event: as
    /// far as it knows it never lost the keyboard, which is the truth.
    ///
    /// What it cannot do, and no framework of this shape can: the
    /// component's own `State` below a re-parented branch is FRESH.
    /// Identity is the path, so a subtree that moves is a subtree that
    /// re-mounts (the SwiftUI rule for an `if` that swaps branches).
    /// Keep what must survive a re-parent ABOVE the branch that moves.
    fn follow_named_inputs(&self) {
        let held = self.focus.borrow().clone();
        if let Some(path) = held {
            let alive = reconciler::has_editor(&path) || reconciler::has_custom(&path);
            if !alive {
                let chain = motor::identity::named_chain(&path);
                if let Some(moved) = reconciler::input_by_chain(&chain, false) {
                    *self.focus.borrow_mut() = Some(moved.clone());
                    self.follow_caret(&path, &moved);
                }
            }
        }
        // a field that moved keeps its caret even when it is not the
        // one holding the keyboard — the column is the field's memory,
        // not the focus's
        let orphans: Vec<String> = self
            .carets
            .borrow()
            .keys()
            .filter(|path| !reconciler::has_editor(path))
            .cloned()
            .collect();
        for path in orphans {
            let chain = motor::identity::named_chain(&path);
            if let Some(moved) = reconciler::input_by_chain(&chain, true) {
                self.follow_caret(&path, &moved);
            }
        }
    }

    /// Moves one input's retained memory from a dead path to a live
    /// one. The destination wins if it already has some: a field that
    /// is really on screen is more truthful than a ghost.
    fn follow_caret(&self, from: &str, to: &str) {
        if from == to {
            return;
        }
        let mut carets = self.carets.borrow_mut();
        if let Some(state) = carets.remove(from) {
            carets.entry(to.to_string()).or_insert(state);
        }
        drop(carets);
        let mut seen = self.auto_focused.borrow_mut();
        if seen.remove(from) {
            seen.insert(to.to_string());
        }
    }

    /// Focuses the first `.auto_focus()` field never seen before — once
    /// per identity: blur is final, remounting does not re-focus.
    fn apply_auto_focus(&self, result: &crate::layout::LayoutResult) -> bool {
        for field in &result.fields {
            if !field.auto_focus || self.auto_focused.borrow().contains(&field.path) {
                continue;
            }
            self.auto_focused.borrow_mut().insert(field.path.clone());
            if self.focus.borrow().is_none() {
                self.focus(&field.path);
                return true;
            }
        }
        // A custom box's auto-focus is KEYED: each new beat is one explicit
        // app intent — "opening a file hands the editor the keyboard" — so
        // unlike a field's it TAKES the keyboard from whoever holds it (the
        // tree row that opened the file is exactly who must lose the keys).
        // Each (box, beat) fires once, so the user can focus away and stay
        // away until the app beats again.
        for placement in &result.customs {
            let Some(beat) = placement.element.auto_focus_beat() else {
                continue;
            };
            let key = format!("{}#{beat}", placement.path);
            if self.auto_focused.borrow().contains(&key) {
                continue;
            }
            self.auto_focused.borrow_mut().insert(key);
            if self.focus.borrow().as_deref() != Some(placement.path.as_str()) {
                self.focus(&placement.path);
                return true;
            }
        }
        false
    }

    fn layout_once(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
    ) -> crate::layout::LayoutResult {
        self.layout_once_with(root, proposal, false, true).0
    }

    /// One layout pass, optionally with the Dom capture riding it.
    fn layout_once_with(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
        dom: bool,
        collect_display: bool,
    ) -> (crate::layout::LayoutResult, Option<crate::dom::DomNode>) {
        // every call walks measure+place (the stable-root shortcut
        // skips BODIES, not geometry) — so every call counts
        crate::stats::note_layout_pass();
        // STABLE boundary-root frame (hover, wheel, blink, the
        // post-settle layout): nothing dirty, same theme, retained
        // root — the walk would be all-skip and emit exactly ONE
        // reference; synthesize the reference and skip the whole pass.
        // Any other situation walks the real pass.
        let stable_root = (self.root_is_boundary.get()
            && crate::theme::version() == self.theme_version.get()
            && !self.has_pending_dirty())
        .then(|| self.last_root.borrow().clone())
        .flatten()
        .filter(|path| reconciler::is_retained(path));
        let tree = match stable_root {
            Some(path) => {
                // the observable contract holds: THIS frame ran zero bodies
                reconciler::note_stable_frame();
                crate::layout::LayoutNode::BoundaryRef { path }
            }
            None => {
                let mut nodes = self.frame_pass(root);
                let mut roots = nodes.take_layout();
                self.root_is_boundary.set(matches!(
                    roots.as_slice(),
                    [crate::layout::LayoutNode::Boundary { .. }]
                        | [crate::layout::LayoutNode::BoundaryRef { .. }]
                ));
                if roots.len() == 1 {
                    roots.remove(0)
                } else {
                    crate::layout::LayoutNode::Stack {
                        axis: crate::layout::Axis::Vertical,
                        spacing: 0.0,
                        align: crate::layout::CrossAlign::Start,
                        children: roots,
                    }
                }
            }
        };
        // layout walks the retention IN PLACE (`BoundaryRef` resolves
        // on-the-fly) and frame state rides in the ENV — no frame
        // clones any tree
        let interaction = self.interaction.borrow().clone();
        let focus = self.focus.borrow().clone();
        let carets = self.carets.borrow();
        let stamp = crate::layout::FrameStamp {
            interaction: &interaction,
            focus: focus.as_deref(),
            carets: &carets,
            caret_visible: self.caret_visible.get(),
        };
        self.cache.begin_frame();
        // the animator's sweep clock follows PLACES, not ticks — this
        // pass's touches mark who is still mounted. A pass whose
        // proposal CHANGED is a resize: geometry moved because the
        // window did, and that is not an animation — retargets snap.
        let resized = self.last_proposal.get() != Some(proposal);
        self.last_proposal.set(Some(proposal));
        {
            let mut animator = self.animator.borrow_mut();
            animator.note_place();
            animator.set_snap_retargets(resized);
        }
        let offsets = self.scroll_offsets.borrow();
        let dialogs = self.dialog_frames.borrow();
        let env = LayoutEnv {
            text: &*self.text,
            images: &*self.images,
            cache: &self.cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
            line_height: None,
            text_align: None,
            stamp,
            animator: Some(&self.animator),
            anim: None,
            live: None,
            overlay_bounds: self.overlay_bounds.get(),
            dialog_frames: Some(&dialogs),
            scale: self.device_scale.get(),
        };
        let stage = if dom {
            crate::stats::Stage::Capture
        } else {
            crate::stats::Stage::Layout
        };
        let (result, scene) = crate::stats::time(stage, || {
            if dom {
                let (result, scene) =
                    crate::layout::layout_dom(&tree, proposal, env, collect_display);
                (result, Some(scene))
            } else {
                (crate::layout::layout_with(&tree, proposal, env), None)
            }
        });
        crate::stats::note_display(result.display.len());
        drop(offsets);
        drop(dialogs);
        drop(carets);
        *self.last_hits.borrow_mut() = result.hits.clone();
        *self.last_scrolls.borrow_mut() = result.scrolls.clone();
        self.last_modal_floor.set(result.modal_floor);
        *self.last_fields.borrow_mut() = result.fields.clone();
        *self.last_splits.borrow_mut() = result.splits.clone();
        *self.last_customs.borrow_mut() = result.customs.clone();
        *self.last_hosts.borrow_mut() = result.hosts.clone();
        *self.last_overlays.borrow_mut() = result.overlays.clone();
        *self.last_tooltips.borrow_mut() = result.tooltips.clone();
        *self.last_menus.borrow_mut() = result.menus.clone();
        *self.last_drag_sources.borrow_mut() = result.drag_sources.clone();
        *self.last_drops.borrow_mut() = result.drops.clone();
        *self.last_drag_regions.borrow_mut() = result.drag_regions.clone();
        *self.last_control_regions.borrow_mut() = result.control_regions.clone();
        // an applied-target memory whose region left the scene goes
        // with it — live regions keep theirs (the wheel stays sovereign)
        self.scroll_targets
            .borrow_mut()
            .retain(|path, _| result.scrolls.iter().any(|region| region.path == *path));
        (result, scene)
    }

    /// Forces every body (drops the retention before the pass) — the
    /// test oracle: incremental must print byte-for-byte what full
    /// prints.
    pub fn render_full(&self, root: &impl View) -> String {
        reconciler::clear();
        self.render(root)
    }

    /// A full frame down to the bitmap: layout at the viewport's exact
    /// proposal and rasterization of the display list — what the
    /// platform backend blits to the window.
    #[cfg(feature = "canvas")]
    pub fn paint(&self, root: &impl View, size: crate::layout::Size) -> crate::raster::Bitmap {
        self.paint_at_scale(root, size, 1)
    }

    /// [`Runtime::paint`] at retina: layout in logical points, bitmap
    /// in physical pixels (`size × scale`).
    #[cfg(feature = "canvas")]
    pub fn paint_at_scale(
        &self,
        root: &impl View,
        size: crate::layout::Size,
        scale: usize,
    ) -> crate::raster::Bitmap {
        // the pass paints for THIS screen: a box that snaps to the
        // pixel grid reads the same number the rasterizer will use
        self.set_device_scale(scale as Px);
        let result = self.layout(root, crate::layout::Proposal::exact(size));
        crate::raster::rasterize_with(
            &result.display,
            (size.width.round() as usize) * scale,
            (size.height.round() as usize) * scale,
            scale,
            crate::layout::Color::WHITE,
            &*self.text,
            &*self.images,
        )
    }

    /// Drains registered effects (`onReceive`, `onChange`, `query`).
    /// Returns whether any of them observed a change.
    pub fn pump(&self) -> bool {
        effects::take().iter().any(|effect| effect(&self.ctx))
    }

    /// Puts a future on the engine's queue. It runs on the next turn —
    /// never inside this call — and whatever it writes into `State`
    /// reaches the scene through the ordinary invalidation.
    ///
    /// The handle OWNS the task: drop it and the task is cancelled.
    /// `.task()` on a view is the front door; this is the raw one, for
    /// a shell or an app that owns the lifetime itself.
    #[must_use = "a dropped Spawned cancels its task — call detach() to let it run alone"]
    pub fn spawn(
        &self,
        future: impl std::future::Future<Output = ()> + 'static,
    ) -> motor::task::Spawned {
        motor::task::spawn(future)
    }

    /// Polls every task that is ready. The frame path calls it at the
    /// top of each cycle, so a result that landed since the last turn
    /// is already state by the time the bodies run.
    pub fn poll_tasks(&self) -> bool {
        motor::task::poll_ready()
    }

    /// Installs how the shell asks itself for a turn when a task wakes
    /// from somewhere else — another thread on the desktop, a browser
    /// callback on the web. Without it a resolved task waits for the
    /// next event.
    pub fn set_wake_hook(&self, hook: std::sync::Arc<dyn Fn() + Send + Sync>) {
        motor::task::set_wake_hook(hook);
    }

    /// The views `set()` dirtied since the last drain — the fine
    /// invalidation (whoever READ the written dependency), by identity
    /// path, scoped to this runtime's root.
    pub fn take_dirty(&self) -> Vec<String> {
        match self.last_root.borrow().as_deref() {
            Some(root) => motor::identity::take_dirty_matching(root),
            None => motor::identity::take_dirty(),
        }
    }

    /// Instrumentation: the bodies the last [`Runtime::render`] ran —
    /// the proof of incrementality (the rest came from the cache).
    pub fn body_runs(&self) -> Vec<String> {
        reconciler::last_body_runs()
    }

    /// Render → pump → re-render until the tree is stable (max 8 cycles).
    /// Stable = printed tree still, no effect observed a change, and no
    /// view dirty. The dirt check PEEKS without draining — pending dirt
    /// is input for the next pass, not for the loop.
    pub fn render_stable(&self, root: &impl View) -> String {
        let mut previous = String::new();
        for _ in 0..8 {
            // what a task resolved since the last turn is state BEFORE
            // the bodies read it
            self.poll_tasks();
            let printed = self.render(root);
            // pump first: side effects fired by THIS render's onAppear
            // nodes must be observed before declaring the tree stable
            let observed_change = self.pump();
            // whoever stopped being declared stops running
            effects::sweep_tasks(&self.last_root.borrow().clone().unwrap_or_default());
            if printed == previous
                && !observed_change
                && !self.has_pending_dirty()
                && !motor::task::has_ready()
            {
                return printed;
            }
            previous = printed;
        }
        previous
    }

    /// The FRAME-path [`Runtime::render_stable`]: settles WITHOUT
    /// building the printed tree (printing is for people; frames are
    /// for pixels). Stable = the pass left NOTHING behind: no effect
    /// observed a change and no view is dirty — bodies having run asks
    /// for no confirmation (a pass with no new dirt produced a
    /// consistent tree by definition; the next pass would be all-skip).
    pub fn settle(&self, root: &impl View) {
        crate::stats::time(crate::stats::Stage::Settle, || {
            for _ in 0..8 {
                // the same order as the print path: a task that resolved
                // writes its state, then the pass reads it
                self.poll_tasks();
                self.frame_pass(root);
                let observed_change = self.pump();
                effects::sweep_tasks(&self.last_root.borrow().clone().unwrap_or_default());
                if !observed_change && !self.has_pending_dirty() && !motor::task::has_ready() {
                    return;
                }
            }
        })
    }

    fn has_pending_dirty(&self) -> bool {
        match self.last_root.borrow().as_deref() {
            Some(root) => motor::identity::has_dirty_matching(root),
            None => false,
        }
    }
}
