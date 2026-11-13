//! The real CountriesSwiftUI on the phone — a list with platform text
//! and real scrolling, under the finger:
//!
//! ```sh
//! crates/bunny_ui_android/android/run-emu.sh countries_window_android
//! ```
//!
//! The list is a navigation stack: tapping a row opens the details of its
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

#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports, unused_variables))]

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
#[cfg(target_os = "android")]
use bunny_ui_android::AndroidTextEngine;
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_pure::root_view;

fn main() {
    // same assembly order as the headless demo
    let app = AppEnvironment::bootstrap();
    // the shell reports the system's languages into the environment;
    // the demo's translations are keyed by language, so the list is
    // served in the reader's own
    #[cfg(target_os = "android")]
    let runtime = Runtime::new().text_engine(Rc::new(AndroidTextEngine::new()));
    #[cfg(not(target_os = "android"))]
    let runtime = Runtime::new();
    let ctx = runtime.context();
    let root = root_view(&app, &ctx);

    // the scene activates before the first frame (blur → 0, push resolves)
    app.systemEventsHandler.sceneDidBecomeActive();

    #[cfg(target_os = "android")]
    bunny_ui_android::run_window_with("Countries", Size { width: 480.0, height: 360.0 }, runtime, root);
}

// the activity's entry point, in the app's own shared object
#[cfg(target_os = "android")]
bunny_ui_android::activity!(main);
