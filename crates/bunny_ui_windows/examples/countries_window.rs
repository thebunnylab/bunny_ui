//! The real CountriesSwiftUI in a native window — a list with platform
//! text and real scrolling:
//!
//! ```sh
//! cargo run -p bunny-ui-windows --example countries_window
//! ```
//!
//! The list is a navigation stack: clicking a row opens the details of its
//! country as a page over the list, and a neighbour in the details opens
//! another page over that one.
//!
//! The stack's bar shows the title of the page, the button that goes back
//! and the items the page puts in it.
//!
//! The list is searchable: the stack puts the field in its bar.
//!
//! Limitations of this phase (noted, not hidden):
//! `.refreshable` is inert; images are 40×40 boxes.

#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
#[cfg(target_os = "windows")]
use bunny_ui_windows::DirectWriteEngine;
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_pure::root_view;

#[cfg(target_os = "windows")]
fn main() {
    // same assembly order as the headless demo
    let app = AppEnvironment::bootstrap();
    // the shell reports the system's languages into the environment;
    // the demo's translations are keyed by language, so the list is
    // served in the reader's own
    let runtime = Runtime::new().text_engine(Rc::new(DirectWriteEngine::new()));
    let ctx = runtime.context();
    let root = root_view(&app, &ctx);

    // the scene activates before the first frame (blur → 0, push resolves)
    // — the real applicationDidBecomeActive hook is a future lifecycle step
    app.systemEventsHandler.sceneDidBecomeActive();

    bunny_ui_windows::run_window_with(
        "Countries",
        Size { width: 480.0, height: 360.0 },
        runtime,
        root,
    );
}

#[cfg(not(target_os = "windows"))]
fn main() {} // this example is Windows-only
