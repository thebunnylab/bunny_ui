//! The platform's own file dialogs, through the desktop portal.
//!
//! A picker is not a view. It is a window the DESKTOP owns, with the
//! reader's bookmarks, their recent places and their idea of what a
//! folder looks like — an app that draws its own is a worse one, and
//! on a sandboxed desktop it is also the only one that grants access:
//! a Flatpak sees the chosen folder because the PORTAL chose it, and
//! sees nothing an app picked for itself.
//!
//! **Why the portal and not a toolkit.** There is no small library on
//! this platform that opens a file chooser: the ones that do are whole
//! toolkits, and the house does not take a toolkit as a dependency to
//! open one window. The portal is the desktop's own service for
//! exactly this, spoken over the bus this shell already speaks for the
//! appearance mirrors — the same standing libsecret has for the
//! keyring.
//!
//! **These calls block.** The reader may take as long as they like,
//! and there is nothing for the caller to do until they answer, so
//! this belongs on a thread — the same rule the keyring keeps:
//!
//! ```ignore
//! let chosen = State::new(None);
//! view.task(move || {
//!     let (send, mut recv) = task::channel();
//!     std::thread::spawn(move || {
//!         let _ = send.send(dialog::open_folder("Open a project"));
//!     });
//!     if let Some(folder) = recv.recv().await {
//!         chosen.set(folder);
//!     }
//! })
//! ```
//!
//! Unlike the two desktop twins this one does NOT run a modal loop of
//! its own: the panel is another process's window, and this waits for
//! its answer on the bus.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::PathBuf;

