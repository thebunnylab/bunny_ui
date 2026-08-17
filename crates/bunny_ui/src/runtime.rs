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

use crate::action::{ActionId, KeyPattern};
use crate::effects;
use crate::layout::{FieldPlacement, Interaction, LayoutEnv, Point, Px, Rect, ScrollRegion};
use crate::reconciler;
use crate::text_engine::{FontSpec, MeasureCache, PixelFont, TextEngine, caret_from_x};
use crate::text_input::{CaretState, EditCommand};
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

pub struct Runtime {
    ctx: Context,
    /// The root of the last pass — scopes `take_dirty` so it does not
    /// drain dirt from another tree mounted on the same thread.
    last_root: RefCell<Option<String>>,
    /// The targets of the last layout, in paint order — the hit-test
    /// map for pointer events.
    last_hits: RefCell<Vec<(String, Rect)>>,
    /// Pointer state for the frame — resolved BEFORE layout (the LAW:
    /// hover swaps paint, never measurement) and stamped at expansion.
    interaction: RefCell<Interaction>,
    /// The text edge of the frame — PixelFont by default (headless,
    /// byte-stable); the shell installs the platform engine.
    text: Rc<dyn TextEngine>,
    /// Double-buffered measure cache, swapped on every layout pass.
    cache: MeasureCache,
    /// Scroll offsets by identity — engine-owned (the premise's dual
    /// ownership: on the DOM the backend will own them and we will
    /// observe). DELIBERATELY not pruned when the identity goes away:
    /// a remounted list RESTORES the position, and the map is bounded
    /// by region SITES, never by rows. The other input maps (carets,
    /// targets, auto-focus) release on the sweep.
    scroll_offsets: RefCell<HashMap<String, Point>>,
    /// The scroll regions of the last layout — the wheel map.
    last_scrolls: RefCell<Vec<ScrollRegion>>,
    /// The focused field (identity path) — owner of the keyboard.
    focus: RefCell<Option<String>>,
    /// Caret + selection per field — they survive blur/refocus and
    /// remount (restored by identity, like scroll).
    carets: RefCell<HashMap<String, CaretState>>,
    /// Blink phase: the caret goes and comes back on the shell tick;
    /// typing or focusing returns it to solid (an idle caret blinks,
    /// an active one does not).
    caret_visible: Cell<bool>,
    /// The fields of the last layout (geometry + effective font) —
    /// click-to-position and IME sync measure through here.
    last_fields: RefCell<Vec<FieldPlacement>>,
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
    /// The last APPLIED `.scroll_target` per region — the follow fires
    /// only when the target changes; in between, the wheel is sovereign.
    scroll_targets: RefCell<HashMap<String, String>>,
    /// Fields whose `.auto_focus()` already fired — first appearance
    /// only; a user blur is final.
    auto_focused: RefCell<std::collections::HashSet<String>>,
    /// The retained animations — springs keyed by identity, resolved
    /// at place through the env, advanced by the shell's tick.
    animator: RefCell<crate::anim::Animator>,
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
    pub fn new() -> Self {
        Self::with_parts(Context::default(), Rc::new(PixelFont))
    }

    pub fn with_environment(values: EnvironmentValues) -> Self {
        let mut ctx = Context::default();
        ctx.values = values;
        Self::with_parts(ctx, Rc::new(PixelFont))
    }

    /// Swaps the text engine (builder — composes with `with_environment`):
    /// `Runtime::new().text_engine(Rc::new(CoreTextEngine::new()))`.
    pub fn text_engine(mut self, engine: Rc<dyn TextEngine>) -> Self {
        self.text = engine;
        self
    }

