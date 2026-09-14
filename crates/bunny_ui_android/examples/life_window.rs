//! The app's life outside its window, on the phone — `bunny_ui::app`.
//!
//! A mail client says "a letter arrived" in the phone's own
//! notification with a button on it, and hears which button was
//! pressed. This screen shows every such event as it lands, and
//! offers the one ask: a notification.
//!
//! ```sh
//! crates/bunny_ui_android/android/run-emu.sh life_window_android
//! adb shell setprop debug.bunny.notify 1   # the ask, 800 ms after the mount
//! ```
//!
//! What to check by hand:
//! - "notify": the first post asks the person (API 33+) and says so
//!   by name; allowed, the next post lands as a banner and in the
//!   shade — `adb shell dumpsys notification --noredact` lists it
//!   under this app with two actions;
//! - a tap on it: the activity is re-created (the shell's trace shows
//!   `create` again, and the last one's callbacks as `stale`), and the
//!   log shows `NotificationActivated` with the id and `None`; the
//!   "Open" button lands as `Some("open")`, and the notification
//!   leaves the shade;
//! - the notification that LAUNCHES the app: notify, home, `adb shell
//!   am kill com.bunnylab.life_window_android`, tap — the first line
//!   of the log is the activation;
//! - home and back: `WillSleep`, `DidWake`.

#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports, unused_variables))]

use std::sync::OnceLock;
use std::time::Instant;

use bunny_ui::app::{AppEvent, Notification};
use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

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
        let notify = text("notify")
            .padding_length(12.0)
            .background_color(theme::control())
            .corner_radius(8.0)
            .on_click(move || {
                answer.set(match bunny_ui::app::notify(&letter()) {
                    Ok(()) => String::from("posted"),
                    Err(why) => format!("refused: {why}"),
                });
            });
        let lines: Vec<_> =
            log.get().iter().rev().take(14).map(|line| text(line.clone())).collect();
        let events = self.life;
        vstack!(
            hstack!(notify, text(answer.get()).foreground_color(theme::fg_secondary()))
                .spacing(12.0)
                .alignment(VerticalAlignment::Center),
            text("what the phone said, newest first").foreground_color(theme::fg_secondary()),
            vstack(lines).spacing(4.0).alignment(HorizontalAlignment::Leading)
        )
        .spacing(12.0)
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
                        drive_the_hand(answer);
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

/// The hand: the notification is asked for and its answer printed.
/// The tap on it is the person's — the activity stays up for it.
fn drive_the_hand(answer: State<String>) {
    task::spawn(async move {
        task::sleep(std::time::Duration::from_millis(800)).await;
        let posted = bunny_ui::app::notify(&letter());
        println!("[{}] notify answered {posted:?}", stamp());
        answer.set(match posted {
            Ok(()) => String::from("posted"),
            Err(why) => format!("refused: {why}"),
        });
    })
    .detach();
}

/// Milliseconds since the app opened.
fn stamp() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    format!("{:>6}ms", START.get_or_init(Instant::now).elapsed().as_millis())
}

#[cfg(target_os = "android")]
fn main() {
    // the app's life is subscribed BEFORE the window: a tap on a
    // notification that created this activity lands as it opens
    let (sender, life) = task::channel::<AppEvent>();
    bunny_ui::app::subscribe(sender);
    let life = Life {
        log: State::new(Vec::new()),
        answer: State::new(String::from("nothing yet")),
        life: Rc::new(life),
        drive: bunny_ui_android::property("debug.bunny.notify").is_some(),
    };
    bunny_ui_android::run_window("a life", Size { width: 360.0, height: 800.0 }, life);
}

#[cfg(not(target_os = "android"))]
fn main() {} // this example is Android-only

#[cfg(target_os = "android")]
bunny_ui_android::activity!(main);
