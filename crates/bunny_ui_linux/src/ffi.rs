//! Hand-written Wayland FFI: the connection, the protocol tables, the
//! event loop, the shm presentation backing and the frame pacing.
//! Every declaration is written here against the system libraries —
//! no bindings crate, no protocol generator.
//!
//! The border binds `libwayland-client` (which owns the wire format
//! and all fd passing), `libwayland-cursor` (theme files as ready
//! buffers) and a small libc floor. The core protocol's interface
//! descriptors are data symbols EXPORTED by libwayland-client itself;
//! only the extension protocols (xdg-shell here) carry hand-written
//! tables, built once at connect and checked against the installed
//! protocol XML by a test.
//!
//! Positions handed to the shell are LAYOUT coordinates: top-left
//! origin, logical points. Wayland surface-local coordinates already
//! ARE that — no flip, no division at the border.
//!
//! One thread owns everything: events are decoded by a single
//! dispatcher trampoline into a queue, and the loop interprets them
//! between polls. The read side uses libwayland's prepare-read
//! protocol because Mesa's EGL joins the same connection as a second
//! reader in the GPU era.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::time::Instant;

// MARK: - libc floor (the only raw syscalls the shell needs)

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 0x1;
const MFD_CLOEXEC: c_uint = 0x1;
const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;
const MAP_SHARED: c_int = 0x1;

unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
    fn ftruncate(fd: c_int, length: i64) -> c_int;
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> c_int;
    fn poll(fds: *mut PollFd, count: u64, timeout_ms: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
}

// MARK: - libwayland-client ABI (the library owns the wire; we own the tables)

#[repr(C)]
pub(crate) struct WlMessage {
    name: *const c_char,
    signature: *const c_char,
    types: *const *const WlInterface,
}

#[repr(C)]
pub(crate) struct WlInterface {
    name: *const c_char,
    version: c_int,
    method_count: c_int,
    methods: *const WlMessage,
    event_count: c_int,
    events: *const WlMessage,
}

/// One slot of a decoded event or an encoded request. The union is the
/// C ABI's `wl_argument`; only the member named by the signature letter
/// is ever alive.
#[repr(C)]
union WlArgument {
    i: i32,
    u: u32,
    /// wl_fixed: signed 24.8
    f: i32,
    s: *const c_char,
    o: *mut c_void,
    n: u32,
    a: *mut c_void,
    h: i32,
}

type Proxy = c_void;
type Display = c_void;

type DispatcherFn = unsafe extern "C" fn(
    *const c_void,
    *mut c_void,
    u32,
    *const WlMessage,
    *mut WlArgument,
) -> c_int;

/// Destroys the proxy atomically with the request it rides — no event
/// can race the teardown.
const MARSHAL_FLAG_DESTROY: u32 = 1;

#[link(name = "wayland-client")]
unsafe extern "C" {
    fn wl_display_connect(name: *const c_char) -> *mut Display;
    fn wl_display_disconnect(display: *mut Display);
    fn wl_display_get_fd(display: *mut Display) -> c_int;
    fn wl_display_flush(display: *mut Display) -> c_int;
    fn wl_display_roundtrip(display: *mut Display) -> c_int;
    fn wl_display_dispatch_pending(display: *mut Display) -> c_int;
    fn wl_display_prepare_read(display: *mut Display) -> c_int;
    fn wl_display_read_events(display: *mut Display) -> c_int;
    fn wl_display_cancel_read(display: *mut Display);
    fn wl_display_get_error(display: *mut Display) -> c_int;

    fn wl_proxy_marshal_array_flags(
        proxy: *mut Proxy,
        opcode: u32,
        interface: *const WlInterface,
        version: u32,
        flags: u32,
        args: *mut WlArgument,
    ) -> *mut Proxy;
    fn wl_proxy_add_dispatcher(
        proxy: *mut Proxy,
        dispatcher: DispatcherFn,
        implementation: *const c_void,
        user_data: *mut c_void,
    ) -> c_int;
    fn wl_proxy_destroy(proxy: *mut Proxy);
    fn wl_proxy_get_version(proxy: *mut Proxy) -> u32;

    // the CORE protocol's descriptors live inside libwayland-client —
    // hand-written tables are only needed for extensions
    static wl_registry_interface: WlInterface;
    static wl_callback_interface: WlInterface;
    static wl_compositor_interface: WlInterface;
    static wl_surface_interface: WlInterface;
    static wl_shm_interface: WlInterface;
    static wl_shm_pool_interface: WlInterface;
    static wl_buffer_interface: WlInterface;
    static wl_output_interface: WlInterface;
    static wl_seat_interface: WlInterface;
    static wl_pointer_interface: WlInterface;
}

// MARK: - libwayland-cursor ABI (theme files arrive as ready wl_buffers)

#[repr(C)]
struct WlCursorImage {
    width: u32,
    height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    delay: u32,
}

#[repr(C)]
struct WlCursor {
    image_count: c_uint,
    images: *mut *mut WlCursorImage,
    name: *mut c_char,
}

#[link(name = "wayland-cursor")]
unsafe extern "C" {
    fn wl_cursor_theme_load(name: *const c_char, size: c_int, shm: *mut Proxy) -> *mut c_void;
    fn wl_cursor_theme_destroy(theme: *mut c_void);
    fn wl_cursor_theme_get_cursor(theme: *mut c_void, name: *const c_char) -> *mut WlCursor;
    fn wl_cursor_image_get_buffer(image: *mut WlCursorImage) -> *mut Proxy;
}

// MARK: - the xdg-shell tables (hand-written, opcode order is law)

/// A message spec before it becomes ABI: name, signature, and which
/// table slot each object/new_id argument points at (`None` for the
/// non-object slots — the ABI wants one entry per argument).
struct Msg(&'static CStr, &'static CStr, &'static [Option<Iface>]);

/// The five extension interfaces this shell speaks, in one enum so the
/// spec rows can reference each other before the tables exist.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Iface {
    Positioner,
    XdgSurface,
    Toplevel,
    Popup,
    CoreSurface,
    CoreSeat,
    CoreOutput,
}

/// The built tables, leaked to `'static` — libwayland keeps pointers
/// into them for every live proxy, so they live as long as the process.
pub(crate) struct Protocols {
    wm_base: &'static WlInterface,
    xdg_surface: &'static WlInterface,
    toplevel: &'static WlInterface,
    // the tables cross-reference these through raw pointers the
    // compiler cannot see; the overlays phase binds them by name
    #[allow(dead_code)]
    popup: &'static WlInterface,
    #[allow(dead_code)]
    positioner: &'static WlInterface,
}

