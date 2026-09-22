//! Touch: the finger speaks the pointer's vocabulary.
//!
//! A mouse says three things — moved, pressed, released — and the
//! runtime already answers each. A finger says one thing, *touched*,
//! and what it MEANT is known only later: a tap when it lifts still, a
//! pan when it moves over a surface that scrolls, a press when it
//! lands where nothing scrolls, a menu when it holds still for half a
//! second, a zoom when a second finger joins. This module is that
//! later. It reads raw touches and answers [`Gesture`]s the runtime
//! performs with the doors it already has — `pointer_clicked`,
//! `pointer_moved`, `pointer_released`, `wheel`, `context_click` — so
//! no view learns a new event and every shell (UIKit today, others
//! tomorrow) forwards touches the same way.
//!
//! The recognizer is CLOCKLESS. No timestamp crosses the door: time
//! enters only through `tick(dt)`, exactly as the tooltip's life does.
//! The consequences are deliberate. The shell passes the platform's own
//! tap count (a double tap is counted where every other shell counts
//! its clicks). The frame driver runs while a finger is down, so a tick
//! can age the hold and sample the velocity. And a fling's decay is an
//! exact integral, so the same fling travels the same distance at 60
//! and at 120 frames a second.
//!
//! What a finger cannot say: hover. Nothing pairs with the pointer
//! between gestures, so a `*_hovered` paint never shows on a touch
//! surface and a tooltip never arms. The pressed paint is the
//! interactive paint. The release therefore ends with `pointer_exited`
//! — a lifted finger is not still there. A rubber band past the edge is
//! not in this round: a fling stops at the clamp.

use crate::layout::{Point, Px};

/// How far a finger travels before a touch stops being a tap.
pub const SLOP: Px = 8.0;
/// How long a still finger holds before it becomes a menu or a press.
pub const LONG_PRESS: f64 = 0.5;
/// Two fingers closer than this cannot be a zoom: the ratio of two
/// tiny spans jumps at every jitter, so the span is held at this floor.
pub const PINCH_MIN_SPAN: Px = 24.0;
/// The fling's velocity keeps this much of itself every millisecond.
const FLING_DECAY_PER_MS: f64 = 0.998;
/// Below this speed a lift does not fling, in points per second.
const FLING_START: Px = 50.0;
/// Below this speed a fling has come to rest, in points per second.
const FLING_STOP: Px = 5.0;
/// The seconds of movement behind a lift the velocity is read from.
const VELOCITY_WINDOW: f64 = 0.1;

/// What the recognizer asks the scene before it decides. The runtime
/// answers from the tables of its last layout.
pub trait TouchScene {
    /// A scroll region with travel on either axis lies under the point,
    /// or an app box (a box scrolls itself through the wheel).
    fn pans_at(&self, at: Point) -> bool;
    /// A box that takes the drag, a split grip or a thumb lies under
    /// the point: a finger there is a hand on the thing at once.
    fn grabs_at(&self, at: Point) -> bool;
    /// A context menu answers a long press at the point.
    fn menu_at(&self, at: Point) -> bool;
}

/// What a touch meant, in the pointer's words.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Gesture {
    /// The pointer went down here — `pointer_clicked(at, taps)`.
    ///
    /// `held` is how long the finger had already been down when this
    /// press was DECIDED, in seconds. A press over something that pans
    /// waits to see whether the finger meant to scroll, so the press a
    /// box receives can be the lift of a finger that was down for half
    /// a second — and a box that answers a hold differently from a tap
    /// (a text surface does) has no other way to tell them apart.
    Press { at: Point, taps: u8, held: f64 },
    /// The pressed pointer moved — `pointer_moved(at)`.
    Move { at: Point },
    /// The pointer came up — `pointer_released(at)`, then it is gone.
    Release { at: Point },
    /// A secondary click — `context_click(at)`.
    Menu { at: Point },
    /// Content slides under the finger: the delta the finger moved,
    /// anchored where the gesture began so one region owns it whole.
    /// The sign is the finger's own; the wheel reads it the same way.
    Scroll { anchor: Point, dx: Px, dy: Px },
    /// A press that ends without a release — nothing fires.
    Cancel,
    /// Two fingers changed their distance: the RATIO of this step, 1.0
    /// for none, at the point between them.
    Magnify { at: Point, scale: f64 },
}

