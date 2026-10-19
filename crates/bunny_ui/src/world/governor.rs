//! The governor: what a box's world may cost, kept under what the device
//! can pay.
//!
//! A world asks for a quality ([`super::frame::WorldStyle::quality`]); the
//! governor decides the quality it draws at, never above the ask. It
//! starts from what the device's name says it can carry, and it watches
//! the beat of the frames while the world moves: a beat that keeps
//! missing the display's steps a quality down, a run of long frames steps
//! down at once, and a long stretch with room to spare steps up — once a
//! session, and one step past where it started at most. A world at rest
//! is never measured: it costs nothing.
//!
//! The beat is measured on the engine clock between the frames that drew
//! the world, so every tier is governed the same way. A tier that times
//! its own GPU tells the governor how long each world took there too
//! ([`Governor::timed`]): nine whole frames in ten must cost under 10 ms
//! and nine redraws of a part under 3 ms, or the quality steps down; a long
//! stretch at a third of that steps up. A world that does not move is
//! drawn once and governs nothing.

// the GPU seam keeps a governor for each box; without it, only the
// device's start is read
#![cfg_attr(not(feature = "gpu"), allow(dead_code))]

use std::collections::VecDeque;

use super::frame::Quality;

/// The frames the governor weighs, while the world moves.
const WINDOW: usize = 60;
/// The share of the window under which nine frames in ten fall.
const PERCENTILE: f64 = 0.9;
/// How far past the display's beat the window's slow end may run.
const OVER_BEAT: f64 = 1.4;
/// A frame longer than this, in seconds, is a long frame...
const LONG_FRAME: f64 = 0.05;
/// ...and this many of them within [`LONG_SPAN`] seconds step down at once.
const LONG_FRAMES: usize = 5;
const LONG_SPAN: f64 = 2.0;
/// The moving frames with room to spare before a step up.
const ROOM_FRAMES: u32 = 600;
/// How close to the display's beat a frame with room to spare stays.
const ROOM_BEAT: f64 = 1.05;
/// A gap longer than this between two drawn frames is a pause, not a beat.
const PAUSE: f64 = 0.25;
/// The shortest beat a display keeps: no measure goes under it.
const FASTEST_BEAT: f64 = 1.0 / 240.0;
/// The GPU times the governor weighs, of each kind of frame.
const TIMED_WINDOW: usize = 30;
/// What nine frames in ten may cost the GPU, in seconds: a frame drawn
/// whole, and a frame that redraws a part.
const WHOLE_BUDGET: f64 = 0.010;
const PART_BUDGET: f64 = 0.003;
/// A share of the budget under which a frame has room to spare...
const TIMED_ROOM: f64 = 0.35;
/// ...and how many such whole frames before a step up.
const TIMED_ROOM_FRAMES: u32 = 120;

/// What a box's world draws at, and why.
#[derive(Clone, Debug)]
pub(crate) struct Governor {
    /// The quality the device's name starts it at.
    start: Quality,
    current: Quality,
    /// The beat between the last moving frames, in seconds.
    beats: VecDeque<f64>,
    /// When each recent long frame ended.
    long: VecDeque<f64>,
    /// The shortest beat seen: the display's step.
    shortest: f64,
    /// When the world last drew.
    last: Option<f64>,
    /// Moving frames in a row with room to spare.
    room: u32,
    /// Whether this session stepped up already.
    stepped_up: bool,
    /// What the GPU took over the last frames drawn whole, and over the
    /// last redraws of a part, in seconds.
    whole_times: VecDeque<f64>,
    part_times: VecDeque<f64>,
    /// Whole frames in a row whose GPU time had room to spare.
    timed_room: u32,
}

impl Governor {
    /// A governor for a device whose name says it carries `start`.
    pub(crate) fn new(start: Quality) -> Governor {
        Governor {
            start,
            current: start,
            beats: VecDeque::with_capacity(WINDOW),
            long: VecDeque::new(),
            shortest: f64::INFINITY,
            last: None,
            room: 0,
            stepped_up: false,
            whole_times: VecDeque::with_capacity(TIMED_WINDOW),
            part_times: VecDeque::with_capacity(TIMED_WINDOW),
            timed_room: 0,
        }
    }

    /// The quality to draw at when the world asks for `asked`.
    pub(crate) fn quality(&self, asked: Quality) -> Quality {
        asked.min(self.current)
    }