/// The xdg-shell message rows, transcribed from the installed
/// `xdg-shell.xml` in exact opcode order. A test diffs the names and
/// the order against that file so a slipped row can never ship.
fn xdg_spec() -> [(&'static CStr, u32, Vec<Msg>, Vec<Msg>); 5] {
    use Iface::*;
    [
        (
            c"xdg_wm_base",
            5,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"create_positioner", c"n", &[Some(Positioner)]),
                Msg(c"get_xdg_surface", c"no", &[Some(XdgSurface), Some(CoreSurface)]),
                Msg(c"pong", c"u", &[None]),
            ],
            vec![Msg(c"ping", c"u", &[None])],
        ),
        (
            c"xdg_positioner",
            5,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"set_size", c"ii", &[None, None]),
                Msg(c"set_anchor_rect", c"iiii", &[None, None, None, None]),
                Msg(c"set_anchor", c"u", &[None]),
                Msg(c"set_gravity", c"u", &[None]),
                Msg(c"set_constraint_adjustment", c"u", &[None]),
                Msg(c"set_offset", c"ii", &[None, None]),
                Msg(c"set_reactive", c"3", &[]),
                Msg(c"set_parent_size", c"3ii", &[None, None]),
                Msg(c"set_parent_configure", c"3u", &[None]),
            ],
            vec![],
        ),
        (
            c"xdg_surface",
            5,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"get_toplevel", c"n", &[Some(Toplevel)]),
                Msg(c"get_popup", c"n?oo", &[Some(Popup), Some(XdgSurface), Some(Positioner)]),
                Msg(c"set_window_geometry", c"iiii", &[None, None, None, None]),
                Msg(c"ack_configure", c"u", &[None]),
            ],
            vec![Msg(c"configure", c"u", &[None])],
        ),
        (
            c"xdg_toplevel",
            5,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"set_parent", c"?o", &[Some(Toplevel)]),
                Msg(c"set_title", c"s", &[None]),
                Msg(c"set_app_id", c"s", &[None]),
                Msg(c"show_window_menu", c"ouii", &[Some(CoreSeat), None, None, None]),
                Msg(c"move", c"ou", &[Some(CoreSeat), None]),
                Msg(c"resize", c"ouu", &[Some(CoreSeat), None, None]),
                Msg(c"set_max_size", c"ii", &[None, None]),
                Msg(c"set_min_size", c"ii", &[None, None]),
                Msg(c"set_maximized", c"", &[]),
                Msg(c"unset_maximized", c"", &[]),
                Msg(c"set_fullscreen", c"?o", &[Some(CoreOutput)]),
                Msg(c"unset_fullscreen", c"", &[]),
                Msg(c"set_minimized", c"", &[]),
            ],
            vec![
                Msg(c"configure", c"iia", &[None, None, None]),
                Msg(c"close", c"", &[]),
                Msg(c"configure_bounds", c"4ii", &[None, None]),
                Msg(c"wm_capabilities", c"5a", &[None]),
            ],
        ),
        (
            c"xdg_popup",
            5,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"grab", c"ou", &[Some(CoreSeat), None]),
                Msg(c"reposition", c"3ou", &[Some(Positioner), None]),
            ],
            vec![
                Msg(c"configure", c"iiii", &[None, None, None, None]),
                Msg(c"popup_done", c"", &[]),
                Msg(c"repositioned", c"3u", &[None]),
            ],
        ),
    ]
}

/// Builds the five `WlInterface` tables and leaks them. Two passes:
/// the interfaces are allocated first so the message rows can point at
/// each other (popup → positioner, xdg_surface → toplevel, …).
fn build_protocols() -> Protocols {
    let spec = xdg_spec();
    // pass 1: stable homes, filled with placeholders
    let slots: &'static mut [WlInterface; 5] = Box::leak(Box::new(std::array::from_fn(|_| {
        WlInterface {
            name: c"".as_ptr(),
            version: 0,
            method_count: 0,
            methods: std::ptr::null(),
            event_count: 0,
            events: std::ptr::null(),
        }
    })));
    let base = slots.as_ptr();
    let resolve = |iface: Iface| -> *const WlInterface {
        match iface {
            Iface::Positioner => unsafe { base.add(1) },
            Iface::XdgSurface => unsafe { base.add(2) },
            Iface::Toplevel => unsafe { base.add(3) },
            Iface::Popup => unsafe { base.add(4) },
            Iface::CoreSurface => &raw const wl_surface_interface,
            Iface::CoreSeat => &raw const wl_seat_interface,
            Iface::CoreOutput => &raw const wl_output_interface,
        }
    };
    let build_rows = |rows: &[Msg]| -> (*const WlMessage, c_int) {
        let built: Vec<WlMessage> = rows
            .iter()
            .map(|Msg(name, signature, types)| {
                let entries: Vec<*const WlInterface> = types
                    .iter()
                    .map(|slot| slot.map_or(std::ptr::null(), resolve))
                    .collect();
                WlMessage {
                    name: name.as_ptr(),
                    signature: signature.as_ptr(),
                    types: Box::leak(entries.into_boxed_slice()).as_ptr(),
                }
            })
            .collect();
        let count = built.len() as c_int;
        (Box::leak(built.into_boxed_slice()).as_ptr(), count)
    };
    // pass 2: fill the homes
    for (slot, (name, version, methods, events)) in slots.iter_mut().zip(spec) {
        let (method_rows, method_count) = build_rows(&methods);
        let (event_rows, event_count) = build_rows(&events);
        *slot = WlInterface {
            name: name.as_ptr(),
            version: version as c_int,
            method_count,
            methods: method_rows,
            event_count,
            events: event_rows,
        };
    }
    Protocols {
        wm_base: &slots[0],
        positioner: &slots[1],
        xdg_surface: &slots[2],
        toplevel: &slots[3],
        popup: &slots[4],
    }
}

// MARK: - marshalling helpers

/// A plain request (no new proxy, no destroy).
unsafe fn request(proxy: *mut Proxy, opcode: u32, args: &mut [WlArgument]) {
    unsafe {
        let version = wl_proxy_get_version(proxy);
        wl_proxy_marshal_array_flags(proxy, opcode, std::ptr::null(), version, 0, args.as_mut_ptr());
    }
}

/// A constructor request: returns the new proxy (inheriting the
/// parent's version, the generated-code convention), wired to the
/// dispatcher under `tag` before any of its events can arrive.
unsafe fn construct(
    proxy: *mut Proxy,
    opcode: u32,
    interface: *const WlInterface,
    args: &mut [WlArgument],
    tag: usize,
) -> *mut Proxy {
    unsafe {
        let version = wl_proxy_get_version(proxy);
        construct_versioned(proxy, opcode, interface, version, args, tag)
    }
}

/// The registry's `bind` is the one constructor whose version is
/// CHOSEN (the global's), not inherited.
unsafe fn construct_versioned(
    proxy: *mut Proxy,
    opcode: u32,
    interface: *const WlInterface,
    version: u32,
    args: &mut [WlArgument],
    tag: usize,
) -> *mut Proxy {
    unsafe {
        let child =
            wl_proxy_marshal_array_flags(proxy, opcode, interface, version, 0, args.as_mut_ptr());
        if !child.is_null() {
            wl_proxy_add_dispatcher(child, dispatcher, tag as *const c_void, std::ptr::null_mut());
        }
        child
    }
}

/// A destructor request: the wire message and the proxy teardown are
/// one atomic step.
unsafe fn destroy(proxy: *mut Proxy, opcode: u32) {
    unsafe {
        let version = wl_proxy_get_version(proxy);
        let mut args = no_args();
        wl_proxy_marshal_array_flags(
            proxy,
            opcode,
            std::ptr::null(),
            version,
            MARSHAL_FLAG_DESTROY,
            args.as_mut_ptr(),
        );
    }
}

fn no_args() -> [WlArgument; 0] {
    []
}

fn arg_u(u: u32) -> WlArgument {
    WlArgument { u }
}

