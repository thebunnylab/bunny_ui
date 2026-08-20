//! The Dom-mode ruler: a 200-row stateful table, headless.
//!
//! ```sh
//! cargo run --release -p bunny-ui --example bench_dom
//! ```
//!
//! The element lowering pays a different bill than the pixel path:
//! settle, measure+place with the capture riding it, the diff against
//! the retained scene, the wire encoding. This harness drives the
//! same operations a web benchmark drives — toggle one row, toggle
//! every row, filter the table down and back up, sustained toggles —
//! and prints two tables: wall time percentiles per operation, and
//! the per-stage cost the [`bunny_ui::stats`] seams collected.
//!
//! Text metrics come from the `PixelFont` (deterministic on any
//! machine). The wall-time samples run with no stats clock installed;
//! the stage table is a separate pass with the clock on, so the
//! certified numbers never include the timers' own cost.

use std::alloc::{GlobalAlloc, Layout, System};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use bunny_ui::dom::encode;
use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::stats::{self, Stage};

// MARK: - Counting allocator (zero deps: the wrapped System)

struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocation_snapshot() -> (u64, u64) {
    (ALLOCATIONS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
}

// MARK: - The fixture: 200 rows, a toggle per row, a filter

const ROWS: usize = 200;
const FILTERED: usize = 10;
const CLEAR: Color = Color::rgba(0, 0, 0, 0);

fn name_of(index: usize) -> String {
    format!("service_{index:03}.rs")
}

fn tools_of(index: usize) -> String {
    format!("tools {}", 5000 + index * 7)
}

fn value_of(index: usize) -> String {
    format!("${}.{}M", 90 + index % 20, index % 10)
}

/// One row, one component: the toggle read lives in THIS body, so a
/// flip dirties this row alone — the reuse promise covers the rest.
/// The same shape a signals framework uses for its O(change) story.
#[derive(Clone, Copy)]
struct Row {
    index: usize,
    on: State<bool>,
}

impl Component for Row {
    fn body(self, _ctx: &Context) -> impl View {
        let on = self.on.get();
        let toggle = self.on;
        hstack!(
            text(name_of(self.index)).foreground_color(theme::fg()),
            text(tools_of(self.index))
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            spacer(),
            text(value_of(self.index))
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            rectangle()
                .frame(12.0, 12.0)
                .background_color(if on { theme::accent() } else { theme::border() })
                .corner_radius(3.0),
        )
        .spacing(8.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Leading, 12.0)
        .padding_edge(Edge::Trailing, 12.0)
        .padding_edge(Edge::Top, 5.0)
        .padding_edge(Edge::Bottom, 5.0)
        .background_color(if on { theme::row_pressed() } else { CLEAR })
        .on_click(move || toggle.set(!toggle.get()))
    }
}

#[derive(Clone)]
struct Table {
    filtered: State<bool>,
    toggles: Rc<Vec<State<bool>>>,
}

impl Component for Table {
    fn body(self, _ctx: &Context) -> impl View {
        let count = if self.filtered.get() { FILTERED } else { ROWS };
        let toggles = self.toggles.clone();
        let items: Vec<usize> = (0..count).collect();

        let rows = list(
            items,
            |index| index.to_string(),
            move |index| Row { index: *index, on: toggles[*index] },
        );

        vstack!(text("engine bench").foreground_color(theme::fg()).padding_length(10.0), rows)
            .alignment(HorizontalAlignment::Leading)
            .frame(640.0, 6000.0)
            .background_color(theme::panel())
    }
}

fn fresh() -> (Table, Runtime) {
    let table = Table {
        filtered: State::new(false),
        toggles: Rc::new((0..ROWS).map(|_| State::new(false)).collect()),
    };
    // a newborn runtime opens its own world (see `Runtime::new`), so
    // this table's reads bind to the states that are alive now
    (table, Runtime::new())
}

// MARK: - The clock

fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

struct Report {
    label: &'static str,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    patches: u32,
    allocations: u64,
    kibibytes: u64,
}

/// Runs `frames` iterations measuring wall time + allocations; `step`
/// is ONE frame and returns that frame's patch count.
fn measure(
    label: &'static str,
    warmup: usize,
    frames: usize,
    mut prepare: impl FnMut(),
    mut step: impl FnMut() -> usize,
) -> Report {
    for _ in 0..warmup {
        prepare();
        step();
    }
    let _ = stats::take();
    let mut samples = Vec::with_capacity(frames);
    let mut patches = 0usize;
    let (allocations_before, bytes_before) = allocation_snapshot();
    for _ in 0..frames {
        prepare();
        let start = Instant::now();
        patches = step();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (allocations_after, bytes_after) = allocation_snapshot();
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() as f64 * q) as usize).min(samples.len() - 1)];
    Report {
        label,
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
        max: *samples.last().unwrap(),
        patches: patches as u32,
        allocations: (allocations_after - allocations_before) / frames as u64,
        kibibytes: (bytes_after - bytes_before) / frames as u64 / 1024,
    }
}

