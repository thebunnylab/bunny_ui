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
use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::time::Instant;

// MARK: - libc floor (the only raw syscalls the shell needs)

#[repr(C)]
pub(crate) struct PollFd {
    pub(crate) fd: c_int,
    pub(crate) events: i16,
    pub(crate) revents: i16,
}

pub(crate) const POLLIN: i16 = 0x1;
const MFD_CLOEXEC: c_uint = 0x1;
const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;
const MAP_SHARED: c_int = 0x1;

pub(crate) const O_CLOEXEC: c_int = 0o2000000;
pub(crate) const O_NONBLOCK: c_int = 0o4000;
const MAP_PRIVATE: c_int = 0x2;

// the libc floor is one — the x11 door borrows these instead of
// redeclaring (a diverging redeclaration is a compile error)
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
    pub(crate) fn poll(fds: *mut PollFd, count: u64, timeout_ms: c_int) -> c_int;
    pub(crate) fn close(fd: c_int) -> c_int;
    pub(crate) fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    pub(crate) fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn getpid() -> c_int;
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
    static wl_keyboard_interface: WlInterface;
    static wl_data_device_manager_interface: WlInterface;
    static wl_data_device_interface: WlInterface;
    static wl_data_source_interface: WlInterface;
    static wl_subcompositor_interface: WlInterface;
    static wl_subsurface_interface: WlInterface;
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

// MARK: - libxkbcommon ABI (the keymap authority — NEVER parsed by hand)

const XKB_KEYMAP_FORMAT_TEXT_V1: c_int = 1;
const XKB_STATE_MODS_EFFECTIVE: c_int = 8;
const XKB_COMPOSE_COMPOSING: c_int = 1;
const XKB_COMPOSE_COMPOSED: c_int = 2;
const XKB_COMPOSE_CANCELLED: c_int = 3;

// the x11 door shares this whole family (its keymap comes from the
// device instead of a string, but the states and compose are one)
#[link(name = "xkbcommon")]
unsafe extern "C" {
    pub(crate) fn xkb_context_new(flags: c_int) -> *mut c_void;
    pub(crate) fn xkb_context_unref(context: *mut c_void);
    fn xkb_keymap_new_from_string(
        context: *mut c_void,
        text: *const c_char,
        format: c_int,
        flags: c_int,
    ) -> *mut c_void;
    pub(crate) fn xkb_keymap_unref(keymap: *mut c_void);
    fn xkb_keymap_key_repeats(keymap: *mut c_void, keycode: u32) -> c_int;
    pub(crate) fn xkb_state_new(keymap: *mut c_void) -> *mut c_void;
    pub(crate) fn xkb_state_unref(state: *mut c_void);
    pub(crate) fn xkb_state_update_mask(
        state: *mut c_void,
        depressed: u32,
        latched: u32,
        locked: u32,
        layout_depressed: u32,
        layout_latched: u32,
        layout_locked: u32,
    ) -> c_int;
    fn xkb_state_key_get_one_sym(state: *mut c_void, keycode: u32) -> u32;
    fn xkb_state_key_get_utf8(
        state: *mut c_void,
        keycode: u32,
        buffer: *mut c_char,
        size: usize,
    ) -> c_int;
    fn xkb_state_mod_name_is_active(
        state: *mut c_void,
        name: *const c_char,
        kind: c_int,
    ) -> c_int;
    pub(crate) fn xkb_compose_table_new_from_locale(
        context: *mut c_void,
        locale: *const c_char,
        flags: c_int,
    ) -> *mut c_void;
    pub(crate) fn xkb_compose_table_unref(table: *mut c_void);
    pub(crate) fn xkb_compose_state_new(table: *mut c_void, flags: c_int) -> *mut c_void;
    pub(crate) fn xkb_compose_state_unref(state: *mut c_void);
    fn xkb_compose_state_feed(state: *mut c_void, sym: u32) -> c_int;
    fn xkb_compose_state_get_status(state: *mut c_void) -> c_int;
    fn xkb_compose_state_get_utf8(state: *mut c_void, buffer: *mut c_char, size: usize) -> c_int;
    fn xkb_compose_state_reset(state: *mut c_void);
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
    TextInput,
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
    popup: &'static WlInterface,
    positioner: &'static WlInterface,
    ti_manager: &'static WlInterface,
    text_input: &'static WlInterface,
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

/// text-input v3, transcribed from the unstable XML in opcode order —
/// its own spec because its own file verifies it.
fn text_input_spec() -> [(&'static CStr, u32, Vec<Msg>, Vec<Msg>); 2] {
    use Iface::*;
    [
        (
            c"zwp_text_input_manager_v3",
            1,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"get_text_input", c"no", &[Some(TextInput), Some(CoreSeat)]),
            ],
            vec![],
        ),
        (
            c"zwp_text_input_v3",
            1,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"enable", c"", &[]),
                Msg(c"disable", c"", &[]),
                Msg(c"set_surrounding_text", c"sii", &[None, None, None]),
                Msg(c"set_text_change_cause", c"u", &[None]),
                Msg(c"set_content_type", c"uu", &[None, None]),
                Msg(c"set_cursor_rectangle", c"iiii", &[None, None, None, None]),
                Msg(c"commit", c"", &[]),
            ],
            vec![
                Msg(c"enter", c"o", &[Some(CoreSurface)]),
                Msg(c"leave", c"o", &[Some(CoreSurface)]),
                Msg(c"preedit_string", c"?sii", &[None, None, None]),
                Msg(c"commit_string", c"?s", &[None]),
                Msg(c"delete_surrounding_text", c"uu", &[None, None]),
                Msg(c"done", c"u", &[None]),
            ],
        ),
    ]
}

