//! The counter in a real window — the first element on the screen.
//!
//! ```sh
//! cargo run -p bunny-ui-windows --example counter_window
//! ```

#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(self, _ctx: &Context) -> impl View {
        vstack!(
            text!("Count: {}", self.count).font(Font::Title),
            spacer(),
            button(text("Tap me!"), move || self.count.add(1)),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

#[cfg(target_os = "windows")]
fn main() {
    let counter = Counter { count: State::new(0) };
    bunny_ui_windows::run_window(
        "bunny_ui",
        Size { width: 280.0, height: 180.0 },
        counter,
    );
}

#[cfg(not(target_os = "windows"))]
fn main() {} // this example is Windows-only
