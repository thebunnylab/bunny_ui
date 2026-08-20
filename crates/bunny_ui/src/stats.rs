//! Frame diagnostics: counters at the pipeline's seams, and wall time
//! per stage when a clock is installed.
//!
//! The counters are always on — plain thread-local cells, one integer
//! add at each seam, no observable cost on the hot path. The timers
//! cost one branch when no clock is installed; a bench or a shell that
//! wants the stage table installs one with [`set_clock`] and drains
//! the totals with [`take`].
//!
//! This module is a diagnostic, not a feature: nothing in the engine
//! reads these numbers back. They exist so a claim about the pipeline
//! ("one layout pass per event", "the diff did not traverse that
//! subtree") is a pinned number instead of a belief.

use std::cell::Cell;

/// The timed stages of a frame, at the seams — never inside a walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Bodies: the settle loop (poll, pass, pump).
    Settle,
    /// A measure+place walk without the Dom capture riding it.
    Layout,
    /// A measure+place walk WITH the Dom capture riding it.
    Capture,
    /// The Dom diff: new scene against the retained one.
    Diff,
    /// The patch list becoming wire bytes.
    Encode,
}

const STAGES: usize = 5;

/// One frame's worth of pipeline work, drained by [`take`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Body passes the settle loop ran.
    pub body_passes: u32,
    /// Measure+place walks that really walked (the stable-root skip
    /// does not count — that is the point of counting).
    pub layout_passes: u32,
    /// Draw commands collected across those walks.
    pub display_commands: u32,
    /// Nodes the Dom capture created.
    pub capture_nodes: u32,
    /// Nodes the Dom diff visited.
    pub diff_visited: u32,
    /// Retained subtrees the diff reused without traversal.
    pub diff_reused: u32,
    /// Patches the diff emitted.
    pub patches: u32,
    /// Bytes the encoder wrote.
    pub encode_bytes: u32,
    /// Text measurements answered by the cache.
    pub measure_hits: u32,
    /// Text measurements that reached the text engine.
    pub measure_misses: u32,
    /// Milliseconds per [`Stage`], all zero without a clock.
    pub stage_ms: [f64; STAGES],
}

impl FrameStats {
    /// The stage's accumulated wall time in milliseconds.
    pub fn ms(&self, stage: Stage) -> f64 {
        self.stage_ms[stage as usize]
    }
}

thread_local! {
    static BODY_PASSES: Cell<u32> = const { Cell::new(0) };
    static LAYOUT_PASSES: Cell<u32> = const { Cell::new(0) };
    static DISPLAY_COMMANDS: Cell<u32> = const { Cell::new(0) };
    static CAPTURE_NODES: Cell<u32> = const { Cell::new(0) };
    static DIFF_VISITED: Cell<u32> = const { Cell::new(0) };
    static DIFF_REUSED: Cell<u32> = const { Cell::new(0) };
    static PATCHES: Cell<u32> = const { Cell::new(0) };
    static ENCODE_BYTES: Cell<u32> = const { Cell::new(0) };
    static MEASURE_HITS: Cell<u32> = const { Cell::new(0) };
    static MEASURE_MISSES: Cell<u32> = const { Cell::new(0) };
    static STAGE_MS: Cell<[f64; STAGES]> = const { Cell::new([0.0; STAGES]) };
    static CLOCK: Cell<Option<fn() -> f64>> = const { Cell::new(None) };
}

/// Installs the wall clock the timers read, in milliseconds. `None`
/// (the default) turns every timer into a single branch. A bench
/// installs `Instant`-based ticks; a wasm shell can install one built
/// on `performance.now` when the page asks for the table.
pub fn set_clock(clock: Option<fn() -> f64>) {
    CLOCK.with(|slot| slot.set(clock));
}

