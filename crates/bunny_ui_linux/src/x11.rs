//! The X11 border — the linux shell's second door.
//!
//! Same crate, same engines, a second protocol: this module is the
//! xcb twin of the wayland half of `ffi.rs`. The public surface stays
//! in `ffi.rs` (the facade dispatches by backend); everything here is
//! `pub(crate)` plumbing behind it.
//!
//! House rules apply: no dependencies. libxcb comes in through
//! hand-written FFI (struct layouts verified against the installed
//! `/usr/include/xcb/xproto.h`; the MIT-SHM requests against the
//! published SHM 1.2 protocol — the dev header is not installed on
//! this box, and the first put_image answers loudly if the wire is
//! wrong). Events are 32-byte tagged frames decoded against our own
//! structs; replies and events are malloc'd by xcb and returned to
//! libc `free`.
//!
//! Deviations from the wayland door, each for the better:
//! - X11 has REAL screen coordinates — overlays become plain windows
//!   placed absolutely (the fidelity bar comes free).
//! - The SERVER repeats keys (detectable autorepeat) — the client
//!   repeat machinery stands down entirely on this backend.
//! - There are no frame callbacks — the poll loop's deadline heap
//!   paces frames at the refresh interval while unpaused.
//! - MIT-SHM has no release dance — one buffer, no wait.
//!
//! IME is absent by design on this door (XIM is a fossil; composed
//! dead keys still work — compose is client-side). The mirror road
//! stays inert exactly like wayland-without-a-portal.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::time::Instant;

use crate::ffi::{
    close, dispatch, pipe2, poll, read, AppEvent, PollFd, O_CLOEXEC, O_NONBLOCK, POLLIN,
};

// MARK: - libc floor additions (SysV shared memory + free — the
// wayland door never needed either; the rest borrows ffi's floor)

unsafe extern "C" {
    fn shmget(key: c_int, size: usize, flags: c_int) -> c_int;
    fn shmat(id: c_int, addr: *const c_void, flags: c_int) -> *mut c_void;
    fn shmdt(addr: *const c_void) -> c_int;
    fn shmctl(id: c_int, command: c_int, buffer: *mut c_void) -> c_int;
    fn free(pointer: *mut c_void);
}

const IPC_PRIVATE: c_int = 0;
const IPC_CREAT: c_int = 0o1000;
const IPC_RMID: c_int = 0;

// MARK: - xcb FFI border (signatures per the installed headers)

/// Opaque `xcb_connection_t`.
#[repr(C)]
pub(crate) struct Connection {
    _opaque: [u8; 0],
}

/// `xcb_screen_t` — verified against xproto.h.
#[repr(C)]
struct Screen {
    root: u32,
    default_colormap: u32,
    white_pixel: u32,
    black_pixel: u32,
    current_input_masks: u32,
    width_in_pixels: u16,
    height_in_pixels: u16,
    width_in_millimeters: u16,
    height_in_millimeters: u16,
    min_installed_maps: u16,
    max_installed_maps: u16,
    root_visual: u32,
    backing_stores: u8,
    save_unders: u8,
    root_depth: u8,
    allowed_depths_len: u8,
}

/// `xcb_screen_iterator_t` — {data, rem, index}, returned by value.
#[repr(C)]
struct ScreenIterator {
    data: *mut Screen,
    rem: c_int,
    index: c_int,
}

/// Request cookies — every checked/unchecked request returns one; the
/// void ones are fire-and-forget, the reply ones pair with a
/// `*_reply` call.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Cookie {
    sequence: c_uint,
}

#[link(name = "xcb")]
unsafe extern "C" {
    fn xcb_connect(display: *const c_char, screen: *mut c_int) -> *mut Connection;
    fn xcb_disconnect(connection: *mut Connection);
    fn xcb_connection_has_error(connection: *mut Connection) -> c_int;
    fn xcb_get_file_descriptor(connection: *mut Connection) -> c_int;
    fn xcb_get_setup(connection: *mut Connection) -> *const c_void;
    fn xcb_setup_roots_iterator(setup: *const c_void) -> ScreenIterator;
    fn xcb_generate_id(connection: *mut Connection) -> u32;
    fn xcb_flush(connection: *mut Connection) -> c_int;
    fn xcb_poll_for_event(connection: *mut Connection) -> *mut GenericEvent;
    fn xcb_create_window(
        connection: *mut Connection,
        depth: u8,
        window: u32,
        parent: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        border_width: u16,
        class: u16,
        visual: u32,
        value_mask: u32,
        value_list: *const u32,
    ) -> Cookie;
    fn xcb_destroy_window(connection: *mut Connection, window: u32) -> Cookie;
    fn xcb_map_window(connection: *mut Connection, window: u32) -> Cookie;
    fn xcb_change_property(
        connection: *mut Connection,
        mode: u8,
        window: u32,
        property: u32,
        kind: u32,
        format: u8,
        length: u32,
        data: *const c_void,
    ) -> Cookie;
    fn xcb_intern_atom(
        connection: *mut Connection,
        only_if_exists: u8,
        name_length: u16,
        name: *const c_char,
    ) -> Cookie;
    fn xcb_intern_atom_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut InternAtomReply;
    fn xcb_get_property(
        connection: *mut Connection,
        delete: u8,
        window: u32,
        property: u32,
        kind: u32,
        offset: u32,
        length: u32,
    ) -> Cookie;
    fn xcb_get_property_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut GetPropertyReply;
    fn xcb_get_property_value(reply: *const GetPropertyReply) -> *const c_void;
    fn xcb_get_property_value_length(reply: *const GetPropertyReply) -> c_int;
    fn xcb_create_gc(
        connection: *mut Connection,
        gc: u32,
        drawable: u32,
        value_mask: u32,
        value_list: *const u32,
    ) -> Cookie;
}

