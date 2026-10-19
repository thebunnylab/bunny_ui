//! bunny-ui as one shared library, for hot reload.
//!
//! `bunny run` builds the framework once per session as this library,
//! and the app's binary and every new build of the app's code link it:
//! the state the framework keeps exists once in the process, and new
//! code finds it where the old code left it. See `bunny-ui-hot`.
//!
//! The crates are here whole; an application names none of them. It
//! depends on `bunny-ui`, whose `hot` feature brings this library in.

pub use bunny_ui as core;
pub use bunny_ui_hot as hot;
pub use motor;

#[cfg(target_os = "macos")]
pub use bunny_ui_macos as platform;

#[cfg(target_os = "ios")]
pub use bunny_ui_ios as platform;

#[cfg(target_os = "windows")]
pub use bunny_ui_windows as platform;

#[cfg(target_os = "linux")]
pub use bunny_ui_linux as platform;

#[cfg(target_os = "android")]
pub use bunny_ui_android as platform;
