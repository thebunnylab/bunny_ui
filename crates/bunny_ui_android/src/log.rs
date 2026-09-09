//! Logcat is the only voice an activity has: its stderr goes nowhere.
//! Every line the shell says goes through `__android_log_write` under
//! the tag `bunny_ui`, and a pump carries what the core and the tiers
//! say on stderr (`eprintln!`) to the same place — so a refused tier
//! is a line in `adb logcat -s bunny_ui`, never a silent blank screen.

use std::ffi::{c_char, c_int, CStr, CString};
use std::io::BufRead;
use std::os::unix::io::FromRawFd;
use std::sync::Once;

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

unsafe extern "C" {
    fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
}

const O_CLOEXEC: c_int = 0o2000000;
/// `PROP_VALUE_MAX` — a property's value never exceeds it.
const PROP_VALUE_MAX: usize = 92;

// android/log.h priorities
pub const INFO: c_int = 4;
pub const WARN: c_int = 5;
pub const ERROR: c_int = 6;
pub const FATAL: c_int = 7;

const TAG: &CStr = c"bunny_ui";

/// One line to logcat under the shell's tag.
pub fn write(priority: c_int, text: &str) {
    let Ok(text) = CString::new(text.replace('\0', "\u{FFFD}")) else { return };
    unsafe { __android_log_write(priority, TAG.as_ptr(), text.as_ptr()) };
}

/// An informational line under the shell's tag.
macro_rules! alog {
    ($($arg:tt)*) => {
        $crate::log::write($crate::log::INFO, &format!($($arg)*))
    };
}

/// A line that names something wrong.
macro_rules! aerr {
    ($($arg:tt)*) => {
        $crate::log::write($crate::log::ERROR, &format!($($arg)*))
    };
}

pub(crate) use alog;

/// Installs the voice, once per process: a panic is a FATAL line, and
/// stdout and stderr flow to logcat as WARN lines.
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| write(FATAL, &format!("panic: {info}"))));
        pump_standard_streams();
    });
}

/// Moves fds 1 and 2 onto a pipe a thread reads line by line.
fn pump_standard_streams() {
    let mut fds = [0 as c_int; 2];
    if unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) } != 0 {
        return;
    }
    unsafe {
        dup2(fds[1], 1);
        dup2(fds[1], 2);
        close(fds[1]);
    }
    let reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let _ = std::thread::Builder::new().name("bunny_ui-log".into()).spawn(move || {
        for line in std::io::BufReader::new(reader).lines().map_while(Result::ok) {
            write(WARN, &line);
        }
    });
}

/// A system property's value, `None` when unset or empty. Apps have no
/// environment, so the switches the other shells read from env vars
/// come from `adb shell setprop debug.bunny.<name> <value>`.
pub fn property(name: &CStr) -> Option<String> {
    let mut value = [0 as c_char; PROP_VALUE_MAX];
    let length = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    if length <= 0 {
        return None;
    }
    let bytes: Vec<u8> = value[..length as usize].iter().map(|&byte| byte as u8).collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `debug.bunny.trace` — every event but the clocks, one line each.
pub fn trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| property(c"debug.bunny.trace").is_some_and(|value| value != "0"))
}
