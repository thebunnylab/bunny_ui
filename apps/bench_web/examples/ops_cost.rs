//! What the RUST side costs per official operation — the counters the
//! browser cannot see. The harness times the whole path (script plus
//! paint); this says how much work the render and the diff really did
//! for a change the app made, and how many allocations it took.
//!
//! Every operation runs on a FRESH runtime, prepared the way the harness
//! prepares it (a thousand rows mounted first), several rounds over; the
//! table shows the median of each stage and the counters of the last
//! round, so one slow round of the machine never reads as a regression.
//!
//! ```sh
//! cargo run --release -p bench-web --example ops_cost
//! cargo run --release -p bench-web --example ops_cost -- --rounds 9
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;
use bunny_ui::stats;

use bench_web::keyed::{App, RowSeed};

const SIZE: Size = Size { width: 1200.0, height: 800.0 };

// MARK: - The counting allocator

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
/// Bytes allocated and not yet freed — what a leak shows up in.
static LIVE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocations() -> (usize, usize) {
    (ALLOCS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
}

fn live_kib() -> isize {
    LIVE.load(Ordering::Relaxed) / 1024
}

/// `--cycles N`: one runtime, N rounds of create 1k → clear → collect,
/// with the live bytes and the engine's retained counts after each step
/// — the probe for a leak, where the official memory benchmark only
/// shows the total.
fn cycles(rounds: usize) {
    let (runtime, app) = fresh();
    println!("cycle step             live KiB  retained");
    let report = |step: &str| println!("{:<22} {:>8}  {}", step, live_kib(), runtime.retained_counts());
    report("ready");
    for round in 1..=rounds {
        create(&app, &runtime, 1_000);
        report(&format!("{round}: after create"));
        app.rows.set(Rc::new(Vec::new()));
        let _ = runtime.dom_frame(&app, SIZE);
        report(&format!("{round}: after clear"));
        runtime.collect_garbage();
        report(&format!("{round}: after collect"));
    }
}

// MARK: - The scene

fn seeds(from: usize, count: usize) -> Vec<RowSeed> {
    (0..count)
        .map(|i| RowSeed {
            id: from + i,
            label: State::new(Rc::from(format!("row {}", from + i).as_str())),
            selected: State::new(false),
        })
        .collect()
}

fn fresh() -> (Runtime, App) {
    let runtime = Runtime::new();
    let app = App {
        rows: State::new(Rc::new(Vec::new())),
        selected: State::new(None),
        next_id: State::new(1),
    };
    // the empty page, mounted: what the harness's page looks like
    // before its first click
    let _ = runtime.dom_frame(&app, SIZE);
    (runtime, app)
}

fn create(app: &App, runtime: &Runtime, count: usize) {
    let from = app.next_id.get();
    app.rows.set(Rc::new(seeds(from, count)));
    app.next_id.set(from + count);
    let _ = runtime.dom_frame(app, SIZE);
}

/// One official operation: how the page is prepared (untimed), and the
/// one change that is measured.
struct Op {
    name: &'static str,
    prep: fn(&App, &Runtime),
    run: fn(&App),
}

const OPS: &[Op] = &[
    Op { name: "create 1k", prep: |_, _| {}, run: |app| app.rows.set(Rc::new(seeds(1, 1_000))) },
    Op {
        name: "replace 1k",
        prep: |app, runtime| create(app, runtime, 1_000),
        run: |app| {
            let from = app.next_id.get();
            app.rows.set(Rc::new(seeds(from, 1_000)));
        },
    },
    Op {
        name: "update 10th",
        prep: |app, runtime| create(app, runtime, 1_000),
        run: |app| {
            for row in app.rows.get().iter().step_by(10) {
                row.label.set(Rc::from(format!("{} !!!", row.label.get()).as_str()));
            }
        },
    },
    Op {
        name: "select",
        prep: |app, runtime| create(app, runtime, 1_000),
        run: |app| app.rows.get()[1].selected.set(true),
    },
    Op {
        name: "swap",
        prep: |app, runtime| create(app, runtime, 1_000),
        run: |app| {
            let mut rows = (*app.rows.get()).clone();
            rows.swap(1, 998);
            app.rows.set(Rc::new(rows));
        },
    },
    Op {
        name: "remove 1",
        prep: |app, runtime| create(app, runtime, 1_000),
        run: |app| {
            let mut rows = (*app.rows.get()).clone();
            rows.remove(1);
            app.rows.set(Rc::new(rows));
        },
    },
    Op { name: "create 10k", prep: |_, _| {}, run: |app| app.rows.set(Rc::new(seeds(1, 10_000))) },
    Op {
        name: "append 1k",
        prep: |app, runtime| create(app, runtime, 1_000),
        run: |app| {
            let from = app.next_id.get();
            let mut rows = (*app.rows.get()).clone();
            rows.extend(seeds(from, 1_000));
            app.rows.set(Rc::new(rows));
        },
    },
    Op {
        name: "clear",
        prep: |app, runtime| create(app, runtime, 1_000),
        run: |app| app.rows.set(Rc::new(Vec::new())),
    },
];

// MARK: - The measurement

struct Sample {
    stats: stats::FrameStats,
    bodies: usize,
    patches: usize,
    allocs: usize,
    kib: usize,
    total_ms: f64,
}

fn measure(op: &Op) -> Sample {
    let (runtime, app) = fresh();
    (op.prep)(&app, &runtime);
    let _ = stats::take();
    let (allocs_before, bytes_before) = allocations();
    let started = now_ms();
    (op.run)(&app);
    let patches = runtime.dom_frame(&app, SIZE);
    let total_ms = now_ms() - started;
    let (allocs_after, bytes_after) = allocations();
    Sample {
        stats: stats::take(),
        bodies: runtime.body_runs().len(),
        patches: patches.len(),
        allocs: allocs_after - allocs_before,
        kib: (bytes_after - bytes_before) / 1024,
        total_ms,
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    values[values.len() / 2]
}

/// A clock for the stage timers — the engine takes a function, so the
/// host decides what "now" means (the browser hands it `performance.now`).
fn now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs_f64() * 1000.0
}

fn main() {
    let rounds: usize = std::env::args()
        .skip_while(|arg| arg != "--rounds")
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(5);
    stats::set_clock(Some(now_ms));
    if let Some(rounds) = std::env::args()
        .skip_while(|arg| arg != "--cycles")
        .nth(1)
        .and_then(|value| value.parse().ok())
    {
        cycles(rounds);
        return;
    }

    println!(
        "{:<12} {:>6} {:>6} {:>7} {:>6} {:>7} {:>7} {:>7} | {:>7} {:>7} {:>7} {:>7} {:>8}",
        "op (median)", "bodies", "built", "visited", "reused", "patches", "allocs", "KiB", "settle", "build",
        "diff", "encode", "total ms"
    );
    for op in OPS {
        let samples: Vec<Sample> = (0..rounds).map(|_| measure(op)).collect();
        let stage = |stage: stats::Stage| {
            let mut values: Vec<f64> = samples.iter().map(|sample| sample.stats.ms(stage)).collect();
            median(&mut values)
        };
        let mut totals: Vec<f64> = samples.iter().map(|sample| sample.total_ms).collect();
        let last = samples.last().expect("at least one round");
        println!(
            "{:<12} {:>6} {:>6} {:>7} {:>6} {:>7} {:>7} {:>7} | {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>8.3}",
            op.name,
            last.bodies,
            last.stats.capture_nodes,
            last.stats.diff_visited,
            last.stats.diff_reused,
            last.patches,
            last.allocs,
            last.kib,
            stage(stats::Stage::Settle),
            stage(stats::Stage::Capture),
            stage(stats::Stage::Diff),
            stage(stats::Stage::Encode),
            median(&mut totals),
        );
    }
    println!(
        "{rounds} rounds per op, each on a fresh runtime; stages are medians, counters are the last round's. \
         `encode` is 0 here: the wire bytes are the shell's business."
    );
}