#[link(name = "xcb-shm")]
unsafe extern "C" {
    fn xcb_shm_query_version(connection: *mut Connection) -> Cookie;
    fn xcb_shm_query_version_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut c_void;
    fn xcb_shm_attach(connection: *mut Connection, segment: u32, shmid: u32, read_only: u8)
    -> Cookie;
    fn xcb_shm_detach(connection: *mut Connection, segment: u32) -> Cookie;
    /// MIT-SHM 1.2 PutImage: total extent, source rect, destination
    /// origin, depth, format, send_event, segment, offset.
    #[allow(clippy::too_many_arguments)]
    fn xcb_shm_put_image(
        connection: *mut Connection,
        drawable: u32,
        gc: u32,
        total_width: u16,
        total_height: u16,
        src_x: u16,
        src_y: u16,
        src_width: u16,
        src_height: u16,
        dst_x: i16,
        dst_y: i16,
        depth: u8,
        format: u8,
        send_event: u8,
        segment: u32,
        offset: u32,
    ) -> Cookie;
}

// MARK: - Event frames (32 bytes, tagged by response_type & 0x7F)

/// The generic frame every event and error arrives in.
#[repr(C)]
pub(crate) struct GenericEvent {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    pad: [u32; 7],
}

#[repr(C)]
struct GenericError {
    response_type: u8,
    error_code: u8,
    sequence: u16,
    resource_id: u32,
    minor_code: u16,
    major_code: u16,
}

#[repr(C)]
struct InternAtomReply {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    length: u32,
    atom: u32,
}

#[repr(C)]
struct GetPropertyReply {
    response_type: u8,
    format: u8,
    sequence: u16,
    length: u32,
    kind: u32,
    bytes_after: u32,
    value_len: u32,
    pad: [u8; 12],
}

/// Expose — verified against xproto.h.
#[repr(C)]
struct ExposeEvent {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    window: u32,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    count: u16,
    pad1: [u8; 2],
}

/// ConfigureNotify — verified against xproto.h.
#[repr(C)]
struct ConfigureNotifyEvent {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    event: u32,
    window: u32,
    above_sibling: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    border_width: u16,
    override_redirect: u8,
    pad1: u8,
}

/// ClientMessage — verified against xproto.h (data union flattened to
/// the 32-bit view; format tells the true one).
#[repr(C)]
struct ClientMessageEvent {
    response_type: u8,
    format: u8,
    sequence: u16,
    window: u32,
    kind: u32,
    data32: [u32; 5],
}

/// KeyPress/KeyRelease and ButtonPress/ButtonRelease and MotionNotify
/// share one frame — verified against xproto.h.
#[repr(C)]
struct InputEvent {
    response_type: u8,
    detail: u8,
    sequence: u16,
    time: u32,
    root: u32,
    event: u32,
    child: u32,
    root_x: i16,
    root_y: i16,
    event_x: i16,
    event_y: i16,
    state: u16,
    same_screen: u8,
    pad0: u8,
}

/// EnterNotify/LeaveNotify — same head as InputEvent through event_y,
/// then mode/detail flags; only the coordinates are read.
#[repr(C)]
struct CrossingEvent {
    response_type: u8,
    detail: u8,
    sequence: u16,
    time: u32,
    root: u32,
    event: u32,
    child: u32,
    root_x: i16,
    root_y: i16,
    event_x: i16,
    event_y: i16,
    state: u16,
    mode: u8,
    same_screen_focus: u8,
}

/// PropertyNotify — verified against xproto.h.
#[repr(C)]
struct PropertyNotifyEvent {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    window: u32,
    atom: u32,
    time: u32,
    state: u8,
    pad1: [u8; 3],
}

