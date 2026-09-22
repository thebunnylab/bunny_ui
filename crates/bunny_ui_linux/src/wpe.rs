//! The WPE WebKit stack at run time — the road the webview walks on
//! Linux, with no GTK in it: libwpe (the view backend and its input
//! vocabulary), WPEBackend-fdo (the exportable backend that hands every
//! page frame over as a `wl_shm` buffer), WPEWebKit (the engine, with
//! its JavaScriptCore inside), GLib and GObject (the engine's main
//! context and its signals) and libwayland-server (the reader of the
//! buffers fdo exports). None is a link-time dependency: an app that
//! never mounts a webview never loads them, and a box without the
//! stack refuses the road with one line naming what is missing. Every
//! symbol is resolved by hand, and any one missing refuses the whole
//! road — never a crash at the first call.
//!
//! The ABI is the headers', verbatim — `wpe/input.h`,
//! `wpe/view-backend.h`, `wpe/view-backend-exportable.h`,
//! `wpe/exported-buffer-shm.h` and the WebKit prototypes — checked
//! against Debian trixie's libwpe 1.16, WPEBackend-fdo 1.16 and
//! WPEWebKit 2.48. `GPollFD` is byte-identical to `pollfd` on Linux,
//! which is how the engine's file descriptors ride the shell's own
//! `poll`: the pump below is GLib's prepare/query/check/dispatch cycle
//! with the shell holding the poll, never `g_main_context_iteration`
//! (which would poll on its own and starve the display).

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_ulong, c_void};

use crate::ffi::PollFd;

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}
const RTLD_NOW: c_int = 2;

// MARK: - the input vocabulary (wpe/input.h)

pub(crate) const MOD_CONTROL: u32 = 1 << 0;
pub(crate) const MOD_SHIFT: u32 = 1 << 1;
pub(crate) const MOD_ALT: u32 = 1 << 2;
pub(crate) const MOD_META: u32 = 1 << 3;
pub(crate) const BUTTON1: u32 = 1 << 20;
pub(crate) const BUTTON2: u32 = 1 << 21;
pub(crate) const BUTTON3: u32 = 1 << 22;

#[repr(C)]
pub(crate) struct KeyboardEvent {
    pub time: u32,
    pub key_code: u32,
    pub hardware_key_code: u32,
    pub pressed: bool,
    pub modifiers: u32,
}

pub(crate) const POINTER_MOTION: c_int = 1;
pub(crate) const POINTER_BUTTON: c_int = 2;

#[repr(C)]
pub(crate) struct PointerEvent {
    pub kind: c_int,
    pub time: u32,
    pub x: c_int,
    pub y: c_int,
    pub button: u32,
    pub state: u32,
    pub modifiers: u32,
}

pub(crate) const AXIS_MOTION_SMOOTH: c_int = 2;
pub(crate) const AXIS_MASK_2D: c_int = 1 << 16;

#[repr(C)]
pub(crate) struct AxisEvent {
    pub kind: c_int,
    pub time: u32,
    pub x: c_int,
    pub y: c_int,
    pub axis: u32,
    pub value: i32,
    pub modifiers: u32,
}

#[repr(C)]
pub(crate) struct Axis2dEvent {
    pub base: AxisEvent,
    pub x_axis: f64,
    pub y_axis: f64,
}

// MARK: - the view backend (wpe/view-backend.h)

pub(crate) const ACTIVITY_VISIBLE: u32 = 1 << 0;
pub(crate) const ACTIVITY_FOCUSED: u32 = 1 << 1;
pub(crate) const ACTIVITY_IN_WINDOW: u32 = 1 << 2;

// MARK: - the exportable backend (wpe/view-backend-exportable.h)

/// One export callback: the client's data, then the buffer.
pub(crate) type ExportFn = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// The client table fdo calls with every frame the page rendered —
/// five slots, and this shell fills the SHM one alone.
#[repr(C)]
pub(crate) struct ExportableClient {
    pub export_buffer_resource: Option<ExportFn>,
    pub export_dmabuf_resource: Option<ExportFn>,
    pub export_shm_buffer: Option<ExportFn>,
    pub reserved0: Option<unsafe extern "C" fn()>,
    pub reserved1: Option<unsafe extern "C" fn()>,
}

