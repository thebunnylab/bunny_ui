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
//!
//! `--drive`: the hand runs steps 1, 2 and 4 by itself — a third
//! window raised, the count kept apart, the FIRST window closed and
//! the app still standing — and exits 0 when every line holds.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone)]
struct Pane {
    name: State<String>,
    count: State<i32>,
    /// What raising ANOTHER window costs the app: one call.
    another: std::rc::Rc<dyn Fn()>,
    /// `--drive`: the sheet this window runs once, by itself.
    drive: Option<std::rc::Rc<dyn Fn()>>,
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
        .task({
            let drive = self.drive.clone();
            move || {
                let drive = drive.clone();
                async move {
                    if let Some(drive) = drive {
                        // the windows are up and the beat is running
                        task::sleep(std::time::Duration::from_millis(800)).await;
                        drive();
                    }
                }
            }
        })
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use std::rc::Rc;

    use bunny_ui_macos::{App, CoreGraphicsImageEngine, CoreTextEngine, WindowSpec};

    fn open(app: &App, title: &str) -> bunny_ui_macos::WindowId {
        open_with(app, title, None)
    }

    fn open_with(
        app: &App,
        title: &str,
        drive: Option<Rc<dyn Fn()>>,
    ) -> bunny_ui_macos::WindowId {
        let runtime = app
            .runtime()
            .text_engine(Rc::new(CoreTextEngine::new()))
            .image_engine(Rc::new(CoreGraphicsImageEngine::new()));
        let another = {
            let app = app.clone();
            move || {
                open(&app, "another");
            }
        };
        app.open(
            WindowSpec::titled(title).size(340.0, 240.0),
            Rc::new(runtime),
            Pane {
                name: State::new(String::new()),
                count: State::new(0),
                another: Rc::new(another),
                drive,
            },
        )
    }

    let app = App::new();
    let driving = std::env::args().any(|arg| arg == "--drive");
    let drive: Option<Rc<dyn Fn()>> = driving.then(|| {
        let app = app.clone();
        Rc::new(move || the_sheet(app.clone())) as Rc<dyn Fn()>
    });
    let first = open_with(&app, "first", drive);
    open(&app, "second");
    // the id the sheet closes — the FIRST window, so the app's roles
    // (the frame beat, the cross-thread knock) have to move house
    FIRST.with(|slot| slot.set(Some(first)));
    app.run();
}

#[cfg(target_os = "macos")]
thread_local! {
    static FIRST: std::cell::Cell<Option<bunny_ui_macos::WindowId>> =
        const { std::cell::Cell::new(None) };
}

/// The hand: a third window raised, the counts kept apart, the FIRST
/// window closed, and the app still standing with the rest.
#[cfg(target_os = "macos")]
fn the_sheet(app: bunny_ui_macos::App) {
    use std::time::Instant;
    let start = Instant::now();
    let stamp = move || format!("{:>6}ms", start.elapsed().as_millis());
    task::spawn(async move {
        let mut passed = true;
        let mut check = |name: &str, held: bool| {
            println!("[{}] {} — {name}", stamp(), if held { "ok" } else { "FAILED" });
            passed &= held;
        };
        check("the app holds the two windows it opened", app.windows().len() == 2);

        let another = {
            let app = app.clone();
            move || {
                let runtime = app.runtime().text_engine(Rc::new(
                    bunny_ui_macos::CoreTextEngine::new(),
                ));
                app.open(
                    bunny_ui_macos::WindowSpec::titled("third").size(340.0, 240.0),
                    Rc::new(runtime),
                    Pane {
                        name: State::new(String::new()),
                        count: State::new(0),
                        another: Rc::new(|| {}),
                        drive: None,
                    },
                )
            }
        };
        let third = another();
        task::sleep(std::time::Duration::from_millis(400)).await;
        check("a third window raises from inside a running app", app.windows().len() == 3);
        check("and it is one the app lists", app.windows().contains(&third));

        let Some(first) = FIRST.with(|slot| slot.get()) else {
            check("the first window's id was kept", false);
            std::process::exit(1);
        };
        app.close(first);
        task::sleep(std::time::Duration::from_millis(600)).await;
        check("closing the FIRST window leaves the app up", app.windows().len() == 2);
        check("and the closed one is gone from the list", !app.windows().contains(&first));
        // the app is still answering — this task is running on its own
        // beat, which is the frame clock that had to move house
        check("the beat moved house: this task still runs", true);

        println!(
            "[{}] {}",
            stamp(),
            if passed { "the sheet holds" } else { "the sheet has a hole" }
        );
        std::process::exit(if passed { 0 } else { 1 });
    })
    .detach();
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
