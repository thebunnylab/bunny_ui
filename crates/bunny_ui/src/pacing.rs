//! Frame pacing: WHEN a shell draws the frame an event asked for.
//!
//! An event changes the scene, and the scene needs a frame. A shell that
//! draws that frame inside every event handler draws as many frames as it
//! hears events — and a wheel speaks faster than a display shows: ninety
//! to a hundred and twenty events a second, then a tail of momentum. Each
//! frame is a whole settle, layout and present, the present can wait for
//! the display, and the handler waits with it: the event queue backs up
//! and every frame drawn is older than the hand. That reads as stutter at
//! any frame rate.
//!
//! The pacer decides, and it holds no clock — it counts display BEATS,
//! which the shell reports:
//!
//! - **Cold** (the display beat is not running): an event that can wait
//!   ([`Urgency::Soon`]) draws AT ONCE. One event after a rest must not
//!   wait for a link to wake up. The draw starts a WARM period and the
//!   shell starts the beat.
//! - **Warm** (the beat runs): an event that can wait only leaves its
//!   name. The next beat draws ONE frame for every event since the last —
//!   a burst folds to one present for each display refresh, at a regular
//!   cadence, and nothing blocks inside a handler.
//! - After [`FramePacer::QUIET_BEATS`] beats with nothing to draw the warm
//!   period ends, and the shell lets the beat stop. At rest nothing runs.
//! - An event that cannot wait ([`Urgency::Now`]) always draws at once: a
//!   key (the input system asks for the caret's place in the same turn),
//!   a press, a redraw the window asked for. It takes whatever was pending
//!   with it, and it starts no warm period — typing never starts the beat.
//! - During a LIVE resize the resize step is the one presenter: an event
//!   that can wait is dropped on the floor (the next step shows what it
//!   changed), and a beat holds.
//! - A present that WAITED for the display ([`FramePacer::congested`]) means
//!   the line of frames in front of the display is full: a frame missed its
//!   refresh and took the next one. One frame a beat never drains that line
//!   — every present after it waits most of a beat inside its handler, and
//!   shows a refresh late. The pacer holds ONE beat, the line drains, and
//!   what waited is drawn on the beat after.

use std::cell::Cell;

/// Can the frame this event asks for wait for the next display beat?
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Urgency {
    /// No: draw before the handler returns.
    Now,
    /// Yes: fold it into the next beat, when a beat is running.
    Soon,
}

/// The pacer's answer to an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Draw a frame now, then call [`FramePacer::drew`].
    Draw,
    /// Do not draw: a beat will, or the resize step will.
    Wait,
}

/// The pacer's answer to a display beat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Beat {
    /// Events are waiting: draw ONE settled frame, then call
    /// [`FramePacer::drew`].
    Draw,
    /// Nothing is waiting. The shell serves its animations as before.
    Quiet,
    /// A live resize owns the window: present nothing.
    Hold,
}

/// Who asked for the frame that is about to be drawn: the first asker,
/// every asker (one bit for each origin), and how many asks folded into it.
/// A tape stays truthful with it when twenty wheel events became one frame.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Asked {
    pub first: u8,
    pub all: u32,
    pub count: u32,
}

/// One window's pacer. Every method takes `&self`: a shell reaches it from
/// `extern "C"` callbacks, and a borrow that panics there ends the process.
#[derive(Debug)]
pub struct FramePacer {
    /// The shell's word: is a display-rate beat running, for any reason?
    beating: Cell<bool>,
    /// Beats with nothing to draw since the last deferred draw.
    quiet: Cell<u32>,
    pending: Cell<Asked>,
    /// Beats to hold so the line of frames in front of the display drains.
    owed: Cell<u32>,
}

impl Default for FramePacer {
    fn default() -> Self {
        FramePacer {
            beating: Cell::new(false),
            // born cold: no warm period is open
            quiet: Cell::new(Self::QUIET_BEATS),
            pending: Cell::new(Asked::default()),
            owed: Cell::new(0),
        }
    }
}

impl FramePacer {
    /// Beats with nothing to draw that end a warm period. Three beats are
    /// 25 ms at 120 Hz and 50 ms at 60 Hz: a trackpad's stream stays warm
    /// through its gaps, and the notches of a mouse wheel — 30 to 100 ms
    /// apart — are each drawn at once, which is right for them.
    pub const QUIET_BEATS: u32 = 3;

