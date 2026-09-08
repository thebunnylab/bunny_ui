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
    close, deliver_key, dispatch, key_road, pipe2, poll, read, AppEvent, ClickClock, Cursor,
    Keyboard, PollFd, O_CLOEXEC, O_NONBLOCK, POLLIN,
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
    fn xcb_open_font(
        connection: *mut Connection,
        font: u32,
        name_length: u16,
        name: *const c_char,
    ) -> Cookie;
    /// The core glyph cursor: source and mask glyphs from one font,
    /// black ink on white ground (the universal fallback tier).
    #[allow(clippy::too_many_arguments)]
    fn xcb_create_glyph_cursor(
        connection: *mut Connection,
        cursor: u32,
        source_font: u32,
        mask_font: u32,
        source_char: u16,
        mask_char: u16,
        fore_red: u16,
        fore_green: u16,
        fore_blue: u16,
        back_red: u16,
        back_green: u16,
        back_blue: u16,
    ) -> Cookie;
    fn xcb_change_window_attributes(
        connection: *mut Connection,
        window: u32,
        value_mask: u32,
        value_list: *const u32,
    ) -> Cookie;
    fn xcb_set_selection_owner(
        connection: *mut Connection,
        owner: u32,
        selection: u32,
        time: u32,
    ) -> Cookie;
    fn xcb_convert_selection(
        connection: *mut Connection,
        requestor: u32,
        selection: u32,
        target: u32,
        property: u32,
        time: u32,
    ) -> Cookie;
    fn xcb_send_event(
        connection: *mut Connection,
        propagate: u8,
        destination: u32,
        event_mask: u32,
        event: *const c_char,
    ) -> Cookie;
    fn xcb_configure_window(
        connection: *mut Connection,
        window: u32,
        value_mask: u16,
        value_list: *const u32,
    ) -> Cookie;
    fn xcb_unmap_window(connection: *mut Connection, window: u32) -> Cookie;
    fn xcb_create_colormap(
        connection: *mut Connection,
        alloc: u8,
        colormap: u32,
        window: u32,
        visual: u32,
    ) -> Cookie;
    fn xcb_ungrab_pointer(connection: *mut Connection, time: u32) -> Cookie;
    fn xcb_translate_coordinates(
        connection: *mut Connection,
        src_window: u32,
        dst_window: u32,
        src_x: i16,
        src_y: i16,
    ) -> Cookie;
    fn xcb_translate_coordinates_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut TranslateCoordinatesReply;
    fn xcb_screen_allowed_depths_iterator(screen: *const Screen) -> DepthIterator;
    fn xcb_depth_next(iterator: *mut DepthIterator);
    fn xcb_depth_visuals(depth: *const Depth) -> *const VisualType;
    fn xcb_depth_visuals_length(depth: *const Depth) -> c_int;
}

#[link(name = "xcb-xfixes")]
unsafe extern "C" {
    /// The version handshake is mandatory before any xfixes request.
    fn xcb_xfixes_query_version(
        connection: *mut Connection,
        major: u32,
        minor: u32,
    ) -> Cookie;
    fn xcb_xfixes_query_version_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut c_void;
    fn xcb_xfixes_create_region(
        connection: *mut Connection,
        region: u32,
        rectangles_len: u32,
        rectangles: *const Rectangle,
    ) -> Cookie;
    fn xcb_xfixes_destroy_region(connection: *mut Connection, region: u32) -> Cookie;
    fn xcb_xfixes_set_window_shape_region(
        connection: *mut Connection,
        window: u32,
        shape_kind: u8,
        x_offset: i16,
        y_offset: i16,
        region: u32,
    ) -> Cookie;
}

/// ShapeInput — the kind that decides where clicks land.
const SHAPE_KIND_INPUT: u8 = 2;