/// One finger the recognizer follows.
#[derive(Clone, Copy)]
struct Finger {
    id: u64,
    at: Point,
}

/// Where the gesture stands.
#[derive(Clone, Copy)]
enum Phase {
    Idle,
    /// A finger is down over something that may pan: the press waits,
    /// because a press has effects a pan must never cause.
    Undecided { start: Point, taps: u8, held: f64 },
    /// The press went down — every move is the pointer's. A press that
    /// holds STILL is still listened to: half a second over a menu, and
    /// the press is taken back for the menu.
    Pressing { start: Point, held: f64, still: bool },
    /// The finger slides content.
    Panning { start: Point },
    /// A long press opened a menu: the rest of this touch is spent.
    Swallowed,
    /// The finger lifted at speed; content keeps sliding on the clock.
    Flinging { anchor: Point, velocity: (Px, Px) },
    /// Two or more fingers: the span between them is the zoom.
    Pinching { span: Px },
}

/// One frame of finger movement, for the velocity behind a lift.
#[derive(Clone, Copy)]
struct Sample {
    dt: f64,
    dx: Px,
    dy: Px,
}

/// The touch state machine. One per runtime; fed by the shell, read by
/// the runtime, driven by the frame clock.
pub struct Recognizer {
    phase: Phase,
    fingers: Vec<Finger>,
    /// Movement since the last tick, waiting to become a sample.
    pending: (Px, Px),
    /// The last few frames of movement, newest last.
    samples: Vec<Sample>,
}

impl Default for Recognizer {
    fn default() -> Self {
        Recognizer::new()
    }
}

impl Recognizer {
    pub fn new() -> Recognizer {
        Recognizer { phase: Phase::Idle, fingers: Vec::new(), pending: (0.0, 0.0), samples: Vec::new() }
    }

    /// A finger landed. `taps` is the platform's count for it.
    pub fn began(&mut self, id: u64, at: Point, taps: u8, scene: &dyn TouchScene) -> Vec<Gesture> {
        let mut out = Vec::new();
        if self.fingers.iter().any(|finger| finger.id == id) {
            return out;
        }
        self.fingers.push(Finger { id, at });
        if self.fingers.len() >= 2 {
            // a second finger: whatever the first was doing ends, and
            // the pair is a zoom from here
            if let Phase::Pressing { .. } = self.phase {
                out.push(Gesture::Cancel);
            }
            self.samples.clear();
            self.pending = (0.0, 0.0);
            self.phase = Phase::Pinching { span: self.spread() };
            return out;
        }
        // a hand on flying content catches it
        if let Phase::Flinging { .. } = self.phase {
            self.phase = Phase::Idle;
        }
        self.samples.clear();
        self.pending = (0.0, 0.0);
        if scene.grabs_at(at) || !scene.pans_at(at) {
            // nothing under the finger can slide: the press is
            // unambiguous, and the pressed paint shows at once
            self.phase = Phase::Pressing { start: at, held: 0.0, still: true };
            out.push(Gesture::Press { at, taps, held: 0.0 });
        } else {
            self.phase = Phase::Undecided { start: at, taps, held: 0.0 };
        }
        out
    }

