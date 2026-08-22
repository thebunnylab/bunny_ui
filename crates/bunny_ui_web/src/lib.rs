//! The web shell — hand-written wasm FFI, no bindgen, no dependencies.
//!
//! The same premise as every other shell: the engine owns layout and
//! state; the platform delivers events and presents frames. Here the
//! platform is a small `glue.js` written by us: it forwards DOM events
//! into the exports below and applies what the imports hand back. This
//! is the CANVAS mode of the web premise — the display list rasterized
//! by the same `Surface` the desktop uses, blitted with `putImageData`
//! (the RGBA mirror is already in its byte order). The Dom lowering is
//! the next mode; the scene stays semantic either way.
//!
//! One frame driver: the glue's `requestAnimationFrame` plays the role
//! of the display link — armed only while an animation wants frames.
#![cfg(target_arch = "wasm32")]

#[cfg(feature = "gpu")]
mod gpu;
#[cfg(feature = "gpu")]
use gpu as tier;

/// The tier that is not in this build: every door answers no and the
/// branches fold away, so the call sites read the same either way.
#[cfg(not(feature = "gpu"))]
mod tier {
    pub(crate) fn try_install(_kind: u32, _physical: (u32, u32)) -> bool { false }
    pub(crate) fn active() -> bool { false }
    pub(crate) fn teardown() {}
}

mod image;
mod text;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::layout::{Color, Size};
use bunny_ui::action::KeyMatch;
use bunny_ui::prelude::*;
#[cfg(feature = "canvas")]
use bunny_ui::raster::Surface;
use bunny_ui::runtime::Runtime;
#[cfg(feature = "gpu")]
use gpu::gl_now;
use bunny_ui::text_input::EditCommand;

pub use image::CanvasImageEngine;
pub use text::CanvasTextEngine;

#[link(wasm_import_module = "bunny")]
unsafe extern "C" {
    /// The glue paints this RGBA buffer onto the canvas, whole.
    fn js_blit(pointer: *const u8, width: u32, height: u32);
    /// The glue schedules ONE requestAnimationFrame that calls
    /// `bunny_frame` back — the browser's display link.
    fn js_request_frame();
    /// A task woke and asks for one turn: the glue calls `bunny_wake`
    /// back, once, out of the current job. NOT the frame driver — the
    /// tick path never settles, and a task landing is exactly what a
    /// settle is for.
    fn js_request_wake();
    /// Dom mode: the glue walks this patch stream (the fixed
    /// little-endian ABI of `bunny_ui::dom::encode`) and mutates the
    /// element tree.
    fn js_apply_patches(pointer: *const u8, len: usize);
    /// Dom mode: fresh pixels for one canvas island (physical size).
    fn js_island(id: u32, pointer: *const u8, width: u32, height: u32);
    /// A panic, on its way to the console. Without it a wasm abort is one
    /// line of `unreachable` and a stack of numbers.
    fn js_panic(pointer: *const u8, len: usize);
}

/// Sends a panic to the console instead of the bare `unreachable` a wasm
/// abort leaves behind. Installed once at boot, before the first frame,
/// so the first failure names itself.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // keep the format minimal — the allocator may be poisoned
        let message = info.to_string();
        unsafe { js_panic(message.as_ptr(), message.len()) };
    }));
}

/// The glue's key table, mirrored: one number per named key.
fn named_key(code: u32) -> Option<bunny_ui::action::Key> {
    use bunny_ui::action::Key;
    Some(match code {
        1 => Key::Backspace,
        2 => Key::Delete,
        3 => Key::Left,
        4 => Key::Right,
        5 => Key::Home,
        6 => Key::End,
        7 => Key::Escape,
        8 => Key::Up,
        9 => Key::Down,
        10 => Key::Enter,
        11 => Key::Tab,
        12 => Key::PageUp,
        13 => Key::PageDown,
        _ => return None,
    })
}

/// The modifier bits the glue sends: 1 shift, 2 command, 4 option, 8
/// control — the same word for a press as for a stroke, because the
/// glue reads them from the same event fields either way.
fn held(mods: u32) -> bunny_ui::action::Modifiers {
    bunny_ui::action::Modifiers {
        shift: mods & 1 != 0,
        command: mods & 2 != 0,
        option: mods & 4 != 0,
        control: mods & 8 != 0,
    }
}

/// The modifier bits the glue sends: 1 shift, 2 command, 4 option, 8
/// control.
fn pattern(key: bunny_ui::action::Key, mods: u32) -> KeyPattern {
    KeyPattern {
        key,
        shift: mods & 1 != 0,
        command: mods & 2 != 0,
        option: mods & 4 != 0,
        control: mods & 8 != 0,
    }
}