#[repr(C)]
struct Rectangle {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

#[repr(C)]
struct TranslateCoordinatesReply {
    response_type: u8,
    same_screen: u8,
    sequence: u16,
    length: u32,
    child: u32,
    dst_x: i16,
    dst_y: i16,
}

/// `xcb_depth_t` fixed head — verified against xproto.h.
#[repr(C)]
struct Depth {
    depth: u8,
    pad0: u8,
    visuals_len: u16,
    pad1: [u8; 4],
}

#[repr(C)]
struct DepthIterator {
    data: *mut Depth,
    rem: c_int,
    index: c_int,
}

/// `xcb_visualtype_t` — verified against xproto.h.
#[repr(C)]
struct VisualType {
    visual_id: u32,
    class: u8,
    bits_per_rgb_value: u8,
    colormap_entries: u16,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    pad0: [u8; 4],
}

#[link(name = "xkbcommon-x11")]
unsafe extern "C" {
    fn xkb_x11_setup_xkb_extension(
        connection: *mut Connection,
        major: u16,
        minor: u16,
        flags: c_int,
        major_out: *mut u16,
        minor_out: *mut u16,
        base_event_out: *mut u8,
        base_error_out: *mut u8,
    ) -> c_int;
    fn xkb_x11_get_core_keyboard_device_id(connection: *mut Connection) -> i32;
    fn xkb_x11_keymap_new_from_device(
        context: *mut c_void,
        connection: *mut Connection,
        device: i32,
        flags: c_int,
    ) -> *mut c_void;
    fn xkb_x11_state_new_from_device(
        keymap: *mut c_void,
        connection: *mut Connection,
        device: i32,
    ) -> *mut c_void;
}

#[link(name = "xcb-xkb")]
unsafe extern "C" {
    /// PerClientFlags — the detectable-autorepeat switch (XKB 1.0).
    #[allow(clippy::too_many_arguments)]
    fn xcb_xkb_per_client_flags(
        connection: *mut Connection,
        device_spec: u16,
        change: u32,
        value: u32,
        controls_to_change: u32,
        auto_controls: u32,
        auto_controls_values: u32,
    ) -> Cookie;
    fn xcb_xkb_per_client_flags_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut c_void;
    /// SelectEvents, the simple affect/select form (details NULL).
    #[allow(clippy::too_many_arguments)]
    fn xcb_xkb_select_events(
        connection: *mut Connection,
        device_spec: u16,
        affect_which: u16,
        clear: u16,
        select_all: u16,
        affect_map: u16,
        map: u16,
        details: *const c_void,
    ) -> Cookie;
}

const XKB_ID_USE_CORE_KBD: u16 = 256;
const XKB_PER_CLIENT_FLAG_DETECTABLE_AUTO_REPEAT: u32 = 1;
const XKB_EVENT_TYPE_STATE_NOTIFY: u16 = 4;

/// `xcb_xkb_state_notify_event_t` head — verified against xkb.h; only
/// the mod/group fields are read (update_mask wants exactly those).
#[repr(C)]
struct XkbStateNotifyEvent {
    response_type: u8,
    xkb_type: u8,
    sequence: u16,
    time: u32,
    device_id: u8,
    mods: u8,
    base_mods: u8,
    latched_mods: u8,
    locked_mods: u8,
    group: u8,
    base_group: i16,
    latched_group: i16,
    locked_group: u8,
    compat_state: u8,
    grab_mods: u8,
    compat_grab_mods: u8,
    lookup_mods: u8,
    compat_lookup_mods: u8,
    ptr_btn_state: u16,
    changed: u16,
    keycode: u8,
    event_type: u8,
}

const _: () = {
    assert!(std::mem::offset_of!(XkbStateNotifyEvent, base_mods) == 10);
    assert!(std::mem::offset_of!(XkbStateNotifyEvent, base_group) == 14);
    assert!(std::mem::offset_of!(XkbStateNotifyEvent, locked_group) == 18);
};

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

/// SelectionRequest — verified against xproto.h.
#[repr(C)]
struct SelectionRequestEvent {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    time: u32,
    owner: u32,
    requestor: u32,
    selection: u32,
    target: u32,
    property: u32,
}

/// SelectionNotify — verified against xproto.h. Doubles as the frame
/// this door SENDS back to requestors (send_event wants 32 bytes; the
/// tail pads).
#[repr(C)]
struct SelectionNotifyEvent {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    time: u32,
    requestor: u32,
    selection: u32,
    target: u32,
    property: u32,
    pad_tail: [u8; 8],
}

/// SelectionClear — verified against xproto.h.
#[repr(C)]
struct SelectionClearEvent {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    time: u32,
    owner: u32,
    selection: u32,
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
const XCB_SELECTION_CLEAR: u8 = 29;
const XCB_SELECTION_REQUEST: u8 = 30;
const XCB_SELECTION_NOTIFY: u8 = 31;
const XCB_CLIENT_MESSAGE: u8 = 33;
const PROPERTY_NEW_VALUE: u8 = 0;

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
const EVENT_MASK_PROPERTY_CHANGE: u32 = 0x0040_0000;
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
    pub(crate) clipboard: u32,
    pub(crate) targets: u32,
    pub(crate) incr: u32,
    /// The property selections land on — ours by name, reused per read.
    pub(crate) transfer: u32,
    pub(crate) net_wm_moveresize: u32,
    pub(crate) net_wm_state: u32,
    pub(crate) net_wm_state_max_horz: u32,
    pub(crate) net_wm_state_max_vert: u32,
    pub(crate) motif_wm_hints: u32,
    pub(crate) wm_change_state: u32,
}

const ATOM_NAMES: [&CStr; 14] = [
    c"WM_PROTOCOLS",
    c"WM_DELETE_WINDOW",
    c"_NET_WM_NAME",
    c"UTF8_STRING",
    c"CLIPBOARD",
    c"TARGETS",
    c"INCR",
    c"BUNNY_SELECTION",
    c"_NET_WM_MOVERESIZE",
    c"_NET_WM_STATE",
    c"_NET_WM_STATE_MAXIMIZED_HORZ",
    c"_NET_WM_STATE_MAXIMIZED_VERT",
    c"_MOTIF_WM_HINTS",
    c"WM_CHANGE_STATE",
];

use std::ffi::CStr;

fn intern_atoms(connection: *mut Connection) -> Atoms {
    let cookies: Vec<Cookie> = ATOM_NAMES
        .iter()
        .map(|name| unsafe {
            xcb_intern_atom(connection, 0, name.to_bytes().len() as u16, name.as_ptr())
        })
        .collect();
    let mut atoms = [0u32; 14];
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
        clipboard: atoms[4],
        targets: atoms[5],
        incr: atoms[6],
        transfer: atoms[7],
        net_wm_moveresize: atoms[8],
        net_wm_state: atoms[9],
        net_wm_state_max_horz: atoms[10],
        net_wm_state_max_vert: atoms[11],
        motif_wm_hints: atoms[12],
        wm_change_state: atoms[13],
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
    /// Scene chrome: the shell owns the border — resize bands, the
    /// crown verbs and the rounded corners.
    scene: bool,
    /// The depth this window presents at (32 when the scene rides the
    /// ARGB ground for its corners; the root depth otherwise).
    depth: u8,
    /// Mirrored off _NET_WM_STATE — bands and corners stand down.
    maximized: bool,
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
    /// The shared xkb walk (states + compose); the keymap here comes
    /// from the device, and the SERVER repeats — the repeat fields
    /// inside stay idle on this door.
    keyboard: Keyboard,
    /// The xkb extension's event code — StateNotify arrives there.
    xkb_base_event: u8,
    /// The core cursor font and one lazily-made cursor per style.
    cursor_font: u32,
    cursors: [u32; 6],
    cursor_current: Option<Cursor>,
    /// Client-side double click — X sends plain buttons, the shell
    /// counts (the same 400 ms / 4 px window every platform keeps).
    clicks: ClickClock,
    /// Our claim on CLIPBOARD: the text this door serves. Cleared by
    /// SelectionClear when someone else takes the selection.
    source: Option<String>,
    /// The freshest input timestamp — selections want a REAL time
    /// (CurrentTime claims are second-class under ICCCM).
    last_time: u32,
    /// The 32-bit ground overlays paint on: depth, visual, colormap —
    /// found once; absent on a server without ARGB (panels go opaque).
    argb: Option<(u8, u32, u32)>,
    /// The overlay pool's windows; `WindowHandle(N)` is slot N−1.
    panels: Vec<Option<XPanel>>,
    /// The resize band under the pointer (0 = none) — it outranks the
    /// scene's own cursor while it holds.
    edge_hover: u32,
}

/// One overlay window: override-redirect, ARGB, placed in ROOT
/// coordinates — a popover hangs past every edge natively.
struct XPanel {
    window: u32,
    gc: u32,
    backing: Option<Backing>,
    /// The overlay's layout origin — the base for translating its
    /// surface-local pointer events back into scene coordinates.
    scene_origin: (f64, f64),
    /// The drag chip never takes input at all.
    chip: bool,
    mapped: bool,
    /// The last carved input inset, physical px — re-carved on resize.
    carved: (usize, usize),
    /// Where the panel sits on the root, physical px — the ground the
    /// outside-press dismissal measures against.
    root_rect: (i32, i32, i32, i32),
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

/// The X keymask, in the shell's own mapping: Control is the
/// accelerator and carries `command`, Mod1 carries `option`, and
/// `control` stays false — the same words the key road and the
/// Wayland backend use.
///
/// A press, a move and an entry all carry the mask, so all three read
/// it here and the three cannot drift.
fn held_modifiers(state: u16) -> bunny_ui::action::Modifiers {
    const SHIFT_MASK: u16 = 0x1;
    const CONTROL_MASK: u16 = 0x4;
    const MOD1_MASK: u16 = 0x8;
    bunny_ui::action::Modifiers {
        shift: state & SHIFT_MASK != 0,
        command: state & CONTROL_MASK != 0,
        option: state & MOD1_MASK != 0,
        control: false,
    }
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
    let (keyboard, xkb_base_event) = setup_keyboard(display);
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
            keyboard,
            xkb_base_event,
            cursor_font: 0,
            cursors: [0; 6],
            cursor_current: None,
            clicks: ClickClock::default(),
            source: None,
            last_time: 0,
            argb: None,
            panels: Vec::new(),
            edge_hover: 0,
        })
    });
    // the ARGB ground and the xfixes handshake wait for the screen
    // walk above — one extra pass, still inside connect
    with_x(|client| {
        client.argb = find_argb(client.connection);
        unsafe {
            let cookie = xcb_xfixes_query_version(client.connection, 6, 0);
            let reply =
                xcb_xfixes_query_version_reply(client.connection, cookie, std::ptr::null_mut());
            if !reply.is_null() {
                free(reply);
            }
        }
    });
}

