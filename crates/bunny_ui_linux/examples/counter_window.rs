//! The counter in a real window — the first element on the screen.
//!
//! ```sh
//! cargo run -p bunny-ui-linux --example counter_window_linux
//! cargo run -p bunny-ui-linux --example counter_window_linux -- --drive
//! cargo run -p bunny-ui-linux --example counter_window_linux -- --drive --fixed
//! ```
//!
//! With `--drive` the window drives itself: a click lands on the
//! button, the count must read one, and the shell must have presented
//! a frame for it — under whichever door (`BUNNY_BACKEND`) and
//! present tier (`BUNNY_PRESENT`) the run chose. With `--fixed` the
//! window opens through `App` as one that cannot resize or be put
//! away, and the sheet reads the manners back: the hints on x11, the
//! frame's owner on wayland — and where the house bar stands in, its
//! close button ends the run. With `--sleeper` a task wakes twenty
//! times a second and writes once a second for three seconds: the
//! shell must present the writes and nothing for the wakes. Exit 0 is
//! the proof.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
    /// What the sleeper wrote so far — a line that changes once a second.
    ticks: State<i32>,
    drive: bool,
    fixed: bool,
    sleeper: bool,
}

impl Component for Counter {
    fn body(self, _ctx: &Context) -> impl View {
        vstack!(
            text!("Count: {}", self.count).font(Font::Title),
            text!("Ticks: {}", self.ticks),
            spacer(),
            button(text("Tap me!"), move || self.count.add(1)),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
        .task(move || async move {
            if self.drive {
                // the window is up and the pump is turning
                task::sleep(std::time::Duration::from_millis(800)).await;
                the_sheet(self.count, self.fixed, self.sleeper).await;
            }
        })
        .task(move || async move {
            if self.sleeper {
                // a poller: awake twenty times a second, news once a
                // second — the shell must draw the news and nothing else
                task::sleep(std::time::Duration::from_millis(1200)).await;
                for wake in 0..60 {
                    task::sleep(std::time::Duration::from_millis(50)).await;
                    if wake % 20 == 19 {
                        self.ticks.add(1);
                    }
                }
            }
        })
    }
}

const WIDTH: f64 = 280.0;
const HEIGHT: f64 = 180.0;

/// The hand: one click on the button, at the bottom-left where the
/// stack puts it (16 pt of padding, a leading column, measured from
/// the size the window was granted), then the two
/// witnesses — the state and the glass. With `fixed`, the manners.
#[cfg(target_os = "linux")]
async fn the_sheet(count: State<i32>, fixed: bool, sleeper: bool) {
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
    // aimed from the granted size: a tiling desktop answers 280×180
    // with a tile of its own
    let (width, height) = drive::window_size();
    drive::click(40.0, height - 28.0);
    task::sleep(std::time::Duration::from_millis(300)).await;
    check("the click counted", count.wrappedValue() == 1);
    let after = drive::presents();
    check("and the count was presented", after > before);
    if sleeper {
        // the poller starts at 1200 ms from mount; watch it for its
        // three writes and a little slack
        let before = drive::presents();
        task::sleep(std::time::Duration::from_millis(3600)).await;
        let grew = drive::presents() - before;
        println!("[{}] sleeper: presents grew by {grew} over sixty wakes and three writes", stamp());
        check("the three writes reached the glass", grew >= 3);
        check("and the wakes with no news drew nothing", grew <= 8);
    }
    let decoration = drive::decoration();
    println!("[{}] frame: backend={} decoration={decoration}", stamp(), drive::backend());
    if fixed {
        if drive::backend() == "x11" {
            // WM_NORMAL_HINTS: min = max = the size, in physical pixels
            let hints = drive::x11_property("WM_NORMAL_HINTS");
            check("WM_NORMAL_HINTS carries PMinSize and PMaxSize", hints.first().is_some_and(|f| f & 0x30 == 0x30));
            let one_size = hints.len() >= 9 && hints[5] == hints[7] && hints[6] == hints[8] && hints[5] > 0;
            check("and min is max: one size", one_size);
            // _MOTIF_WM_HINTS: ALL, with resize, maximize and minimize removed
            let motif = drive::x11_property("_MOTIF_WM_HINTS");
            check("the Motif hints speak of functions", motif.first().is_some_and(|f| f & 1 != 0));
            check(
                "and drop resize, maximize and minimize",
                motif.get(1).is_some_and(|f| *f == (1 | 2 | 8 | 16)),
            );
        } else {
            check("the wayland door reports who owns the frame", decoration != "");
        }
    }
    let house_bar = drive::backend() == "wayland" && decoration != "server";
    if house_bar {
        // the bar stands in for the compositor's: its close button is
        // the window's own, and closing the last window ends the run —
        // main prints the verdict after `run` returns
        println!("[{}] the house bar stands — closing through its button", stamp());
        drive::click(width - 20.0, 16.0);
        task::sleep(std::time::Duration::from_millis(1500)).await;
        check("the close button closed the window", false);
    }
    println!(
        "[{}] {} — backend={} presents={after}",
        stamp(),
        if passed { "the sheet holds" } else { "the sheet has a hole" },
        drive::backend()
    );
    std::process::exit(if passed { 0 } else { 1 });
}

#[cfg(not(target_os = "linux"))]
async fn the_sheet(_count: State<i32>, _fixed: bool, _sleeper: bool) {}

#[cfg(target_os = "linux")]
fn main() {
    let drive = std::env::args().any(|arg| arg == "--drive");
    let fixed = std::env::args().any(|arg| arg == "--fixed");
    let sleeper = std::env::args().any(|arg| arg == "--sleeper");
    if drive {
        bunny_ui_linux::drive::watchdog(25);
    }
    let counter = Counter { count: State::new(0), ticks: State::new(0), drive, fixed, sleeper };
    if fixed {
        // the App road: a spec carries the manners
        let app = bunny_ui_linux::App::new();
        let runtime = app
            .runtime()
            .text_engine(std::rc::Rc::new(bunny_ui_linux::FreeTypeEngine::new()))
            .image_engine(std::rc::Rc::new(bunny_ui_linux::LinuxImageEngine::new()));
        app.open(
            bunny_ui_linux::WindowSpec::titled("bunny_ui")
                .size(WIDTH, HEIGHT)
                .fixed()
                .no_minimize(),
            std::rc::Rc::new(runtime),
            counter,
        );
        app.run();
    } else {
        bunny_ui_linux::run_window("bunny_ui", Size { width: WIDTH, height: HEIGHT }, counter);
    }
    if drive {
        // the pump returned: the last window closed by its own button
        println!("[drive] the window closed — the sheet holds");
        std::process::exit(0);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
