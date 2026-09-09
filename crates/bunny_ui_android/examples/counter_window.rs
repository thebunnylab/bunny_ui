//! The counter on the phone — the first element on the screen.
//!
//! ```sh
//! crates/bunny_ui_android/android/run-emu.sh counter_window_android
//! ```

#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports, unused_variables))]

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

fn main() {
    let counter = Counter { count: State::new(0) };
    #[cfg(target_os = "android")]
    bunny_ui_android::run_window("bunny_ui", Size { width: 280.0, height: 180.0 }, counter);
}

// the activity's entry point, in the app's own shared object
#[cfg(target_os = "android")]
bunny_ui_android::activity!(main);
