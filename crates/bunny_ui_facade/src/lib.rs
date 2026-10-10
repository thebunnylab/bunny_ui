//! Declarative UI in Rust with fine-grained reactivity, inspired by SwiftUI.
//!
//! This is the crate an application adds. It re-exports the core
//! (`bunny_ui_core`) whole and, with the default `shell` feature, the
//! shell of the target it compiles for: macOS, iOS, Windows, Linux,
//! Android or the web. On every native one the same `main` opens the
//! window:
//!
//! ```no_run
//! use bunny_ui::layout::Size;
//! use bunny_ui::prelude::*;
//!
//! #[derive(Clone, Copy)]
//! struct Counter {
//!     count: State<i32>,
//! }
//!
//! impl Component for Counter {
//!     fn body(self) -> impl View {
//!         vstack!(
//!             text!("Count: {}", self.count),
//!             button(text("Tap"), move || self.count.add(1)),
//!         )
//!     }
//! }
//!
//! fn main() {
//!     let counter = Counter { count: State::new(0) };
//!     bunny_ui::run_window("Counter", Size { width: 280.0, height: 180.0 }, counter);
//! }
//! ```
//!
//! What a shell offers beyond that one window — who draws a desktop
//! window's title bar, several windows, the Android activity, the web's
//! start functions — lives in `platform`, the shell crate itself.
//!
//! A crate that only builds views, such as a component library or a
//! theme, depends on `bunny-ui` with `default-features = false`: the
//! views without a shell, the window left to the application.

#![forbid(unsafe_code)]

pub use bunny_ui_core::*;

/// The shell of this target, whole.
#[cfg(all(feature = "shell", target_os = "macos"))]
pub use bunny_ui_macos as platform;
/// The shell of this target, whole.
#[cfg(all(feature = "shell", target_os = "ios"))]
pub use bunny_ui_ios as platform;
/// The shell of this target, whole.
#[cfg(all(feature = "shell", target_os = "windows"))]
pub use bunny_ui_windows as platform;
/// The shell of this target, whole.
#[cfg(all(feature = "shell", target_os = "linux"))]
pub use bunny_ui_linux as platform;
/// The shell of this target, whole.
#[cfg(all(feature = "shell", target_os = "android"))]
pub use bunny_ui_android as platform;
/// The shell of this target, whole.
#[cfg(all(feature = "shell", target_arch = "wasm32"))]
pub use bunny_ui_web as platform;

// One window, one signature, on every native shell. The web starts
// from the page instead (`platform::start`), and anything a single
// platform alone has stays under `platform`.
#[cfg(all(
    feature = "shell",
    any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "linux",
        target_os = "android",
    )
))]
pub use platform::{run_window, run_window_with};