    /// The world drew at `now` (seconds of the engine clock).
    pub(crate) fn drew(&mut self, now: f64) {
        let last = self.last.replace(now);
        let Some(last) = last else { return };
        let beat = now - last;
        if beat <= 0.0 || beat.is_nan() || beat > PAUSE {
            // a pause between moves: the beat starts again
            return;
        }
        let beat = beat.max(FASTEST_BEAT);
        self.shortest = self.shortest.min(beat);
        if self.beats.len() == WINDOW {
            self.beats.pop_front();
        }
        self.beats.push_back(beat);
        // a run of long frames steps down at once
        if beat > LONG_FRAME {
            self.long.push_back(now);
        }
        while self.long.front().is_some_and(|&at| now - at > LONG_SPAN) {
            self.long.pop_front();
        }
        if self.long.len() >= LONG_FRAMES {
            self.step_down();
            return;
        }
        if self.beats.len() < WINDOW {
            return;
        }
        let slow = self.slow_end();
        if slow > self.shortest * OVER_BEAT {
            self.step_down();
        } else if slow <= self.shortest * ROOM_BEAT {
            self.room += 1;
            if self.room >= ROOM_FRAMES {
                self.step_up();
            }
        } else {
            self.room = 0;
        }
    }

    /// The GPU took `seconds` over one of the world's frames: one drawn
    /// whole, or one that redrew a part.
    pub(crate) fn timed(&mut self, seconds: f64, whole: bool) {
        if !(seconds.is_finite() && seconds >= 0.0) {
            return;
        }
        let window = if whole { &mut self.whole_times } else { &mut self.part_times };
        if window.len() == TIMED_WINDOW {
            window.pop_front();
        }
        window.push_back(seconds);
        if window.len() < TIMED_WINDOW {
            return;
        }
        let slow = percentile(window);
        let budget = if whole { WHOLE_BUDGET } else { PART_BUDGET };
        if slow > budget {
            self.step_down();
        } else if whole && slow < budget * TIMED_ROOM {
            self.timed_room += 1;
            if self.timed_room >= TIMED_ROOM_FRAMES {
                self.step_up();
            }
        } else if whole {
            self.timed_room = 0;
        }
    }

    /// The beat nine frames in ten keep under.
    fn slow_end(&self) -> f64 {
        percentile(&self.beats)
    }

    fn step_down(&mut self) {
        self.current = match self.current {
            Quality::High => Quality::Medium,
            Quality::Medium | Quality::Low => Quality::Low,
        };
        // the next window weighs the new quality alone
        self.beats.clear();
        self.long.clear();
        self.room = 0;
        self.whole_times.clear();
        self.part_times.clear();
        self.timed_room = 0;
    }

    fn step_up(&mut self) {
        self.room = 0;
        self.timed_room = 0;
        let next = match self.current {
            Quality::Low => Quality::Medium,
            Quality::Medium | Quality::High => Quality::High,
        };
        if self.stepped_up || next > step(self.start) {
            return;
        }
        self.stepped_up = true;
        self.current = next;
        self.beats.clear();
        self.whole_times.clear();
        self.part_times.clear();
    }
}

/// The value nine in ten of `values` keep under.
fn percentile(values: &VecDeque<f64>) -> f64 {
    let mut sorted: Vec<f64> = values.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * PERCENTILE) as usize).min(sorted.len() - 1)]
}

/// One quality above `quality`.
fn step(quality: Quality) -> Quality {
    match quality {
        Quality::Low => Quality::Medium,
        Quality::Medium | Quality::High => Quality::High,
    }
}

