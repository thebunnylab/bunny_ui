//! The counter from one `main` on every native platform: the facade
//! picks the shell, the application never names one.
//!
//! cargo run -p bunny-ui --example counter

#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(
            text!("Count: {}", self.count).font(Font::Title),
            spacer(),
            button(text("Tap me!"), move || self.count.add(1)),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let counter = Counter { count: State::new(0) };
    bunny_ui::run_window("bunny_ui", Size { width: 280.0, height: 180.0 }, counter);
}

#[cfg(target_arch = "wasm32")]
fn main() {} // the web starts from the page: `bunny_ui::platform::start`
