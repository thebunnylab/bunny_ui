//! Declarative UI in Rust with fine-grained reactivity, inspired by SwiftUI.
//!
//! This is the crate an application adds. It re-exports the core
//! (`bunny_ui_core`) whole and, with the default `shell` feature, the
//! shell of the target it compiles for: macOS, iOS, Windows, Linux,
//! Android or the web. One line starts the app on all of them —
//! [`app!`] writes the entry each target expects:
//!
//! ```no_run
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
//! fn home() -> impl View {
//!     Counter { count: State::new(0) }
//! }
//!
//! bunny_ui::app!(home, bunny_ui::AppConfig::new().size(280.0, 180.0));
//!
//! fn main() {
//!     run()
//! }
//! ```
//!
//! Underneath, `run_window` opens the window on every native platform
//! with the same signature. What a shell offers beyond that one window
//! — who draws a desktop window's title bar, several windows, the
//! Android activity, the web's start functions — lives in `platform`,
//! the shell crate itself.
//!
//! A crate that only builds views, such as a component library or a
//! theme, depends on `bunny-ui` with `default-features = false`: the
//! views without a shell, the window left to the application.
//!
//! `bunny run` reloads an app hot on macOS and Linux. It builds the app
//! with the `hot` feature: the framework comes in as one shared library,
//! and [`app!`] makes each new build of the app's library loadable by
//! the running app, which keeps its state. An app does not turn the
//! feature on itself.

#![forbid(unsafe_code)]

mod entry;

// A hot build reaches the framework through its one shared library.
// Naming the crate is what links it: without the name, the compiler
// would link the framework into the app statically and leave the shared
// library out.
#[cfg(all(feature = "hot", any(target_os = "macos", target_os = "linux")))]
extern crate bunny_ui_dylib as _;

pub use bunny_ui_core::*;
pub use entry::{__private, AppConfig};

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