/// Walks the screen's depths for the 32-bit TrueColor visual — the
/// ground overlays need for their shadow bleed. A colormap is minted
/// for it (a depth different from the parent demands one, and border
/// pixel besides — the classic BadMatch pair).
fn find_argb(connection: *mut Connection) -> Option<(u8, u32, u32)> {
    unsafe {
        let setup = xcb_get_setup(connection);
        let screens = xcb_setup_roots_iterator(setup);
        if screens.rem <= 0 || screens.data.is_null() {
            return None;
        }
        let screen = screens.data;
        let mut depths = xcb_screen_allowed_depths_iterator(screen);
        while depths.rem > 0 {
            let depth = depths.data;
            if (*depth).depth == 32 {
                let visuals = xcb_depth_visuals(depth);
                let count = xcb_depth_visuals_length(depth).max(0) as usize;
                for index in 0..count {
                    let visual = visuals.add(index);
                    const TRUE_COLOR: u8 = 4;
                    if (*visual).class == TRUE_COLOR {
                        let colormap = xcb_generate_id(connection);
                        xcb_create_colormap(
                            connection,
                            0, // AllocNone
                            colormap,
                            (*screen).root,
                            (*visual).visual_id,
                        );
                        return Some((32, (*visual).visual_id, colormap));
                    }
                }
            }
            xcb_depth_next(&mut depths);
        }
        None
    }
}

// MARK: - Clipboard (the selections dance)

/// Claims CLIPBOARD and keeps the text to serve. X wants no serial —
/// only a real timestamp and a live window.
pub(crate) fn clipboard_write(text: &str) {
    with_x(|client| {
        let Some(win) = client.win.as_ref() else { return };
        client.source = Some(text.to_string());
        unsafe {
            xcb_set_selection_owner(client.connection, win.id, client.atoms.clipboard, client.last_time);
            xcb_flush(client.connection);
        }
    });
}

/// Answers one SelectionRequest: TARGETS lists what this door speaks,
/// the text targets carry the bytes, anything else is refused with a
/// null property — and the notify goes back whatever happened.
fn serve_selection(event: &SelectionRequestEvent) {
    with_x(|client| {
        let atoms = &client.atoms;
        // obsolete requestors pass property None: the target names it
        let property = if event.property != 0 { event.property } else { event.target };
        let answered = match &client.source {
            Some(text) if event.selection == atoms.clipboard => unsafe {
                if event.target == atoms.targets {
                    let list = [atoms.targets, atoms.utf8_string, ATOM_STRING];
                    xcb_change_property(
                        client.connection,
                        PROP_MODE_REPLACE,
                        event.requestor,
                        property,
                        ATOM_ATOM,
                        32,
                        list.len() as u32,
                        list.as_ptr().cast(),
                    );
                    true
                } else if event.target == atoms.utf8_string || event.target == ATOM_STRING {
                    // direct write — the big-request road carries any
                    // realistic text; INCR out is the post-war stream
                    xcb_change_property(
                        client.connection,
                        PROP_MODE_REPLACE,
                        event.requestor,
                        property,
                        event.target,
                        8,
                        text.len() as u32,
                        text.as_ptr().cast(),
                    );
                    true
                } else {
                    false
                }
            },
            _ => false,
        };
        let notify = SelectionNotifyEvent {
            response_type: XCB_SELECTION_NOTIFY,
            pad0: 0,
            sequence: 0,
            time: event.time,
            requestor: event.requestor,
            selection: event.selection,
            target: event.target,
            property: if answered { property } else { 0 },
            pad_tail: [0; 8],
        };
        unsafe {
            xcb_send_event(
                client.connection,
                0,
                event.requestor,
                0,
                (&raw const notify).cast(),
            );
            xcb_flush(client.connection);
        }
    });
}

/// One bounded wait for a specific event during a selection read: the
/// wanted frame comes back, everything else queues for the main drain.
fn pump_for(
    deadline: Instant,
    mut wanted: impl FnMut(*mut GenericEvent) -> bool,
) -> Option<*mut GenericEvent> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let (connection, fd) = with_x(|client| {
            (client.connection, unsafe { xcb_get_file_descriptor(client.connection) })
        });
        loop {
            let event = unsafe { xcb_poll_for_event(connection) };
            if event.is_null() {
                break;
            }
            if wanted(event) {
                return Some(event);
            }
            PENDING.with(|q| q.borrow_mut().push_back(event));
        }
        let mut fds = [PollFd { fd, events: POLLIN, revents: 0 }];
        unsafe {
            xcb_flush(connection);
            poll(fds.as_mut_ptr(), 1, remaining.as_millis().min(200) as c_int);
        }
    }
}

/// Reads one property chunk (deleting it — the INCR handshake) and
/// appends; answers (bytes_appended, was_incr_header).
fn take_property(window: u32, out: &mut Vec<u8>) -> (usize, bool) {
    with_x(|client| unsafe {
        let cookie = xcb_get_property(
            client.connection,
            1, // delete — the reader's ack in the INCR protocol
            window,
            client.atoms.transfer,
            0, // AnyPropertyType
            0,
            u32::MAX / 4,
        );
        let reply = xcb_get_property_reply(client.connection, cookie, std::ptr::null_mut());
        if reply.is_null() {
            return (0, false);
        }
        let incr = (*reply).kind == client.atoms.incr;
        let length = xcb_get_property_value_length(reply).max(0) as usize;
        if !incr && length > 0 {
            let bytes = std::slice::from_raw_parts(
                xcb_get_property_value(reply) as *const u8,
                length,
            );
            out.extend_from_slice(bytes);
        }
        free(reply.cast());
        xcb_flush(client.connection);
        (length, incr)
    })
}

/// The main window's id — what a system panel needs to hang from, so
/// the desktop can make it modal to the app that asked. `None` before
/// the window is up.
pub(crate) fn main_window() -> Option<u32> {
    with_x(|client| client.win.as_ref().map(|win| win.id))
}

