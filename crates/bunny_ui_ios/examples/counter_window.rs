//! The counter on the phone — the first element on the screen.
//!
//! ```sh
//! crates/bunny_ui_ios/simulator/run-sim.sh counter_window_ios
//! ```

#![cfg_attr(not(target_os = "ios"), allow(dead_code, unused_imports))]

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

#[cfg(target_os = "ios")]
fn main() {
    let counter = Counter { count: State::new(0) };
    bunny_ui_ios::run_window("bunny_ui", Size { width: 280.0, height: 180.0 }, counter);
}

#[cfg(not(target_os = "ios"))]
fn main() {} // this example is iOS-only
