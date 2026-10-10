//! One app, every platform: the entry each target expects, from one
//! line at the root of the app's library.
//!
//! The desktop and iOS start from a `main` that opens the window; an
//! Android activity starts from a symbol the system looks up in the
//! app's shared object; a page starts from an export its script calls.
//! [`app!`](crate::app!) writes all three, and the target keeps the one
//! it runs:
//!
//! ```no_run
//! use bunny_ui::prelude::*;
//!
//! fn home() -> impl View {
//!     text("Hello")
//! }
//!
//! bunny_ui::app!(home, bunny_ui::AppConfig::new().size(420.0, 640.0));
//!
//! fn main() {
//!     run()
//! }
//! ```
//!
//! In a project the macro sits in `src/lib.rs` and `src/main.rs` is the
//! one line `fn main() { my_app::run() }`: the desktop and iOS build the
//! binary, Android and the web build the library.

use bunny_ui_core::layout::Size;

/// What the app asks of its window, where the platform gives it one to
/// shape — the desktop's. A phone hands the app its screen and a page
/// its element, and both leave this unread.
#[derive(Clone, Debug)]
pub struct AppConfig {
    title: Option<String>,
    size: Size,
}

impl AppConfig {
    /// The window titled after the app, at the house size.
    pub fn new() -> AppConfig {
        AppConfig { title: None, size: Size { width: 1024.0, height: 640.0 } }
    }

    /// The window's title, when it is not the app's name.
    pub fn title(mut self, title: impl Into<String>) -> AppConfig {
        self.title = Some(title.into());
        self
    }

    /// The window's first size, in points.
    pub fn size(mut self, width: f64, height: f64) -> AppConfig {
        self.size = Size { width, height };
        self
    }
}

impl Default for AppConfig {
    fn default() -> AppConfig {
        AppConfig::new()
    }
}

/// Starts the app on every platform from one root: `$root` is the
/// function (or closure) that builds the first view, `$config` an
/// [`AppConfig`] — the house one when left out.
///
/// It defines `pub fn run()` at the crate's root — what `main` calls on
/// the desktop and iOS, and what the Android activity runs when the
/// system creates it — and, on the web, the `start` export the page's
/// script boots. Call it once, at the root of the app's library.
///
/// The app's name and id are the ones `bunny` builds with
/// (`[package.metadata.bunny]`); a plain `cargo build` names the app
/// after its package.
#[macro_export]
macro_rules! app {
    ($root:expr $(,)?) => {
        $crate::app!($root, $crate::AppConfig::new());
    };
    ($root:expr, $config:expr $(,)?) => {
        /// Starts the app: the window on the desktop and iOS, the
        /// activity's content on Android. The web starts from the
        /// page's `start` export instead, and this returns at once.
        pub fn run() {
            $crate::__private::run($crate::__bunny_identity!(), $config, $root)
        }
        $crate::__bunny_entry!(run, $root, $config);
    };
}

/// The app's identity as its OWN crate was built: expanded there, the
/// package's name and version are the app's, and so is what `bunny`
/// set in the environment of that build.
#[doc(hidden)]
#[macro_export]
macro_rules! __bunny_identity {
    () => {
        $crate::app::Identity {
            name: match ::core::option_env!("BUNNY_APP_NAME") {
                ::core::option::Option::Some(name) => name,
                ::core::option::Option::None => ::core::env!("CARGO_PKG_NAME"),
            },
            id: ::core::option_env!("BUNNY_APP_ID"),
            version: ::core::env!("CARGO_PKG_VERSION"),
        }
    };
}

// The entry the target looks up, decided HERE: a cfg inside the expanded
// tokens would test the app's features, not this crate's. Each export
// sits in an anonymous const, out of the app's namespace — the symbol
// is the system's business, not a name the app can collide with.

/// Android: the activity's entry symbol, running `run`.
#[cfg(all(feature = "shell", target_os = "android"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __bunny_entry {
    ($run:ident, $root:expr, $config:expr) => {
        const _: () = {
            $crate::platform::activity!($run);
        };
    };
}

/// The web: the export the page's script boots, with the element's size
/// and the device pixel ratio.
#[cfg(all(feature = "shell", target_arch = "wasm32"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __bunny_entry {
    ($run:ident, $root:expr, $config:expr) => {
        const _: () = {
            #[unsafe(no_mangle)]
            pub extern "C" fn start(width: f64, height: f64, scale: f64) {
                $crate::__private::start(
                    $crate::__bunny_identity!(),
                    $config,
                    $root,
                    width,
                    height,
                    scale,
                )
            }
        };
    };
}