// MARK: - the engine's enums (WebKitWebView.h, WebKitPolicyDecision.h, WebKitUserContent.h)

pub(crate) const LOAD_COMMITTED: c_int = 2;
pub(crate) const POLICY_NAVIGATION_ACTION: c_int = 0;
pub(crate) const POLICY_NEW_WINDOW_ACTION: c_int = 1;
pub(crate) const NAVIGATION_LINK_CLICKED: c_int = 0;
pub(crate) const INJECT_TOP_FRAME: c_int = 1;
pub(crate) const INJECT_AT_DOCUMENT_START: c_int = 0;

/// The pixel format a wl_shm buffer from the engine carries when it
/// has alpha: little-endian words, so B G R A in memory, PREMULTIPLIED.
/// The other word the engine can send is XRGB8888 (1), opaque.
pub(crate) const SHM_ARGB8888: u32 = 0;

/// `GError`, as GLib lays it out.
#[repr(C)]
pub(crate) struct GError {
    pub domain: u32,
    pub code: c_int,
    pub message: *mut c_char,
}

// MARK: - the symbol tables

pub(crate) struct GlibFns {
    pub main_context_default: unsafe extern "C" fn() -> *mut c_void,
    pub main_context_acquire: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub main_context_release: unsafe extern "C" fn(*mut c_void),
    pub main_context_prepare: unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int,
    pub main_context_query:
        unsafe extern "C" fn(*mut c_void, c_int, *mut c_int, *mut PollFd, c_int) -> c_int,
    pub main_context_check: unsafe extern "C" fn(*mut c_void, c_int, *mut PollFd, c_int) -> c_int,
    pub main_context_dispatch: unsafe extern "C" fn(*mut c_void),
    pub main_context_iteration: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    pub quark_to_string: unsafe extern "C" fn(u32) -> *const c_char,
    pub free: unsafe extern "C" fn(*mut c_void),
}

pub(crate) struct GObjectFns {
    pub signal_connect_data: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *const c_void,
        *mut c_void,
        *const c_void,
        c_uint,
    ) -> c_ulong,
    pub object_unref: unsafe extern "C" fn(*mut c_void),
}

pub(crate) struct WpeFns {
    pub dispatch_set_size: unsafe extern "C" fn(*mut c_void, u32, u32),
    pub dispatch_set_device_scale_factor: unsafe extern "C" fn(*mut c_void, f32),
    pub add_activity_state: unsafe extern "C" fn(*mut c_void, u32),
    pub remove_activity_state: unsafe extern "C" fn(*mut c_void, u32),
    pub dispatch_keyboard_event: unsafe extern "C" fn(*mut c_void, *mut KeyboardEvent),
    pub dispatch_pointer_event: unsafe extern "C" fn(*mut c_void, *mut PointerEvent),
    pub dispatch_axis_event: unsafe extern "C" fn(*mut c_void, *mut AxisEvent),
}

pub(crate) struct FdoFns {
    pub initialize_shm: unsafe extern "C" fn() -> bool,
    pub exportable_create:
        unsafe extern "C" fn(*const ExportableClient, *mut c_void, u32, u32) -> *mut c_void,
    pub exportable_destroy: unsafe extern "C" fn(*mut c_void),
    pub exportable_get_view_backend: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub exportable_dispatch_frame_complete: unsafe extern "C" fn(*mut c_void),
    pub exportable_dispatch_release_shm_exported_buffer:
        unsafe extern "C" fn(*mut c_void, *mut c_void),
    pub shm_exported_buffer_get_shm_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
}