const _: () = {
    assert!(std::mem::size_of::<GenericEvent>() == 32);
    assert!(std::mem::size_of::<ExposeEvent>() == 20);
    assert!(std::mem::size_of::<ConfigureNotifyEvent>() == 28);
    assert!(std::mem::size_of::<ClientMessageEvent>() == 32);
    assert!(std::mem::size_of::<InputEvent>() == 32);
    assert!(std::mem::size_of::<CrossingEvent>() == 32);
    assert!(std::mem::size_of::<PropertyNotifyEvent>() == 20);
    assert!(std::mem::offset_of!(InputEvent, event_x) == 24);
    assert!(std::mem::offset_of!(ClientMessageEvent, data32) == 12);
};

// event codes (xproto.h)
const XCB_KEY_PRESS: u8 = 2;
const XCB_KEY_RELEASE: u8 = 3;
const XCB_BUTTON_PRESS: u8 = 4;
const XCB_BUTTON_RELEASE: u8 = 5;
const XCB_MOTION_NOTIFY: u8 = 6;
const XCB_ENTER_NOTIFY: u8 = 7;
const XCB_LEAVE_NOTIFY: u8 = 8;
const XCB_FOCUS_IN: u8 = 9;
const XCB_FOCUS_OUT: u8 = 10;
const XCB_EXPOSE: u8 = 12;
const XCB_DESTROY_NOTIFY: u8 = 17;
const XCB_CONFIGURE_NOTIFY: u8 = 22;
const XCB_PROPERTY_NOTIFY: u8 = 28;
const XCB_CLIENT_MESSAGE: u8 = 33;

// request vocabulary (xproto.h)
const WINDOW_CLASS_INPUT_OUTPUT: u16 = 1;
const PROP_MODE_REPLACE: u8 = 0;
const CW_BACK_PIXEL: u32 = 0x0002;
const CW_EVENT_MASK: u32 = 0x0800;
const EVENT_MASK_KEY_PRESS: u32 = 0x0000_0001;
const EVENT_MASK_KEY_RELEASE: u32 = 0x0000_0002;
const EVENT_MASK_BUTTON_PRESS: u32 = 0x0000_0004;
const EVENT_MASK_BUTTON_RELEASE: u32 = 0x0000_0008;
const EVENT_MASK_ENTER_WINDOW: u32 = 0x0000_0010;
const EVENT_MASK_LEAVE_WINDOW: u32 = 0x0000_0020;
const EVENT_MASK_POINTER_MOTION: u32 = 0x0000_0040;
const EVENT_MASK_EXPOSURE: u32 = 0x0000_8000;
const EVENT_MASK_STRUCTURE_NOTIFY: u32 = 0x0002_0000;
const EVENT_MASK_FOCUS_CHANGE: u32 = 0x0020_0000;
const IMAGE_FORMAT_Z_PIXMAP: u8 = 2;
const ATOM_ATOM: u32 = 4;
const ATOM_STRING: u32 = 31;
const ATOM_WM_NAME: u32 = 39;
const ATOM_WM_CLASS: u32 = 67;
const ATOM_RESOURCE_MANAGER: u32 = 23;

// MARK: - The atom table (interned once, one round trip)

/// Every atom this door speaks, interned in one batch at connect.
pub(crate) struct Atoms {
    pub(crate) wm_protocols: u32,
    pub(crate) wm_delete_window: u32,
    pub(crate) net_wm_name: u32,
    pub(crate) utf8_string: u32,
}

const ATOM_NAMES: [&CStr; 4] =
    [c"WM_PROTOCOLS", c"WM_DELETE_WINDOW", c"_NET_WM_NAME", c"UTF8_STRING"];

use std::ffi::CStr;

fn intern_atoms(connection: *mut Connection) -> Atoms {
    let cookies: Vec<Cookie> = ATOM_NAMES
        .iter()
        .map(|name| unsafe {
            xcb_intern_atom(connection, 0, name.to_bytes().len() as u16, name.as_ptr())
        })
        .collect();
    let mut atoms = [0u32; 4];
    for (slot, cookie) in atoms.iter_mut().zip(cookies) {
        unsafe {
            let reply = xcb_intern_atom_reply(connection, cookie, std::ptr::null_mut());
            if !reply.is_null() {
                *slot = (*reply).atom;
                free(reply.cast());
            }
        }
    }
    Atoms {
        wm_protocols: atoms[0],
        wm_delete_window: atoms[1],
        net_wm_name: atoms[2],
        utf8_string: atoms[3],
    }
}

// MARK: - The SHM backing (single retained buffer, no release dance)