fn arg_i(i: i32) -> WlArgument {
    WlArgument { i }
}

fn arg_o(o: *mut Proxy) -> WlArgument {
    WlArgument { o }
}

fn arg_n() -> WlArgument {
    WlArgument { n: 0 }
}

/// The marshal COPIES string payloads before returning, so a borrow
/// that lives through the call is enough.
fn arg_s(s: &CStr) -> WlArgument {
    WlArgument { s: s.as_ptr() }
}

fn arg_h(h: i32) -> WlArgument {
    WlArgument { h }
}

/// wl_fixed is signed 24.8.
fn fixed_to_f64(fixed: i32) -> f64 {
    fixed as f64 / 256.0
}

// MARK: - the dispatcher (decode only; interpretation happens in the loop)

/// Proxy identities for the dispatcher. Outputs carry their registry
/// name so removal can find them: tag = OUTPUT_TAG_BASE + name.
const TAG_REGISTRY: usize = 1;
const TAG_SYNC: usize = 2;
const TAG_FRAME: usize = 3;
const TAG_WM_BASE: usize = 4;
const TAG_XDG_SURFACE: usize = 5;
const TAG_TOPLEVEL: usize = 6;
const TAG_MAIN_SURFACE: usize = 7;
const TAG_SEAT: usize = 8;
const TAG_POINTER: usize = 9;
const TAG_BUFFER: usize = 10;
const TAG_CURSOR_SURFACE: usize = 11;
const TAG_KEYBOARD: usize = 12;
const OUTPUT_TAG_BASE: usize = 0x1000;

/// A decoded protocol event, queued for the loop. The dispatcher owns
/// NOTHING but this queue — state and marshalling stay outside, so a
/// roundtrip can never re-enter a borrow.
enum Ev {
    Global { name: u32, interface: String, version: u32 },
    GlobalRemove { name: u32 },
    Ping { serial: u32 },
    SurfaceConfigure { serial: u32 },
    ToplevelConfigure { width: i32, height: i32 },
    ToplevelClose,
    FrameDone,
    SurfaceEnter { output_ptr: usize },
    SurfaceLeave { output_ptr: usize },
    OutputScale { output_name: u32, scale: i32 },
    OutputDone { output_name: u32 },
    PointerEnter { serial: u32, x: f64, y: f64 },
    PointerLeave,
    PointerMotion { x: f64, y: f64 },
    PointerButton { serial: u32, time_ms: u32, button: u32, pressed: bool },
    BufferRelease,
}

thread_local! {
    static EVQ: RefCell<VecDeque<Ev>> = const { RefCell::new(VecDeque::new()) };
}

fn push_ev(ev: Ev) {
    EVQ.with(|q| q.borrow_mut().push_back(ev));
}

unsafe extern "C" fn dispatcher(
    tag: *const c_void,
    _proxy: *mut c_void,
    opcode: u32,
    _msg: *const WlMessage,
    args: *mut WlArgument,
) -> c_int {
    let tag = tag as usize;
    let arg = |index: usize| unsafe { &*args.add(index) };
    match tag {
        TAG_REGISTRY => match opcode {
            0 => {
                let interface = unsafe { CStr::from_ptr(arg(1).s) }.to_string_lossy().into_owned();
                push_ev(Ev::Global {
                    name: unsafe { arg(0).u },
                    interface,
                    version: unsafe { arg(2).u },
                });
            }
            1 => push_ev(Ev::GlobalRemove { name: unsafe { arg(0).u } }),
            _ => {}
        },
        TAG_SYNC => {} // roundtrip consumes the done itself
        TAG_FRAME => push_ev(Ev::FrameDone),
        TAG_WM_BASE => {
            if opcode == 0 {
                push_ev(Ev::Ping { serial: unsafe { arg(0).u } });
            }
        }
        TAG_XDG_SURFACE => {
            if opcode == 0 {
                push_ev(Ev::SurfaceConfigure { serial: unsafe { arg(0).u } });
            }
        }
        TAG_TOPLEVEL => match opcode {
            0 => push_ev(Ev::ToplevelConfigure {
                width: unsafe { arg(0).i },
                height: unsafe { arg(1).i },
            }),
            1 => push_ev(Ev::ToplevelClose),
            _ => {} // configure_bounds v4 / wm_capabilities v5 never arrive at our bind
        },
        TAG_MAIN_SURFACE => match opcode {
            // the dispatcher may fire while the client is borrowed
            // (release waits dispatch too), so it carries the raw
            // output pointer and the drain resolves the name
            0 => push_ev(Ev::SurfaceEnter { output_ptr: unsafe { arg(0).o } as usize }),
            1 => push_ev(Ev::SurfaceLeave { output_ptr: unsafe { arg(0).o } as usize }),
            _ => {}
        },
        TAG_SEAT => {} // capabilities handled at bind time (pointer today, keyboard at its phase)
        TAG_POINTER => match opcode {
            0 => push_ev(Ev::PointerEnter {
                serial: unsafe { arg(0).u },
                x: fixed_to_f64(unsafe { arg(2).f }),
                y: fixed_to_f64(unsafe { arg(3).f }),
            }),
            1 => push_ev(Ev::PointerLeave),
            2 => push_ev(Ev::PointerMotion {
                x: fixed_to_f64(unsafe { arg(1).f }),
                y: fixed_to_f64(unsafe { arg(2).f }),
            }),
            3 => push_ev(Ev::PointerButton {
                serial: unsafe { arg(0).u },
                time_ms: unsafe { arg(1).u },
                button: unsafe { arg(2).u },
                pressed: unsafe { arg(3).u } == 1,
            }),
            _ => {} // axis family joins at the scroll phase; frame batches then
        },
        TAG_BUFFER => push_ev(Ev::BufferRelease),
        TAG_CURSOR_SURFACE | TAG_KEYBOARD => {}
        tag if tag >= OUTPUT_TAG_BASE => {
            let output_name = (tag - OUTPUT_TAG_BASE) as u32;
            match opcode {
                2 => push_ev(Ev::OutputDone { output_name }),
                3 => push_ev(Ev::OutputScale { output_name, scale: unsafe { arg(0).i } }),
                _ => {} // geometry/mode/name: nothing the shell reads yet
            }
        }
        _ => {}
    }
    0
}

// MARK: - pure protocol state (testable without a compositor)

/// The xdg map dance: a buffer may only attach after the first
/// configure was acked, and the window counts as mapped once a buffer
/// commits. Violations are FATAL protocol errors, so the machine is
/// the single authority.
#[derive(Default)]
struct MapState {
    configured: bool,
    mapped: bool,
}

impl MapState {
    /// A configure arrived; ack is always owed.
    fn on_configure(&mut self) {
        self.configured = true;
    }

    fn can_attach(&self) -> bool {
        self.configured
    }

    fn on_present(&mut self) {
        if self.configured {
            self.mapped = true;
        }
    }
}

/// The serial slots the protocol demands back. Buttons record on PRESS
/// only — compositors decline moves and grabs quoting release serials.
#[derive(Default)]
struct Serials {
    enter: u32,
    press: u32,
}

impl Serials {
    fn record_button(&mut self, serial: u32, pressed: bool) {
        if pressed {
            self.press = serial;
        }
    }
}

