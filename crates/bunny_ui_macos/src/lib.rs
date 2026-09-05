//! The bunny-ui macOS shell: native window, pointer events and the live
//! cycle — hover/press → repaint per event; action on up-inside → state →
//! incremental render → blit. Not a single dependency.
//!
//! The project's `unsafe` lives ONLY here (the [`ffi`] FFI), wrapped in
//! this safe API. The core and the facade keep `#![forbid(unsafe_code)]`.

#![cfg(target_os = "macos")]

pub mod credentials;
pub mod dialog;
mod ffi;
mod image;
mod life;
mod metal;
mod text;
pub mod webview;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::action::{Key, KeyMatch, KeyPattern, Stroke};
use bunny_ui::layout::{Axis, Size};
use bunny_ui::prelude::{EditCommand, Runtime};
use bunny_ui::view::View;

use ffi::AppEvent;
pub use image::CoreGraphicsImageEngine;
pub use metal::OffscreenGpu;
pub use text::CoreTextEngine;

/// Points the shell's frame driver at the pace the runtime asks for:
/// the display link for springs, one timer beat per step for loop
/// clocks alone, and nothing at all for a still scene.
fn sync_frame_driver(runtime: &Runtime) {
    ffi::set_frame_driver(match runtime.frame_pace() {
        bunny_ui::anim::FramePace::Display => ffi::DriverPace::Full,
        bunny_ui::anim::FramePace::Slow(interval) => ffi::DriverPace::Slow(interval),
        bunny_ui::anim::FramePace::Idle => ffi::DriverPace::Off,
    });
}

/// AppKit keyCode → the keymap vocabulary. Named keys come from the
/// virtual-key table; the rest becomes `Char` through the key's OWN
/// character — what it types with no modifier applied, read from the
/// user's layout — lowercased, because CapsLock is never a chord.
/// `None` = lone modifier/function key.
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
        // the KEY's own character, not the one it would type: AppKit's
        // charactersIgnoringModifiers applies shift, so a chord on
        // shifted punctuation used to arrive as its shifted twin and
        // never matched its spec (command_shift(Char('\\')) came in as
        // '|'). Reading the bare character asks the user's own layout
        // instead of assuming a US keyboard.
        let base = stroke.chars_bare.chars().next()?;
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
#[derive(Clone, Copy, PartialEq, Debug)]
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
    /// [`Chrome::Scene`], and the app says WHERE the native buttons
    /// sit: points from the window's top-left corner.
    ///
    /// It is the one piece of the frame an app could not reach. With a
    /// transparent titlebar the system centres the buttons in the bar
    /// it WOULD have drawn — a standard 28 points — so a scene that
    /// draws a taller bar gets them sitting high, and there is no
    /// AppKit call to say otherwise. This moves them by hand, and puts
    /// them back after every resize and every trip through full screen.
    ///
    /// ```ignore
    /// // a bar of forty points wants its lights at (16, 14)
    /// run_window_chrome(title, size, Chrome::SceneAt(Lights::at(16.0, 14.0)), runtime, root)
    /// ```
    ///
    /// The spacing BETWEEN the three stays the system's own: the group
    /// moves, the buttons keep their manners.
    SceneAt(Lights),
}

/// Where the native window buttons sit, and how big they are.
///
/// ```ignore
/// Lights::at(16.0, 15.0)              // the system's own size
/// Lights::at(16.0, 15.0).sized(12.0)  // smaller, gaps and all
/// ```
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Lights {
    x: f64,
    y: f64,
    size: Option<f64>,
}

impl Lights {
    /// Points from the window's TOP-LEFT corner, the way a designer
    /// counts. The buttons keep the size macOS draws them at.
    pub fn at(x: f64, y: f64) -> Lights {
        Lights { x, y, size: None }
    }

    /// The diameter of one light, in points — macOS draws them at
    /// fourteen. The circle IS the button's box, so this is exactly
    /// what lands on the glass, and the distance between the three
    /// scales with it: the group shrinks whole, gaps and all.
    pub fn sized(self, size: f64) -> Lights {
        Lights { size: Some(size), ..self }
    }
}

impl Chrome {
    /// Does the scene draw the bar?
    fn scene(self) -> bool {
        matches!(self, Chrome::Scene | Chrome::SceneAt(_))
    }

    /// Where the app wants the native buttons, if it said.
    fn lights(self) -> Option<(f64, f64, Option<f64>)> {
        match self {
            Chrome::SceneAt(lights) => Some((lights.x, lights.y, lights.size)),
            _ => None,
        }
    }
}

// =============================================================================
// The app, and the windows it holds
// =============================================================================

/// What a window is before it exists: its bar, its size, who draws its
/// top edge, and what the OS lets the reader do to it.
///
/// ```ignore
/// // the workbench: it resizes, it minimizes, the scene draws the bar
/// WindowSpec::titled("Trinity").size(1280.0, 800.0).chrome(Chrome::SceneAt(Lights::at(16.0, 14.0)))
/// // a door has ONE size
/// WindowSpec::titled("Trinity").size(1040.0, 660.0).fixed().no_minimize()
/// ```
#[derive(Clone, Debug)]
pub struct WindowSpec {
    title: Rc<str>,
    size: Size,
    chrome: Chrome,
    manners: ffi::Manners,
}

impl WindowSpec {
    /// A window named for its bar, at the house size — the OS names it
    /// by this in Mission Control and the Dock even when the scene
    /// draws the top edge itself.
    pub fn titled(title: impl Into<Rc<str>>) -> WindowSpec {
        WindowSpec {
            title: title.into(),
            size: Size { width: 1024.0, height: 640.0 },
            chrome: Chrome::Native,
            manners: ffi::Manners::default(),
        }
    }

    /// The content size the window opens at, centred on the screen.
    pub fn size(mut self, width: f64, height: f64) -> WindowSpec {
        self.size = Size { width, height };
        self
    }

    /// Who draws the top edge — see [`Chrome`].
    pub fn chrome(mut self, chrome: Chrome) -> WindowSpec {
        self.chrome = chrome;
        self
    }

    /// One size, and no other: the reader cannot resize it.
    pub fn fixed(mut self) -> WindowSpec {
        self.manners.resizable = false;
        self
    }

    /// It cannot be put away in the Dock — the yellow light draws dead,
    /// which is what a window that must be answered looks like.
    pub fn no_minimize(mut self) -> WindowSpec {
        self.manners.minimizable = false;
        self
    }
}

/// A window's handle in an [`App`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct WindowId(usize);

/// Everything one window owns for as long as it is open. The app holds
/// these and routes every event to the one it belongs to.
struct Slot {
    /// The `NSWindow` as an address — what an event's source is
    /// compared against.
    window: usize,
    handler: RefCell<Box<dyn FnMut(AppEvent)>>,
    key_gate: RefCell<Box<dyn FnMut(&ffi::KeyStroke) -> bool>>,
    drag_gate: Box<dyn Fn(f64, f64) -> bool>,
    ime_index: Box<dyn Fn(f64, f64) -> Option<u64>>,
    ime_rect: Box<dyn Fn(u64) -> Option<ffi::CGRect>>,
    on_web: Box<dyn Fn(webview::WebviewEvent)>,
}

