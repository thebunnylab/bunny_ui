//! The bunny-ui Linux shell: a Wayland window, pointer events and the
//! live cycle — hover/press → repaint per event; action on up-inside →
//! state → incremental render → blit. Not a single dependency.
//!
//! The project's `unsafe` lives ONLY in the shell crates (here, the
//! [`ffi`] FFI), wrapped in this safe API. The core and the facade
//! keep `#![forbid(unsafe_code)]`.

#![cfg(target_os = "linux")]

pub mod credentials;
pub mod dialog;
#[doc(hidden)]
pub mod drive;
mod ffi;
mod gl;
mod image;
mod life;
mod text;
mod trace;
mod vk;
mod x11;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::action::{Key, KeyMatch, KeyPattern, Stroke};
use bunny_ui::layout::{Axis, Size};
use bunny_ui::prelude::{EditCommand, Runtime};
use bunny_ui::view::{Either, Single, View};

use ffi::AppEvent;
pub use gl::OffscreenGl;
pub use image::LinuxImageEngine;
pub use text::FreeTypeEngine;
pub use vk::OffscreenVk;

/// XKB keysym → the keymap vocabulary. Named keys come from the sym
/// table; the rest becomes `Char` through the base char (a clean
/// keyboard state), lowercased. `None` = lone modifier/function key.
///
/// The modifier mapping is the platform's: Ctrl is the accelerator,
/// so Ctrl carries `command`; Alt (Mod1) carries `option`; the
/// `control` flag stays false (Super belongs to the system). An AltGr
/// chord that types (`types_text`) never reaches this table — the
/// gate lets it through to the character road first.
fn key_pattern(stroke: &ffi::KeyStroke) -> Option<KeyPattern> {
    let named = match stroke.sym {
        0xff54 => Some(Key::Down),
        0xff52 => Some(Key::Up),
        0xff51 => Some(Key::Left),
        0xff53 => Some(Key::Right),
        0xff0d | 0xff8d => Some(Key::Enter), // Return and the keypad Enter share it
        0xff1b => Some(Key::Escape),
        0xff09 | 0xfe20 => Some(Key::Tab), // shift turns Tab into ISO_Left_Tab
        0xff55 => Some(Key::PageUp),
        0xff56 => Some(Key::PageDown),
        0xff08 => Some(Key::Backspace),
        0xffff => Some(Key::Delete),
        0xff50 => Some(Key::Home),
        0xff57 => Some(Key::End),
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
        .text_engine(Rc::new(FreeTypeEngine::new()))
        .image_engine(Rc::new(LinuxImageEngine::new()));
    run_window_with(title, size, runtime, root)
}

/// Who draws the window's top edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chrome {
    /// The compositor's decoration, where the compositor offers one —
    /// and the house's own bar where it does not: GNOME never draws a
    /// frame for a Wayland window, so the shell asks through
    /// `xdg-decoration` and, refused or unanswered, stands a 32-point
    /// bar of its own on the scene, with the crown answering its verbs.
    Native,
    /// The SCENE draws the bar. The crown phase wires the drag and
    /// control regions to the compositor's move/resize/menu verbs.
    Scene,
}

/// How a window behaves under the reader's hand: whether it resizes,
/// whether it can be put away. The PLATFORM refuses the gesture — the
/// scene never catches it afterwards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Manners {
    pub resizable: bool,
    pub minimizable: bool,
}

impl Default for Manners {
    fn default() -> Manners {
        Manners { resizable: true, minimizable: true }
    }
}

/// Like [`run_window`], but with the `Runtime` assembled by the caller —
/// the path for apps with their own environment (the text engine is
/// still the assembler's responsibility).
pub fn run_window_with(title: &str, size: Size, runtime: Runtime, root: impl View) {
    run_window_chrome(title, size, Chrome::Native, runtime, root)
}

// =============================================================================
// The app, and the window it holds
// =============================================================================

/// What a window is before it exists: its bar, its size and who draws
/// its top edge — the twin of the other two shells' spec.
#[derive(Clone, Debug)]
pub struct WindowSpec {
    title: Rc<str>,
    size: Size,
    chrome: Chrome,
    manners: Manners,
}

impl WindowSpec {
    /// A window named for its bar, at the house size.
    pub fn titled(title: impl Into<Rc<str>>) -> WindowSpec {
        WindowSpec {
            title: title.into(),
            size: Size { width: 1024.0, height: 640.0 },
            chrome: Chrome::Native,
            manners: Manners::default(),
        }
    }