/// Client-side double click: the compositor sends plain buttons, the
/// shell counts. Same window the platforms use: 400 ms and a 4 px
/// wander budget.
#[derive(Default)]
struct ClickClock {
    last: Option<(u32, f64, f64)>,
    count: u8,
}

impl ClickClock {
    fn click(&mut self, time_ms: u32, x: f64, y: f64) -> u8 {
        let chained = self.last.is_some_and(|(t, lx, ly)| {
            time_ms.wrapping_sub(t) <= 400 && (x - lx).abs() <= 4.0 && (y - ly).abs() <= 4.0
        });
        self.count = if chained { self.count.saturating_add(1) } else { 1 };
        self.last = Some((time_ms, x, y));
        self.count
    }
}

/// Integer scale from the outputs the surface touches — tier 3 of the
/// ladder, the one every compositor has. Tiers 1–2 (fractional scale,
/// preferred_buffer_scale) slot in above when a compositor offers them.
fn resolve_scale(entered: &[u32], outputs: &[(u32, i32)]) -> usize {
    entered
        .iter()
        .filter_map(|name| outputs.iter().find(|(id, _)| id == name))
        .map(|&(_, scale)| scale.max(1) as usize)
        .max()
        .unwrap_or(1)
}

/// Damage in engine physical pixels → a buffer rect, clamped. `None`
/// when the wound misses the buffer entirely.
fn damage_to_buffer(
    rect: (i64, i64, i64, i64),
    width: usize,
    height: usize,
) -> Option<(i32, i32, i32, i32)> {
    let (x0, y0, x1, y1) = rect;
    let x0 = x0.clamp(0, width as i64);
    let y0 = y0.clamp(0, height as i64);
    let x1 = x1.clamp(0, width as i64);
    let y1 = y1.clamp(0, height as i64);
    (x1 > x0 && y1 > y0).then(|| (x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

// MARK: - the client (one thread, one window this phase)

struct OutputInfo {
    name: u32,
    proxy: *mut Proxy,
    scale: i32,
    pending_scale: i32,
}

struct Backing {
    pool: *mut Proxy,
    buffer: *mut Proxy,
    map: *mut u8,
    len: usize,
    width: usize,
    height: usize,
    fd: c_int,
    released: bool,
}

struct Window {
    surface: *mut Proxy,
    xdg_surface: *mut Proxy,
    toplevel: *mut Proxy,
    logical: (f64, f64),
    map: MapState,
    /// The size the role staged for the next `xdg_surface.configure`.
    pending_size: Option<(i32, i32)>,
    scale: usize,
    entered: Vec<u32>,
    backing: Option<Backing>,
    frame_inflight: bool,
    paused: bool,
    last_frame: Option<Instant>,
}

struct CursorState {
    surface: *mut Proxy,
    theme: *mut c_void,
    theme_scale: usize,
    current: Cursor,
}

struct Client {
    display: *mut Display,
    registry: *mut Proxy,
    compositor: *mut Proxy,
    shm: *mut Proxy,
    pointer: *mut Proxy,
    wm_base: *mut Proxy,
    protocols: &'static Protocols,
    outputs: Vec<OutputInfo>,
    globals: Vec<(u32, String, u32)>,
    win: Option<Window>,
    serials: Serials,
    clicks: ClickClock,
    pointer_pos: (f64, f64),
    cursor: CursorState,
    /// Counts presenting commits — the configure road checks whether
    /// an ack was followed by one.
    presents: u64,
    quit: bool,
}

thread_local! {
    static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) };
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
    static NEXT_BLINK: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn with_client<R>(body: impl FnOnce(&mut Client) -> R) -> R {
    CLIENT.with(|slot| {
        let mut slot = slot.borrow_mut();
        body(slot.as_mut().expect("the wayland client exists"))
    })
}

// MARK: - events out (the shell's vocabulary)

pub enum AppEvent {
    Redraw,
    MouseMoved { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, clicks: u8 },
    MouseUp { x: f64, y: f64 },
    RightMouseDown { x: f64, y: f64 },
    MouseExited,
    Blink,
    Frame { dt: f64 },
}

pub fn set_handler(handler: Box<dyn FnMut(AppEvent)>) {
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

/// Delivers an event to the handler — used by the loop and by the
/// first frame. The drain never runs nested, so the borrow holds.
pub fn dispatch(event: AppEvent) {
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

// MARK: - connect and the registry census

fn connect() {
    let display = unsafe { wl_display_connect(std::ptr::null()) };
    assert!(!display.is_null(), "no wayland display — is WAYLAND_DISPLAY set?");
    let protocols: &'static Protocols = Box::leak(Box::new(build_protocols()));
    // wl_display is itself a proxy; get_registry is its opcode 1
    let registry = unsafe {
        construct(display as *mut Proxy, 1, &raw const wl_registry_interface, &mut [arg_n()], TAG_REGISTRY)
    };
    assert!(!registry.is_null(), "wl_registry");
    unsafe { wl_display_roundtrip(display) };

    // the census landed in the queue; drain it before anything binds
    let mut globals = Vec::new();
    EVQ.with(|q| {
        for ev in q.borrow_mut().drain(..) {
            if let Ev::Global { name, interface, version } = ev {
                globals.push((name, interface, version));
            }
        }
    });

    let bind = |interface: &CStr, table: *const WlInterface, max: u32, tag: usize| -> *mut Proxy {
        let found = globals
            .iter()
            .find(|(_, name, _)| name.as_bytes() == interface.to_bytes())
            .map(|&(name, _, version)| (name, version.min(max)));
        match found {
            Some((name, version)) => unsafe {
                construct_versioned(
                    registry,
                    0, // wl_registry.bind
                    table,
                    version,
                    &mut [arg_u(name), arg_s(interface), arg_u(version), arg_n()],
                    tag,
                )
            },
            None => std::ptr::null_mut(),
        }
    };

    let compositor =
        bind(c"wl_compositor", &raw const wl_compositor_interface, 6, TAG_MAIN_SURFACE);
    let shm = bind(c"wl_shm", &raw const wl_shm_interface, 1, TAG_SYNC);
    let seat = bind(c"wl_seat", &raw const wl_seat_interface, 9, TAG_SEAT);
    let wm_base = bind(c"xdg_wm_base", protocols.wm_base, 5, TAG_WM_BASE);
    assert!(!compositor.is_null(), "wl_compositor is mandatory");
    assert!(!shm.is_null(), "wl_shm is mandatory");
    assert!(!wm_base.is_null(), "xdg_wm_base is mandatory");

    let mut outputs = Vec::new();
    for &(name, ref interface, version) in &globals {
        // below v2 an output has no scale/done — it cannot vote
        if interface == "wl_output" && version >= 2 {
            let bound = version.min(4);
            let proxy = unsafe {
                construct_versioned(
                    registry,
                    0,
                    &raw const wl_output_interface,
                    bound,
                    &mut [arg_u(name), arg_s(c"wl_output"), arg_u(bound), arg_n()],
                    OUTPUT_TAG_BASE + name as usize,
                )
            };
            outputs.push(OutputInfo { name, proxy, scale: 1, pending_scale: 1 });
        }
    }

    // the pointer: capability events are racy at bind, the census is
    // not — WSLg and every desktop advertise the pointer up front
    let pointer = if seat.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { construct(seat, 0, &raw const wl_pointer_interface, &mut [arg_n()], TAG_POINTER) }
    };

    let cursor_surface =
        unsafe { construct(compositor, 0, &raw const wl_surface_interface, &mut [arg_n()], TAG_CURSOR_SURFACE) };

    CLIENT.with(|slot| {
        *slot.borrow_mut() = Some(Client {
            display,
            registry,
            compositor,
            shm,
            pointer,
            wm_base,
            protocols,
            outputs,
            globals,
            win: None,
            serials: Serials::default(),
            clicks: ClickClock::default(),
            pointer_pos: (0.0, 0.0),
            cursor: CursorState {
                surface: cursor_surface,
                theme: std::ptr::null_mut(),
                theme_scale: 0,
                current: Cursor::Arrow,
            },
            presents: 0,
            quit: false,
        })
    });

    // outputs answer their geometry/scale burst; the census settles
    unsafe { wl_display_roundtrip(display) };
    drain_protocol_events();
}

// MARK: - the window

/// One window this phase. Creation runs the first half of the map
/// dance and BLOCKS until the compositor's first configure — from here
/// on, attaching is legal and the first present maps the window.
pub fn create_window(title: &str, width: f64, height: f64, _scene_chrome: bool) -> WindowHandle {
    if CLIENT.with(|slot| slot.borrow().is_none()) {
        connect();
    }
    let title_c = std::ffi::CString::new(title).unwrap_or_default();
    with_client(|client| {
        let surface = unsafe {
            construct(
                client.compositor,
                0,
                &raw const wl_surface_interface,
                &mut [arg_n()],
                TAG_MAIN_SURFACE,
            )
        };
        let xdg_surface = unsafe {
            construct(
                client.wm_base,
                2,
                client.protocols.xdg_surface as *const WlInterface,
                &mut [arg_n(), arg_o(surface)],
                TAG_XDG_SURFACE,
            )
        };
        let toplevel = unsafe {
            construct(
                xdg_surface,
                1,
                client.protocols.toplevel as *const WlInterface,
                &mut [arg_n()],
                TAG_TOPLEVEL,
            )
        };
        unsafe {
            request(toplevel, 2, &mut [WlArgument { s: title_c.as_ptr() }]);
            request(toplevel, 3, &mut [arg_s(c"bunny_ui")]);
            // the first commit carries NO buffer — it asks to be configured
            request(surface, 6, &mut no_args());
            wl_display_flush(client.display);
        }
        client.win = Some(Window {
            surface,
            xdg_surface,
            toplevel,
            logical: (width, height),
            map: MapState::default(),
            pending_size: None,
            scale: 1,
            entered: Vec::new(),
            backing: None,
            frame_inflight: false,
            paused: true,
            last_frame: None,
        });
    });
    // the first configure arrives async; wait for it so the first
    // present (still before anyone sees the window) is legal — the
    // roundtrip runs OUTSIDE the client borrow (it dispatches)
    for _ in 0..64 {
        let display = with_client(|client| client.display);
        unsafe { wl_display_roundtrip(display) };
        drain_protocol_events();
        if with_client(|client| client.win.as_ref().is_some_and(|w| w.map.configured)) {
            break;
        }
    }
    WindowHandle
}

/// On wayland a window appears when its first buffer commits — the
/// first present IS the reveal, so the anti-flash order holds by
/// protocol design and this is a no-op kept for the twins' shape.
pub fn show_window(_window: WindowHandle) {}

/// The one window this phase; the handle is the twins' shape with the
/// identity living in the client state.
#[derive(Clone, Copy)]
pub struct WindowHandle;

impl WindowHandle {
    /// Logical size of the content area (the layout viewport).
    pub fn content_size(&self) -> (f64, f64) {
        with_client(|client| client.win.as_ref().map(|w| w.logical).unwrap_or((0.0, 0.0)))
    }

    /// The integer raster scale the engine sees.
    pub fn scale(&self) -> usize {
        with_client(|client| client.win.as_ref().map(|w| w.scale).unwrap_or(1))
    }

    /// Presents damaged rects only: syncs the shm backing with
    /// damage-only row copies (RGBA → XRGB in the same pass), marks
    /// each rect on the surface, arms the frame callback and commits.
    pub fn blit_partial(
        &self,
        width: usize,
        height: usize,
        rgba: &[u8],
        damage: &[(i64, i64, i64, i64)],
    ) {
        if damage.is_empty() {
            return;
        }
        present_rows(width, height, rgba, damage);
    }

    pub fn set_cursor(&self, cursor: Cursor) {
        let changed = with_client(|client| {
            let previous = client.cursor.current;
            client.cursor.current = cursor;
            previous != cursor
        });
        if changed {
            apply_cursor();
        }
    }
}

// MARK: - the shm backing and the present

fn ensure_backing(client: &mut Client, width: usize, height: usize) -> bool {
    let win = client.win.as_mut().expect("window for the backing");
    if let Some(backing) = &win.backing
        && backing.width == width
        && backing.height == height
    {
        return true;
    }
    if let Some(old) = win.backing.take() {
        unsafe {
            destroy(old.buffer, 0);
            destroy(old.pool, 1);
            munmap(old.map as *mut c_void, old.len);
            close(old.fd);
        }
    }
    let len = width * height * 4;
    if len == 0 {
        return false;
    }
    let fd = unsafe { memfd_create(c"bunny-shm".as_ptr(), MFD_CLOEXEC) };
    if fd < 0 || unsafe { ftruncate(fd, len as i64) } != 0 {
        if fd >= 0 {
            unsafe { close(fd) };
        }
        eprintln!("bunny_ui_linux: shm backing failed");
        return false;
    }
    let map = unsafe {
        mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0)
    };
    if map as isize == -1 {
        unsafe { close(fd) };
        eprintln!("bunny_ui_linux: shm map failed");
        return false;
    }
    let pool = unsafe {
        construct(
            client.shm,
            0,
            &raw const wl_shm_pool_interface,
            &mut [arg_n(), arg_h(fd), arg_i(len as i32)],
            TAG_SYNC,
        )
    };
    const XRGB8888: u32 = 1;
    let buffer = unsafe {
        construct(
            pool,
            0,
            &raw const wl_buffer_interface,
            &mut [
                arg_n(),
                arg_i(0),
                arg_i(width as i32),
                arg_i(height as i32),
                arg_i((width * 4) as i32),
                arg_u(XRGB8888),
            ],
            TAG_BUFFER,
        )
    };
    win.backing = Some(Backing {
        pool,
        buffer,
        map: map as *mut u8,
        len,
        width,
        height,
        fd,
        released: true,
    });
    true
}

/// The single retained buffer: damage-only copies NEED last frame's
/// pixels in place, so there is one buffer and the shell waits for its
/// release before writing — weston releases on commit-upload, so the
/// wait is almost always already over.
fn wait_release(client: &mut Client) {
    for _ in 0..20 {
        let released = client
            .win
            .as_ref()
            .and_then(|w| w.backing.as_ref())
            .is_none_or(|backing| backing.released);
        if released {
            return;
        }
        unsafe {
            wl_display_flush(client.display);
            let mut fds = [PollFd { fd: wl_display_get_fd(client.display), events: POLLIN, revents: 0 }];
            poll(fds.as_mut_ptr(), 1, 4);
            if wl_display_prepare_read(client.display) == 0 {
                wl_display_read_events(client.display);
            }
            wl_display_dispatch_pending(client.display);
        }
        // pick ONLY the release out of the queue — the rest belongs to
        // the loop, and a nested AppEvent would re-enter the handler
        EVQ.with(|q| {
            let mut q = q.borrow_mut();
            let mut keep = VecDeque::with_capacity(q.len());
            while let Some(ev) = q.pop_front() {
                if matches!(ev, Ev::BufferRelease) {
                    if let Some(backing) =
                        client.win.as_mut().and_then(|w| w.backing.as_mut())
                    {
                        backing.released = true;
                    }
                } else {
                    keep.push_back(ev);
                }
            }
            *q = keep;
        });
    }
}

fn present_rows(width: usize, height: usize, rgba: &[u8], damage: &[(i64, i64, i64, i64)]) {
    with_client(|client| {
        if !client.win.as_ref().is_some_and(|w| w.map.can_attach()) {
            return;
        }
        if !ensure_backing(client, width, height) {
            return;
        }
        wait_release(client);
        let win = client.win.as_mut().expect("window for the present");
        let backing = win.backing.as_mut().expect("backing for the present");
        // damage rows: RGBA → XRGB (little-endian bytes B,G,R,X) in one pass
        for &rect in damage {
            let Some((x, y, w, h)) = damage_to_buffer(rect, width, height) else {
                continue;
            };
            for row in y..y + h {
                let start = (row as usize * width + x as usize) * 4;
                let source = &rgba[start..start + w as usize * 4];
                let target = unsafe {
                    std::slice::from_raw_parts_mut(backing.map.add(start), w as usize * 4)
                };
                for (source_px, target_px) in
                    source.chunks_exact(4).zip(target.chunks_exact_mut(4))
                {
                    target_px[0] = source_px[2];
                    target_px[1] = source_px[1];
                    target_px[2] = source_px[0];
                    target_px[3] = 0xFF;
                }
            }
        }
        backing.released = false;
        let buffer = backing.buffer;
        unsafe {
            let surface_version = wl_proxy_get_version(win.surface);
            if win.scale > 1 && surface_version >= 3 {
                request(win.surface, 8, &mut [arg_i(win.scale as i32)]);
            }
            request(win.surface, 1, &mut [arg_o(buffer), arg_i(0), arg_i(0)]);
            for &rect in damage {
                if let Some((x, y, w, h)) = damage_to_buffer(rect, width, height) {
                    if surface_version >= 4 {
                        // damage_buffer speaks buffer pixels directly
                        request(win.surface, 9, &mut [arg_i(x), arg_i(y), arg_i(w), arg_i(h)]);
                    } else {
                        // the legacy damage speaks surface coordinates
                        let scale = win.scale.max(1) as i32;
                        request(
                            win.surface,
                            2,
                            &mut [
                                arg_i(x / scale),
                                arg_i(y / scale),
                                arg_i((w + scale - 1) / scale + 1),
                                arg_i((h + scale - 1) / scale + 1),
                            ],
                        );
                    }
                }
            }
            // every presenting commit carries a frame callback; `done`
            // is gated by the paused flag, so a parked app simply lets
            // it fall — and no bare commit ever follows a present
            if !win.frame_inflight {
                let callback = construct(
                    win.surface,
                    3,
                    &raw const wl_callback_interface,
                    &mut [arg_n()],
                    TAG_FRAME,
                );
                win.frame_inflight = !callback.is_null();
            }
            request(win.surface, 6, &mut no_args());
            wl_display_flush(client.display);
        }
        win.map.on_present();
        client.presents += 1;
    });
}

// MARK: - cursor (theme tier; the shape protocol joins on compositors that have it)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cursor {
    Arrow,
    Pointing,
    ResizeLeftRight,
}

/// Theme names are inconsistent across the ecosystem; each style
/// carries an ordered chain and the first hit wins.
fn cursor_names(cursor: Cursor) -> &'static [&'static CStr] {
    match cursor {
        Cursor::Arrow => &[c"default", c"left_ptr", c"arrow"],
        Cursor::Pointing => &[c"pointer", c"hand2", c"hand1", c"pointing_hand"],
        Cursor::ResizeLeftRight => &[c"ew-resize", c"sb_h_double_arrow", c"size_hor", c"col-resize"],
    }
}

