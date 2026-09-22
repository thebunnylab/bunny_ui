//! A dashboard that scrolls — and, with `--drive`, scrolls ITSELF, so the
//! shell's frame pacing can be read off the present tape.
//!
//! ```sh
//! cargo run --release -p bunny-ui-linux --example scroll_window_linux
//! BUNNY_PRESENT_TRACE=/tmp/scroll.{pid}.trace \
//!     cargo run --release -p bunny-ui-linux --example scroll_window_linux -- --drive
//! cargo run --release -p bunny-ui-macos --example trace_report -- /tmp/scroll.<pid>.trace
//! ```
//!
//! The scene is the frame benches' own (`bunny_ui/examples/support`): a
//! tab strip, a sidebar, and a plain `scroll` of chart panels, each with a
//! legend that scrolls.
//!
//! `--drive` is a wheel the window system did not send. A worker thread
//! keeps a wheel's pace with a real clock — 240 steps a second, faster
//! than any display, which is the point — and hands each step to a task on
//! the main thread, which raises it through the shell's own door
//! ([`bunny_ui_linux::drive`]). The script is two seconds of rest, three of
//! wheel, a second of rest, then the process ends. On the tape, the wheel
//! must read as ONE present for each display beat, however many steps
//! arrived between two beats; the rests must read as no present at all.
//!
//! `--swap` is the other script: the page changes between the board and
//! the table, eight times, with a rest between two changes. Each change
//! mounts a page the window has not drawn yet — or has, the second time —
//! so the tape reads what a change of page costs with a real face: the
//! measures of text never seen, and its first raster.
//!
//! `--kept` makes every chart keep its picture (`.cached`), under either
//! script: the same tape, with the paints of a still chart gone from it.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::time::Duration;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[path = "../../bunny_ui/examples/support/dashboard.rs"]
mod dashboard;

use dashboard::{VIEWPORT, Workbench};

/// Wheel steps a second while driving — faster than any display.
const STEPS_PER_SECOND: u64 = 240;
const REST_BEFORE: Duration = Duration::from_secs(2);
const WHEEL_FOR: Duration = Duration::from_secs(3);
const REST_AFTER: Duration = Duration::from_secs(1);
/// Page changes in the `--swap` script, and the rest between two of them.
const SWAPS: usize = 8;
const REST_BETWEEN: Duration = Duration::from_millis(600);
/// A point over the board's first chart, in layout points. The board hugs
/// its panels, and with a real face they are narrower than the window: the
/// point stays near the board's leading edge.
const OVER_BOARD: (f64, f64) = (400.0, 140.0);

/// One step of the script, from the worker's clock to the main thread.
enum Step {
    Wheel(f64),
    Swap,
    Done,
}

/// Which script the window runs, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    None,
    Wheel,
    Swap,
}

#[derive(Clone)]
struct Driven {
    scene: Workbench,
    script: Script,
}

impl Component for Driven {
    fn body(self, _ctx: &Context) -> impl View {
        let script = self.script;
        let mode = self.scene.mode;
        self.scene.task(move || async move {
            if script == Script::None {
                return;
            }
            let (sender, receiver) = task::channel::<Step>();
            // the worker keeps REAL time: the engine's own clock is the
            // frame tick, and a script that slept on it would pace itself
            // by the very thing it measures
            std::thread::spawn(move || {
                std::thread::sleep(REST_BEFORE);
                if script == Script::Swap {
                    for _ in 0..SWAPS {
                        if sender.send(Step::Swap).is_err() {
                            return;
                        }
                        std::thread::sleep(REST_BETWEEN);
                    }
                    let _ = sender.send(Step::Done);
                    return;
                }
                let steps = WHEEL_FOR.as_millis() as u64 * STEPS_PER_SECOND / 1000;
                let pause = Duration::from_micros(1_000_000 / STEPS_PER_SECOND);
                for step in 0..steps {
                    // down for a second and a half, then back up
                    let delta = if step < steps / 2 { -6.0 } else { 6.0 };
                    if sender.send(Step::Wheel(delta)).is_err() {
                        return;
                    }
                    std::thread::sleep(pause);
                }
                std::thread::sleep(REST_AFTER);
                let _ = sender.send(Step::Done);
            });
            while let Some(step) = receiver.recv().await {
                match step {
                    #[cfg(target_os = "linux")]
                    Step::Wheel(delta) => {
                        bunny_ui_linux::drive::wheel(OVER_BOARD.0, OVER_BOARD.1, 0.0, delta);
                    }
                    #[cfg(not(target_os = "linux"))]
                    Step::Wheel(_) => {}
                    Step::Swap => mode.update(|mode| {
                        *mode = if *mode == dashboard::Mode::Board {
                            dashboard::Mode::Table
                        } else {
                            dashboard::Mode::Board
                        };
                    }),
                    Step::Done => std::process::exit(0),
                }
            }
        })
    }
}

#[cfg(target_os = "linux")]
fn main() {
    let script = if std::env::args().any(|arg| arg == "--drive") {
        Script::Wheel
    } else if std::env::args().any(|arg| arg == "--swap") {
        Script::Swap
    } else {
        Script::None
    };
    let scene = if std::env::args().any(|arg| arg == "--kept") {
        Workbench::new(60).keeping_pictures()
    } else {
        Workbench::new(60)
    };
    bunny_ui_linux::run_window(
        "bunny_ui — a dashboard that scrolls",
        Size { width: VIEWPORT.width, height: VIEWPORT.height },
        Driven { scene, script },
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