    /// Frames that can wait in front of the display: the most beats a
    /// pacer ever owes.
    pub const LINE: u32 = 2;

    pub fn new() -> FramePacer {
        FramePacer::default()
    }

    fn note(&self, origin: u8) {
        let mut pending = self.pending.get();
        if pending.count == 0 {
            pending.first = origin;
        }
        pending.all |= 1u32 << (origin % 32);
        pending.count += 1;
        self.pending.set(pending);
    }

    /// An event changed the scene. `origin` names it for the tape (a small
    /// number the shell chooses); `live` says a live resize owns the window.
    pub fn ask(&self, origin: u8, urgency: Urgency, live: bool) -> Verdict {
        match urgency {
            Urgency::Now => {
                self.note(origin);
                Verdict::Draw
            }
            // the resize step is the one presenter, and it shows what this
            // event changed: nothing is left waiting behind the gesture
            Urgency::Soon if live => Verdict::Wait,
            Urgency::Soon if self.beating.get() => {
                self.note(origin);
                Verdict::Wait
            }
            Urgency::Soon => {
                self.note(origin);
                self.quiet.set(0);
                Verdict::Draw
            }
        }
    }

    /// A frame went up: the asks it carried are served. Returns them, for
    /// the tape.
    pub fn drew(&self) -> Asked {
        self.pending.replace(Asked::default())
    }

    /// The present that just went up WAITED for the display: the line of
    /// frames in front of it is full. The next beat holds, and the line
    /// drains by one. Two waits in a row owe two beats and no more — the
    /// line is never longer than that.
    pub fn congested(&self) {
        self.owed.set((self.owed.get() + 1).min(Self::LINE));
    }

    /// One display beat.
    pub fn beat(&self, live: bool) -> Beat {
        if live {
            return Beat::Hold;
        }
        if self.owed.get() > 0 {
            // a held beat is not a quiet one: what waits is still owed its
            // frame, and the warm period must not end under it
            self.owed.set(self.owed.get() - 1);
            return Beat::Hold;
        }
        if self.pending.get().count > 0 {
            self.quiet.set(0);
            return Beat::Draw;
        }
        self.quiet.set(self.quiet.get().saturating_add(1));
        Beat::Quiet
    }

    /// Is a warm period open? While it is, the shell keeps the display
    /// beat running for this window.
    pub fn warm(&self) -> bool {
        self.pending.get().count > 0 || self.owed.get() > 0 || self.quiet.get() < Self::QUIET_BEATS
    }

    /// The shell says whether a display-rate beat is running — for this
    /// window's warm period, for a spring, or for another window. While
    /// one runs, an event that can wait is never drawn cold.
    pub fn set_beating(&self, on: bool) {
        self.beating.set(on);
    }