    /// A finger moved.
    pub fn moved(&mut self, id: u64, at: Point) -> Vec<Gesture> {
        let mut out = Vec::new();
        let Some(index) = self.fingers.iter().position(|finger| finger.id == id) else {
            return out;
        };
        let was = self.fingers[index].at;
        self.fingers[index].at = at;
        if let Phase::Pinching { span } = self.phase {
            let now = self.spread();
            let scale = now / span;
            self.phase = Phase::Pinching { span: now };
            if scale != 1.0 {
                out.push(Gesture::Magnify { at: self.centroid(), scale });
            }
            return out;
        }
        if index != 0 {
            return out;
        }
        self.pending.0 += at.x - was.x;
        self.pending.1 += at.y - was.y;
        match self.phase {
            Phase::Pressing { start, held, still } => {
                // a press that travels is a drag, and a drag is never a
                // long press
                let still = still && (at.x - start.x).hypot(at.y - start.y) <= SLOP;
                self.phase = Phase::Pressing { start, held, still };
                out.push(Gesture::Move { at });
            }
            Phase::Panning { start } => {
                out.push(Gesture::Scroll { anchor: start, dx: at.x - was.x, dy: at.y - was.y });
            }
            Phase::Undecided { start, .. } => {
                if (at.x - start.x).hypot(at.y - start.y) > SLOP {
                    // the whole excursion, so the content never jumps
                    // by the slop it waited through
                    self.phase = Phase::Panning { start };
                    out.push(Gesture::Scroll {
                        anchor: start,
                        dx: at.x - start.x,
                        dy: at.y - start.y,
                    });
                }
            }
            Phase::Idle | Phase::Swallowed | Phase::Flinging { .. } | Phase::Pinching { .. } => {}
        }
        out
    }

    /// A finger lifted.
    pub fn ended(&mut self, id: u64, at: Point) -> Vec<Gesture> {
        let mut out = Vec::new();
        let Some(index) = self.fingers.iter().position(|finger| finger.id == id) else {
            return out;
        };
        self.fingers.remove(index);
        if let Phase::Pinching { .. } = self.phase {
            return self.after_pinch_lift();
        }
        if index != 0 {
            return out;
        }
        match self.phase {
            Phase::Pressing { .. } => out.push(Gesture::Release { at }),
            Phase::Undecided { start, taps, held } => {
                // a still finger: the press it waited with, then the lift —
                // and the wait travels with it, because a box that answers a
                // hold differently has only this to tell it from a tap
                out.push(Gesture::Press { at: start, taps, held });
                out.push(Gesture::Release { at });
            }
            Phase::Panning { start } => {
                let velocity = self.velocity();
                if velocity.0.hypot(velocity.1) >= FLING_START {
                    self.phase = Phase::Flinging { anchor: start, velocity };
                    return out;
                }
            }
            Phase::Idle | Phase::Swallowed | Phase::Flinging { .. } | Phase::Pinching { .. } => {}
        }
        self.phase = Phase::Idle;
        self.samples.clear();
        self.pending = (0.0, 0.0);
        out
    }

    /// The system took the touch (a gesture of its own, a call). A press
    /// in flight is cancelled; a pan that never lifted never flings.
    pub fn cancelled(&mut self, id: u64) -> Vec<Gesture> {
        let mut out = Vec::new();
        let Some(index) = self.fingers.iter().position(|finger| finger.id == id) else {
            return out;
        };
        self.fingers.remove(index);
        if let Phase::Pinching { .. } = self.phase {
            return self.after_pinch_lift();
        }
        if index != 0 {
            return out;
        }
        if let Phase::Pressing { .. } = self.phase {
            out.push(Gesture::Cancel);
        }
        self.phase = Phase::Idle;
        self.samples.clear();
        self.pending = (0.0, 0.0);
        out
    }