/// Builds the five `WlInterface` tables and leaks them. Two passes:
/// the interfaces are allocated first so the message rows can point at
/// each other (popup → positioner, xdg_surface → toplevel, …).
fn build_protocols() -> Protocols {
    let spec: Vec<(&'static CStr, u32, Vec<Msg>, Vec<Msg>)> =
        xdg_spec().into_iter().chain(text_input_spec()).collect();
    // pass 1: stable homes, filled with placeholders
    let slots: &'static mut [WlInterface; 7] = Box::leak(Box::new(std::array::from_fn(|_| {
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
            Iface::TextInput => unsafe { base.add(6) },
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
        ti_manager: &slots[5],
        text_input: &slots[6],
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

/// The wl_array the C side hands events — reads copy out immediately.
#[repr(C)]
struct WlArray {
    size: usize,
    alloc: usize,
    data: *mut c_void,
}

fn array_u32s(array: *mut c_void) -> Vec<u32> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe {
        let array = &*(array as *const WlArray);
        std::slice::from_raw_parts(array.data as *const u32, array.size / 4).to_vec()
    }
}

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
const TAG_DATA_DEVICE: usize = 13;
const TAG_DATA_OFFER: usize = 14;
const TAG_DATA_SOURCE: usize = 15;
const TAG_TEXT_INPUT: usize = 16;
const OUTPUT_TAG_BASE: usize = 0x1000;
/// Panel proxies encode index and role: base | (index << 2) | kind.
const PANEL_TAG_BASE: usize = 0x1000_0000;
const PANEL_KIND_SURFACE: usize = 0;
const PANEL_KIND_XDG: usize = 1;
const PANEL_KIND_POPUP: usize = 2;

/// A decoded protocol event, queued for the loop. The dispatcher owns
/// NOTHING but this queue — state and marshalling stay outside, so a
/// roundtrip can never re-enter a borrow.
enum Ev {
    Global { name: u32, interface: String, version: u32 },
    GlobalRemove { name: u32 },
    Ping { serial: u32 },
    SurfaceConfigure { serial: u32 },
    ToplevelConfigure { width: i32, height: i32, states: Vec<u32> },
    ToplevelClose,
    FrameDone,
    SurfaceEnter { output_ptr: usize },
    SurfaceLeave { output_ptr: usize },
    OutputScale { output_name: u32, scale: i32 },
    OutputDone { output_name: u32 },
    PointerEnter { serial: u32, surface_ptr: usize, x: f64, y: f64 },
    PointerLeave,
    PointerMotion { x: f64, y: f64 },
    PointerButton { serial: u32, time_ms: u32, button: u32, pressed: bool },
    PointerAxis { axis: u32, value: f64 },
    PointerAxisDiscrete { axis: u32, steps: i32 },
    PointerFrame,
    BufferRelease,
    KeyboardKeymap { format: u32, fd: i32, size: u32 },
    KeyboardEnter,
    KeyboardLeave,
    KeyboardKey { serial: u32, key: u32, pressed: bool },
    KeyboardMods { depressed: u32, latched: u32, locked: u32, group: u32 },
    RepeatInfo { rate: i32, delay: i32 },
    NewOffer { offer_ptr: usize },
    OfferMime { offer_ptr: usize, mime: String },
    Selection { offer_ptr: usize },
    SourceSend { mime: String, fd: i32 },
    SourceCancelled,
    PanelConfigure { index: usize, serial: u32 },
    PopupPosition { index: usize, x: i32, y: i32 },
    PopupDone { index: usize },
    ImePreedit { text: String, cursor_begin: i32 },
    ImeCommit { text: String },
    ImeDone { serial: u32 },
    ImeLeave,
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
                states: array_u32s(unsafe { arg(2).a }),
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
                surface_ptr: unsafe { arg(1).o } as usize,
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
            4 => push_ev(Ev::PointerAxis {
                axis: unsafe { arg(1).u },
                value: fixed_to_f64(unsafe { arg(2).f }),
            }),
            5 => push_ev(Ev::PointerFrame),
            8 => push_ev(Ev::PointerAxisDiscrete {
                axis: unsafe { arg(0).u },
                steps: unsafe { arg(1).i },
            }),
            _ => {} // axis_source/stop and the v8+ refinements: unread
        },
        TAG_BUFFER => push_ev(Ev::BufferRelease),
        TAG_KEYBOARD => match opcode {
            0 => push_ev(Ev::KeyboardKeymap {
                format: unsafe { arg(0).u },
                fd: unsafe { arg(1).h },
                size: unsafe { arg(2).u },
            }),
            1 => push_ev(Ev::KeyboardEnter),
            2 => push_ev(Ev::KeyboardLeave),
            3 => push_ev(Ev::KeyboardKey {
                serial: unsafe { arg(0).u },
                key: unsafe { arg(2).u },
                pressed: unsafe { arg(3).u } == 1,
            }),
            4 => push_ev(Ev::KeyboardMods {
                depressed: unsafe { arg(1).u },
                latched: unsafe { arg(2).u },
                locked: unsafe { arg(3).u },
                group: unsafe { arg(4).u },
            }),
            5 => push_ev(Ev::RepeatInfo {
                rate: unsafe { arg(0).i },
                delay: unsafe { arg(1).i },
            }),
            _ => {}
        },
        TAG_DATA_DEVICE => match opcode {
            0 => {
                // the offer proxy is SERVER-created; wire its
                // dispatcher before any of its events can land
                let offer = unsafe { arg(0).o };
                if !offer.is_null() {
                    unsafe {
                        wl_proxy_add_dispatcher(
                            offer,
                            dispatcher,
                            TAG_DATA_OFFER as *const c_void,
                            std::ptr::null_mut(),
                        );
                    }
                    push_ev(Ev::NewOffer { offer_ptr: offer as usize });
                }
            }
            5 => push_ev(Ev::Selection { offer_ptr: unsafe { arg(0).o } as usize }),
            _ => {} // drag-and-drop events: out of this war's scope
        },
        TAG_DATA_OFFER => {
            if opcode == 0 {
                let mime =
                    unsafe { CStr::from_ptr(arg(0).s) }.to_string_lossy().into_owned();
                push_ev(Ev::OfferMime { offer_ptr: _proxy as usize, mime });
            }
        }
        TAG_DATA_SOURCE => match opcode {
            1 => {
                let mime =
                    unsafe { CStr::from_ptr(arg(0).s) }.to_string_lossy().into_owned();
                push_ev(Ev::SourceSend { mime, fd: unsafe { arg(1).h } });
            }
            2 => push_ev(Ev::SourceCancelled),
            _ => {} // target(0): a dnd-only hint
        },
        TAG_CURSOR_SURFACE => {}
        TAG_TEXT_INPUT => match opcode {
            1 => push_ev(Ev::ImeLeave),
            2 => {
                let s = unsafe { arg(0).s };
                let text = if s.is_null() {
                    String::new()
                } else {
                    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
                };
                push_ev(Ev::ImePreedit { text, cursor_begin: unsafe { arg(1).i } });
            }
            3 => {
                let s = unsafe { arg(0).s };
                let text = if s.is_null() {
                    String::new()
                } else {
                    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
                };
                push_ev(Ev::ImeCommit { text });
            }
            5 => push_ev(Ev::ImeDone { serial: unsafe { arg(0).u } }),
            _ => {} // enter(0): the focus follows sync_ime; delete_surrounding(4): documented gap
        },
        tag if tag >= PANEL_TAG_BASE => {
            let index = (tag - PANEL_TAG_BASE) >> 2;
            match tag & 0x3 {
                PANEL_KIND_XDG => {
                    if opcode == 0 {
                        push_ev(Ev::PanelConfigure { index, serial: unsafe { arg(0).u } });
                    }
                }
                PANEL_KIND_POPUP => match opcode {
                    0 => push_ev(Ev::PopupPosition {
                        index,
                        x: unsafe { arg(0).i },
                        y: unsafe { arg(1).i },
                    }),
                    1 => push_ev(Ev::PopupDone { index }),
                    _ => {}
                },
                _ => {} // the panel's wl_surface: enter/leave unread
            }
        }
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

/// The serial slots the protocol demands back. Buttons and keys record
/// on PRESS only — compositors decline moves and grabs quoting release
/// serials.
#[derive(Default)]
struct Serials {
    enter: u32,
    press: u32,
    key_press: u32,
}

impl Serials {
    fn record_button(&mut self, serial: u32, pressed: bool) {
        if pressed {
            self.press = serial;
        }
    }

    fn record_key(&mut self, serial: u32, pressed: bool) {
        if pressed {
            self.key_press = serial;
        }
    }

    /// Compositor serials rise monotonically; a selection claim wants
    /// the freshest of ANY kind — a stale kind is silently rejected.
    fn latest(&self) -> u32 {
        self.enter.max(self.press).max(self.key_press)
    }
}

/// Client-side double click: the compositor sends plain buttons, the
/// shell counts. Same window the platforms use: 400 ms and a 4 px
/// wander budget.
#[derive(Default)]
pub(crate) struct ClickClock {
    last: Option<(u32, f64, f64)>,
    count: u8,
}

impl ClickClock {
    pub(crate) fn click(&mut self, time_ms: u32, x: f64, y: f64) -> u8 {
        let chained = self.last.is_some_and(|(t, lx, ly)| {
            time_ms.wrapping_sub(t) <= 400 && (x - lx).abs() <= 4.0 && (y - ly).abs() <= 4.0
        });
        self.count = if chained { self.count.saturating_add(1) } else { 1 };
        self.last = Some((time_ms, x, y));
        self.count
    }
}

/// The wheel between pointer frames: continuous values in surface px,
/// discrete detents when a real wheel turns. The flush prefers the
/// detents (the ×16 line doctrine all platforms share) and flips the
/// sign — wayland's positive is content-down, the engine's is up.
#[derive(Default)]
struct AxisAccumulator {
    vertical: f64,
    horizontal: f64,
    vertical_steps: i32,
    horizontal_steps: i32,
}

impl AxisAccumulator {
    fn axis(&mut self, axis: u32, value: f64) {
        match axis {
            0 => self.vertical += value,
            1 => self.horizontal += value,
            _ => {}
        }
    }

    fn discrete(&mut self, axis: u32, steps: i32) {
        match axis {
            0 => self.vertical_steps += steps,
            1 => self.horizontal_steps += steps,
            _ => {}
        }
    }

    fn flush(&mut self) -> Option<(f64, f64)> {
        let dy = if self.vertical_steps != 0 {
            -(self.vertical_steps as f64) * 16.0
        } else {
            -self.vertical
        };
        let dx = if self.horizontal_steps != 0 {
            -(self.horizontal_steps as f64) * 16.0
        } else {
            -self.horizontal
        };
        *self = AxisAccumulator::default();
        (dx != 0.0 || dy != 0.0).then_some((dx, dy))
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
    maximized: bool,
    /// Scene chrome: the shell owns the resize bands at the border.
    scene: bool,
}

struct CursorState {
    surface: *mut Proxy,
    theme: *mut c_void,
    theme_scale: usize,
    current: Cursor,
}

/// The keyboard: the compositor sends the keymap as text, xkbcommon
/// compiles it, and the shell asks the state questions. `scratch` is a
/// second state that never learns the modifiers — the chars-ignoring
/// road (the ToUnicode-zeroed twin).
pub(crate) struct Keyboard {
    pub(crate) context: *mut c_void,
    pub(crate) keymap: *mut c_void,
    pub(crate) state: *mut c_void,
    pub(crate) scratch: *mut c_void,
    pub(crate) compose: *mut c_void,
    repeat_rate: i32,
    repeat_delay: i32,
    /// The held repeating keycode and its generation — a bumped
    /// generation orphans any timer already scheduled (the
    /// ghost-repeat cure).
    held: Option<(u32, u64)>,
    generation: u64,
    /// A second press of the held keycode without a release means the
    /// compositor repeats for us — our timer stands down for the hold.
    compositor_repeats: bool,
}

impl Keyboard {
    pub(crate) fn new() -> Keyboard {
        Keyboard {
            context: std::ptr::null_mut(),
            keymap: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            scratch: std::ptr::null_mut(),
            compose: std::ptr::null_mut(),
            repeat_rate: 16,
            repeat_delay: 500,
            held: None,
            generation: 0,
            compositor_repeats: false,
        }
    }
}

/// Our claim on the selection: the source proxy and the text it serves.
struct SourceState {
    proxy: *mut Proxy,
    text: String,
}

/// The v3 composition cycle: events stage, `done` applies atomically.
/// The result lands BEFORE the fresh preedit — the order every
/// platform's IME phase learned the hard way.
#[derive(Default)]
struct ImeCycle {
    preedit: Option<(String, i32)>,
    commit: Option<String>,
}

enum ImeOp {
    Insert(String),
    Mark { text: String, caret_utf16: usize },
    Unmark,
}

impl ImeCycle {
    /// `marked` = a composition was live before this done.
    fn finish(&mut self, marked: bool) -> (Vec<ImeOp>, bool) {
        let mut ops = Vec::new();
        if let Some(text) = self.commit.take()
            && !text.is_empty()
        {
            ops.push(ImeOp::Insert(text));
        }
        match self.preedit.take() {
            Some((text, begin)) if !text.is_empty() => {
                let caret = utf16_index_at(&text, begin.max(0) as usize);
                ops.push(ImeOp::Mark { text, caret_utf16: caret });
                (ops, true)
            }
            _ => {
                // a done without a fresh preedit ends the marked run —
                // but only if one was live (post-commit must not fire)
                if marked {
                    ops.push(ImeOp::Unmark);
                }
                (ops, false)
            }
        }
    }
}

/// v3 speaks BYTE offsets into utf-8; the core resolvers speak utf-16.
fn utf16_index_at(text: &str, byte_offset: usize) -> usize {
    text.get(..byte_offset.min(text.len()))
        .map(|prefix| prefix.encode_utf16().count())
        .unwrap_or_else(|| text.encode_utf16().count())
}

struct ImeState {
    text_input: *mut Proxy,
    enabled: bool,
    marked: bool,
    cycle: ImeCycle,
    /// Our commit count vs the compositor's `done` echo — extra
    /// commits only flow while they agree (the loop breaker).
    commits: u32,
    done_serial: u32,
    last_rect: (i32, i32, i32, i32),
}

/// One overlay panel. A popover/tooltip/menu is an xdg_popup (it may
/// hang past the window's edge — the fidelity bar); the drag chip is a
/// subsurface (a mouse-following popup would be a recreate storm).
/// The protocol objects materialize at the first present, when the
/// position is known.
struct Panel {
    chip: bool,
    surface: *mut Proxy,
    xdg: *mut Proxy,
    popup: *mut Proxy,
    subsurface: *mut Proxy,
    backing: Option<Backing>,
    /// Premultiplied BGRA waiting for the popup's first configure.
    staged: Option<(usize, usize, Vec<u8>)>,
    scene_origin: (f64, f64),
    asked: (f64, f64),
    /// configured − asked: the compositor's adjustment, folded into
    /// event translation so hit-testing follows truth.
    delta: (f64, f64),
    configured: bool,
}

impl Panel {
    fn new(chip: bool) -> Panel {
        Panel {
            chip,
            surface: std::ptr::null_mut(),
            xdg: std::ptr::null_mut(),
            popup: std::ptr::null_mut(),
            subsurface: std::ptr::null_mut(),
            backing: None,
            staged: None,
            scene_origin: (0.0, 0.0),
            asked: (f64::NAN, f64::NAN),
            delta: (0.0, 0.0),
            configured: false,
        }
    }
}

struct Client {
    display: *mut Display,
    registry: *mut Proxy,
    compositor: *mut Proxy,
    shm: *mut Proxy,
    seat: *mut Proxy,
    pointer: *mut Proxy,
    keyboard_proxy: *mut Proxy,
    data_device: *mut Proxy,
    data_manager: *mut Proxy,
    wm_base: *mut Proxy,
    protocols: &'static Protocols,
    outputs: Vec<OutputInfo>,
    globals: Vec<(u32, String, u32)>,
    win: Option<Window>,
    serials: Serials,
    clicks: ClickClock,
    pointer_pos: (f64, f64),
    cursor: CursorState,
    keyboard: Keyboard,
    /// Live offers and their advertised mimes, keyed by proxy address.
    offers: HashMap<usize, Vec<String>>,
    /// The current selection's offer (0 = cleared).
    selection: usize,
    source: Option<SourceState>,
    wake_read: c_int,
    subcompositor: *mut Proxy,
    ime: ImeState,
    panels: Vec<Option<Panel>>,
    /// 0 = the main window; N = panel N−1 (event translation).
    pointer_focus: usize,
    /// The resize band under the pointer (0 = none) — it outranks the
    /// scene's cursor while it holds.
    edge_hover: u32,
    axis: AxisAccumulator,
    /// Counts presenting commits — the configure road checks whether
    /// an ack was followed by one.
    presents: u64,
    quit: bool,
}

thread_local! {
    static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) };
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
    static NEXT_BLINK: Cell<Option<Instant>> = const { Cell::new(None) };
    static NEXT_REPEAT: Cell<Option<Instant>> = const { Cell::new(None) };
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
    Wake,
    ResignKey,
    MouseMoved { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, clicks: u8, shift: bool },
    MouseUp { x: f64, y: f64 },
    RightMouseDown { x: f64, y: f64 },
    MouseExited,
    /// Typing, paste of characters, and the composed dead-key result —
    /// the same road for all of them.
    Text(String),
    /// An editing key that passed the gate unconsumed.
    Key { sym: u32, shift: bool, command: bool },
    /// A press landed outside every open overlay — the x11 door has no
    /// compositor grab to say `popup_done`, so it says this instead.
    DismissOverlays,
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    ImeMark { text: String, caret: usize },
    ImeUnmark,
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

    // the devices: capability events are racy at bind, the census is
    // not — WSLg and every desktop advertise pointer+keyboard up front
    let (pointer, keyboard_proxy) = if seat.is_null() {
        (std::ptr::null_mut(), std::ptr::null_mut())
    } else {
        unsafe {
            (
                construct(seat, 0, &raw const wl_pointer_interface, &mut [arg_n()], TAG_POINTER),
                construct(seat, 1, &raw const wl_keyboard_interface, &mut [arg_n()], TAG_KEYBOARD),
            )
        }
    };

    let data_manager = bind(
        c"wl_data_device_manager",
        &raw const wl_data_device_manager_interface,
        3,
        TAG_SYNC,
    );
    let subcompositor =
        bind(c"wl_subcompositor", &raw const wl_subcompositor_interface, 1, TAG_SYNC);
    // text-input v3 where the compositor speaks it (WSLg's does not —
    // the road stays inert and typing flows untouched)
    let ti_manager =
        bind(c"zwp_text_input_manager_v3", protocols.ti_manager as *const WlInterface, 1, TAG_SYNC);
    let text_input = if ti_manager.is_null() || seat.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe {
            construct(
                ti_manager,
                1, // get_text_input(new, seat)
                protocols.text_input as *const WlInterface,
                &mut [arg_n(), arg_o(seat)],
                TAG_TEXT_INPUT,
            )
        }
    };
    let data_device = if data_manager.is_null() || seat.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe {
            construct(
                data_manager,
                1, // get_data_device(new id, seat)
                &raw const wl_data_device_interface,
                &mut [arg_n(), arg_o(seat)],
                TAG_DATA_DEVICE,
            )
        }
    };

    let cursor_surface =
        unsafe { construct(compositor, 0, &raw const wl_surface_interface, &mut [arg_n()], TAG_CURSOR_SURFACE) };

    // the wake pipe: any thread writes a byte, the poll loop turns it
    // into one more frame of the pump
    let mut pipe_fds = [0 as c_int; 2];
    let wake_read = if unsafe { pipe2(pipe_fds.as_mut_ptr(), O_CLOEXEC | O_NONBLOCK) } == 0 {
        WAKE_WRITE_FD.store(pipe_fds[1], std::sync::atomic::Ordering::Release);
        pipe_fds[0]
    } else {
        -1
    };

    let mut keyboard = Keyboard::new();
    unsafe {
        keyboard.context = xkb_context_new(0);
        if !keyboard.context.is_null() {
            // the locale drives the dead-key table; empty falls to C
            let locale = std::env::var("LC_ALL")
                .or_else(|_| std::env::var("LC_CTYPE"))
                .or_else(|_| std::env::var("LANG"))
                .unwrap_or_else(|_| "C".into());
            if let Ok(locale_c) = CString::new(locale) {
                let table = xkb_compose_table_new_from_locale(keyboard.context, locale_c.as_ptr(), 0);
                if !table.is_null() {
                    keyboard.compose = xkb_compose_state_new(table, 0);
                    xkb_compose_table_unref(table);
                }
            }
        }
    }

    CLIENT.with(|slot| {
        *slot.borrow_mut() = Some(Client {
            display,
            registry,
            compositor,
            shm,
            seat,
            pointer,
            keyboard_proxy,
            data_device,
            data_manager,
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
            keyboard,
            offers: HashMap::new(),
            selection: 0,
            source: None,
            wake_read,
            subcompositor,
            ime: ImeState {
                text_input,
                enabled: false,
                marked: false,
                cycle: ImeCycle::default(),
                commits: 0,
                done_serial: 0,
                last_rect: (0, 0, 0, 0),
            },
            panels: Vec::new(),
            pointer_focus: 0,
            edge_hover: 0,
            axis: AxisAccumulator::default(),
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
pub fn create_window(title: &str, width: f64, height: f64, scene_chrome: bool) -> WindowHandle {
    if is_x11() {
        crate::x11::create_window(title, width, height, scene_chrome);
        return WindowHandle(0);
    }
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
            maximized: false,
            scene: scene_chrome,
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
    WindowHandle(0)
}

/// On wayland a window appears when its first buffer commits — the
/// first present IS the reveal, so the anti-flash order holds by
/// protocol design and this is a no-op on that door. The x11 door
/// maps here, AFTER the first present landed in the backing.
pub fn show_window(_window: WindowHandle) {
    if is_x11() {
        crate::x11::show_window();
    }
}

/// 0 is the main window; N is panel N−1. The identity lives in the
/// client state, the handle is the twins' shape.
#[derive(Clone, Copy)]
pub struct WindowHandle(usize);

impl WindowHandle {
    /// Logical size of the content area (the layout viewport).
    pub fn content_size(&self) -> (f64, f64) {
        if is_x11() {
            return crate::x11::content_size();
        }
        with_client(|client| client.win.as_ref().map(|w| w.logical).unwrap_or((0.0, 0.0)))
    }

    /// The integer raster scale the engine sees.
    pub fn scale(&self) -> usize {
        if is_x11() {
            return crate::x11::scale();
        }
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
        if is_x11() {
            return crate::x11::present_rows(width, height, rgba, damage);
        }
        present_rows(width, height, rgba, damage);
    }

    pub fn set_cursor(&self, cursor: Cursor) {
        if is_x11() {
            return crate::x11::set_cursor(cursor);
        }
        let changed = with_client(|client| {
            let previous = client.cursor.current;
            client.cursor.current = cursor;
            previous != cursor
        });
        if changed {
            apply_cursor();
        }
    }

    /// Layout coordinates ARE the wayland door's positioning currency
    /// (it knows no screen), so that conversion is identity. The x11
    /// door has REAL screen coordinates: the window's root origin is
    /// the base, and panels land absolutely.
    pub fn layout_rect_to_screen(&self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
        if is_x11() {
            let origin = crate::x11::window_origin_logical();
            return (origin.0 + x, origin.1 + y, w, h);
        }
        (x, y, w, h)
    }

    /// The work area the placement math clamps against. Wayland has no
    /// screen geometry, so the window's bounds inflate by a generous
    /// margin: core places freely (popovers may hang past the edge —
    /// the fidelity bar) and only pathological overflow is reined in.
    /// The x11 door answers with the REAL root bounds instead.
    pub fn screen_bounds_in_layout(&self) -> Option<(f64, f64, f64, f64)> {
        if is_x11() {
            return crate::x11::screen_bounds_in_layout();
        }
        const MARGIN: f64 = 512.0;
        let (w, h) = self.content_size();
        Some((-MARGIN, -MARGIN, w + 2.0 * MARGIN, h + 2.0 * MARGIN))
    }

    /// The panel's identity in the scene: the overlay's layout origin,
    /// the base for translating its surface-local pointer events.
    pub fn set_scene_origin(&self, x: f64, y: f64) {
        if self.0 == 0 {
            return;
        }
        if is_x11() {
            return crate::x11::set_scene_origin(self.0 - 1, x, y);
        }
        with_client(|client| {
            if let Some(Some(panel)) = client.panels.get_mut(self.0 - 1) {
                panel.scene_origin = (x, y);
            }
        });
    }

    /// Position, size and pixels land together: the straight RGBA
    /// slice premultiplies into ARGB8888 on the way in (the layered
    /// twin's fused pass), the popup materializes lazily and re-homes
    /// by recreation when the placement moved.
    pub fn present_layered(
        &self,
        rect: (f64, f64, f64, f64),
        width: usize,
        height: usize,
        rgba: &[u8],
    ) {
        if self.0 == 0 {
            return;
        }
        if is_x11() {
            return crate::x11::panel_present(self.0 - 1, rect, width, height, rgba);
        }
        panel_present(self.0 - 1, rect, width, height, rgba);
    }

    /// Hide and forget: the pool retires a panel whose overlay closed.
    pub fn close_panel(&self) {
        if self.0 == 0 {
            return;
        }
        if is_x11() {
            return crate::x11::close_panel(self.0 - 1);
        }
        with_client(|client| {
            let index = self.0 - 1;
            if let Some(slot) = client.panels.get_mut(index) {
                if let Some(panel) = slot.take() {
                    unsafe { teardown_panel(panel) };
                }
            }
            if client.pointer_focus == self.0 {
                client.pointer_focus = 0;
            }
        });
    }
}

/// The pool asks for a panel slot; the protocol objects wait for the
/// first present, when the placement is known. `chip` picks the
/// subsurface road (the mouse-following drag label).
pub fn create_panel(_window: &WindowHandle, chip: bool) -> WindowHandle {
    if is_x11() {
        return WindowHandle(crate::x11::create_panel(chip));
    }
    with_client(|client| {
        client.panels.push(Some(Panel::new(chip)));
        WindowHandle(client.panels.len())
    })
}

/// Anchors: top-left of the anchor rect; gravity: down-right — the
/// popup sits exactly where core placed it. Constraint adjustment is
/// NONE on purpose: core is the placement authority and a popover may
/// hang past every edge.
const XDG_ANCHOR_TOP_LEFT: u32 = 5;
const XDG_GRAVITY_BOTTOM_RIGHT: u32 = 8;

unsafe fn teardown_panel(panel: Panel) {
    unsafe {
        if !panel.popup.is_null() {
            destroy(panel.popup, 0);
        }
        if !panel.xdg.is_null() {
            destroy(panel.xdg, 0);
        }
        if !panel.subsurface.is_null() {
            destroy(panel.subsurface, 0);
        }
        if let Some(backing) = panel.backing {
            destroy(backing.buffer, 0);
            destroy(backing.pool, 1);
            munmap(backing.map as *mut c_void, backing.len);
            close(backing.fd);
        }
        if !panel.surface.is_null() {
            destroy(panel.surface, 0);
        }
    }
}

fn panel_present(index: usize, rect: (f64, f64, f64, f64), width: usize, height: usize, rgba: &[u8]) {
    let (x, y, w, h) = rect;
    with_client(|client| {
        let (parent_surface, parent_xdg, parent_logical, scale) = match client.win.as_ref() {
            Some(win) if win.map.can_attach() => {
                (win.surface, win.xdg_surface, win.logical, win.scale)
            }
            _ => return,
        };
        let wm_base = client.wm_base;
        let compositor = client.compositor;
        let subcompositor = client.subcompositor;
        let protocols = client.protocols;
        let Some(Some(panel)) = client.panels.get_mut(index) else { return };
        // a moved popup cannot re-anchor below reposition v3 — it is
        // reborn at the new place (moves are rare: a reopened popover)
        let moved = !panel.chip
            && !panel.surface.is_null()
            && ((panel.asked.0 - x).abs() > 0.5 || (panel.asked.1 - y).abs() > 0.5);
        if moved {
            let dead = std::mem::replace(panel, Panel::new(false));
            unsafe { teardown_panel(dead) };
        }
        if panel.surface.is_null() {
            unsafe {
                let tag = PANEL_TAG_BASE + (index << 2);
                let surface = construct(
                    compositor,
                    0,
                    &raw const wl_surface_interface,
                    &mut [arg_n()],
                    tag + PANEL_KIND_SURFACE,
                );
                if panel.chip && !subcompositor.is_null() {
                    let subsurface = construct(
                        subcompositor,
                        1, // get_subsurface(new, surface, parent)
                        &raw const wl_subsurface_interface,
                        &mut [arg_n(), arg_o(surface), arg_o(parent_surface)],
                        TAG_SYNC,
                    );
                    request(subsurface, 5, &mut no_args()); // set_desync
                    request(subsurface, 2, &mut [arg_o(parent_surface)]); // place_above
                    request(
                        subsurface,
                        1,
                        &mut [arg_i(x.round() as i32), arg_i(y.round() as i32)],
                    );
                    panel.subsurface = subsurface;
                    panel.configured = true; // subsurfaces know no configure
                } else {
                    let xdg = construct(
                        wm_base,
                        2,
                        protocols.xdg_surface as *const WlInterface,
                        &mut [arg_n(), arg_o(surface)],
                        tag + PANEL_KIND_XDG,
                    );
                    let positioner = construct(
                        wm_base,
                        1,
                        protocols.positioner as *const WlInterface,
                        &mut [arg_n()],
                        TAG_SYNC,
                    );
                    request(
                        positioner,
                        1, // set_size
                        &mut [arg_i(w.ceil().max(1.0) as i32), arg_i(h.ceil().max(1.0) as i32)],
                    );
                    // the anchor rect must sit INSIDE the parent's
                    // geometry; the offset carries the true position
                    let ax = x.clamp(0.0, (parent_logical.0 - 1.0).max(0.0));
                    let ay = y.clamp(0.0, (parent_logical.1 - 1.0).max(0.0));
                    request(
                        positioner,
                        2, // set_anchor_rect
                        &mut [arg_i(ax as i32), arg_i(ay as i32), arg_i(1), arg_i(1)],
                    );
                    request(positioner, 3, &mut [arg_u(XDG_ANCHOR_TOP_LEFT)]);
                    request(positioner, 4, &mut [arg_u(XDG_GRAVITY_BOTTOM_RIGHT)]);
                    request(positioner, 5, &mut [arg_u(0)]); // no constraint adjustment
                    request(
                        positioner,
                        6, // set_offset
                        &mut [arg_i((x - ax).round() as i32), arg_i((y - ay).round() as i32)],
                    );
                    let popup = construct(
                        xdg,
                        2, // get_popup(new, parent, positioner)
                        protocols.popup as *const WlInterface,
                        &mut [arg_n(), arg_o(parent_xdg), arg_o(positioner)],
                        tag + PANEL_KIND_POPUP,
                    );
                    destroy(positioner, 0);
                    panel.xdg = xdg;
                    panel.popup = popup;
                    panel.configured = false;
                    // the popup's map dance: an empty commit asks for
                    // the first configure; the pixels wait staged
                    request(surface, 6, &mut no_args());
                }
                panel.surface = surface;
                panel.asked = (x, y);
                panel.delta = (0.0, 0.0);
            }
        } else if panel.chip {
            let position_moved =
                (panel.asked.0 - x).abs() > 0.5 || (panel.asked.1 - y).abs() > 0.5;
            if position_moved && !panel.subsurface.is_null() {
                unsafe {
                    request(
                        panel.subsurface,
                        1,
                        &mut [arg_i(x.round() as i32), arg_i(y.round() as i32)],
                    );
                }
                // double-buffered against the PARENT: the drag's own
                // repaint of the main window applies it
                panel.asked = (x, y);
            }
        }
        // the fused pass: straight RGBA → premultiplied BGRA
        let mut bytes = vec![0u8; width * height * 4];
        for (source, target) in rgba.chunks_exact(4).zip(bytes.chunks_exact_mut(4)) {
            let alpha = source[3] as u32;
            target[0] = ((source[2] as u32 * alpha + 127) / 255) as u8;
            target[1] = ((source[1] as u32 * alpha + 127) / 255) as u8;
            target[2] = ((source[0] as u32 * alpha + 127) / 255) as u8;
            target[3] = alpha as u8;
        }
        if !panel.configured {
            panel.staged = Some((width, height, bytes));
            unsafe { wl_display_flush(client.display) };
            return;
        }
        unsafe {
            flush_panel_pixels(client.shm, panel, width, height, &bytes, scale);
            wl_display_flush(client.display);
        }
    });
}

/// Writes the premultiplied pixels into the panel's shm and commits.
unsafe fn flush_panel_pixels(
    shm: *mut Proxy,
    panel: &mut Panel,
    width: usize,
    height: usize,
    bytes: &[u8],
    scale: usize,
) {
    unsafe {
        let stale = panel
            .backing
            .as_ref()
            .is_none_or(|backing| backing.width != width || backing.height != height);
        if stale {
            if let Some(old) = panel.backing.take() {
                destroy(old.buffer, 0);
                destroy(old.pool, 1);
                munmap(old.map as *mut c_void, old.len);
                close(old.fd);
            }
            const ARGB8888: u32 = 0;
            panel.backing = make_backing(shm, width, height, ARGB8888, TAG_SYNC);
        }
        let Some(backing) = panel.backing.as_mut() else { return };
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), backing.map, bytes.len().min(backing.len));
        let surface_version = wl_proxy_get_version(panel.surface);
        if scale > 1 && surface_version >= 3 {
            request(panel.surface, 8, &mut [arg_i(scale as i32)]);
        }
        request(panel.surface, 1, &mut [arg_o(backing.buffer), arg_i(0), arg_i(0)]);
        if surface_version >= 4 {
            request(
                panel.surface,
                9,
                &mut [arg_i(0), arg_i(0), arg_i(width as i32), arg_i(height as i32)],
            );
        } else {
            request(panel.surface, 2, &mut [arg_i(0), arg_i(0), arg_i(i32::MAX), arg_i(i32::MAX)]);
        }
        request(panel.surface, 6, &mut no_args());
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
    const XRGB8888: u32 = 1;
    const ARGB8888: u32 = 0;
    // a scene-chrome window carries alpha: its corners round by mask
    let format = if win.scene { ARGB8888 } else { XRGB8888 };
    win.backing = make_backing(client.shm, width, height, format, TAG_BUFFER);
    win.backing.is_some()
}

/// The rounded corners the platforms give their windows — here the
/// shell earns them: an anti-aliased quarter-circle mask over each
/// corner, premultiplied because the surface carries real alpha.
/// Runs only over the corner boxes a damage rect touched.
pub(crate) fn mask_corners(map: *mut u8, width: usize, height: usize, radius: f64) {
    let r = radius.min(width as f64 / 2.0).min(height as f64 / 2.0);
    let span = r.ceil() as usize;
    // center and the OUTWARD signs per corner: a pixel rounds only
    // when it sits beyond the center toward its corner
    let corners = [
        (r - 0.5, r - 0.5, -1.0, -1.0, 0, 0),
        (width as f64 - r - 0.5, r - 0.5, 1.0, -1.0, width - span, 0),
        (r - 0.5, height as f64 - r - 0.5, -1.0, 1.0, 0, height - span),
        (width as f64 - r - 0.5, height as f64 - r - 0.5, 1.0, 1.0, width - span, height - span),
    ];
    for (cx, cy, sx, sy, x0, y0) in corners {
        for y in y0..(y0 + span).min(height) {
            for x in x0..(x0 + span).min(width) {
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                if dx * sx <= 0.0 || dy * sy <= 0.0 {
                    continue;
                }
                let coverage = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                let alpha = (coverage * 255.0).round() as u32;
                if alpha == 255 {
                    continue;
                }
                unsafe {
                    let px = map.add((y * width + x) * 4);
                    *px = (*px as u32 * alpha / 255) as u8;
                    *px.add(1) = (*px.add(1) as u32 * alpha / 255) as u8;
                    *px.add(2) = (*px.add(2) as u32 * alpha / 255) as u8;
                    *px.add(3) = alpha as u8;
                }
            }
        }
    }
}

/// One shm pool, one buffer: the whole backing story. The main window
/// rides XRGB (opaque, release-tracked); panels ride ARGB
/// (premultiplied, rewritten whole each present).
fn make_backing(
    shm: *mut Proxy,
    width: usize,
    height: usize,
    format: u32,
    buffer_tag: usize,
) -> Option<Backing> {
    let len = width * height * 4;
    if len == 0 {
        return None;
    }
    let fd = unsafe { memfd_create(c"bunny-shm".as_ptr(), MFD_CLOEXEC) };
    if fd < 0 || unsafe { ftruncate(fd, len as i64) } != 0 {
        if fd >= 0 {
            unsafe { close(fd) };
        }
        eprintln!("bunny_ui_linux: shm backing failed");
        return None;
    }
    let map = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0) };
    if map as isize == -1 {
        unsafe { close(fd) };
        eprintln!("bunny_ui_linux: shm map failed");
        return None;
    }
    unsafe {
        let pool = construct(
            shm,
            0,
            &raw const wl_shm_pool_interface,
            &mut [arg_n(), arg_h(fd), arg_i(len as i32)],
            TAG_SYNC,
        );
        let buffer = construct(
            pool,
            0,
            &raw const wl_buffer_interface,
            &mut [
                arg_n(),
                arg_i(0),
                arg_i(width as i32),
                arg_i(height as i32),
                arg_i((width * 4) as i32),
                arg_u(format),
            ],
            buffer_tag,
        );
        Some(Backing { pool, buffer, map: map as *mut u8, len, width, height, fd, released: true })
    }
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
        if win.scene {
            mask_corners(backing.map, width, height, 8.0 * win.scale as f64);
        }
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
    ResizeUpDown,
    ResizeNwSe,
    ResizeNeSw,
}

/// The border band's cursor for an xdg resize-edge bitfield.
pub(crate) fn edge_cursor(edge: u32) -> Cursor {
    match edge {
        5 | 10 => Cursor::ResizeNwSe,
        6 | 9 => Cursor::ResizeNeSw,
        1 | 2 => Cursor::ResizeUpDown,
        _ => Cursor::ResizeLeftRight,
    }
}

/// Theme names are inconsistent across the ecosystem; each style
/// carries an ordered chain and the first hit wins.
fn cursor_names(cursor: Cursor) -> &'static [&'static CStr] {
    match cursor {
        Cursor::Arrow => &[c"default", c"left_ptr", c"arrow"],
        Cursor::Pointing => &[c"pointer", c"hand2", c"hand1", c"pointing_hand"],
        Cursor::ResizeLeftRight => &[c"ew-resize", c"sb_h_double_arrow", c"size_hor", c"col-resize"],
        Cursor::ResizeUpDown => &[c"ns-resize", c"sb_v_double_arrow", c"size_ver", c"row-resize"],
        Cursor::ResizeNwSe => &[c"nwse-resize", c"size_fdiag", c"bd_double_arrow"],
        Cursor::ResizeNeSw => &[c"nesw-resize", c"size_bdiag", c"fd_double_arrow"],
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
        // a border band outranks the scene's own cursor
        let effective = if client.edge_hover != 0 {
            edge_cursor(client.edge_hover)
        } else {
            client.cursor.current
        };
        let cursor = cursor_names(effective)
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

// MARK: - the keyboard road (gate → edit keys → text)

/// What one key press looks like at the gate. `control` carries Ctrl
/// (the accelerator — it maps to `command` in the keymap vocabulary),
/// `alt` carries Mod1. `chars_ignoring` is the base character from a
/// modifier-clean state; `types_text` marks an AltGr chord that TYPES
/// (level-3 text is never a binding).
pub struct KeyStroke {
    pub sym: u32,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub chars_ignoring: String,
    pub types_text: bool,
}

thread_local! {
    static KEY_GATE: RefCell<Option<Box<dyn FnMut(&KeyStroke) -> bool>>> =
        const { RefCell::new(None) };
}

pub fn set_key_gate(gate: Box<dyn FnMut(&KeyStroke) -> bool>) {
    KEY_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

/// What one press resolved to, computed under the client borrow and
/// acted on outside it.
pub(crate) enum KeyRoad {
    Silence,
    Composed(String),
    Stroke(KeyStroke, String),
}

/// The xkb walk for one PRESSED key: compose first (dead keys), then
/// Is shift held right now? The one modifier a PRESS carries — over a
/// field it extends the selection instead of replacing it.
fn shift_held(keyboard: &Keyboard) -> bool {
    if keyboard.state.is_null() {
        return false;
    }
    unsafe {
        xkb_state_mod_name_is_active(keyboard.state, c"Shift".as_ptr(), XKB_STATE_MODS_EFFECTIVE)
            == 1
    }
}

/// the stroke with both texts. `keycode` is already evdev+8 (the x11
/// server's own keycodes live on the same lattice).
pub(crate) fn key_road(keyboard: &mut Keyboard, keycode: u32) -> KeyRoad {
    if keyboard.state.is_null() {
        return KeyRoad::Silence;
    }
    unsafe {
        let sym = xkb_state_key_get_one_sym(keyboard.state, keycode);
        if !keyboard.compose.is_null() && xkb_compose_state_feed(keyboard.compose, sym) == 1 {
            match xkb_compose_state_get_status(keyboard.compose) {
                XKB_COMPOSE_COMPOSING => return KeyRoad::Silence,
                XKB_COMPOSE_COMPOSED => {
                    let mut buffer = [0 as c_char; 64];
                    let n = xkb_compose_state_get_utf8(
                        keyboard.compose,
                        buffer.as_mut_ptr(),
                        buffer.len(),
                    );
                    xkb_compose_state_reset(keyboard.compose);
                    let text = utf8_of(&buffer, n);
                    return if text.is_empty() { KeyRoad::Silence } else { KeyRoad::Composed(text) };
                }
                XKB_COMPOSE_CANCELLED => {
                    xkb_compose_state_reset(keyboard.compose);
                    return KeyRoad::Silence;
                }
                _ => {}
            }
        }
        let mut buffer = [0 as c_char; 64];
        let n = xkb_state_key_get_utf8(keyboard.state, keycode, buffer.as_mut_ptr(), buffer.len());
        let text = utf8_of(&buffer, n);
        let mut ignoring = [0 as c_char; 64];
        let n = xkb_state_key_get_utf8(
            keyboard.scratch,
            keycode,
            ignoring.as_mut_ptr(),
            ignoring.len(),
        );
        let chars_ignoring = utf8_of(&ignoring, n);
        let active = |name: &CStr| {
            xkb_state_mod_name_is_active(keyboard.state, name.as_ptr(), XKB_STATE_MODS_EFFECTIVE)
                == 1
        };
        let printable = !text.is_empty() && !text.chars().any(|ch| ch.is_control());
        // the AltGr rule: a level-3 chord that types IS text — it
        // skips the gate so the binding never steals the character
        let types_text = printable && active(c"Mod5");
        KeyRoad::Stroke(
            KeyStroke {
                sym,
                shift: active(c"Shift"),
                control: active(c"Control"),
                alt: active(c"Mod1"),
                chars_ignoring,
                types_text,
            },
            text,
        )
    }
}

fn utf8_of(buffer: &[c_char], written: c_int) -> String {
    if written <= 0 {
        return String::new();
    }
    let bytes: Vec<u8> =
        buffer[..(written as usize).min(buffer.len())].iter().map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The editing keys the shell forwards when the gate declines: the
/// EditCommand set plus the Ctrl accelerator quartet.
fn is_edit_key(stroke: &KeyStroke) -> bool {
    matches!(stroke.sym, 0xff08 | 0xffff | 0xff51 | 0xff53 | 0xff50 | 0xff57 | 0xff1b)
        || (stroke.control && matches!(stroke.sym, 0x61 | 0x63 | 0x78 | 0x76))
}

/// One pressed (or repeated) key walks the whole road: gate first,
/// then the editing keys, then the character road. Runs OUTSIDE the
/// client borrow.
pub(crate) fn deliver_key(road: KeyRoad) {
    // step one of the gate: a live composition wins outright
    if ime_marked() {
        return;
    }
    match road {
        KeyRoad::Silence => {}
        KeyRoad::Composed(text) => dispatch(AppEvent::Text(text)),
        KeyRoad::Stroke(stroke, text) => {
            let consumed = KEY_GATE.with(|slot| {
                slot.borrow_mut().as_mut().is_some_and(|gate| gate(&stroke))
            });
            if consumed {
                return;
            }
            if is_edit_key(&stroke) {
                dispatch(AppEvent::Key {
                    sym: stroke.sym,
                    shift: stroke.shift,
                    command: stroke.control,
                });
            } else if !text.is_empty()
                && !text.chars().any(|ch| ch.is_control())
                && (stroke.types_text || (!stroke.control && !stroke.alt))
            {
                dispatch(AppEvent::Text(text));
            }
        }
    }
}

// MARK: - the season's mirrors (the desktop portal over libdbus)

#[link(name = "dbus-1")]
unsafe extern "C" {
    fn dbus_bus_get_private(kind: c_int, error: *mut DbusError) -> *mut c_void;
    fn dbus_connection_close(connection: *mut c_void);
    fn dbus_connection_unref(connection: *mut c_void);
    fn dbus_error_init(error: *mut DbusError);
    fn dbus_error_free(error: *mut DbusError);
    fn dbus_message_new_method_call(
        destination: *const c_char,
        path: *const c_char,
        interface: *const c_char,
        method: *const c_char,
    ) -> *mut c_void;
    fn dbus_message_unref(message: *mut c_void);
    fn dbus_message_iter_init_append(message: *mut c_void, iter: *mut DbusIter);
    fn dbus_message_iter_append_basic(
        iter: *mut DbusIter,
        kind: c_int,
        value: *const c_void,
    ) -> c_int;
    fn dbus_connection_send_with_reply_and_block(
        connection: *mut c_void,
        message: *mut c_void,
        timeout_ms: c_int,
        error: *mut DbusError,
    ) -> *mut c_void;
    fn dbus_message_iter_init(message: *mut c_void, iter: *mut DbusIter) -> c_int;
    fn dbus_message_iter_recurse(iter: *mut DbusIter, sub: *mut DbusIter);
    fn dbus_message_iter_get_arg_type(iter: *mut DbusIter) -> c_int;
    fn dbus_message_iter_get_basic(iter: *mut DbusIter, value: *mut c_void);
}

#[repr(C)]
struct DbusError {
    name: *const c_char,
    message: *const c_char,
    dummy: [c_uint; 2],
    padding: *mut c_void,
}

/// libdbus asks callers for 16 pointers of iterator space; the layout
/// is opaque by contract.
#[repr(C)]
struct DbusIter {
    opaque: [usize; 16],
}

const DBUS_TYPE_STRING: c_int = 115; // 's'
const DBUS_TYPE_VARIANT: c_int = 118; // 'v'
const DBUS_TYPE_UINT32: c_int = 117; // 'u'
const DBUS_TYPE_BOOLEAN: c_int = 98; // 'b'
const DBUS_BUS_SESSION: c_int = 0;

/// One portal Settings.Read: connect, ask, unwrap the nested variant,
/// close. A missing portal (this compositor runs none) answers `None`
/// inside the timeout and the mirrors keep their defaults.
fn portal_read(namespace: &CStr, key: &CStr) -> Option<(c_int, u64)> {
    unsafe {
        let mut error = DbusError {
            name: std::ptr::null(),
            message: std::ptr::null(),
            dummy: [0; 2],
            padding: std::ptr::null_mut(),
        };
        dbus_error_init(&mut error);
        let connection = dbus_bus_get_private(DBUS_BUS_SESSION, &mut error);
        if connection.is_null() {
            dbus_error_free(&mut error);
            return None;
        }
        let message = dbus_message_new_method_call(
            c"org.freedesktop.portal.Desktop".as_ptr(),
            c"/org/freedesktop/portal/desktop".as_ptr(),
            c"org.freedesktop.portal.Settings".as_ptr(),
            c"Read".as_ptr(),
        );
        let mut iter = DbusIter { opaque: [0; 16] };
        dbus_message_iter_init_append(message, &mut iter);
        let namespace_ptr = namespace.as_ptr();
        let key_ptr = key.as_ptr();
        dbus_message_iter_append_basic(&mut iter, DBUS_TYPE_STRING, (&raw const namespace_ptr).cast());
        dbus_message_iter_append_basic(&mut iter, DBUS_TYPE_STRING, (&raw const key_ptr).cast());
        let reply = dbus_connection_send_with_reply_and_block(connection, message, 250, &mut error);
        dbus_message_unref(message);
        let value = (!reply.is_null()).then(|| {
            let mut top = DbusIter { opaque: [0; 16] };
            let mut inner = DbusIter { opaque: [0; 16] };
            let mut value = 0u64;
            if dbus_message_iter_init(reply, &mut top) == 0 {
                return None;
            }
            // Read answers Variant(Variant(actual)) — unwrap until flat
            let mut kind = dbus_message_iter_get_arg_type(&mut top);
            let cursor = &mut top;
            while kind == DBUS_TYPE_VARIANT {
                dbus_message_iter_recurse(cursor, &mut inner);
                std::mem::swap(cursor, &mut inner);
                kind = dbus_message_iter_get_arg_type(cursor);
            }
            dbus_message_iter_get_basic(cursor, (&raw mut value).cast());
            Some((kind, value))
        });
        if !reply.is_null() {
            dbus_message_unref(reply);
        }
        dbus_error_free(&mut error);
        dbus_connection_close(connection);
        dbus_connection_unref(connection);
        value.flatten()
    }
}

/// The standardized appearance key: 1 = dark, 2 = light, 0 = no say.
pub fn os_prefers_dark() -> Option<bool> {
    let (kind, value) = portal_read(c"org.freedesktop.appearance", c"color-scheme")?;
    (kind == DBUS_TYPE_UINT32).then(|| color_scheme_wants_dark(value as u32)).flatten()
}

fn color_scheme_wants_dark(scheme: u32) -> Option<bool> {
    match scheme {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Best-effort: the gnome namespace answers where it exists; silence
/// means animations stay on (accessibility never defaults to less).
pub fn animations_enabled() -> bool {
    match portal_read(c"org.gnome.desktop.interface", c"enable-animations") {
        Some((kind, value)) if kind == DBUS_TYPE_BOOLEAN => value != 0,
        _ => true,
    }
}

// MARK: - the crown (drag regions, window controls, the system menu)

/// The window's own buttons, answered by the scene's semantic marks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlHit {
    Close,
    Minimize,
    Maximize,
}

type DragGate = Box<dyn Fn(f64, f64) -> bool>;
type ControlGate = Box<dyn Fn(f64, f64) -> Option<ControlHit>>;

thread_local! {
    static CHROME_GATES: RefCell<Option<(DragGate, ControlGate)>> = const { RefCell::new(None) };
}

pub fn set_chrome_gates(drag: DragGate, control: ControlGate) {
    CHROME_GATES.with(|slot| *slot.borrow_mut() = Some((drag, control)));
}

/// What a main-window left press resolved to before the scene sees it.
pub(crate) enum CrownTake {
    None,
    Move,
    Menu,
    Control(ControlHit),
    ToggleMaximize,
    Resize(u32),
}

/// The xdg resize-edge bitfield for a point near the border — a
/// six-point band on every side of a scene-chrome window. Zero means
/// the interior. (The x11 door speaks the same bitfield and folds it
/// to EWMH directions at the send.)
pub(crate) fn resize_edge_of(x: f64, y: f64, width: f64, height: f64) -> u32 {
    const BAND: f64 = 6.0;
    let mut edge = 0;
    if y < BAND {
        edge |= 1; // top
    } else if y > height - BAND {
        edge |= 2; // bottom
    }
    if x < BAND {
        edge |= 4; // left
    } else if x > width - BAND {
        edge |= 8; // right
    }
    edge
}

pub(crate) fn crown_take(x: f64, y: f64, clicks: u8, right: bool) -> CrownTake {
    CHROME_GATES.with(|slot| {
        let gates = slot.borrow();
        let Some((drag, control)) = gates.as_ref() else { return CrownTake::None };
        if !right && let Some(hit) = control(x, y) {
            return CrownTake::Control(hit);
        }
        if drag(x, y) {
            if right {
                return CrownTake::Menu;
            }
            if clicks >= 2 {
                return CrownTake::ToggleMaximize;
            }
            return CrownTake::Move;
        }
        CrownTake::None
    })
}

/// Executes a crown verb against the toplevel with the press serial.
fn crown_execute(take: CrownTake, x: f64, y: f64) -> bool {
    with_client(|client| {
        let seat = client.seat;
        let serial = client.serials.press;
        let Some(win) = client.win.as_ref() else { return false };
        if seat.is_null() {
            return false;
        }
        unsafe {
            match take {
                CrownTake::None => false,
                CrownTake::Move => {
                    request(win.toplevel, 5, &mut [arg_o(seat), arg_u(serial)]);
                    wl_display_flush(client.display);
                    true
                }
                CrownTake::Menu => {
                    request(
                        win.toplevel,
                        4,
                        &mut [arg_o(seat), arg_u(serial), arg_i(x as i32), arg_i(y as i32)],
                    );
                    wl_display_flush(client.display);
                    true
                }
                CrownTake::ToggleMaximize => {
                    request(win.toplevel, if win.maximized { 10 } else { 9 }, &mut no_args());
                    wl_display_flush(client.display);
                    true
                }
                CrownTake::Resize(edge) => {
                    request(win.toplevel, 6, &mut [arg_o(seat), arg_u(serial), arg_u(edge)]);
                    wl_display_flush(client.display);
                    true
                }
                CrownTake::Control(hit) => {
                    match hit {
                        ControlHit::Close => client.quit = true,
                        ControlHit::Minimize => request(win.toplevel, 13, &mut no_args()),
                        ControlHit::Maximize => {
                            request(
                                win.toplevel,
                                if win.maximized { 10 } else { 9 },
                                &mut no_args(),
                            );
                        }
                    }
                    wl_display_flush(client.display);
                    true
                }
            }
        }
    })
}

// MARK: - clipboard (the selection, both directions, never blocking)

const SELF_MIME_PREFIX: &str = "pid/";
const TEXT_MIMES: [&CStr; 3] = [c"text/plain;charset=utf-8", c"UTF8_STRING", c"text/plain"];

/// Claims the selection with a fresh data source serving `text`.
pub fn clipboard_write(text: &str) {
    if is_x11() {
        return crate::x11::clipboard_write(text);
    }
    with_client(|client| {
        if client.data_manager.is_null() || client.data_device.is_null() {
            return;
        }
        unsafe {
            if let Some(old) = client.source.take() {
                destroy(old.proxy, 1); // wl_data_source.destroy
            }
            let source = construct(
                client.data_manager,
                0, // create_data_source
                &raw const wl_data_source_interface,
                &mut [arg_n()],
                TAG_DATA_SOURCE,
            );
            if source.is_null() {
                return;
            }
            for mime in TEXT_MIMES {
                request(source, 0, &mut [arg_s(mime)]);
            }
            // the self-mime: paste-from-self never touches a pipe
            let own = CString::new(format!("{}{}", SELF_MIME_PREFIX, getpid())).unwrap_or_default();
            request(source, 0, &mut [WlArgument { s: own.as_ptr() }]);
            // a stale serial KIND is silently rejected — take the max
            request(
                client.data_device,
                1, // set_selection(source, serial)
                &mut [arg_o(source), arg_u(client.serials.latest())],
            );
            wl_display_flush(client.display);
            client.source = Some(SourceState { proxy: source, text: text.to_string() });
        }
    });
}

/// Reads the selection. Our own claim answers from memory; a peer's
/// answers through a pipe under a hard deadline — a hung peer must
/// never hang the UI thread.
pub fn clipboard_read() -> Option<String> {
    if is_x11() {
        return crate::x11::clipboard_read();
    }
    // the self short-circuit
    let own = with_client(|client| {
        let mimes = client.offers.get(&client.selection)?;
        mimes
            .iter()
            .any(|mime| mime.starts_with(SELF_MIME_PREFIX))
            .then(|| client.source.as_ref().map(|source| source.text.clone()))
            .flatten()
    });
    if own.is_some() {
        return own;
    }
    let (display, offer, mime) = with_client(|client| {
        let mimes = client.offers.get(&client.selection)?;
        let mime = pick_text_mime(mimes)?;
        Some((client.display, client.selection as *mut Proxy, mime))
    })?;
    let mut fds = [0 as c_int; 2];
    if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) } != 0 {
        return None;
    }
    unsafe {
        // receive(mime, write_fd) is opcode 1 (accept is 0) — and
        // FLUSH before reading: the request otherwise sits in our
        // buffer while we block on a pipe nobody will ever write
        request(offer, 1, &mut [arg_s(mime), arg_h(fds[1])]);
        wl_display_flush(display);
        close(fds[1]);
    }
    let mut bytes = Vec::new();
    let deadline = Instant::now() + std::time::Duration::from_secs(4);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut poll_fds = [PollFd { fd: fds[0], events: POLLIN, revents: 0 }];
        let ready = unsafe { poll(poll_fds.as_mut_ptr(), 1, remaining.as_millis() as c_int) };
        if ready <= 0 {
            break;
        }
        let mut chunk = [0u8; 4096];
        let got = unsafe { read(fds[0], chunk.as_mut_ptr().cast(), chunk.len()) };
        if got <= 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..got as usize]);
    }
    unsafe { close(fds[0]) };
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    (!text.is_empty()).then_some(text)
}