/// The application: the run loop, and the windows on it.
///
/// [`run_window_chrome`] is one window and this is several — the sign-in
/// door that opens before the workbench, and the second workbench a
/// reader opens on another project. Every window has its own
/// [`Runtime`] (its own scene, its own keymap, its own focus) and the
/// app routes each event to the window it came from.
///
/// ```ignore
/// let app = App::new();
/// let runtime = app.runtime().text_engine(Rc::new(CoreTextEngine::new()));
/// app.open(WindowSpec::titled("Trinity").size(1040.0, 660.0).fixed(), Rc::new(runtime), gate);
/// app.run();
/// ```
///
/// The app quits when its LAST window closes, which is the single-window
/// contract said again.
///
/// **Production gotchas.** The frame beat — the display link and the
/// caret blink — is one per app and reaches every window; a window that
/// wants no frames simply does nothing with the tick. A page's own news
/// (navigation, console, an eval answer) is offered to every window and
/// answered by the one that owns the page, so hosting webviews in two
/// windows costs a walk of the list per event and nothing else. Opening
/// and closing a window from inside an event — a sign-out that raises
/// the door and takes the workbench down — is the ordinary case and is
/// safe: the AppKit ceremony runs with the scene's ears closed, because
/// its notifications are SYNCHRONOUS and would re-enter the very
/// handler that asked.
#[derive(Clone)]
pub struct App {
    inner: Rc<AppInner>,
}

/// The half the platform gates hold: they outlive any borrow of the
/// app, so what they capture is a handle and never a reference.
struct AppInner {
    slots: RefCell<Vec<Rc<Slot>>>,
    routed: std::cell::Cell<bool>,
    scenes: std::cell::Cell<usize>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An app with no windows yet.
    pub fn new() -> App {
        // the app's life outside its windows opens with the app: the
        // delegate, the workspace's sleep and wake, the notifier
        life::install();
        App {
            inner: Rc::new(AppInner {
                slots: RefCell::new(Vec::new()),
                routed: std::cell::Cell::new(false),
                scenes: std::cell::Cell::new(0),
            }),
        }
    }

    /// A runtime for the next window — named for its own scene, so two
    /// windows showing the same view are two trees and not one.
    ///
    /// Dress it before handing it back ([`Runtime::text_engine`], the
    /// keymap, the host's action handlers): the app never touches it
    /// again.
    pub fn runtime(&self) -> Runtime {
        let seq = self.inner.scenes.get();
        self.inner.scenes.set(seq + 1);
        Runtime::scene(format!("w{seq}"))
    }

    /// Raises a window on `runtime`, showing `root`.
    pub fn open(&self, spec: WindowSpec, runtime: Rc<Runtime>, root: impl View) -> WindowId {
        let slot = mount(&spec, runtime, root);
        let id = WindowId(slot.window);
        self.inner.slots.borrow_mut().push(slot.clone());
        self.inner.clone().route();
        // Its own first frame, through its OWN handler and not the global
        // road: a window is very often opened from inside an event (a
        // sign-in that raises the workbench), and the road is busy carrying
        // that event. This is a direct call to a handler nobody is inside,
        // which is what "paint this window now" actually means.
        (slot.handler.borrow_mut())(AppEvent::Redraw);
        id
    }

    /// Closes a window. The app stays up unless it was the last.
    pub fn close(&self, id: WindowId) {
        // the delegate still runs (the window leaves the registry and
        // the frame beat moves house if it lived here), but its
        // notification does NOT re-enter the scene: this is usually
        // called from inside the handler of the very window that asked
        ffi::lend_hand(|| ffi::close_top_level(id.0));
        self.inner.buried(id.0);
    }

    /// The windows the app has open, oldest first.
    pub fn windows(&self) -> Vec<WindowId> {
        self.inner.slots.borrow().iter().map(|slot| WindowId(slot.window)).collect()
    }

    /// Enters the AppKit run loop. Returns when the app terminates.
    pub fn run(&self) {
        ffi::run();
    }
}

impl AppInner {
    /// Installs the one set of platform gates, routing each to the
    /// window the event came from. Idempotent — the first `open` arms
    /// it and the rest ride it.
    fn route(self: Rc<Self>) {
        if self.routed.replace(true) {
            return;
        }
        let app = Rc::clone(&self);
        ffi::set_handler(Box::new(move |event| {
            let source = ffi::event_source();
            let gone = matches!(event, AppEvent::WindowClosed);
            for slot in app.live() {
                // source 0 is a beat every window shares
                if source == 0 || slot.window == source {
                    let mut handler = slot.handler.borrow_mut();
                    handler(event.clone());
                }
            }
            if gone {
                app.buried(source);
            }
        }));
        let app = Rc::clone(&self);
        ffi::set_key_gate(Box::new(move |stroke| {
            app.addressed().is_some_and(|slot| (slot.key_gate.borrow_mut())(stroke))
        }));
        let app = Rc::clone(&self);
        ffi::set_drag_gate(Box::new(move |x, y| {
            app.addressed().is_some_and(|slot| (slot.drag_gate)(x, y))
        }));
        let index_app = Rc::clone(&self);
        let rect_app = Rc::clone(&self);
        ffi::set_ime_resolvers(
            Box::new(move |x, y| {
                index_app.addressed().and_then(|slot| (slot.ime_index)(x, y))
            }),
            Box::new(move |utf16| {
                rect_app.addressed().and_then(|slot| (slot.ime_rect)(utf16))
            }),
        );
        let app = Rc::clone(&self);
        webview::set_dispatch(move |event| {
            // a page belongs to ONE window and every other answers
            // "not mine" — the walk is the routing
            for slot in app.live() {
                (slot.on_web)(event.clone());
            }
        });
    }

    /// A snapshot of the open slots — taken before any handler runs, so
    /// a window that opens or closes inside one does not disturb the
    /// walk.
    fn live(&self) -> Vec<Rc<Slot>> {
        self.slots.borrow().clone()
    }

    /// The window the event now being answered belongs to.
    fn addressed(&self) -> Option<Rc<Slot>> {
        let source = ffi::event_source();
        self.live().into_iter().find(|slot| source == 0 || slot.window == source)
    }

    /// Drops the slot of a window that has gone.
    fn buried(&self, window: usize) {
        self.slots.borrow_mut().retain(|slot| slot.window != window);
    }
}

/// Like [`run_window`], but with the `Runtime` assembled by the caller —
/// the path for apps with their own environment (the text engine is still
/// the assembler's responsibility).
pub fn run_window_with(title: &str, size: Size, runtime: Runtime, root: impl View) {
    run_window_chrome(title, size, Chrome::Native, runtime, root)
}

/// Like [`run_window_with`], choosing who draws the window's top edge.
///
/// One window, and the loop: the sugar over [`App`] every single-window
/// app is.
pub fn run_window_chrome(
    title: &str,
    size: Size,
    chrome: Chrome,
    runtime: Runtime,
    root: impl View,
) {
    let app = App::new();
    app.open(WindowSpec::titled(title).size(size.width, size.height).chrome(chrome), Rc::new(runtime), root);
    app.run();
}