/// One stroke, in the desktop gate's order: the focused escape hatch
/// first, then the keymap, then the field's editing vocabulary. `true`
/// = something changed and the frame is worth presenting.
fn stroke(runtime: &Runtime, pattern: KeyPattern) -> bool {
    use bunny_ui::action::Key;
    if runtime.key_stroke(&pattern).handled {
        return true;
    }
    // a field of MANY lines owns the bare break and the bare vertical
    // arrows, before any binding — and only it: a one-line field
    // declines and the stroke walks on, so the app keeps its Enter and
    // a list keeps its arrows
    let mid_chord = !runtime.pending_chord().is_empty();
    if !mid_chord
        && pattern.is_plain()
        && let Some(command) = match pattern.key {
            Key::Enter => Some(EditCommand::Newline),
            Key::Up => Some(EditCommand::Up(pattern.shift)),
            Key::Down => Some(EditCommand::Down(pattern.shift)),
            _ => None,
        }
        && runtime.key(command).applied
    {
        return true;
    }
    // typing is never stolen by a binding — but only whoever holds the
    // keyboard AND is taking text is typing (a modal box in command
    // mode declines), and MID-CHORD the keyboard belongs to the
    // keymap: the stroke that finishes `cmd-k s` is not typing
    let typing = !mid_chord && runtime.focus_takes_text() && pattern.is_text_input();
    if !typing {
        match runtime.chord(&pattern) {
            KeyMatch::Action(action) if runtime.dispatch_action(action) => return true,
            // the stroke opened (or let go of) a sequence: it is spent,
            // and a which-key panel may have just changed
            KeyMatch::Pending => return true,
            _ => {}
        }
    }
    let edit = match pattern.key {
        Key::Backspace => Some(EditCommand::Backspace),
        Key::Delete => Some(EditCommand::Delete),
        Key::Left => Some(EditCommand::Left(pattern.shift)),
        Key::Right => Some(EditCommand::Right(pattern.shift)),
        Key::Home => Some(EditCommand::Home(pattern.shift)),
        Key::End => Some(EditCommand::End(pattern.shift)),
        Key::Char('a') if pattern.command => Some(EditCommand::SelectAll),
        Key::Escape => return runtime.blur(),
        _ => None,
    };
    match edit {
        Some(edit) => runtime.key(edit).applied,
        None => false,
    }
}

/// What the exports feed the shell — the web twin of the mac AppEvent.
enum Event {
    PointerMove { x: f64, y: f64, modifiers: bunny_ui::action::Modifiers },
    PointerDown { x: f64, y: f64, clicks: u8, modifiers: bunny_ui::action::Modifiers },
    PointerUp { x: f64, y: f64 },
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    Text(String),
    /// A named key from the glue's table, with the modifier bits.
    Key(u32, u32),
    /// A character stroke (the modifiers decide whether it types or
    /// commands).
    KeyChar(char, u32),
    Frame { dt: f64 },
    /// The platform's motion preference, at boot and on every change.
    Motion { allowed: bool },
    Resize { width: f64, height: f64, scale: f64 },
    /// The browser finished decoding a registered image — measure and
    /// paint can answer for real now.
    ImageReady,
    /// A task has something to run: a fetch came back, a callback fired.
    Wake,
    /// The glue's slow clock beat once — the tooltip ages, then shows.
    TooltipTick,
    /// A right press (the browser's contextmenu, default prevented).
    ContextClick { x: f64, y: f64 },
    /// Dom mode: the browser's scroll observer — the element scrolled
    /// and the engine mirrors the offset (the dual ownership).
    DomScroll { id: u32, x: f64, y: f64 },
    DomViewport { id: u32, width: f64, height: f64 },
    DomBox { id: u32, width: f64, height: f64 },
    IslandPointer { id: u32, kind: u32, x: f64, y: f64, mods: u32 },
    Action { path: String, clicks: u8 },
    /// Dom mode: the browser's input edited — value + selectionStart.
    Field { path: String, value: String, caret: usize },
}

struct Shell {
    handle: Box<dyn FnMut(Event)>,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    /// Did the last press arm a drag? The element mode's glue reads it
    /// right after a press and opens its pointer-move door only then.
    static DRAG_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The running click count, `(when, x, y, count)`. The browser
    /// counts on `mousedown` and NOT on `pointerdown` (which reports
    /// `detail` zero), and the glue listens on `pointerdown` so touch
    /// and pen keep working — so the shell counts, exactly like the
    /// Windows one does over its own message stream.
    static CLICK_STATE: std::cell::Cell<(f64, f64, f64, u8)> =
        const { std::cell::Cell::new((f64::NEG_INFINITY, 0.0, 0.0, 0)) };
}

/// How long after a press a second one is still the SAME gesture. The
/// browser exposes no system preference for it; a half second is what
/// Win32 defaults to.
const DOUBLE_CLICK_MS: f64 = 500.0;
/// How far the pointer may travel and still count as the same spot.
const CLICK_TRAVEL: f64 = 4.0;