/// The mime negotiation, in preference order: utf-8 first, the legacy
/// names after — the offer's advertised list decides.
fn pick_text_mime(mimes: &[String]) -> Option<&'static CStr> {
    TEXT_MIMES
        .iter()
        .find(|wanted| mimes.iter().any(|have| have == &wanted.to_string_lossy()))
        .copied()
}

/// Serves our claimed selection to a peer, chunked and poll-gated —
/// a reader that never drains must not wedge us either.
fn serve_selection(text: String, fd: c_int) {
    let bytes = text.as_bytes();
    let mut at = 0;
    let deadline = Instant::now() + std::time::Duration::from_secs(4);
    while at < bytes.len() && Instant::now() < deadline {
        const POLLOUT: i16 = 0x4;
        let mut poll_fds = [PollFd { fd, events: POLLOUT, revents: 0 }];
        if unsafe { poll(poll_fds.as_mut_ptr(), 1, 100) } <= 0 {
            continue;
        }
        let wrote = unsafe { write(fd, bytes[at..].as_ptr().cast(), bytes.len() - at) };
        if wrote <= 0 {
            break;
        }
        at += wrote as usize;
    }
    unsafe { close(fd) };
}

// MARK: - wake (any thread asks the pump for one more turn)

static WAKE_WRITE_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// The x11 door stores its own pipe's write end here — the waker is
/// one static either way, so any thread pokes the right loop.
pub(crate) fn set_wake_write_fd(fd: c_int) {
    WAKE_WRITE_FD.store(fd, std::sync::atomic::Ordering::Release);
}