/// Snapshots the totals accumulated since the last call, and resets.
pub fn take() -> FrameStats {
    FrameStats {
        body_passes: BODY_PASSES.with(|c| c.replace(0)),
        layout_passes: LAYOUT_PASSES.with(|c| c.replace(0)),
        display_commands: DISPLAY_COMMANDS.with(|c| c.replace(0)),
        capture_nodes: CAPTURE_NODES.with(|c| c.replace(0)),
        diff_visited: DIFF_VISITED.with(|c| c.replace(0)),
        diff_reused: DIFF_REUSED.with(|c| c.replace(0)),
        patches: PATCHES.with(|c| c.replace(0)),
        encode_bytes: ENCODE_BYTES.with(|c| c.replace(0)),
        measure_hits: MEASURE_HITS.with(|c| c.replace(0)),
        measure_misses: MEASURE_MISSES.with(|c| c.replace(0)),
        stage_ms: STAGE_MS.with(|c| c.replace([0.0; STAGES])),
    }
}

/// Times `run` under `stage` when a clock is installed; otherwise the
/// only cost is reading the empty slot.
pub(crate) fn time<T>(stage: Stage, run: impl FnOnce() -> T) -> T {
    let Some(clock) = CLOCK.with(|slot| slot.get()) else {
        return run();
    };
    let start = clock();
    let out = run();
    let elapsed = clock() - start;
    STAGE_MS.with(|cell| {
        let mut totals = cell.get();
        totals[stage as usize] += elapsed;
        cell.set(totals);
    });
    out
}

#[inline]
fn bump(cell: &'static std::thread::LocalKey<Cell<u32>>, by: u32) {
    cell.with(|c| c.set(c.get().wrapping_add(by)));
}

#[inline]
pub(crate) fn note_body_pass() {
    bump(&BODY_PASSES, 1);
}

#[inline]
pub(crate) fn note_layout_pass() {
    bump(&LAYOUT_PASSES, 1);
}

#[inline]
pub(crate) fn note_display(commands: usize) {
    bump(&DISPLAY_COMMANDS, commands as u32);
}

#[inline]
pub(crate) fn note_capture_node() {
    bump(&CAPTURE_NODES, 1);
}

#[inline]
pub(crate) fn note_diff_visit() {
    bump(&DIFF_VISITED, 1);
}

#[inline]
#[allow(dead_code)] // the diff learns to reuse in the O(change) round
pub(crate) fn note_diff_reuse() {
    bump(&DIFF_REUSED, 1);
}

#[inline]
pub(crate) fn note_encode(patches: usize, bytes: usize) {
    bump(&PATCHES, patches as u32);
    bump(&ENCODE_BYTES, bytes as u32);
}

#[inline]
pub(crate) fn note_measure(hit: bool) {
    if hit {
        bump(&MEASURE_HITS, 1);
    } else {
        bump(&MEASURE_MISSES, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_drains_and_resets() {
        let _ = take();
        note_body_pass();
        note_layout_pass();
        note_layout_pass();
        note_capture_node();
        note_encode(3, 40);
        note_measure(true);
        note_measure(false);
        let stats = take();
        assert_eq!(stats.body_passes, 1);
        assert_eq!(stats.layout_passes, 2);
        assert_eq!(stats.capture_nodes, 1);
        assert_eq!(stats.patches, 3);
        assert_eq!(stats.encode_bytes, 40);
        assert_eq!(stats.measure_hits, 1);
        assert_eq!(stats.measure_misses, 1);
        assert_eq!(take(), FrameStats::default(), "the drain resets");
    }

    #[test]
    fn timers_sleep_without_a_clock() {
        let _ = take();
        let out = time(Stage::Diff, || 7);
        assert_eq!(out, 7);
        assert_eq!(take().ms(Stage::Diff), 0.0, "no clock, no time");
    }

    #[test]
    fn timers_accumulate_under_a_clock() {
        let _ = take();
        fn ticks() -> f64 {
            use std::cell::Cell;
            thread_local! {
                static NEXT: Cell<f64> = const { Cell::new(0.0) };
            }
            NEXT.with(|next| {
                let now = next.get();
                next.set(now + 1.5);
                now
            })
        }
        set_clock(Some(ticks));
        time(Stage::Layout, || ());
        time(Stage::Layout, || ());
        set_clock(None);
        assert_eq!(take().ms(Stage::Layout), 3.0);
    }
}