    /// One size, and no other: the reader cannot resize it. On Wayland
    /// the minimum and the maximum size are the one size, so the
    /// compositor refuses the grab; on X11 `WM_NORMAL_HINTS` says the
    /// same and the Motif hints drop the resize and maximize verbs.
    pub fn fixed(mut self) -> WindowSpec {
        self.manners.resizable = false;
        self
    }

    /// It cannot be put away: the minimize verb is refused by the
    /// crown, dropped from the Motif hints, and the house bar draws no
    /// button for it. A compositor's own frame keeps its button —
    /// no protocol takes a verb off a server-side frame.
    pub fn no_minimize(mut self) -> WindowSpec {
        self.manners.minimizable = false;
        self
    }

    /// The content size the window opens at.
    pub fn size(mut self, width: f64, height: f64) -> WindowSpec {
        self.size = Size { width, height };
        self
    }

    /// Who draws the top edge — see [`Chrome`].
    pub fn chrome(mut self, chrome: Chrome) -> WindowSpec {
        self.chrome = chrome;
        self
    }
}

/// A window's handle in an [`App`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct WindowId(usize);

/// Whether this shell opens more than one window at a time — it does:
/// one poll loop, a `wl_surface` (or an xcb window) each, a presenter
/// each. An app that must run on every platform asks the constant
/// before it detaches a second window; the phones answer false.
pub const MANY_WINDOWS: bool = true;

/// The application: the event road, and the window on it.
///
/// The same shape the other two shells carry, so an app writes one
/// boot for three platforms — but this one holds a SINGLE window
/// ([`MANY_WINDOWS`] is false). Opening a second one is refused by
/// name rather than half-served: ask the constant before you ask for
/// the window.
///
/// ```ignore
/// let app = App::new();
/// let runtime = app.runtime().text_engine(Rc::new(FreeTypeEngine::new()));
/// app.open(WindowSpec::titled("Trinity Mail").size(1080.0, 720.0), Rc::new(runtime), mail);
/// app.run();
/// ```
#[derive(Clone)]
pub struct App {
    inner: Rc<AppInner>,
}

/// Everything ONE window owns: its address, its handle, and the
/// closures the doors consult for it — the event handler, the key
/// gate, the crown's gates. The app installs ONE of each with the
/// door and routes by the window the event arrived at.
struct Slot {
    window: usize,
    handle: ffi::WindowHandle,
    handler: RefCell<Box<dyn FnMut(AppEvent)>>,
    key_gate: RefCell<Box<dyn FnMut(&ffi::KeyStroke) -> bool>>,
    drag_gate: Box<dyn Fn(f64, f64) -> bool>,
    control_gate: Box<dyn Fn(f64, f64) -> Option<ffi::ControlHit>>,
}

struct AppInner {
    slots: RefCell<Vec<Rc<Slot>>>,
    routed: std::cell::Cell<bool>,
    scenes: std::cell::Cell<usize>,
}

impl AppInner {
    /// Installs the door's one handler and one set of gates, once:
    /// each asks the door which window the event arrived at and
    /// answers for that slot — or for every slot, when the source is 0
    /// (a beat every window shares).
    fn route(self: &Rc<Self>) {
        if self.routed.replace(true) {
            return;
        }
        let app = Rc::clone(self);
        ffi::set_handler(Box::new(move |event| {
            let source = ffi::event_source();
            let closing = matches!(event, AppEvent::WindowClosed);
            for slot in app.live() {
                if source == 0 || slot.window == source {
                    (slot.handler.borrow_mut())(event.clone());
                }
            }
            if closing {
                app.buried(source);
            }
        }));
        let app = Rc::clone(self);
        ffi::set_key_gate(Box::new(move |stroke| {
            app.addressed().is_some_and(|slot| (slot.key_gate.borrow_mut())(stroke))
        }));
        let drag = Rc::clone(self);
        let control = Rc::clone(self);
        ffi::set_chrome_gates(
            Box::new(move |x, y| drag.addressed().is_some_and(|slot| (slot.drag_gate)(x, y))),
            Box::new(move |x, y| control.addressed().and_then(|slot| (slot.control_gate)(x, y))),
        );
    }

