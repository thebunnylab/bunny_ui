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

mod image;
mod text;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::layout::{Color, Size};
use bunny_ui::prelude::*;
use bunny_ui::raster::Surface;
use bunny_ui::runtime::Runtime;
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
    // typing with a focused field is never stolen by a binding
    let typing = runtime.focused().is_some() && pattern.is_text_input();
    if !typing
        && let Some(action) = runtime.match_key(&pattern)
        && runtime.dispatch_action(action)
    {
        return true;
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
    PointerMove { x: f64, y: f64 },
    PointerDown { x: f64, y: f64 },
    PointerUp { x: f64, y: f64 },
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    Text(String),
    /// A named key from the glue's table, with the modifier bits.
    Key(u32, u32),
    /// A character stroke (the modifiers decide whether it types or
    /// commands).
    KeyChar(char, u32),
    Frame { dt: f64 },
    Resize { width: f64, height: f64, scale: f64 },
    /// The browser finished decoding a registered image — measure and
    /// paint can answer for real now.
    ImageReady,
    /// A task has something to run: a fetch came back, a callback fired.
    Wake,
    /// Dom mode: the browser's scroll observer — the element scrolled
    /// and the engine mirrors the offset (the dual ownership).
    DomScroll { id: u32, x: f64, y: f64 },
    /// Dom mode: the browser's input edited — value + selectionStart.
    Field { path: String, value: String, caret: usize },
}

struct Shell {
    handle: Box<dyn FnMut(Event)>,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
}

fn dispatch(event: Event) {
    SHELL.with(|slot| {
        if let Some(shell) = slot.borrow_mut().as_mut() {
            (shell.handle)(event);
        }
    });
}

/// Boots the shell with the app's root view. The demo crate calls this
/// from its exported `start`; everything after travels through events.
pub fn start(width: f64, height: f64, scale: f64, root: impl View + 'static) {
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
            Event::PointerMove { x, y } => {
                if runtime.pointer_moved(x, y) {
                    present(&runtime, &full, size, scale, &mut surface);
                }
            }
            Event::PointerDown { x, y } => {
                if runtime.pointer_pressed(x, y) {
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
                if stroke(&runtime, pattern(bunny_ui::action::Key::Char(character), mods)) {
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
                if runtime.tick(dt) {
                    present(&runtime, &tick, size, scale, &mut surface);
                } else if runtime.wants_frame() {
                    unsafe { js_request_frame() };
                }
            }
            Event::Resize { width, height, scale: ratio } => {
                size = Size { width, height };
                scale = (ratio.round() as usize).max(1);
                present(&runtime, &full, size, scale, &mut surface);
            }
            Event::ImageReady | Event::Wake => {
                // the layout reflows around the fresh intrinsic size
                // (or around what a task just wrote) and the paint asks
                // the engine again — one full frame, settle included
                present(&runtime, &full, size, scale, &mut surface);
            }
            // Dom-mode traffic — this shell rasterizes, nothing to do
            Event::DomScroll { .. } | Event::Field { .. } => {}
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
    let runtime = Runtime::new()
        .text_engine(Rc::new(CanvasTextEngine::new()))
        .image_engine(Rc::new(CanvasImageEngine::new()));
    // a task that woke asks the page for one turn — the browser's
    // answer to the desktop's run loop source
    runtime.set_wake_hook(std::sync::Arc::new(|| unsafe { js_request_wake() }));
    runtime.set_reduce_motion(true);
    let mut size = Size { width, height };
    let scale = (scale.round() as usize).max(1);

    // patches first, then any island whose pixels changed — the
    // element exists before its bitmap arrives
    fn present(runtime: &Runtime, patches: Vec<bunny_ui::dom::DomPatch>, scale: usize) {
        if !patches.is_empty() {
            let bytes = bunny_ui::dom::encode(&patches);
            unsafe { js_apply_patches(bytes.as_ptr(), bytes.len()) };
        }
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

    let handle = Box::new(move |event: Event| {
        match event {
            Event::PointerDown { x, y } => {
                let _ = runtime.pointer_pressed(x, y);
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::PointerUp { x, y } => {
                let _ = runtime.pointer_released(x, y);
                present(&runtime, runtime.dom_frame(&root, size), scale);
            }
            Event::DomScroll { id, x, y } => {
                // the browser moved the element; the engine mirrors the
                // offset so windows re-materialize and reveals compose
                if let Some(path) = runtime.dom_scroll_path(id) {
                    runtime.set_scroll_offset(&path, bunny_ui::layout::Point { x, y });
                    present(&runtime, runtime.dom_frame(&root, size), scale);
                }
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
                if stroke(&runtime, pattern(bunny_ui::action::Key::Char(character), mods)) {
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
            // hover, wheel and ticks belong to the browser in this
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

#[unsafe(no_mangle)]
pub extern "C" fn bunny_pointer_move(x: f64, y: f64) {
    dispatch(Event::PointerMove { x, y });
}

#[unsafe(no_mangle)]
pub extern "C" fn bunny_pointer_down(x: f64, y: f64) {
    dispatch(Event::PointerDown { x, y });
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

/// The wire contract this binary encodes. The glue reads it before it
/// boots and refuses a stream it was not written for — see
/// `bunny_ui::dom::ABI_VERSION` for the bump checklist.
#[unsafe(no_mangle)]
pub extern "C" fn bunny_abi_version() -> u32 {
    bunny_ui::dom::ABI_VERSION
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
