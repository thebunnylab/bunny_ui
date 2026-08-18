//! The bunny-ui macOS shell: native window, pointer events and the live
//! cycle — hover/press → repaint per event; action on up-inside → state →
//! incremental render → blit. Not a single dependency.
//!
//! The project's `unsafe` lives ONLY here (the [`ffi`] FFI), wrapped in
//! this safe API. The core and the facade keep `#![forbid(unsafe_code)]`.

#![cfg(target_os = "macos")]

mod ffi;
mod image;
mod metal;
mod text;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::action::{Key, KeyPattern};
use bunny_ui::layout::Size;
use bunny_ui::prelude::{EditCommand, Runtime};
use bunny_ui::view::View;

use ffi::AppEvent;
pub use image::CoreGraphicsImageEngine;
pub use metal::OffscreenGpu;
pub use text::CoreTextEngine;

/// AppKit keyCode → the keymap vocabulary. Named keys come from the
/// virtual-key table; the rest becomes `Char` through the base char
/// (ignoring modifiers), lowercased. `None` = lone modifier/function key.
fn key_pattern(stroke: &ffi::KeyStroke) -> Option<KeyPattern> {
    let named = match stroke.code {
        125 => Some(Key::Down),
        126 => Some(Key::Up),
        123 => Some(Key::Left),
        124 => Some(Key::Right),
        36 | 76 => Some(Key::Enter), // Return and the numeric keypad Enter
        53 => Some(Key::Escape),
        48 => Some(Key::Tab),
        116 => Some(Key::PageUp),
        121 => Some(Key::PageDown),
        51 => Some(Key::Backspace),
        117 => Some(Key::Delete),
        115 => Some(Key::Home),
        119 => Some(Key::End),
        _ => None,
    };
    let key = named.or_else(|| {
        let base = stroke.chars_ignoring.chars().next()?;
        // PUA F700–F8FF: AppKit function keys — never text
        (!base.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&base))
            .then(|| Key::Char(base.to_ascii_lowercase()))
    })?;
    Some(KeyPattern {
        key,
        shift: stroke.shift,
        command: stroke.command,
        option: stroke.option,
        control: stroke.control,
    })
}

/// Opens the window and enters the live cycle. Returns when the app quits
/// (closing the window quits).
pub fn run_window(title: &str, size: Size, root: impl View) {
    // real text and real images: the platform engines take the place
    // of the house defaults
    let runtime = Runtime::new()
        .text_engine(Rc::new(CoreTextEngine::new()))
        .image_engine(Rc::new(CoreGraphicsImageEngine::new()));
    run_window_with(title, size, runtime, root)
}

/// Who draws the window's top edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chrome {
    /// The system title bar.
    Native,
    /// The SCENE draws the bar: transparent titlebar, hidden title,
    /// native traffic lights preserved at the top-left corner (reserve
    /// roughly 78×28 logical points around them). Mark the bar with
    /// [`ViewExt::window_drag_region`] so the window drags by it.
    ///
    /// [`ViewExt::window_drag_region`]: bunny_ui::ext::ViewExt::window_drag_region
    Scene,
}

/// Like [`run_window`], but with the `Runtime` assembled by the caller —
/// the path for apps with their own environment (the text engine is still
/// the assembler's responsibility).
pub fn run_window_with(title: &str, size: Size, runtime: Runtime, root: impl View) {
    run_window_chrome(title, size, Chrome::Native, runtime, root)
}

