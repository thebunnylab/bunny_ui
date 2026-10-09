//! An alert with the system's own alerts' manners — the proof, by hand.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example alert_window [-- --open]
//! ```
//!
//! `--open` boots with the alert already up.
//!
//! The probe script this example exists to run:
//! 1. Click the field and type: it holds the keyboard.
//! 2. "Delete…" raises a small window centred over this one: no bar, no
//!    lights, the system's shadow and corners. Its corner does not resize
//!    it; nothing zooms or minimizes it.
//! 3. While it is up, this window is inert (its lights dark, the counter
//!    unreachable) and typing reaches nothing — not the field under it.
//! 4. Escape, and ⌘., are Cancel; Return is Delete. Each closes the alert
//!    and the field has the keyboard again without a click (type to prove).
//! 5. "Make it taller" inside the alert grows the window where it stands.
//! 6. Drag the alert by its ground, answer it, reopen: it opens centred
//!    again — a question appears where questions appear.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::action::ALERT_DEFAULT;
use bunny_ui::layout::{AlertSpec, Size};
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Page {
    asking: State<bool>,
    long: State<bool>,
    note: State<String>,
    reached: State<i32>,
    answered: State<&'static str>,
}

impl Component for Page {
    fn body(self) -> impl View {
        let (asking, long, reached, answered) = (self.asking, self.long, self.reached, self.answered);
        vstack!(
            text("The page behind the alert").font(Font::Title),
            text_field("type here — the keyboard is the page's until it asks", self.note.binding()),
            // the modal proof: while the alert is up, no click reaches this
            text!("Clicks that reached me: {}", self.reached),
            button(text("I count clicks"), move || reached.add(1)),
            text!("Last answer: {}", self.answered),
            spacer(),
            button(text("Delete…"), move || asking.set(true)),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
        .alert(
            // the binding's `false` IS the cancel answer: Escape, ⌘. and a
            // close button all write it
            Binding::new(
                move || asking.get(),
                move |shown: bool| {
                    if !shown {
                        asking.set(false);
                        answered.set("cancel");
                    }
                },
            ),
            AlertSpec::new("Delete", 380.0),
            move |_| {
                let delete = move || {
                    asking.set(false);
                    answered.set("delete");
                };
                erased(
                    vstack!(
                        text("Delete the file notes.md?").bold(),
                        text(if long.get() {
                            "It leaves the disk, and this cannot be undone. Everything written \
                             in it since the last commit goes with it, and nothing in the \
                             workbench can bring it back."
                        } else {
                            "It leaves the disk — this cannot be undone."
                        }),
                        hstack!(
                            button(text("Make it taller"), move || long.set(!long.get())),
                            spacer(),
                            button(text("Cancel"), move || {
                                asking.set(false);
                                answered.set("cancel");
                            }),
                            button(text("Delete"), delete),
                        ),
                    )
                    .alignment(HorizontalAlignment::Leading)
                    .spacing(12.0)
                    .padding()
                    // Return presses Delete: the default is the content's to name
                    .on_action(ALERT_DEFAULT, delete),
                )
            },
        )
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let open = std::env::args().any(|arg| arg == "--open");
    let page = Page {
        asking: State::new(open),
        long: State::new(false),
        note: State::new(String::new()),
        reached: State::new(0),
        answered: State::new("none yet"),
    };
    bunny_ui_macos::run_window(
        "The page behind",
        Size { width: 900.0, height: 640.0 },
        page,
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
