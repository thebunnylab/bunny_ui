//! The bunny-ui Windows shell: native window, pointer events and the
//! live cycle — hover/press → repaint per event; action on up-inside →
//! state → incremental render → blit. Not a single dependency.
//!
//! The project's `unsafe` lives ONLY in the shell crates (here, the
//! [`ffi`] FFI), wrapped in this safe API. The core and the facade
//! keep `#![forbid(unsafe_code)]`.

#![cfg(target_os = "windows")]

mod ffi;
mod text;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::Runtime;
use bunny_ui::view::View;

use ffi::AppEvent;
pub use text::DirectWriteEngine;

/// Opens the window and enters the live cycle. Returns when the app
/// quits (closing the window quits).
pub fn run_window(title: &str, size: Size, root: impl View) {
    // real text: the platform engine takes the place of the house
    // default (the image engine follows in its own phase)
    let runtime = Runtime::new().text_engine(Rc::new(DirectWriteEngine::new()));
    run_window_with(title, size, runtime, root)
}

/// Like [`run_window`], but with the `Runtime` assembled by the caller —
/// the path for apps with their own environment (the text engine is
/// still the assembler's responsibility).
pub fn run_window_with(title: &str, size: Size, runtime: Runtime, root: impl View) {
    let window = ffi::create_window(title, size.width, size.height, false);
    let runtime = Rc::new(runtime);
    let root = Rc::new(root);

    // one frame: the Runtime settles, lays out, retains the hits for
    // pointer events; the RETAINED surface repaints only the damage
    // (hover repaints one row, not the window) — the shell blits and
    // aligns the cursor. Resize, scale or theme change retires the
    // surface and starts a fresh one.
    let surface: Rc<RefCell<Option<(bunny_ui::raster::Surface, usize, bunny_ui::layout::Color)>>> =
        Rc::new(RefCell::new(None));
    // present takes a READY display list to the window — the tick path
    // reuses it without paying settle or effects. The list goes to the
    // window WHOLE: overlays clamp into the viewport until the panel
    // pool of the overlay phase re-routes them to their own windows.
    let present: Rc<dyn Fn(&Runtime, bunny_ui::layout::DisplayList)> = Rc::new({
        let surface = Rc::clone(&surface);
        move |runtime: &Runtime, display: bunny_ui::layout::DisplayList| {
            let (width, height) = window.content_size();
            let scale = window.scale();
            let canvas = bunny_ui::theme::canvas();
            let physical = (
                (width * scale as f64).round() as usize,
                (height * scale as f64).round() as usize,
            );
            if physical.0 == 0 || physical.1 == 0 {
                // minimized or degenerate: a zero surface is an abort,
                // not a frame
                return;
            }
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
                let (width, height) = (retained.bitmap().width(), retained.bitmap().height());
                window.blit_partial(width, height, retained.rgba(), &damage);
            }
        }
    });
    let blit = {
        let present = Rc::clone(&present);
        move |runtime: &Runtime, root: &_| {
            let (width, height) = window.content_size();
            let display = runtime.display_frame(root, Size { width, height });
            present(runtime, display);
            let interaction = runtime.interaction();
            // a live divider drag keeps the resizer even while the
            // pointer runs ahead of the seam; hovering the grip
            // announces it
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
            // wake or park the frame driver — the event may have
            // started (or finished) an animation
            ffi::set_frame_driver_paused(!runtime.wants_frame());
        }
    };

    let handler_runtime = Rc::clone(&runtime);
    let handler_root = Rc::clone(&root);
    let handler_present = Rc::clone(&present);
    ffi::set_handler(Box::new(move |event| {
        let runtime = &handler_runtime;
        let root = &*handler_root;
        match event {
            AppEvent::Redraw => blit(runtime, root),
            AppEvent::ResignKey => {
                // the user switched away: popovers close like the
                // platform's own
                if runtime.dismiss_all_overlays() {
                    blit(runtime, root);
                }
            }
            AppEvent::MouseMoved { x, y } => {
                if runtime.pointer_moved(x, y) {
                    blit(runtime, root);
                }
            }
            AppEvent::RightMouseDown { x, y } => {
                // the runtime opens (or closes) the context menu; it
                // presents with the scene until panels take it outside
                if runtime.context_click(x, y) {
                    blit(runtime, root);
                }
            }
            AppEvent::MouseDown { x, y, clicks } => {
                if runtime.pointer_clicked(x, y, clicks) {
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
            AppEvent::Blink => {
                // an idle caret blinks; without focus the tick is
                // silence — and the same slow clock ages the tooltip's
                // wait and then shows it: the delay is this tick twice
                let blinked = runtime.blink();
                let explained = runtime.tooltip_tick();
                if blinked || explained {
                    blit(runtime, root);
                }
            }
            AppEvent::Frame { dt } => {
                // the tick path: springs advance, then layout only —
                // zero bodies on a stable tree; settle and effects
                // belong to the real-event path
                if runtime.tick(dt) {
                    let (width, height) = window.content_size();
                    let display = runtime.animation_frame(root, Size { width, height });
                    handler_present(runtime, display);
                }
                ffi::set_frame_driver_paused(!runtime.wants_frame());
            }
        }
    }));

    // first frame into the hidden window, then the reveal — the window
    // never flashes unpainted — and the pump takes over
    ffi::dispatch(AppEvent::Redraw);
    ffi::show_window(window);
    ffi::run();
}
