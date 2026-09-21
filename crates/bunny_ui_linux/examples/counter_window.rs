//! The counter in a real window — the first element on the screen.
//!
//! ```sh
//! cargo run -p bunny-ui-linux --example counter_window_linux
//! cargo run -p bunny-ui-linux --example counter_window_linux -- --drive
//! ```
//!
//! With `--drive` the window drives itself: a click lands on the
//! button, the count must read one, and the shell must have presented
//! a frame for it — under whichever door (`BUNNY_BACKEND`) and
//! present tier (`BUNNY_PRESENT`) the run chose. Exit 0 is the proof.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
    drive: bool,
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
        .task(move || async move {
            if self.drive {
                // the window is up and the pump is turning
                task::sleep(std::time::Duration::from_millis(800)).await;
                the_sheet(self.count).await;
            }
        })
    }
}

/// The hand: one click on the button, at the bottom-left where the
/// stack puts it (16 pt of padding, a leading column), then the two
/// witnesses — the state and the glass.
#[cfg(target_os = "linux")]
async fn the_sheet(count: State<i32>) {
    use bunny_ui_linux::drive;
    let start = std::time::Instant::now();
    let stamp = move || format!("{:>6}ms", start.elapsed().as_millis());
    let mut passed = true;
    let mut check = |name: &str, held: bool| {
        println!("[{}] {} — {name}", stamp(), if held { "ok" } else { "FAILED" });
        passed &= held;
    };
    let before = drive::presents();
    check("the first frame reached the glass", before >= 1);
    drive::click(40.0, 152.0);
    task::sleep(std::time::Duration::from_millis(300)).await;
    check("the click counted", count.wrappedValue() == 1);
    let after = drive::presents();
    check("and the count was presented", after > before);
    println!(
        "[{}] {} — backend={} presents={after}",
        stamp(),
        if passed { "the sheet holds" } else { "the sheet has a hole" },
        drive::backend()
    );
    std::process::exit(if passed { 0 } else { 1 });
}

#[cfg(not(target_os = "linux"))]
async fn the_sheet(_count: State<i32>) {}

#[cfg(target_os = "linux")]
fn main() {
    let drive = std::env::args().any(|arg| arg == "--drive");
    if drive {
        bunny_ui_linux::drive::watchdog(20);
    }
    let counter = Counter { count: State::new(0), drive };
    bunny_ui_linux::run_window(
        "bunny_ui",
        Size { width: 280.0, height: 180.0 },
        counter,
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
