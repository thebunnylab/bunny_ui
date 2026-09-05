//! The app's life outside its window, on this platform: the desktop's
//! notifications (`org.freedesktop.Notifications`) and the login
//! manager's sleep and wake (`org.freedesktop.login1`), both over the
//! bus this shell already speaks for the portal — libdbus, no crate.
//!
//! ONE thread owns both connections and pumps them: the session bus
//! for the notifications and their signals (a button pressed, a
//! notification closed), the system bus for `PrepareForSleep`. A
//! notification is handed to that thread and `notify` answers at
//! once; the thread's refusal at boot — no session bus — is the
//! answer every later ask gets, by name. A second launch reaches the
//! running one through the spool, like everywhere.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use bunny_ui::app::{AppEvent, Notification, emit};

// The notification's own half of libdbus — the same functions the
// portal speaks, declared here for this module's use.
#[allow(clashing_extern_declarations)]
#[link(name = "dbus-1")]
unsafe extern "C" {
    fn dbus_bus_get_private(kind: c_int, error: *mut DbusError) -> *mut c_void;
    fn dbus_bus_add_match(connection: *mut c_void, rule: *const c_char, error: *mut DbusError);
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

    /// The error's words, and the error freed.
    fn take(&mut self) -> Option<String> {
        unsafe {
            if dbus_error_is_set(self) == 0 {
                return None;
            }
            let words = if self.message.is_null() {
                String::from("the bus refused unnamed")
            } else {
                CStr::from_ptr(self.message).to_string_lossy().into_owned()
            };
            dbus_error_free(self);
            Some(words)
        }
    }
}

/// libdbus asks callers for 16 pointers of iterator space.
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
const DBUS_BUS_SYSTEM: c_int = 1;
const DBUS_TYPE_STRING: c_int = 115; // 's'
const DBUS_TYPE_ARRAY: c_int = 97; // 'a'
const DBUS_TYPE_BOOLEAN: c_int = 98; // 'b'
const DBUS_TYPE_UINT32: c_int = 117; // 'u'
const DBUS_TYPE_INT32: c_int = 105; // 'i'

/// How long the desktop has to answer a `Notify`.
const REPLY_TIMEOUT_MS: c_int = 2_000;
/// One turn of the pump on each bus.
const PUMP_SLICE_MS: c_int = 50;

/// The letters on their way to the thread.
static LETTERS: OnceLock<Mutex<Sender<Notification>>> = OnceLock::new();
/// Why the thread cannot show any — set once at boot, if the session
/// bus is not there.
static REFUSAL: Mutex<Option<String>> = Mutex::new(None);

/// Starts the thread and installs the notifier — at boot.
pub(crate) fn install() {
    let (sender, receiver) = mpsc::channel::<Notification>();
    if LETTERS.set(Mutex::new(sender)).is_err() {
        return;
    }
    std::thread::spawn(move || pump(receiver));
    bunny_ui::app::install_notifier(notify);
}

/// Hands the letter to the thread, or says why the desktop will never
/// show it.
fn notify(notification: &Notification) -> Result<(), String> {
    if let Some(why) = REFUSAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() {
        return Err(why);
    }
    let Some(letters) = LETTERS.get() else {
        return Err(String::from("no shell is running to show a notification"));
    };
    letters
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .send(notification.clone())
        .map_err(|_| String::from("the notification thread is gone"))
}

/// This process's name for the desktop — its executable's.
fn app_name() -> CString {
    let stem = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .unwrap_or_else(|| String::from("app"));
    CString::new(stem).unwrap_or_default()
}