fn apply_cursor() {
    with_client(|client| {
        let scale = client.win.as_ref().map(|w| w.scale).unwrap_or(1);
        if client.cursor.theme.is_null() || client.cursor.theme_scale != scale {
            if !client.cursor.theme.is_null() {
                unsafe { wl_cursor_theme_destroy(client.cursor.theme) };
            }
            // NULL theme name honors $XCURSOR_THEME and the theme's
            // own inheritance chain
            client.cursor.theme =
                unsafe { wl_cursor_theme_load(std::ptr::null(), 24 * scale as c_int, client.shm) };
            client.cursor.theme_scale = scale;
        }
        if client.cursor.theme.is_null() || client.pointer.is_null() {
            return;
        }
        let cursor = cursor_names(client.cursor.current)
            .iter()
            .map(|name| unsafe { wl_cursor_theme_get_cursor(client.cursor.theme, name.as_ptr()) })
            .find(|found| !found.is_null());
        let Some(cursor) = cursor else {
            return; // resolves nowhere: leave the pointer as it is
        };
        unsafe {
            let images = std::slice::from_raw_parts((*cursor).images, (*cursor).image_count as usize);
            let Some(&image) = images.first() else { return };
            let buffer = wl_cursor_image_get_buffer(image);
            if buffer.is_null() {
                return;
            }
            let surface = client.cursor.surface;
            if wl_proxy_get_version(surface) >= 3 {
                request(surface, 8, &mut [arg_i(scale as i32)]);
            }
            request(surface, 1, &mut [arg_o(buffer), arg_i(0), arg_i(0)]);
            request(surface, 2, &mut [arg_i(0), arg_i(0), arg_i(i32::MAX), arg_i(i32::MAX)]);
            request(surface, 6, &mut no_args());
            request(
                client.pointer,
                0,
                &mut [
                    arg_u(client.serials.enter),
                    arg_o(surface),
                    arg_i(((*image).hotspot_x / scale as u32) as i32),
                    arg_i(((*image).hotspot_y / scale as u32) as i32),
                ],
            );
        }
    });
}

