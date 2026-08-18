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

/// The wait between a hover and its bubble — armed by the pointer,
/// aged by one shell tick, shown on the next. No clock in sight.
#[derive(Default)]
struct TooltipLife {
    pending: Option<crate::layout::TooltipRegion>,
    aged: bool,
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
    /// The splits of the last layout — a divider drag maps the pointer
    /// back to a lane extent through this geometry.
    last_splits: RefCell<Vec<crate::layout::SplitPlacement>>,
    /// The app's own boxes from the last layout — an event resolves its
    /// element and its local coordinates through here.
    last_customs: RefCell<Vec<crate::layout::CustomPlacement>>,
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
    /// The pressed-but-not-lifted drag: its builder and where the
    /// press landed. Past the threshold it becomes the live value.
    drag_armed: RefCell<Option<(crate::layout::DragBuilder, Point)>>,
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

    /// Closes every open popover, outermost last — the app-switch
    /// behavior (the desktop shell calls it when the window resigns
    /// key). `true` = something closed and the shell repaints.
    pub fn dismiss_all_overlays(&self) -> bool {
        let paths: Vec<String> = self
            .last_overlays
            .borrow()
            .iter()
            .rev()
            .map(|overlay| overlay.path.clone())
            .collect();
        let mut closed = false;
        for path in paths {
            closed |= reconciler::run_action(&format!("{path}/#dismiss"));
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
        let was_open = self.close_menu();
        let cleared = self.clear_tooltip();
        let region = self
            .last_menus
            .borrow()
            .iter()
            .rev()
            .find(|region| region.rect.contains(x, y))
            .cloned();
        let Some(region) = region else {
            return was_open || cleared;
        };
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

    /// The compatible drop region under a point — topmost first, by
    /// GEOMETRY: a drag lands through every opaque hover gate, which
    /// is the transparent catcher the dock wanted.
    fn drop_at(&self, x: Px, y: Px, value: &dyn std::any::Any) -> Option<crate::layout::DropRegion> {
        self.last_drops
            .borrow()
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
        for (path, rect) in hits.iter().rev() {
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
        self.last_tooltips
            .borrow()
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
        if crate::layout::hit_test(&self.last_hits.borrow(), x, y).is_some() {
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
        let runtime = Runtime {
            ctx,
            last_root: RefCell::new(None),
            last_hits: RefCell::new(Vec::new()),
            interaction: RefCell::new(Interaction::default()),
            text,
            images: Rc::new(RawImages::default()),
            cache: MeasureCache::default(),
            scroll_offsets: RefCell::new(HashMap::default()),
            last_scrolls: RefCell::new(Vec::new()),
            focus: RefCell::new(None),
            carets: RefCell::new(HashMap::default()),
            caret_visible: Cell::new(true),
            last_fields: RefCell::new(Vec::new()),
            last_splits: RefCell::new(Vec::new()),
            last_customs: RefCell::new(Vec::new()),
            theme_version: Cell::new(crate::theme::version()),
            keymap: RefCell::new(HashMap::default()),
            scoped_keymap: RefCell::new(HashMap::default()),
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
            drag_armed: RefCell::new(None),
            drag_value: RefCell::new(None),
            drag_preview: RefCell::new(None),
            tooltip: RefCell::new(TooltipLife::default()),
            last_drag_regions: RefCell::new(Vec::new()),
            last_control_regions: RefCell::new(Vec::new()),
            overlay_bounds: Cell::new(None),
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
                            offsets: region.row_offsets.clone(),
                            applied: applied.get(&region.path).cloned(),
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
            reconciler::assemble_splits(pass_root);
            reconciler::assemble_customs(pass_root);
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
        // a box that took the press owns every move until the release —
        // dragging a selection past the frame is one gesture, not two
        let grabbed = self.interaction.borrow().element_grab.clone();
        if let Some(path) = grabbed {
            self.interaction.borrow_mut().pointer = Some(Point { x, y });
            if let Some(placement) = self.custom_at(&path) {
                let at = Self::local(&placement, x, y);
                let event = crate::custom::ElementEvent::PointerMoved { at, pressed: true };
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
                let event = crate::custom::ElementEvent::PointerMoved { at, pressed: false };
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
        let reached = pointer_main - origin_main;
        let at = match split.unit {
            crate::layout::SeamUnit::Points => reached
                .clamp(split.min_a, (split.room - split.min_b).max(split.min_a)),
            crate::layout::SeamUnit::Fraction => {
                if split.room <= 0.0 {
                    return false;
                }
                (reached / split.room)
                    .clamp(split.min_a, (1.0 - split.min_b).max(split.min_a))
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

    /// The app's box registered at `path` in the last layout.
    fn custom_at(&self, path: &str) -> Option<crate::layout::CustomPlacement> {
        self.last_customs
            .borrow()
            .iter()
            .find(|placement| placement.path == path)
            .cloned()
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
        }
    }

    /// One keystroke offered to the focused box BEFORE the keymap: an
    /// editor owns its arrows, its Enter and its Tab while it has the
    /// keyboard. The answer's `text` is what a copy hands the
    /// platform's clipboard; `handled: false` sends the stroke on to
    /// the app's bindings.
    pub fn key_stroke(&self, pattern: &KeyPattern) -> crate::custom::Response {
        // an open menu takes Escape before anyone — its owner is the
        // runtime, so no reconciler handler could; a live drag is next
        if *pattern == KeyPattern::key(crate::action::Key::Escape)
            && (self.close_menu() || self.cancel_drag())
        {
            return crate::custom::Response::handled();
        }
        let Some(placement) = self.focused_custom() else {
            return crate::custom::Response::ignored();
        };
        let response =
            self.deliver(&placement, crate::custom::ElementEvent::Key(*pattern));
        if response.handled {
            self.caret_visible.set(true);
        }
        response
    }

    /// Button down: ARMS pressed on the target under the point — no
    /// action fires here (up-inside is button semantics). `true` =
    /// repaint.
    pub fn pointer_pressed(&self, x: Px, y: Px) -> bool {
        self.pointer_clicked(x, y, 1)
    }

    /// [`Self::pointer_pressed`] with the platform's click count — the
    /// shells pass it through (AppKit and the browser both count; the
    /// runtime never needs a clock). A box hears it in `PointerDown`.
    pub fn pointer_clicked(&self, x: Px, y: Px, clicks: u8) -> bool {
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
        let source = self
            .last_drag_sources
            .borrow()
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
                })
                .filter(|top| !top.frame.contains(x, y))
                .map(|top| top.path.clone())
        };
        if let Some(path) = outside {
            reconciler::run_action(&format!("{path}/#dismiss"));
            return true;
        }
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
                crate::custom::ElementEvent::PointerDown { at, clicks },
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
                if inside && self.activate(&above) {
                    return Some(above);
                }
            }
            return None;
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

    /// Routes the wheel to the innermost region under the point WITH
    /// travel on the delta's axis. AppKit convention: positive delta
    /// reveals content above — the offset shrinks. `true` = it moved
    /// (the shell repaints; no render: zero bodies).
    pub fn wheel(&self, x: Px, y: Px, dx: Px, dy: Px) -> bool {
        // the content is about to slide under a still pointer — the
        // explanation dies and so does the menu, rather than pointing
        // at the wrong row
        let explained = self.clear_tooltip() | self.close_menu();
        let _ = explained; // folded into the returns below
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
        // each AXIS routes to the innermost region under the point
        // that travels that way — a diagonal gesture over a table
        // scrolls the rows AND slides the columns, instead of losing
        // half of itself inside the inner list
        let region_y = (dy != 0.0)
            .then(|| {
                scrolls.iter().find(|region| {
                    region.frame.contains(x, y) && travel(region).1 > 0.0
                })
            })
            .flatten();
        let region_x = (dx != 0.0)
            .then(|| {
                scrolls.iter().find(|region| {
                    region.frame.contains(x, y) && travel(region).0 > 0.0
                })
            })
            .flatten();
        if region_x.is_none() && region_y.is_none() {
            return false;
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
        let dropped = self.focus.borrow_mut().take();
        // the box hears the keyboard leave — a selection that only
        // means something while focused goes quiet
        if let Some(placement) = dropped.as_deref().and_then(|path| self.custom_at(path)) {
            self.deliver(&placement, crate::custom::ElementEvent::Focused(false));
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
        let Some(path) = self.focus.borrow().clone() else {
            return Edited { applied: false, output: None };
        };
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
            }
            return Edited { applied: response.handled, output: response.text };
        }
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

    /// The frame in DOM mode: the same settle + layout machinery as
    /// [`Runtime::display_frame`], then ONE capture pass over the now-
    /// stable tree and the diff against the retained scene. The result
    /// is the patch list that brings the element tree up to date —
    /// empty when nothing observable changed (a hover, a caret blink).
    ///
    /// The engine never ticks springs here: animation specs lower into
    /// the patches as CSS transitions and the browser animates.
    pub fn dom_frame(
        &self,
        root: &impl View,
        size: crate::layout::Size,
    ) -> Vec<crate::dom::DomPatch> {
        // the pass settles state, applies scroll targets and heals
        // virtual windows; its display list feeds the canvas islands
        let _ = self.display_frame(root, size);
        let (result, scene) = self.layout_once_with(
            root,
            crate::layout::Proposal::exact(size),
            true,
        );
        let scene = scene.expect("the capture rode the pass");
        self.dom.borrow_mut().lower(&scene, &result.display)
    }

    /// The canvas islands whose pixels changed since the last call —
    /// rasterized at `scale` and ready to blit. Empty when the scene
    /// has no islands or nothing inside one moved.
    pub fn dom_islands(&self, scale: usize) -> Vec<crate::dom::IslandFrame> {
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
                let bitmap = crate::raster::rasterize_with(
                    &display,
                    physical.0,
                    physical.1,
                    scale,
                    crate::layout::Color::rgba(0, 0, 0, 0),
                    &*self.text,
                    &*self.images,
                );
                crate::dom::IslandFrame {
                    id,
                    width: physical.0,
                    height: physical.1,
                    rgba: bitmap.to_rgba_bytes(),
                }
            })
            .collect()
    }

    /// The scroll region path behind a Dom element id — the glue's
    /// scroll observer reports by id, the runtime scrolls by path.
    pub fn dom_scroll_path(&self, id: u32) -> Option<String> {
        self.dom.borrow().scroll_path(id)
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
            let moved = self.apply_scroll_targets(&result) | self.apply_element_reveals(&result);
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
                closed |= reconciler::run_action(&format!("{}/#dismiss", overlay.path));
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
        self.carets.borrow_mut().retain(|path, _| reconciler::has_editor(path));
        self.auto_focused.borrow_mut().retain(|path| reconciler::has_editor(path));
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
        self.layout_once_with(root, proposal, false).0
    }

    /// One layout pass, optionally with the Dom capture riding it.
    fn layout_once_with(
        &self,
        root: &impl View,
        proposal: crate::layout::Proposal,
        dom: bool,
    ) -> (crate::layout::LayoutResult, Option<crate::dom::DomNode>) {
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
        let env = LayoutEnv {
            text: &*self.text,
            images: &*self.images,
            cache: &self.cache,
            scroll_offsets: &offsets,
            font: FontSpec::DEFAULT,
            stamp,
            animator: Some(&self.animator),
            anim: None,
            overlay_bounds: self.overlay_bounds.get(),
        };
        let (result, scene) = if dom {
            let (result, scene) = crate::layout::layout_dom(&tree, proposal, env);
            (result, Some(scene))
        } else {
            (crate::layout::layout_with(&tree, proposal, env), None)
        };
        drop(offsets);
        drop(carets);
        *self.last_hits.borrow_mut() = result.hits.clone();
        *self.last_scrolls.borrow_mut() = result.scrolls.clone();
        *self.last_fields.borrow_mut() = result.fields.clone();
        *self.last_splits.borrow_mut() = result.splits.clone();
        *self.last_customs.borrow_mut() = result.customs.clone();
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
            effects::sweep_tasks();
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
        for _ in 0..8 {
            // the same order as the print path: a task that resolved
            // writes its state, then the pass reads it
            self.poll_tasks();
            self.frame_pass(root);
            let observed_change = self.pump();
            effects::sweep_tasks();
            if !observed_change && !self.has_pending_dirty() && !motor::task::has_ready() {
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
