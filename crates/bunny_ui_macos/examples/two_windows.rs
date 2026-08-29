//! Two windows, by hand — the proof that a thread can hold more than
//! one scene.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example two_windows
//! ```
//!
//! The probe script this example exists to run:
//! 1. Two windows open. They show the SAME view, so every identity path
//!    under them is identical — which is exactly what a second window
//!    of one app is, and exactly what used to collide.
//! 2. Counting in one leaves the other at zero, whichever you click
//!    first and however you alternate. A click that landed in the
//!    window that merely rendered LAST is the bug this is here to
//!    catch.
//! 3. Typing goes to the window holding the keyboard, and the caret
//!    blinks in that one only.
//! 4. "Open another" raises a third; closing any one of them leaves the
//!    app up, and closing the LAST one quits it. Animations keep
//!    running in the survivors after the first window is the one you
//!    closed — that is the frame beat moving house.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone)]
struct Pane {
    name: State<String>,
    count: State<i32>,
    /// What raising ANOTHER window costs the app: one call.
    another: std::rc::Rc<dyn Fn()>,
}

impl Component for Pane {
    fn body(self, _ctx: &Context) -> impl View {
        let count = self.count;
        let another = std::rc::Rc::clone(&self.another);
        vstack!(
            text!("Count: {}", count).font(Font::Title),
            text_field("type here", self.name.binding()),
            spacer(),
            hstack!(
                button(text("Count"), move || count.add(1)),
                button(text("Open another"), move || another()),
            )
            .spacing(8.0),
        )
        .spacing(12.0)
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use std::rc::Rc;

    use bunny_ui_macos::{App, CoreGraphicsImageEngine, CoreTextEngine, WindowSpec};

    fn open(app: &App, title: &str) {
        let runtime = app
            .runtime()
            .text_engine(Rc::new(CoreTextEngine::new()))
            .image_engine(Rc::new(CoreGraphicsImageEngine::new()));
        let another = {
            let app = app.clone();
            move || open(&app, "another")
        };
        app.open(
            WindowSpec::titled(title).size(340.0, 240.0),
            Rc::new(runtime),
            Pane {
                name: State::new(String::new()),
                count: State::new(0),
                another: Rc::new(another),
            },
        );
    }

    let app = App::new();
    open(&app, "first");
    open(&app, "second");
    app.run();
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