    fn with_parts(ctx: Context, text: Rc<dyn TextEngine>) -> Self {
        Runtime {
            ctx,
            last_root: RefCell::new(None),
            last_hits: RefCell::new(Vec::new()),
            interaction: RefCell::new(Interaction::default()),
            text,
            cache: MeasureCache::default(),
            scroll_offsets: RefCell::new(HashMap::default()),
            last_scrolls: RefCell::new(Vec::new()),
            focus: RefCell::new(None),
            carets: RefCell::new(HashMap::default()),
            caret_visible: Cell::new(true),
            last_fields: RefCell::new(Vec::new()),
            theme_version: Cell::new(crate::theme::version()),
            keymap: RefCell::new(HashMap::default()),
            scoped_keymap: RefCell::new(HashMap::default()),
            scroll_targets: RefCell::new(HashMap::default()),
            auto_focused: RefCell::new(std::collections::HashSet::default()),
            animator: RefCell::new(crate::anim::Animator::default()),
            root_is_boundary: Cell::new(false),
            printless: Cell::new(false),
        }
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
            crate::viewport::publish(self.last_scrolls.borrow().iter().filter_map(
                |region| {
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
                        },
                    ))
                },
            ));
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
        root.render_into(&self.ctx, &mut nodes);

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
            effects::set_queue(reconciler::assemble_effects(pass_root));
            reconciler::assemble_actions(pass_root);
            reconciler::assemble_editors(pass_root);
            reconciler::assemble_handlers(pass_root);
            reconciler::assemble_contexts(pass_root);
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
        reconciler::run_action(path)
    }

    // MARK: - Pointer (resolved BEFORE layout — the LAW)

    /// The target under the point, against the last layout's hits.
    fn hover_target(&self, x: Px, y: Px) -> Option<String> {
        crate::layout::hit_test(&self.last_hits.borrow(), x, y).map(str::to_string)
    }

    /// Pointer moved. `true` = the visible state changed (the shell
    /// repaints). During a press, hover only re-resolves against the
    /// pressed target: dragging out drops the visual, coming back
    /// re-arms it (AppKit).
    pub fn pointer_moved(&self, x: Px, y: Px) -> bool {
        let target = self.hover_target(x, y);
        let mut interaction = self.interaction.borrow_mut();
        let hovered = match &interaction.pressed {
            Some(pressed) => target.filter(|candidate| candidate == pressed),
            None => target,
        };
        let changed = interaction.hovered != hovered;
        interaction.pointer = Some(Point { x, y });
        interaction.hovered = hovered;
        changed
    }

    /// Button down: ARMS pressed on the target under the point — no
    /// action fires here (up-inside is button semantics). `true` =
    /// repaint.
    pub fn pointer_pressed(&self, x: Px, y: Px) -> bool {
        let target = self.hover_target(x, y);
        let mut interaction = self.interaction.borrow_mut();
        interaction.pointer = Some(Point { x, y });
        let changed = interaction.pressed != target || interaction.hovered != target;
        interaction.hovered = target.clone();
        interaction.pressed = target;
        changed
    }

    /// Button up: fires the action IF released inside the pressed
    /// target — or FOCUSES, if the target is a text field. Releasing
    /// outside any field drops focus (first responder follows the
    /// click). Returns the fired/focused path; the pressed visual
    /// always clears.
    pub fn pointer_released(&self, x: Px, y: Px) -> Option<String> {
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
            Some(path) if reconciler::has_editor(&path) => {
                self.focus_at(&path, x);
                Some(path)
            }
            Some(path) if self.activate(&path) => {
                self.blur();
                Some(path)
            }
            _ => {
                self.blur();
                None
            }
        }
    }

    /// The pointer left the window: clears hover (an in-flight press
    /// already had its visual dropped by the drag's `pointer_moved`).
    pub fn pointer_exited(&self) -> bool {
        let mut interaction = self.interaction.borrow_mut();
        let changed = interaction.hovered.is_some();
        interaction.hovered = None;
        interaction.pointer = None;
        changed
    }

    /// Snapshot of the pointer state — the shell cursor and the asserts.
    pub fn interaction(&self) -> Interaction {
        self.interaction.borrow().clone()
    }

    // MARK: - Scrolling (offset is ENGINE state: no view invalidates)

    /// Routes the wheel to the innermost region under the point WITH
    /// travel on the delta's axis. AppKit convention: positive delta
    /// reveals content above — the offset shrinks. `true` = it moved
    /// (the shell repaints; no render: zero bodies).
    pub fn wheel(&self, x: Px, y: Px, dx: Px, dy: Px) -> bool {
        let scrolls = self.last_scrolls.borrow();
        let travel = |region: &ScrollRegion| {
            let max_x =
                (region.content.width.round() - region.frame.size.width.round()).max(0.0);
            let max_y =
                (region.content.height.round() - region.frame.size.height.round()).max(0.0);
            (max_x, max_y)
        };
        let Some(region) = scrolls.iter().find(|region| {
            region.frame.contains(x, y) && {
                let (max_x, max_y) = travel(region);
                (dx != 0.0 && max_x > 0.0) || (dy != 0.0 && max_y > 0.0)
            }
        }) else {
            return false;
        };
        let (max_x, max_y) = travel(region);
        // the wheel is sovereign: a reveal in flight dies on the spot
        self.animator.borrow_mut().cancel_scroll(&region.path);
        let mut offsets = self.scroll_offsets.borrow_mut();
        let current = offsets.get(&region.path).copied().unwrap_or_default();
        let next = Point {
            x: (current.x - dx).clamp(0.0, max_x),
            y: (current.y - dy).clamp(0.0, max_y),
        };
        let moved = next != current;
        if moved {
            offsets.insert(region.path.clone(), next);
        }
        moved
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

    /// The binding for the pattern: ACTIVE scoped contexts first (a
    /// mounted `.key_context` turns its bindings on), the global map as
    /// the fallback.
    pub fn match_key(&self, pattern: &KeyPattern) -> Option<ActionId> {
        let scoped = self.scoped_keymap.borrow();
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

    /// Fires the innermost live handler. `false` = no handler mounted —
    /// the caller decides the fallback (the gate lets the key continue
    /// on to the field).
    pub fn dispatch_action(&self, id: ActionId) -> bool {
        reconciler::run_handler(id)
    }

    // MARK: - Focus and keyboard (the focused field owns the keyboard)

    pub fn focused(&self) -> Option<String> {
        self.focus.borrow().clone()
    }

    /// Focuses a field. The caret goes to the END the first time (the
    /// stamp's clamp resolves the `usize::MAX`); refocusing restores
    /// the retained position.
    pub fn focus(&self, path: &str) {
        self.caret_visible.set(true);
        *self.focus.borrow_mut() = Some(path.to_string());
        self.carets
            .borrow_mut()
            .entry(path.to_string())
            .or_insert(CaretState { caret: usize::MAX, anchor: None, marked: None });
    }

    /// Focuses and places the caret from the click's X — prefix
    /// measurement with the field's effective FONT (retained from the
    /// last layout).
    fn focus_at(&self, path: &str, x: Px) {
        self.caret_visible.set(true);
        *self.focus.borrow_mut() = Some(path.to_string());
        let mut probe = CaretState::default();
        let text = match reconciler::run_editor(path, EditCommand::Read, &mut probe) {
            Some(Some(text)) => text,
            _ => {
                self.carets
                    .borrow_mut()
                    .entry(path.to_string())
                    .or_insert(CaretState { caret: usize::MAX, anchor: None, marked: None });
                return;
            }
        };
        let placement = self
            .last_fields
            .borrow()
            .iter()
            .find(|field| field.path == path)
            .cloned();
        let caret = match placement {
            Some(field) => {
                caret_from_x(&text, x - field.text_origin.x, &field.font, &*self.text, &self.cache)
            }
            None => text.len(),
        };
        self.carets
            .borrow_mut()
            .insert(path.to_string(), CaretState { caret, anchor: None, marked: None });
    }

    pub fn blur(&self) -> bool {
        self.focus.borrow_mut().take().is_some()
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

    /// Applies an edit command to the focused field. The binding write
    /// already dirtied whoever reads; typing returns the caret to solid.
    pub fn key(&self, command: EditCommand) -> Edited {
        let Some(path) = self.focus.borrow().clone() else {
            return Edited { applied: false, output: None };
        };
        let mut state = self.carets.borrow().get(&path).copied().unwrap_or_default();
        // outside the map borrow: the editor writes to the binding and
        // can re-enter the runtime
        match reconciler::run_editor(&path, command, &mut state) {
            Some(output) => {
                self.carets.borrow_mut().insert(path, state);
                self.caret_visible.set(true);
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
            && self.pointer_moved(point.x, point.y)
        {
            result = self.layout(root, crate::layout::Proposal::exact(size));
        }
        result.display
    }

    /// Advances the retained animations by `dt` seconds. `true` = a
    /// value moved and the frame must repaint. With nothing animating
    /// the call is free — the shell pauses its frame driver while this
    /// stays false.
    pub fn tick(&self, dt: f64) -> bool {
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
        self.animator.borrow().wants_frame()
    }

    /// Accessibility: on, every animation completes instantly. The
    /// shell mirrors the system setting; an app may also set it.
    pub fn set_reduce_motion(&self, on: bool) {
        self.animator.borrow_mut().set_reduce_motion(on);
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
            && self.pointer_moved(point.x, point.y)
        {
            result = self.layout(root, crate::layout::Proposal::exact(size));
        }
        result.display
    }

    /// The text engine of this runtime — the shell pairs it with its
    /// retained paint surface.
    pub fn text(&self) -> Rc<dyn TextEngine> {
        Rc::clone(&self.text)
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

    /// The FRAME-path pass: identical to the print one, but the printed
    /// tree's lines are not even formatted (printing is for people;
    /// frames are for pixels).
    fn frame_pass(&self, root: &impl View) -> NodeList {
        crate::view::set_print(false);
        self.printless.set(true);
        let nodes = self.render_pass(root);
        crate::view::set_print(true);
        nodes
    }

    /// Layout of the current frame: runs one pass (incremental — stable
    /// tree = zero bodies), expands the retained layout tree, and
    /// answers the proposal with the frames by identity.
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
            let moved = self.apply_scroll_targets(&result);
            let focused = self.apply_auto_focus(&result);
            // a miss measured on the round a target just moved is
            // spurious — it audited the PRE-jump offset; the relayout
            // below re-audits against the real one
            let missed = if moved {
                false
            } else {
                self.invalidate_window_misses(&result)
            };
            if !moved && !focused && !missed {
                break;
            }
            result = self.layout_once(root, proposal);
        }
        result
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
        self.carets.borrow_mut().retain(|path, _| reconciler::has_editor(path));
        self.auto_focused.borrow_mut().retain(|path| reconciler::has_editor(path));
        let focus_died = self
            .focus
            .borrow()
            .as_deref()
            .is_some_and(|path| !reconciler::has_editor(path));
        if focus_died {
            *self.focus.borrow_mut() = None;
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
        false
    }

    fn layout_once(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
    ) -> crate::layout::LayoutResult {
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
        // pass's touches mark who is still mounted
        self.animator.borrow_mut().note_place();
        let offsets = self.scroll_offsets.borrow();
        let result = crate::layout::layout_with(
            &tree,
            proposal,
            LayoutEnv {
                text: &*self.text,
                cache: &self.cache,
                scroll_offsets: &offsets,
                font: FontSpec::DEFAULT,
                stamp,
                animator: Some(&self.animator),
                anim: None,
            },
        );
        drop(offsets);
        drop(carets);
        *self.last_hits.borrow_mut() = result.hits.clone();
        *self.last_scrolls.borrow_mut() = result.scrolls.clone();
        *self.last_fields.borrow_mut() = result.fields.clone();
        // an applied-target memory whose region left the scene goes
        // with it — live regions keep theirs (the wheel stays sovereign)
        self.scroll_targets
            .borrow_mut()
            .retain(|path, _| result.scrolls.iter().any(|region| region.path == *path));
        result
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
    pub fn paint(&self, root: &impl View, size: crate::layout::Size) -> crate::raster::Bitmap {
        self.paint_at_scale(root, size, 1)
    }

    /// [`Runtime::paint`] at retina: layout in logical points, bitmap
    /// in physical pixels (`size × scale`).
    pub fn paint_at_scale(
        &self,
        root: &impl View,
        size: crate::layout::Size,
        scale: usize,
    ) -> crate::raster::Bitmap {
        let result = self.layout(root, crate::layout::Proposal::exact(size));
        crate::raster::rasterize_with(
            &result.display,
            (size.width.round() as usize) * scale,
            (size.height.round() as usize) * scale,
            scale,
            crate::layout::Color::WHITE,
            &*self.text,
        )
    }

    /// Drains registered effects (`onReceive`, `onChange`, `query`).
    /// Returns whether any of them observed a change.
    pub fn pump(&self) -> bool {
        effects::take().iter().any(|effect| effect(&self.ctx))
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
            let printed = self.render(root);
            // pump first: side effects fired by THIS render's onAppear
            // nodes must be observed before declaring the tree stable
            let observed_change = self.pump();
            if printed == previous && !observed_change && !self.has_pending_dirty() {
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
        for _ in 0..8 {
            self.frame_pass(root);
            let observed_change = self.pump();
            if !observed_change && !self.has_pending_dirty() {
                return;
            }
        }
    }

    fn has_pending_dirty(&self) -> bool {
        match self.last_root.borrow().as_deref() {
            Some(root) => motor::identity::has_dirty_matching(root),
            None => false,
        }
    }
}