struct Backing {
    shmid: c_int,
    segment: u32,
    map: *mut u8,
    len: usize,
    width: usize,
    height: usize,
}

fn make_backing(connection: *mut Connection, width: usize, height: usize) -> Option<Backing> {
    let len = width * height * 4;
    if len == 0 {
        return None;
    }
    unsafe {
        let shmid = shmget(IPC_PRIVATE, len, IPC_CREAT | 0o600);
        if shmid < 0 {
            eprintln!("bunny_ui x11: shmget failed");
            return None;
        }
        let map = shmat(shmid, std::ptr::null(), 0);
        if map as isize == -1 {
            shmctl(shmid, IPC_RMID, std::ptr::null_mut());
            eprintln!("bunny_ui x11: shmat failed");
            return None;
        }
        let segment = xcb_generate_id(connection);
        xcb_shm_attach(connection, segment, shmid as u32, 0);
        xcb_flush(connection);
        // RMID immediately: the kernel keeps the segment while both
        // sides stay attached, and a crash leaks nothing
        shmctl(shmid, IPC_RMID, std::ptr::null_mut());
        Some(Backing { shmid, segment, map: map.cast(), len, width, height })
    }
}

unsafe fn drop_backing(connection: *mut Connection, backing: Backing) {
    unsafe {
        xcb_shm_detach(connection, backing.segment);
        shmdt(backing.map.cast());
        let _ = backing.shmid; // RMID already done at attach
        let _ = backing.len;
    }
}

// MARK: - The client (thread_local twin of the wayland Client)

struct Window {
    id: u32,
    gc: u32,
    logical: (f64, f64),
    scale: usize,
    backing: Option<Backing>,
    mapped: bool,
    paused: bool,
    last_frame: Option<Instant>,
    /// Scene chrome: the shell owns the border (crown arrives in Q4).
    #[allow(dead_code)] // read from Q4 on — the flag lands with the door
    scene: bool,
}

pub(crate) struct XClient {
    connection: *mut Connection,
    root: u32,
    root_depth: u8,
    root_visual: u32,
    atoms: Atoms,
    win: Option<Window>,
    wake_read: c_int,
    pointer_pos: (f64, f64),
    quit: bool,
    presents: u64,
}

thread_local! {
    static X_CLIENT: RefCell<Option<XClient>> = const { RefCell::new(None) };
    static NEXT_FRAME: Cell<Option<Instant>> = const { Cell::new(None) };
    static NEXT_BLINK: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Events pulled from xcb while the client was borrowed elsewhere
    /// wait here — same discipline as the wayland EVQ, though xcb has
    /// no dispatcher reentrancy: the queue only smooths the drain.
    static PENDING: RefCell<VecDeque<*mut GenericEvent>> = const { RefCell::new(VecDeque::new()) };
}

fn with_x<R>(body: impl FnOnce(&mut XClient) -> R) -> R {
    X_CLIENT.with(|slot| {
        let mut slot = slot.borrow_mut();
        body(slot.as_mut().expect("the x11 client exists"))
    })
}

pub(crate) fn connect() {
    let display = unsafe { xcb_connect(std::ptr::null(), std::ptr::null_mut()) };
    assert!(
        !display.is_null() && unsafe { xcb_connection_has_error(display) } == 0,
        "no x11 display — is DISPLAY set?"
    );
    let (root, root_depth, root_visual) = unsafe {
        let setup = xcb_get_setup(display);
        let screens = xcb_setup_roots_iterator(setup);
        assert!(screens.rem > 0 && !screens.data.is_null(), "an x11 screen exists");
        let screen = &*screens.data;
        (screen.root, screen.root_depth, screen.root_visual)
    };
    // MIT-SHM must answer or the CPU road degrades to core PutImage —
    // over XWayland it always answers; the fallback is post-war
    unsafe {
        let cookie = xcb_shm_query_version(display);
        let reply = xcb_shm_query_version_reply(display, cookie, std::ptr::null_mut());
        assert!(!reply.is_null(), "MIT-SHM is required this war (remote-X fallback post-war)");
        free(reply);
    }
    let atoms = intern_atoms(display);
    // the wake pipe: any thread pokes the write end; the poll loop
    // owns the read end — the shared WAKE_WRITE_FD static routes it
    let mut fds = [-1 as c_int; 2];
    let wake_read = unsafe {
        if pipe2(fds.as_mut_ptr(), O_CLOEXEC | O_NONBLOCK) == 0 {
            crate::ffi::set_wake_write_fd(fds[1]);
            fds[0]
        } else {
            -1
        }
    };
    X_CLIENT.with(|slot| {
        *slot.borrow_mut() = Some(XClient {
            connection: display,
            root,
            root_depth,
            root_visual,
            atoms,
            win: None,
            wake_read,
            pointer_pos: (0.0, 0.0),
            quit: false,
            presents: 0,
        })
    });
}

