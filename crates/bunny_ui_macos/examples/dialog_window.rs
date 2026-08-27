//! A dialog window with JetBrains manners — the proof, by hand.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example dialog_window [-- --open]
//! ```
//!
//! `--open` boots with the dialog already up (the headless-adjacent
//! probe: two windows on the very first frame).
//!
//! The probe script this example exists to run:
//! 1. "Open Settings" raises a REAL titled window, centered over this
//!    one: the yellow light is disabled and dead; the green one ZOOMS
//!    in place (never native fullscreen); the corner resizes, and
//!    refuses to go under 480×320.
//! 2. While it is up, this window's own lights are dark and every
//!    click, hover and drag on it does nothing — no scrim, and a press
//!    beside the dialog does NOT dismiss it.
//! 3. Put THIS window into native fullscreen first: the dialog opens
//!    over the fullscreen space, no space switch; closing it hands the
//!    keyboard back (type to prove).
//! 4. Typing (and the IME) lands in the dialog's field; the dropdown
//!    opens ABOVE the dialog and follows when the dialog is dragged.
//! 5. The red button and the Close button close through the same
//!    binding; reopening lands where the window was left.
//! 6. Cmd-tab away and back: the dialog stands, and it is still key.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::layout::{DialogSpec, Size};
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Page {
    open: State<bool>,
    theme: State<bool>,
    name: State<String>,
    reached: State<i32>,
}

impl Component for Page {
    fn body(self, _ctx: &Context) -> impl View {
        let open = self.open;
        let theme = self.theme;
        let reached = self.reached;
        vstack!(
            text("The page behind the dialog").font(Font::Title),
            // the counter is the modal proof: while the dialog is up,
            // no click may reach this button
            text!("Clicks that reached me: {}", self.reached),
            button(text("I count clicks"), move || reached.add(1)),
            spacer(),
            button(text("Open Settings"), move || open.set(true)),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
        .dialog(
            self.open.binding(),
            DialogSpec::titled("Settings").min_size(480.0, 320.0),
            move |_| {
                erased(
                    vstack!(
                        text("A real window with dialog manners").font(Font::Title),
                        text_field(
                            "type here — the keyboard is the dialog's",
                            self.name.binding(),
                        ),
                        button(text("Theme ▾"), move || theme.set(true)).popover(
                            theme.binding(),
                            Side::Bottom,
                            |_| {
                                erased(
                                    vstack!(
                                        text("Islands Dark"),
                                        text("Islands Light"),
                                    )
                                    .alignment(HorizontalAlignment::Leading)
                                    .padding(),
                                )
                            },
                        ),
                        spacer(),
                        button(text("Close"), move || open.set(false)),
                    )
                    .alignment(HorizontalAlignment::Leading)
                    .padding(),
                )
            },
        )
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let open = std::env::args().any(|arg| arg == "--open");
    let page = Page {
        open: State::new(open),
        theme: State::new(false),
        name: State::new(String::new()),
        reached: State::new(0),
    };
    bunny_ui_macos::run_window(
        "The page behind",
        Size { width: 900.0, height: 640.0 },
        page,
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
