//! The frame ruler: a dashboard-shaped scene, one WHOLE frame per sample.
//!
//! ```sh
//! cargo run --release -p bunny-ui --example bench_frame
//! ```
//!
//! `bench_pipeline` times `layout` on a small finder. This harness times
//! what a shell calls for every event — `display_frame`: settle, layout,
//! the follow-up rounds and the pointer re-read — on a scene with the
//! shape of a product screen (see `support/dashboard.rs`), in a NAMED
//! scene, because that is what every window of a shell is.
//!
//! `-- --soak wheel` (or `swap`) holds ONE scenario for ten seconds and
//! prints nothing but its rate: a loop a profiler can take a sample of.
//!
//! It prints wall time and allocations for each scenario, then the stage
//! table from [`bunny_ui::stats`] in a separate pass, so the timers never
//! pay into the wall numbers. Text metrics come from the `PixelFont`:
//! the allocation counts are the same on every machine.

use bunny_ui::action::Modifiers;
use bunny_ui::prelude::*;

#[path = "support/dashboard.rs"]
mod dashboard;
#[path = "support/harness.rs"]
mod harness;

use dashboard::{Mode, VIEWPORT, Workbench};
use harness::{CountingAllocator, measure, print_reports, print_stages, stages};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const WARMUP: usize = 20;
const FRAMES: usize = 300;
const SWAPS: usize = 60;

/// A point over the first panel's painted box: nothing there paints a
/// hover.
const OVER_CHART: (f64, f64) = (700.0, 200.0);
/// A point over the sidebar's rows: every row paints a hover.
const OVER_ROWS: (f64, f64) = (100.0, 300.0);

/// The wheel of one frame: 30 steps one way, 30 steps back, so the
/// content really travels under a pointer at rest and never sits on a
/// clamp for long.
fn travel(turn: usize) -> f64 {
    if (turn / 30) % 2 == 0 { -8.0 } else { 8.0 }
}

struct Bench {
    root: Workbench,
    runtime: Runtime,
}

impl Bench {
    fn new(runtime: Runtime, legend_rows: usize) -> Bench {
        Bench::with(Workbench::new(legend_rows), runtime)
    }

    fn with(root: Workbench, runtime: Runtime) -> Bench {
        // as a shell mounts it: what no pixel can show is not drawn
        runtime.drop_unseen();
        let bench = Bench { root, runtime };
        // the mount, and the second pass the legends' probes ask for
        bench.frame();
        bench.frame();
        bench
    }

    fn frame(&self) -> usize {
        self.runtime.display_frame(&self.root, VIEWPORT).len()
    }
}