/// The desktop and iOS: `main` calls `run`, nothing more to export.
#[cfg(all(
    feature = "shell",
    any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux"),
    not(all(feature = "hot", any(target_os = "macos", target_os = "linux"))),
))]
#[doc(hidden)]
#[macro_export]
macro_rules! __bunny_entry {
    ($run:ident, $root:expr, $config:expr) => {};
}

/// A hot build on the desktop (`bunny run`): `main` calls `run`, and
/// each build of the app's library exports the entry of a generation —
/// what the running app loads after a save. A function in the Rust ABI:
/// only an app built by the same compiler loads it.
#[cfg(all(feature = "shell", feature = "hot", any(target_os = "macos", target_os = "linux")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __bunny_entry {
    ($run:ident, $root:expr, $config:expr) => {
        const _: () = {
            fn root() -> $crate::__private::hot::Root {
                $crate::__private::hot::Root::new(($root)())
            }
            static ENTRY: $crate::__private::hot::Entry = $crate::__private::hot::Entry::new(root);
            #[unsafe(no_mangle)]
            pub fn bunny_hot_entry_v1() -> &'static $crate::__private::hot::Entry {
                &ENTRY
            }
        };
    };
}

/// No shell, or a target without one: an app has nowhere to start.
#[cfg(not(all(
    feature = "shell",
    any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "linux",
        target_os = "android",
        target_arch = "wasm32",
    )
)))]
#[doc(hidden)]
#[macro_export]
macro_rules! __bunny_entry {
    ($run:ident, $root:expr, $config:expr) => {
        ::core::compile_error!(
            "`bunny_ui::app!` needs the `shell` feature of bunny-ui and a target it has a \
             shell for: macOS, iOS, Windows, Linux, Android or the web"
        );
    };
}

/// What the macro expands to calls, so the app's crate names nothing
/// but `bunny_ui`.
#[doc(hidden)]
pub mod __private {
    use super::AppConfig;
    use bunny_ui_core::app::{Identity, set_identity};
    use bunny_ui_core::view::View;

    /// The part of hot reload inside the app.
    #[cfg(all(feature = "hot", any(target_os = "macos", target_os = "linux")))]
    pub use bunny_ui_hot as hot;

    /// Says who the app is, then opens its window with the first view.
    #[cfg(all(
        feature = "shell",
        any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "linux",
            target_os = "android",
        ),
        not(all(feature = "hot", any(target_os = "macos", target_os = "linux"))),
    ))]
    pub fn run<V: View>(identity: Identity, config: AppConfig, root: impl FnOnce() -> V) {
        set_identity(identity);
        let title = config.title.unwrap_or_else(|| identity.name.to_string());
        crate::run_window(&title, config.size, root())
    }

    /// A hot build: the window opens on the view of the generation
    /// `bunny run` names, and each save brings the next one. The first
    /// view is built again with each generation's code, inside the pass,
    /// so the state it makes is found again — `root` runs more than once.
    #[cfg(all(feature = "shell", feature = "hot", any(target_os = "macos", target_os = "linux")))]
    pub fn run<V: View>(identity: Identity, config: AppConfig, root: impl Fn() -> V + 'static) {
        set_identity(identity);
        let title = config.title.unwrap_or_else(|| identity.name.to_string());
        let root = hot::start(move || hot::Root::new(root()));
        crate::run_window(&title, config.size, root)
    }

    /// The page starts from `start`; here there is only the name to say.
    #[cfg(not(all(
        feature = "shell",
        any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "linux",
            target_os = "android",
        )
    )))]
    pub fn run<V: View>(identity: Identity, _config: AppConfig, _root: impl FnOnce() -> V) {
        set_identity(identity);
    }

    /// The page's entry: who the app is, then the shell on the element
    /// the script measured.
    #[cfg(all(feature = "shell", target_arch = "wasm32"))]
    pub fn start<V: View>(
        identity: Identity,
        _config: AppConfig,
        root: impl FnOnce() -> V,
        width: f64,
        height: f64,
        scale: f64,
    ) {
        set_identity(identity);
        crate::platform::start(width, height, scale, root())
    }
}