/// Reads the CLIPBOARD selection. Our own claim answers from memory;
/// a peer's arrives by convert → notify → property, INCR-streamed
/// when large, all under the hard four-second cap the wayland door
/// keeps — a hung owner must never hang the UI thread.
pub(crate) fn clipboard_read() -> Option<String> {
    // the self short-circuit: we are the owner, no round trip
    let (own, window) = with_x(|client| {
        (client.source.clone(), client.win.as_ref().map(|w| w.id))
    });
    if own.is_some() {
        return own;
    }
    let window = window?;
    with_x(|client| unsafe {
        xcb_convert_selection(
            client.connection,
            window,
            client.atoms.clipboard,
            client.atoms.utf8_string,
            client.atoms.transfer,
            client.last_time,
        );
        xcb_flush(client.connection);
    });
    let deadline = Instant::now() + std::time::Duration::from_secs(4);
    let notify = pump_for(deadline, |event| {
        let kind = unsafe { (*event).response_type } & 0x7F;
        kind == XCB_SELECTION_NOTIFY
            && unsafe { (*(event as *mut SelectionNotifyEvent)).requestor } == window
    })?;
    let property = unsafe { (*(notify as *mut SelectionNotifyEvent)).property };
    unsafe { free(notify.cast()) };
    if property == 0 {
        return None; // the owner refused the target
    }
    let mut bytes = Vec::new();
    let (_, incr) = take_property(window, &mut bytes);
    if incr {
        // the streamed road: each PropertyNotify(NewValue) carries a
        // chunk; the empty chunk closes the stream
        loop {
            let chunk_event = pump_for(deadline, |event| {
                let kind = unsafe { (*event).response_type } & 0x7F;
                if kind != XCB_PROPERTY_NOTIFY {
                    return false;
                }
                let notify = event as *mut PropertyNotifyEvent;
                unsafe {
                    (*notify).window == window && (*notify).state == PROPERTY_NEW_VALUE
                }
            })?;
            unsafe { free(chunk_event.cast()) };
            let (appended, _) = take_property(window, &mut bytes);
            if appended == 0 {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    (!text.is_empty()).then_some(text)
}

// MARK: - Keyboard (xkbcommon-x11: the device keymap, server repeat)

/// Builds the whole xkb walk off the server's core keyboard: keymap
/// and state from the device, the mods-blind scratch, the locale
/// compose table — and flips detectable autorepeat so a HELD key
/// arrives as repeated presses with no releases between (the server
/// repeats; the client machinery of the first door stands down).
fn setup_keyboard(connection: *mut Connection) -> (Keyboard, u8) {
    let mut keyboard = Keyboard::new();
    let mut base_event = 0u8;
    unsafe {
        let (mut major, mut minor, mut base_error) = (0u16, 0u16, 0u8);
        if xkb_x11_setup_xkb_extension(
            connection,
            1,
            0,
            0,
            &mut major,
            &mut minor,
            &mut base_event,
            &mut base_error,
        ) == 0
        {
            eprintln!("bunny_ui x11: no XKB extension — keys stay silent");
            return (keyboard, 0);
        }
        keyboard.context = crate::ffi::xkb_context_new(0);
        if keyboard.context.is_null() {
            return (keyboard, base_event);
        }
        let device = xkb_x11_get_core_keyboard_device_id(connection);
        if device >= 0 {
            keyboard.keymap =
                xkb_x11_keymap_new_from_device(keyboard.context, connection, device, 0);
        }
        if !keyboard.keymap.is_null() {
            keyboard.state = xkb_x11_state_new_from_device(keyboard.keymap, connection, device);
            // the mods-blind twin state — the chars_ignoring road
            keyboard.scratch = crate::ffi::xkb_state_new(keyboard.keymap);
        }
        // the locale drives the dead-key table; empty falls to C
        let locale = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LC_CTYPE"))
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_else(|_| "C".into());
        if let Ok(locale_c) = std::ffi::CString::new(locale) {
            let table = crate::ffi::xkb_compose_table_new_from_locale(
                keyboard.context,
                locale_c.as_ptr(),
                0,
            );
            if !table.is_null() {
                keyboard.compose = crate::ffi::xkb_compose_state_new(table, 0);
                crate::ffi::xkb_compose_table_unref(table);
            }
        }
        // server-side repeat, made visible: presses repeat, releases
        // only land when the finger truly lifts
        let cookie = xcb_xkb_per_client_flags(
            connection,
            XKB_ID_USE_CORE_KBD,
            XKB_PER_CLIENT_FLAG_DETECTABLE_AUTO_REPEAT,
            XKB_PER_CLIENT_FLAG_DETECTABLE_AUTO_REPEAT,
            0,
            0,
            0,
        );
        let reply =
            xcb_xkb_per_client_flags_reply(connection, cookie, std::ptr::null_mut());
        if !reply.is_null() {
            free(reply);
        }
        // modifier truth flows through StateNotify, not per-key guesses
        xcb_xkb_select_events(
            connection,
            XKB_ID_USE_CORE_KBD,
            XKB_EVENT_TYPE_STATE_NOTIFY,
            0,
            XKB_EVENT_TYPE_STATE_NOTIFY,
            0,
            0,
            std::ptr::null(),
        );
        xcb_flush(connection);
    }
    (keyboard, base_event)
}

// MARK: - Cursor (the core font tier: universal, theme-blind)

/// The glyph each style wears in the core `cursor` font (source; the
/// mask is always glyph+1).
fn glyph_of(cursor: Cursor) -> u16 {
    match cursor {
        Cursor::Arrow => 68,            // left_ptr
        Cursor::Pointing => 60,         // hand2
        Cursor::ResizeLeftRight => 108, // sb_h_double_arrow
        Cursor::ResizeUpDown => 116,    // sb_v_double_arrow
        Cursor::ResizeNwSe => 134,      // top_left_corner
        Cursor::ResizeNeSw => 136,      // top_right_corner
    }
}

fn cursor_slot(cursor: Cursor) -> usize {
    match cursor {
        Cursor::Arrow => 0,
        Cursor::Pointing => 1,
        Cursor::ResizeLeftRight => 2,
        Cursor::ResizeUpDown => 3,
        Cursor::ResizeNwSe => 4,
        Cursor::ResizeNeSw => 5,
    }
}

pub(crate) fn set_cursor(cursor: Cursor) {
    let changed = with_x(|client| {
        let was = client.cursor_current;
        client.cursor_current = Some(cursor);
        was != Some(cursor)
    });
    if changed {
        apply_current_cursor();
    }
}

/// Applies the effective cursor: a live border band outranks the
/// scene's own choice (the crown's certified override).
fn apply_current_cursor() {
    with_x(|client| {
        let cursor = if client.edge_hover != 0 {
            crate::ffi::edge_cursor(client.edge_hover)
        } else {
            client.cursor_current.unwrap_or(Cursor::Arrow)
        };
        let Some(win) = client.win.as_ref() else { return };
        unsafe {
            if client.cursor_font == 0 {
                client.cursor_font = xcb_generate_id(client.connection);
                let name = c"cursor";
                xcb_open_font(
                    client.connection,
                    client.cursor_font,
                    name.to_bytes().len() as u16,
                    name.as_ptr(),
                );
            }
            let slot = cursor_slot(cursor);
            if client.cursors[slot] == 0 {
                let id = xcb_generate_id(client.connection);
                let glyph = glyph_of(cursor);
                xcb_create_glyph_cursor(
                    client.connection,
                    id,
                    client.cursor_font,
                    client.cursor_font,
                    glyph,
                    glyph + 1,
                    0,
                    0,
                    0,
                    u16::MAX,
                    u16::MAX,
                    u16::MAX,
                );
                client.cursors[slot] = id;
            }
            const CW_CURSOR: u32 = 0x4000;
            let values = [client.cursors[slot]];
            xcb_change_window_attributes(client.connection, win.id, CW_CURSOR, values.as_ptr());
            xcb_flush(client.connection);
        }
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
            let events = EVENT_MASK_KEY_PRESS
                | EVENT_MASK_KEY_RELEASE
                | EVENT_MASK_BUTTON_PRESS
                | EVENT_MASK_BUTTON_RELEASE
                | EVENT_MASK_ENTER_WINDOW
                | EVENT_MASK_LEAVE_WINDOW
                | EVENT_MASK_POINTER_MOTION
                | EVENT_MASK_EXPOSURE
                | EVENT_MASK_STRUCTURE_NOTIFY
                | EVENT_MASK_FOCUS_CHANGE
                | EVENT_MASK_PROPERTY_CHANGE;
            const CW_BORDER_PIXEL: u32 = 0x0008;
            const CW_COLORMAP: u32 = 0x2000;
            // the scene rounds its corners — it needs the ARGB ground
            // (with the border-pixel/colormap pair a foreign depth
            // demands); everything else stays on the root visual
            let scene_ground = scene.then_some(client.argb).flatten();
            let (depth, visual, mask, values): (u8, u32, u32, Vec<u32>) = match scene_ground {
                Some((depth, visual, colormap)) => (
                    depth,
                    visual,
                    CW_BACK_PIXEL | CW_BORDER_PIXEL | CW_EVENT_MASK | CW_COLORMAP,
                    vec![0, 0, events, colormap],
                ),
                None => (
                    0, // CopyFromParent
                    client.root_visual,
                    CW_BACK_PIXEL | CW_EVENT_MASK,
                    vec![back, events],
                ),
            };
            xcb_create_window(
                client.connection,
                depth,
                id,
                client.root,
                0,
                0,
                physical.0.max(1),
                physical.1.max(1),
                0,
                WINDOW_CLASS_INPUT_OUTPUT,
                visual,
                mask,
                values.as_ptr(),
            );
            if scene {
                // the WM's own decorations stand down — the scene
                // draws the bar and the crown answers the verbs
                let hints: [u32; 5] = [2, 0, 0, 0, 0]; // flags=DECORATIONS, none
                xcb_change_property(
                    client.connection,
                    PROP_MODE_REPLACE,
                    id,
                    client.atoms.motif_wm_hints,
                    client.atoms.motif_wm_hints,
                    32,
                    5,
                    hints.as_ptr().cast(),
                );
            }
            let window_depth =
                if depth == 0 { client.root_depth } else { depth };
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
                depth: window_depth,
                maximized: false,
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

/// Asks the road to end — the app's `close` on the one window.
pub(crate) fn ask_quit() {
    with_x(|client| client.quit = true);
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
        let argb = client.argb.is_some();
        let win = client.win.as_mut().expect("window for the present");
        let depth = win.depth;
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
        }
        // the scene's corners round through the same premultiplied
        // mask the wayland door wears — only on the ARGB ground
        if win.scene && argb {
            crate::ffi::mask_corners(backing.map, width, height, 8.0 * win.scale as f64);
        }
        for &rect in damage {
            let Some((x, y, w, h)) = clamp_rect(rect, width, height) else { continue };
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
                    depth,
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
        let Some(win) = client.win.as_ref() else { return };
        let root_depth = win.depth;
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

// MARK: - Overlays (override-redirect ARGB windows in root coordinates)

/// The bleed lib.rs inflates every overlay by — the shadow ring. The
/// input carve gives that ring back to whatever lies under it.
const PANEL_BLEED: f64 = 32.0;

/// Claims a pool slot and builds its window: override-redirect (the
/// WM never decorates or moves it), ARGB when the ground offers it,
/// its own input events. Returns the handle index (slot + 1).
pub(crate) fn create_panel(chip: bool) -> usize {
    with_x(|client| {
        let (depth, visual, colormap) = client
            .argb
            .unwrap_or((0, client.root_visual, 0));
        unsafe {
            let id = xcb_generate_id(client.connection);
            const CW_BORDER_PIXEL: u32 = 0x0008;
            const CW_OVERRIDE_REDIRECT: u32 = 0x0200;
            const CW_COLORMAP: u32 = 0x2000;
            // value order follows the mask's bit order — the protocol law
            let (mask, values): (u32, Vec<u32>) = if client.argb.is_some() {
                (
                    CW_BACK_PIXEL
                        | CW_BORDER_PIXEL
                        | CW_OVERRIDE_REDIRECT
                        | CW_EVENT_MASK
                        | CW_COLORMAP,
                    vec![
                        0, // transparent ground
                        0, // border pixel — the BadMatch guard
                        1, // override-redirect
                        EVENT_MASK_BUTTON_PRESS
                            | EVENT_MASK_BUTTON_RELEASE
                            | EVENT_MASK_ENTER_WINDOW
                            | EVENT_MASK_LEAVE_WINDOW
                            | EVENT_MASK_POINTER_MOTION
                            | EVENT_MASK_EXPOSURE,
                        colormap,
                    ],
                )
            } else {
                (
                    CW_BACK_PIXEL | CW_OVERRIDE_REDIRECT | CW_EVENT_MASK,
                    vec![
                        0,
                        1,
                        EVENT_MASK_BUTTON_PRESS
                            | EVENT_MASK_BUTTON_RELEASE
                            | EVENT_MASK_ENTER_WINDOW
                            | EVENT_MASK_LEAVE_WINDOW
                            | EVENT_MASK_POINTER_MOTION
                            | EVENT_MASK_EXPOSURE,
                    ],
                )
            };
            xcb_create_window(
                client.connection,
                depth,
                id,
                client.root,
                0,
                0,
                1,
                1,
                0,
                WINDOW_CLASS_INPUT_OUTPUT,
                visual,
                mask,
                values.as_ptr(),
            );
            let gc = xcb_generate_id(client.connection);
            xcb_create_gc(client.connection, gc, id, 0, std::ptr::null());
            let panel = XPanel {
                window: id,
                gc,
                backing: None,
                scene_origin: (0.0, 0.0),
                chip,
                mapped: false,
                carved: (0, 0),
                root_rect: (0, 0, 0, 0),
            };
            let slot = client.panels.iter().position(Option::is_none);
            match slot {
                Some(index) => {
                    client.panels[index] = Some(panel);
                    index + 1
                }
                None => {
                    client.panels.push(Some(panel));
                    client.panels.len()
                }
            }
        }
    })
}

pub(crate) fn set_scene_origin(index: usize, x: f64, y: f64) {
    with_x(|client| {
        if let Some(Some(panel)) = client.panels.get_mut(index) {
            panel.scene_origin = (x, y);
        }
    });
}

/// The window's own origin in root pixels — the truth every overlay
/// placement builds on.
fn window_root_origin(client: &mut XClient) -> (i32, i32) {
    let Some(win) = client.win.as_ref() else { return (0, 0) };
    unsafe {
        let cookie = xcb_translate_coordinates(client.connection, win.id, client.root, 0, 0);
        let reply =
            xcb_translate_coordinates_reply(client.connection, cookie, std::ptr::null_mut());
        if reply.is_null() {
            return (0, 0);
        }
        let origin = ((*reply).dst_x as i32, (*reply).dst_y as i32);
        free(reply.cast());
        origin
    }
}

/// Position, size and pixels land together: the straight RGBA slice
/// premultiplies into ARGB on the way in (the fused pass every door
/// keeps), the window configures in ROOT pixels and maps on its
/// first present. The chip takes no input at all; a popover gives
/// its bleed ring back through the input carve.
pub(crate) fn panel_present(
    index: usize,
    rect: (f64, f64, f64, f64),
    width: usize,
    height: usize,
    rgba: &[u8],
) {
    with_x(|client| {
        let connection = client.connection;
        let scale = client.win.as_ref().map(|w| w.scale).unwrap_or(1);
        let Some(Some(panel)) = client.panels.get_mut(index) else { return };
        // the backing follows the size
        let stale = panel
            .backing
            .as_ref()
            .is_none_or(|backing| backing.width != width || backing.height != height);
        if stale {
            if let Some(old) = panel.backing.take() {
                unsafe { drop_backing(connection, old) };
            }
            panel.backing = make_backing(connection, width, height);
        }
        let Some(backing) = panel.backing.as_ref() else { return };
        // premultiply RGBA → BGRA in one pass (little-endian ARGB32)
        let pixels = rgba.len().min(backing.len) / 4;
        unsafe {
            let target = std::slice::from_raw_parts_mut(backing.map, pixels * 4);
            for (source_px, target_px) in
                rgba.chunks_exact(4).take(pixels).zip(target.chunks_exact_mut(4))
            {
                let alpha = source_px[3] as u32;
                target_px[0] = ((source_px[2] as u32 * alpha + 127) / 255) as u8;
                target_px[1] = ((source_px[1] as u32 * alpha + 127) / 255) as u8;
                target_px[2] = ((source_px[0] as u32 * alpha + 127) / 255) as u8;
                target_px[3] = alpha as u8;
            }
        }
        // root placement: the rect arrives ALREADY in logical root
        // coordinates (layout_rect_to_screen added the window origin);
        // device pixels from here, always stacked above
        let x = (rect.0 * scale as f64).round() as i32;
        let y = (rect.1 * scale as f64).round() as i32;
        panel.root_rect = (x, y, width as i32, height as i32);
        unsafe {
            const CONFIG_X: u16 = 1;
            const CONFIG_Y: u16 = 2;
            const CONFIG_W: u16 = 4;
            const CONFIG_H: u16 = 8;
            const CONFIG_STACK: u16 = 64;
            let values = [
                x as u32,
                y as u32,
                width.max(1) as u32,
                height.max(1) as u32,
                0, // Above
            ];
            xcb_configure_window(
                connection,
                panel.window,
                CONFIG_X | CONFIG_Y | CONFIG_W | CONFIG_H | CONFIG_STACK,
                values.as_ptr(),
            );
            // the input carve: the chip passes everything through; a
            // popover keeps its content and returns the bleed ring
            if panel.carved != (width, height) {
                panel.carved = (width, height);
                let region = xcb_generate_id(connection);
                if panel.chip {
                    xcb_xfixes_create_region(connection, region, 0, std::ptr::null());
                } else {
                    let inset = (PANEL_BLEED * scale as f64).round() as i64;
                    let rect = Rectangle {
                        x: inset.min(i16::MAX as i64) as i16,
                        y: inset.min(i16::MAX as i64) as i16,
                        width: (width as i64 - 2 * inset).max(0) as u16,
                        height: (height as i64 - 2 * inset).max(0) as u16,
                    };
                    xcb_xfixes_create_region(connection, region, 1, &rect);
                }
                xcb_xfixes_set_window_shape_region(
                    connection,
                    panel.window,
                    SHAPE_KIND_INPUT,
                    0,
                    0,
                    region,
                );
                xcb_xfixes_destroy_region(connection, region);
            }
            let depth = client.argb.map(|(depth, _, _)| depth).unwrap_or(client.root_depth);
            xcb_shm_put_image(
                connection,
                panel.window,
                panel.gc,
                width as u16,
                height as u16,
                0,
                0,
                width as u16,
                height as u16,
                0,
                0,
                depth,
                IMAGE_FORMAT_Z_PIXMAP,
                0,
                backing.segment,
                0,
            );
            if !panel.mapped {
                xcb_map_window(connection, panel.window);
                panel.mapped = true;
            }
            xcb_flush(connection);
        }
    });
}

/// Hide and forget: the pool retires a panel whose overlay closed.
pub(crate) fn close_panel(index: usize) {
    with_x(|client| {
        let connection = client.connection;
        if let Some(slot) = client.panels.get_mut(index) {
            if let Some(panel) = slot.take() {
                unsafe {
                    xcb_unmap_window(connection, panel.window);
                    if let Some(backing) = panel.backing {
                        drop_backing(connection, backing);
                    }
                    xcb_destroy_window(connection, panel.window);
                    xcb_flush(connection);
                }
            }
        }
    });
}

/// The main window's origin in LOGICAL root coordinates — the base
/// `layout_rect_to_screen` adds to, and the bounds math subtracts.
pub(crate) fn window_origin_logical() -> (f64, f64) {
    with_x(|client| {
        let scale = client.win.as_ref().map(|w| w.scale).unwrap_or(1) as f64;
        let origin = window_root_origin(client);
        (origin.0 as f64 / scale, origin.1 as f64 / scale)
    })
}

/// The whole root, in layout coordinates relative to the window — the
/// REAL screen bounds the placement math clamps against (popovers may
/// hang past the window; the screen edge is the only wall).
pub(crate) fn screen_bounds_in_layout() -> Option<(f64, f64, f64, f64)> {
    with_x(|client| {
        let scale = client.win.as_ref().map(|w| w.scale).unwrap_or(1) as f64;
        let origin = window_root_origin(client);
        unsafe {
            let setup = xcb_get_setup(client.connection);
            let screens = xcb_setup_roots_iterator(setup);
            if screens.rem <= 0 || screens.data.is_null() {
                return None;
            }
            let screen = &*screens.data;
            Some((
                -(origin.0 as f64) / scale,
                -(origin.1 as f64) / scale,
                screen.width_in_pixels as f64 / scale,
                screen.height_in_pixels as f64 / scale,
            ))
        }
    })
}

// MARK: - The crown (EWMH verbs — the WM moves, sizes and stacks)

/// The xdg edge bitfield spoken as an EWMH moveresize direction.
fn moveresize_direction(edge: u32) -> u32 {
    match edge {
        5 => 0,  // top-left
        1 => 1,  // top
        9 => 2,  // top-right
        8 => 3,  // right
        10 => 4, // bottom-right
        2 => 5,  // bottom
        6 => 6,  // bottom-left
        4 => 7,  // left
        _ => 8,  // move
    }
}

const SUBSTRUCTURE_MASKS: u32 = 0x0008_0000 | 0x0010_0000; // notify | redirect

/// Sends one client message to the root the way EWMH wants: both
/// substructure masks, format 32.
fn send_root_message(client: &mut XClient, window: u32, kind: u32, data: [u32; 5]) {
    let message = ClientMessageEvent {
        response_type: XCB_CLIENT_MESSAGE,
        format: 32,
        sequence: 0,
        window,
        kind,
        data32: data,
    };
    unsafe {
        xcb_send_event(
            client.connection,
            0,
            client.root,
            SUBSTRUCTURE_MASKS,
            (&raw const message).cast(),
        );
        xcb_flush(client.connection);
    }
}

/// Executes a crown verb through the WM. The implicit press grab is
/// released first — a moveresize under our own grab never moves.
fn crown_execute(take: crate::ffi::CrownTake, root_x: i16, root_y: i16) -> bool {
    use crate::ffi::{ControlHit, CrownTake};
    with_x(|client| {
        let Some(win) = client.win.as_ref() else { return false };
        let (id, time) = (win.id, client.last_time);
        let atoms_moveresize = client.atoms.net_wm_moveresize;
        let atoms_state = client.atoms.net_wm_state;
        let max_pair = (client.atoms.net_wm_state_max_horz, client.atoms.net_wm_state_max_vert);
        let change_state = client.atoms.wm_change_state;
        let moveresize = |client: &mut XClient, direction: u32| {
            unsafe { xcb_ungrab_pointer(client.connection, time) };
            send_root_message(
                client,
                id,
                atoms_moveresize,
                [root_x as u32, root_y as u32, direction, 1, 1],
            );
        };
        match take {
            CrownTake::None => false,
            CrownTake::Menu => false, // no WM menu on this door — the scene's own may answer
            CrownTake::Move => {
                moveresize(client, 8);
                true
            }
            CrownTake::Resize(edge) => {
                moveresize(client, moveresize_direction(edge));
                true
            }
            CrownTake::Control(ControlHit::Close) => {
                client.quit = true;
                true
            }
            CrownTake::Control(ControlHit::Minimize) => {
                const ICONIC: u32 = 3;
                send_root_message(client, id, change_state, [ICONIC, 0, 0, 0, 0]);
                true
            }
            CrownTake::Control(ControlHit::Maximize) | CrownTake::ToggleMaximize => {
                const TOGGLE: u32 = 2;
                send_root_message(
                    client,
                    id,
                    atoms_state,
                    [TOGGLE, max_pair.0, max_pair.1, 0, 1],
                );
                true
            }
        }
    })
}

/// Re-reads _NET_WM_STATE off the main window — the maximized mirror
/// bands and corners consult.
fn refresh_wm_state(client: &mut XClient) {
    let Some(win) = client.win.as_ref() else { return };
    let (id, state_atom) = (win.id, client.atoms.net_wm_state);
    let max_pair = (client.atoms.net_wm_state_max_horz, client.atoms.net_wm_state_max_vert);
    unsafe {
        let cookie =
            xcb_get_property(client.connection, 0, id, state_atom, ATOM_ATOM, 0, 64);
        let reply = xcb_get_property_reply(client.connection, cookie, std::ptr::null_mut());
        if reply.is_null() {
            return;
        }
        let count = (xcb_get_property_value_length(reply).max(0) as usize) / 4;
        let atoms = std::slice::from_raw_parts(
            xcb_get_property_value(reply) as *const u32,
            count,
        );
        let maximized = atoms.contains(&max_pair.0) || atoms.contains(&max_pair.1);
        free(reply.cast());
        if let Some(win) = client.win.as_mut() {
            win.maximized = maximized;
        }
    }
}

// MARK: - The gpu graft (the x11 side of gl.rs)

/// What the EGL surface wraps on this door: the xcb connection and
/// the window xid (Mesa's xcb platform speaks both natively).
pub(crate) fn gpu_targets() -> Option<crate::ffi::GpuTargets> {
    with_x(|client| {
        client.win.as_ref().map(|win| crate::ffi::GpuTargets::X11 {
            connection: client.connection.cast(),
            window: win.id,
            scene: win.scene,
        })
    })
}

pub(crate) fn gpu_buffer_size() -> (usize, usize) {
    with_x(|client| {
        client.win.as_ref().map_or((1, 1), |win| {
            let scale = win.scale.max(1) as f64;
            (
                (win.logical.0 * scale).round().max(1.0) as usize,
                (win.logical.1 * scale).round().max(1.0) as usize,
            )
        })
    })
}

/// The only present gate this door needs: a living window. No map
/// dance, no callback — the swap presents whenever it likes.
pub(crate) fn gpu_can_present() -> bool {
    with_x(|client| client.win.is_some())
}

pub(crate) fn gpu_note_present() {
    with_x(|client| client.presents += 1);
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

/// Which of our surfaces an input event landed on: the main window
/// translates from (0,0); a panel from its scene origin — hit-testing
/// follows the overlay math, not the server's window tree. Also the
/// one place every input event stamps the selection timestamp.
fn surface_base(window: u32, time: u32) -> Option<((f64, f64), f64)> {
    with_x(|client| {
        client.last_time = time;
        let scale = client.win.as_ref().map(|w| w.scale).unwrap_or(1) as f64;
        if client.win.as_ref().is_some_and(|w| w.id == window) {
            return Some(((0.0, 0.0), scale));
        }
        client
            .panels
            .iter()
            .flatten()
            .find(|panel| panel.window == window)
            .map(|panel| (panel.scene_origin, scale))
    })
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
            let (window, x, y, time, state) = unsafe {
                (
                    (*motion).event,
                    (*motion).event_x,
                    (*motion).event_y,
                    (*motion).time,
                    (*motion).state,
                )
            };
            let Some((base, scale)) = surface_base(window, time) else {
                return Step::Silence;
            };
            // a border band under the pointer outranks the scene's
            // own cursor while it holds
            let band_change = with_x(|client| {
                client.pointer_pos = (x as f64, y as f64);
                let win = client.win.as_ref()?;
                let edge = if win.id != window || !win.scene || win.maximized {
                    0
                } else {
                    crate::ffi::resize_edge_of(
                        x as f64 / win.scale as f64,
                        y as f64 / win.scale as f64,
                        win.logical.0,
                        win.logical.1,
                    )
                };
                let was = client.edge_hover;
                client.edge_hover = edge;
                (was != edge).then_some(())
            });
            if band_change.is_some() {
                apply_current_cursor();
            }
            Step::Deliver(AppEvent::MouseMoved {
                x: base.0 + x as f64 / scale,
                y: base.1 + y as f64 / scale,
                modifiers: held_modifiers(state),
            })
        }
        XCB_BUTTON_PRESS | XCB_BUTTON_RELEASE => {
            let button = event as *mut InputEvent;
            let (window, detail, x, y, state, time, root_x, root_y) = unsafe {
                (
                    (*button).event,
                    (*button).detail,
                    (*button).event_x,
                    (*button).event_y,
                    (*button).state,
                    (*button).time,
                    (*button).root_x,
                    (*button).root_y,
                )
            };
            let Some((base, scale)) = surface_base(window, time) else {
                return Step::Silence;
            };
            // the crown outranks the scene: a press on a scene-chrome
            // main window asks the border bands first, then the drag
            // and control gates — a consumed press never reaches the
            // engine (the certified order of every door)
            if kind == XCB_BUTTON_PRESS && matches!(detail, 1 | 3) {
                let crown = with_x(|client| {
                    let win = client.win.as_ref()?;
                    if win.id != window || !win.scene {
                        return None;
                    }
                    let logical_x = x as f64 / win.scale as f64;
                    let logical_y = y as f64 / win.scale as f64;
                    if detail == 1 && !win.maximized {
                        let edge = crate::ffi::resize_edge_of(
                            logical_x,
                            logical_y,
                            win.logical.0,
                            win.logical.1,
                        );
                        if edge != 0 {
                            return Some(crate::ffi::CrownTake::Resize(edge));
                        }
                    }
                    Some(crate::ffi::crown_take(
                        logical_x,
                        logical_y,
                        1,
                        detail == 3,
                    ))
                });
                if let Some(take) = crown {
                    // clicks for the double-click maximize: recount
                    // through the shared clock on the raw press
                    let take = if matches!(take, crate::ffi::CrownTake::Move) {
                        let clicks = with_x(|client| {
                            client.clicks.click(time, root_x as f64, root_y as f64)
                        });
                        if clicks >= 2 {
                            crate::ffi::CrownTake::ToggleMaximize
                        } else {
                            take
                        }
                    } else {
                        take
                    };
                    if crown_execute(take, root_x, root_y) {
                        return Step::Silence;
                    }
                }
            }
            // no compositor grab exists on this door: a press on the
            // MAIN window while a popover floats — outside all of them
            // — dismisses first, then lands as its own event
            if kind == XCB_BUTTON_PRESS && matches!(detail, 1 | 3) {
                let outside_all = with_x(|client| {
                    let on_main = client.win.as_ref().is_some_and(|w| w.id == window);
                    let inset =
                        (PANEL_BLEED * client.win.as_ref().map(|w| w.scale).unwrap_or(1) as f64)
                            .round() as i32;
                    on_main
                        && client.panels.iter().flatten().any(|panel| panel.mapped && !panel.chip)
                        && client.panels.iter().flatten().all(|panel| {
                            if !panel.mapped || panel.chip {
                                return true;
                            }
                            // the CONTENT box decides — the bleed ring
                            // is click-through by carve, so a press on
                            // it is visually outside the card
                            let (px, py, pw, ph) = panel.root_rect;
                            let (px, py) = (px + inset, py + inset);
                            let (pw, ph) = ((pw - 2 * inset).max(0), (ph - 2 * inset).max(0));
                            let (rx, ry) = (root_x as i32, root_y as i32);
                            rx < px || ry < py || rx >= px + pw || ry >= py + ph
                        })
                });
                if outside_all {
                    dispatch(AppEvent::DismissOverlays);
                }
            }
            let (x, y) = (base.0 + x as f64 / scale, base.1 + y as f64 / scale);
            match (kind, detail) {
                (XCB_BUTTON_PRESS, 1) => {
                    let clicks = with_x(|client| client.clicks.click(time, x, y));
                    Step::Deliver(AppEvent::MouseDown {
                        x,
                        y,
                        clicks,
                        modifiers: held_modifiers(state),
                    })
                }
                (XCB_BUTTON_RELEASE, 1) => Step::Deliver(AppEvent::MouseUp { x, y }),
                (XCB_BUTTON_PRESS, 3) => Step::Deliver(AppEvent::RightMouseDown { x, y }),
                // the wheel speaks buttons: one press per detent, the
                // ×16 line doctrine, up positive toward the engine
                (XCB_BUTTON_PRESS, 4) => {
                    Step::Deliver(AppEvent::Wheel { x, y, dx: 0.0, dy: 16.0 })
                }
                (XCB_BUTTON_PRESS, 5) => {
                    Step::Deliver(AppEvent::Wheel { x, y, dx: 0.0, dy: -16.0 })
                }
                (XCB_BUTTON_PRESS, 6) => {
                    Step::Deliver(AppEvent::Wheel { x, y, dx: 16.0, dy: 0.0 })
                }
                (XCB_BUTTON_PRESS, 7) => {
                    Step::Deliver(AppEvent::Wheel { x, y, dx: -16.0, dy: 0.0 })
                }
                _ => Step::Silence,
            }
        }
        XCB_KEY_PRESS => {
            // detectable autorepeat holds: a held key arrives as
            // repeated presses — each walks the same road the first
            // door's timer used to walk
            let (keycode, time) =
                unsafe { ((*(event as *mut InputEvent)).detail, (*(event as *mut InputEvent)).time) };
            let road = with_x(|client| {
                client.last_time = time;
                key_road(&mut client.keyboard, keycode as u32)
            });
            deliver_key(road);
            Step::Silence
        }
        XCB_KEY_RELEASE => Step::Silence,
        XCB_ENTER_NOTIFY => {
            let crossing = event as *mut CrossingEvent;
            let (window, x, y, time, state) = unsafe {
                (
                    (*crossing).event,
                    (*crossing).event_x,
                    (*crossing).event_y,
                    (*crossing).time,
                    (*crossing).state,
                )
            };
            let Some((base, scale)) = surface_base(window, time) else {
                return Step::Silence;
            };
            with_x(|client| client.pointer_pos = (x as f64, y as f64));
            Step::Deliver(AppEvent::MouseMoved {
                x: base.0 + x as f64 / scale,
                y: base.1 + y as f64 / scale,
                modifiers: held_modifiers(state),
            })
        }
        XCB_LEAVE_NOTIFY => {
            // leaving a PANEL usually means entering the window (or a
            // sibling) — only the main window's leave exits the scene
            let window = unsafe { (*(event as *mut CrossingEvent)).event };
            let main = with_x(|client| {
                client.win.as_ref().is_some_and(|w| w.id == window)
            });
            if main {
                Step::Deliver(AppEvent::MouseExited)
            } else {
                Step::Silence
            }
        }
        XCB_FOCUS_OUT => Step::Deliver(AppEvent::ResignKey),
        XCB_SELECTION_REQUEST => {
            let request = unsafe { std::ptr::read(event as *const SelectionRequestEvent) };
            serve_selection(&request);
            Step::Silence
        }
        XCB_SELECTION_CLEAR => {
            // someone else took the selection — our claim is over
            let cleared = unsafe { (*(event as *mut SelectionClearEvent)).selection };
            with_x(|client| {
                if cleared == client.atoms.clipboard {
                    client.source = None;
                }
            });
            Step::Silence
        }
        // a stray notify outside a read pump answers nothing
        XCB_SELECTION_NOTIFY => Step::Silence,
        XCB_PROPERTY_NOTIFY => {
            // the WM writes _NET_WM_STATE on the main window; the
            // maximized mirror keeps bands and corners honest
            let (window, atom) = unsafe {
                let notify = event as *mut PropertyNotifyEvent;
                ((*notify).window, (*notify).atom)
            };
            with_x(|client| {
                let interesting = client.win.as_ref().is_some_and(|w| w.id == window)
                    && atom == client.atoms.net_wm_state;
                if interesting {
                    refresh_wm_state(client);
                }
            });
            Step::Silence
        }
        XCB_FOCUS_IN => Step::Silence,
        _ => {
            // the xkb extension's own events ride above the core range;
            // StateNotify feeds the modifier truth into the state
            let base = with_x(|client| client.xkb_base_event);
            if base != 0 && kind == base {
                let notify = event as *mut XkbStateNotifyEvent;
                if unsafe { (*notify).xkb_type } == XKB_EVENT_TYPE_STATE_NOTIFY as u8 {
                    with_x(|client| unsafe {
                        if !client.keyboard.state.is_null() {
                            crate::ffi::xkb_state_update_mask(
                                client.keyboard.state,
                                (*notify).base_mods as u32,
                                (*notify).latched_mods as u32,
                                (*notify).locked_mods as u32,
                                (*notify).base_group as u32,
                                (*notify).latched_group as u32,
                                (*notify).locked_group as u32,
                            );
                        }
                    });
                }
            }
            Step::Silence
        }
    }
}

/// Pulls every queued xcb event and interprets it — events free after
/// use (xcb mallocs each frame). Frames a selection pump set aside go
/// first: their order against fresh events must hold.
fn drain_events() -> bool {
    let mut quit = false;
    loop {
        let event = PENDING
            .with(|q| q.borrow_mut().pop_front())
            .unwrap_or_else(|| {
                with_x(|client| unsafe { xcb_poll_for_event(client.connection) })
            });
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
            for slot in &client.panels {
                if let Some(panel) = slot {
                    if let Some(backing) = &panel.backing {
                        xcb_shm_detach(client.connection, backing.segment);
                        shmdt(backing.map.cast());
                    }
                    xcb_destroy_window(client.connection, panel.window);
                }
            }
            if let Some(win) = client.win {
                if let Some(backing) = win.backing {
                    drop_backing(client.connection, backing);
                }
                xcb_destroy_window(client.connection, win.id);
            }
            let kb = &client.keyboard;
            if !kb.compose.is_null() {
                crate::ffi::xkb_compose_state_unref(kb.compose);
            }
            if !kb.state.is_null() {
                crate::ffi::xkb_state_unref(kb.state);
            }
            if !kb.scratch.is_null() {
                crate::ffi::xkb_state_unref(kb.scratch);
            }
            if !kb.keymap.is_null() {
                crate::ffi::xkb_keymap_unref(kb.keymap);
            }
            if !kb.context.is_null() {
                crate::ffi::xkb_context_unref(kb.context);
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
    fn every_cursor_style_wears_a_core_glyph() {
        // sources are even (mask = glyph+1 pairs with it), all six
        // styles resolve, and no two share a face
        let all = [
            Cursor::Arrow,
            Cursor::Pointing,
            Cursor::ResizeLeftRight,
            Cursor::ResizeUpDown,
            Cursor::ResizeNwSe,
            Cursor::ResizeNeSw,
        ];
        let mut seen = std::collections::HashSet::new();
        for cursor in all {
            let glyph = glyph_of(cursor);
            assert_eq!(glyph % 2, 0, "cursor-font sources sit on even codes");
            assert!(seen.insert(glyph), "two styles share glyph {glyph}");
            assert!(cursor_slot(cursor) < 6);
        }
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn the_xkb_state_notify_layout_holds() {
        assert_eq!(std::mem::offset_of!(XkbStateNotifyEvent, base_mods), 10);
        assert_eq!(std::mem::offset_of!(XkbStateNotifyEvent, base_group), 14);
        assert_eq!(std::mem::offset_of!(XkbStateNotifyEvent, locked_group), 18);
        assert_eq!(std::mem::offset_of!(XkbStateNotifyEvent, keycode), 28);
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
