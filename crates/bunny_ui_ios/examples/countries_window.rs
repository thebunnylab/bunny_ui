//! The real CountriesSwiftUI on the phone — a list with platform text
//! and real scrolling, under the finger:
//!
//! ```sh
//! crates/bunny_ui_ios/simulator/run-sim.sh countries_window_ios
//! ```
//!
//! Limitations of this phase (noted, not hidden): `navigation_link` does
//! not mount a destination yet (tapping a row does not navigate);
//! `.searchable`/`.refreshable`/`.toolbar` are inert; images are 40×40 boxes.

#![cfg_attr(not(target_os = "ios"), allow(dead_code, unused_imports))]

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
#[cfg(target_os = "ios")]
use bunny_ui_ios::CoreTextEngine;
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_pure::root_view;

#[cfg(target_os = "ios")]
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
    app.systemEventsHandler.sceneDidBecomeActive();

    bunny_ui_ios::run_window_with("Countries", Size { width: 480.0, height: 360.0 }, runtime, root);
}

#[cfg(not(target_os = "ios"))]
fn main() {} // this example is iOS-only