struct StageRow {
    label: &'static str,
    stats: stats::FrameStats,
    frames: u32,
}

/// The stage pass: same step, clock on, totals divided per frame.
fn stages(label: &'static str, frames: usize, mut step: impl FnMut() -> usize) -> StageRow {
    stats::set_clock(Some(now_ms));
    let _ = stats::take();
    for _ in 0..frames {
        step();
    }
    let collected = stats::take();
    stats::set_clock(None);
    StageRow { label, stats: collected, frames: frames as u32 }
}

fn main() {
    let size = Size { width: 760.0, height: 640.0 };

    let mut reports = Vec::new();
    let mut stage_rows = Vec::new();

    // 1. MOUNT: a fresh runtime builds and lowers the whole scene
    reports.push(measure("mount (build+capture+diff)", 2, 30, || (), || {
        let (table, runtime) = fresh();
        {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
    }));
    stage_rows.push(stages("mount", 30, || {
        let (table, runtime) = fresh();
        {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
    }));

    // the steady fixture every update scenario shares
    let (table, runtime) = fresh();
    let _ = runtime.dom_frame(&table, size);

    // 2. toggle ONE row: the O(change) question in one number
    let toggles = table.toggles.clone();
    reports.push(measure("toggle 1 row", 5, 51, || (), || {
        toggles[5].set(!toggles[5].get());
        {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
    }));
    stage_rows.push(stages("toggle 1 row", 51, || {
        toggles[5].set(!toggles[5].get());
        {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
    }));

    // 3. toggle ALL rows: the bulk update that must fit a frame budget
    reports.push(measure("toggle all 200", 3, 51, || (), || {
        for toggle in toggles.iter() {
            toggle.set(!toggle.get());
        }
        {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
    }));
    stage_rows.push(stages("toggle all 200", 51, || {
        for toggle in toggles.iter() {
            toggle.set(!toggle.get());
        }
        {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
    }));

    // 4. filter: 200 → 10 rows and back — creates and removes.
    // `prepare` walks to the opposite side untimed; the sample times
    // one direction only.
    let filtered = table.filtered;
    reports.push(measure(
        "filter 10 → 200",
        4,
        50,
        || {
            filtered.set(true);
            let _ = runtime.dom_frame(&table, size);
        },
        || {
            filtered.set(false);
            {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
        },
    ));
    reports.push(measure(
        "filter 200 → 10",
        4,
        50,
        || {
            filtered.set(false);
            let _ = runtime.dom_frame(&table, size);
        },
        || {
            filtered.set(true);
            {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
        },
    ));
    stage_rows.push(stages("filter flip", 50, || {
        filtered.set(!filtered.get());
        {
        let patches = runtime.dom_frame(&table, size);
        std::hint::black_box(encode(&patches).len());
        patches.len()
    }
    }));

    // 5. sustained toggles over the FULL table: frames in one second
    filtered.set(false);
    let _ = runtime.dom_frame(&table, size);
    let deadline = Instant::now();
    let mut sustained = 0u64;
    while deadline.elapsed().as_secs_f64() < 1.0 {
        toggles[sustained as usize % ROWS].set(!toggles[sustained as usize % ROWS].get());
        let _ = runtime.dom_frame(&table, size);
        sustained += 1;
    }

    // MARK: - The tables

    println!(
        "\n{:<28} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "operation", "p50 ms", "p95 ms", "p99 ms", "max ms", "patches", "allocs", "KiB"
    );
    println!("{}", "─".repeat(92));
    for report in &reports {
        println!(
            "{:<28} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>8} {:>8} {:>8}",
            report.label,
            report.p50,
            report.p95,
            report.p99,
            report.max,
            report.patches,
            report.allocations,
            report.kibibytes
        );
    }
    println!("sustained toggles/sec: {sustained}");

    println!(
        "\n{:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>7} {:>7} {:>7} {:>8} {:>8}",
        "per frame", "settle", "layout", "capture", "diff", "encode", "bodies", "walks", "nodes", "visited", "draw"
    );
    println!("{}", "─".repeat(104));
    for row in &stage_rows {
        let per = |value: u32| value as f64 / row.frames as f64;
        let ms = |stage: Stage| row.stats.ms(stage) / row.frames as f64;
        println!(
            "{:<16} {:>7.3}m {:>7.3}m {:>7.3}m {:>7.3}m {:>7.3}m {:>7.1} {:>7.1} {:>7.0} {:>8.0} {:>8.1}",
            row.label,
            ms(Stage::Settle),
            ms(Stage::Layout),
            ms(Stage::Capture),
            ms(Stage::Diff),
            ms(Stage::Encode),
            per(row.stats.body_passes),
            per(row.stats.layout_passes),
            per(row.stats.capture_nodes),
            per(row.stats.diff_visited),
            per(row.stats.display_commands),
        );
    }

    println!("\nfixture: {ROWS} rows, viewport 760×640, PixelFont (deterministic)");
    println!("stage pass runs separately with the stats clock on — wall numbers stay clean");
}