pub(crate) struct WebKitFns {
    pub web_view_backend_new: unsafe extern "C" fn(
        *mut c_void,
        Option<unsafe extern "C" fn(*mut c_void)>,
        *mut c_void,
    ) -> *mut c_void,
    pub web_view_new: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub web_view_get_user_content_manager: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub web_view_get_settings: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub settings_set_enable_developer_extras: unsafe extern "C" fn(*mut c_void, c_int),
    pub web_view_load_uri: unsafe extern "C" fn(*mut c_void, *const c_char),
    pub web_view_load_html: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),
    pub web_view_go_back: unsafe extern "C" fn(*mut c_void),
    pub web_view_go_forward: unsafe extern "C" fn(*mut c_void),
    pub web_view_get_uri: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub web_view_evaluate_javascript: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        isize,
        *const c_char,
        *const c_char,
        *mut c_void,
        *const c_void,
        *mut c_void,
    ),
    pub user_content_manager_register_script_message_handler:
        unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    pub user_content_manager_add_script: unsafe extern "C" fn(*mut c_void, *mut c_void),
    pub user_content_manager_remove_all_scripts: unsafe extern "C" fn(*mut c_void),
    pub user_script_new: unsafe extern "C" fn(
        *const c_char,
        c_int,
        c_int,
        *const *const c_char,
        *const *const c_char,
    ) -> *mut c_void,
    pub user_script_unref: unsafe extern "C" fn(*mut c_void),
    pub navigation_policy_decision_get_navigation_action:
        unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub navigation_action_get_navigation_type: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub navigation_action_get_request: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub uri_request_get_uri: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub policy_decision_use: unsafe extern "C" fn(*mut c_void),
    pub policy_decision_ignore: unsafe extern "C" fn(*mut c_void),
    pub jsc_value_is_string: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub jsc_value_to_string: unsafe extern "C" fn(*mut c_void) -> *mut c_char,
}

pub(crate) struct WlServerFns {
    pub shm_buffer_begin_access: unsafe extern "C" fn(*mut c_void),
    pub shm_buffer_end_access: unsafe extern "C" fn(*mut c_void),
    pub shm_buffer_get_data: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub shm_buffer_get_stride: unsafe extern "C" fn(*mut c_void) -> i32,
    pub shm_buffer_get_width: unsafe extern "C" fn(*mut c_void) -> i32,
    pub shm_buffer_get_height: unsafe extern "C" fn(*mut c_void) -> i32,
    pub shm_buffer_get_format: unsafe extern "C" fn(*mut c_void) -> u32,
}

/// The whole stack, resolved once.
pub(crate) struct Stack {
    pub glib: GlibFns,
    pub gobject: GObjectFns,
    pub wpe: WpeFns,
    pub fdo: FdoFns,
    pub webkit: WebKitFns,
    pub wl: WlServerFns,
}

/// The backend library libwpe loads for the engine's web process —
/// the fdo one, always: a distribution ships no `libWPEBackend-default`
/// and this shell speaks fdo's export contract and no other.
const BACKEND_LIBRARY: &str = "libWPEBackend-fdo-1.0.so.1";

/// The six libraries, with the Debian package each one comes in — the
/// refusal names the one that is missing.
const LIBRARIES: [(&CStr, &str); 6] = [
    (c"libglib-2.0.so.0", "libglib2.0-0"),
    (c"libgobject-2.0.so.0", "libglib2.0-0"),
    (c"libwpe-1.0.so.1", "libwpe-1.0-1"),
    (c"libWPEBackend-fdo-1.0.so.1", "libwpebackend-fdo-1.0-1"),
    (c"libWPEWebKit-2.0.so.1", "libwpewebkit-2.0-1"),
    (c"libwayland-server.so.0", "libwayland-server0"),
];

fn resolve(handle: *mut c_void, name: &str) -> Option<*mut c_void> {
    let name = CString::new(name).expect("a symbol name");
    let symbol = unsafe { dlsym(handle, name.as_ptr()) };
    (!symbol.is_null()).then_some(symbol)
}

