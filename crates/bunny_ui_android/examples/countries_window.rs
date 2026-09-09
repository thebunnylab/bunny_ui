//! The real CountriesSwiftUI on the phone — a list with real scrolling,
//! under the finger (the pixel font stands in until the platform text
//! engine lands):
//!
//! ```sh
//! crates/bunny_ui_android/android/run-emu.sh countries_window_android
//! ```
//!
//! Limitations of this phase (noted, not hidden): `navigation_link` does
//! not mount a destination yet (tapping a row does not navigate);
//! `.searchable`/`.refreshable`/`.toolbar` are inert; images are 40×40 boxes.

#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports, unused_variables))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_pure::root_view;

fn main() {
    // same assembly order as the headless demo
    let app = AppEnvironment::bootstrap();
    let mut environment = EnvironmentValues::default();
    environment.locale = Locale::new("en");
    let runtime = Runtime::with_environment(environment);
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