// MARK: - the backend pick (the facade's one decision)

/// Which protocol border this process speaks — decided ONCE, at the
/// first window, and never again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Backend {
    Wayland,
    X11,
}

/// The pure resolver: an explicit `BUNNY_BACKEND` wins, then the
/// wayland display, then the x11 display — and a box with neither
/// has no shell to offer.
fn resolve_backend(
    forced: Option<&str>,
    wayland_display: bool,
    x11_display: bool,
) -> Option<Backend> {
    match forced {
        Some("wayland") => return Some(Backend::Wayland),
        Some("x11") => return Some(Backend::X11),
        Some(other) => {
            eprintln!("bunny_ui: unknown BUNNY_BACKEND `{other}` — picking by display");
        }
        None => {}
    }
    if wayland_display {
        Some(Backend::Wayland)
    } else if x11_display {
        Some(Backend::X11)
    } else {
        None
    }
}

pub(crate) fn backend() -> Backend {
    static BACKEND: std::sync::OnceLock<Backend> = std::sync::OnceLock::new();
    *BACKEND.get_or_init(|| {
        let forced = std::env::var("BUNNY_BACKEND").ok();
        resolve_backend(
            forced.as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
            std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()),
        )
        .expect("no wayland or x11 display — is this a graphical session?")
    })
}