/// The stack, loaded on the first ask and kept for the process. `None`
/// (said once on stderr, with the package to install) when a library
/// or a symbol is missing.
pub(crate) fn loader() -> Option<&'static Stack> {
    static STACK: std::sync::OnceLock<Option<Stack>> = std::sync::OnceLock::new();
    STACK
        .get_or_init(|| {
            // libwpe reads which backend to load at its first use; the
            // engine's processes inherit the choice. Set before any of
            // the stack is up, and never over a choice already made
            if std::env::var_os("WPE_BACKEND_LIBRARY").is_none() {
                // SAFETY: called once, from the UI thread, before the
                // engine spawns anything that reads the environment
                unsafe { std::env::set_var("WPE_BACKEND_LIBRARY", BACKEND_LIBRARY) };
            }
            let mut handles = [std::ptr::null_mut(); 6];
            for (slot, (soname, package)) in handles.iter_mut().zip(LIBRARIES) {
                *slot = unsafe { dlopen(soname.as_ptr(), RTLD_NOW) };
                if slot.is_null() {
                    eprintln!(
                        "bunny_ui_linux: no {} — the webview mounts nothing; install {package}",
                        soname.to_string_lossy()
                    );
                    return None;
                }
            }
            let [glib, gobject, wpe, fdo, webkit, wl] = handles;
            let stack = unsafe { table(glib, gobject, wpe, fdo, webkit, wl) };
            if stack.is_none() {
                eprintln!(
                    "bunny_ui_linux: the WPE WebKit stack is incomplete (a symbol is missing) — the webview mounts nothing"
                );
            }
            stack
        })
        .as_ref()
}

