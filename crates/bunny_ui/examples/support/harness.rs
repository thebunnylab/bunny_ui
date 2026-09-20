//! The frame benches' shared harness: a counting allocator, the
//! percentile loop, the stage pass, and the two tables.
//!
//! This file is not an example of its own. A bench includes it with
//! `#[path = "support/harness.rs"] mod harness;`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use bunny_ui::stats::{self, FrameStats, Stage};

// MARK: - Counting allocator (zero deps: the wrapped System)

pub struct CountingAllocator;

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

fn allocation_snapshot() -> (u64, u64) {
    (ALLOCATIONS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
}

// MARK: - The clock

pub fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

// MARK: - Wall time and allocations

pub struct Report {
    pub label: String,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub commands: usize,
    pub allocations: u64,
    pub kibibytes: u64,
}

/// Runs `frames` iterations. `prepare` is outside the timed part;
/// `step` is ONE frame and returns that frame's draw command count.
pub fn measure(
    label: impl Into<String>,
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
    let mut commands = 0usize;
    let mut allocations = 0u64;
    let mut bytes = 0u64;
    for _ in 0..frames {
        prepare();
        let (allocations_before, bytes_before) = allocation_snapshot();
        let start = Instant::now();
        commands = step();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        let (allocations_after, bytes_after) = allocation_snapshot();
        allocations += allocations_after - allocations_before;
        bytes += bytes_after - bytes_before;
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() as f64 * q) as usize).min(samples.len() - 1)];
    Report {
        label: label.into(),
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
        max: *samples.last().unwrap(),
        commands,
        allocations: allocations / frames as u64,
        kibibytes: bytes / frames as u64 / 1024,
    }
}

// MARK: - The stage pass

pub struct StageRow {
    pub label: String,
    pub stats: FrameStats,
    pub frames: u32,
}

/// The same step with the stats clock on, totals divided per frame.
/// It is a separate pass, so the wall numbers never pay for the timers.
pub fn stages(
    label: impl Into<String>,
    frames: usize,
    mut prepare: impl FnMut(),
    mut step: impl FnMut() -> usize,
) -> StageRow {
    stats::set_clock(Some(now_ms));
    let mut total = FrameStats::default();
    for _ in 0..frames {
        prepare();
        let _ = stats::take();
        step();
        add(&mut total, &stats::take());
    }
    stats::set_clock(None);
    StageRow { label: label.into(), stats: total, frames: frames as u32 }
}

fn add(total: &mut FrameStats, frame: &FrameStats) {
    total.body_passes += frame.body_passes;
    total.layout_passes += frame.layout_passes;
    total.display_commands += frame.display_commands;
    total.measure_hits += frame.measure_hits;
    total.measure_misses += frame.measure_misses;
    total.assemblies += frame.assemblies;
    total.hover_relayouts += frame.hover_relayouts;
    total.paints += frame.paints;
    for (sum, part) in total.stage_ms.iter_mut().zip(frame.stage_ms) {
        *sum += part;
    }
}

// MARK: - The tables

pub fn print_reports(title: &str, reports: &[Report]) {
    println!("\n{title}");
    println!(
        "{:<44} {:>8} {:>8} {:>8} {:>8} {:>7} {:>8} {:>7}",
        "scenario (1 frame =)", "p50 ms", "p95 ms", "p99 ms", "max ms", "cmds", "allocs", "KiB"
    );
    println!("{}", "─".repeat(106));
    for report in reports {
        println!(
            "{:<44} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>7} {:>8} {:>7}",
            report.label,
            report.p50,
            report.p95,
            report.p99,
            report.max,
            report.commands,
            report.allocations,
            report.kibibytes,
        );
    }
}

pub fn print_stages(title: &str, rows: &[StageRow]) {
    println!("\n{title}");
    println!(
        "{:<30} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} | {:>6} {:>7} {:>5} {:>6} {:>6}",
        "per frame, ms",
        "settle",
        "layout",
        "pass",
        "asm",
        "measure",
        "place",
        "hover",
        "passes",
        "layouts",
        "asm#",
        "hover#",
        "paints",
    );
    println!("{}", "─".repeat(128));
    for row in rows {
        let frames = f64::from(row.frames);
        let ms = |stage: Stage| row.stats.ms(stage) / frames;
        let count = |total: u32| f64::from(total) / frames;
        println!(
            "{:<30} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} | {:>6.1} {:>7.1} {:>5.1} {:>6.1} {:>6.1}",
            row.label,
            ms(Stage::Settle),
            ms(Stage::Layout),
            ms(Stage::Pass),
            ms(Stage::Assemble),
            ms(Stage::Measure),
            ms(Stage::Place),
            ms(Stage::Hover),
            count(row.stats.body_passes),
            count(row.stats.layout_passes),
            count(row.stats.assemblies),
            count(row.stats.hover_relayouts),
            count(row.stats.paints),
        );
    }
    println!("pass is inside settle or layout; asm is inside pass; measure and place are inside layout;");
    println!("hover holds its own second layout. Do not add the nested columns.");
}