/// Raises the window `spec` asks for and wires everything that lives as
/// long as it does — the frame path, the pools, the gates and the event
/// handler — into a slot the [`App`] holds and routes to.
fn mount(spec: &WindowSpec, runtime: Rc<Runtime>, root: impl View) -> Rc<Slot> {
    let title: &str = &spec.title;
    let size = spec.size;
    let chrome = spec.chrome;
    // the placement is armed before the window exists: the buttons are
    // born with it, and the first frame already has them in place
    ffi::set_traffic_lights(chrome.lights());
    // the window is raised with the scene's ears closed: AppKit's key
    // notifications are SYNCHRONOUS, and a window opened from inside an
    // event would re-enter the handler that asked for it
    let window = ffi::lend_hand(|| {
        ffi::create_window(title, size.width, size.height, chrome.scene(), spec.manners)
    });
    // a task that lands on a worker thread asks the main run loop for
    // one more turn; the frame it takes drains the queue on its way
    ffi::install_wake_source();
    runtime.set_wake_hook(std::sync::Arc::new(ffi::wake_from_any_thread));
    // two owners: the keyboard gate and the event handler
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
    // the open DIALOGS' real windows, pooled the same way — a closed
    // dialog's window stays reusable-dead like a panel, and reopening
    // re-adopts it
    let dialogs: Rc<RefCell<std::collections::HashMap<String, ffi::WindowHandle>>> =
        Rc::new(RefCell::new(std::collections::HashMap::new()));
    // each dialog's retained surface — the main CPU road's discipline
    // (damage only), so hover inside a dialog repaints a row, never
    // the whole card
    type DialogSurface = (bunny_ui::raster::Surface, usize, bunny_ui::layout::Color);
    let dialog_surfaces: Rc<RefCell<std::collections::HashMap<String, DialogSurface>>> =
        Rc::new(RefCell::new(std::collections::HashMap::new()));
    // The scene a popover's MATERIAL samples, kept per panel beside the base
    // commands it was rasterized from. A card that scrolls its own text does
    // not move the window behind it, and re-rasterizing that window every
    // frame to hand the pane the same pixels is fifteen milliseconds a frame
    // spent proving nothing changed.
    type Beneath = (Vec<bunny_ui::layout::DrawCommand>, (usize, usize), bunny_ui::raster::Bitmap);
    let beneaths: Rc<RefCell<std::collections::HashMap<String, Beneath>>> =
        Rc::new(RefCell::new(std::collections::HashMap::new()));
    // The commands each host SEGMENT was last rasterized from, with
    // the content box and scale they were rasterized at — the ledger
    // that keeps a steady frame from re-proving the same pixels (the
    // beneaths' twin, for the sandwich above the island).
    type SegmentKept =
        (Vec<bunny_ui::layout::DrawCommand>, (f64, f64, f64, f64), usize);
    let segments_kept: Rc<RefCell<std::collections::HashMap<String, SegmentKept>>> =
        Rc::new(RefCell::new(std::collections::HashMap::new()));
    // present takes a READY display list to the window — the tick path
    // reuses it without paying settle, effects or the IME tail
    let present: Rc<dyn Fn(&Runtime, bunny_ui::layout::DisplayList, trace::Origin)> = Rc::new({
        let surface = Rc::clone(&surface);
        let panels = Rc::clone(&panels);
        let dialogs = Rc::clone(&dialogs);
        let dialog_surfaces = Rc::clone(&dialog_surfaces);
        let segments_kept = Rc::clone(&segments_kept);
        let beneaths = Rc::clone(&beneaths);
        move |runtime: &Runtime,
              full_display: bunny_ui::layout::DisplayList,
              via: trace::Origin| {
            let (width, height) = window.content_size();
            let scale = window.scale();
            let canvas = bunny_ui::theme::canvas();
            let physical = ((width.round() as usize) * scale, (height.round() as usize) * scale);
            let live_resize = window.in_live_resize();
            // BUNNY_PRESENT_TRACE=1: the tape a trembling resize is
            // diagnosed from — what presented, at which size, on whose
            // ask, how long each stage took, and whether the window
            // moved under it. See the `trace` module for the format.
            let mut traced =
                trace::begin(width, height, live_resize, full_display.len(), via);
            // the window presents everything BEFORE the first popover;
            // each popover re-presents its own slice on a child panel
            // in screen coordinates — that is how it leaves the window
            // the native hosts FIRST — before the window's own
            // present, before the popover panels, before anything
            // that blocks. A hosted engine renders OUT of process:
            // the sooner it holds its frame, the sooner its relayout
            // runs — in parallel with everything below — and a live
            // resize stops reading as the page chasing the window.
            // Mounted on first sight, placed every frame, swept when
            // the subtree goes; the flip is into the LAYOUT's world,
            // like the live layers.
            let hosts = runtime.hosts();
            let placed =
                runtime.last_viewport().map_or(height, |viewport| viewport.height);
            for host in &hosts {
                let bunny_ui::host::HostSpec::Webview {
                    url,
                    document,
                    scripts,
                    console,
                    requests,
                    full_motion,
                } = &host.spec;
                // the stamp fingerprints the whole spec: a changed
                // url, script set or declared hook re-instructs; the
                // separators are control characters no url or script
                // spells. `full_motion` rides for symmetry — this
                // backend does not serve MediaEmulation (WKWebView
                // offers no public override), but the stamp must not
                // lie about what the spec says
                let mut stamp = String::from(&**url);
                // a document stamps by its fingerprint, never by its
                // pages — the letter is the app's to hold, not the
                // stamp's to copy every frame
                if let Some(document) = document {
                    stamp.push('\u{3}');
                    stamp.push_str(&format!("{:016x}", document.digest));
                }
                stamp.push('\u{2}');
                stamp.push(if *console { 'c' } else { '-' });
                stamp.push(if *requests { 'r' } else { '-' });
                stamp.push(if *full_motion { 'm' } else { '-' });
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
                    placed,
                    || webview::create(&host.path, &host.spec),
                    |child, _stamp| webview::update(&host.path, child, &host.spec),
                );
            }
            let alive = hosts.iter().map(|host| host.path.clone()).collect::<Vec<_>>();
            window.host_sweep(&alive);
            webview::sweep(&alive);
            traced.stage("H", format_args!("hosts={}", hosts.len()));
            let overlays = runtime.overlays();
            let display = match overlays.first() {
                Some(first) => full_display.translated_slice((0, first.display.0), 0.0, 0.0),
                None => full_display.clone(),
            };
            // the sandwich: what painted ABOVE a host leaves the
            // window's present and composites on a segment surface
            // between the platform views — paint order stays the
            // truth over the islands. A scene with nothing after its
            // hosts answers nothing here and pays nothing. Every
            // carve below cuts from the ORIGINAL list: ranges never
            // chase indices another carve already moved.
            // only what actually LANDS on an island leaves the scene —
            // command by command, each under the clips that governed
            // it. A focus ring hugging the pane lifts alone; the
            // stripe beside it and the status bar under it STAY, and a
            // scene with nothing on its islands pays exactly what it
            // paid before the sandwich existed.
            //
            // WHILE THE WINDOW CHANGES SIZE the segments come home,
            // under the law the live boxes already obey: a segment's
            // commands change on every step of a resize, and a
            // changed segment is a CPU raster of its whole box INSIDE
            // the present. A border hugging an island rasterized the
            // island — measured at ~3.3M pixels and ~165 ms per step,
            // an 18-step drag where the same gesture without it ran
            // 198 — so the drag starved and the compositor stretched
            // stale frames between rare presents, which is what a
            // trembling resize IS. Home, the commands paint in the
            // drawable and move with the window by construction; the
            // island covers its one-point overlap for the length of
            // the gesture, and the end-of-gesture Redraw mints the
            // segment back.
            let mut segment_ranges: Vec<(usize, usize)> = Vec::new();
            let segments: Vec<(String, bunny_ui::layout::DisplayList)> = if live_resize {
                Vec::new()
            } else {
                runtime
                    .host_segments(full_display.len())
                    .into_iter()
                    .filter_map(|(path, range)| {
                        let host = hosts.iter().find(|host| {
                            host.path == path
                                && host.visible.size.width > 0.0
                                && host.visible.size.height > 0.0
                        })?;
                        let island = bunny_ui::layout::Rect {
                            origin: bunny_ui::layout::Point {
                                x: host.frame.origin.x + host.visible.origin.x,
                                y: host.frame.origin.y + host.visible.origin.y,
                            },
                            size: host.visible.size,
                        };
                        let (carves, lifted) =
                            bunny_ui::raster::carve_covering(&full_display, range, island)?;
                        segment_ranges.extend(carves);
                        Some((path, lifted))
                    })
                    .collect()
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
                    beneaths.borrow_mut().remove(&path);
                }
                // the dialogs' own sweep, AFTER the panels': a dropdown
                // inside a closing dialog detaches from the dialog
                // first. The window stays pooled reusable-dead like a
                // panel; the ceremony runs under `lend_hand` — the
                // key-travel notifications AppKit fires synchronously
                // must not re-enter the handler mid-present.
                let mut dialog_store = dialogs.borrow_mut();
                let dead_dialogs: Vec<String> = dialog_store
                    .iter()
                    .filter(|(path, dialog)| {
                        dialog.is_visible()
                            && !overlays.iter().any(|overlay| &overlay.path == *path)
                    })
                    .map(|(path, _)| path.clone())
                    .collect();
                for path in dead_dialogs {
                    if let Some(dialog) = dialog_store.get(&path) {
                        ffi::lend_hand(|| {
                            // the flag drops FIRST: the make-key below
                            // asks the parent `canBecomeKeyWindow`, and
                            // the answer has to already be yes. Safe
                            // over a fullscreen parent too — its space
                            // is the one on screen, so re-keying it
                            // switches nothing.
                            ffi::end_window_modal(&window);
                            dialog.close_panel(&window);
                            window.make_key_with_view();
                        });
                    }
                    dialog_surfaces.borrow_mut().remove(&path);
                }
                for overlay in &overlays {
                    // a dialog overlay presents on a REAL window, not a
                    // panel — raised on first sight, held to the frame
                    // layout answered (which is the frame the window
                    // itself reported through `Runtime::set_dialog_frame`,
                    // so a steady frame is a no-op under the ε guard)
                    if let bunny_ui::layout::OverlaySurface::Window(spec) = &overlay.surface {
                        let x = overlay.frame.origin.x;
                        let y = overlay.frame.origin.y;
                        let w = overlay.frame.size.width;
                        let h = overlay.frame.size.height;
                        let created = !dialog_store.contains_key(&overlay.path);
                        // scene chrome: the header owns the top edge and
                        // the native lights sit where the spec says
                        let lights = match &spec.chrome {
                            bunny_ui::layout::DialogChrome::Native => None,
                            bunny_ui::layout::DialogChrome::Scene { lights } => {
                                Some((lights.x, lights.y))
                            }
                        };
                        let dialog =
                            *dialog_store.entry(overlay.path.clone()).or_insert_with(|| {
                                ffi::lend_hand(|| {
                                    ffi::create_dialog(
                                        &window,
                                        spec.title.as_ref(),
                                        w,
                                        h,
                                        spec.min.width,
                                        spec.min.height,
                                        lights,
                                    )
                                })
                            });
                        let opening = created || !dialog.is_visible();
                        ffi::lend_hand(|| {
                            if opening && !created {
                                // a pooled window lost its child tie on
                                // close — re-adopt before it fronts
                                dialog.attach_to(&window);
                            }
                            dialog.set_content_frame_screen(
                                window.layout_rect_to_screen(x, y, w, h),
                            );
                            if opening {
                                // the modal ceremony: the parent's
                                // lights go dark, its key is refused,
                                // and the keyboard moves into the
                                // dialog
                                ffi::begin_window_modal(&window);
                                dialog.make_key_with_view();
                            }
                        });
                        dialog.set_scene_origin(x, y);
                        if opening {
                            // the hover painted at open time froze over
                            // a parent that is now inert — clear it
                            let _ = runtime.pointer_exited();
                        }
                        // the slice presents by the main CPU road's
                        // discipline: a retained surface, damage only.
                        // No BLEED and no backdrop sampling — the
                        // chrome and the shadow are the system's own.
                        let slice = full_display.translated_slice(overlay.display, -x, -y);
                        // the GPU road first: a grafted dialog presents
                        // its slice on its own layer, and pays no
                        // Surface, no RGBA mirror and no blit — which is
                        // the whole of what a resize step used to cost
                        if metal::present_view(
                            dialog.view(),
                            &slice,
                            Size { width: w, height: h },
                            scale,
                            canvas,
                            &*runtime.text(),
                            &*runtime.images(),
                        ) {
                            continue;
                        }
                        let physical =
                            ((w.round() as usize) * scale, (h.round() as usize) * scale);
                        let mut kept = dialog_surfaces.borrow_mut();
                        let stale = match kept.get(&overlay.path) {
                            Some((retained, retained_scale, retained_canvas)) => {
                                retained.bitmap().width() != physical.0
                                    || retained.bitmap().height() != physical.1
                                    || *retained_scale != scale
                                    || *retained_canvas != canvas
                            }
                            None => true,
                        };
                        if stale {
                            kept.insert(
                                overlay.path.clone(),
                                (
                                    bunny_ui::raster::Surface::new(
                                        physical.0, physical.1, scale, canvas,
                                    ),
                                    scale,
                                    canvas,
                                ),
                            );
                        }
                        let (retained, _, _) =
                            kept.get_mut(&overlay.path).expect("surface for the dialog");
                        let damage =
                            retained.frame(slice, &*runtime.text(), &*runtime.images());
                        if !damage.is_empty() {
                            dialog.blit_partial(
                                physical.0,
                                physical.1,
                                retained.rgba(),
                                &damage,
                            );
                        }
                        continue;
                    }
                    // the panel is BLED around the frame so the card's
                    // own shadow has room — the same pixels every
                    // target paints, no system shadow involved
                    const BLEED: f64 = 32.0;
                    let x = overlay.frame.origin.x - BLEED;
                    let y = overlay.frame.origin.y - BLEED;
                    let w = overlay.frame.size.width + 2.0 * BLEED;
                    let h = overlay.frame.size.height + 2.0 * BLEED;
                    // a popover born inside a dialog is the DIALOG's
                    // child: it stacks above the dialog and rides its
                    // moves — the identity path says whose it is
                    let host = dialog_store
                        .iter()
                        .find(|(dialog_path, dialog)| {
                            dialog.is_visible()
                                && overlay.path.starts_with(dialog_path.as_str())
                        })
                        .map_or(window, |(_, dialog)| *dialog);
                    let panel = store
                        .entry(overlay.path.clone())
                        .or_insert_with(|| ffi::create_panel(&host, w, h));
                    panel.set_frame_screen(window.layout_rect_to_screen(x, y, w, h));
                    panel.set_scene_origin(x, y);
                    let slice = full_display.translated_slice(overlay.display, -x, -y);
                    let panel_physical =
                        ((w.round() as usize) * scale, (h.round() as usize) * scale);
                    // What a MATERIAL inside the popover samples. The panel
                    // carries only the popover's own commands, so a pane in
                    // one would otherwise read transparency and show nothing
                    // — which is a glass card with no glass on it. The window
                    // under the panel is rasterized into the panel's own
                    // pixels and handed over for sampling ONLY: the panel has
                    // to stay transparent everywhere the popover does not
                    // paint, so this is never drawn.
                    //
                    // Skipped when the popover asks for no material, which is
                    // every menu and every tooltip.
                    let wants_backdrop = slice.iter().any(|command| {
                        matches!(command, bunny_ui::layout::DrawCommand::Backdrop { .. })
                    });
                    if wants_backdrop {
                        let base = full_display.translated_slice(
                            (0, overlays.first().map_or(full_display.len(), |o| o.display.0)),
                            -x,
                            -y,
                        );
                        let commands: Vec<_> = base.iter().cloned().collect();
                        let mut kept = beneaths.borrow_mut();
                        let stale = kept.get(&overlay.path).is_none_or(|(was, size, _)| {
                            *size != panel_physical || *was != commands
                        });
                        if stale {
                            let fresh = bunny_ui::raster::rasterize_with(
                                &base,
                                panel_physical.0,
                                panel_physical.1,
                                scale,
                                canvas,
                                &*runtime.text(),
                                &*runtime.images(),
                            );
                            kept.insert(overlay.path.clone(), (commands, panel_physical, fresh));
                        }
                    } else {
                        beneaths.borrow_mut().remove(&overlay.path);
                    }
                    let kept = beneaths.borrow();
                    let bitmap = bunny_ui::raster::rasterize_over(
                        &slice,
                        panel_physical.0,
                        panel_physical.1,
                        scale,
                        bunny_ui::layout::Color { r: 0, g: 0, b: 0, a: 0 },
                        &*runtime.text(),
                        &*runtime.images(),
                        kept.get(&overlay.path).map(|(_, _, bitmap)| bitmap),
                    );
                    drop(kept);

                    // a raster onto a transparent ground is ALREADY
                    // premultiplied (rgb = colour x coverage) — exactly
                    // what the panel's CGImage declares. Multiplying
                    // here again squared the alpha: card bodies are
                    // opaque and never showed it, their soft shadows
                    // were quietly half as deep as designed
                    let rgba = bitmap.to_rgba_bytes();
                    panel.blit_partial(
                        panel_physical.0,
                        panel_physical.1,
                        &rgba,
                        &[(0, 0, panel_physical.0 as i64, panel_physical.1 as i64)],
                    );
                }
            }
            traced.stage("O", format_args!("panels={}", overlays.len()));
            if metal::active() {
                // GPU present: the same display list, no Surface in the
                // path — the drawable is the frame. The LIVE boxes are
                // carved out: their commands would churn the atlas on
                // every step, so each presents on its own sublayer and
                // the drawable keeps the hole (the ground behind the
                // box paints; the layer composites over it).
                //
                // WHILE THE WINDOW CHANGES SIZE they come home. A
                // sublayer is placed by us and the window frame by the
                // system, and the two land in different beats — a drag
                // makes that beat visible as a mark that trails the
                // corner it sits in. Inside the drawable there is no
                // second beat: the presenter already commits it in the
                // resize's own transaction, so the box moves with the
                // window by construction. The layers come back, seeded
                // afresh, when the hand lets go.
                let resizing = live_resize;
                if resizing {
                    window.live_layer_sweep(&[]);
                    runtime.forget_live_surfaces();
                }
                let live = if resizing { Vec::new() } else { runtime.live_slices() };
                // one carve, all ranges against the original indices
                // (a live box above a host is carved TWICE over the
                // same commands, which removes them once — its hidden
                // layer below the page is waste the segment covers)
                let mut carve = live.clone();
                carve.extend(segment_ranges.iter().copied());
                if carve.is_empty() {
                    metal::present_window(
                        &display,
                        Size { width, height },
                        scale,
                        canvas,
                        &*runtime.text(),
                        &*runtime.images(),
                    );
                } else {
                    metal::present_window(
                        &display.without_slices(&carve),
                        Size { width, height },
                        scale,
                        canvas,
                        &*runtime.text(),
                        &*runtime.images(),
                    );
                    // an ordinary frame repaints only the live boxes
                    // whose picture changed OR whose size did (the
                    // ledger decides), and re-places every layer so a
                    // moved bar carries its mark along for the cost of
                    // a frame set. The flip is into the LAYOUT's world,
                    // which mid-resize is not the view's.
                    let placed = runtime
                        .last_viewport()
                        .map_or(height, |viewport| viewport.height);
                    for blit in runtime.live_islands_all(scale) {
                        window.live_layer_blit(
                            &blit.path,
                            blit.frame.origin.x,
                            blit.frame.origin.y,
                            blit.frame.size.width,
                            blit.frame.size.height,
                            placed,
                            scale,
                            blit.width,
                            blit.height,
                            &blit.rgba,
                        );
                    }
                    for (path, frame) in runtime.live_frames() {
                        window.live_layer_place(
                            &path,
                            frame.origin.x,
                            frame.origin.y,
                            frame.size.width,
                            frame.size.height,
                            placed,
                        );
                    }
                }
                if !resizing {
                    window.live_layer_sweep(&runtime.live_paths());
                }
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
                // the CPU surface keeps the same law: the segments'
                // commands leave the window and ride their surfaces
                let display = if segment_ranges.is_empty() {
                    display
                } else {
                    display.without_slices(&segment_ranges)
                };
                let damage = retained.frame(display, &*runtime.text(), &*runtime.images());
                if !damage.is_empty() {
                    // present only the wounds: damage-only mirror sync +
                    // damage-only backing copy + dirty-rect redraw
                    let (width, height) =
                        (retained.bitmap().width(), retained.bitmap().height());
                    window.blit_partial(width, height, retained.rgba(), &damage);
                }
            }
            traced.stage(
                "M",
                format_args!("sync={}", u8::from(metal::active() && live_resize)),
            );
            // the segments themselves: rasterized only when their
            // commands changed (the ledger's answer, the beneaths'
            // discipline), blitted between the platform views, swept
            // the frame nothing paints above a host
            {
                let mut kept = segments_kept.borrow_mut();
                let mut alive: Vec<String> = Vec::new();
                let mut rastered = 0usize;
                let mut raster_px = 0usize;
                for (host, slice) in &segments {
                    // the surface is the CONTENT's box, never the
                    // window: a ring's segment rasterizes a ring —
                    // which is what keeps a live resize at the
                    // window's own pace
                    let Some(bounds) =
                        bunny_ui::raster::list_bounds(slice, &*runtime.text())
                    else {
                        continue; // nothing paints — nothing to lift
                    };
                    let pad = 2.0;
                    let x0 = (bounds.origin.x - pad).max(0.0);
                    let y0 = (bounds.origin.y - pad).max(0.0);
                    let x1 = (bounds.origin.x + bounds.size.width + pad).min(width);
                    let y1 = (bounds.origin.y + bounds.size.height + pad).min(height);
                    if x1 <= x0 || y1 <= y0 {
                        continue;
                    }
                    alive.push(host.clone());
                    let frame = (x0, y0, x1 - x0, y1 - y0);
                    let commands: Vec<_> = slice.iter().cloned().collect();
                    let stale = kept.get(host).is_none_or(|(was, box_, at)| {
                        *was != commands || *box_ != frame || *at != scale
                    });
                    if !stale {
                        // same picture — at most a new flip height
                        window.segment_place(host, frame, placed);
                        continue;
                    }
                    let box_physical = (
                        ((x1 - x0) * scale as f64).round().max(1.0) as usize,
                        ((y1 - y0) * scale as f64).round().max(1.0) as usize,
                    );
                    let local = slice.translated_slice((0, slice.len()), -x0, -y0);
                    let bitmap = bunny_ui::raster::rasterize_over(
                        &local,
                        box_physical.0,
                        box_physical.1,
                        scale,
                        bunny_ui::layout::Color { r: 0, g: 0, b: 0, a: 0 },
                        &*runtime.text(),
                        &*runtime.images(),
                        None,
                    );
                    window.segment_blit(
                        host,
                        host,
                        &bitmap.to_rgba_bytes(),
                        frame,
                        placed,
                        scale,
                        box_physical.0,
                        box_physical.1,
                    );
                    kept.insert(host.clone(), (commands, frame, scale));
                    rastered += 1;
                    raster_px += box_physical.0 * box_physical.1;
                }
                kept.retain(|key, _| alive.iter().any(|host| host == key));
                window.segment_sweep(&alive);
                traced.stage(
                    "S",
                    format_args!("n={} raster={rastered} px={raster_px}", alive.len()),
                );
            }
        }
    });
    let blit = {
        let present = Rc::clone(&present);
        let dialogs = Rc::clone(&dialogs);
        move |runtime: &Runtime, root: &_, via: trace::Origin| {
            // the handles' commands are spent BEFORE the frame
            // renders: the state an expired eval writes lands in this
            // very layout, and a navigation the app just asked for is
            // already the engine's when the frame goes up
            for op in runtime.webview_commands() {
                use bunny_ui::host::WebviewOp;
                match op {
                    WebviewOp::Navigate { path, url } => {
                        if let Some(child) = ffi::host_child(&path) {
                            webview::navigate(child, &url);
                        }
                    }
                    WebviewOp::Back { path } => {
                        if let Some(child) = ffi::host_child(&path) {
                            webview::back(child);
                        }
                    }
                    WebviewOp::Forward { path } => {
                        if let Some(child) = ffi::host_child(&path) {
                            webview::forward(child);
                        }
                    }
                    // an eval with no page answers NOW, with a name —
                    // never silence that looks like a slow page
                    WebviewOp::Eval { path, token, js, raw } => match ffi::host_child(&path) {
                        Some(child) => webview::eval(child, token, &js, raw),
                        None => {
                            let _ = runtime.webview_eval_done(
                                token,
                                Err("the webview is not mounted".into()),
                            );
                        }
                    },
                    WebviewOp::Snapshot { path, token } => match ffi::host_child(&path) {
                        Some(child) => webview::snapshot(child, token),
                        None => {
                            let _ = runtime.webview_snapshot_done(
                                token,
                                Err("the webview is not mounted".into()),
                            );
                        }
                    },
                    // an edit on a document that left is spent on
                    // nothing, like the hand
                    WebviewOp::Edit { path, action } => {
                        if let Some(child) = ffi::host_child(&path) {
                            webview::edit(child, &action);
                        }
                    }
                    // a hand over a page that left is a hand over
                    // nothing: there is no answer to refuse in
                    WebviewOp::Input { path, event } => {
                        if let Some(child) = ffi::host_child(&path) {
                            webview::input(child, &event);
                        }
                    }
                }
            }
            let (width, height) = window.content_size();
            // a box that draws parts which TOUCH puts the shared edge
            // on a whole PIXEL — it needs the screen's scale
            runtime.set_device_scale(window.scale() as f64);
            // popovers position against the SCREEN, in layout
            // coordinates — overflow becomes plain geometry
            runtime.set_overlay_bounds(window.screen_bounds_in_layout().map(
                |(x, y, w, h)| bunny_ui::layout::Rect {
                    origin: bunny_ui::layout::Point { x, y },
                    size: Size { width: w, height: h },
                },
            ));
            // an open dialog's WINDOW is the truth of its frame: pull
            // it into the runtime before the pass, so this very layout
            // follows the user's drag, resize or zoom — the mirror
            // discipline `sync_ime` already keeps, in the other
            // direction
            for (path, dialog) in dialogs.borrow().iter() {
                if dialog.is_visible() {
                    let (x, y, w, h) = dialog.content_rect_in_layout(&window);
                    runtime.set_dialog_frame(
                        path,
                        bunny_ui::layout::Rect {
                            origin: bunny_ui::layout::Point { x, y },
                            size: Size { width: w, height: h },
                        },
                    );
                }
            }
            let display = runtime.display_frame(root, Size { width, height });
            present(runtime, display, via);
        let interaction = runtime.interaction();
        // a live divider drag keeps the resizer even while the pointer
        // runs ahead of the seam; hovering the grip announces it
        let desired = match runtime.seam_axis() {
            // lanes side by side: the seam travels left and right
            Some(Axis::Horizontal) => ffi::Cursor::ResizeLeftRight,
            // lanes stacked: it travels up and down
            Some(Axis::Vertical) => ffi::Cursor::ResizeUpDown,
            // The BOX under the pointer answers first — text wants an I-beam,
            // and the rule below cannot know that. Only where nobody answers
            // does the old rule stand: the hand over anything hoverable.
            None => match runtime.hovered_cursor() {
                Some(bunny_ui::layout::Cursor::Text) => ffi::Cursor::Text,
                Some(bunny_ui::layout::Cursor::Pointing) => ffi::Cursor::Pointing,
                Some(bunny_ui::layout::Cursor::Arrow) => ffi::Cursor::Arrow,
                None if interaction.hovered.is_some() => ffi::Cursor::Pointing,
                None => ffi::Cursor::Arrow,
            },
        };
        // over the island with only the DEFAULT to say, the shell
        // YIELDS: the engine owns the cursor over its own page (the
        // hand over a link is the webview's to give). Yielding also
        // rearms the gate, so the first real claim off the island —
        // or on it, a toast's hand — asserts again.
        let over_host = interaction.pointer.is_some_and(|point| {
            runtime.hosts().iter().any(|host| {
                let x0 = host.frame.origin.x + host.visible.origin.x;
                let y0 = host.frame.origin.y + host.visible.origin.y;
                point.x >= x0
                    && point.y >= y0
                    && point.x < x0 + host.visible.size.width
                    && point.y < y0 + host.visible.size.height
            })
        });
        if over_host && desired == ffi::Cursor::Arrow {
            ffi::yield_cursor();
        } else {
            window.set_cursor(desired);
        }
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
        sync_frame_driver(runtime);
        }
    };

    // the drag gate: a press on a `.window_drag_region()` (with no
    // interactive target above it) moves the window — the scene's own
    // title bar on a chrome-less window
    let drag_gate: Box<dyn Fn(f64, f64) -> bool> = Box::new({
        let runtime = Rc::clone(&runtime);
        move |x, y| runtime.window_drag_at(x, y)
    });

    // the gate: keymap BEFORE the input system — bare chars pass straight
    // through to whoever holds the keyboard AND is taking text (typing is
    // never stolen; a modal box in command mode declines and the stroke
    // walks on); a binding with no handler mounted does not consume (the
    // palette-less screen types fine)
    let key_gate: Box<dyn FnMut(&ffi::KeyStroke) -> bool> = Box::new({
        let runtime = Rc::clone(&runtime);
        let root = Rc::clone(&root);
        let blit = blit.clone();
        move |stroke: &ffi::KeyStroke| {
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
            // text back for the pasteboard
            let taken = runtime.key_stroke(Stroke::new(pattern, stroke.typed));
            if taken.handled {
                if let Some(text) = taken.text {
                    ffi::clipboard_write(&text);
                }
                blit(&runtime, &*root, trace::Origin::Input);
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
                blit(&runtime, &*root, trace::Origin::Input);
                return true;
            }
            let action = match runtime.chord(Stroke::new(pattern, stroke.typed)) {
                KeyMatch::Action(action) => action,
                // the stroke opened (or let go of) a sequence: it is
                // spent, and a which-key panel may have just changed
                KeyMatch::Pending => {
                    blit(&runtime, &*root, trace::Origin::Input);
                    return true;
                }
                KeyMatch::None => return false,
            };
            if runtime.dispatch_action(action) {
                blit(&runtime, &*root, trace::Origin::Input);
                true
            } else {
                false
            }
        }
    });

    // the input system's questions BEYOND the mirror: index under the
    // mouse (dictionary lookup) and rect at a composition index — both
    // answered live by the runtime
    let ime_index: Box<dyn Fn(f64, f64) -> Option<u64>> = Box::new({
        let runtime = Rc::clone(&runtime);
        move |x, y| runtime.ime_index_at(x, y).map(|index| index as u64)
    });
    let ime_rect: Box<dyn Fn(u64) -> Option<ffi::CGRect>> = {
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
        })
    };

    // what the pages report — navigations, the bus, eval answers —
    // lands here from WebKit's own runloop callbacks, and re-renders
    // exactly when a retained writer ran
    let on_web: Box<dyn Fn(webview::WebviewEvent)> = Box::new({
        let runtime = Rc::clone(&runtime);
        let root = Rc::clone(&root);
        let blit = blit.clone();
        move |event| {
            let root = &*root;
            match event {
                webview::WebviewEvent::Navigated { view, url } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_navigated(&path, &url)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::Linked { view, url } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_linked(&path, &url)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::Changed { view, html } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_changed(&path, &html)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::Pasted { view, html, text } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_pasted(&path, &html, &text)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::NavigationFailed { view, url, why } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_navigate_failed(&path, &url, &why)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::Posted { view, body } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_posted(&path, &body)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::Console { view, line } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_console(&path, &line)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::Requested { view, line } => {
                    if let Some(path) = ffi::host_key_of_child(view)
                        && runtime.webview_requested(&path, &line)
                    {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::EvalDone { token, result } => {
                    if runtime.webview_eval_done(token, result) {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
                webview::WebviewEvent::SnapshotDone { token, result } => {
                    let result = result.map(|(width, height, rgba)| {
                        bunny_ui::host::WebviewSnapshot { width, height, rgba }
                    });
                    if runtime.webview_snapshot_done(token, result) {
                        blit(&runtime, root, trace::Origin::Web);
                    }
                }
            }
        }
    });

    let handler_runtime = Rc::clone(&runtime);
    let handler_root = Rc::clone(&root);
    let handler_present = Rc::clone(&present);
    let handler_dialogs = Rc::clone(&dialogs);
    let handler: Box<dyn FnMut(AppEvent)> = Box::new(move |event| {
        let runtime = &handler_runtime;
        let root = &*handler_root;
        // mid-drag of a DIALOG the resize steps are the only presenter,
        // the same law the main window's drag already enforces below
        let dialog_resizing = || {
            handler_dialogs
                .borrow()
                .values()
                .any(|dialog| dialog.is_visible() && dialog.in_live_resize())
        };
        match event {
        AppEvent::Redraw => blit(runtime, root, trace::Origin::Redraw),
        AppEvent::WindowClosed => {
            // Nothing to take down by hand: the popover panels and the
            // dialogs are CHILDREN of this window and AppKit closes
            // them with it, and the slot the app drops right after this
            // carries the runtime — and with it the retained tree, the
            // scene's world and every task hanging off it.
        }
        AppEvent::DialogClose { window: which } => {
            // the red button: the window did NOT close (the delegate
            // answered NO) — the overlay's dismissal flips the app's
            // binding, and the frame this blit draws takes the window
            // down through the ordinary sweep
            let path = handler_dialogs.borrow().iter().find_map(|(path, dialog)| {
                (dialog.raw_window() == which).then(|| path.clone())
            });
            if let Some(path) = path
                && runtime.dismiss_overlay(&path)
            {
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::Wake => {
            // Mid-drag there is exactly ONE presenter — the law the
            // tick path already obeys below. A worker's wake used to
            // present a whole scene between two steps of the resize:
            // measured on a workbench as 113 same-size presents
            // inside one 2.9 s gesture, every one via wake — a caret
            // clock and its kin racing the drag. The WORK is not held
            // back: the tasks are polled and their state lands; the
            // next step of the resize (or the end-of-gesture Redraw)
            // presents what they moved. Only the present yields.
            if window.in_live_resize() || dialog_resizing() {
                runtime.poll_tasks();
            } else {
                blit(runtime, root, trace::Origin::Wake);
            }
        }
        AppEvent::ResignKey => {
            // the user switched away: popovers close like the
            // platform's own (their panels never take key, so this
            // only ever fires on the parent) — and the decorations
            // freeze: they animate for eyes that are on them
            runtime.set_loops_paused(true);
            if runtime.dismiss_all_overlays() {
                blit(runtime, root, trace::Origin::Input);
            } else {
                sync_frame_driver(runtime);
            }
        }
        AppEvent::BecomeKey => {
            // the front returns: a frozen loop resumes mid-phase
            runtime.set_loops_paused(false);
            sync_frame_driver(runtime);
        }
        AppEvent::MouseMoved { x, y, modifiers } => {
            if runtime.pointer_moved(x, y, modifiers) {
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::RightMouseDown { x, y } => {
            // the runtime opens (or closes) the context menu; the
            // panel presents like any overlay — outside the window too
            if runtime.context_click(x, y) {
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::MouseDown { x, y, clicks, modifiers } => {
            if runtime.pointer_clicked(x, y, clicks, modifiers) {
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::MouseUp { x, y } => {
            // fires on up-inside; the pressed visual always clears
            let _ = runtime.pointer_released(x, y);
            blit(runtime, root, trace::Origin::Input);
        }
        AppEvent::MouseExited => {
            if runtime.pointer_exited() {
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::Wheel { x, y, dx, dy } => {
            // offset is engine state: repaint without render (zero bodies)
            if runtime.wheel(x, y, dx, dy) {
                blit(runtime, root, trace::Origin::Input);
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
                        blit(runtime, root, trace::Origin::Input);
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
                        blit(runtime, root, trace::Origin::Input);
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
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::Blink => {
            // an idle caret blinks; without focus the tick is silence —
            // and the same slow clock ages the tooltip's wait and then
            // shows it: the delay is this tick seen twice
            let blinked = runtime.blink();
            let explained = runtime.tooltip_tick();
            // the same slow beat ages a sequence in the air: two ticks
            // and `cmd-k` lets the keyboard go
            let chorded = runtime.chord_tick();
            // …but never mid-drag: the resize steps are the only presenter
            // there. A caret blinking in a focused URL bar presented whole
            // frames between two steps and the two geometries composited
            // as a ghost of the entire chrome (the page, being the
            // engine's own surface, stayed clean — which is what named
            // this path). The clock still aged; the caret shows its next
            // phase on the step that lands — and the RESIZE path itself
            // must never be gated: holding it back leaves the compositor
            // stretching a stale drawable through the whole drag.
            if (blinked || explained || chorded)
                && !window.in_live_resize()
                && !dialog_resizing()
            {
                blit(runtime, root, trace::Origin::Blink);
            }
        }
        AppEvent::Frame { dt } => {
            // the tick path: springs advance, then layout only — zero
            // bodies on a stable tree; settle and effects belong to the
            // real-event path. A tick that moved ONLY loop clocks does
            // even less: the live boxes repaint on their own layers and
            // the scene is not touched at all.
            let moved = runtime.tick(dt);
            // Mid-drag there is exactly ONE presenter: the resize step
            // itself, arriving at display cadence. A tick present lands
            // with its own latency BETWEEN two steps, so the window
            // shows sizes out of order and the whole drag trembles — an
            // app with a blinking caret or a hover under the held
            // pointer trembles, and an example with no clocks stays
            // fluid, which is exactly the pair that found this. The
            // clocks still advanced; the next step carries what they
            // moved, and a hand held still mid-drag parks the scene
            // until it moves — the same stillness every step ends in.
            if window.in_live_resize() || dialog_resizing() {
            } else if moved.scene {
                let (width, height) = window.content_size();
                let display = runtime.animation_frame(root, Size { width, height });
                handler_present(runtime, display, trace::Origin::Frame);
            } else if moved.islands {
                // mid-resize the boxes are in the drawable, not on
                // layers — a step repaints the scene like any other
                if metal::active() && !window.in_live_resize() {
                    let scale = window.scale();
                    // the flip is into the world the boxes were PLACED
                    // in, not the one the view measures — mid-resize
                    // the view is already the new size while the last
                    // layout is still the old one
                    let (_, measured) = window.content_size();
                    let height = runtime
                        .last_viewport()
                        .map_or(measured, |viewport| viewport.height);
                    for blit in runtime.live_islands(scale) {
                        window.live_layer_blit(
                            &blit.path,
                            blit.frame.origin.x,
                            blit.frame.origin.y,
                            blit.frame.size.width,
                            blit.frame.size.height,
                            height,
                            scale,
                            blit.width,
                            blit.height,
                            &blit.rgba,
                        );
                    }
                } else {
                    // the CPU path has no layers (the damage diff
                    // already confines the repaint to the box), and
                    // neither does a window mid-resize — there the box
                    // rides the drawable, so a step is a scene frame
                    let (width, height) = window.content_size();
                    let display = runtime.animation_frame(root, Size { width, height });
                    handler_present(runtime, display, trace::Origin::Frame);
                }
            }
            sync_frame_driver(runtime);
        }
        AppEvent::ImeInsert { text } => {
            // the IME commit (or plain typing through the input system)
            if runtime.key(EditCommand::Insert(text)).applied {
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::ImeMark { text, location, length } => {
            let command = EditCommand::SetMarked {
                text,
                caret_utf16: (location as usize, length as usize),
            };
            if runtime.key(command).applied {
                blit(runtime, root, trace::Origin::Input);
            }
        }
        AppEvent::ImeUnmark => {
            if runtime.key(EditCommand::Unmark).applied {
                blit(runtime, root, trace::Origin::Input);
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
                        blit(runtime, root, trace::Origin::Input);
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
                blit(runtime, root, trace::Origin::Input);
            }
        }
        }
        // EVERY event, not only the ones that repainted. An event can arm a
        // TIMER without changing a pixel — a hover that starts a settle, a
        // debounce waiting for typing to stop — and the pace those timers need
        // is exactly what `sync_frame_driver` reads. Syncing only inside
        // `blit` left the driver parked with a sleeper on the queue: the clock
        // is the frame tick, so the sleeper never woke, and the card it was
        // waiting for arrived on whatever unrelated click came next.
        sync_frame_driver(runtime);
    });

    Rc::new(Slot {
        window: window.raw_window(),
        handler: RefCell::new(handler),
        key_gate: RefCell::new(key_gate),
        drag_gate,
        ime_index,
        ime_rect,
        on_web,
    })
}

// =============================================================================
// BUNNY_PRESENT_TRACE — the tape a trembling present is diagnosed from
// =============================================================================

/// The present tape. `BUNNY_PRESENT_TRACE=1` writes one file per
/// process — `/tmp/bunny-present.<pid>.trace`, truncated on start, so
/// two processes never interleave on one tape. Any other value is used
/// as the path, with a literal `{pid}` replaced by the process id.
/// `BUNNY_TRACE_TAG` stamps the header with free text (a build or an
/// experiment name). Off, each mark costs one branch.
///
/// One event per line. Times are milliseconds from the first mark of
/// the process:
///
/// ```text
/// # bunny-trace v2 pid=<pid> t0=<unix_ms> tag=<tag>
/// R <ms> <w>x<h> kind=<resize|move|backing> live=<0|1>
/// P <ms> <w>x<h> live=<0|1> cmds=<n> via=<origin>
/// H <ms> dur=<ms> hosts=<n>
/// O <ms> dur=<ms> panels=<n>
/// M <ms> dur=<ms> sync=<0|1>
/// S <ms> dur=<ms> n=<alive> raster=<n> px=<n>
/// E <ms> dur=<ms>
/// X <ms> what=<name>
/// ```
///
/// `R` is a window callback (which notification asked, and at what
/// size). `P` opens a present; `H` (host pass), `O` (overlay panels),
/// `M` (scene presented, `sync` = inside the resize transaction) and
/// `S` (segments: mounted, rasterized, pixels) each carry the time
/// since the previous mark of the same present; `E` closes it with the
/// total. `X` names a one-time cost (`sync-on`, `sync-off`,
/// `buffer-grow`, `atlas-drain`, `segment-class`). `via` names the
/// code path that asked for the present: `redraw` (a window callback),
/// `wake` (a worker), `input` (an event), `frame` (the animation
/// tick), `web` (a page report), `blink` (the slow clock).
mod trace {
    use std::io::Write as _;

    /// The code path that asked for a present.
    #[derive(Clone, Copy)]
    pub(crate) enum Origin {
        Redraw,
        Wake,
        Input,
        Frame,
        Web,
        Blink,
    }

    impl Origin {
        fn name(self) -> &'static str {
            match self {
                Origin::Redraw => "redraw",
                Origin::Wake => "wake",
                Origin::Input => "input",
                Origin::Frame => "frame",
                Origin::Web => "web",
                Origin::Blink => "blink",
            }
        }
    }

    /// The tape, opened once — truncated, headed, and kept. Opening
    /// per line was measurable inside the present it was measuring.
    fn out() -> Option<&'static std::sync::Mutex<std::fs::File>> {
        static OUT: std::sync::OnceLock<Option<std::sync::Mutex<std::fs::File>>> =
            std::sync::OnceLock::new();
        OUT.get_or_init(|| {
            let value = std::env::var("BUNNY_PRESENT_TRACE").ok()?;
            let pid = std::process::id();
            let path = if value == "1" || value.is_empty() {
                format!("/tmp/bunny-present.{pid}.trace")
            } else {
                value.replace("{pid}", &pid.to_string())
            };
            let mut file = std::fs::File::create(path).ok()?;
            let t0 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |t| t.as_millis());
            let tag = std::env::var("BUNNY_TRACE_TAG").unwrap_or_default();
            let _ = writeln!(file, "# bunny-trace v2 pid={pid} t0={t0} tag={tag}");
            Some(std::sync::Mutex::new(file))
        })
        .as_ref()
    }

    /// True when the tape is on — the gate a caller checks before
    /// paying for anything a mark would need.
    pub(crate) fn active() -> bool {
        out().is_some()
    }

    fn ms() -> f64 {
        static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
    }

    fn line(args: std::fmt::Arguments<'_>) {
        if let Some(file) = out()
            && let Ok(mut file) = file.lock()
        {
            let _ = writeln!(file, "{args}");
        }
    }

    /// One line outside a present — the window callbacks (`R`) and the
    /// one-time costs (`X`).
    pub(crate) fn mark(kind: &str, args: std::fmt::Arguments<'_>) {
        if !active() {
            return;
        }
        line(format_args!("{kind} {:.1} {args}", ms()));
    }

    /// The marks of one present: `P` on begin, one line per stage, and
    /// `E` with the total on drop — so every exit answers with its
    /// duration.
    pub(crate) struct Traced(Option<Stages>);

    struct Stages {
        start: std::time::Instant,
        last: std::time::Instant,
    }

    impl Traced {
        /// Closes one stage: the line carries the time since the
        /// previous mark of this present.
        pub(crate) fn stage(&mut self, kind: &str, args: std::fmt::Arguments<'_>) {
            if let Some(stages) = &mut self.0 {
                let now = std::time::Instant::now();
                let dur = now.duration_since(stages.last).as_secs_f64() * 1000.0;
                line(format_args!("{kind} {:.1} dur={dur:.1} {args}", ms()));
                stages.last = now;
            }
        }
    }

    impl Drop for Traced {
        fn drop(&mut self) {
            if let Some(stages) = &self.0 {
                line(format_args!(
                    "E {:.1} dur={:.1}",
                    ms(),
                    stages.start.elapsed().as_secs_f64() * 1000.0
                ));
            }
        }
    }

    pub(crate) fn begin(w: f64, h: f64, live: bool, cmds: usize, via: Origin) -> Traced {
        if !active() {
            return Traced(None);
        }
        line(format_args!(
            "P {:.1} {w:.0}x{h:.0} live={} cmds={cmds} via={}",
            ms(),
            u8::from(live),
            via.name()
        ));
        let now = std::time::Instant::now();
        Traced(Some(Stages { start: now, last: now }))
    }
}