fn is_x11() -> bool {
    backend() == Backend::X11
}

pub fn wake_from_any_thread() {
    let fd = WAKE_WRITE_FD.load(std::sync::atomic::Ordering::Acquire);
    if fd >= 0 {
        let byte = 1u8;
        unsafe { write(fd, (&raw const byte).cast(), 1) };
    }
}

// MARK: - IME mirror (synced per blit — the v3 door)

/// The per-blit mirror: a focused field enables the text input and
/// feeds the caret rectangle; blur disables it. Every state change is
/// double-buffered behind `commit`, and extra commits only flow while
/// the compositor's `done` echo agrees with our count.
pub fn sync_ime(state: Option<(bool, usize, (f64, f64, f64, f64))>) {
    if is_x11() {
        // no composition road on this door (XIM is a fossil); dead
        // keys still compose client-side
        return;
    }
    with_client(|client| {
        let text_input = client.ime.text_input;
        if text_input.is_null() {
            return;
        }
        unsafe {
            match state {
                Some((_marked, _start, rect)) => {
                    let rect = (
                        rect.0.round() as i32,
                        rect.1.round() as i32,
                        rect.2.round().max(1.0) as i32,
                        rect.3.round().max(1.0) as i32,
                    );
                    let mut dirty = false;
                    if !client.ime.enabled {
                        request(text_input, 1, &mut no_args()); // enable
                        request(text_input, 5, &mut [arg_u(0), arg_u(0)]); // content: none/normal
                        client.ime.enabled = true;
                        dirty = true;
                    }
                    if client.ime.last_rect != rect {
                        request(
                            text_input,
                            6,
                            &mut [arg_i(rect.0), arg_i(rect.1), arg_i(rect.2), arg_i(rect.3)],
                        );
                        client.ime.last_rect = rect;
                        dirty = true;
                    }
                    if dirty && client.ime.commits == client.ime.done_serial {
                        request(text_input, 7, &mut no_args()); // commit
                        client.ime.commits += 1;
                    }
                }
                None => {
                    if client.ime.enabled {
                        request(text_input, 2, &mut no_args()); // disable
                        request(text_input, 7, &mut no_args());
                        client.ime.enabled = false;
                        client.ime.marked = false;
                        client.ime.commits += 1;
                    }
                }
            }
        }
    });
}