fn scenarios(tag: &str, scene: bool, legend_rows: usize, full: bool) -> (Vec<harness::Report>, Vec<harness::StageRow>) {
    let runtime = || if scene { Runtime::scene("w0") } else { Runtime::new() };
    let mut reports = Vec::new();
    let mut rows = Vec::new();
    let label = |name: &str| format!("{tag} {name}");

    // 1. REST: nothing changed — the frame a wake with no news asks for
    {
        let bench = Bench::new(runtime(), legend_rows);
        reports.push(measure(label("rest"), WARMUP, FRAMES, || (), || bench.frame()));
        rows.push(stages(label("rest"), FRAMES, || (), || bench.frame()));
    }

    // 2. WHEEL over the board, with no pointer in the scene
    {
        let bench = Bench::new(runtime(), legend_rows);
        let mut turn = 0usize;
        let mut step = || {
            turn += 1;
            bench.runtime.wheel(OVER_CHART.0, OVER_CHART.1, 0.0, travel(turn));
            bench.frame()
        };
        reports.push(measure(label("wheel, no pointer"), WARMUP, FRAMES, || (), &mut step));
        rows.push(stages(label("wheel, no pointer"), FRAMES, || (), &mut step));
    }

    // 2b. the same wheel, with every chart keeping its picture (`.cached`)
    {
        let bench = Bench::with(Workbench::new(legend_rows).keeping_pictures(), runtime());
        let mut turn = 0usize;
        let mut step = || {
            turn += 1;
            bench.runtime.wheel(OVER_CHART.0, OVER_CHART.1, 0.0, travel(turn));
            bench.frame()
        };
        reports.push(measure(label("wheel, charts keep pictures"), WARMUP, FRAMES, || (), &mut step));
        rows.push(stages(label("wheel, charts keep pictures"), FRAMES, || (), &mut step));
    }

    // 3. WHEEL with the pointer at rest: the frame re-reads the hover
    for (name, at, wheel_at) in [
        ("wheel, pointer on a chart", OVER_CHART, OVER_CHART),
        ("wheel, pointer on hover rows", OVER_ROWS, OVER_ROWS),
    ] {
        let bench = Bench::new(runtime(), legend_rows);
        bench.runtime.pointer_moved(at.0, at.1, Modifiers::NONE);
        bench.frame();
        let mut turn = 0usize;
        let mut step = || {
            turn += 1;
            bench.runtime.wheel(wheel_at.0, wheel_at.1, 0.0, travel(turn));
            bench.frame()
        };
        reports.push(measure(label(name), WARMUP, FRAMES, || (), &mut step));
        rows.push(stages(label(name), FRAMES, || (), &mut step));
    }

    if !full {
        return (reports, rows);
    }

    // 4. MODE SWAP: one large subtree leaves, another mounts
    for (name, to, back) in [
        ("swap board → table", Mode::Table, Mode::Board),
        ("swap table → board", Mode::Board, Mode::Table),
    ] {
        let bench = Bench::new(runtime(), legend_rows);
        let prepare = || {
            bench.root.mode.set(back);
            bench.frame();
            bench.frame();
        };
        let step = || {
            bench.root.mode.set(to);
            bench.frame()
        };
        reports.push(measure(label(name), 3, SWAPS, prepare, step));
        rows.push(stages(label(name), SWAPS, prepare, step));
    }

    // 5. DEEP STATE: one small body far from the root
    {
        let bench = Bench::new(runtime(), legend_rows);
        let flag = bench.root.flags[3];
        let mut step = || {
            flag.set(!flag.get());
            bench.frame()
        };
        reports.push(measure(label("deep state (1 body)"), WARMUP, FRAMES, || (), &mut step));
        rows.push(stages(label("deep state (1 body)"), FRAMES, || (), &mut step));
    }

    // 6. TICK: a colour in flight, no body runs
    {
        let bench = Bench::new(runtime(), legend_rows);
        let mut ticks = 0usize;
        let mut step = || {
            if ticks % 24 == 0 {
                bench.root.pulse.set(ticks / 24 + 1);
                bench.frame();
            }
            ticks += 1;
            bench.runtime.tick(1.0 / 120.0);
            bench.runtime.animation_frame(&bench.root, VIEWPORT).len()
        };
        reports.push(measure(label("tick (a flight, no body)"), WARMUP, FRAMES, || (), &mut step));
        rows.push(stages(label("tick (a flight, no body)"), FRAMES, || (), &mut step));
    }

    (reports, rows)
}

/// One scenario, held for ten seconds: a loop a profiler can sample.
fn soak(which: &str) {
    let held = std::time::Duration::from_secs(10);
    let started = std::time::Instant::now();
    let mut frames = 0u64;
    match which {
        "wheel" => {
            let bench = Bench::new(Runtime::scene("w0"), 240);
            let mut turn = 0usize;
            while started.elapsed() < held {
                turn += 1;
                bench.runtime.wheel(OVER_CHART.0, OVER_CHART.1, 0.0, travel(turn));
                frames += bench.frame() as u64 & 1 | 1;
            }
        }
        "swap" => {
            let bench = Bench::new(Runtime::scene("w0"), 60);
            while started.elapsed() < held {
                bench.root.mode.set(Mode::Table);
                bench.frame();
                bench.root.mode.set(Mode::Board);
                bench.frame();
                bench.frame();
                frames += 3;
            }
        }
        other => {
            eprintln!("--soak takes `wheel` or `swap`, not `{other}`");
            std::process::exit(2);
        }
    }
    println!("{which}: {frames} frames in {:.1} s", started.elapsed().as_secs_f64());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(at) = args.iter().position(|arg| arg == "--soak") {
        soak(args.get(at + 1).map_or("wheel", String::as_str));
        return;
    }
    let (reports, rows) = scenarios("scene", true, 60, true);
    print_reports("named scene (what a shell runs), 8 panels × 60 legend rows", &reports);
    print_stages("stages", &rows);

    let (reports, rows) = scenarios("scene", true, 240, false);
    print_reports("named scene, 8 panels × 240 legend rows — does the frame grow with what is off screen?", &reports);
    print_stages("stages", &rows);

    let (reports, rows) = scenarios("plain", false, 60, false);
    print_reports("control: the same scene under `Runtime::new()`", &reports);
    print_stages("stages", &rows);

    println!(
        "\nfixture: viewport {}×{}, PixelFont (deterministic); every sample is one `display_frame`",
        VIEWPORT.width, VIEWPORT.height
    );
    println!("the stage pass runs separately with the stats clock on — wall numbers stay clean");
}
