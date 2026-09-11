//! The bunny-ui Android shell: the phone's one window over an
//! `android.app.NativeActivity`, touches and the soft keyboard,
//! presented by the shared Vulkan tier or by the CPU floor — the live
//! cycle on a screen the finger drives. Not a single dependency.
//!
//! The activity is spoken by hand: its callback table, the main
//! looper our clocks and the input queue ride, the window, the
//! configuration (`ffi`). Everything runs on the UI thread, where the
//! system calls the activity and where the runtime lives — no glue
//! thread, no command pipe, no acknowledgement to wait on. The core
//! hears touches through its own touch doors and lays the root out
//! inside the safe area; the shell only forwards.
//!
//! An app enters through [`activity!`]: the macro exports the entry
//! the system looks for in the app's shared object, and hands it the
//! app's own `main`, which opens the window like on every other shell.
//!
//! What this shell does not have, said once: a second window
//! ([`MANY_WINDOWS`] is false — the screen is the window), a title bar
//! or a chrome to choose, a cursor, an IME road for composed text (a
//! `NativeActivity` receives KEYS, and the keys of a latin layout are
//! all that type; a composition, a script the table does not know, an
//! emoji need an `InputConnection`, which is Java), a hosted web view
//! (also Java), and notifications (`bunny_ui::app::notify` answers by
//! name that this shell has none). The scale is a whole number: the
//! density over 160, rounded — a 420 dpi phone lays out at 3× and 360
//! points wide.
//!
//! The project's `unsafe` lives ONLY in the shell crates (here, the
//! FFI and the JNI), wrapped in this safe API. The core and the facade
//! keep `#![forbid(unsafe_code)]`.

pub mod keys;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod face;

#[cfg(target_os = "android")]
#[macro_use]
mod log;
#[cfg(target_os = "android")]
mod jni;
#[cfg(target_os = "android")]
mod ffi;
#[cfg(target_os = "android")]
mod image;
#[cfg(target_os = "android")]
mod text;
#[cfg(target_os = "android")]
mod app;

#[cfg(target_os = "android")]
pub use app::{run_window, run_window_with, App, WindowId, WindowSpec, MANY_WINDOWS};
#[cfg(target_os = "android")]
pub use image::AndroidImageEngine;
#[cfg(target_os = "android")]
pub use text::AndroidTextEngine;

/// Where this app may WRITE: the activity's own private files directory.
///
/// Every other shell hands an app a home the OS already named — `$HOME` on
/// the Unixes, the container on iOS, `%APPDATA%` on Windows. Android names
/// none: an app process inherits no writable path in its environment, and the
/// one directory it may write without a permission and without asking is the
/// activity's, which only the activity knows. So the shell answers, the way
/// it answers for the clipboard and the insets.
///
/// `None` before the system has handed an activity over, and if the platform
/// left the path null — an app that cannot write is a sentence to say, not a
/// path to guess.
#[cfg(target_os = "android")]
pub fn data_dir() -> Option<std::path::PathBuf> {
    ffi::internal_data_path().map(std::path::PathBuf::from)
}

/// Exports the entry point the system looks for — `ANativeActivity_onCreate`
/// — from the app's own shared object, and hands it `$main`: the
/// function that builds the app and opens its window, the same `main`
/// the other shells run.
///
/// The symbol is defined in the app's crate on purpose: a `no_mangle`
/// in a library only reaches the shared object when its unit is
/// linked in, and here it always is.
///
/// ```ignore
/// fn main() {
///     bunny_ui_android::run_window("Counter", Size { width: 280.0, height: 180.0 }, counter);
/// }
/// #[cfg(target_os = "android")]
/// bunny_ui_android::activity!(main);
/// ```
#[macro_export]
macro_rules! activity {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn ANativeActivity_onCreate(
            activity: *mut ::core::ffi::c_void,
            _saved_state: *mut ::core::ffi::c_void,
            _saved_state_size: usize,
        ) {
            unsafe { $crate::on_create(activity, $main) }
        }
    };
}

/// The entry's body, behind [`activity!`].
///
/// # Safety
///
/// `activity` is the `ANativeActivity` the system handed the entry.
#[cfg(target_os = "android")]
#[doc(hidden)]
pub unsafe fn on_create(activity: *mut ::core::ffi::c_void, main: fn()) {
    unsafe { ffi::on_create(activity.cast(), main) }
}
