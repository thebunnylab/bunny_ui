//! The real CountriesSwiftUI in a native window — a list with platform
//! text and real scrolling:
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example countries_window
//! ```
//!
//! Limitations of this phase (noted, not hidden): `navigation_link` does
//! not mount a destination yet (clicking a row does not navigate);
//! `.searchable`/`.refreshable`/`.toolbar` are inert; images are 40×40 boxes.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
#[cfg(target_os = "macos")]
use bunny_ui_macos::CoreTextEngine;
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_pure::root_view;

#[cfg(target_os = "macos")]
fn main() {
    // same assembly order as the headless demo
    let app = AppEnvironment::bootstrap();
    let mut environment = EnvironmentValues::default();
    environment.locale = Locale::new("en");
    let runtime =
        Runtime::with_environment(environment).text_engine(Rc::new(CoreTextEngine::new()));
    let ctx = runtime.context();
    let root = root_view(&app, &ctx);

    // the scene activates before the first frame (blur → 0, push resolves)
    // — the real applicationDidBecomeActive hook is a future lifecycle step
    app.systemEventsHandler.sceneDidBecomeActive();

    bunny_ui_macos::run_window_with(
        "Countries",
        Size { width: 480.0, height: 360.0 },
        runtime,
        root,
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