    /// The slots, snapshotted BEFORE any handler runs — a window opened
    /// or closed inside a handler does not disturb the walk.
    fn live(&self) -> Vec<Rc<Slot>> {
        self.slots.borrow().clone()
    }

    /// The slot the current event is addressed to (the first, for a
    /// shared beat).
    fn addressed(&self) -> Option<Rc<Slot>> {
        let source = ffi::event_source();
        self.slots.borrow().iter().find(|slot| source == 0 || slot.window == source).cloned()
    }

    fn buried(&self, window: usize) {
        self.slots.borrow_mut().retain(|slot| slot.window != window);
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An app with no window yet.
    pub fn new() -> App {
        // the app's life outside its window: the bus thread, the
        // notifier
        life::install();
        App {
            inner: Rc::new(AppInner {
                slots: RefCell::new(Vec::new()),
                routed: std::cell::Cell::new(false),
                scenes: std::cell::Cell::new(0),
            }),
        }
    }

    /// A runtime for the window — named for its own scene.
    pub fn runtime(&self) -> Runtime {
        let seq = self.inner.scenes.get();
        self.inner.scenes.set(seq + 1);
        Runtime::scene(format!("w{seq}"))
    }

    /// Raises a window on `runtime`, showing `root` — painted first,
    /// then shown, so it never appears blank. A window is usually
    /// opened from inside an event, so the first paint goes through
    /// the new slot's own handler, not the door's road.
    pub fn open(&self, spec: WindowSpec, runtime: Rc<Runtime>, root: impl View) -> WindowId {
        let slot = mount(&spec, runtime, root);
        self.inner.slots.borrow_mut().push(Rc::clone(&slot));
        self.inner.route();
        (slot.handler.borrow_mut())(AppEvent::Redraw);
        ffi::show_window(slot.handle);
        WindowId(slot.window)
    }

    /// Closes the window. The last one out quits the app.
    pub fn close(&self, id: WindowId) {
        ffi::close_top_level(id.0);
        self.inner.buried(id.0);
    }

    /// The windows the app has open, oldest first.
    pub fn windows(&self) -> Vec<WindowId> {
        self.inner.slots.borrow().iter().map(|slot| WindowId(slot.window)).collect()
    }

    /// Enters the event road. Returns when the window closes.
    pub fn run(&self) {
        ffi::run();
    }
}

/// Like [`run_window_with`], choosing who draws the window's top edge.
///
/// One window, and the road: the sugar over [`App`] every single-window
/// app is.
pub fn run_window_chrome(
    title: &str,
    size: Size,
    chrome: Chrome,
    runtime: Runtime,
    root: impl View,
) {
    let app = App::new();
    app.open(
        WindowSpec::titled(title).size(size.width, size.height).chrome(chrome),
        Rc::new(runtime),
        root,
    );
    app.run();
}

/// The app's root as ONE node, whatever its arity: a component is the
/// boundary the core provides for that — its body may be several
/// nodes, the component is one. The window's own root, so the house
/// bar can stack above it.
#[derive(Clone)]
struct WindowRoot<V: View> {
    root: V,
}

impl<V: View> bunny_ui::view::Component for WindowRoot<V> {
    fn body(self, _ctx: &bunny_ui::prelude::Context) -> impl View {
        self.root
    }
}

/// The app's root under the house bar, or the root alone — one type
/// either way, so `mount` stays one function.
fn framed(bar: Option<(Rc<str>, bool)>, root: impl View) -> impl View<Arity = Single> {
    let root = WindowRoot { root };
    match bar {
        Some((title, minimizable)) => {
            Either::First(bunny_ui::vstack!(house_bar(title, minimizable), root).spacing(0.0))
        }
        None => Either::Second(root),
    }
}

/// The house's own bar — 32 points, the title, and the controls the
/// crown answers — for a compositor that draws no frame of its own.
/// The whole bar drags the window; the buttons are the window's own
/// (`.window_control`), so the platform closes, minimizes, maximizes.
fn house_bar(title: Rc<str>, minimizable: bool) -> impl View<Arity = Single> {
    use bunny_ui::layout::{Color, WindowControl};
    use bunny_ui::prelude::*;
    const BAR_H: f64 = 32.0;
    const CAPTION_W: f64 = 40.0;
    const CLEAR: Color = Color { r: 0, g: 0, b: 0, a: 0 };
    fn caption(
        glyph: impl UnaryView + 'static,
        control: WindowControl,
        wash: Color,
    ) -> impl View<Arity = Single> {
        glyph
            .frame(CAPTION_W, BAR_H)
            .background_color(CLEAR)
            .background_hovered(wash)
            .on_click(|| {})
            .window_control(control)
    }
    let minimize = if minimizable {
        Either::First(caption(
            icon(symbol::MINUS).font_size(10.0).foreground_color(theme::fg_secondary()),
            WindowControl::Minimize,
            theme::row_hover(),
        ))
    } else {
        Either::Second(empty())
    };
    let maximize_glyph = canvas(|ctx, painter| {
        let bounds = ctx.bounds();
        painter.stroke(bounds, theme::fg_secondary(), 1.0, 1.0);
    })
    .frame(9.0, 9.0);
    hstack!(
        text(title.to_string())
            .foreground_color(theme::fg_secondary())
            .padding_edge(Edge::Leading, 12.0),
        spacer(),
        minimize,
        caption(maximize_glyph, WindowControl::Maximize, theme::row_hover()),
        caption(
            icon(symbol::CLOSE)
                .font_size(10.0)
                .foreground_color(theme::fg_secondary())
                .foreground_hovered(Color::WHITE),
            WindowControl::Close,
            Color::rgb(196, 43, 28),
        ),
    )
    .spacing(0.0)
    .alignment(VerticalAlignment::Center)
    .frame_max(f64::INFINITY, BAR_H, Alignment::Leading)
    .background_color(theme::panel())
    .window_drag_region()
}

/// Raises the window `spec` asks for and wires everything that lives
/// as long as it does — the frame path, the pools, the gates and the
/// event handler — into a slot the app routes to.
fn mount(spec: &WindowSpec, runtime: Rc<Runtime>, root: impl View) -> Rc<Slot> {
    // a shell presents the list and never reads it: what no pixel can show
    // is not drawn
    runtime.drop_unseen();
    let window = ffi::create_window(
        &spec.title,
        spec.size.width,
        spec.size.height,
        ffi::WindowOptions {
            scene: spec.chrome == Chrome::Scene,
            resizable: spec.manners.resizable,
            minimizable: spec.manners.minimizable,
        },
    );
    // the bar: the compositor's where it offers one, the house's own
    // where it does not — decided BEFORE the GPU installs, because the
    // crown's corners want an alpha ground
    let house_bar = spec.chrome == Chrome::Native && ffi::wants_house_bar(window.raw_window());
    if house_bar {
        ffi::adopt_crown(window.raw_window());
        eprintln!("bunny_ui_linux: no server decoration — the house bar stands in");
    }
    // the present backend, chosen ONCE: the GPU by default, the CPU
    // raster on refusal — and the window is still unmapped, so the
    // first frame (whichever road) IS the reveal
    ffi::install_gpu(&window);
    // the season's mirrors: reduce-motion always follows the system
    // (accessibility is never the app's to refuse); the theme follows
    // ONLY while the app has not chosen one — an installed theme means
    // the scene owns its colors and the shell stays out. Where no
    // portal runs (this compositor), both keep their defaults; the
    // live change signal joins at the real-desktop pass.
    let mirror_theme = bunny_ui::theme::version() == 0;
    if mirror_theme && ffi::os_prefers_dark() == Some(true) {
        bunny_ui::theme::install(bunny_ui::theme::Theme::dark());
    }
    runtime.set_reduce_motion(!ffi::animations_enabled());
    // a task that lands on a worker thread asks the pump for one more
    // turn; the frame it takes drains the queue on its way
    runtime.set_wake_hook(std::sync::Arc::new(ffi::wake_from_any_thread));
    // the engine's stage timers ride the tape's clock: an `F` line for
    // each frame says where the time went BEFORE the present opened
    let frame_stats = trace::active();
    if frame_stats {
        bunny_ui::stats::set_clock(Some(trace::clock_ms));
    }
    // two owners: the keyboard gate and the event handler
    let root = framed(house_bar.then(|| (Rc::clone(&spec.title), spec.manners.minimizable)), root);
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
                // degenerate: a zero surface is an abort, not a frame
                return;
            }
            // the window presents everything BEFORE the first overlay;
            // each overlay re-presents its own slice on an owned panel
            // — a popup that may hang past the window's edge
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
                    let chip = overlay.path == bunny_ui::layout::DRAG_LABEL_PATH;
                    let panel = store
                        .entry(overlay.path.clone())
                        .or_insert_with(|| ffi::create_panel(&window, chip));
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
                    // premultiply for the alpha surface fuses into the
                    // copy at the boundary
                    panel.present_layered(
                        window.layout_rect_to_screen(x, y, w, h),
                        panel_physical.0,
                        panel_physical.1,
                        &bitmap.to_rgba_bytes(),
                    );
                }
            }
            if vk::active(window.raw_window()) {
                // the front of the ladder: the same display list, no
                // Surface in the path — the queue present is the frame
                vk::present_window(
                    window.raw_window(),
                    &display,
                    Size { width, height },
                    scale,
                    canvas,
                    &*runtime.text(),
                    &*runtime.images(),
                );
                if frame_stats {
                    trace::mark("P", format_args!("road=vk presents={}", trace::presents()));
                }
                return;
            }
            if gl::active(window.raw_window()) {
                // GPU present: the same display list, no Surface in
                // the path — the swap is the frame
                gl::present_window(
                    window.raw_window(),
                    &display,
                    Size { width, height },
                    scale,
                    canvas,
                    &*runtime.text(),
                    &*runtime.images(),
                );
                if frame_stats {
                    trace::mark("P", format_args!("road=gl presents={}", trace::presents()));
                }
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
                // present only the wounds: damage-only backing copy +
                // damage-only surface marks in the same pass
                let (width, height) = (retained.bitmap().width(), retained.bitmap().height());
                window.blit_partial(width, height, retained.rgba(), &damage);
                if frame_stats {
                    trace::mark(
                        "P",
                        format_args!("road=cpu presents={} wounds={}", trace::presents(), damage.len()),
                    );
                }
            }
        }
    });
    let blit = {
        let present = Rc::clone(&present);
        move |runtime: &Runtime, root: &_| {
            let (width, height) = window.content_size();
            // a box that draws parts which TOUCH puts the shared edge
            // on a whole PIXEL — it needs the screen's scale
            runtime.set_device_scale(window.scale() as f64);
            // popovers position against an inflated work area, in
            // layout coordinates — overflow past the window's edge is
            // plain geometry here, and welcome
            runtime.set_overlay_bounds(window.screen_bounds_in_layout().map(
                |(x, y, w, h)| bunny_ui::layout::Rect {
                    origin: bunny_ui::layout::Point { x, y },
                    size: Size { width: w, height: h },
                },
            ));
            let display = runtime.display_frame(root, Size { width, height });
            if frame_stats {
                let stats = bunny_ui::stats::take();
                let ms = |stage| stats.ms(stage);
                use bunny_ui::stats::Stage;
                trace::mark(
                    "F",
                    format_args!(
                        "settle={:.2} layout={:.2} pass={:.2} asm={:.2} measure={:.2} place={:.2} hover={:.2} passes={} layouts={} asm#={} hover#={} paints={} cmds={} scale={} factor={:.2}",
                        ms(Stage::Settle),
                        ms(Stage::Layout),
                        ms(Stage::Pass),
                        ms(Stage::Assemble),
                        ms(Stage::Measure),
                        ms(Stage::Place),
                        ms(Stage::Hover),
                        stats.body_passes,
                        stats.layout_passes,
                        stats.assemblies,
                        stats.hover_relayouts,
                        stats.paints,
                        display.len(),
                        window.scale(),
                        window.scale_factor(),
                    ),
                );
            }
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
                // the BOX under the pointer answers first — text wants
                // an I-beam, and the rule below cannot know that. Only
                // where nobody answers does the old rule stand: the
                // hand over anything hoverable
                None => match runtime.hovered_cursor() {
                    Some(bunny_ui::layout::Cursor::Text) => ffi::Cursor::Text,
                    Some(bunny_ui::layout::Cursor::Pointing) => ffi::Cursor::Pointing,
                    Some(bunny_ui::layout::Cursor::Cell) => ffi::Cursor::Cell,
                    Some(bunny_ui::layout::Cursor::Arrow) => ffi::Cursor::Arrow,
                    None if interaction.hovered.is_some() => ffi::Cursor::Pointing,
                    None => ffi::Cursor::Arrow,
                },
            });
            // the input system's mirror: the door opens at the IME
            // phase; the slot keeps the twins' step order today
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
            ffi::want_frames(window.raw_window(), runtime.wants_frame());
        }
    };

    // the frame conversation: a press on a `.window_drag_region()`
    // (with no interactive target above) moves the window by the
    // compositor's own grab; a `.window_control(…)` answers as the
    // window's own button
    let drag_gate: Box<dyn Fn(f64, f64) -> bool> = Box::new({
        let runtime = Rc::clone(&runtime);
        move |x, y| runtime.window_drag_at(x, y)
    });
    let control_gate: Box<dyn Fn(f64, f64) -> Option<ffi::ControlHit>> = Box::new({
        let runtime = Rc::clone(&runtime);
        move |x, y| {
            runtime.window_control_at(x, y).map(|control| match control {
                bunny_ui::layout::WindowControl::Close => ffi::ControlHit::Close,
                bunny_ui::layout::WindowControl::Minimize => ffi::ControlHit::Minimize,
                bunny_ui::layout::WindowControl::Maximize => ffi::ControlHit::Maximize,
            })
        }
    });

    // the gate: keymap BEFORE the input system — bare chars pass
    // straight through to whoever holds the keyboard AND is taking
    // text (typing is never stolen; a modal box in command mode
    // declines and the stroke walks on); a binding with no handler
    // mounted does not consume; an AltGr chord that types IS text and
    // never enters. The composition-first step arrives with the IME
    // phase.
    let key_gate: Box<dyn FnMut(&ffi::KeyStroke) -> bool> = Box::new({
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
    });

    let handler_runtime = Rc::clone(&runtime);
    let handler_root = Rc::clone(&root);
    let handler_present = Rc::clone(&present);
    let handler: Box<dyn FnMut(AppEvent)> = Box::new(move |event| {
        let runtime = &handler_runtime;
        let root = &*handler_root;
        match event {
            AppEvent::Redraw => blit(runtime, root),
            AppEvent::WindowClosed => {}
            // The work always lands: the tasks are polled. The FRAME is for a
            // turn that changed something. Most wakes change nothing — a poll
            // that found no news, a sleeper that went back to sleep — and a
            // window with a few of those mounted drew whole frames of what
            // was already on screen, dozens of times a second, at rest. A
            // change the engine cannot see asks by hand
            // (`bunny_ui::request_frame`).
            AppEvent::Wake => {
                runtime.poll_tasks();
                if runtime.needs_frame() {
                    blit(runtime, root);
                } else {
                    // no frame — but a task may have gone to sleep with a
                    // new deadline, and the driver follows it
                    ffi::want_frames(window.raw_window(), runtime.wants_frame());
                }
            }
            AppEvent::ResignKey => {
                // the user switched away: popovers close like the
                // platform's own
                if runtime.dismiss_all_overlays() {
                    blit(runtime, root);
                }
            }
            AppEvent::DismissOverlays => {
                // the x11 outside-press: no compositor grab exists to
                // dismiss for us — the shell watched the geometry and
                // says so; the press itself follows as its own event
                if runtime.dismiss_all_overlays() {
                    blit(runtime, root);
                }
            }
            AppEvent::Text(text) => {
                // typing, paste of characters, and the composed
                // dead-key result — the same road for all of them
                if !text.is_empty() && runtime.key(EditCommand::Insert(text)).applied {
                    blit(runtime, root);
                }
            }
            AppEvent::Key { sym, shift, command } => {
                let edit = match sym {
                    0xff08 => Some(EditCommand::Backspace),
                    0xffff => Some(EditCommand::Delete),
                    0xff51 => Some(EditCommand::Left(shift)),
                    0xff53 => Some(EditCommand::Right(shift)),
                    0xff50 => Some(EditCommand::Home(shift)),
                    0xff57 => Some(EditCommand::End(shift)),
                    0xff1b => {
                        // esc releases focus
                        if runtime.blur() {
                            blit(runtime, root);
                        }
                        None
                    }
                    0x61 if command => Some(EditCommand::SelectAll), // Ctrl+A
                    0x63 if command => {
                        // Ctrl+C — the field's output goes to the system
                        if let Some(text) = runtime.key(EditCommand::Copy).output {
                            ffi::clipboard_write(&text);
                        }
                        None
                    }
                    0x78 if command => {
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
                    0x76 if command => ffi::clipboard_read().map(EditCommand::Insert), // Ctrl+V
                    _ => None,
                };
                if let Some(edit) = edit
                    && runtime.key(edit).applied
                {
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
            AppEvent::Touch { phase, id, x, y } => {
                // a touch that changed nothing visible may still have
                // put the finger on the clock (a fling in the air)
                let changed = match phase {
                    ffi::TouchPhase::Began => runtime.touch_began(id, x, y, 1),
                    ffi::TouchPhase::Moved => runtime.touch_moved(id, x, y),
                    ffi::TouchPhase::Ended => runtime.touch_ended(id, x, y),
                    ffi::TouchPhase::Cancelled => runtime.touch_cancelled(id),
                };
                if changed || phase == ffi::TouchPhase::Ended {
                    blit(runtime, root);
                } else {
                    ffi::want_frames(window.raw_window(), runtime.wants_frame());
                }
            }
            AppEvent::Magnify { x, y, scale } => {
                if runtime.magnify(x, y, scale) {
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
            AppEvent::Blink => {
                // an idle caret blinks; without focus the tick is
                // silence — and the same slow clock ages the tooltip's
                // wait and then shows it: the delay is this tick twice
                let blinked = runtime.blink();
                let explained = runtime.tooltip_tick();
                // the same slow beat ages a sequence in the air: two
                // ticks and `cmd-k` lets the keyboard go
                let chorded = runtime.chord_tick();
                // and the wheel's latch: two ticks with no wheel end
                // the scroll gesture
                runtime.wheel_tick();
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
                ffi::want_frames(window.raw_window(), runtime.wants_frame());
            }
        }
    });

    // the first frame and the reveal are the app's: painted through
    // this slot's own handler, then shown — on wayland the first
    // presenting commit IS the reveal, so the window never flashes
    Rc::new(Slot {
        window: window.raw_window(),
        handle: window,
        handler: RefCell::new(handler),
        key_gate: RefCell::new(key_gate),
        drag_gate,
        control_gate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(sym: u32, base: &str, shift: bool, control: bool, alt: bool) -> ffi::KeyStroke {
        ffi::KeyStroke {
            sym,
            shift,
            control,
            alt,
            chars_ignoring: base.to_string(),
            types_text: false,
            // the fixture names the KEY; what it typed is the live
            // layout's answer and no test here asks for one
            typed: None,
        }
    }

    #[test]
    fn named_keys_map_to_the_vocabulary() {
        let pattern = key_pattern(&stroke(0xff54, "", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Down);
        let pattern = key_pattern(&stroke(0xff0d, "\r", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Enter);
        let pattern = key_pattern(&stroke(0xff1b, "\u{1b}", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Escape);
    }

    #[test]
    fn ctrl_carries_command_the_accelerator() {
        let pattern = key_pattern(&stroke(0x66, "f", false, true, false)).unwrap();
        assert_eq!(pattern.key, Key::Char('f'));
        assert!(pattern.command, "Ctrl is the accelerator");
        assert!(!pattern.control, "the control flag stays with the system");
        assert!(!pattern.is_text_input(), "a chord is never typing");
    }

    #[test]
    fn a_bare_letter_is_text_input() {
        let pattern = key_pattern(&stroke(0x61, "a", false, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Char('a'));
        assert!(pattern.is_text_input());
    }

    #[test]
    fn shift_tab_matches_exactly() {
        // shift turns Tab into ISO_Left_Tab — the table folds it back
        let pattern = key_pattern(&stroke(0xfe20, "\t", true, false, false)).unwrap();
        assert_eq!(pattern.key, Key::Tab);
        assert!(pattern.shift);
        assert!(!pattern.command && !pattern.option);
    }

    #[test]
    fn a_lone_modifier_is_no_pattern() {
        // Shift_L alone: no named key, no base char
        assert!(key_pattern(&stroke(0xffe1, "", true, false, false)).is_none());
        // a function key has no base char either
        assert!(key_pattern(&stroke(0xffc1, "", false, false, false)).is_none(), "F4 is silent");
    }

    #[test]
    fn the_base_char_lowers_and_alt_rides_as_option() {
        let pattern = key_pattern(&stroke(0x41, "A", true, false, true)).unwrap();
        assert_eq!(pattern.key, Key::Char('a'), "shift does not change the key's identity");
        assert!(pattern.option, "Alt rides as option");
    }
}