    /// The frame clock: ages a hold into a menu or a press, samples the
    /// velocity of a pan, and steps a fling.
    pub fn tick(&mut self, dt: f64, scene: &dyn TouchScene) -> Vec<Gesture> {
        let mut out = Vec::new();
        match self.phase {
            Phase::Undecided { start, taps, held } => {
                let held = held + dt;
                if held >= LONG_PRESS {
                    if scene.menu_at(start) {
                        // the rest of this touch is spent: a lift after a
                        // menu opened must not close it as a tap
                        self.phase = Phase::Swallowed;
                        out.push(Gesture::Menu { at: start });
                    } else {
                        // mouse mode: the following drag sweeps or drags;
                        // the hold is spent, so no menu asks again
                        self.phase = Phase::Pressing { start, held, still: false };
                        out.push(Gesture::Press { at: start, taps: taps.max(1), held });
                    }
                } else {
                    self.phase = Phase::Undecided { start, taps, held };
                }
            }
            Phase::Panning { .. } => {
                let (dx, dy) = std::mem::take(&mut self.pending);
                self.samples.push(Sample { dt, dx, dy });
                // the window is short; the ring stays short
                if self.samples.len() > 16 {
                    self.samples.remove(0);
                }
            }
            Phase::Flinging { anchor, velocity } => {
                // v(t) = v₀·f^t, so the distance over dt is the integral
                // v₀·(1 − f^dt)/λ — the same fling travels the same
                // distance whatever the frame rate
                let lambda = -FLING_DECAY_PER_MS.ln() * 1000.0;
                let factor = FLING_DECAY_PER_MS.powf(dt * 1000.0);
                let travelled = (1.0 - factor) / lambda;
                out.push(Gesture::Scroll {
                    anchor,
                    dx: velocity.0 * travelled,
                    dy: velocity.1 * travelled,
                });
                let velocity = (velocity.0 * factor, velocity.1 * factor);
                if velocity.0.hypot(velocity.1) < FLING_STOP {
                    self.phase = Phase::Idle;
                } else {
                    self.phase = Phase::Flinging { anchor, velocity };
                }
            }
            Phase::Pressing { start, held, still: true } => {
                // a press held still over a menu is taken back for the
                // menu: the row was pressed, and the hold is the second
                // click. Nothing under it has fired — a button fires on
                // the lift — so the cancel costs no one anything
                let held = held + dt;
                if held < LONG_PRESS {
                    self.phase = Phase::Pressing { start, held, still: true };
                } else if scene.menu_at(start) {
                    self.phase = Phase::Swallowed;
                    out.push(Gesture::Cancel);
                    out.push(Gesture::Menu { at: start });
                } else {
                    // no menu here: the hold is spent and the clock rests
                    self.phase = Phase::Pressing { start, held, still: false };
                }
            }
            Phase::Idle
            | Phase::Pressing { still: false, .. }
            | Phase::Swallowed
            | Phase::Pinching { .. } => {}
        }
        out
    }

    /// The content hit its edge: the fling dies here.
    pub fn stop_fling(&mut self) {
        if let Phase::Flinging { .. } = self.phase {
            self.phase = Phase::Idle;
        }
    }

    /// Does the recognizer need the clock? A hold that may become a
    /// menu, a pan whose speed is being read, a fling in flight.
    pub fn alive(&self) -> bool {
        match self.phase {
            Phase::Undecided { .. } | Phase::Panning { .. } | Phase::Flinging { .. } => true,
            // a still press listens for the menu until the hold is spent
            Phase::Pressing { held, still: true, .. } => held < LONG_PRESS,
            Phase::Idle | Phase::Pressing { .. } | Phase::Swallowed | Phase::Pinching { .. } => false,
        }
    }

    /// The pan's speed at the lift, in points per second, read from the
    /// frames just behind it.
    fn velocity(&self) -> (Px, Px) {
        let (mut dt, mut dx, mut dy) = (0.0, 0.0, 0.0);
        for sample in self.samples.iter().rev() {
            if dt >= VELOCITY_WINDOW {
                break;
            }
            dt += sample.dt;
            dx += sample.dx;
            dy += sample.dy;
        }
        if dt <= 0.0 {
            return (0.0, 0.0);
        }
        (dx / dt, dy / dt)
    }

    /// After a finger leaves a pinch: two or more left re-anchor the
    /// span (no jump); one left is a fresh touch of its own, undecided
    /// where it stands; none is rest.
    fn after_pinch_lift(&mut self) -> Vec<Gesture> {
        match self.fingers.len() {
            0 => self.phase = Phase::Idle,
            1 => {
                let start = self.fingers[0].at;
                self.pending = (0.0, 0.0);
                self.samples.clear();
                self.phase = Phase::Undecided { start, taps: 1, held: 0.0 };
            }
            _ => self.phase = Phase::Pinching { span: self.spread() },
        }
        Vec::new()
    }

