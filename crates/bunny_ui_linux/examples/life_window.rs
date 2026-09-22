//! The app's life outside its window — `bunny_ui::app`.
//!
//! A mail client is one process, hears the machine wake, and says "a
//! letter arrived" in the desktop's own notification with a button on
//! it. This window shows every such event as it lands, and offers the
//! two asks: a notification, and a second launch of this very binary
//! (which reaches THIS process and exits).
//!
//! What to check by hand:
//! - "a second launch" spawns this binary again with arguments: the
//!   second process forwards them and exits, and the log here shows
//!   `Reopened` with the arguments — the spool road;
//! - "notify": the desktop's own notification, over the session bus
//!   (`org.freedesktop.Notifications`); a click on it or on "Open"
//!   lands here as `NotificationActivated` with the id and the button;
//!   a box with no daemon on the bus is refused by name;
//! - close the lid, open it: `WillSleep`, `DidWake`.
//!
//! `--drive`: the second launch is spawned by the hand, the reopen is
//! awaited, the notification is asked for and its answer read (`Ok`
//! under a desktop, the refusal by name without one); the process
//! exits 0 when every line holds.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::sync::OnceLock;
use std::time::Instant;

use bunny_ui::app::{AppEvent, Instance, Notification};
use bunny_ui::prelude::*;
#[cfg(target_os = "linux")]
use bunny_ui_linux::FreeTypeEngine;
#[cfg(target_os = "linux")]
use std::rc::Rc;

/// The name this process holds — one per user, whoever launches first.
const NAME: &str = "bunny-life-example";

#[derive(Clone)]
struct Life {
    log: State<Vec<String>>,
    answer: State<String>,
    life: Rc<task::Receiver<AppEvent>>,
    drive: bool,
}

impl Component for Life {
    fn body(self, _ctx: &Context) -> impl View {
        let (log, answer) = (self.log, self.answer);
        let chip = |label: &str| {
            text(label)
                .padding_length(8.0)
                .background_color(theme::control())
                .background_hovered(theme::control_hovered())
                .corner_radius(6.0)
        };
        let notify = chip("notify").on_click(move || {
            answer.set(match bunny_ui::app::notify(&letter()) {
                Ok(()) => String::from("posted"),
                Err(why) => format!("refused: {why}"),
            });
        });
        let again = chip("a second launch").on_click(|| second_launch("by hand"));
        let lines: Vec<_> =
            log.get().iter().rev().take(14).map(|line| text(line.clone())).collect();
        let events = self.life;
        vstack!(
            hstack!(notify, again, text(answer.get()).foreground_color(theme::fg_secondary()))
                .spacing(8.0)
                .alignment(VerticalAlignment::Center),
            text("what the platform said, newest first").foreground_color(theme::fg_secondary()),
            vstack(lines).spacing(4.0).alignment(HorizontalAlignment::Leading)
        )
        .spacing(10.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(16.0)
        // the app's life, read on the app's own thread — a send from
        // the platform wakes this task like any channel
        .task({
            let drive = self.drive;
            move || {
                let events = Rc::clone(&events);
                async move {
                    if drive {
                        drive_the_hand(log, answer);
                    }
                    while let Some(event) = events.recv().await {
                        let line = format!("{:?}", event);
                        println!("[{}] {line}", stamp());
                        log.update(|lines| lines.push(line));
                    }
                }
            }
        })
    }
}

/// A letter arrived — the notification a mail client would post.
fn letter() -> Notification {
    Notification::new("thread-7", "Ada", "Could you send the figures?")
        .action("open", "Open")
        .action("archive", "Archive")
}

/// This very binary, launched again with arguments: it forwards them
/// to the process holding the name and exits.
fn second_launch(word: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--again").arg(word).spawn();
    }
}

/// The hand: a second launch must reach this process as a reopen with
/// its arguments; a notification must answer — `Ok` inside a bundle,
/// the refusal by name in a bare binary. Each line prints as it is
/// measured, and the process answers 0 or 1.
fn drive_the_hand(log: State<Vec<String>>, answer: State<String>) {
    task::spawn(async move {
        let mut passed = true;
        let mut check = |name: &str, held: bool| {
            println!("[{}] {} — {name}", stamp(), if held { "ok" } else { "FAILED" });
            passed &= held;
        };
        task::sleep(std::time::Duration::from_millis(800)).await;
        second_launch("hello");
        let mut heard = false;
        for _ in 0..30 {
            task::sleep(std::time::Duration::from_millis(200)).await;
            let expected = format!("{:?}", AppEvent::Reopened {
                arguments: vec![String::from("--again"), String::from("hello")],
            });
            if log.get().iter().any(|line| *line == expected) {
                heard = true;
                break;
            }
        }
        check("a second launch reaches this process as a reopen with its arguments", heard);

        // the desktop's notification daemon answers over the session
        // bus; a box without one (the container) refuses by name
        let posted = bunny_ui::app::notify(&letter());
        println!("[{}] notify answered {posted:?}", stamp());
        answer.set(format!("{posted:?}"));
        check(
            "the notification answers by name — posted through the bus, or refused with a reason",
            posted.is_ok() || posted.as_ref().is_err_and(|why| !why.is_empty()),
        );
        println!(
            "[{}] {}",
            stamp(),
            if passed { "the sheet holds" } else { "the sheet has a hole" }
        );
        std::process::exit(if passed { 0 } else { 1 });
    })
    .detach();
}

/// Milliseconds since the app opened.
fn stamp() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    format!("{:>6}ms", START.get_or_init(Instant::now).elapsed().as_millis())
}

#[cfg(target_os = "linux")]
fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // the app's life is subscribed BEFORE the name is claimed, so a
    // launch that lands early is heard
    let (sender, life) = task::channel::<AppEvent>();
    bunny_ui::app::subscribe(sender);
    if bunny_ui::app::instance(NAME, &arguments) == Instance::Secondary {
        println!("[{}] forwarded {arguments:?} to the running one", stamp());
        return;
    }
    println!("[{}] holding the name {NAME}", stamp());
    let runtime = Runtime::new().text_engine(Rc::new(FreeTypeEngine::new()));
    bunny_ui_linux::run_window_with(
        "a life",
        Size { width: 640.0, height: 420.0 },
        runtime,
        Life {
            log: State::new(Vec::new()),
            answer: State::new(String::from("nothing yet")),
            life: Rc::new(life),
            drive: arguments.iter().any(|arg| arg == "--drive"),
        },
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