/// Every symbol, by hand; any one missing refuses the road.
unsafe fn table(
    glib: *mut c_void,
    gobject: *mut c_void,
    wpe: *mut c_void,
    fdo: *mut c_void,
    webkit: *mut c_void,
    wl: *mut c_void,
) -> Option<Stack> {
    let g = |name: &str| resolve(glib, name);
    let go = |name: &str| resolve(gobject, name);
    let w = |name: &str| resolve(wpe, name);
    let f = |name: &str| resolve(fdo, name);
    let k = |name: &str| resolve(webkit, name);
    let s = |name: &str| resolve(wl, name);
    unsafe {
        Some(Stack {
            glib: GlibFns {
                main_context_default: std::mem::transmute(g("g_main_context_default")?),
                main_context_acquire: std::mem::transmute(g("g_main_context_acquire")?),
                main_context_release: std::mem::transmute(g("g_main_context_release")?),
                main_context_prepare: std::mem::transmute(g("g_main_context_prepare")?),
                main_context_query: std::mem::transmute(g("g_main_context_query")?),
                main_context_check: std::mem::transmute(g("g_main_context_check")?),
                main_context_dispatch: std::mem::transmute(g("g_main_context_dispatch")?),
                main_context_iteration: std::mem::transmute(g("g_main_context_iteration")?),
                quark_to_string: std::mem::transmute(g("g_quark_to_string")?),
                free: std::mem::transmute(g("g_free")?),
            },
            gobject: GObjectFns {
                signal_connect_data: std::mem::transmute(go("g_signal_connect_data")?),
                object_unref: std::mem::transmute(go("g_object_unref")?),
            },
            wpe: WpeFns {
                dispatch_set_size: std::mem::transmute(w("wpe_view_backend_dispatch_set_size")?),
                dispatch_set_device_scale_factor: std::mem::transmute(w(
                    "wpe_view_backend_dispatch_set_device_scale_factor",
                )?),
                add_activity_state: std::mem::transmute(w("wpe_view_backend_add_activity_state")?),
                remove_activity_state: std::mem::transmute(w(
                    "wpe_view_backend_remove_activity_state",
                )?),
                dispatch_keyboard_event: std::mem::transmute(w(
                    "wpe_view_backend_dispatch_keyboard_event",
                )?),
                dispatch_pointer_event: std::mem::transmute(w(
                    "wpe_view_backend_dispatch_pointer_event",
                )?),
                dispatch_axis_event: std::mem::transmute(w("wpe_view_backend_dispatch_axis_event")?),
            },
            fdo: FdoFns {
                initialize_shm: std::mem::transmute(f("wpe_fdo_initialize_shm")?),
                exportable_create: std::mem::transmute(f("wpe_view_backend_exportable_fdo_create")?),
                exportable_destroy: std::mem::transmute(f(
                    "wpe_view_backend_exportable_fdo_destroy",
                )?),
                exportable_get_view_backend: std::mem::transmute(f(
                    "wpe_view_backend_exportable_fdo_get_view_backend",
                )?),
                exportable_dispatch_frame_complete: std::mem::transmute(f(
                    "wpe_view_backend_exportable_fdo_dispatch_frame_complete",
                )?),
                exportable_dispatch_release_shm_exported_buffer: std::mem::transmute(f(
                    "wpe_view_backend_exportable_fdo_dispatch_release_shm_exported_buffer",
                )?),
                shm_exported_buffer_get_shm_buffer: std::mem::transmute(f(
                    "wpe_fdo_shm_exported_buffer_get_shm_buffer",
                )?),
            },
            webkit: WebKitFns {
                web_view_backend_new: std::mem::transmute(k("webkit_web_view_backend_new")?),
                web_view_new: std::mem::transmute(k("webkit_web_view_new")?),
                web_view_get_user_content_manager: std::mem::transmute(k(
                    "webkit_web_view_get_user_content_manager",
                )?),
                web_view_get_settings: std::mem::transmute(k("webkit_web_view_get_settings")?),
                settings_set_enable_developer_extras: std::mem::transmute(k(
                    "webkit_settings_set_enable_developer_extras",
                )?),
                web_view_load_uri: std::mem::transmute(k("webkit_web_view_load_uri")?),
                web_view_load_html: std::mem::transmute(k("webkit_web_view_load_html")?),
                web_view_go_back: std::mem::transmute(k("webkit_web_view_go_back")?),
                web_view_go_forward: std::mem::transmute(k("webkit_web_view_go_forward")?),
                web_view_get_uri: std::mem::transmute(k("webkit_web_view_get_uri")?),
                web_view_evaluate_javascript: std::mem::transmute(k(
                    "webkit_web_view_evaluate_javascript",
                )?),
                user_content_manager_register_script_message_handler: std::mem::transmute(k(
                    "webkit_user_content_manager_register_script_message_handler",
                )?),
                user_content_manager_add_script: std::mem::transmute(k(
                    "webkit_user_content_manager_add_script",
                )?),
                user_content_manager_remove_all_scripts: std::mem::transmute(k(
                    "webkit_user_content_manager_remove_all_scripts",
                )?),
                user_script_new: std::mem::transmute(k("webkit_user_script_new")?),
                user_script_unref: std::mem::transmute(k("webkit_user_script_unref")?),
                navigation_policy_decision_get_navigation_action: std::mem::transmute(k(
                    "webkit_navigation_policy_decision_get_navigation_action",
                )?),
                navigation_action_get_navigation_type: std::mem::transmute(k(
                    "webkit_navigation_action_get_navigation_type",
                )?),
                navigation_action_get_request: std::mem::transmute(k(
                    "webkit_navigation_action_get_request",
                )?),
                uri_request_get_uri: std::mem::transmute(k("webkit_uri_request_get_uri")?),
                policy_decision_use: std::mem::transmute(k("webkit_policy_decision_use")?),
                policy_decision_ignore: std::mem::transmute(k("webkit_policy_decision_ignore")?),
                jsc_value_is_string: std::mem::transmute(k("jsc_value_is_string")?),
                jsc_value_to_string: std::mem::transmute(k("jsc_value_to_string")?),
            },
            wl: WlServerFns {
                shm_buffer_begin_access: std::mem::transmute(s("wl_shm_buffer_begin_access")?),
                shm_buffer_end_access: std::mem::transmute(s("wl_shm_buffer_end_access")?),
                shm_buffer_get_data: std::mem::transmute(s("wl_shm_buffer_get_data")?),
                shm_buffer_get_stride: std::mem::transmute(s("wl_shm_buffer_get_stride")?),
                shm_buffer_get_width: std::mem::transmute(s("wl_shm_buffer_get_width")?),
                shm_buffer_get_height: std::mem::transmute(s("wl_shm_buffer_get_height")?),
                shm_buffer_get_format: std::mem::transmute(s("wl_shm_buffer_get_format")?),
            },
        })
    }
}