// The portal's own half of libdbus — the containers, the match rule
// and the manual pump the appearance mirrors never needed, because a
// setting is ONE call and a picker is a call plus a signal.
#[link(name = "dbus-1")]
unsafe extern "C" {
    fn dbus_bus_get_private(kind: c_int, error: *mut DbusError) -> *mut c_void;
    fn dbus_bus_get_unique_name(connection: *mut c_void) -> *const c_char;
    fn dbus_bus_add_match(connection: *mut c_void, rule: *const c_char, error: *mut DbusError);
    fn dbus_connection_close(connection: *mut c_void);
    fn dbus_connection_unref(connection: *mut c_void);
    fn dbus_connection_flush(connection: *mut c_void);
    fn dbus_connection_read_write(connection: *mut c_void, timeout_ms: c_int) -> c_int;
    fn dbus_connection_pop_message(connection: *mut c_void) -> *mut c_void;
    fn dbus_connection_send_with_reply_and_block(
        connection: *mut c_void,
        message: *mut c_void,
        timeout_ms: c_int,
        error: *mut DbusError,
    ) -> *mut c_void;
    fn dbus_error_init(error: *mut DbusError);
    fn dbus_error_is_set(error: *const DbusError) -> c_int;
    fn dbus_error_free(error: *mut DbusError);
    fn dbus_message_new_method_call(
        destination: *const c_char,
        path: *const c_char,
        interface: *const c_char,
        method: *const c_char,
    ) -> *mut c_void;
    fn dbus_message_unref(message: *mut c_void);
    fn dbus_message_is_signal(
        message: *mut c_void,
        interface: *const c_char,
        member: *const c_char,
    ) -> c_int;
    fn dbus_message_iter_init_append(message: *mut c_void, iter: *mut DbusIter);
    fn dbus_message_iter_append_basic(
        iter: *mut DbusIter,
        kind: c_int,
        value: *const c_void,
    ) -> c_int;
    fn dbus_message_iter_open_container(
        iter: *mut DbusIter,
        kind: c_int,
        signature: *const c_char,
        sub: *mut DbusIter,
    ) -> c_int;
    fn dbus_message_iter_close_container(iter: *mut DbusIter, sub: *mut DbusIter) -> c_int;
    fn dbus_message_iter_init(message: *mut c_void, iter: *mut DbusIter) -> c_int;
    fn dbus_message_iter_next(iter: *mut DbusIter) -> c_int;
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

impl DbusError {
    fn new() -> DbusError {
        let mut error = DbusError {
            name: std::ptr::null(),
            message: std::ptr::null(),
            dummy: [0; 2],
            padding: std::ptr::null_mut(),
        };
        unsafe { dbus_error_init(&mut error) };
        error
    }
}

/// libdbus asks callers for 16 pointers of iterator space; the layout
/// is opaque by contract.
#[repr(C)]
struct DbusIter {
    opaque: [usize; 16],
}

impl DbusIter {
    fn new() -> DbusIter {
        DbusIter { opaque: [0; 16] }
    }
}

const DBUS_BUS_SESSION: c_int = 0;
const DBUS_TYPE_STRING: c_int = 115; // 's'
const DBUS_TYPE_VARIANT: c_int = 118; // 'v'
const DBUS_TYPE_ARRAY: c_int = 97; // 'a'
const DBUS_TYPE_BOOLEAN: c_int = 98; // 'b'
const DBUS_TYPE_UINT32: c_int = 117; // 'u'
const DBUS_TYPE_DICT_ENTRY: c_int = 101; // 'e'

/// `org.freedesktop.portal.Request::Response` — 0 chose, 1 cancelled,
/// 2 ended some other way. Only 0 carries a result.
const RESPONSE_CHOSE: u32 = 0;

/// How long the portal has to ACCEPT the request. This is not the
/// reader's time — it is the desktop's, to say the service is there
/// at all. A box with no portal running answers inside this and the
/// call comes back empty.
const ACCEPT_TIMEOUT_MS: c_int = 2_000;

/// One turn of the manual pump. The reader is not on a clock: this
/// only decides how often the loop looks up from the socket.
const PUMP_SLICE_MS: c_int = 200;

/// The reader picks ONE folder. `None` = they cancelled, or no portal
/// answered — both are "nothing was chosen", which is an answer and
/// not a failure.
///
/// `prompt` becomes the panel's title — the sentence that says what
/// the folder is FOR ("Open a project"). An empty one leaves the panel
/// with its own wording.
pub fn open_folder(prompt: &str) -> Option<PathBuf> {
    open(prompt, true)
}

/// The same panel, for ONE file. `None` = cancelled.
pub fn open_file(prompt: &str) -> Option<PathBuf> {
    open(prompt, false)
}

fn open(prompt: &str, directory: bool) -> Option<PathBuf> {
    let title = CString::new(prompt).ok()?;
    unsafe { ask_the_portal(&title, directory) }
}

unsafe fn ask_the_portal(title: &CStr, directory: bool) -> Option<PathBuf> {
    unsafe {
        let mut error = DbusError::new();
        let connection = dbus_bus_get_private(DBUS_BUS_SESSION, &mut error);
        if connection.is_null() {
            dbus_error_free(&mut error);
            return None;
        }
        let chosen = converse(connection, title, directory, &mut error);
        dbus_error_free(&mut error);
        dbus_connection_close(connection);
        dbus_connection_unref(connection);
        chosen
    }
}

unsafe fn converse(
    connection: *mut c_void,
    title: &CStr,
    directory: bool,
    error: *mut DbusError,
) -> Option<PathBuf> {
    unsafe {
        // The answer is a SIGNAL on an object the portal is about to
        // make, so the match rule goes up FIRST — a portal that
        // answers before the rule exists would answer into nothing.
        // The object's path is predictable from the token, which is
        // the whole reason the request carries one.
        let token = c"bunny_ui_picker";
        let handle = request_path(connection, token)?;
        let rule = CString::new(format!(
            "type='signal',interface='org.freedesktop.portal.Request',\
             member='Response',path='{handle}'"
        ))
        .ok()?;
        dbus_bus_add_match(connection, rule.as_ptr(), error);
        if dbus_error_is_set(error) != 0 {
            return None;
        }
        dbus_connection_flush(connection);

        let message = dbus_message_new_method_call(
            c"org.freedesktop.portal.Desktop".as_ptr(),
            c"/org/freedesktop/portal/desktop".as_ptr(),
            c"org.freedesktop.portal.FileChooser".as_ptr(),
            c"OpenFile".as_ptr(),
        );
        if message.is_null() {
            return None;
        }
        let parent = parent_window();
        write_request(message, &parent, title, token, directory);
        let reply =
            dbus_connection_send_with_reply_and_block(connection, message, ACCEPT_TIMEOUT_MS, error);
        dbus_message_unref(message);
        if reply.is_null() {
            return None;
        }
        dbus_message_unref(reply);
        wait_for_response(connection)
    }
}

/// The object path the portal will answer on:
/// `/org/freedesktop/portal/desktop/request/SENDER/TOKEN`, where
/// SENDER is this connection's unique name with the leading colon
/// dropped and every dot turned into an underscore. The rule is the
/// portal's, and following it is what lets the match rule go up before
/// the request does.
unsafe fn request_path(connection: *mut c_void, token: &CStr) -> Option<String> {
    unsafe {
        let unique = dbus_bus_get_unique_name(connection);
        if unique.is_null() {
            return None;
        }
        let unique = CStr::from_ptr(unique).to_str().ok()?;
        let sender = unique.trim_start_matches(':').replace('.', "_");
        let token = token.to_str().ok()?;
        Some(format!("/org/freedesktop/portal/desktop/request/{sender}/{token}"))
    }
}

/// The window the panel hangs from, in the portal's own spelling. On
/// X11 it is the window id in hex; on Wayland a handle would have to
/// be exported first, and an empty string is the documented "no
/// parent" — the panel opens unattached rather than not at all.
fn parent_window() -> CString {
    let identifier = match crate::ffi::backend() {
        crate::ffi::Backend::X11 => {
            crate::x11::main_window().map(|id| format!("x11:{id:x}")).unwrap_or_default()
        }
        crate::ffi::Backend::Wayland => String::new(),
    };
    CString::new(identifier).unwrap_or_default()
}

/// `OpenFile(s parent_window, s title, a{sv} options)`.
unsafe fn write_request(
    message: *mut c_void,
    parent: &CStr,
    title: &CStr,
    token: &CStr,
    directory: bool,
) {
    unsafe {
        let mut iter = DbusIter::new();
        dbus_message_iter_init_append(message, &mut iter);
        append_string(&mut iter, parent);
        append_string(&mut iter, title);

        let mut options = DbusIter::new();
        dbus_message_iter_open_container(
            &mut iter,
            DBUS_TYPE_ARRAY,
            c"{sv}".as_ptr(),
            &mut options,
        );
        // the token the answer's object path is built from
        append_option(&mut options, c"handle_token", Variant::Text(token));
        // a container, not a file: the one flag this module exists for
        append_option(&mut options, c"directory", Variant::Flag(directory));
        // one answer, so the result is a path and not a list
        append_option(&mut options, c"multiple", Variant::Flag(false));
        dbus_message_iter_close_container(&mut iter, &mut options);
    }
}

enum Variant<'a> {
    Text(&'a CStr),
    Flag(bool),
}

unsafe fn append_string(iter: &mut DbusIter, text: &CStr) {
    unsafe {
        let pointer = text.as_ptr();
        dbus_message_iter_append_basic(iter, DBUS_TYPE_STRING, (&raw const pointer).cast());
    }
}

/// One `{sv}` entry: the key, then the value inside its variant.
unsafe fn append_option(array: &mut DbusIter, key: &CStr, value: Variant<'_>) {
    unsafe {
        let mut entry = DbusIter::new();
        // a dict entry's signature is implied by the array's own
        dbus_message_iter_open_container(
            array,
            DBUS_TYPE_DICT_ENTRY,
            std::ptr::null(),
            &mut entry,
        );
        append_string(&mut entry, key);
        let mut variant = DbusIter::new();
        match value {
            Variant::Text(text) => {
                dbus_message_iter_open_container(
                    &mut entry,
                    DBUS_TYPE_VARIANT,
                    c"s".as_ptr(),
                    &mut variant,
                );
                append_string(&mut variant, text);
            }
            Variant::Flag(flag) => {
                dbus_message_iter_open_container(
                    &mut entry,
                    DBUS_TYPE_VARIANT,
                    c"b".as_ptr(),
                    &mut variant,
                );
                // the wire's boolean is a 32-bit word, not a byte
                let word: u32 = flag as u32;
                dbus_message_iter_append_basic(
                    &mut variant,
                    DBUS_TYPE_BOOLEAN,
                    (&raw const word).cast(),
                );
            }
        }
        dbus_message_iter_close_container(&mut entry, &mut variant);
        dbus_message_iter_close_container(array, &mut entry);
    }
}

/// Pumps the connection until the Response arrives. The reader is not
/// on a clock — the only way out other than the answer is the bus
/// going away, which is what `read_write` reports.
unsafe fn wait_for_response(connection: *mut c_void) -> Option<PathBuf> {
    unsafe {
        loop {
            if dbus_connection_read_write(connection, PUMP_SLICE_MS) == 0 {
                return None; // the bus is gone; so is the panel
            }
            loop {
                let message = dbus_connection_pop_message(connection);
                if message.is_null() {
                    break;
                }
                let is_response = dbus_message_is_signal(
                    message,
                    c"org.freedesktop.portal.Request".as_ptr(),
                    c"Response".as_ptr(),
                ) != 0;
                let chosen = is_response.then(|| read_response(message));
                dbus_message_unref(message);
                if let Some(chosen) = chosen {
                    return chosen;
                }
            }
        }
    }
}

/// `Response(u response, a{sv} results)` — the chosen paths arrive as
/// `uris`, a list of file URLs, and this call asked for one.
unsafe fn read_response(message: *mut c_void) -> Option<PathBuf> {
    unsafe {
        let mut iter = DbusIter::new();
        if dbus_message_iter_init(message, &mut iter) == 0 {
            return None;
        }
        if dbus_message_iter_get_arg_type(&mut iter) != DBUS_TYPE_UINT32 {
            return None;
        }
        let mut code = 0u32;
        dbus_message_iter_get_basic(&mut iter, (&raw mut code).cast());
        if code != RESPONSE_CHOSE {
            return None; // cancelled, and a cancel is an answer
        }
        if dbus_message_iter_next(&mut iter) == 0
            || dbus_message_iter_get_arg_type(&mut iter) != DBUS_TYPE_ARRAY
        {
            return None;
        }
        let mut results = DbusIter::new();
        dbus_message_iter_recurse(&mut iter, &mut results);
        while dbus_message_iter_get_arg_type(&mut results) == DBUS_TYPE_DICT_ENTRY {
            let mut entry = DbusIter::new();
            dbus_message_iter_recurse(&mut results, &mut entry);
            if read_string(&mut entry).as_deref() == Some("uris")
                && dbus_message_iter_next(&mut entry) != 0
            {
                let mut variant = DbusIter::new();
                dbus_message_iter_recurse(&mut entry, &mut variant);
                let mut uris = DbusIter::new();
                dbus_message_iter_recurse(&mut variant, &mut uris);
                return read_string(&mut uris).as_deref().and_then(path_of_uri);
            }
            if dbus_message_iter_next(&mut results) == 0 {
                break;
            }
        }
        None
    }
}

unsafe fn read_string(iter: &mut DbusIter) -> Option<String> {
    unsafe {
        if dbus_message_iter_get_arg_type(iter) != DBUS_TYPE_STRING {
            return None;
        }
        let mut text: *const c_char = std::ptr::null();
        dbus_message_iter_get_basic(iter, (&raw mut text).cast());
        (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
    }
}

/// `file:///home/reader/My%20Projects` → the path it names. Anything
/// that is not a local file has no path here, which is exactly what
/// `FORCE_FILE_SYSTEM` means on the other two shells.
fn path_of_uri(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    // the authority is empty for a local file, and a portal never
    // sends another one
    let encoded = encoded.strip_prefix('/').map(|rest| format!("/{rest}"))?;
    let mut path = Vec::with_capacity(encoded.len());
    let bytes = encoded.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let escape = (byte == b'%' && at + 2 < bytes.len())
            .then(|| {
                let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).ok()?;
                u8::from_str_radix(hex, 16).ok()
            })
            .flatten();
        match escape {
            Some(decoded) => {
                path.push(decoded);
                at += 3;
            }
            None => {
                path.push(byte);
                at += 1;
            }
        }
    }
    let path = String::from_utf8(path).ok()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The half of this module that is arithmetic and not a bus: a
    /// portal answers URLs, an app wants paths, and the escapes in
    /// between are where a folder with a space in its name is lost.
    #[test]
    fn a_file_url_becomes_the_path_it_names() {
        assert_eq!(
            path_of_uri("file:///home/reader/projects"),
            Some(PathBuf::from("/home/reader/projects")),
        );
        assert_eq!(
            path_of_uri("file:///home/reader/My%20Projects"),
            Some(PathBuf::from("/home/reader/My Projects")),
        );
        // the escapes are bytes, so a name outside ASCII survives
        assert_eq!(
            path_of_uri("file:///home/reader/Documenta%C3%A7%C3%A3o"),
            Some(PathBuf::from("/home/reader/Documentação")),
        );
        // a stray percent is a percent, not a truncation
        assert_eq!(
            path_of_uri("file:///tmp/100%"),
            Some(PathBuf::from("/tmp/100%")),
        );
        // nothing that is not a local file has a path here
        assert_eq!(path_of_uri("https://example.com/x"), None);
        assert_eq!(path_of_uri("file://"), None);
    }
}
