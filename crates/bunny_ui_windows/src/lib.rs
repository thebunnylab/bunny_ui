//! The bunny-ui Windows shell: native window, pointer events and the
//! live cycle — hover/press → repaint per event; action on up-inside →
//! state → incremental render → blit. Not a single dependency.
//!
//! The project's `unsafe` lives ONLY in the shell crates (here, the
//! [`ffi`] FFI), wrapped in this safe API. The core and the facade
//! keep `#![forbid(unsafe_code)]`.

#![cfg(target_os = "windows")]

pub mod credentials;
mod d3d;
pub mod dialog;
mod ffi;
mod image;
mod text;
pub mod webview;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::action::{Key, KeyMatch, KeyPattern, Stroke};
use bunny_ui::layout::{Axis, Size};
use bunny_ui::prelude::{EditCommand, Runtime};
use bunny_ui::view::View;

use ffi::AppEvent;
pub use d3d::OffscreenD3d;
pub use image::WicImageEngine;
pub use text::DirectWriteEngine;

/// Virtual key → the keymap vocabulary. Named keys come from the
/// VK table; the rest becomes `Char` through the base char (a clean
/// keyboard state), lowercased. `None` = lone modifier/function key.
///
/// The modifier mapping is the platform's: Ctrl is the accelerator,
/// so Ctrl carries `command`; Alt carries `option`; the `control`
/// flag stays false (the Windows key belongs to the system). An
/// AltGr chord that types (`types_text`) never reaches this table —
/// the gate lets it through to the character road first.
fn key_pattern(stroke: &ffi::KeyStroke) -> Option<KeyPattern> {
    let named = match stroke.vk {
        0x28 => Some(Key::Down),
        0x26 => Some(Key::Up),
        0x25 => Some(Key::Left),
        0x27 => Some(Key::Right),
        0x0D => Some(Key::Enter), // Return and the numeric keypad Enter share it
        0x1B => Some(Key::Escape),
        0x09 => Some(Key::Tab),
        0x21 => Some(Key::PageUp),
        0x22 => Some(Key::PageDown),
        0x08 => Some(Key::Backspace),
        0x2E => Some(Key::Delete),
        0x24 => Some(Key::Home),
        0x23 => Some(Key::End),
        _ => None,
    };
    let key = named.or_else(|| {
        let base = stroke.chars_ignoring.chars().next()?;
        (!base.is_control()).then(|| Key::Char(base.to_ascii_lowercase()))
    })?;
    Some(KeyPattern {
        key,
        shift: stroke.shift,
        command: stroke.control,
        option: stroke.alt,
        control: false,
    })
}

/// Opens the window and enters the live cycle. Returns when the app
/// quits (closing the window quits).
pub fn run_window(title: &str, size: Size, root: impl View) {
    // real text and real images: the platform engines take the place
    // of the house defaults
    let runtime = Runtime::new()
        .text_engine(Rc::new(DirectWriteEngine::new()))
        .image_engine(Rc::new(WicImageEngine::new()));
    run_window_with(title, size, runtime, root)
}

/// Who draws the window's top edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chrome {
    /// The system title bar.
    Native,
    /// The SCENE draws the bar: no system frame, resize borders kept.
    /// Mark the bar with [`ViewExt::window_drag_region`] so the window
    /// drags by it, and mark the scene's own buttons with
    /// [`ViewExt::window_control`] — the platform activates them, and
    /// the maximize button offers the system's snap flyout.
    ///
    /// [`ViewExt::window_drag_region`]: bunny_ui::ext::ViewExt::window_drag_region
    /// [`ViewExt::window_control`]: bunny_ui::ext::ViewExt::window_control
    Scene,
}