// MARK: - Scale (Xft.dpi off the root's RESOURCE_MANAGER)

/// Parses `Xft.dpi: <n>` out of an xrdb blob — the one resource the
/// shell reads. Returns the integer raster scale, floor 1.
fn scale_from_resources(blob: &str) -> usize {
    for line in blob.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        if key.trim() == "Xft.dpi" {
            if let Ok(dpi) = value.trim().parse::<f64>() {
                return ((dpi / 96.0).round() as usize).max(1);
            }
        }
    }
    1
}

fn read_scale(client: &mut XClient) -> usize {
    unsafe {
        let cookie = xcb_get_property(
            client.connection,
            0,
            client.root,
            ATOM_RESOURCE_MANAGER,
            ATOM_STRING,
            0,
            64 * 1024,
        );
        let reply = xcb_get_property_reply(client.connection, cookie, std::ptr::null_mut());
        if reply.is_null() {
            return 1;
        }
        let bytes = std::slice::from_raw_parts(
            xcb_get_property_value(reply) as *const u8,
            xcb_get_property_value_length(reply).max(0) as usize,
        );
        let scale = scale_from_resources(&String::from_utf8_lossy(bytes));
        free(reply.cast());
        scale
    }
}

// MARK: - Window

pub(crate) fn create_window(title: &str, width: f64, height: f64, scene: bool) {
    if X_CLIENT.with(|slot| slot.borrow().is_none()) {
        connect();
    }
    with_x(|client| {
        let scale = read_scale(client);
        let physical = ((width * scale as f64) as u16, (height * scale as f64) as u16);
        unsafe {
            let id = xcb_generate_id(client.connection);
            // anti-flash: the background pixel IS the canvas — a map
            // before the first present shows theme ground, never white
            let canvas = bunny_ui::theme::canvas();
            let back = ((canvas.r as u32) << 16) | ((canvas.g as u32) << 8) | canvas.b as u32;
            let values = [
                back,
                EVENT_MASK_KEY_PRESS
                    | EVENT_MASK_KEY_RELEASE
                    | EVENT_MASK_BUTTON_PRESS
                    | EVENT_MASK_BUTTON_RELEASE
                    | EVENT_MASK_ENTER_WINDOW
                    | EVENT_MASK_LEAVE_WINDOW
                    | EVENT_MASK_POINTER_MOTION
                    | EVENT_MASK_EXPOSURE
                    | EVENT_MASK_STRUCTURE_NOTIFY
                    | EVENT_MASK_FOCUS_CHANGE,
            ];
            xcb_create_window(
                client.connection,
                0, // CopyFromParent depth
                id,
                client.root,
                0,
                0,
                physical.0.max(1),
                physical.1.max(1),
                0,
                WINDOW_CLASS_INPUT_OUTPUT,
                client.root_visual,
                CW_BACK_PIXEL | CW_EVENT_MASK,
                values.as_ptr(),
            );
            let gc = xcb_generate_id(client.connection);
            xcb_create_gc(client.connection, gc, id, 0, std::ptr::null());
            // the close handshake and both name spellings
            xcb_change_property(
                client.connection,
                PROP_MODE_REPLACE,
                id,
                client.atoms.wm_protocols,
                ATOM_ATOM,
                32,
                1,
                (&raw const client.atoms.wm_delete_window).cast(),
            );
            xcb_change_property(
                client.connection,
                PROP_MODE_REPLACE,
                id,
                client.atoms.net_wm_name,
                client.atoms.utf8_string,
                8,
                title.len() as u32,
                title.as_ptr().cast(),
            );
            xcb_change_property(
                client.connection,
                PROP_MODE_REPLACE,
                id,
                ATOM_WM_NAME,
                ATOM_STRING,
                8,
                title.len() as u32,
                title.as_ptr().cast(),
            );
            let class = b"bunny_ui\0bunny_ui\0";
            xcb_change_property(
                client.connection,
                PROP_MODE_REPLACE,
                id,
                ATOM_WM_CLASS,
                ATOM_STRING,
                8,
                class.len() as u32,
                class.as_ptr().cast(),
            );
            xcb_flush(client.connection);
            client.win = Some(Window {
                id,
                gc,
                logical: (width, height),
                scale,
                backing: None,
                mapped: false,
                paused: true,
                last_frame: None,
                scene,
            });
        }
    });
}

/// The reveal: the first present landed into the unmapped window's
/// backing; mapping shows it (background pixel covers the gap between
/// map and the first Expose re-put).
pub(crate) fn show_window() {
    with_x(|client| {
        if let Some(win) = client.win.as_mut() {
            unsafe {
                xcb_map_window(client.connection, win.id);
                xcb_flush(client.connection);
            }
            win.mapped = true;
        }
    });
}

