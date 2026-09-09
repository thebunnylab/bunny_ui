//! The app, and the window it holds — the phone's one window over an
//! `android.app.NativeActivity`, touches and the soft keyboard, the
//! live cycle on a screen the finger drives.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bunny_ui::action::{Key, KeyMatch, Stroke};
use bunny_ui::layout::{Edges, Size};
use bunny_ui::prelude::{EditCommand, Runtime, SizeClass};
use bunny_ui::view::View;

use crate::ffi::{self, AppEvent};
use crate::keys::{self, key_pattern};

/// Points the shell's frame driver at the pace the runtime asks for:
/// the choreographer for springs, flings and a finger on the clock, one
/// timer beat per step for loop clocks alone, and nothing at all for a
/// still scene — a phone that ticks at vsync for nothing is a phone
/// that heats.
fn sync_frame_driver(runtime: &Runtime) {
    ffi::set_frame_driver(match runtime.frame_pace() {
        bunny_ui::anim::FramePace::Display => ffi::DriverPace::Full,
        bunny_ui::anim::FramePace::Slow(interval) => ffi::DriverPace::Slow(interval),
        bunny_ui::anim::FramePace::Idle => ffi::DriverPace::Off,
    });
}

/// Opens the app's window on the screen. The activity is already
/// running when this is called (from the entry the [`activity!`] macro
/// exports), so it returns at once and the window is raised when the
/// system hands one over. `title` and `size` are the twins' vocabulary
/// — a phone shows no bar and the screen is the window — kept so an
/// example runs unchanged on every shell.
///
/// [`activity!`]: crate::activity
pub fn run_window(title: &str, size: Size, root: impl View) {
    let runtime = Runtime::new();
    let app = App::new();
    app.open(WindowSpec::titled(title).size(size.width, size.height), Rc::new(runtime), root);
    app.run();
}

/// Like [`run_window`], but with the `Runtime` assembled by the caller —
/// the path for apps with their own environment (the text engine is
/// still the assembler's responsibility).
pub fn run_window_with(title: &str, size: Size, runtime: Runtime, root: impl View) {
    let app = App::new();
    app.open(WindowSpec::titled(title).size(size.width, size.height), Rc::new(runtime), root);
    app.run();
}

// =============================================================================
// The app, and the window it holds
// =============================================================================

/// What a window is before it exists — the phone's twin of the other
/// shells' spec. The title and the size are accepted and set aside:
/// the screen is the window, and its size is the system's word.
#[derive(Clone, Debug)]
pub struct WindowSpec {
    title: Rc<str>,
    size: Size,
}

impl WindowSpec {
    /// A window named for a bar the phone never shows.
    pub fn titled(title: impl Into<Rc<str>>) -> WindowSpec {
        WindowSpec { title: title.into(), size: Size { width: 360.0, height: 800.0 } }
    }

    /// The content size the twins open at — noted, not obeyed.
    pub fn size(mut self, width: f64, height: f64) -> WindowSpec {
        self.size = Size { width, height };
        self
    }

    /// The name the spec was given.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The size the spec asked for — what a desktop would open at.
    pub fn asked_size(&self) -> Size {
        self.size
    }
}

/// A window's handle in an [`App`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct WindowId(usize);

/// This shell holds ONE window: the screen. A phone app that opens a
/// second document on the other shells asks this constant first and
/// keeps its second view INSIDE the window here.
///
/// ```ignore
/// if bunny_ui_android::MANY_WINDOWS {
///     app.open(composer_spec, runtime, composer);
/// } else {
///     shell.detach_inside(composer);   // a pane, not a window
/// }
/// ```
pub const MANY_WINDOWS: bool = false;

/// The application: the activity the system runs, and the window on it.
///
/// The same shape the other shells carry, so an app writes one boot
/// for five platforms — but this one holds a SINGLE window
/// ([`MANY_WINDOWS`] is false), and it is raised when the system hands
/// the activity a window, not when `open` is called: `open` records
/// the scene, `run` hands it to the activity and returns at once.
///
/// ```ignore
/// let app = App::new();
/// let runtime = app.runtime().text_engine(Rc::new(AndroidTextEngine::new()));
/// app.open(WindowSpec::titled("Trinity"), Rc::new(runtime), workbench);
/// app.run();
/// ```
#[derive(Clone)]
pub struct App {
    inner: Rc<AppInner>,
}