/// Like [`run_window`], but with the `Runtime` assembled by the caller —
/// the path for apps with their own environment (the text engine is
/// still the assembler's responsibility).
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
    let window = ffi::create_window(title, size.width, size.height, chrome == Chrome::Scene);
    // the present backend, chosen ONCE: the GPU by default, the CPU
    // raster on refusal — and the window has not shown yet, so the
    // first frame (whichever road) lands before anyone looks
    ffi::install_gpu(&window);
    // the season's mirrors: reduce-motion always follows the system
    // (accessibility is never the app's to refuse); the theme follows
    // ONLY while the app has not chosen one — an installed theme means
    // the scene owns its colors and the shell stays out
    let mirror_theme = bunny_ui::theme::version() == 0;
    if mirror_theme && ffi::os_uses_light_theme() == Some(false) {
        bunny_ui::theme::install(bunny_ui::theme::Theme::dark());
    }
    runtime.set_reduce_motion(!ffi::animations_enabled());
    // a task that lands on a worker thread asks the pump for one more
    // turn; the frame it takes drains the queue on its way
    runtime.set_wake_hook(std::sync::Arc::new(ffi::wake_from_any_thread));
    // two owners: the keyboard gate and the event handler
    let runtime = Rc::new(runtime);
    let root = Rc::new(root);

    // one frame: the Runtime settles, lays out, retains the hits for
    // pointer events; the RETAINED surface repaints only the damage
    // (hover repaints one row, not the window) — the shell blits and
    // aligns the cursor. Resize, scale or theme change retires the
    // surface and starts a fresh one.
    let surface: Rc<RefCell<Option<(bunny_ui::raster::Surface, usize, bunny_ui::layout::Color)>>> =
        Rc::new(RefCell::new(None));
    // the open popovers' panels, pooled by identity path
    let panels: Rc<RefCell<std::collections::HashMap<String, ffi::WindowHandle>>> =
        Rc::new(RefCell::new(std::collections::HashMap::new()));
    // present takes a READY display list to the window — the tick path
    // reuses it without paying settle or effects
    let present: Rc<dyn Fn(&Runtime, bunny_ui::layout::DisplayList)> = Rc::new({
        let surface = Rc::clone(&surface);
        let panels = Rc::clone(&panels);
        move |runtime: &Runtime, full_display: bunny_ui::layout::DisplayList| {
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
            // the native hosts FIRST — before the window's own present
            // and the overlay panels. A hosted engine renders OUT of
            // process: the sooner it holds its frame the sooner its
            // relayout runs in parallel with everything below — and
            // inside a WM_SIZE turn this is the earliest beat the
            // platform has (no throttles: the mac deleted its own)
            let hosts = runtime.hosts();
            for host in &hosts {
                let bunny_ui::host::HostSpec::Webview { url, scripts, console, requests } =
                    &host.spec;
                // the stamp fingerprints the whole spec — a change
                // re-instructs the mounted view, never re-creates it
                let mut stamp = String::with_capacity(url.len() + 4);
                stamp.push_str(url);
                stamp.push('\u{2}');
                stamp.push(if *console { 'c' } else { '-' });
                stamp.push(if *requests { 'r' } else { '-' });
                for script in scripts.iter() {
                    stamp.push('\u{1}');
                    stamp.push_str(script);
                }
                window.host_place(
                    &host.path,
                    &stamp,
                    (
                        host.frame.origin.x,
                        host.frame.origin.y,
                        host.frame.size.width,
                        host.frame.size.height,
                    ),
                    (
                        host.visible.origin.x,
                        host.visible.origin.y,
                        host.visible.size.width,
                        host.visible.size.height,
                    ),
                    |container| webview::create(&host.path, container, &host.spec),
                    |_stamp| webview::update(&host.path, &host.spec),
                    |bounds, shown| webview::place(&host.path, bounds, shown),
                );
            }
            window.host_sweep(
                &hosts.iter().map(|host| host.path.clone()).collect::<Vec<_>>(),
                |key| webview::sweep(key),
            );
            // the window presents everything BEFORE the first overlay;
            // each overlay re-presents its own slice on an owned panel
            // in screen coordinates — that is how it leaves the window
            let overlays = runtime.overlays();
            let display = match overlays.first() {
                Some(first) => full_display.translated_slice((0, first.display.0), 0.0, 0.0),
                None => full_display.clone(),
            };
            {
                let mut store = panels.borrow_mut();
                let dead: Vec<String> = store
                    .keys()
                    .filter(|path| !overlays.iter().any(|overlay| &overlay.path == *path))
                    .cloned()
                    .collect();
                for path in dead {
                    if let Some(panel) = store.remove(&path) {
                        panel.close_panel();
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
                        .or_insert_with(|| ffi::create_panel(&window));
                    panel.set_scene_origin(x, y);
                    let slice = full_display.translated_slice(overlay.display, -x, -y);
                    let panel_physical =
                        ((w * scale as f64).round() as usize, (h * scale as f64).round() as usize);
                    let bitmap = bunny_ui::raster::rasterize_with(
                        &slice,
                        panel_physical.0,
                        panel_physical.1,
                        scale,
                        bunny_ui::layout::Color { r: 0, g: 0, b: 0, a: 0 },
                        &*runtime.text(),
                        &*runtime.images(),
                    );
                    // position, size and pixels land atomically; the
                    // premultiply for the per-pixel-alpha window fuses
                    // into the copy at the boundary
                    panel.present_layered(
                        window.layout_rect_to_screen(x, y, w, h),
                        panel_physical.0,
                        panel_physical.1,
                        &bitmap.to_rgba_bytes(),
                    );
                }
            }
            if d3d::active() {
                // GPU present: the same display list, no Surface in the
                // path — the swapchain is the frame
                d3d::present_window(
                    &display,
                    Size { width, height },
                    scale,
                    canvas,
                    &*runtime.text(),
                    &*runtime.images(),
                );
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
            // the webview commands are spent BEFORE the frame renders,
            // so the state an expired eval writes lands in THIS layout.
            // An op whose page is not mounted answers immediately —
            // never silence that looks like a slow page.
            for op in runtime.webview_commands() {
                use bunny_ui::host::WebviewOp;
                match op {
                    WebviewOp::Navigate { path, url } => webview::navigate(&path, &url),
                    WebviewOp::Back { path } => webview::back(&path),
                    WebviewOp::Forward { path } => webview::forward(&path),
                    WebviewOp::Input { path, event } => webview::input(&path, &event),
                    WebviewOp::Eval { path, token, js } => {
                        if let Err(why) = webview::eval(&path, token, &js) {
                            let _ = runtime.webview_eval_done(token, Err(why));
                        }
                    }
                    WebviewOp::Snapshot { path, token } => {
                        if let Err(why) = webview::snapshot(&path, token) {
                            let _ = runtime.webview_snapshot_done(token, Err(why));
                        }
                    }
                }
            }
            let (width, height) = window.content_size();
            // a box that draws parts which TOUCH puts the shared edge
            // on a whole PIXEL — it needs the screen's scale
            runtime.set_device_scale(window.scale() as f64);
            // popovers position against the SCREEN's work area, in
            // layout coordinates — overflow becomes plain geometry
            runtime.set_overlay_bounds(window.screen_bounds_in_layout().map(
                |(x, y, w, h)| bunny_ui::layout::Rect {
                    origin: bunny_ui::layout::Point { x, y },
                    size: Size { width: w, height: h },
                },
            ));
            let display = runtime.display_frame(root, Size { width, height });
            present(runtime, display);
            let interaction = runtime.interaction();
            // a live divider drag keeps the resizer even while the
            // pointer runs ahead of the seam; hovering the grip
            // announces it
            window.set_cursor(match runtime.seam_axis() {
                // lanes side by side: the seam travels left and right
                Some(Axis::Horizontal) => ffi::Cursor::ResizeLeftRight,
                // lanes stacked: it travels up and down
                Some(Axis::Vertical) => ffi::Cursor::ResizeUpDown,
                None if interaction.hovered.is_some() => ffi::Cursor::Pointing,
                None => ffi::Cursor::Arrow,
            });
            // the input system's mirror: the doors answer from this
            // without asking the runtime mid-message
            ffi::sync_ime(runtime.ime_snapshot().map(|snapshot| {
                let rect = snapshot.caret_rect;
                (
                    snapshot.marked.is_some(),
                    snapshot.marked.map(|(start, _)| start).unwrap_or(0),
                    (rect.origin.x, rect.origin.y, rect.size.width, rect.size.height),
                )
            }));
            // wake or park the frame driver — the event may have
            // started (or finished) an animation
            ffi::set_frame_driver_paused(!runtime.wants_frame());
        }
    };

    // the frame conversation: a press on a `.window_drag_region()`
    // (with no interactive target above) moves the window; a
    // `.window_control(…)` answers as the window's own button — the
    // platform closes, minimizes, maximizes and offers its snap flyout
    ffi::set_chrome_gates(
        Box::new({
            let runtime = Rc::clone(&runtime);
            move |x, y| runtime.window_drag_at(x, y)
        }),
        Box::new({
            let runtime = Rc::clone(&runtime);
            move |x, y| {
                runtime.window_control_at(x, y).map(|control| match control {
                    bunny_ui::layout::WindowControl::Close => ffi::ControlHit::Close,
                    bunny_ui::layout::WindowControl::Minimize => ffi::ControlHit::Minimize,
                    bunny_ui::layout::WindowControl::Maximize => ffi::ControlHit::Maximize,
                })
            }
        }),
    );

    // the candidate window's anchor: the rect at the composition's
    // start, answered live by the runtime in layout points
    ffi::set_ime_rect_resolver(Box::new({
        let runtime = Rc::clone(&runtime);
        move |utf16| {
            runtime.ime_rect_for(utf16).map(|rect| {
                (rect.origin.x, rect.origin.y, rect.size.width, rect.size.height)
            })
        }
    }));

    // the gate: keymap BEFORE the input system — bare chars pass
    // straight through to whoever holds the keyboard AND is taking
    // text (typing is never stolen; a modal box in command mode
    // declines and the stroke walks on); a binding with no handler
    // mounted does not consume (the palette-less screen types fine);
    // an AltGr chord that types IS text and never enters. The
    // composition-first step arrives with the IME phase.
    ffi::set_key_gate(Box::new({
        let runtime = Rc::clone(&runtime);
        let root = Rc::clone(&root);
        let blit = blit.clone();
        move |stroke: &ffi::KeyStroke| {
            if stroke.types_text {
                return false;
            }
            let Some(pattern) = key_pattern(stroke) else {
                return false;
            };
            // MID-CHORD the keyboard belongs to the keymap: the stroke
            // that finishes `cmd-k s` is not typing, and it is not the
            // focused box's either
            let mid_chord = !runtime.pending_chord().is_empty();
            if !mid_chord && runtime.focus_takes_text() && pattern.is_text_input() {
                return false;
            }
            // a focused escape hatch owns its strokes: an editor's
            // arrows, Enter and Tab are its own, and a copy hands the
            // text back for the clipboard
            let taken = runtime.key_stroke(Stroke::new(pattern, stroke.typed));
            if taken.handled {
                if let Some(text) = taken.text {
                    ffi::clipboard_write(&text);
                }
                blit(&runtime, &*root);
                return true;
            }
            // a field of MANY lines owns the bare break and the bare
            // vertical arrows, before any binding — and only it: a
            // one-line field declines and the stroke walks on, so the
            // app keeps its Enter and a list keeps its arrows
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
                blit(&runtime, &*root);
                return true;
            }
            let action = match runtime.chord(Stroke::new(pattern, stroke.typed)) {
                KeyMatch::Action(action) => action,
                // the stroke opened (or let go of) a sequence: it is
                // spent, and a which-key panel may have just changed
                KeyMatch::Pending => {
                    blit(&runtime, &*root);
                    return true;
                }
                KeyMatch::None => return false,
            };
            if runtime.dispatch_action(action) {
                blit(&runtime, &*root);
                true
            } else {
                false
            }
        }
    }));

    // everything a page reports lands here and runs the matching
    // runtime door; a door that ran a retained writer re-presents —
    // but never mid-drag (the state is written; the resize's own
    // presenter shows it, the one-presenter law)
    webview::set_dispatch({
        let runtime = Rc::clone(&runtime);
        let root = Rc::clone(&root);
        let blit = blit.clone();
        move |event| {
            let woke = match event {
                webview::WebviewEvent::Navigated { path, url } => {
                    runtime.webview_navigated(&path, &url)
                }
                webview::WebviewEvent::NavigationFailed { path, url, why } => {
                    runtime.webview_navigate_failed(&path, &url, &why)
                }
                webview::WebviewEvent::Posted { path, body } => {
                    runtime.webview_posted(&path, &body)
                }
                webview::WebviewEvent::Console { path, line } => {
                    runtime.webview_console(&path, &line)
                }
                webview::WebviewEvent::Requested { path, line } => {
                    runtime.webview_requested(&path, &line)
                }
                webview::WebviewEvent::EvalDone { token, result } => {
                    runtime.webview_eval_done(token, result)
                }
                webview::WebviewEvent::SnapshotDone { token, result } => runtime
                    .webview_snapshot_done(
                        token,
                        result.map(|(width, height, rgba)| bunny_ui::host::WebviewSnapshot {
                            width,
                            height,
                            rgba,
                        }),
                    ),
                // a click landed in the island: what a click beside a
                // popover dismisses, this dismisses too
                webview::WebviewEvent::FocusTaken => runtime.dismiss_all_overlays(),
            };
            if woke && !ffi::in_size_move() {
                blit(&runtime, &*root);
            }
        }
    });

    let handler_runtime = Rc::clone(&runtime);
    let handler_root = Rc::clone(&root);
    let handler_present = Rc::clone(&present);
    ffi::set_handler(Box::new(move |event| {
        let runtime = &handler_runtime;
        let root = &*handler_root;
        match event {
            AppEvent::Redraw | AppEvent::Wake => blit(runtime, root),
            AppEvent::SettingsChanged => {
                runtime.set_reduce_motion(!ffi::animations_enabled());
                if mirror_theme {
                    let wants_dark = ffi::os_uses_light_theme() == Some(false);
                    let is_dark =
                        bunny_ui::theme::current().canvas == bunny_ui::theme::Theme::dark().canvas;
                    if wants_dark != is_dark {
                        bunny_ui::theme::install(if wants_dark {
                            bunny_ui::theme::Theme::dark()
                        } else {
                            bunny_ui::theme::Theme::light()
                        });
                    }
                }
                blit(runtime, root);
            }
            AppEvent::ResignKey => {
                // the user switched away: popovers close like the
                // platform's own
                if runtime.dismiss_all_overlays() {
                    blit(runtime, root);
                }
            }
            AppEvent::MouseMoved { x, y, modifiers } => {
                if runtime.pointer_moved(x, y, modifiers) {
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
            AppEvent::MouseDown { x, y, clicks, modifiers } => {
                if runtime.pointer_clicked(x, y, clicks, modifiers) {
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
                // offset is engine state: repaint without render
                if runtime.wheel(x, y, dx, dy) {
                    blit(runtime, root);
                }
            }
            AppEvent::Text(text) => {
                // typing, paste of characters, and the IME's commit —
                // the same road for all of them
                if !text.is_empty() && runtime.key(EditCommand::Insert(text)).applied {
                    blit(runtime, root);
                }
            }
            AppEvent::ImeMark { text, caret } => {
                let command = EditCommand::SetMarked { text, caret_utf16: (caret, 0) };
                if runtime.key(command).applied {
                    blit(runtime, root);
                }
            }
            AppEvent::ImeUnmark => {
                if runtime.key(EditCommand::Unmark).applied {
                    blit(runtime, root);
                }
            }
            AppEvent::Key { vk, shift, command } => {
                let edit = match vk {
                    0x08 => Some(EditCommand::Backspace),
                    0x2E => Some(EditCommand::Delete),
                    0x25 => Some(EditCommand::Left(shift)),
                    0x27 => Some(EditCommand::Right(shift)),
                    0x24 => Some(EditCommand::Home(shift)),
                    0x23 => Some(EditCommand::End(shift)),
                    0x1B => {
                        // esc releases focus
                        if runtime.blur() {
                            blit(runtime, root);
                        }
                        None
                    }
                    0x41 if command => Some(EditCommand::SelectAll), // Ctrl+A
                    0x43 if command => {
                        // Ctrl+C — the field's output goes to the system
                        if let Some(text) = runtime.key(EditCommand::Copy).output {
                            ffi::clipboard_write(&text);
                        }
                        None
                    }
                    0x58 if command => {
                        // Ctrl+X
                        let cut = runtime.key(EditCommand::Cut);
                        if let Some(text) = &cut.output {
                            ffi::clipboard_write(text);
                        }
                        if cut.output.is_some() {
                            blit(runtime, root);
                        }
                        None
                    }
                    0x56 if command => ffi::clipboard_read().map(EditCommand::Insert), // Ctrl+V
                    _ => None,
                };
                if let Some(edit) = edit
                    && runtime.key(edit).applied
                {
                    blit(runtime, root);
                }
            }
            AppEvent::Blink => {
                // an idle caret blinks; without focus the tick is
                // silence — and the same slow clock ages the tooltip's
                // wait and then shows it: the delay is this tick twice
                let blinked = runtime.blink();
                let explained = runtime.tooltip_tick();
                // the same slow beat ages a sequence in the air: two
                // ticks and `cmd-k` lets the keyboard go
                let chorded = runtime.chord_tick();
                if blinked || explained || chorded {
                    blit(runtime, root);
                }
            }
            AppEvent::Frame { dt } => {
                // the tick path: springs advance, then layout only —
                // zero bodies on a stable tree; settle and effects
                // belong to the real-event path
                if runtime.tick(dt).any() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(vk: u32, base: &str, shift: bool, control: bool, alt: bool) -> ffi::KeyStroke {
        ffi::KeyStroke {
            vk,
            shift,
            control,
            alt,
            chars_ignoring: base.to_string(),
            types_text: false,
            typed: None,
        }
    }

    #[test]
    fn named_keys_map_to_the_vocabulary() {
        let pattern = key_pattern(&stroke(0x28, "", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Down);
        let pattern = key_pattern(&stroke(0x0D, "\r", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Enter);
        let pattern = key_pattern(&stroke(0x1B, "\u{1b}", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Escape);
    }

    #[test]
    fn ctrl_carries_command_the_accelerator() {
        let pattern = key_pattern(&stroke(0x46, "f", false, true, false)).unwrap();
        assert_eq!(pattern.key, Key::Char('f'));
        assert!(pattern.command, "Ctrl is the accelerator");
        assert!(!pattern.control, "the control flag stays with the system");
        assert!(!pattern.is_text_input(), "a chord is never typing");
    }

    #[test]
    fn a_bare_letter_is_text_input() {
        let pattern = key_pattern(&stroke(0x41, "a", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Char('a'));
        assert!(pattern.is_text_input());
    }

    #[test]
    fn shift_tab_matches_exactly() {
        let pattern = key_pattern(&stroke(0x09, "\t", true, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Tab);
        assert!(pattern.shift);
        assert!(!pattern.command && !pattern.option);
    }

    #[test]
    fn a_lone_modifier_is_no_pattern() {
        // VK_SHIFT alone: no named key, no base char
        assert!(key_pattern(&stroke(0x10, "", true, false, false)).is_none());
        // a control character as the base is not a Char either
        assert!(key_pattern(&stroke(0x73, "", false, false, false)).is_none(), "F4 is silent");
    }

    #[test]
    fn the_base_char_lowers_and_alt_rides_as_option() {
        let pattern = key_pattern(&stroke(0x41, "A", true, false, true)).unwrap();
        assert_eq!(pattern.key, Key::Char('a'), "shift does not change the key's identity");
        assert!(pattern.option, "Alt rides as option");
    }
}