pub(crate) fn content_size() -> (f64, f64) {
    with_x(|client| client.win.as_ref().map(|w| w.logical).unwrap_or((0.0, 0.0)))
}

pub(crate) fn scale() -> usize {
    with_x(|client| client.win.as_ref().map(|w| w.scale).unwrap_or(1))
}

fn ensure_backing(client: &mut XClient, width: usize, height: usize) -> bool {
    let stale = client
        .win
        .as_ref()
        .and_then(|w| w.backing.as_ref())
        .is_none_or(|backing| backing.width != width || backing.height != height);
    if !stale {
        return true;
    }
    let connection = client.connection;
    let Some(win) = client.win.as_mut() else { return false };
    if let Some(old) = win.backing.take() {
        unsafe { drop_backing(connection, old) };
    }
    win.backing = make_backing(connection, width, height);
    win.backing.is_some()
}

/// The present twin: damage rows RGBA→BGRX into the shm map, one
/// put per damage rect. ZPixmap depth-24 little-endian is the same
/// byte lattice as wayland's XRGB8888 — the swizzle is identical.
pub(crate) fn present_rows(
    width: usize,
    height: usize,
    rgba: &[u8],
    damage: &[(i64, i64, i64, i64)],
) {
    with_x(|client| {
        if !ensure_backing(client, width, height) {
            return;
        }
        let connection = client.connection;
        let root_depth = client.root_depth;
        let win = client.win.as_mut().expect("window for the present");
        let backing = win.backing.as_ref().expect("backing for the present");
        for &rect in damage {
            let Some((x, y, w, h)) = clamp_rect(rect, width, height) else { continue };
            for row in y..y + h {
                let start = (row * width + x) * 4;
                let source = &rgba[start..start + w * 4];
                let target = unsafe {
                    std::slice::from_raw_parts_mut(backing.map.add(start), w * 4)
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
            unsafe {
                xcb_shm_put_image(
                    connection,
                    win.id,
                    win.gc,
                    width as u16,
                    height as u16,
                    x as u16,
                    y as u16,
                    w as u16,
                    h as u16,
                    x as i16,
                    y as i16,
                    root_depth,
                    IMAGE_FORMAT_Z_PIXMAP,
                    0,
                    backing.segment,
                    0,
                );
            }
        }
        unsafe { xcb_flush(connection) };
        client.presents += 1;
    });
}

/// Clamps a damage rect to the buffer, in usize pixels.
fn clamp_rect(
    rect: (i64, i64, i64, i64),
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let x0 = rect.0.max(0).min(width as i64);
    let y0 = rect.1.max(0).min(height as i64);
    let x1 = rect.2.max(0).min(width as i64);
    let y1 = rect.3.max(0).min(height as i64);
    (x1 > x0 && y1 > y0).then(|| {
        (x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize)
    })
}

/// An Expose re-puts the wounded rect from the retained shm image —
/// the frame is already there; the server only lost its copy.
fn handle_expose(x: u16, y: u16, w: u16, h: u16) {
    with_x(|client| {
        let connection = client.connection;
        let root_depth = client.root_depth;
        let Some(win) = client.win.as_ref() else { return };
        let Some(backing) = win.backing.as_ref() else { return };
        let x1 = (x as usize + w as usize).min(backing.width);
        let y1 = (y as usize + h as usize).min(backing.height);
        if x1 <= x as usize || y1 <= y as usize {
            return;
        }
        unsafe {
            xcb_shm_put_image(
                connection,
                win.id,
                win.gc,
                backing.width as u16,
                backing.height as u16,
                x,
                y,
                (x1 - x as usize) as u16,
                (y1 - y as usize) as u16,
                x as i16,
                y as i16,
                root_depth,
                IMAGE_FORMAT_Z_PIXMAP,
                0,
                backing.segment,
                0,
            );
            xcb_flush(connection);
        }
    });
}

// MARK: - The frame clock (no callbacks on this door — the deadline
// heap paces at the refresh interval while unpaused)

const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_micros(16_666);
const BLINK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

pub(crate) fn set_frame_driver_paused(paused: bool) {
    with_x(|client| {
        if let Some(win) = client.win.as_mut() {
            win.paused = paused;
        }
    });
    NEXT_FRAME.with(|cell| {
        if paused {
            cell.set(None);
        } else if cell.get().is_none() {
            cell.set(Some(Instant::now() + FRAME_INTERVAL));
        }
    });
}

fn frame_due() {
    let dt = with_x(|client| {
        let Some(win) = client.win.as_mut() else { return None };
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
        NEXT_FRAME.with(|cell| cell.set(Some(Instant::now() + FRAME_INTERVAL)));
        dispatch(AppEvent::Frame { dt });
    }
}

// MARK: - The loop

/// One decoded step of the drain: the borrow closes before any
/// AppEvent leaves (the handler re-enters the facade freely).
enum Step {
    Deliver(AppEvent),
    Quit,
    Silence,
}

fn interpret(event: *mut GenericEvent) -> Step {
    let kind = unsafe { (*event).response_type } & 0x7F;
    match kind {
        0 => {
            let error = event as *mut GenericError;
            unsafe {
                eprintln!(
                    "bunny_ui x11: error code {} major {} minor {}",
                    (*error).error_code,
                    (*error).major_code,
                    (*error).minor_code
                );
            }
            Step::Silence
        }
        XCB_EXPOSE => {
            let expose = event as *mut ExposeEvent;
            let (x, y, w, h) =
                unsafe { ((*expose).x, (*expose).y, (*expose).width, (*expose).height) };
            handle_expose(x, y, w, h);
            Step::Silence
        }
        XCB_CONFIGURE_NOTIFY => {
            let configure = event as *mut ConfigureNotifyEvent;
            let (width, height) = unsafe { ((*configure).width, (*configure).height) };
            let resized = with_x(|client| {
                let Some(win) = client.win.as_mut() else { return false };
                let logical = (
                    width as f64 / win.scale as f64,
                    height as f64 / win.scale as f64,
                );
                if logical != win.logical && width > 0 && height > 0 {
                    win.logical = logical;
                    true
                } else {
                    false
                }
            });
            if resized {
                Step::Deliver(AppEvent::Redraw)
            } else {
                Step::Silence
            }
        }
        XCB_CLIENT_MESSAGE => {
            let message = event as *mut ClientMessageEvent;
            let close = with_x(|client| unsafe {
                (*message).kind == client.atoms.wm_protocols
                    && (*message).data32[0] == client.atoms.wm_delete_window
            });
            if close {
                Step::Quit
            } else {
                Step::Silence
            }
        }
        XCB_DESTROY_NOTIFY => Step::Quit,
        XCB_MOTION_NOTIFY => {
            let motion = event as *mut InputEvent;
            let (x, y) = unsafe { ((*motion).event_x, (*motion).event_y) };
            let scale = with_x(|client| {
                client.pointer_pos = (x as f64, y as f64);
                client.win.as_ref().map(|w| w.scale).unwrap_or(1)
            });
            Step::Deliver(AppEvent::MouseMoved {
                x: x as f64 / scale as f64,
                y: y as f64 / scale as f64,
            })
        }
        XCB_BUTTON_PRESS | XCB_BUTTON_RELEASE => {
            let button = event as *mut InputEvent;
            let (detail, x, y, state) = unsafe {
                ((*button).detail, (*button).event_x, (*button).event_y, (*button).state)
            };
            let scale = with_x(|client| client.win.as_ref().map(|w| w.scale).unwrap_or(1));
            let (x, y) = (x as f64 / scale as f64, y as f64 / scale as f64);
            match (kind, detail) {
                // wheel buttons arrive in Q1 with the accumulator
                (XCB_BUTTON_PRESS, 1) => Step::Deliver(AppEvent::MouseDown {
                    x,
                    y,
                    clicks: 1,
                    shift: state & 0x1 != 0,
                }),
                (XCB_BUTTON_RELEASE, 1) => Step::Deliver(AppEvent::MouseUp { x, y }),
                (XCB_BUTTON_PRESS, 3) => Step::Deliver(AppEvent::RightMouseDown { x, y }),
                _ => Step::Silence,
            }
        }
        XCB_LEAVE_NOTIFY => Step::Deliver(AppEvent::MouseExited),
        XCB_FOCUS_OUT => Step::Deliver(AppEvent::ResignKey),
        // keys arrive in Q1 with xkbcommon-x11; enter/focus-in and
        // property notify have no engine mirror yet
        XCB_KEY_PRESS | XCB_KEY_RELEASE | XCB_ENTER_NOTIFY | XCB_FOCUS_IN
        | XCB_PROPERTY_NOTIFY => Step::Silence,
        _ => Step::Silence,
    }
}

/// Pulls every queued xcb event and interprets it — events free after
/// use (xcb mallocs each frame).
fn drain_events() -> bool {
    let mut quit = false;
    loop {
        let event = with_x(|client| unsafe { xcb_poll_for_event(client.connection) });
        if event.is_null() {
            break;
        }
        let step = interpret(event);
        unsafe { free(event.cast()) };
        match step {
            Step::Deliver(app_event) => dispatch(app_event),
            Step::Quit => quit = true,
            Step::Silence => {}
        }
    }
    quit
}

pub(crate) fn run() {
    NEXT_BLINK.with(|cell| cell.set(Some(Instant::now() + BLINK_INTERVAL)));
    loop {
        let (fd, wake_fd, quit, connection) = with_x(|client| {
            (
                unsafe { xcb_get_file_descriptor(client.connection) },
                client.wake_read,
                client.quit,
                client.connection,
            )
        });
        if quit {
            break;
        }
        unsafe {
            xcb_flush(connection);
        }
        let timeout = [NEXT_BLINK.with(Cell::get), NEXT_FRAME.with(Cell::get)]
            .into_iter()
            .flatten()
            .map(|at| at.saturating_duration_since(Instant::now()).as_millis() as c_int)
            .min()
            .unwrap_or(1000)
            .clamp(0, 1000);
        let mut fds = [
            PollFd { fd, events: POLLIN, revents: 0 },
            PollFd { fd: wake_fd, events: POLLIN, revents: 0 },
        ];
        let count = if wake_fd >= 0 { 2 } else { 1 };
        let ready = unsafe { poll(fds.as_mut_ptr(), count, timeout) };
        let wake_woke = count == 2 && ready > 0 && fds[1].revents & POLLIN != 0;
        if wake_woke {
            let mut drain = [0u8; 64];
            unsafe { while read(wake_fd, drain.as_mut_ptr().cast(), drain.len()) > 0 {} }
        }
        if unsafe { xcb_connection_has_error(connection) } != 0 {
            eprintln!("bunny_ui x11: the connection died");
            break;
        }
        if drain_events() {
            with_x(|client| client.quit = true);
        }
        if wake_woke {
            dispatch(AppEvent::Wake);
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
        let frame_is_due =
            NEXT_FRAME.with(|cell| cell.get().is_some_and(|at| Instant::now() >= at));
        if frame_is_due {
            frame_due();
        }
    }
    teardown();
}

fn teardown() {
    X_CLIENT.with(|slot| {
        let Some(client) = slot.borrow_mut().take() else { return };
        unsafe {
            if let Some(win) = client.win {
                if let Some(backing) = win.backing {
                    drop_backing(client.connection, backing);
                }
                xcb_destroy_window(client.connection, win.id);
            }
            xcb_flush(client.connection);
            xcb_disconnect(client.connection);
            if client.wake_read >= 0 {
                close(client.wake_read);
            }
        }
    });
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_frames_hold_their_layout() {
        // the const asserts gate the build; these give CI a line to
        // point at when a header revision ever moves a field
        assert_eq!(std::mem::size_of::<GenericEvent>(), 32);
        assert_eq!(std::mem::size_of::<InputEvent>(), 32);
        assert_eq!(std::mem::offset_of!(InputEvent, time), 4);
        assert_eq!(std::mem::offset_of!(InputEvent, event_x), 24);
        assert_eq!(std::mem::offset_of!(InputEvent, state), 28);
        assert_eq!(std::mem::offset_of!(ConfigureNotifyEvent, width), 20);
        assert_eq!(std::mem::offset_of!(ExposeEvent, count), 16);
        assert_eq!(std::mem::offset_of!(ClientMessageEvent, data32), 12);
        assert_eq!(std::mem::offset_of!(PropertyNotifyEvent, atom), 8);
        assert_eq!(std::mem::size_of::<Screen>(), 40);
    }

    #[test]
    fn xft_dpi_parses_and_defaults() {
        assert_eq!(scale_from_resources("Xft.dpi:\t96\n"), 1);
        assert_eq!(scale_from_resources("Xft.dpi: 192"), 2);
        assert_eq!(scale_from_resources("Xft.dpi:144.0"), 2, "150% rounds to the 2x lattice");
        assert_eq!(scale_from_resources("Xft.dpi: 120"), 1, "125% floors to 1 (integer law)");
        assert_eq!(scale_from_resources("Xcursor.size: 24\n*background: #000\n"), 1);
        assert_eq!(scale_from_resources(""), 1);
        assert_eq!(scale_from_resources("Xft.dpi: garbage"), 1);
    }

    #[test]
    fn damage_rects_clamp_to_the_buffer() {
        assert_eq!(clamp_rect((0, 0, 10, 10), 100, 100), Some((0, 0, 10, 10)));
        assert_eq!(clamp_rect((-5, -5, 10, 10), 100, 100), Some((0, 0, 10, 10)));
        assert_eq!(clamp_rect((90, 90, 200, 200), 100, 100), Some((90, 90, 10, 10)));
        assert_eq!(clamp_rect((50, 50, 50, 50), 100, 100), None, "empty is nothing");
        assert_eq!(clamp_rect((200, 0, 300, 10), 100, 100), None, "outside is nothing");
    }
}