struct AppInner {
    /// The mount, waiting for the window.
    pending: RefCell<Option<Box<dyn FnOnce()>>>,
    opened: Cell<bool>,
    scenes: Cell<usize>,
    /// What runs when the system asks for memory back.
    memory: RefCell<Option<Rc<dyn Fn()>>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An app with no window yet.
    pub fn new() -> App {
        // the app's life outside its window: the phone has no
        // notification road this shell speaks yet, and says so by name
        bunny_ui::app::install_notifier(|_| {
            Err("bunny_ui android: notifications are not served on this shell yet".to_string())
        });
        App {
            inner: Rc::new(AppInner {
                pending: RefCell::new(None),
                opened: Cell::new(false),
                scenes: Cell::new(0),
                memory: RefCell::new(None),
            }),
        }
    }

    /// A runtime for the window — named for its own scene.
    pub fn runtime(&self) -> Runtime {
        let seq = self.inner.scenes.get();
        self.inner.scenes.set(seq + 1);
        Runtime::scene(format!("w{seq}"))
    }

    /// Records the window `run` raises on `runtime`, showing `root`.
    ///
    /// # Panics
    ///
    /// A SECOND window: this shell holds one ([`MANY_WINDOWS`]), and a
    /// silent half-window would be worse than the refusal.
    pub fn open(&self, spec: WindowSpec, runtime: Rc<Runtime>, root: impl View) -> WindowId {
        assert!(
            !self.inner.opened.replace(true),
            "this shell holds ONE window (bunny_ui_android::MANY_WINDOWS is false) — \
             ask the constant before opening a second"
        );
        let inner = Rc::clone(&self.inner);
        let _ = spec;
        *self.inner.pending.borrow_mut() = Some(Box::new(move || mount(runtime, root, inner)));
        WindowId(1)
    }

    /// The window cannot close on a phone: the system owns the app's
    /// life. Nothing happens, by design.
    pub fn close(&self, _id: WindowId) {}

    /// The windows the app has open.
    pub fn windows(&self) -> Vec<WindowId> {
        if self.inner.opened.get() { vec![WindowId(1)] } else { Vec::new() }
    }

    /// What to let go of when the system asks for memory back — the
    /// image caches, a thumbnail store.
    pub fn on_memory_warning(&self, release: impl Fn() + 'static) {
        *self.inner.memory.borrow_mut() = Some(Rc::new(release));
    }

    /// Hands the window to the activity, which raises it when the
    /// system gives it a surface. Returns at once: the activity was
    /// running before the app was built.
    ///
    /// # Panics
    ///
    /// Without a window to open: `open` first.
    pub fn run(&self) {
        let mount = self
            .inner
            .pending
            .borrow_mut()
            .take()
            .expect("bunny_ui android: open a window before run");
        ffi::set_mount(mount);
    }
}