// MARK: - IME mirror (the door opens at its phase; the slot keeps the twins' order)

pub fn sync_ime(_state: Option<(bool, usize, (f64, f64, f64, f64))>) {}

// MARK: - the frame driver (no thread: the compositor's callback is the clock)

pub fn set_frame_driver_paused(paused: bool) {
    with_client(|client| {
        if let Some(win) = client.win.as_mut() {
            win.paused = paused;
        }
    });
}

// MARK: - the loop

/// Interprets the queued protocol events. Runs only from the loop and
/// from init — never nested inside a handler.
fn drain_protocol_events() {
    loop {
        let Some(ev) = EVQ.with(|q| q.borrow_mut().pop_front()) else {
            return;
        };
        match ev {
            Ev::Ping { serial } => with_client(|client| unsafe {
                request(client.wm_base, 3, &mut [arg_u(serial)]);
            }),
            Ev::ToplevelConfigure { width, height } => with_client(|client| {
                if let Some(win) = client.win.as_mut() {
                    // zero means "your choice": keep what we have
                    win.pending_size = (width > 0 && height > 0).then_some((width, height));
                }
            }),
            Ev::SurfaceConfigure { serial } => {
                let (resized, mapped, before) = with_client(|client| {
                    let presents = client.presents;
                    let Some(win) = client.win.as_mut() else { return (false, false, presents) };
                    let staged = win.pending_size.take();
                    win.map.on_configure();
                    unsafe { request(win.xdg_surface, 4, &mut [arg_u(serial)]) };
                    let mapped = win.map.mapped;
                    if let Some((w, h)) = staged {
                        let logical = (w as f64, h as f64);
                        if logical != win.logical {
                            win.logical = logical;
                            return (mapped, mapped, presents);
                        }
                    }
                    (false, mapped, presents)
                });
                if resized {
                    dispatch(AppEvent::Redraw);
                }
                // an ack only takes effect on the NEXT commit — a
                // state-only configure on a parked app would otherwise
                // never see one, and the shell may hold the window's
                // very reveal on that cycle closing
                if mapped {
                    with_client(|client| {
                        if client.presents == before
                            && let Some(win) = client.win.as_ref()
                        {
                            unsafe {
                                request(win.surface, 6, &mut no_args());
                                wl_display_flush(client.display);
                            }
                        }
                    });
                }
            }
            Ev::ToplevelClose => with_client(|client| client.quit = true),
            Ev::FrameDone => {
                let dt = with_client(|client| {
                    let Some(win) = client.win.as_mut() else { return None };
                    win.frame_inflight = false;
                    if win.paused {
                        win.last_frame = None;
                        return None;
                    }
                    let now = Instant::now();
                    let dt = win
                        .last_frame
                        .map(|last| (now - last).as_secs_f64())
                        .unwrap_or(1.0 / 60.0)
                        .clamp(0.0, 1.0 / 30.0);
                    win.last_frame = Some(now);
                    Some(dt)
                });
                if let Some(dt) = dt {
                    dispatch(AppEvent::Frame { dt });
                }
            }
            Ev::PointerEnter { serial, x, y } => {
                with_client(|client| {
                    client.serials.enter = serial;
                    client.pointer_pos = (x, y);
                });
                // a stale enter serial means an ignored set_cursor —
                // re-assert, then let the scene see the entry as a move
                apply_cursor();
                dispatch(AppEvent::MouseMoved { x, y });
            }
            Ev::PointerLeave => dispatch(AppEvent::MouseExited),
            Ev::PointerMotion { x, y } => {
                with_client(|client| client.pointer_pos = (x, y));
                dispatch(AppEvent::MouseMoved { x, y });
            }
            Ev::PointerButton { serial, time_ms, button, pressed } => {
                let (x, y) = with_client(|client| {
                    client.serials.record_button(serial, pressed);
                    client.pointer_pos
                });
                const BTN_LEFT: u32 = 0x110;
                const BTN_RIGHT: u32 = 0x111;
                match (button, pressed) {
                    (BTN_LEFT, true) => {
                        let clicks =
                            with_client(|client| client.clicks.click(time_ms, x, y));
                        dispatch(AppEvent::MouseDown { x, y, clicks });
                    }
                    (BTN_LEFT, false) => dispatch(AppEvent::MouseUp { x, y }),
                    (BTN_RIGHT, true) => dispatch(AppEvent::RightMouseDown { x, y }),
                    _ => {}
                }
            }
            Ev::SurfaceEnter { output_ptr } => {
                if let Some(name) = resolve_output(output_ptr) {
                    update_scale(|win| win.entered.push(name));
                }
            }
            Ev::SurfaceLeave { output_ptr } => {
                if let Some(name) = resolve_output(output_ptr) {
                    update_scale(|win| win.entered.retain(|&entered| entered != name));
                }
            }
            Ev::OutputScale { output_name, scale } => with_client(|client| {
                if let Some(output) =
                    client.outputs.iter_mut().find(|output| output.name == output_name)
                {
                    output.pending_scale = scale;
                }
            }),
            Ev::OutputDone { output_name } => {
                with_client(|client| {
                    if let Some(output) =
                        client.outputs.iter_mut().find(|output| output.name == output_name)
                    {
                        output.scale = output.pending_scale;
                    }
                });
                update_scale(|_| {});
            }
            Ev::Global { name, interface, version } => with_client(|client| {
                // late arrivals join the census; later phases bind on demand
                client.globals.push((name, interface, version));
            }),
            Ev::GlobalRemove { name } => {
                // outputs genuinely unplug (an RDP resize can); drop it
                // from the census and let the scale re-resolve
                let removed = with_client(|client| {
                    client.globals.retain(|&(id, _, _)| id != name);
                    if let Some(index) =
                        client.outputs.iter().position(|output| output.name == name)
                    {
                        let output = client.outputs.remove(index);
                        unsafe { wl_proxy_destroy(output.proxy) };
                        true
                    } else {
                        false
                    }
                });
                if removed {
                    update_scale(|win| win.entered.retain(|_| true));
                }
            }
            Ev::BufferRelease => with_client(|client| {
                if let Some(backing) = client.win.as_mut().and_then(|w| w.backing.as_mut()) {
                    backing.released = true;
                }
            }),
        }
    }
}