/// A C string the engine handed over, copied out. Empty for null.
pub(crate) unsafe fn text_of(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

/// Connects one signal on a GObject, by name, with `data` as the
/// handler's last argument.
pub(crate) unsafe fn connect(
    stack: &Stack,
    instance: *mut c_void,
    signal: &CStr,
    handler: *const c_void,
    data: *mut c_void,
) {
    unsafe {
        (stack.gobject.signal_connect_data)(
            instance,
            signal.as_ptr(),
            handler,
            data,
            std::ptr::null(),
            0,
        );
    }
}

// MARK: - the pump

/// One turn of GLib's cycle, between `pump_prepare` and `pump_after`.
#[derive(Clone, Copy)]
struct Turn {
    context: *mut c_void,
    priority: c_int,
    /// Where the engine's descriptors start in the shell's poll set.
    base: usize,
    count: usize,
}

thread_local! {
    static TURN: Cell<Option<Turn>> = const { Cell::new(None) };
    static ACQUIRED: Cell<bool> = const { Cell::new(false) };
}

/// Before the shell polls: the engine's descriptors are appended to
/// `fds`, and the timeout GLib asks for comes back (`None` = no bound).
/// The default main context is acquired on the first turn — this
/// thread owns it from then on, which is the law WebKit expects.
pub(crate) fn pump_prepare(fds: &mut Vec<PollFd>) -> Option<c_int> {
    let stack = loader()?;
    unsafe {
        let context = (stack.glib.main_context_default)();
        if !ACQUIRED.get() {
            if (stack.glib.main_context_acquire)(context) == 0 {
                return None;
            }
            ACQUIRED.set(true);
        }
        let mut priority: c_int = 0;
        (stack.glib.main_context_prepare)(context, &mut priority);
        let base = fds.len();
        let mut timeout: c_int = -1;
        let mut room = 8usize;
        let count = loop {
            fds.truncate(base);
            for _ in 0..room {
                fds.push(PollFd { fd: -1, events: 0, revents: 0 });
            }
            let wanted = (stack.glib.main_context_query)(
                context,
                priority,
                &mut timeout,
                fds.as_mut_ptr().add(base),
                room as c_int,
            );
            let wanted = wanted.max(0) as usize;
            if wanted <= room {
                fds.truncate(base + wanted);
                break wanted;
            }
            room = wanted;
        };
        TURN.set(Some(Turn { context, priority, base, count }));
        (timeout >= 0).then_some(timeout)
    }
}

/// After the shell polled, with the WHOLE set it polled: what is ready
/// is dispatched — the page's frames, the signals, the messages all
/// land here, on this thread.
pub(crate) fn pump_after(fds: &mut [PollFd]) {
    let Some(turn) = TURN.take() else { return };
    let Some(stack) = loader() else { return };
    if fds.len() < turn.base + turn.count {
        return;
    }
    unsafe {
        let ready = (stack.glib.main_context_check)(
            turn.context,
            turn.priority,
            fds.as_mut_ptr().add(turn.base),
            turn.count as c_int,
        );
        if ready != 0 {
            (stack.glib.main_context_dispatch)(turn.context);
        }
    }
}

/// A few turns of the context on its own, never blocking — at
/// teardown, so the engine reaps the processes it started.
pub(crate) fn pump_settle(turns: usize) {
    let Some(stack) = loader() else { return };
    unsafe {
        let context = (stack.glib.main_context_default)();
        for _ in 0..turns {
            if (stack.glib.main_context_iteration)(context, 0) == 0 {
                break;
            }
        }
    }
}

/// Lets go of the default context, if this thread took it.
pub(crate) fn pump_release() {
    if !ACQUIRED.replace(false) {
        return;
    }
    let Some(stack) = loader() else { return };
    unsafe {
        let context = (stack.glib.main_context_default)();
        (stack.glib.main_context_release)(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structs the engine reads are laid out as the headers say.
    #[test]
    fn the_input_structs_match_the_headers() {
        assert_eq!(std::mem::size_of::<KeyboardEvent>(), 20);
        assert_eq!(std::mem::size_of::<PointerEvent>(), 28);
        assert_eq!(std::mem::size_of::<AxisEvent>(), 28);
        assert_eq!(std::mem::size_of::<Axis2dEvent>(), 48);
        assert_eq!(std::mem::size_of::<ExportableClient>(), 5 * std::mem::size_of::<usize>());
        assert_eq!(std::mem::size_of::<GError>(), 16);
        // GPollFD is pollfd: a poll set the shell builds is one GLib reads
        assert_eq!(std::mem::size_of::<PollFd>(), 8);
    }
}