    /// The point between the fingers — symmetric in the set, because
    /// the platform hands the set in no order.
    fn centroid(&self) -> Point {
        let n = self.fingers.len().max(1) as f64;
        let (x, y) = self
            .fingers
            .iter()
            .fold((0.0, 0.0), |(x, y), finger| (x + finger.at.x, y + finger.at.y));
        Point { x: x / n, y: y / n }
    }

    /// Twice the mean distance to the centroid — the span two fingers
    /// hold, held at the floor a zoom can be read from.
    fn spread(&self) -> Px {
        let center = self.centroid();
        let n = self.fingers.len().max(1) as f64;
        let mean = self
            .fingers
            .iter()
            .map(|finger| (finger.at.x - center.x).hypot(finger.at.y - center.y))
            .sum::<f64>()
            / n;
        (mean * 2.0).max(PINCH_MIN_SPAN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene that answers by rectangles: what pans, what grabs, what
    /// has a menu.
    struct Fake {
        pans: bool,
        grabs: bool,
        menu: bool,
    }

    impl TouchScene for Fake {
        fn pans_at(&self, _at: Point) -> bool {
            self.pans
        }
        fn grabs_at(&self, _at: Point) -> bool {
            self.grabs
        }
        fn menu_at(&self, _at: Point) -> bool {
            self.menu
        }
    }

    const LIST: Fake = Fake { pans: true, grabs: false, menu: false };
    const FLAT: Fake = Fake { pans: false, grabs: false, menu: false };

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn a_still_finger_lifts_as_a_tap() {
        let mut touch = Recognizer::new();
        assert!(touch.began(1, p(10.0, 10.0), 1, &LIST).is_empty(), "the press waits");
        assert!(touch.moved(1, p(12.0, 11.0)).is_empty(), "inside the slop nothing is said");
        let out = touch.ended(1, p(12.0, 11.0));
        assert_eq!(
            out,
            vec![Gesture::Press { at: p(10.0, 10.0), taps: 1, held: 0.0 }, Gesture::Release { at: p(12.0, 11.0) }]
        );
        assert!(!touch.alive());
    }

    #[test]
    fn slop_exit_over_a_pannable_surface_pans_with_the_whole_excursion() {
        let mut touch = Recognizer::new();
        touch.began(1, p(10.0, 100.0), 1, &LIST);
        assert!(touch.moved(1, p(10.0, 95.0)).is_empty());
        let out = touch.moved(1, p(10.0, 80.0));
        assert_eq!(out, vec![Gesture::Scroll { anchor: p(10.0, 100.0), dx: 0.0, dy: -20.0 }]);
        let out = touch.moved(1, p(10.0, 70.0));
        assert_eq!(out, vec![Gesture::Scroll { anchor: p(10.0, 100.0), dx: 0.0, dy: -10.0 }]);
        assert!(touch.ended(1, p(10.0, 70.0)).is_empty(), "a lift with no speed fires nothing");
        assert!(!touch.alive());
    }

    #[test]
    fn a_hold_on_a_flat_menu_row_takes_the_press_back_for_the_menu() {
        let mut touch = Recognizer::new();
        let row = Fake { pans: false, grabs: false, menu: true };
        assert_eq!(touch.began(1, p(10.0, 10.0), 1, &row), vec![Gesture::Press { at: p(10.0, 10.0), taps: 1, held: 0.0 }]);
        assert!(touch.alive(), "a still press listens for the menu");
        assert!(touch.tick(0.3, &row).is_empty());
        assert_eq!(touch.tick(0.3, &row), vec![Gesture::Cancel, Gesture::Menu { at: p(10.0, 10.0) }]);
        assert!(touch.ended(1, p(10.0, 10.0)).is_empty(), "the lift after a menu is spent");

        // a flat surface with no menu: the hold spends itself and the
        // clock rests, the press stands
        let mut touch = Recognizer::new();
        touch.began(1, p(10.0, 10.0), 1, &FLAT);
        touch.tick(0.3, &FLAT);
        assert!(touch.tick(0.3, &FLAT).is_empty());
        assert!(!touch.alive(), "nothing left to decide on the clock");
        assert_eq!(touch.ended(1, p(10.0, 10.0)), vec![Gesture::Release { at: p(10.0, 10.0) }]);

        // a press that travelled is a drag: no menu, however long
        let mut touch = Recognizer::new();
        touch.began(1, p(10.0, 10.0), 1, &row);
        touch.moved(1, p(40.0, 10.0));
        touch.tick(0.6, &row);
        assert_eq!(touch.ended(1, p(40.0, 10.0)), vec![Gesture::Release { at: p(40.0, 10.0) }]);
    }

    #[test]
    fn a_surface_that_cannot_pan_presses_at_once() {
        let mut touch = Recognizer::new();
        let out = touch.began(1, p(5.0, 5.0), 2, &FLAT);
        assert_eq!(out, vec![Gesture::Press { at: p(5.0, 5.0), taps: 2, held: 0.0 }]);
        assert_eq!(touch.moved(1, p(30.0, 30.0)), vec![Gesture::Move { at: p(30.0, 30.0) }]);
        assert_eq!(touch.ended(1, p(30.0, 30.0)), vec![Gesture::Release { at: p(30.0, 30.0) }]);
    }

    #[test]
    fn a_grabbing_box_presses_at_once_inside_a_pannable_surface() {
        let mut touch = Recognizer::new();
        let canvas = Fake { pans: true, grabs: true, menu: false };
        let out = touch.began(1, p(5.0, 5.0), 1, &canvas);
        assert_eq!(out, vec![Gesture::Press { at: p(5.0, 5.0), taps: 1, held: 0.0 }]);
    }

    #[test]
    fn half_a_second_of_stillness_is_a_menu_or_a_press() {
        let mut touch = Recognizer::new();
        let rows = Fake { pans: true, grabs: false, menu: true };
        touch.began(1, p(10.0, 10.0), 1, &rows);
        assert!(touch.tick(0.3, &rows).is_empty());
        assert_eq!(touch.tick(0.3, &rows), vec![Gesture::Menu { at: p(10.0, 10.0) }]);
        assert!(touch.ended(1, p(10.0, 10.0)).is_empty(), "the lift after a menu is spent");

        let mut touch = Recognizer::new();
        touch.began(1, p(10.0, 10.0), 1, &LIST);
        touch.tick(0.3, &LIST);
        assert_eq!(touch.tick(0.3, &LIST), vec![Gesture::Press { at: p(10.0, 10.0), taps: 1, held: 0.6 }]);
        assert_eq!(touch.moved(1, p(40.0, 10.0)), vec![Gesture::Move { at: p(40.0, 10.0) }]);
    }

    /// Runs a pan of `steps` moves of `dy` each at the given frame rate,
    /// lifts, and returns the fling's total travel.
    fn fling_travel(hz: f64, steps: usize, dy: f64) -> f64 {
        let dt = 1.0 / hz;
        let mut touch = Recognizer::new();
        touch.began(1, p(10.0, 500.0), 1, &LIST);
        let mut y = 500.0;
        let mut travel = 0.0;
        for _ in 0..steps {
            y -= dy;
            for gesture in touch.moved(1, p(10.0, y)) {
                if let Gesture::Scroll { dy, .. } = gesture {
                    travel += dy;
                }
            }
            touch.tick(dt, &LIST);
        }
        touch.ended(1, p(10.0, y));
        assert!(touch.alive(), "a fast lift flings");
        let mut ticks = 0;
        while touch.alive() {
            for gesture in touch.tick(dt, &LIST) {
                if let Gesture::Scroll { dy, .. } = gesture {
                    travel += dy;
                }
            }
            ticks += 1;
            assert!(ticks < 10_000, "a fling comes to rest");
        }
        travel
    }

    #[test]
    fn the_fling_decay_is_framerate_independent() {
        // the same speed (1200 pt/s) at both rates: 60 Hz moves 20 a
        // frame, 120 Hz moves 10
        let slow = fling_travel(60.0, 6, 20.0);
        let fast = fling_travel(120.0, 12, 10.0);
        assert!(slow < -700.0, "the fling travels far past the lift: {slow}");
        assert!((slow - fast).abs() < 1.0, "60 Hz {slow} vs 120 Hz {fast}");
    }

    #[test]
    fn a_stopped_fling_is_idle() {
        let mut touch = Recognizer::new();
        touch.began(1, p(10.0, 500.0), 1, &LIST);
        for step in 1..=6 {
            touch.moved(1, p(10.0, 500.0 - 20.0 * step as f64));
            touch.tick(1.0 / 60.0, &LIST);
        }
        touch.ended(1, p(10.0, 380.0));
        assert!(touch.alive());
        touch.stop_fling();
        assert!(!touch.alive());
        assert!(touch.tick(1.0 / 60.0, &LIST).is_empty());
    }

    #[test]
    fn a_second_finger_turns_a_press_into_a_zoom_and_cancels_it() {
        let mut touch = Recognizer::new();
        assert_eq!(touch.began(1, p(100.0, 100.0), 1, &FLAT), vec![Gesture::Press { at: p(100.0, 100.0), taps: 1, held: 0.0 }]);
        assert_eq!(touch.began(2, p(200.0, 100.0), 1, &FLAT), vec![Gesture::Cancel]);
        // the fingers spread: 100 apart → 200 apart is a scale of 2 at
        // the point between them
        let out = touch.moved(2, p(300.0, 100.0));
        assert_eq!(out, vec![Gesture::Magnify { at: p(200.0, 100.0), scale: 2.0 }]);
        // the next step is read from the NEW span
        let out = touch.moved(1, p(200.0, 100.0));
        assert_eq!(out, vec![Gesture::Magnify { at: p(250.0, 100.0), scale: 0.5 }]);
    }

    #[test]
    fn lifting_one_finger_of_two_hands_the_touch_to_the_other() {
        let mut touch = Recognizer::new();
        touch.began(1, p(100.0, 100.0), 1, &LIST);
        touch.began(2, p(200.0, 100.0), 1, &LIST);
        assert!(touch.ended(2, p(200.0, 100.0)).is_empty());
        // the remaining finger starts over where it stands: a drag
        // from here pans from here, and a still lift is a tap here
        let out = touch.moved(1, p(100.0, 60.0));
        assert_eq!(out, vec![Gesture::Scroll { anchor: p(100.0, 100.0), dx: 0.0, dy: -40.0 }]);
    }

    #[test]
    fn a_third_finger_re_anchors_the_span_instead_of_jumping() {
        let mut touch = Recognizer::new();
        touch.began(1, p(100.0, 100.0), 1, &FLAT);
        touch.began(2, p(200.0, 100.0), 1, &FLAT);
        assert!(touch.began(3, p(150.0, 300.0), 1, &FLAT).is_empty(), "a new finger says nothing");
        // a still hand after the third landed reads as no zoom at all
        assert!(touch.moved(3, p(150.0, 300.0)).is_empty());
    }

    #[test]
    fn a_cancelled_press_says_cancel_and_a_cancelled_pan_never_flings() {
        let mut touch = Recognizer::new();
        touch.began(1, p(5.0, 5.0), 1, &FLAT);
        assert_eq!(touch.cancelled(1), vec![Gesture::Cancel]);

        let mut touch = Recognizer::new();
        touch.began(1, p(10.0, 500.0), 1, &LIST);
        for step in 1..=6 {
            touch.moved(1, p(10.0, 500.0 - 20.0 * step as f64));
            touch.tick(1.0 / 60.0, &LIST);
        }
        assert!(touch.cancelled(1).is_empty());
        assert!(!touch.alive(), "a cancelled pan has no fling");
    }
}