/// The census is keyed by registry name; enter/leave carried a proxy.
fn resolve_output(output_ptr: usize) -> Option<u32> {
    with_client(|client| {
        client
            .outputs
            .iter()
            .find(|output| output.proxy as usize == output_ptr)
            .map(|output| output.name)
    })
}

/// Applies an entered-outputs edit, re-resolves the ladder's tier 3,
/// and asks for a fresh frame when the scale actually moved.
fn update_scale(edit: impl FnOnce(&mut Window)) {
    let changed = with_client(|client| {
        let outputs: Vec<(u32, i32)> =
            client.outputs.iter().map(|output| (output.name, output.scale)).collect();
        let Some(win) = client.win.as_mut() else { return false };
        edit(win);
        let scale = resolve_scale(&win.entered, &outputs);
        if scale != win.scale {
            win.scale = scale;
            true
        } else {
            false
        }
    });
    if changed {
        dispatch(AppEvent::Redraw);
    }
}

/// The pump: prepare-read integration of the wayland fd plus the
/// timeout scheduler (blink today; repeat and more join later). Runs
/// until the window closes.
pub fn run() {
    NEXT_BLINK.with(|cell| cell.set(Some(Instant::now() + BLINK_INTERVAL)));
    loop {
        let (display, quit) = with_client(|client| (client.display, client.quit));
        if quit {
            break;
        }
        unsafe {
            while wl_display_prepare_read(display) != 0 {
                wl_display_dispatch_pending(display);
            }
            wl_display_flush(display);
            let timeout = NEXT_BLINK.with(|cell| {
                cell.get()
                    .map(|at| {
                        at.saturating_duration_since(Instant::now()).as_millis().min(1000) as c_int
                    })
                    .unwrap_or(1000)
            });
            let mut fds =
                [PollFd { fd: wl_display_get_fd(display), events: POLLIN, revents: 0 }];
            let ready = poll(fds.as_mut_ptr(), 1, timeout.max(0));
            if ready > 0 && fds[0].revents & POLLIN != 0 {
                wl_display_read_events(display);
            } else {
                wl_display_cancel_read(display);
            }
            wl_display_dispatch_pending(display);
            if wl_display_get_error(display) != 0 {
                eprintln!("bunny_ui_linux: the wayland connection died");
                break;
            }
        }
        drain_protocol_events();
        let blink_due = NEXT_BLINK.with(|cell| {
            let due = cell.get().is_some_and(|at| Instant::now() >= at);
            if due {
                cell.set(Some(Instant::now() + BLINK_INTERVAL));
            }
            due
        });
        if blink_due {
            dispatch(AppEvent::Blink);
        }
    }
    teardown();
}