/// The platform's click count, counted here: 1, then 2 on a second
/// press inside the window and the travel box, then 3. Pure over its
/// state, so the rule is testable without a browser.
fn count_click(state: (f64, f64, f64, u8), x: f64, y: f64, now_ms: f64) -> (f64, f64, f64, u8) {
    let (last, last_x, last_y, count) = state;
    let near = (x - last_x).abs() <= CLICK_TRAVEL && (y - last_y).abs() <= CLICK_TRAVEL;
    let clicks = if now_ms - last <= DOUBLE_CLICK_MS && near {
        count.saturating_add(1)
    } else {
        1
    };
    (now_ms, x, y, clicks)
}

fn dispatch(event: Event) {
    // Take the shell OUT of the cell for the length of the event, so the
    // borrow is released before the handler runs. A `bunny_*` export that
    // fires while this one is on the stack then finds `None` and no-ops,
    // instead of a second `borrow_mut` that aborts — and, aborting, never
    // releases the first, wedging every later call. A re-entrant event is
    // dropped, not queued; the wake path already defers off the stack, so
    // nothing on the normal road re-enters.
    let taken = SHELL.with(|slot| slot.borrow_mut().take());
    if let Some(mut shell) = taken {
        (shell.handle)(event);
        // Put it back, unless the handler booted a fresh shell in place.
        SHELL.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(shell);
            }
        });
    }
}