/// Wires everything that lives as long as the app does — the road to
/// the window, the mirrors, the gates and the event handler — once the
/// system has handed the activity its first window.
fn mount(runtime: Rc<Runtime>, root: impl View, app: Rc<AppInner>) {
    // the road, chosen ONCE per window: the GPU, or the floor
    ffi::install_gpu();
    // the season's mirrors: reduce-motion always follows the system
    // (accessibility is never the app's to refuse); the theme follows
    // ONLY while the app has not chosen one — an installed theme means
    // the scene owns its colors and the shell stays out
    let mirror_theme = bunny_ui::theme::version() == 0;
    let (dark, compact, _) = ffi::config();
    if mirror_theme && dark == Some(true) {
        bunny_ui::theme::install(bunny_ui::theme::Theme::dark());
    }
    runtime.set_reduce_motion(ffi::reduce_motion());
    runtime.set_environment(|values| values.horizontalSizeClass = size_class(compact));
    let (top, left, bottom, right) = ffi::safe_area();
    runtime.set_safe_area(edges(top, left, bottom, right));
    // a task that lands on a worker thread asks the loop for one more
    // turn; the frame it takes drains the queue on its way
    runtime.set_wake_hook(std::sync::Arc::new(ffi::wake_from_any_thread));
    // two owners: the keyboard gate and the event handler
    let root = Rc::new(root);

    // present takes a READY display list to the window — the tick path
    // reuses it without paying settle or effects
    let present: Rc<dyn Fn(&Runtime, bunny_ui::layout::DisplayList)> =
        Rc::new(move |runtime: &Runtime, display: bunny_ui::layout::DisplayList| {
            let (width, height) = ffi::view_size();
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            ffi::present(
                &display,
                Size { width, height },
                ffi::view_scale(),
                bunny_ui::theme::canvas(),
                &*runtime.text(),
                &*runtime.images(),
            );
        });
    let blit = {
        let present = Rc::clone(&present);
        move |runtime: &Runtime, root: &_| {
            let (width, height) = ffi::view_size();
            // a box that draws parts which TOUCH puts the shared edge
            // on a whole PIXEL — it needs the screen's scale
            runtime.set_device_scale(ffi::view_scale() as f64);
            // the WHOLE window: the safe area is the layout's, applied
            // by the core, and overlays position inside it
            let display = runtime.display_frame(root, Size { width, height });
            present(runtime, display);
            // the keyboard follows the focus: a field that took it wants
            // the keys, a scene with none wants them gone
            ffi::want_keyboard(runtime.focus_takes_text());
            // wake or park the frame driver — the event may have
            // started (or finished) an animation, or a finger's clock
            sync_frame_driver(runtime);
        }
    };

    // the gate: keymap BEFORE the keyboard's own road — bare chars pass
    // straight through to whoever holds the keyboard AND is taking text
    // (typing is never stolen); a binding with no handler mounted does
    // not consume
    let key_gate: Box<dyn FnMut(&keys::KeyStroke) -> bool> = Box::new({
        let runtime = Rc::clone(&runtime);
        let root = Rc::clone(&root);
        let blit = blit.clone();
        move |stroke: &keys::KeyStroke| {
            let Some(pattern) = key_pattern(stroke) else {
                return false;
            };
            // MID-CHORD the keyboard belongs to the keymap: the stroke
            // that finishes `ctrl-k s` is not typing
            let mid_chord = !runtime.pending_chord().is_empty();
            if !mid_chord && runtime.focus_takes_text() && pattern.is_text_input() {
                return false;
            }
            // a focused escape hatch owns its strokes: an editor's
            // arrows, Enter and Tab are its own
            let taken = runtime.key_stroke(Stroke::new(pattern, stroke.typed));
            if taken.handled {
                if let Some(text) = taken.text {
                    ffi::clipboard_write(&text);
                }
                blit(&runtime, &*root);
                return true;
            }
            // a field of MANY lines owns the bare vertical arrows before
            // any binding — the break arrives by the keyboard's road
            if !mid_chord
                && pattern.is_plain()
                && let Some(command) = match pattern.key {
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
    });

    let handler_runtime = Rc::clone(&runtime);
    let handler_root = Rc::clone(&root);
    let handler_present = Rc::clone(&present);
    // `adb shell setprop debug.bunny.trace 1`: every event but the
    // clocks, one line each, in logcat
    let trace = crate::log::trace();
    let handler: Box<dyn FnMut(AppEvent)> = Box::new(move |event| {
        let runtime = &handler_runtime;
        let root = &*handler_root;
        if trace && !matches!(event, AppEvent::Blink | AppEvent::Frame { .. }) {
            crate::log::alog!("{event:?}");
        }
        match event {
            AppEvent::Redraw | AppEvent::Wake | AppEvent::WindowGained => blit(runtime, root),
            // the presents are gated already; the next window brings
            // its own frame
            AppEvent::WindowLost => {}
            AppEvent::Background => {
                // off the screen the decorations rest, and nothing
                // presents until the app is back
                runtime.set_loops_paused(true);
                if runtime.dismiss_all_overlays() {
                    let _ = runtime.display_frame(root, {
                        let (width, height) = ffi::view_size();
                        Size { width, height }
                    });
                }
            }
            AppEvent::Foreground => {
                runtime.set_loops_paused(false);
                blit(runtime, root);
            }
            AppEvent::LowMemory => {
                if let Some(release) = app.memory.borrow().clone() {
                    release();
                }
            }
            AppEvent::Config { dark, compact, scale } => {
                if mirror_theme && let Some(wants_dark) = dark {
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
                runtime.set_environment(|values| values.horizontalSizeClass = size_class(compact));
                runtime.set_device_scale(scale as f64);
                blit(runtime, root);
            }
            AppEvent::SafeArea { top, left, bottom, right } => {
                runtime.set_safe_area(edges(top, left, bottom, right));
                blit(runtime, root);
            }
            AppEvent::Keyboard { overlap } => {
                runtime.set_keyboard_inset(overlap);
                blit(runtime, root);
            }
            AppEvent::KeyboardDismissed => {
                if runtime.blur() {
                    blit(runtime, root);
                }
            }
            // a touch that changed nothing visible may still have put
            // the finger on the clock (a hold that may become a menu) —
            // the driver syncs either way
            AppEvent::TouchBegan { id, x, y, taps } => {
                if runtime.touch_began(id, x, y, taps) {
                    blit(runtime, root);
                } else {
                    sync_frame_driver(runtime);
                }
            }
            AppEvent::TouchMoved { id, x, y } => {
                if runtime.touch_moved(id, x, y) {
                    blit(runtime, root);
                } else {
                    sync_frame_driver(runtime);
                }
            }
            AppEvent::TouchEnded { id, x, y } => {
                // the lift may fire; the pressed visual always clears
                let _ = runtime.touch_ended(id, x, y);
                blit(runtime, root);
            }
            AppEvent::TouchCancelled { id } => {
                if runtime.touch_cancelled(id) {
                    blit(runtime, root);
                } else {
                    sync_frame_driver(runtime);
                }
            }
            AppEvent::Text(text) => {
                // typing and a paste — one road. A return is the break a
                // field of many lines takes and a field of one line
                // declines
                let command = if text == "\n" {
                    EditCommand::Newline
                } else {
                    EditCommand::Insert(text)
                };
                if runtime.key(command).applied {
                    blit(runtime, root);
                }
            }
            AppEvent::DeleteBackward => {
                if runtime.key(EditCommand::Backspace).applied {
                    blit(runtime, root);
                }
            }
            AppEvent::Key(stroke) => {
                let shift = stroke.shift;
                let edit = match stroke.keycode {
                    keys::KEYCODE_FORWARD_DEL => Some(EditCommand::Delete),
                    keys::KEYCODE_DPAD_LEFT => Some(EditCommand::Left(shift)),
                    keys::KEYCODE_DPAD_RIGHT => Some(EditCommand::Right(shift)),
                    keys::KEYCODE_MOVE_HOME => Some(EditCommand::Home(shift)),
                    keys::KEYCODE_MOVE_END => Some(EditCommand::End(shift)),
                    keys::KEYCODE_ESCAPE | keys::KEYCODE_BACK => {
                        // escape — and the phone's back key — release
                        // the keyboard; a back key with nothing to
                        // release is the platform's, and leaves
                        if runtime.blur() {
                            ffi::take_key();
                            blit(runtime, root);
                        }
                        None
                    }
                    keys::KEYCODE_A if stroke.control => Some(EditCommand::SelectAll),
                    keys::KEYCODE_C if stroke.control => {
                        // the field's output goes to the clipboard
                        if let Some(text) = runtime.key(EditCommand::Copy).output {
                            ffi::clipboard_write(&text);
                        }
                        None
                    }
                    keys::KEYCODE_X if stroke.control => {
                        let cut = runtime.key(EditCommand::Cut);
                        if let Some(text) = &cut.output {
                            ffi::clipboard_write(text);
                        }
                        if cut.output.is_some() {
                            blit(runtime, root);
                        }
                        None
                    }
                    keys::KEYCODE_V if stroke.control => {
                        ffi::clipboard_read().map(EditCommand::Insert)
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
                // an idle caret blinks; the same slow clock ages the
                // tooltip's wait and a sequence in the air
                let blinked = runtime.blink();
                let explained = runtime.tooltip_tick();
                let chorded = runtime.chord_tick();
                if blinked || explained || chorded {
                    blit(runtime, root);
                }
            }
            AppEvent::Frame { dt } => {
                // the tick path: springs and flings advance, then layout
                // only — zero bodies on a stable tree. A finger the
                // clock decided for reached the app: that frame settles
                let ticked = runtime.tick(dt);
                if ticked.input {
                    blit(runtime, root);
                } else {
                    if ticked.any() {
                        let (width, height) = ffi::view_size();
                        let display = runtime.animation_frame(root, Size { width, height });
                        handler_present(runtime, display);
                    }
                    sync_frame_driver(runtime);
                }
            }
        }
    });

    ffi::set_key_gate(key_gate);
    ffi::set_handler(handler);
}

fn size_class(compact: bool) -> SizeClass {
    if compact { SizeClass::Compact } else { SizeClass::Regular }
}

fn edges(top: f64, left: f64, bottom: f64, right: f64) -> Edges {
    Edges { top, leading: left, bottom, trailing: right }
}
