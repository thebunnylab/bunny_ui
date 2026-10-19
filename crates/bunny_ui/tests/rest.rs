//! Rest is rest. A scene nobody touches, with every poller an app mounts
//! still running, owes the shell nothing: no body pass, no layout, no
//! frame — and no block of memory kept per wake. These tests mount what
//! a product window mounts at rest and count.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use bunny_ui::custom::{CustomElement, PaintCtx, Painter, canvas, custom};
use bunny_ui::layout::{Color, Point, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;
use bunny_ui::stats;
use bunny_ui::task;

// MARK: - Counting both ways, per thread

/// The harness runs each test on a thread of its own, and the engine is
/// single-threaded: a count kept per thread is the test's own, whatever
/// its neighbours allocate or free.
struct Counting;

thread_local! {
    static MADE: Cell<u64> = const { Cell::new(0) };
    static FREED: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = MADE.try_with(|made| made.set(made.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = FREED.try_with(|freed| freed.set(freed.get() + 1));
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Blocks this thread made and has not freed — what a leak grows.
fn live() -> i64 {
    MADE.with(Cell::get) as i64 - FREED.with(Cell::get) as i64
}

fn clock() -> f64 {
    0.0
}

// MARK: - The scene

const SIZE: Size = Size { width: 400.0, height: 300.0 };
const POLL_MS: f64 = 1.0 / 30.0;

/// A box that reads a value as it paints — an editor, a meter.
struct Reader {
    value: State<u32>,
}

impl CustomElement for Reader {
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        let color = if self.value.get() % 2 == 0 { Color::WHITE } else { Color::BLACK };
        painter.fill(Rect { origin: Point::ZERO, size: ctx.frame.size }, color);
    }
}

#[derive(Clone)]
struct Page {
    /// Written by a poller every tick, to the same value.
    status: State<u32>,
    /// Written by a poller every tick, to the same value, through a store.
    count: State<u32>,
    /// Read by a box's paint only.
    meter: State<u32>,
}

impl Component for Page {
    fn body(self) -> impl View {
        let status = self.status;
        let count = self.count;
        vstack!(
            text!("status {}", status),
            text!("count {}", count),
            custom(Reader { value: self.meter }).frame(SIZE.width, 40.0).id("meter"),
            // the product's decoration: a loop of its own, on its own layer
            canvas(|ctx, painter| {
                painter.fill(ctx.bounds(), Color::BLACK);
            })
            .looping(Loop::secs(2.0).fps(5.0))
            .frame(26.0, 26.0)
            .id("mark"),
        )
        // the pollers: a terminal asking its pty, an archive asking its
        // disk, a finder draining its worker — each lands the same answer
        .task(move || async move {
            loop {
                task::sleep(std::time::Duration::from_secs_f64(POLL_MS)).await;
                status.set_if_changed(7);
            }
        })
        .task(move || async move {
            loop {
                task::sleep(std::time::Duration::from_millis(8)).await;
                count.set_if_changed(0);
            }
        })
    }
}

fn mount() -> (Runtime, Page) {
    stats::set_clock(Some(clock));
    let page = Page { status: State::new(7), count: State::new(0), meter: State::new(0) };
    let runtime = Runtime::new();
    runtime.set_overlay_layers(true);
    // the first frame mounts the tasks and paints everything
    let _ = runtime.display_frame(&page, SIZE);
    let _ = runtime.display_frame(&page, SIZE);
    let _ = stats::take();
    (runtime, page)
}

/// One wake of the shell's: the clock moves, the sleepers due run, and the
/// engine is asked whether a frame is owed — the Wake road, headless.
fn wake(runtime: &Runtime, page: &Page, dt: f64) -> bunny_ui::runtime::FrameNeed {
    let _ = runtime.tick(dt);
    runtime.poll_tasks();
    let need = runtime.frame_need();
    if need.any() {
        let _ = runtime.display_frame(page, SIZE);
    }
    need
}

#[test]
fn pollers_that_land_the_same_answer_owe_no_frame() {
    let (runtime, page) = mount();
    let mut frames = 0;
    for _ in 0..600 {
        let need = wake(&runtime, &page, 1.0 / 120.0);
        if need.any() {
            frames += 1;
        }
        assert!(!need.wrote && !need.dirty, "a poller landing the same answer reaches nobody: {need:?}");
    }
    assert_eq!(frames, 0, "five seconds at rest, no frame");
    let stats = stats::take();
    assert_eq!(stats.body_passes, 0, "no body ran");
    assert_eq!(stats.layout_passes, 0, "no layout ran");
}

#[test]
fn a_quiet_settle_runs_no_pass() {
    let (runtime, page) = mount();
    let _ = stats::take();
    // a blink, a pointer, a window callback: frames with nothing written
    for _ in 0..20 {
        runtime.settle(&page);
    }
    let stats = stats::take();
    assert_eq!(stats.body_passes, 0, "twenty quiet settles, no pass");
    assert_eq!(stats.settles_skipped, 20);
}

#[test]
fn a_value_only_a_box_paints_repaints_the_box_not_the_scene() {
    let (runtime, page) = mount();
    page.meter.set(1);
    let need = runtime.frame_need();
    assert!(need.paints && !need.wrote && !need.dirty, "{need:?}");
    assert!(need.only_paints(), "{need:?}");
    // the box has no overlay island: the frame's road, once
    let (blits, plain) = runtime.repaint_dirty_paints(1);
    assert!(blits.is_empty());
    assert!(plain);
}

#[test]
fn the_driver_parks_when_the_shell_keeps_the_sleepers() {
    let (runtime, page) = mount();
    // headless, the clock is the tick's: a sleeper eight milliseconds away
    // asks the display to beat for it
    assert_eq!(runtime.frame_pace(), FramePace::Display, "a sleeper under a frame away, on the tick road");
    // a shell that drives the sleepers' clock by the wall asks nothing of
    // the display on their account: only the loop's own step is left
    runtime.drive_tasks_by_wall();
    assert_eq!(runtime.frame_pace(), FramePace::Slow(1.0 / 5.0), "the mark's five frames a second");
    let _ = page;
}

#[test]
fn ten_thousand_wakes_keep_no_block() {
    let (runtime, page) = mount();
    // warm every path once
    for _ in 0..200 {
        let _ = wake(&runtime, &page, 1.0 / 120.0);
    }
    let before = live();
    for _ in 0..10_000 {
        let _ = wake(&runtime, &page, 1.0 / 120.0);
    }
    let grown = live() - before;
    assert!(grown <= 8, "ten thousand wakes at rest kept {grown} blocks");
}