/// The quality a device starts at, from the name its graphics answer to:
/// a software rasterizer and an older integrated part start low, a part
/// of the middle at the middle, and a discrete or a newer part high. An
/// unknown name starts low: a fast device climbs a step on its own.
pub fn device_quality(name: &str) -> Quality {
    let name = name.to_ascii_lowercase();
    let has = |word: &str| name.contains(word);
    // software: the CPU draws the picture in the GPU's stead
    if has("llvmpipe") || has("lavapipe") || has("swiftshader") || has("softpipe") || has("microsoft basic render") {
        return Quality::Low;
    }
    // discrete parts and the larger Apple chips
    if has("geforce") || has("quadro") || has("rtx") || has("radeon rx") || has("radeon pro") || has("arc a") {
        return Quality::High;
    }
    if has("apple m") {
        let pro = has(" pro") || has(" max") || has(" ultra");
        let generation = name.split("apple m").nth(1).and_then(|rest| rest.chars().next()).and_then(|c| c.to_digit(10)).unwrap_or(1);
        return if pro || generation >= 3 { Quality::High } else { Quality::Medium };
    }
    // the integrated parts of the middle
    if has("iris") || has("intel(r) xe") || has("intel xe") || has("arc(tm) graphics") || has("vega") || has("radeon 680m") || has("radeon 780m") {
        return Quality::Medium;
    }
    if has("adreno") {
        let model = name.split("adreno").nth(1).map(|rest| rest.trim_start_matches(|c: char| !c.is_ascii_digit()));
        let series = model.and_then(|rest| rest.chars().next()).and_then(|c| c.to_digit(10)).unwrap_or(0);
        return if series >= 7 { Quality::Medium } else { Quality::Low };
    }
    if has("apple a") {
        let generation: u32 = name.split("apple a").nth(1).map(|rest| rest.chars().take_while(char::is_ascii_digit).collect::<String>()).and_then(|digits| digits.parse().ok()).unwrap_or(0);
        return if generation >= 14 { Quality::Medium } else { Quality::Low };
    }
    Quality::Low
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(governor: &mut Governor, from: f64, beat: f64, frames: usize) -> f64 {
        let mut now = from;
        for _ in 0..frames {
            now += beat;
            governor.drew(now);
        }
        now
    }

    #[test]
    fn a_world_that_keeps_the_beat_keeps_its_quality() {
        let mut governor = Governor::new(Quality::High);
        let end = run(&mut governor, 0.0, 1.0 / 60.0, 300);
        assert_eq!(governor.quality(Quality::High), Quality::High);
        // pauses between moves are not beats
        run(&mut governor, end + 3.0, 1.0 / 60.0, 30);
        assert_eq!(governor.quality(Quality::High), Quality::High);
    }

    #[test]
    fn a_beat_that_keeps_missing_steps_down() {
        let mut governor = Governor::new(Quality::High);
        let now = run(&mut governor, 0.0, 1.0 / 60.0, 20);
        // every other frame takes two of the display's steps: one window
        // of it steps down once (a second would step again)
        let mut at = now;
        for frame in 0..(WINDOW + 10) {
            at += if frame % 2 == 0 { 2.0 / 60.0 } else { 1.0 / 60.0 };
            governor.drew(at);
        }
        assert_eq!(governor.quality(Quality::High), Quality::Medium, "one step down");
        assert_eq!(governor.quality(Quality::Low), Quality::Low, "never above the ask");
    }

    #[test]
    fn a_run_of_long_frames_steps_down_at_once() {
        let mut governor = Governor::new(Quality::Medium);
        let now = run(&mut governor, 0.0, 1.0 / 60.0, 10);
        run(&mut governor, now, 0.08, 5);
        assert_eq!(governor.quality(Quality::High), Quality::Low);
    }

    #[test]
    fn room_to_spare_steps_up_once_and_one_step_past_the_start() {
        let mut governor = Governor::new(Quality::Low);
        run(&mut governor, 0.0, 1.0 / 60.0, WINDOW + ROOM_FRAMES as usize + 10);
        assert_eq!(governor.quality(Quality::High), Quality::Medium, "one step up");
        run(&mut governor, 100.0, 1.0 / 60.0, 2 * (WINDOW + ROOM_FRAMES as usize));
        assert_eq!(governor.quality(Quality::High), Quality::Medium, "once a session");
    }

    #[test]
    fn the_gpu_time_steps_down_a_slow_world_and_up_a_light_one() {
        // nine whole frames in ten over 10 ms on the GPU: a step down
        let mut governor = Governor::new(Quality::High);
        for _ in 0..TIMED_WINDOW {
            governor.timed(0.014, true);
        }
        assert_eq!(governor.quality(Quality::High), Quality::Medium);
        // redraws of a part answer to 3 ms
        for _ in 0..TIMED_WINDOW {
            governor.timed(0.002, false);
        }
        assert_eq!(governor.quality(Quality::High), Quality::Medium, "under the part's budget");
        for _ in 0..TIMED_WINDOW {
            governor.timed(0.005, false);
        }
        assert_eq!(governor.quality(Quality::High), Quality::Low, "over it");
        // a long stretch of light whole frames steps up once
        for _ in 0..(TIMED_WINDOW + TIMED_ROOM_FRAMES as usize) {
            governor.timed(0.001, true);
        }
        assert_eq!(governor.quality(Quality::High), Quality::Medium);
        // a lost measure is no measure
        let held = governor.whole_times.len();
        governor.timed(f64::NAN, true);
        governor.timed(-1.0, true);
        assert_eq!(governor.whole_times.len(), held);
    }

    #[test]
    fn the_device_names_its_start() {
        assert_eq!(device_quality("Intel(R) UHD Graphics 620"), Quality::Low);
        assert_eq!(device_quality("llvmpipe (LLVM 20.1.2, 256 bits)"), Quality::Low);
        assert_eq!(device_quality("Microsoft Basic Render Driver"), Quality::Low);
        assert_eq!(device_quality("Intel(R) Iris(R) Xe Graphics"), Quality::Medium);
        assert_eq!(device_quality("Apple M1"), Quality::Medium);
        assert_eq!(device_quality("Apple M2 Pro"), Quality::High);
        assert_eq!(device_quality("Apple M5"), Quality::High);
        assert_eq!(device_quality("NVIDIA GeForce RTX 3060"), Quality::High);
        assert_eq!(device_quality("Adreno (TM) 740"), Quality::Medium);
        assert_eq!(device_quality("Adreno (TM) 618"), Quality::Low);
        assert_eq!(device_quality("Apple A15 GPU"), Quality::Medium);
        assert_eq!(device_quality("Apple A12 GPU"), Quality::Low);
        assert_eq!(device_quality("Something New"), Quality::Low);
    }
}