/// Like [`run_window_with`], choosing who draws the window's top edge.
pub fn run_window_chrome(
    title: &str,
    size: Size,
    chrome: Chrome,
    runtime: Runtime,
    root: impl View,
) {
    let window =
        ffi::create_window(title, size.width, size.height, chrome == Chrome::Scene);
    // a task that lands on a worker thread asks the main run loop for
    // one more turn; the frame it takes drains the queue on its way
    ffi::install_wake_source();
    runtime.set_wake_hook(std::sync::Arc::new(ffi::wake_from_any_thread));
    // two owners: the keyboard gate and the event handler
    let runtime = Rc::new(runtime);
    let root = Rc::new(root);

    // one frame: the Runtime settles, lays out, retains the hits for
    // pointer events; the RETAINED surface repaints only the damage
    // (hover repaints one row, not the window) — the shell blits, aligns
    // the cursor and mirrors the focused field for the input system (the
    // IME's synchronous questions answer from this mirror). Resize,
    // scale or theme change retires the surface and starts a fresh one.
    let surface: Rc<RefCell<Option<(bunny_ui::raster::Surface, usize, bunny_ui::layout::Color)>>> =
        Rc::new(RefCell::new(None));
    // the open popovers' child panels, pooled by identity path
    let panels: Rc<RefCell<std::collections::HashMap<String, ffi::WindowHandle>>> =
        Rc::new(RefCell::new(std::collections::HashMap::new()));
    // present takes a READY display list to the window — the tick path
    // reuses it without paying settle, effects or the IME tail
    let present: Rc<dyn Fn(&Runtime, bunny_ui::layout::DisplayList)> = Rc::new({
        let surface = Rc::clone(&surface);
        let panels = Rc::clone(&panels);
        move |runtime: &Runtime, full_display: bunny_ui::layout::DisplayList| {
            let (width, height) = window.content_size();
            let scale = window.scale();
            let canvas = bunny_ui::theme::canvas();
            let physical = ((width.round() as usize) * scale, (height.round() as usize) * scale);
            // the window presents everything BEFORE the first popover;
            // each popover re-presents its own slice on a child panel
            // in screen coordinates — that is how it leaves the window
            let overlays = runtime.overlays();
            let display = match overlays.first() {
                Some(first) => full_display.translated_slice((0, first.display.0), 0.0, 0.0),
                None => full_display.clone(),
            };
            {
                let mut store = panels.borrow_mut();
                let mut dead: Vec<String> = store
                    .keys()
                    .filter(|path| !overlays.iter().any(|overlay| &overlay.path == *path))
                    .cloned()
                    .collect();
                for path in dead.drain(..) {
                    if let Some(panel) = store.remove(&path) {
                        panel.close_panel(&window);
                    }
                }
                for overlay in &overlays {
                    // the panel is BLED around the frame so the card's
                    // own shadow has room — the same pixels every
                    // target paints, no system shadow involved
                    const BLEED: f64 = 32.0;
                    let x = overlay.frame.origin.x - BLEED;
                    let y = overlay.frame.origin.y - BLEED;
                    let w = overlay.frame.size.width + 2.0 * BLEED;
                    let h = overlay.frame.size.height + 2.0 * BLEED;
                    let panel = store
                        .entry(overlay.path.clone())
                        .or_insert_with(|| ffi::create_panel(&window, w, h));
                    panel.set_frame_screen(window.layout_rect_to_screen(x, y, w, h));
                    panel.set_scene_origin(x, y);
                    let slice = full_display.translated_slice(overlay.display, -x, -y);
                    let panel_physical =
                        ((w.round() as usize) * scale, (h.round() as usize) * scale);
                    let bitmap = bunny_ui::raster::rasterize_with(
                        &slice,
                        panel_physical.0,
                        panel_physical.1,
                        scale,
                        bunny_ui::layout::Color { r: 0, g: 0, b: 0, a: 0 },
                        &*runtime.text(),
                        &*runtime.images(),
                    );
                    // the panel's CGImage is premultiplied; the house
                    // compositor is straight — one pass, panel-sized
                    let mut rgba = bitmap.to_rgba_bytes();
                    for pixel in rgba.chunks_exact_mut(4) {
                        let alpha = pixel[3] as u32;
                        if alpha < 255 {
                            for channel in 0..3 {
                                pixel[channel] =
                                    (pixel[channel] as u32 * alpha / 255) as u8;
                            }
                        }
                    }
                    panel.blit_partial(
                        panel_physical.0,
                        panel_physical.1,
                        &rgba,
                        &[(0, 0, panel_physical.0 as i64, panel_physical.1 as i64)],
                    );
                }
            }
            if metal::active() {
                // GPU present: the same display list, no Surface in the
                // path — the drawable is the frame
                metal::present_window(
                    &display,
                    Size { width, height },
                    scale,
                    canvas,
                    &*runtime.text(),
                    &*runtime.images(),
                );
            } else {
                let mut slot = surface.borrow_mut();
                let stale = match &*slot {
                    Some((retained, retained_scale, retained_canvas)) => {
                        retained.bitmap().width() != physical.0
                            || retained.bitmap().height() != physical.1
                            || *retained_scale != scale
                            || *retained_canvas != canvas
                    }
                    None => true,
                };
                if stale {
                    *slot = Some((
                        bunny_ui::raster::Surface::new(physical.0, physical.1, scale, canvas),
                        scale,
                        canvas,
                    ));
                }
                let (retained, _, _) = slot.as_mut().expect("surface for the frame");
                let damage = retained.frame(display, &*runtime.text(), &*runtime.images());
                if !damage.is_empty() {
                    // present only the wounds: damage-only mirror sync +
                    // damage-only backing copy + dirty-rect redraw
                    let (width, height) =
                        (retained.bitmap().width(), retained.bitmap().height());
                    window.blit_partial(width, height, retained.rgba(), &damage);
                }
            }
        }
    });
    let blit = {
        let present = Rc::clone(&present);
        move |runtime: &Runtime, root: &_| {
            let (width, height) = window.content_size();
            // popovers position against the SCREEN, in layout
            // coordinates — overflow becomes plain geometry
            runtime.set_overlay_bounds(window.screen_bounds_in_layout().map(
                |(x, y, w, h)| bunny_ui::layout::Rect {
                    origin: bunny_ui::layout::Point { x, y },
                    size: Size { width: w, height: h },
                },
            ));
            let display = runtime.display_frame(root, Size { width, height });
            present(runtime, display);
        let interaction = runtime.interaction();
        // a live divider drag keeps the resizer even while the pointer
        // runs ahead of the seam; hovering the grip announces it
        let over_grip = interaction.split_drag.is_some()
            || interaction
                .hovered
                .as_deref()
                .is_some_and(|target| target.ends_with("/#split"));
        window.set_cursor(if over_grip {
            ffi::Cursor::ResizeLeftRight
        } else if interaction.hovered.is_some() {
            ffi::Cursor::Pointing
        } else {
            ffi::Cursor::Arrow
        });
        ffi::sync_ime(runtime.ime_snapshot().map(|snapshot| {
            let rect = snapshot.caret_rect;
            (
                std::rc::Rc::from(snapshot.text),
                ffi::NSRange {
                    location: snapshot.selected.0 as u64,
                    length: snapshot.selected.1 as u64,
                },
                snapshot.marked.map(|(location, length)| ffi::NSRange {
                    location: location as u64,
                    length: length as u64,
                }),
                window.layout_rect_to_screen(
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                ),
            )
        }));
        // wake or park the frame driver — the event may have started
        // (or finished) an animation
        ffi::set_frame_driver_paused(!runtime.wants_frame());
        }
    };

    // the drag gate: a press on a `.window_drag_region()` (with no
    // interactive target above it) moves the window — the scene's own
    // title bar on a chrome-less window
    ffi::set_drag_gate(Box::new({
        let runtime = Rc::clone(&runtime);
        move |x, y| runtime.window_drag_at(x, y)
    }));

    // the gate: keymap BEFORE the input system — bare chars with a focused
    // field pass straight through (typing is never stolen); a binding with
    // no handler mounted does not consume (the palette-less screen types fine)
    ffi::set_key_gate(Box::new({
        let runtime = Rc::clone(&runtime);
        let root = Rc::clone(&root);
        let blit = blit.clone();
        move |stroke: &ffi::KeyStroke| {
            let Some(pattern) = key_pattern(stroke) else {
                return false;
            };
            if runtime.focused().is_some() && pattern.is_text_input() {
                return false;
            }
            // a focused escape hatch owns its strokes: an editor's
            // arrows, Enter and Tab are its own, and a copy hands the
            // text back for the pasteboard
            let taken = runtime.key_stroke(&pattern);
            if taken.handled {
                if let Some(text) = taken.text {
                    ffi::clipboard_write(&text);
                }
                blit(&runtime, &*root);
                return true;
            }
            let Some(action) = runtime.match_key(&pattern) else {
                return false;
            };
            if runtime.dispatch_action(action) {
                blit(&runtime, &*root);
                true
            } else {
                false
            }
        }
    }));

    // the input system's questions BEYOND the mirror: index under the
    // mouse (dictionary lookup) and rect at a composition index — both
    // answered live by the runtime
    ffi::set_ime_resolvers(
        Box::new({
            let runtime = Rc::clone(&runtime);
            move |x, y| runtime.ime_index_at(x, y).map(|index| index as u64)
        }),
        Box::new({
            let runtime = Rc::clone(&runtime);
            move |utf16| {
                runtime.ime_rect_for(utf16 as usize).map(|rect| {
                    window.layout_rect_to_screen(
                        rect.origin.x,
                        rect.origin.y,
                        rect.size.width,
                        rect.size.height,
                    )
                })
            }
        }),
    );

    let handler_runtime = Rc::clone(&runtime);
    let handler_root = Rc::clone(&root);
    let handler_present = Rc::clone(&present);
    ffi::set_handler(Box::new(move |event| {
        let runtime = &handler_runtime;
        let root = &*handler_root;
        match event {
        AppEvent::Redraw | AppEvent::Wake => blit(runtime, root),
        AppEvent::ResignKey => {
            // the user switched away: popovers close like the
            // platform's own (their panels never take key, so this
            // only ever fires on the parent)
            if runtime.dismiss_all_overlays() {
                blit(runtime, root);
            }
        }
        AppEvent::MouseMoved { x, y } => {
            if runtime.pointer_moved(x, y) {
                blit(runtime, root);
            }
        }
        AppEvent::MouseDown { x, y } => {
            if runtime.pointer_pressed(x, y) {
                blit(runtime, root);
            }
        }
        AppEvent::MouseUp { x, y } => {
            // fires on up-inside; the pressed visual always clears
            let _ = runtime.pointer_released(x, y);
            blit(runtime, root);
        }
        AppEvent::MouseExited => {
            if runtime.pointer_exited() {
                blit(runtime, root);
            }
        }
        AppEvent::Wheel { x, y, dx, dy } => {
            // offset is engine state: repaint without render (zero bodies)
            if runtime.wheel(x, y, dx, dy) {
                blit(runtime, root);
            }
        }
        AppEvent::Key { code, shift, command, chars } => {
            // printable keys become Insert; PUA F700–F8FF are AppKit
            // function keys — never text
            let printable = |c: char| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c);
            let edit = match code {
                51 => Some(EditCommand::Backspace),
                117 => Some(EditCommand::Delete),
                123 => Some(EditCommand::Left(shift)),
                124 => Some(EditCommand::Right(shift)),
                115 => Some(EditCommand::Home(shift)),
                119 => Some(EditCommand::End(shift)),
                53 => {
                    // esc releases focus
                    if runtime.blur() {
                        blit(runtime, root);
                    }
                    None
                }
                0 if command => Some(EditCommand::SelectAll),
                8 if command => {
                    // cmd+C — the field's output goes to the system
                    if let Some(text) = runtime.key(EditCommand::Copy).output {
                        ffi::clipboard_write(&text);
                    }
                    None
                }
                7 if command => {
                    // cmd+X
                    let cut = runtime.key(EditCommand::Cut);
                    if let Some(text) = &cut.output {
                        ffi::clipboard_write(text);
                    }
                    if cut.output.is_some() {
                        blit(runtime, root);
                    }
                    None
                }
                9 if command => ffi::clipboard_read().map(EditCommand::Insert),
                _ if !command && !chars.is_empty() && chars.chars().all(printable) => {
                    Some(EditCommand::Insert(chars))
                }
                _ => None,
            };
            if let Some(edit) = edit
                && runtime.key(edit).applied
            {
                blit(runtime, root);
            }
        }
        AppEvent::Blink => {
            // an idle caret blinks; without focus the tick is silence
            if runtime.blink() {
                blit(runtime, root);
            }
        }
        AppEvent::Frame { dt } => {
            // the tick path: springs advance, then layout only — zero
            // bodies on a stable tree; settle and effects belong to the
            // real-event path
            if runtime.tick(dt) {
                let (width, height) = window.content_size();
                let display = runtime.animation_frame(root, Size { width, height });
                handler_present(runtime, display);
            }
            ffi::set_frame_driver_paused(!runtime.wants_frame());
        }
        AppEvent::ImeInsert { text } => {
            // the IME commit (or plain typing through the input system)
            if runtime.key(EditCommand::Insert(text)).applied {
                blit(runtime, root);
            }
        }
        AppEvent::ImeMark { text, location, length } => {
            let command = EditCommand::SetMarked {
                text,
                caret_utf16: (location as usize, length as usize),
            };
            if runtime.key(command).applied {
                blit(runtime, root);
            }
        }
        AppEvent::ImeUnmark => {
            if runtime.key(EditCommand::Unmark).applied {
                blit(runtime, root);
            }
        }
        AppEvent::Command { selector } => {
            let edit = match selector.as_str() {
                "deleteBackward:" => Some(EditCommand::Backspace),
                "deleteForward:" => Some(EditCommand::Delete),
                "moveLeft:" => Some(EditCommand::Left(false)),
                "moveRight:" => Some(EditCommand::Right(false)),
                "moveLeftAndModifySelection:" => Some(EditCommand::Left(true)),
                "moveRightAndModifySelection:" => Some(EditCommand::Right(true)),
                "moveToBeginningOfLine:" | "moveToLeftEndOfLine:" | "moveUp:" => {
                    Some(EditCommand::Home(false))
                }
                "moveToBeginningOfLineAndModifySelection:"
                | "moveToLeftEndOfLineAndModifySelection:" => Some(EditCommand::Home(true)),
                "moveToEndOfLine:" | "moveToRightEndOfLine:" | "moveDown:" => {
                    Some(EditCommand::End(false))
                }
                "moveToEndOfLineAndModifySelection:"
                | "moveToRightEndOfLineAndModifySelection:" => Some(EditCommand::End(true)),
                "selectAll:" => Some(EditCommand::SelectAll),
                "cancelOperation:" => {
                    // esc releases focus
                    if runtime.blur() {
                        blit(runtime, root);
                    }
                    None
                }
                // insertNewline:/insertTab: — submit/focus switch are the
                // field's next phase of typed events
                _ => None,
            };
            if let Some(edit) = edit
                && runtime.key(edit).applied
            {
                blit(runtime, root);
            }
        }
        }
    }));

    // first frame, and the run loop takes over
    ffi::dispatch(AppEvent::Redraw);
    ffi::run();
}