/// The gate's composition-first step: while a composition is live the
/// IME owns the key stream.
fn ime_marked() -> bool {
    if is_x11() {
        // no composition road on the second door
        return false;
    }
    with_client(|client| client.ime.marked)
}

// MARK: - the frame driver (no thread: the compositor's callback is the clock)

pub fn set_frame_driver_paused(paused: bool) {
    if is_x11() {
        return crate::x11::set_frame_driver_paused(paused);
    }
    with_client(|client| {
        if let Some(win) = client.win.as_mut() {
            win.paused = paused;
        }
    });
}

// MARK: - the gpu graft (the shell side of gl.rs)

/// Grafts the GPU present onto the main window — called by the shell
/// assembler after [`create_window`] and BEFORE the first frame, so the
/// first presenting commit (the reveal, by protocol design) already
/// walks the GPU road and the CPU path never allocates a backing it
/// will not use. A refusal (`BUNNY_PRESENT=cpu`, no libEGL, a shader
/// that does not compile) changes nothing — the shm road, byte for
/// byte.
pub fn install_gpu(_window: &WindowHandle) {
    let _ = crate::gl::try_install();
}

/// What the GPU surface wraps — one variant per door.
pub(crate) enum GpuTargets {
    Wayland { display: *mut c_void, surface: *mut c_void, scene: bool },
    X11 { connection: *mut c_void, window: u32, scene: bool },
}

pub(crate) fn gpu_targets() -> Option<GpuTargets> {
    if is_x11() {
        return crate::x11::gpu_targets();
    }
    with_client(|client| {
        client.win.as_ref().map(|win| GpuTargets::Wayland {
            display: client.display as *mut c_void,
            surface: win.surface as *mut c_void,
            scene: win.scene,
        })
    })
}

/// The buffer size the GPU surface is born with, in device pixels.
pub(crate) fn gpu_buffer_size() -> (usize, usize) {
    if is_x11() {
        return crate::x11::gpu_buffer_size();
    }
    with_client(|client| {
        client.win.as_ref().map_or((1, 1), |win| {
            let scale = win.scale.max(1) as f64;
            (
                (win.logical.0 * scale).round().max(1.0) as usize,
                (win.logical.1 * scale).round().max(1.0) as usize,
            )
        })
    })
}