    /// Asks that wait for a beat.
    pub fn pending(&self) -> Asked {
        self.pending.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHEEL: u8 = 2;
    const WAKE: u8 = 1;
    const KEY: u8 = 3;

    #[test]
    fn a_cold_ask_draws_at_once_and_opens_a_warm_period() {
        let pacer = FramePacer::new();
        assert!(!pacer.warm(), "born cold");
        assert_eq!(pacer.ask(WHEEL, Urgency::Soon, false), Verdict::Draw);
        assert_eq!(pacer.drew(), Asked { first: WHEEL, all: 1 << WHEEL, count: 1 });
        assert!(pacer.warm(), "the shell starts the beat now");
    }

    #[test]
    fn a_burst_folds_into_one_frame_for_each_beat() {
        let pacer = FramePacer::new();
        assert_eq!(pacer.ask(WHEEL, Urgency::Soon, false), Verdict::Draw);
        pacer.drew();
        pacer.set_beating(true);
        // twenty events between two beats: none of them draws
        for _ in 0..19 {
            assert_eq!(pacer.ask(WHEEL, Urgency::Soon, false), Verdict::Wait);
        }
        assert_eq!(pacer.ask(WAKE, Urgency::Soon, false), Verdict::Wait);
        assert_eq!(pacer.beat(false), Beat::Draw, "ONE frame for all of them");
        let asked = pacer.drew();
        assert_eq!(asked.count, 20);
        assert_eq!(asked.first, WHEEL, "the tape still says who asked first");
        assert_eq!(asked.all, (1 << WHEEL) | (1 << WAKE), "and everyone who did");
        assert_eq!(pacer.beat(false), Beat::Quiet, "nothing is left");
    }

    #[test]
    fn three_quiet_beats_end_the_warm_period() {
        let pacer = FramePacer::new();
        pacer.ask(WHEEL, Urgency::Soon, false);
        pacer.drew();
        pacer.set_beating(true);
        for beat in 1..=FramePacer::QUIET_BEATS {
            assert!(pacer.warm(), "still warm before beat {beat}");
            assert_eq!(pacer.beat(false), Beat::Quiet);
        }
        assert!(!pacer.warm(), "the shell lets the beat stop: nothing runs at rest");

        // a drawn beat is not a quiet one: a stream with gaps stays warm
        pacer.set_beating(false);
        pacer.ask(WHEEL, Urgency::Soon, false);
        pacer.drew();
        pacer.set_beating(true);
        assert_eq!(pacer.beat(false), Beat::Quiet);
        assert_eq!(pacer.beat(false), Beat::Quiet);
        pacer.ask(WHEEL, Urgency::Soon, false);
        assert_eq!(pacer.beat(false), Beat::Draw);
        pacer.drew();
        assert!(pacer.warm(), "the count started again");
    }

    #[test]
    fn an_event_that_cannot_wait_always_draws_and_takes_the_rest_with_it() {
        let pacer = FramePacer::new();
        pacer.ask(WHEEL, Urgency::Soon, false);
        pacer.drew();
        pacer.set_beating(true);
        assert_eq!(pacer.ask(WHEEL, Urgency::Soon, false), Verdict::Wait);
        assert_eq!(pacer.ask(KEY, Urgency::Now, false), Verdict::Draw, "a key never waits for a beat");
        let asked = pacer.drew();
        assert_eq!(asked.count, 2, "the wheel that waited rides the key's frame");
        assert_eq!(pacer.beat(false), Beat::Quiet, "and the beat finds nothing to draw");

        // typing never starts the beat
        let typing = FramePacer::new();
        assert_eq!(typing.ask(KEY, Urgency::Now, false), Verdict::Draw);
        typing.drew();
        assert!(!typing.warm(), "a frame that could not wait opens no warm period");
    }

    #[test]
    fn a_present_that_waited_holds_one_beat_and_loses_nothing() {
        let pacer = FramePacer::new();
        pacer.ask(WHEEL, Urgency::Soon, false);
        pacer.drew();
        pacer.set_beating(true);
        pacer.ask(WHEEL, Urgency::Soon, false);
        assert_eq!(pacer.beat(false), Beat::Draw);
        pacer.drew();
        // that present waited for the display: the line in front of it is full
        pacer.congested();
        pacer.ask(WHEEL, Urgency::Soon, false);
        assert_eq!(pacer.beat(false), Beat::Hold, "one beat is held, and the line drains");
        assert_eq!(pacer.pending().count, 1, "what waited is still owed its frame");
        assert!(pacer.warm(), "and the beat keeps running for it");
        assert_eq!(pacer.beat(false), Beat::Draw, "the beat after draws it");
        pacer.drew();

        // the line is two frames long: no pacer owes more than two beats
        for _ in 0..5 {
            pacer.congested();
        }
        assert_eq!(pacer.beat(false), Beat::Hold);
        assert_eq!(pacer.beat(false), Beat::Hold);
        assert_eq!(pacer.beat(false), Beat::Quiet);
    }

    #[test]
    fn a_live_resize_is_the_one_presenter() {
        let pacer = FramePacer::new();
        // an event that can wait is never drawn mid-gesture, cold or warm,
        // and leaves nothing behind: the next step of the resize shows it
        assert_eq!(pacer.ask(WAKE, Urgency::Soon, true), Verdict::Wait);
        assert_eq!(pacer.pending().count, 0);
        assert!(!pacer.warm());
        pacer.set_beating(true);
        assert_eq!(pacer.ask(WHEEL, Urgency::Soon, true), Verdict::Wait);
        assert_eq!(pacer.beat(true), Beat::Hold, "a beat presents nothing mid-gesture");
        // the resize step itself cannot wait
        assert_eq!(pacer.ask(KEY, Urgency::Now, true), Verdict::Draw);
    }
}
