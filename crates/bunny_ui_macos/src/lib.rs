//! The bunny-ui macOS shell: native window, pointer events and the live
//! cycle — hover/press → repaint per event; action on up-inside → state →
//! incremental render → blit. Not a single dependency.
//!
//! The project's `unsafe` lives ONLY here (the [`ffi`] FFI), wrapped in
//! this safe API. The core and the facade keep `#![forbid(unsafe_code)]`.

#![cfg(target_os = "macos")]

mod ffi;
mod metal;
mod text;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::action::{Key, KeyPattern};
use bunny_ui::layout::Size;
use bunny_ui::prelude::{EditCommand, Runtime};
use bunny_ui::view::View;

use ffi::AppEvent;
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
    // real text: the platform engine takes the place of the PixelFont
    let runtime = Runtime::new().text_engine(Rc::new(CoreTextEngine::new()));
    run_window_with(title, size, runtime, root)
}

/// Like [`run_window`], but with the `Runtime` assembled by the caller —
/// the path for apps with their own environment (the text engine is still
/// the assembler's responsibility).
pub fn run_window_with(title: &str, size: Size, runtime: Runtime, root: impl View) {
    let window = ffi::create_window(title, size.width, size.height);
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
    // present takes a READY display list to the window — the tick path
    // reuses it without paying settle, effects or the IME tail
    let present: Rc<dyn Fn(&Runtime, bunny_ui::layout::DisplayList)> = Rc::new({
        let surface = Rc::clone(&surface);
        move |runtime: &Runtime, display: bunny_ui::layout::DisplayList| {
            let (width, height) = window.content_size();
            let scale = window.scale();
            let canvas = bunny_ui::theme::canvas();
            let physical = ((width.round() as usize) * scale, (height.round() as usize) * scale);
            if metal::active() {
                // GPU present: the same display list, no Surface in the
                // path — the drawable is the frame
                metal::present_window(
                    &display,
                    Size { width, height },
                    scale,
                    canvas,
                    &*runtime.text(),
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
                let damage = retained.frame(display, &*runtime.text());
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
            let display = runtime.display_frame(root, Size { width, height });
            present(runtime, display);
        window.set_cursor_pointing(runtime.interaction().hovered.is_some());
        ffi::sync_ime(runtime.ime_snapshot().map(|snapshot| {
            let rect = snapshot.caret_rect;
            (
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

    let handler_runtime = Rc::clone(&runtime);
    let handler_root = Rc::clone(&root);
    let handler_present = Rc::clone(&present);
    ffi::set_handler(Box::new(move |event| {
        let runtime = &handler_runtime;
        let root = &*handler_root;
        match event {
        AppEvent::Redraw => blit(runtime, root),
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