/// The surface state a GPU present rides in front of the swap: the
/// buffer scale and the frame callback — the same envelope the CPU
/// commit wears, minus attach and damage (the swap carries those).
/// `false` = the window is not configured yet; committing is illegal
/// and the caller keeps its frame for the next redraw.
pub(crate) fn gpu_pre_present(scale: usize) -> bool {
    if is_x11() {
        // no buffer scale to declare, no frame callback to arm — the
        // deadline clock paces; the only gate is a living window
        return crate::x11::gpu_can_present();
    }
    with_client(|client| {
        let Some(win) = client.win.as_mut() else { return false };
        if !win.map.can_attach() {
            return false;
        }
        unsafe {
            if scale > 1 && wl_proxy_get_version(win.surface) >= 3 {
                request(win.surface, 8, &mut [arg_i(scale as i32)]);
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
        }
        true
    })
}

/// The swap went through: the surface is mapped and the ack road sees
/// a presenting commit — the CPU present's bookkeeping, verbatim.
pub(crate) fn gpu_note_present() {
    if is_x11() {
        return crate::x11::gpu_note_present();
    }
    with_client(|client| {
        if let Some(win) = client.win.as_mut() {
            win.map.on_present();
        }
        client.presents += 1;
    });
}

/// The compositor must never resize the window past what the GPU can
/// render — the texture ceiling, spoken as the toplevel's max size in
/// logical units.
pub(crate) fn gpu_limit_size(max_px: usize) {
    if is_x11() {
        // x11 has no protocol max-size request the WM must honor the
        // way xdg does; the texture ceiling is far past any monitor —
        // the real-box ledger notes the WM_NORMAL_HINTS polish
        let _ = max_px;
        return;
    }
    with_client(|client| {
        let Some(win) = client.win.as_ref() else { return };
        let logical = (max_px / win.scale.max(1)) as i32;
        unsafe {
            request(win.toplevel, 7, &mut [arg_i(logical), arg_i(logical)]);
            wl_display_flush(client.display);
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
            Ev::ToplevelConfigure { width, height, states } => with_client(|client| {
                if let Some(win) = client.win.as_mut() {
                    // zero means "your choice": keep what we have
                    win.pending_size = (width > 0 && height > 0).then_some((width, height));
                    const STATE_MAXIMIZED: u32 = 1;
                    win.maximized = states.contains(&STATE_MAXIMIZED);
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
                    let owed = with_client(|client| client.presents == before);
                    if owed && crate::gl::active() {
                        // an EGL surface never takes a bare commit (the
                        // recorded old-compositor corruption) — the ack
                        // rides a REAL present instead, and the skip
                        // key forgets so the swap cannot decline
                        crate::gl::invalidate();
                        dispatch(AppEvent::Redraw);
                    } else if owed {
                        with_client(|client| {
                            if let Some(win) = client.win.as_ref() {
                                unsafe {
                                    request(win.surface, 6, &mut no_args());
                                    wl_display_flush(client.display);
                                }
                            }
                        });
                    }
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
            Ev::PointerEnter { serial, surface_ptr, x, y } => {
                let (x, y) = with_client(|client| {
                    client.serials.enter = serial;
                    // which of our surfaces the pointer entered decides
                    // the translation of everything that follows
                    client.pointer_focus = client
                        .panels
                        .iter()
                        .position(|panel| {
                            panel.as_ref().is_some_and(|p| p.surface as usize == surface_ptr)
                        })
                        .map(|index| index + 1)
                        .unwrap_or(0);
                    client.pointer_pos = (x, y);
                    translate_pointer(client, x, y)
                });
                // a stale enter serial means an ignored set_cursor —
                // re-assert, then let the scene see the entry as a move
                apply_cursor();
                dispatch(AppEvent::MouseMoved { x, y });
            }
            Ev::PointerLeave => {
                with_client(|client| client.edge_hover = 0);
                dispatch(AppEvent::MouseExited);
            }
            Ev::PointerMotion { x, y } => {
                let (band_changed, (x, y)) = with_client(|client| {
                    client.pointer_pos = (x, y);
                    let band = client
                        .win
                        .as_ref()
                        .filter(|win| {
                            client.pointer_focus == 0 && win.scene && !win.maximized
                        })
                        .map(|win| resize_edge_of(x, y, win.logical.0, win.logical.1))
                        .unwrap_or(0);
                    let changed = band != client.edge_hover;
                    client.edge_hover = band;
                    (changed, translate_pointer(client, x, y))
                });
                if band_changed {
                    apply_cursor();
                }
                dispatch(AppEvent::MouseMoved { x, y });
            }
            Ev::PointerButton { serial, time_ms, button, pressed } => {
                let (x, y) = with_client(|client| {
                    client.serials.record_button(serial, pressed);
                    let (x, y) = client.pointer_pos;
                    translate_pointer(client, x, y)
                });
                const BTN_LEFT: u32 = 0x110;
                const BTN_RIGHT: u32 = 0x111;
                let on_main = with_client(|client| client.pointer_focus == 0);
                match (button, pressed) {
                    (BTN_LEFT, true) => {
                        let clicks =
                            with_client(|client| client.clicks.click(time_ms, x, y));
                        // shift rides in with the press: over a field it
                        // EXTENDS the selection instead of replacing it,
                        // and the keymap is the authority on who is held
                        let shift = with_client(|client| shift_held(&client.keyboard));
                        // the frame conversation comes first: a press on
                        // a drag region moves the window, a control
                        // answers as the window's own button
                        // the border of a scene-chrome window belongs
                        // to the resize grab before anything else
                        let edge = with_client(|client| {
                            client
                                .win
                                .as_ref()
                                .filter(|win| win.scene && !win.maximized)
                                .map(|win| resize_edge_of(x, y, win.logical.0, win.logical.1))
                                .unwrap_or(0)
                        });
                        let take = if on_main && edge != 0 {
                            CrownTake::Resize(edge)
                        } else if on_main {
                            crown_take(x, y, clicks, false)
                        } else {
                            CrownTake::None
                        };
                        if matches!(take, CrownTake::None) || !crown_execute(take, x, y) {
                            dispatch(AppEvent::MouseDown { x, y, clicks, shift });
                        }
                        // else: the compositor took the grab — the
                        // click is spent on the frame
                    }
                    (BTN_LEFT, false) => dispatch(AppEvent::MouseUp { x, y }),
                    (BTN_RIGHT, true) => {
                        if on_main && matches!(crown_take(x, y, 1, true), CrownTake::Menu) {
                            let _ = crown_execute(CrownTake::Menu, x, y);
                        } else {
                            dispatch(AppEvent::RightMouseDown { x, y });
                        }
                    }
                    _ => {}
                }
            }
            Ev::PointerAxis { axis, value } => {
                with_client(|client| client.axis.axis(axis, value))
            }
            Ev::PointerAxisDiscrete { axis, steps } => {
                with_client(|client| client.axis.discrete(axis, steps))
            }
            Ev::PointerFrame => {
                let wheel = with_client(|client| {
                    client.axis.flush().map(|(dx, dy)| {
                        let (x, y) = client.pointer_pos;
                        let (x, y) = translate_pointer(client, x, y);
                        (x, y, dx, dy)
                    })
                });
                if let Some((x, y, dx, dy)) = wheel {
                    dispatch(AppEvent::Wheel { x, y, dx, dy });
                }
            }
            Ev::PanelConfigure { index, serial } => with_client(|client| {
                let shm = client.shm;
                let scale = client.win.as_ref().map(|w| w.scale).unwrap_or(1);
                if let Some(Some(panel)) = client.panels.get_mut(index) {
                    unsafe { request(panel.xdg, 4, &mut [arg_u(serial)]) };
                    panel.configured = true;
                    if let Some((width, height, bytes)) = panel.staged.take() {
                        unsafe {
                            flush_panel_pixels(shm, panel, width, height, &bytes, scale);
                            wl_display_flush(client.display);
                        }
                    }
                }
            }),
            Ev::PopupPosition { index, x, y } => with_client(|client| {
                if let Some(Some(panel)) = client.panels.get_mut(index) {
                    // the compositor's answer is the truth hit-testing
                    // follows; asked was our intention
                    panel.delta = (x as f64 - panel.asked.0, y as f64 - panel.asked.1);
                }
            }),
            Ev::ImePreedit { text, cursor_begin } => with_client(|client| {
                client.ime.cycle.preedit = Some((text, cursor_begin));
            }),
            Ev::ImeCommit { text } => with_client(|client| {
                client.ime.cycle.commit = Some(text);
            }),
            Ev::ImeDone { serial } => {
                let ops = with_client(|client| {
                    client.ime.done_serial = serial;
                    let (ops, marked) = client.ime.cycle.finish(client.ime.marked);
                    client.ime.marked = marked;
                    ops
                });
                for op in ops {
                    match op {
                        ImeOp::Insert(text) => dispatch(AppEvent::Text(text)),
                        ImeOp::Mark { text, caret_utf16 } => {
                            dispatch(AppEvent::ImeMark { text, caret: caret_utf16 })
                        }
                        ImeOp::Unmark => dispatch(AppEvent::ImeUnmark),
                    }
                }
            }
            Ev::ImeLeave => {
                let was_marked = with_client(|client| {
                    let was = client.ime.marked;
                    client.ime.marked = false;
                    client.ime.cycle = ImeCycle::default();
                    was
                });
                if was_marked {
                    dispatch(AppEvent::ImeUnmark);
                }
            }
            Ev::PopupDone { index } => with_client(|client| {
                // the compositor dismissed it (parent unmap, rare on
                // this road); the pool recreates if core still wants it
                if let Some(slot) = client.panels.get_mut(index) {
                    if let Some(panel) = slot.take() {
                        unsafe { teardown_panel(panel) };
                    }
                    *slot = Some(Panel::new(false));
                }
            }),
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
            Ev::KeyboardKeymap { format, fd, size } => {
                with_client(|client| unsafe {
                    // format 1 is xkb text v1 — the only one we compile
                    if format == 1 && fd >= 0 && size > 0 && !client.keyboard.context.is_null() {
                        let map = mmap(
                            std::ptr::null_mut(),
                            size as usize,
                            PROT_READ,
                            MAP_PRIVATE,
                            fd,
                            0,
                        );
                        if map as isize != -1 {
                            let keymap = xkb_keymap_new_from_string(
                                client.keyboard.context,
                                map.cast(),
                                XKB_KEYMAP_FORMAT_TEXT_V1,
                                0,
                            );
                            munmap(map, size as usize);
                            if !keymap.is_null() {
                                let kb = &mut client.keyboard;
                                if !kb.state.is_null() {
                                    xkb_state_unref(kb.state);
                                }
                                if !kb.scratch.is_null() {
                                    xkb_state_unref(kb.scratch);
                                }
                                if !kb.keymap.is_null() {
                                    xkb_keymap_unref(kb.keymap);
                                }
                                kb.keymap = keymap;
                                kb.state = xkb_state_new(keymap);
                                // the scratch state never learns the
                                // modifiers — the chars-ignoring road
                                kb.scratch = xkb_state_new(keymap);
                            }
                        }
                    }
                    if fd >= 0 {
                        close(fd);
                    }
                });
            }
            Ev::KeyboardEnter => {}
            Ev::KeyboardLeave => {
                with_client(|client| {
                    let kb = &mut client.keyboard;
                    kb.generation += 1;
                    kb.held = None;
                    kb.compositor_repeats = false;
                    if !kb.compose.is_null() {
                        unsafe { xkb_compose_state_reset(kb.compose) };
                    }
                });
                NEXT_REPEAT.with(|cell| cell.set(None));
                // focus left: popovers close like the platform's own
                dispatch(AppEvent::ResignKey);
            }
            Ev::KeyboardMods { depressed, latched, locked, group } => with_client(|client| {
                if !client.keyboard.state.is_null() {
                    unsafe {
                        xkb_state_update_mask(
                            client.keyboard.state,
                            depressed,
                            latched,
                            locked,
                            0,
                            0,
                            group,
                        );
                    }
                }
            }),
            Ev::RepeatInfo { rate, delay } => with_client(|client| {
                client.keyboard.repeat_rate = rate;
                client.keyboard.repeat_delay = delay;
            }),
            Ev::KeyboardKey { serial, key, pressed } => {
                let road = with_client(|client| {
                    client.serials.record_key(serial, pressed);
                    let keycode = key + 8; // the evdev offset
                    let kb = &mut client.keyboard;
                    if pressed {
                        // a second press of the held key without a
                        // release: the compositor repeats for us —
                        // our timer stands down for this hold
                        if kb.held.as_ref().is_some_and(|&(held, _)| held == keycode) {
                            kb.compositor_repeats = true;
                            kb.held = None;
                            NEXT_REPEAT.with(|cell| cell.set(None));
                        } else if !kb.compositor_repeats
                            && kb.repeat_rate > 0
                            && !kb.keymap.is_null()
                            && unsafe { xkb_keymap_key_repeats(kb.keymap, keycode) } == 1
                        {
                            kb.generation += 1;
                            kb.held = Some((keycode, kb.generation));
                            NEXT_REPEAT.with(|cell| {
                                cell.set(Some(
                                    Instant::now()
                                        + std::time::Duration::from_millis(
                                            kb.repeat_delay.max(0) as u64,
                                        ),
                                ))
                            });
                        }
                        key_road(kb, keycode)
                    } else {
                        if kb.held.as_ref().is_some_and(|&(held, _)| held == keycode) {
                            kb.held = None;
                            kb.generation += 1;
                            NEXT_REPEAT.with(|cell| cell.set(None));
                        }
                        kb.compositor_repeats = false;
                        KeyRoad::Silence
                    }
                });
                deliver_key(road);
            }
            Ev::NewOffer { offer_ptr } => with_client(|client| {
                client.offers.insert(offer_ptr, Vec::new());
            }),
            Ev::OfferMime { offer_ptr, mime } => with_client(|client| {
                client.offers.entry(offer_ptr).or_default().push(mime);
            }),
            Ev::Selection { offer_ptr } => with_client(|client| {
                // at most the current offer stays alive
                let stale: Vec<usize> =
                    client.offers.keys().copied().filter(|&ptr| ptr != offer_ptr).collect();
                for ptr in stale {
                    client.offers.remove(&ptr);
                    if ptr != 0 {
                        unsafe { destroy(ptr as *mut Proxy, 2) }; // wl_data_offer.destroy
                    }
                }
                client.selection = offer_ptr;
            }),
            Ev::SourceSend { mime, fd } => {
                let text = with_client(|client| {
                    let serves = TEXT_MIMES.iter().any(|t| t.to_string_lossy() == mime)
                        || mime.starts_with(SELF_MIME_PREFIX);
                    (serves).then(|| client.source.as_ref().map(|s| s.text.clone())).flatten()
                });
                match text {
                    Some(text) => serve_selection(text, fd),
                    None => unsafe {
                        close(fd);
                    },
                }
            }
            Ev::SourceCancelled => with_client(|client| {
                if let Some(source) = client.source.take() {
                    unsafe { destroy(source.proxy, 1) };
                }
            }),
        }
    }
}

/// Panel-surface events speak panel-local coordinates; the scene
/// speaks the window's. The panel's origin plus the compositor's
/// adjustment is the bridge.
fn translate_pointer(client: &Client, x: f64, y: f64) -> (f64, f64) {
    if client.pointer_focus == 0 {
        return (x, y);
    }
    match client.panels.get(client.pointer_focus - 1) {
        Some(Some(panel)) => {
            (x + panel.scene_origin.0 + panel.delta.0, y + panel.scene_origin.1 + panel.delta.1)
        }
        _ => (x, y),
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
    if is_x11() {
        return crate::x11::run();
    }
    NEXT_BLINK.with(|cell| cell.set(Some(Instant::now() + BLINK_INTERVAL)));
    loop {
        let (display, wake_fd, quit) =
            with_client(|client| (client.display, client.wake_read, client.quit));
        if quit {
            break;
        }
        let wake_woke;
        unsafe {
            while wl_display_prepare_read(display) != 0 {
                wl_display_dispatch_pending(display);
            }
            wl_display_flush(display);
            // the deadline heap, two entries tall: blink and repeat
            let timeout = [NEXT_BLINK.with(Cell::get), NEXT_REPEAT.with(Cell::get)]
                .into_iter()
                .flatten()
                .map(|at| at.saturating_duration_since(Instant::now()).as_millis() as c_int)
                .min()
                .unwrap_or(1000)
                .clamp(0, 1000);
            let mut fds = [
                PollFd { fd: wl_display_get_fd(display), events: POLLIN, revents: 0 },
                PollFd { fd: wake_fd, events: POLLIN, revents: 0 },
            ];
            let count = if wake_fd >= 0 { 2 } else { 1 };
            let ready = poll(fds.as_mut_ptr(), count, timeout);
            if ready > 0 && fds[0].revents & POLLIN != 0 {
                wl_display_read_events(display);
            } else {
                wl_display_cancel_read(display);
            }
            wake_woke = count == 2 && ready > 0 && fds[1].revents & POLLIN != 0;
            if wake_woke {
                let mut drain = [0u8; 64];
                while read(wake_fd, drain.as_mut_ptr().cast(), drain.len()) > 0 {}
            }
            wl_display_dispatch_pending(display);
            if wl_display_get_error(display) != 0 {
                eprintln!("bunny_ui_linux: the wayland connection died");
                break;
            }
        }
        drain_protocol_events();
        if wake_woke {
            dispatch(AppEvent::Wake);
        }
        let repeat_due = NEXT_REPEAT.with(|cell| {
            let due = cell.get().is_some_and(|at| Instant::now() >= at);
            if due {
                cell.set(None); // fire_repeat re-arms while the key holds
            }
            due
        });
        if repeat_due {
            fire_repeat();
        }
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

/// The repeat timer fired: if the key still holds under the same
/// generation, the whole road runs again (gate first — the platforms'
/// own repeat semantics) and the clock re-arms at the rate.
fn fire_repeat() {
    let road = with_client(|client| {
        let kb = &mut client.keyboard;
        let Some((keycode, generation)) = kb.held else {
            return KeyRoad::Silence;
        };
        if generation != kb.generation {
            // a bumped generation orphans the timer — the ghost cure
            kb.held = None;
            return KeyRoad::Silence;
        }
        let interval = (1000 / kb.repeat_rate.max(1)).max(1) as u64;
        NEXT_REPEAT
            .with(|cell| cell.set(Some(Instant::now() + std::time::Duration::from_millis(interval))));
        key_road(kb, keycode)
    });
    deliver_key(road);
}

const BLINK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Protocol teardown order is law: role → xdg_surface → wl_surface
/// last, devices released, then the connection. The GPU goes first of
/// all — its EGL surface and `wl_egl_window` must die before the
/// wayland surface they wrap.
fn teardown() {
    crate::gl::teardown();
    CLIENT.with(|slot| {
        let Some(client) = slot.borrow_mut().take() else { return };
        unsafe {
            // children before the parent — the protocol's teardown law
            for slot in client.panels {
                if let Some(panel) = slot {
                    teardown_panel(panel);
                }
            }
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
            if !client.keyboard_proxy.is_null() {
                if wl_proxy_get_version(client.keyboard_proxy) >= 3 {
                    destroy(client.keyboard_proxy, 0); // release
                } else {
                    wl_proxy_destroy(client.keyboard_proxy);
                }
            }
            if let Some(source) = &client.source {
                destroy(source.proxy, 1);
            }
            if !client.data_device.is_null() {
                if wl_proxy_get_version(client.data_device) >= 2 {
                    destroy(client.data_device, 2); // release
                } else {
                    wl_proxy_destroy(client.data_device);
                }
            }
            let kb = &client.keyboard;
            if !kb.compose.is_null() {
                xkb_compose_state_unref(kb.compose);
            }
            if !kb.state.is_null() {
                xkb_state_unref(kb.state);
            }
            if !kb.scratch.is_null() {
                xkb_state_unref(kb.scratch);
            }
            if !kb.keymap.is_null() {
                xkb_keymap_unref(kb.keymap);
            }
            if !kb.context.is_null() {
                xkb_context_unref(kb.context);
            }
            if client.wake_read >= 0 {
                close(client.wake_read);
                let wake_write = WAKE_WRITE_FD.swap(-1, std::sync::atomic::Ordering::AcqRel);
                if wake_write >= 0 {
                    close(wake_write);
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

    #[test]
    fn the_backend_pick_honors_force_then_displays() {
        use Backend::{Wayland, X11};
        // the explicit word wins over any display
        assert_eq!(resolve_backend(Some("x11"), true, true), Some(X11));
        assert_eq!(resolve_backend(Some("wayland"), false, true), Some(Wayland));
        // an unknown word degrades to the display walk, loudly
        assert_eq!(resolve_backend(Some("cocoa"), true, false), Some(Wayland));
        // wayland outranks x11 when both offer
        assert_eq!(resolve_backend(None, true, true), Some(Wayland));
        assert_eq!(resolve_backend(None, false, true), Some(X11));
        assert_eq!(resolve_backend(None, true, false), Some(Wayland));
        // a box with neither has no shell to offer
        assert_eq!(resolve_backend(None, false, false), None);
    }

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

    #[test]
    fn the_color_scheme_reads_its_three_answers() {
        assert_eq!(color_scheme_wants_dark(1), Some(true), "one is dark");
        assert_eq!(color_scheme_wants_dark(2), Some(false), "two is light");
        assert_eq!(color_scheme_wants_dark(0), None, "zero has no say");
        assert_eq!(color_scheme_wants_dark(9), None, "the future has no say either");
    }

    #[test]
    fn the_text_input_tables_match_their_xml() {
        let path =
            "/usr/share/wayland-protocols/unstable/text-input/text-input-unstable-v3.xml";
        let Ok(xml) = std::fs::read_to_string(path) else { return };
        for (name, _, methods, events) in text_input_spec() {
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
    fn the_ime_cycle_lands_the_result_before_the_fresh_preedit() {
        let mut cycle = ImeCycle::default();
        cycle.commit = Some("水".into());
        cycle.preedit = Some(("すい".into(), 3));
        let (ops, marked) = cycle.finish(true);
        assert!(marked);
        assert!(matches!(&ops[0], ImeOp::Insert(text) if text == "水"), "result first");
        assert!(
            matches!(&ops[1], ImeOp::Mark { text, caret_utf16 } if text == "すい" && *caret_utf16 == 1),
            "then the composition, caret in utf-16"
        );
    }

    #[test]
    fn a_done_without_preedit_unmarks_only_a_live_composition() {
        let mut cycle = ImeCycle::default();
        let (ops, marked) = cycle.finish(true);
        assert!(!marked);
        assert!(matches!(ops.as_slice(), [ImeOp::Unmark]), "a live run ends");
        let (ops, marked) = cycle.finish(false);
        assert!(!marked);
        assert!(ops.is_empty(), "post-commit silence: nothing was live, nothing fires");
    }

    #[test]
    fn utf16_indexes_walk_surrogates() {
        assert_eq!(utf16_index_at("aé水", 0), 0);
        assert_eq!(utf16_index_at("aé水", 1), 1);
        assert_eq!(utf16_index_at("aé水", 3), 2, "é is two bytes, one unit");
        assert_eq!(utf16_index_at("🐰b", 4), 2, "the emoji is four bytes, two units");
        assert_eq!(utf16_index_at("ab", 99), 2, "past the end clamps");
    }

    #[test]
    fn the_wheel_prefers_detents_and_flips_the_sign() {
        let mut axis = AxisAccumulator::default();
        axis.axis(0, 10.0);
        axis.discrete(0, 1);
        assert_eq!(axis.flush(), Some((0.0, -16.0)), "a detent wins over its own px value");
        axis.axis(0, -7.5);
        assert_eq!(axis.flush(), Some((0.0, 7.5)), "trackpad px flip sign, keep magnitude");
        axis.axis(1, 4.0);
        axis.discrete(1, -2);
        assert_eq!(axis.flush(), Some((32.0, 0.0)), "horizontal detents ride the same law");
        assert_eq!(axis.flush(), None, "a flush drains the accumulator");
    }

    #[test]
    fn the_mime_negotiation_prefers_utf8() {
        let offered = vec!["text/plain".to_string(), "text/plain;charset=utf-8".to_string()];
        assert_eq!(
            pick_text_mime(&offered).map(|m| m.to_string_lossy().into_owned()),
            Some("text/plain;charset=utf-8".to_string())
        );
        let legacy = vec!["TEXT".to_string(), "text/plain".to_string()];
        assert_eq!(
            pick_text_mime(&legacy).map(|m| m.to_string_lossy().into_owned()),
            Some("text/plain".to_string())
        );
        assert!(pick_text_mime(&["image/png".to_string()]).is_none());
    }

    #[test]
    fn the_wake_crosses_threads() {
        let mut fds = [0 as c_int; 2];
        assert_eq!(unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) }, 0);
        WAKE_WRITE_FD.store(fds[1], std::sync::atomic::Ordering::Release);
        std::thread::spawn(wake_from_any_thread).join().expect("the waker thread lands");
        let mut byte = [0u8; 1];
        let got = unsafe { read(fds[0], byte.as_mut_ptr().cast(), 1) };
        assert_eq!(got, 1, "one byte crossed the pipe from another thread");
        WAKE_WRITE_FD.store(-1, std::sync::atomic::Ordering::Release);
        unsafe {
            close(fds[0]);
            close(fds[1]);
        }
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