const BLINK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Protocol teardown order is law: role → xdg_surface → wl_surface
/// last, devices released, then the connection.
fn teardown() {
    CLIENT.with(|slot| {
        let Some(client) = slot.borrow_mut().take() else { return };
        unsafe {
            if let Some(win) = client.win {
                destroy(win.toplevel, 0);
                destroy(win.xdg_surface, 0);
                if let Some(backing) = win.backing {
                    destroy(backing.buffer, 0);
                    destroy(backing.pool, 1);
                    munmap(backing.map as *mut c_void, backing.len);
                    close(backing.fd);
                }
                destroy(win.surface, 0);
            }
            if !client.cursor.theme.is_null() {
                wl_cursor_theme_destroy(client.cursor.theme);
            }
            destroy(client.cursor.surface, 0);
            if !client.pointer.is_null() {
                if wl_proxy_get_version(client.pointer) >= 3 {
                    destroy(client.pointer, 1); // release
                } else {
                    wl_proxy_destroy(client.pointer);
                }
            }
            for output in &client.outputs {
                wl_proxy_destroy(output.proxy);
            }
            destroy(client.wm_base, 0);
            wl_proxy_destroy(client.registry);
            wl_display_disconnect(client.display);
        }
    });
}

// MARK: - tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Argument count a signature promises (digits are since-versions,
    /// `?` is nullability — neither is an argument).
    fn arg_count(signature: &CStr) -> usize {
        signature
            .to_bytes()
            .iter()
            .filter(|byte| matches!(byte, b'u' | b'i' | b'f' | b's' | b'o' | b'n' | b'a' | b'h'))
            .count()
    }

    #[test]
    fn every_table_row_matches_its_signature() {
        for (name, _, methods, events) in xdg_spec() {
            for Msg(msg, signature, types) in methods.iter().chain(events.iter()) {
                assert_eq!(
                    types.len(),
                    arg_count(signature),
                    "{}.{}: one types slot per argument",
                    name.to_string_lossy(),
                    msg.to_string_lossy(),
                );
            }
        }
    }

    /// The installed protocol XML is the ground truth: every message
    /// name in our tables must appear in ITS interface block in OUR
    /// opcode order. Skips silently where the file is not installed.
    #[test]
    fn the_tables_match_the_installed_xml() {
        let path = "/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml";
        let Ok(xml) = std::fs::read_to_string(path) else { return };
        for (name, _, methods, events) in xdg_spec() {
            let open = format!("<interface name=\"{}\"", name.to_string_lossy());
            let start = xml.find(&open).expect("interface present in the XML");
            let block = &xml[start..];
            let end = block.find("</interface>").expect("interface block closes");
            let block = &block[..end];
            let mut cursor = 0;
            for Msg(msg, _, _) in &methods {
                let needle = format!("<request name=\"{}\"", msg.to_string_lossy());
                let at = block[cursor..]
                    .find(&needle)
                    .unwrap_or_else(|| panic!("{needle} in opcode order"));
                cursor += at + needle.len();
            }
            let mut cursor = 0;
            for Msg(msg, _, _) in &events {
                let needle = format!("<event name=\"{}\"", msg.to_string_lossy());
                let at = block[cursor..]
                    .find(&needle)
                    .unwrap_or_else(|| panic!("{needle} in opcode order"));
                cursor += at + needle.len();
            }
        }
    }

    #[test]
    fn fixed_point_converts_both_directions() {
        assert_eq!(fixed_to_f64(256), 1.0);
        assert_eq!(fixed_to_f64(-256), -1.0);
        assert_eq!(fixed_to_f64(384), 1.5);
        assert_eq!(fixed_to_f64(1), 1.0 / 256.0);
    }

    #[test]
    fn damage_clamps_to_the_buffer() {
        assert_eq!(damage_to_buffer((10, 20, 30, 40), 100, 100), Some((10, 20, 20, 20)));
        assert_eq!(damage_to_buffer((-5, -5, 10, 10), 100, 100), Some((0, 0, 10, 10)));
        assert_eq!(damage_to_buffer((90, 90, 200, 200), 100, 100), Some((90, 90, 10, 10)));
        assert_eq!(damage_to_buffer((200, 0, 300, 10), 100, 100), None, "misses entirely");
        assert_eq!(damage_to_buffer((10, 10, 10, 40), 100, 100), None, "zero width");
    }

    #[test]
    fn the_map_dance_holds_its_order() {
        let mut map = MapState::default();
        assert!(!map.can_attach(), "a buffer before the first configure is a protocol error");
        map.on_present();
        assert!(!map.mapped, "a present that could not attach maps nothing");
        map.on_configure();
        assert!(map.can_attach());
        assert!(!map.mapped, "configured is not yet mapped");
        map.on_present();
        assert!(map.mapped, "the first real present maps the window");
    }

    #[test]
    fn button_serials_record_on_press_only() {
        let mut serials = Serials::default();
        serials.record_button(7, true);
        assert_eq!(serials.press, 7);
        serials.record_button(9, false);
        assert_eq!(serials.press, 7, "a release serial would be declined by compositors");
    }

    #[test]
    fn the_click_clock_counts_and_resets() {
        let mut clock = ClickClock::default();
        assert_eq!(clock.click(1000, 50.0, 50.0), 1);
        assert_eq!(clock.click(1200, 51.0, 50.0), 2, "inside 400ms and 4px");
        assert_eq!(clock.click(1300, 52.0, 51.0), 3, "triple");
        assert_eq!(clock.click(1800, 52.0, 51.0), 1, "too late resets");
        assert_eq!(clock.click(1900, 80.0, 51.0), 1, "too far resets");
    }

    #[test]
    fn the_scale_ladder_takes_the_max_of_entered_outputs() {
        let outputs = [(1, 1), (2, 2), (3, 3)];
        assert_eq!(resolve_scale(&[], &outputs), 1, "nowhere yet: the safe floor");
        assert_eq!(resolve_scale(&[1], &outputs), 1);
        assert_eq!(resolve_scale(&[1, 2], &outputs), 2, "straddling takes the max");
        assert_eq!(resolve_scale(&[9], &outputs), 1, "an unknown output cannot vote");
        assert_eq!(resolve_scale(&[3], &[(3, 0)]), 1, "a zero scale clamps to one");
    }

    /// The one test that talks to a compositor — and skips in silence
    /// where there is none (CI, bare consoles).
    #[test]
    fn the_display_answers_when_present() {
        if std::env::var("WAYLAND_DISPLAY").is_err() {
            return;
        }
        unsafe {
            let display = wl_display_connect(std::ptr::null());
            if display.is_null() {
                return;
            }
            assert!(wl_display_roundtrip(display) >= 0, "a live display answers a roundtrip");
            wl_display_disconnect(display);
        }
    }
}