/// Boots the shell with the app's root view. The demo crate calls this
/// from its exported `start`; everything after travels through events.
#[cfg(feature = "canvas")]
pub fn start(width: f64, height: f64, scale: f64, root: impl View + 'static) {
    install_panic_hook();
    let runtime = Runtime::new()
        .text_engine(Rc::new(CanvasTextEngine::new()))
        .image_engine(Rc::new(CanvasImageEngine::new()));
    // a task that woke asks the page for one turn — the browser's
    // answer to the desktop's run loop source
    runtime.set_wake_hook(std::sync::Arc::new(|| unsafe { js_request_wake() }));
    let mut size = Size { width, height };
    // the surface wants an INTEGER scale (the snapping contract);
    // fractional device ratios round to the nearest whole step
    let mut scale = (scale.round() as usize).max(1);
    // a box that draws parts which TOUCH puts the shared edge on a
    // whole PIXEL — it needs the screen's scale
    runtime.set_device_scale(scale as f64);
    // the tier comes up before the first frame, and the first frame is
    // already the one the page keeps
    let physical = (
        ((size.width.round() as usize) * scale).max(1) as u32,
        ((size.height.round() as usize) * scale).max(1) as u32,
    );
    tier::try_install(0, physical);
    // stays None on the GPU road: a page presenting by GPU never
    // allocates the CPU bitmap at all
    let mut surface: Option<(Surface, usize, Color)> = None;

    let present = move |runtime: &Runtime,
                            root: &dyn Fn(&Runtime, Size) -> bunny_ui::layout::DisplayList,
                            size: Size,
                            scale: usize,
                            surface: &mut Option<(Surface, usize, Color)>| {
        let canvas = bunny_ui::theme::canvas();
        let physical =
            ((size.width.round() as usize) * scale, (size.height.round() as usize) * scale);
        let display = root(runtime, size);
        #[cfg(feature = "gpu")]
        if tier::active() {
            // the same display list, no Surface in the path — the frame
            // IS the drawable
            tier::present_window(
                None,
                &display,
                size,
                scale,
                canvas,
                &*runtime.text(),
                &*runtime.images(),
            );
            if runtime.wants_frame() {
                unsafe { js_request_frame() };
            }
            return;
        }
        let stale = match &*surface {
            Some((retained, kept_scale, kept_canvas)) => {
                retained.bitmap().width() != physical.0
                    || retained.bitmap().height() != physical.1
                    || *kept_scale != scale
                    || *kept_canvas != canvas
            }
            None => true,
        };
        if stale {
            *surface = Some((Surface::new(physical.0, physical.1, scale, canvas), scale, canvas));
        }
        let (retained, _, _) = surface.as_mut().expect("surface for the frame");
        let damage = retained.frame(display, &*runtime.text(), &*runtime.images());
        if !damage.is_empty() || stale {
            let (width, height) = (retained.bitmap().width(), retained.bitmap().height());
            let rgba = retained.rgba();
            unsafe { js_blit(rgba.as_ptr(), width as u32, height as u32) };
        }
        if runtime.wants_frame() {
            unsafe { js_request_frame() };
        }
    };

    let handle = Box::new(move |event: Event| {
        let full =
            |runtime: &Runtime, size: Size| runtime.display_frame(&root, size);
        let tick =
            |runtime: &Runtime, size: Size| runtime.animation_frame(&root, size);
        match event {
            Event::PointerMove { x, y, modifiers } => {
                if runtime.pointer_moved(x, y, modifiers) {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::PointerDown { x, y, clicks, modifiers } => {
                if runtime.pointer_clicked(x, y, clicks, modifiers) {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::PointerUp { x, y } => {
                let _ = runtime.pointer_released(x, y);
                present(&runtime, &full, size, scale, &mut surface);
            }
            Event::Wheel { x, y, dx, dy } => {
                // browser deltas are the OPPOSITE of the engine's
                // convention (positive reveals content above) — the
                // sign flips here, once
                if runtime.wheel(x, y, -dx, -dy) {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::Text(text) => {
                if runtime.key(EditCommand::Insert(text)).applied {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::KeyChar(character, mods) => {
                if stroke(&runtime, pattern(bunny_ui::action::Key::Char(character.to_ascii_lowercase()), mods)) {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::Key(code, mods) => {
                let Some(key) = named_key(code) else {
                    return;
                };
                if stroke(&runtime, pattern(key, mods)) {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::Frame { dt } => {
                if runtime.tick(dt).any() {
                    present(&runtime, &tick, size, scale, &mut surface);
                } else if runtime.wants_frame() {
                    unsafe { js_request_frame() };
                }
            }
            Event::Resize { width, height, scale: ratio } => {
                size = Size { width, height };
                scale = (ratio.round() as usize).max(1);
                runtime.set_device_scale(scale as f64);
                present(&runtime, &full, size, scale, &mut surface);
            }
            Event::TooltipTick => {
                // the same slow beat ages a sequence in the air: two
                // ticks and `cmd-k` lets the keyboard go
                if runtime.tooltip_tick() | runtime.chord_tick() {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::ContextClick { x, y } => {
                if runtime.context_click(x, y) {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            // the canvas shell drives every animation itself, so the
            // reader's preference is the whole switch
            Event::Motion { allowed } => {
                runtime.set_reduce_motion(!allowed);
                if runtime.wants_frame() {
                    unsafe { js_request_frame() };
                }
            }
            Event::ImageReady | Event::Wake => {
                // the layout reflows around the fresh intrinsic size
                // (or around what a task just wrote) and the paint asks
                // the engine again — one full frame, settle included
                present(&runtime, &full, size, scale, &mut surface);
            }
            // Dom-mode traffic — this shell rasterizes, nothing to do
            Event::DomScroll { .. }
            | Event::DomViewport { .. }
            | Event::DomBox { .. }
            | Event::IslandPointer { .. }
            | Event::Action { .. }
            | Event::Field { .. } => {}
        }
    });
    SHELL.with(|slot| {
        *slot.borrow_mut() = Some(Shell { handle });
    });
    dispatch(Event::Resize { width, height, scale: scale as f64 });
}

/// Boots the DOM mode: the same scene lowers to element patches and
/// the browser renders at home — native text selection, momentum
/// scroll, the platform's own input. The engine never ticks springs
/// here (reduce-motion on): animation specs lower to CSS transitions
/// and programmatic scrolls ride `scroll-behavior`, so the browser
/// animates while the engine stays event-driven.
pub fn start_dom(width: f64, height: f64, scale: f64, root: impl View + 'static) {
    start_dom_with(width, height, scale, false, root)
}

/// [`start_dom`] over a page the BUILD already painted: the runtime
/// adopts the served elements as its retained truth, and the first
/// frame says nothing — only the islands blit their first pixels.
pub fn start_dom_hydrated(width: f64, height: f64, scale: f64, root: impl View + 'static) {
    start_dom_with(width, height, scale, true, root)
}

fn start_dom_with(
    width: f64,
    height: f64,
    scale: f64,
    hydrate: bool,
    root: impl View + 'static,
) {
    install_panic_hook();
    let runtime = Runtime::new()
        .text_engine(Rc::new(CanvasTextEngine::new()))
        .image_engine(Rc::new(CanvasImageEngine::new()));
    // a task that woke asks the page for one turn — the browser's
    // answer to the desktop's run loop source
    runtime.set_wake_hook(std::sync::Arc::new(|| unsafe { js_request_wake() }));
    // the browser animates the SPRINGS (a spec lowers to a CSS
    // transition, a programmatic scroll rides `scroll-behavior`), so ours
    // stay silent. The loop clocks are the motion nothing else drives —
    // they start silent too, and the page turns them on through
    // `bunny_set_motion` once it has asked the platform whether motion is
    // welcome here.
    runtime.set_motion(true, true);
    // one context for every island on the page. A canvas per island
    // would hit the browser's context ceiling, and an island that
    // claimed webgl2 could never take putImageData back when the
    // context is lost — the element itself keeps its 2d road.
    tier::try_install(1, (1, 1));
    let mut size = Size { width, height };
    let scale = (scale.round() as usize).max(1);
    if hydrate {
        // the page shipped painted: adopt it, and let the first frame
        // agree in silence
        runtime.dom_adopt(&root, size);
    }

    // patches first, then any island whose pixels changed — the
    // element exists before its bitmap arrives
    fn present(runtime: &Runtime, patches: Vec<bunny_ui::dom::DomPatch>, scale: usize) {
        if !patches.is_empty() {
            let bytes = bunny_ui::dom::encode(&patches);
            unsafe { js_apply_patches(bytes.as_ptr(), bytes.len()) };
        }
        #[cfg(all(feature = "canvas", feature = "gpu"))]
        if tier::active() {
            // the dirty marks are CONSUMED by whichever road reads
            // them, so the branch is here and never inside the loop
            let mut lists = runtime.dom_island_lists(scale);
            // one backing resize per distinct SIZE, not per island
            lists.sort_by_key(|island| (island.width, island.height));
            for island in lists {
                let physical = (island.width as u32, island.height as u32);
                let size = bunny_ui::layout::Size {
                    width: island.width as f64 / scale as f64,
                    height: island.height as f64 / scale as f64,
                };
                tier::present_window(
                    Some((island.id, physical)),
                    &island.display,
                    size,
                    scale,
                    // an island sits OVER the page's own elements: it
                    // clears to nothing, never to the theme's canvas
                    Color::rgba(0, 0, 0, 0),
                    &*runtime.text(),
                    &*runtime.images(),
                );
            }
            return;
        }
        #[cfg(feature = "canvas")]
        for island in runtime.dom_islands(scale) {
            unsafe {
                js_island(
                    island.id,
                    island.rgba.as_ptr(),
                    island.width as u32,
                    island.height as u32,
                );
            }
        }
        #[cfg(not(feature = "canvas"))]
        let _ = scale;
    }

    let handle = Box::new(move |event: Event| {
        match event {
            Event::PointerDown { x, y, clicks, modifiers } => {
                let _ = runtime.pointer_clicked(x, y, clicks, modifiers);
                // the glue asks this next: a press on a drag source is
                // the ONLY thing that opens the move door in this mode
                DRAG_ARMED.with(|armed| armed.set(runtime.drag_armed()));
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::PointerUp { x, y } => {
                let _ = runtime.pointer_released(x, y);
                DRAG_ARMED.with(|armed| armed.set(false));
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::DomScroll { id, x, y } => {
                // the browser moved: the offset folds into the engine
                // AND the retained scene (the diff meets its own echo),
                // and the region's window body re-runs
                runtime.dom_scrolled(id, x, y);
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::DomViewport { id, width, height } => {
                // stored for the next window math — no frame of its own
                runtime.set_dom_viewport(id, width, height);
            }
            Event::DomBox { id, width, height } => {
                // a flexible island's real box — news re-measures the
                // island against it; an echo costs nothing
                if runtime.dom_island_box(id, width, height) {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            Event::IslandPointer { id, kind, x, y, mods } => {
                // the canvas's own coordinates, routed to the app's
                // box under the point — the pixels follow its answer
                if runtime.dom_island_pointer(id, kind, x, y, held(mods)) {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            Event::Action { path, clicks } => {
                // the browser resolved the press; the engine runs the
                // handler and the frame follows
                runtime.dom_action(&path, clicks);
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::Field { path, value, caret } => {
                if runtime.sync_field(&path, &value, caret) {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            Event::Resize { width, height, .. } => {
                size = Size { width, height };
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::TooltipTick => {
                // the same slow beat ages a sequence in the air: two
                // ticks and `cmd-k` lets the keyboard go
                if runtime.tooltip_tick() | runtime.chord_tick() {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            Event::ContextClick { x, y } => {
                if runtime.context_click(x, y) {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            Event::ImageReady | Event::Wake => {
                // geometry reflows around the fresh intrinsic size (or
                // around what a task just wrote); the <img> elements
                // themselves paint on their own
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::Key(code, mods) => {
                // the browser owns the <input>s here; the strokes that
                // matter to US are the focused island's and the
                // keymap's (Escape dismisses a popover through it)
                let Some(key) = named_key(code) else {
                    return;
                };
                if stroke(&runtime, pattern(key, mods)) {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            Event::KeyChar(character, mods) => {
                if stroke(&runtime, pattern(bunny_ui::action::Key::Char(character.to_ascii_lowercase()), mods)) {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            Event::Text(text) => {
                // a focused island types through the same door the
                // desktop uses; a field's text never comes this way
                // (the <input> owns it and syncs through bunny_field)
                if runtime.key(EditCommand::Insert(text)).applied {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
            }
            // a pointer MOVE reaches us in this mode only between a
            // press that armed a drag and its release (the glue opens
            // the door and closes it) — so a drag works here too and
            // the zero-patch hover stays untouched, by construction
            Event::PointerMove { x, y, modifiers } => {
                if runtime.pointer_moved(x, y, modifiers) {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
                DRAG_ARMED.with(|armed| armed.set(runtime.drag_armed()));
            }
            // The reader's own answer about motion. Springs stay the
            // browser's; the loops are ours, so they follow this.
            Event::Motion { allowed } => {
                runtime.set_motion(true, !allowed);
                if runtime.wants_frame() {
                    unsafe { js_request_frame() };
                }
            }
            // A loop step: the clocks advance and the live boxes
            // repaint on their own canvases. The scene is untouched —
            // no body, no patch, no settle — which is what lets a
            // decoration tick beside real elements without the page
            // re-laying itself out around it.
            Event::Frame { dt } => {
                let moved = runtime.tick(dt);
                if moved.islands {
                    #[cfg(feature = "canvas")]
                    for island in runtime.dom_islands(scale) {
                        unsafe {
                            js_island(
                                island.id,
                                island.rgba.as_ptr(),
                                island.width as u32,
                                island.height as u32,
                            );
                        }
                    }
                }
                // a scene that moved is a real frame — but in this mode
                // only the clocks tick, so this is the rare road
                if moved.scene {
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
                if runtime.wants_frame() {
                    unsafe { js_request_frame() };
                }
            }
            // hover, wheel and the rest belong to the browser in this
            // mode — nothing to do on our side of the border
            _ => {}
        }
    });
    SHELL.with(|slot| {
        *slot.borrow_mut() = Some(Shell { handle });
    });
    dispatch(Event::Resize { width, height, scale: 1.0 });
}

// MARK: - Exports (the glue's side of the border)

/// The glue asks for wasm memory to write a UTF-8 string into; the
/// receiving export takes the ownership back — no free needed.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len.max(1));
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer
}

#[unsafe(no_mangle)]
pub extern "C" fn bunny_text(pointer: *mut u8, len: usize) {
    let text = unsafe { String::from_raw_parts(pointer, len, len.max(1)) };
    dispatch(Event::Text(text));
}

/// `mods` is the same four bits a press and a stroke carry: 1 shift,
/// 2 command, 4 option, 8 control. A move says what the hand HOLDS, so
/// a box can offer a command-click before the hand commits to it.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_pointer_move(x: f64, y: f64, mods: u32) {
    dispatch(Event::PointerMove { x, y, modifiers: held(mods) });
}

/// One beat of the glue's slow clock: the tooltip ages, then shows.
/// The glue arms two of these after a pointer settles — the runtime
/// no-ops the strays.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_tooltip_tick() {
    dispatch(Event::TooltipTick);
}

/// Did the last press arm a drag? The element mode's glue asks this
/// to decide whether to listen for moves at all.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_drag_armed() -> u32 {
    DRAG_ARMED.with(|armed| armed.get() as u32)
}

/// The browser's contextmenu, default prevented by the glue.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_context_click(x: f64, y: f64) {
    dispatch(Event::ContextClick { x, y });
}

#[unsafe(no_mangle)]
pub extern "C" fn bunny_pointer_down(x: f64, y: f64, time_ms: f64, button: u32, mods: u32) {
    // the glue hands the event's own timestamp and which button it was;
    // the count is the shell's to keep, because `pointerdown` carries
    // no count of its own. Only the PRIMARY button counts, the way
    // AppKit and Win32 count — a right press between two left ones must
    // not turn the second into a double.
    let clicks = if button == 0 {
        let next = CLICK_STATE.with(|state| {
            let next = count_click(state.get(), x, y, time_ms);
            state.set(next);
            next
        });
        next.3
    } else {
        1
    };
    dispatch(Event::PointerDown { x, y, clicks, modifiers: held(mods) });
}

#[unsafe(no_mangle)]
pub extern "C" fn bunny_pointer_up(x: f64, y: f64) {
    dispatch(Event::PointerUp { x, y });
}

#[unsafe(no_mangle)]
pub extern "C" fn bunny_wheel(x: f64, y: f64, dx: f64, dy: f64) {
    dispatch(Event::Wheel { x, y, dx, dy });
}

/// One named key: `code` from the glue's table (mirrored in
/// [`named_key`]), `mods` as the bit flags 1 shift, 2 command, 4
/// option, 8 control.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_key(code: u32, mods: u32) {
    dispatch(Event::Key(code, mods));
}

/// One character stroke — the code point plus the same modifier bits.
/// Plain typing does NOT come through here: it is text (`bunny_text`),
/// so a composition and a paste take the same road.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_key_char(code_point: u32, mods: u32) {
    if let Some(character) = char::from_u32(code_point) {
        dispatch(Event::KeyChar(character, mods));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn bunny_frame(dt: f64) {
    dispatch(Event::Frame { dt: dt.clamp(0.0, 1.0 / 30.0) });
}

#[unsafe(no_mangle)]
pub extern "C" fn bunny_resize(width: f64, height: f64, scale: f64) {
    dispatch(Event::Resize { width, height, scale });
}

/// Dom mode: a scroll element moved (by finger, wheel or momentum) —
/// reported by element id, in logical px.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_dom_scroll(id: u32, x: f64, y: f64) {
    dispatch(Event::DomScroll { id, x, y });
}

/// The browser decoded a registered image (the async half of the
/// image engine) — the shell repaints so measure and paint see it.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_image_ready(_key_hi: u32, _key_lo: u32) {
    dispatch(Event::ImageReady);
}

/// A task is ready to run: the app's own callback (a fetch that came
/// back, a socket message) sent on a channel, and the glue delivers
/// this out of that job. The turn drains the queue on its way.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_wake() {
    dispatch(Event::Wake);
}

/// Dom mode: the browser resolved a click to the nearest interactive
/// path — no coordinates cross the border in this mode.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_action(pointer: *mut u8, len: usize, clicks: u32) {
    let path = unsafe { String::from_raw_parts(pointer, len, len.max(1)) };
    // the browser counted the press for us: a `click` carries its own
    // detail, so a double never needs a clock on this side
    dispatch(Event::Action { path, clicks: clicks.max(1).min(255) as u8 });
}

/// Dom mode: a scroll box resized (the glue's ResizeObserver) — the
/// window math reads the real box next frame.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_dom_viewport(id: u32, width: f64, height: f64) {
    dispatch(Event::DomViewport { id, width, height });
}

/// Dom mode: a canvas island's resize observer fired — the box the
/// browser really gave the element.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_dom_box(id: u32, width: f64, height: f64) {
    dispatch(Event::DomBox { id, width, height });
}

/// Dom mode: a pointer event on a canvas island, in the canvas's own
/// coordinates (`kind`: 0 down, 1 move, 2 up).
#[unsafe(no_mangle)]
pub extern "C" fn bunny_island_pointer(id: u32, kind: u32, x: f64, y: f64, mods: u32) {
    dispatch(Event::IslandPointer { id, kind, x, y, mods });
}

/// The wire contract this binary encodes. The glue reads it before it
/// boots and refuses a stream it was not written for — see
/// `bunny_ui::dom::ABI_VERSION` for the bump checklist.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_abi_version() -> u32 {
    bunny_ui::dom::ABI_VERSION
}

/// Is motion welcome on this page? The glue reads the PLATFORM's own
/// answer (`prefers-reduced-motion`) at boot and again whenever the
/// viewer changes it, so a decoration that loops is the reader's choice
/// and not the shell's guess.
///
/// The springs stay the browser's either way — in the element lowering
/// an animation spec is a CSS transition, and driving it here as well
/// would animate everything twice. This turns the LOOP clocks on: the
/// motion nothing else drives.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_set_motion(allowed: u32) {
    dispatch(Event::Motion { allowed: allowed != 0 });
}

/// Dom mode: the input edited. Both strings arrive through
/// `bunny_alloc` buffers (ownership comes back here); `caret` is the
/// input's `selectionStart` in UTF-16 units.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_field(
    path_pointer: *mut u8,
    path_len: usize,
    value_pointer: *mut u8,
    value_len: usize,
    caret: usize,
) {
    let path = unsafe { String::from_raw_parts(path_pointer, path_len, path_len.max(1)) };
    let value = unsafe { String::from_raw_parts(value_pointer, value_len, value_len.max(1)) };
    dispatch(Event::Field { path, value, caret });
}

/// The WebGL2 context died. The CPU takes the page this very turn.
#[cfg(feature = "gpu")]
#[unsafe(no_mangle)]
pub extern "C" fn bunny_gpu_lost() {
    gpu::lost();
}

/// The context came back. One silent rebuild is owed; after that the
/// CPU keeps the page for as long as it lives.
#[cfg(feature = "gpu")]
#[unsafe(no_mangle)]
pub extern "C" fn bunny_gpu_restored(width: u32, height: u32) {
    let _ = gpu::restored((width.max(1), height.max(1)));
}

/// Clears the drawable to one colour and reads the middle pixel back,
/// packed as `0xRRGGBBAA`. The first thing a tier must be able to say,
/// and the first thing a browser can check: the clear is exact, the
/// readback is in the byte order the rasterizer writes, and the rows
/// come home the way up the raster counts them.
#[cfg(feature = "gpu")]
#[unsafe(no_mangle)]
pub extern "C" fn bunny_gpu_selftest(packed: u32) -> u32 {
    if !gpu::active() {
        return 0;
    }
    let colour = bunny_ui::layout::Color {
        r: (packed >> 24) as u8,
        g: (packed >> 16) as u8,
        b: (packed >> 8) as u8,
        a: packed as u8,
    };
    let size = bunny_ui::layout::Size { width: 4.0, height: 4.0 };
    let runtime = Runtime::new();
    gpu::present_window(
        None,
        &bunny_ui::layout::DisplayList::default(),
        size,
        1,
        colour,
        &*runtime.text(),
        &*runtime.images(),
    );
    let rgba = gpu::read_rgba((4, 4));
    let at = (1 * 4 + 1) * 4;
    ((rgba[at] as u32) << 24)
        | ((rgba[at + 1] as u32) << 16)
        | ((rgba[at + 2] as u32) << 8)
        | rgba[at + 3] as u32
}

// MARK: - Parity against the oracle

/// One scene from the catalog, drawn twice: once by this tier and once
/// by the rasterizer the tier must match. Returns the worst channel
/// delta; the detail goes to the console, where a person can read it.
///
/// This is the first GPU tier in the repo that a machine can actually
/// certify — the others compile for targets nobody here can run.
#[cfg(feature = "gpu")]
#[unsafe(no_mangle)]
pub extern "C" fn bunny_gpu_parity(scene: u32) -> u32 {
    use bunny_ui::layout::{Color, Size};

    let size = Size { width: 120.0, height: 80.0 };
    let scale = 2usize;
    let canvas = Color::rgba(242, 243, 247, 255);
    let (display, name) = bunny_ui::gpu::scenes::scene(scene);
    let runtime = Runtime::new();
    let physical = (
        (size.width as usize * scale) as u32,
        (size.height as usize * scale) as u32,
    );
    gpu::present_window(None, &display, size, scale, canvas, &*runtime.text(), &*runtime.images());
    let mine = gpu::read_rgba(physical);
    let theirs = bunny_ui::raster::rasterize_with(
        &display,
        physical.0 as usize,
        physical.1 as usize,
        scale,
        canvas,
        &*runtime.text(),
        &*runtime.images(),
    )
    .to_rgba_bytes();

    if mine.len() != theirs.len() {
        gpu::say(&format!("parity {name}: sizes differ"));
        return 255;
    }
    let mut worst = 0u8;
    let mut beyond_one = 0usize;
    for (a, b) in mine.iter().zip(theirs.iter()) {
        let delta = a.abs_diff(*b);
        worst = worst.max(delta);
        if delta > 1 {
            beyond_one += 1;
        }
    }
    let share = beyond_one as f64 / mine.len() as f64;
    gpu::say(&format!(
        "parity {name}: worst {worst}, {:.4}% beyond one step ({beyond_one} of {})",
        share * 100.0,
        mine.len()
    ));
    worst as u32
}

/// One timed present of a full-window scene, in milliseconds.
///
/// `road` 0 is this tier, 1 is a FRESH `Surface` each sample (the shape
/// the 2.2ms figure was taken in), 2 is the rasterizer in its damage
/// steady state — the same surface, one command moved.
///
/// What a sample INCLUDES, because the house asks every line to say so:
/// the display-list walk, the instance and atlas upload, and the draw
/// submission through `glFlush` on road 0; `Surface::frame` plus the
/// RGBA mirror on roads 1 and 2. It EXCLUDES layout and settle (the
/// list is built once, outside the clock), GPU execution, and the
/// browser's compositing.
///
/// Both roads skip an identical frame — the tier by its retained list,
/// the surface by an empty damage set — so every sample perturbs one
/// command, or the number would be the cost of the skip check.
#[cfg(feature = "gpu")]
#[unsafe(no_mangle)]
pub extern "C" fn bunny_bench_present(road: u32, width: u32, height: u32, samples: u32) -> f64 {
    use bunny_ui::layout::{Color, Size};

    let scale = 2usize;
    let logical = Size { width: width as f64 / scale as f64, height: height as f64 / scale as f64 };
    let canvas = Color::rgba(242, 243, 247, 255);
    let runtime = Runtime::new();

    let build = |nudge: f64| bunny_ui::gpu::scenes::bench_scene(logical, nudge);

    let physical = (width.max(1) as usize, height.max(1) as usize);
    let mut best: Vec<f64> = Vec::with_capacity(samples as usize);
    let mut surface = bunny_ui::raster::Surface::new(physical.0, physical.1, scale, canvas);
    for sample in 0..samples {
        // one command moves every sample: an identical frame is skipped
        // by BOTH roads, and the skip is not what is being measured
        let display = build((sample % 2) as f64 * 0.5);
        let started = unsafe { gl_now() };
        match road {
            0 => {
                gpu::present_window(
                    None, &display, logical, scale, canvas,
                    &*runtime.text(), &*runtime.images(),
                );
            }
            1 => {
                let mut fresh =
                    bunny_ui::raster::Surface::new(physical.0, physical.1, scale, canvas);
                let _ = fresh.frame(display, &*runtime.text(), &*runtime.images());
                let _ = fresh.rgba();
            }
            _ => {
                let _ = surface.frame(display, &*runtime.text(), &*runtime.images());
                let _ = surface.rgba();
            }
        }
        best.push(unsafe { gl_now() } - started);
    }
    best.sort_by(|a, b| a.partial_cmp(b).expect("no nan on a clock"));
    best[best.len() / 2]
}