/// The thread: both buses, pumped in turn, for the life of the process.
fn pump(letters: Receiver<Notification>) {
    let mut error = DbusError::new();
    let session = unsafe { dbus_bus_get_private(DBUS_BUS_SESSION, &mut error) };
    if session.is_null() {
        let why = error.take().unwrap_or_else(|| String::from("no session bus"));
        *REFUSAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(format!("the desktop's notifications are not reachable: {why}"));
    } else {
        unsafe {
            dbus_bus_add_match(
                session,
                c"type='signal',interface='org.freedesktop.Notifications'".as_ptr(),
                &mut error,
            );
        }
        let _ = error.take();
    }
    let system = unsafe { dbus_bus_get_private(DBUS_BUS_SYSTEM, &mut error) };
    if system.is_null() {
        let _ = error.take();
    } else {
        unsafe {
            dbus_bus_add_match(
                system,
                c"type='signal',interface='org.freedesktop.login1.Manager',member='PrepareForSleep'"
                    .as_ptr(),
                &mut error,
            );
        }
        let _ = error.take();
    }
    let name = app_name();
    // the desktop's number for each of the app's ids, both ways: a
    // repeat REPLACES, and a button comes back by number
    let mut numbers: HashMap<String, u32> = HashMap::new();
    let mut ids: HashMap<u32, String> = HashMap::new();
    loop {
        while let Ok(letter) = letters.try_recv() {
            if session.is_null() {
                continue;
            }
            let replaces = numbers.get(&letter.id).copied().unwrap_or(0);
            if let Some(number) = unsafe { send_notify(session, &name, &letter, replaces) } {
                numbers.insert(letter.id.clone(), number);
                ids.insert(number, letter.id);
            }
        }
        let mut heard = false;
        if !session.is_null() {
            unsafe {
                if dbus_connection_read_write(session, PUMP_SLICE_MS) != 0 {
                    heard = true;
                    loop {
                        let message = dbus_connection_pop_message(session);
                        if message.is_null() {
                            break;
                        }
                        notification_signal(message, &mut numbers, &mut ids);
                        dbus_message_unref(message);
                    }
                }
            }
        }
        if !system.is_null() {
            unsafe {
                if dbus_connection_read_write(system, PUMP_SLICE_MS) != 0 {
                    heard = true;
                    loop {
                        let message = dbus_connection_pop_message(system);
                        if message.is_null() {
                            break;
                        }
                        sleep_signal(message);
                        dbus_message_unref(message);
                    }
                }
            }
        }
        if !heard {
            // neither bus: nothing to hear but the app's own letters
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// `Notify(app_name, replaces_id, app_icon, summary, body, actions,
/// hints, expire_timeout)` → the desktop's number for it. The first
/// action is the desktop's own "default" — the notification itself,
/// clicked — then the app's buttons, key and label in turn.
unsafe fn send_notify(
    session: *mut c_void,
    name: &CStr,
    letter: &Notification,
    replaces: u32,
) -> Option<u32> {
    unsafe {
        let message = dbus_message_new_method_call(
            c"org.freedesktop.Notifications".as_ptr(),
            c"/org/freedesktop/Notifications".as_ptr(),
            c"org.freedesktop.Notifications".as_ptr(),
            c"Notify".as_ptr(),
        );
        if message.is_null() {
            return None;
        }
        let summary = CString::new(letter.title.as_str()).unwrap_or_default();
        let body = CString::new(letter.body.as_str()).unwrap_or_default();
        let mut iter = DbusIter::new();
        dbus_message_iter_init_append(message, &mut iter);
        append_string(&mut iter, name);
        dbus_message_iter_append_basic(&mut iter, DBUS_TYPE_UINT32, (&raw const replaces).cast());
        append_string(&mut iter, c"");
        append_string(&mut iter, &summary);
        append_string(&mut iter, &body);
        let mut actions = DbusIter::new();
        dbus_message_iter_open_container(&mut iter, DBUS_TYPE_ARRAY, c"s".as_ptr(), &mut actions);
        append_string(&mut actions, c"default");
        append_string(&mut actions, c"Open");
        for action in &letter.actions {
            let key = CString::new(action.key.as_str()).unwrap_or_default();
            let label = CString::new(action.label.as_str()).unwrap_or_default();
            append_string(&mut actions, &key);
            append_string(&mut actions, &label);
        }
        dbus_message_iter_close_container(&mut iter, &mut actions);
        let mut hints = DbusIter::new();
        dbus_message_iter_open_container(&mut iter, DBUS_TYPE_ARRAY, c"{sv}".as_ptr(), &mut hints);
        dbus_message_iter_close_container(&mut iter, &mut hints);
        let forever: i32 = -1;
        dbus_message_iter_append_basic(&mut iter, DBUS_TYPE_INT32, (&raw const forever).cast());

        let mut error = DbusError::new();
        let reply = dbus_connection_send_with_reply_and_block(
            session,
            message,
            REPLY_TIMEOUT_MS,
            &mut error,
        );
        dbus_message_unref(message);
        let _ = error.take();
        if reply.is_null() {
            return None;
        }
        let mut answer = DbusIter::new();
        let number = if dbus_message_iter_init(reply, &mut answer) != 0 {
            read_u32(&mut answer)
        } else {
            None
        };
        dbus_message_unref(reply);
        number
    }
}

/// `ActionInvoked(id, action_key)` — the person acted; the desktop's
/// "default" is the notification itself. `NotificationClosed(id,
/// reason)` — the number is spent.
unsafe fn notification_signal(
    message: *mut c_void,
    numbers: &mut HashMap<String, u32>,
    ids: &mut HashMap<u32, String>,
) {
    unsafe {
        let interface = c"org.freedesktop.Notifications";
        if dbus_message_is_signal(message, interface.as_ptr(), c"ActionInvoked".as_ptr()) != 0 {
            let mut iter = DbusIter::new();
            if dbus_message_iter_init(message, &mut iter) == 0 {
                return;
            }
            let Some(number) = read_u32(&mut iter) else {
                return;
            };
            dbus_message_iter_next(&mut iter);
            let Some(key) = read_string(&mut iter) else {
                return;
            };
            if let Some(id) = ids.get(&number) {
                let action = (key != "default").then_some(key);
                emit(AppEvent::NotificationActivated { id: id.clone(), action });
            }
        } else if dbus_message_is_signal(
            message,
            interface.as_ptr(),
            c"NotificationClosed".as_ptr(),
        ) != 0
        {
            let mut iter = DbusIter::new();
            if dbus_message_iter_init(message, &mut iter) == 0 {
                return;
            }
            if let Some(number) = read_u32(&mut iter)
                && let Some(id) = ids.remove(&number)
            {
                numbers.remove(&id);
            }
        }
    }
}

/// `PrepareForSleep(true)` is the sleep, `(false)` the wake.
unsafe fn sleep_signal(message: *mut c_void) {
    unsafe {
        if dbus_message_is_signal(
            message,
            c"org.freedesktop.login1.Manager".as_ptr(),
            c"PrepareForSleep".as_ptr(),
        ) == 0
        {
            return;
        }
        let mut iter = DbusIter::new();
        if dbus_message_iter_init(message, &mut iter) == 0
            || dbus_message_iter_get_arg_type(&mut iter) != DBUS_TYPE_BOOLEAN
        {
            return;
        }
        let mut word: u32 = 0;
        dbus_message_iter_get_basic(&mut iter, (&raw mut word).cast());
        emit(if word != 0 { AppEvent::WillSleep } else { AppEvent::DidWake });
    }
}

unsafe fn append_string(iter: &mut DbusIter, text: &CStr) {
    unsafe {
        let pointer = text.as_ptr();
        dbus_message_iter_append_basic(iter, DBUS_TYPE_STRING, (&raw const pointer).cast());
    }
}

unsafe fn read_u32(iter: &mut DbusIter) -> Option<u32> {
    unsafe {
        if dbus_message_iter_get_arg_type(iter) != DBUS_TYPE_UINT32 {
            return None;
        }
        let mut value: u32 = 0;
        dbus_message_iter_get_basic(iter, (&raw mut value).cast());
        Some(value)
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
