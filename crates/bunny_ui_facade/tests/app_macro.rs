//! `app!` in an app that forbids `unsafe`: the entry it writes for
//! Android and the web is the macro's own `unsafe`, never the app's, so
//! a crate that keeps the strictest lint still starts everywhere.
//! `cargo check --tests --target …` for each platform compiles the
//! expansion that platform keeps.

#![forbid(unsafe_code)]

use bunny_ui::prelude::*;

fn home() -> impl View {
    text("Hello")
}

bunny_ui::app!(home, bunny_ui::AppConfig::new().title("Hello").size(320.0, 240.0));

/// The entry is a plain `fn()` — what `main` calls, what the Android
/// activity is handed.
#[test]
fn run_is_the_entry() {
    let entry: fn() = run;
    let _ = entry;
}

/// The default configuration is the house window, titled after the app.
#[test]
fn the_house_window_is_the_default() {
    let config = format!("{:?}", bunny_ui::AppConfig::default());
    assert!(config.contains("title: None"), "{config}");
    assert!(config.contains("1024"), "{config}");
}

/// The reserved entry uses the configured window and remains callable under forbid(unsafe_code).
#[cfg(all(feature = "preview", not(target_arch = "wasm32")))]
#[test]
fn the_app_entry_carries_the_window_size() {
    let entry = bunny_ui::__private::hot::preview::preview_entry("app").unwrap();
    assert_eq!(entry.meta.scope, "App");
    assert_eq!(entry.meta.name, "App");
    assert_eq!(entry.meta.example, env!("CARGO_PKG_NAME"));
    assert_eq!(
        (entry.make)().traits.size,
        bunny_ui::preview::Sizing::Fixed(bunny_ui::layout::Size {
            width: 320.0,
            height: 240.0
        })
    );
}
