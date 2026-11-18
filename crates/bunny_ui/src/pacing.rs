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
//!
//! A shell that knows the display's refresh grid ([`FramePacer::refresh`])
//! does better than a beat: the refresh is what counts, not the beat. A
//! warm event that can wait draws AT ONCE when the refresh its frame would
//! reach has no frame yet ([`FramePacer::ask_at`]), and that frame shows a
//! refresh sooner than the beat's would. A frame that changed the picture
//! claims its refresh ([`FramePacer::claim`]); what comes after it in the
//! same refresh waits for the beat, and a beat whose refresh is already
//! claimed holds ([`FramePacer::beat_at`]) — still one frame for each
//! refresh, and none of them a beat late.
//!
//! Some asks only FOLLOW the hand: the engine's frame after a wheel turn
//! the window already scrolled. While the hand keeps drawing — a frame it
//! led within the last refresh and a half ([`FramePacer::led`]) — such
//! asks ride its next frame instead of taking a refresh of their own; when
//! it stops, the beat after draws them.

use std::cell::Cell;
use std::time::{Duration, Instant};

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

/// The display's refreshes as the last beat told them: the one a frame drawn
/// at that beat reaches, and the time between two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grid {
    target: Instant,
    interval: Duration,
}

impl Grid {
    /// The first refresh at or after `at`.
    fn after(self, at: Instant) -> Instant {
        if at <= self.target || self.interval.is_zero() {
            return self.target;
        }
        let behind = (at - self.target).as_nanos();
        let interval = self.interval.as_nanos();
        let steps = u32::try_from(behind.div_ceil(interval)).unwrap_or(u32::MAX);
        self.target + self.interval.saturating_mul(steps)
    }

    fn same(self, a: Instant, b: Instant) -> bool {
        let apart = if a > b { a - b } else { b - a };
        apart < self.interval / 2
    }
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
    /// The refresh grid, when the shell reports it.
    grid: Cell<Option<Grid>>,
    /// The refresh the last frame that changed the picture reaches.
    claimed: Cell<Option<Instant>>,
    /// How long a frame takes, from the decision to draw to its present: the
    /// longest of the last ones, fading.
    lead: Cell<Duration>,
    /// When the last frame the hand led was presented.
    led: Cell<Option<Instant>>,
    /// How long a frame takes on the GPU after its present: the longest of
    /// the last ones, fading.
    gpu: Cell<Duration>,
}

impl Default for FramePacer {
    fn default() -> Self {
        FramePacer {
            beating: Cell::new(false),
            // born cold: no warm period is open
            quiet: Cell::new(Self::QUIET_BEATS),
            pending: Cell::new(Asked::default()),
            owed: Cell::new(0),
            grid: Cell::new(None),
            claimed: Cell::new(None),
            lead: Cell::new(Duration::ZERO),
            led: Cell::new(None),
            gpu: Cell::new(Duration::ZERO),
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

    /// What a frame needs after its present to make a refresh: the
    /// compositor's own share.
    pub const MARGIN: Duration = Duration::from_millis(1);

    /// A grid older than this says nothing of the refreshes to come: the
    /// link rested, and the display may have changed its pace.
    pub const STALE: Duration = Duration::from_millis(100);

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

    /// The shell's word on the display: at its beat, the refresh a frame
    /// drawn now reaches and the time between refreshes.
    pub fn refresh(&self, target: Instant, interval: Duration) {
        self.grid.set(Some(Grid { target, interval }));
    }

    /// How long the frame just drawn took, from the decision to its present.
    pub fn spent(&self, took: Duration) {
        self.lead.set(fade(self.lead.get(), took));
    }

    /// How long a recent frame took on the GPU: a frame reaches its refresh
    /// only once that is done too.
    pub fn gpu(&self, took: Duration) {
        self.gpu.set(fade(self.gpu.get(), took));
    }

    /// The grid, while it still says something about `now`.
    fn grid_at(&self, now: Instant) -> Option<Grid> {
        self.grid
            .get()
            .filter(|grid| now <= grid.target + Self::STALE)
    }

    /// The refresh a frame decided on at `now` reaches.
    fn aim(&self, now: Instant) -> Option<Instant> {
        let grid = self.grid_at(now)?;
        Some(grid.after(now + self.lead.get() + self.gpu.get() + Self::MARGIN))
    }

    /// Has the refresh a frame decided on at `now` no frame yet?
    fn open(&self, now: Instant) -> bool {
        let (Some(grid), Some(aim)) = (self.grid_at(now), self.aim(now)) else {
            return true;
        };
        !self
            .claimed
            .get()
            .is_some_and(|claimed| grid.same(claimed, aim))
    }

    /// The frame just presented, at `presented`, changed the picture: the
    /// refresh it reaches is taken. A frame that changed nothing claims
    /// nothing — the change that follows it may still have that refresh.
    pub fn claim(&self, presented: Instant) {
        if let Some(grid) = self.grid_at(presented) {
            self.claimed
                .set(Some(grid.after(presented + self.gpu.get() + Self::MARGIN)));
        }
    }

    /// [`FramePacer::ask`] for a shell that reports the refresh grid: a warm
    /// event that can wait draws at once when the refresh its frame reaches
    /// is still open, and waits for the beat when a frame already took it.
    pub fn ask_at(&self, origin: u8, urgency: Urgency, live: bool, now: Instant) -> Verdict {
        match urgency {
            Urgency::Soon if !live && self.beating.get() => {
                self.note(origin);
                if self.owed.get() == 0 && self.open(now) {
                    self.quiet.set(0);
                    Verdict::Draw
                } else {
                    Verdict::Wait
                }
            }
            _ => self.ask(origin, urgency, live),
        }
    }

    /// The frame just presented, at `presented`, was led by the hand: a
    /// wheel turn, a drag.
    pub fn led(&self, presented: Instant) {
        self.led.set(Some(presented));
    }

    /// Is the hand still drawing at `now`: did it lead a frame within the
    /// last refresh and a half? What only follows it then waits for its
    /// next frame ([`FramePacer::ask`] folds it into the beat).
    pub fn hand_drawing(&self, now: Instant) -> bool {
        let (Some(grid), Some(led)) = (self.grid_at(now), self.led.get()) else {
            return false;
        };
        now.saturating_duration_since(led) < grid.interval.mul_f64(1.5)
    }

    /// [`FramePacer::beat`] for a shell that reports the refresh grid: what
    /// waits is drawn unless its refresh already has a frame, and then the
    /// next beat draws it. `followers` says every ask that waits only
    /// follows the hand: while the hand keeps drawing, they ride its next
    /// frame.
    pub fn beat_at(&self, live: bool, now: Instant, followers: bool) -> Beat {
        if !live
            && self.owed.get() == 0
            && self.pending.get().count > 0
            && (!self.open(now) || (followers && self.hand_drawing(now)))
        {
            return Beat::Hold;
        }
        self.beat(live)
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

    /// A discrete event — a release, a press, a key — ended the gesture:
    /// the warm period closes now. The quiet beats exist for a STREAM
    /// with gaps (a trackpad's pauses, a wheel's notches), where the
    /// next event is coming and must not be drawn cold; after a click
    /// nothing is coming, and three beats with nothing to draw would be
    /// three empty frames the display still has to commit.
    pub fn rest(&self) {
        self.quiet.set(Self::QUIET_BEATS);
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

/// The longest of the recent costs: a new one counts at once, an old one
/// fades by an eighth a frame.
fn fade(kept: Duration, took: Duration) -> Duration {
    took.max(kept.mul_f64(0.875)).min(Duration::from_millis(50))
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
    fn a_rest_ends_the_warm_period_at_once() {
        let pacer = FramePacer::new();
        assert_eq!(pacer.ask(WHEEL, Urgency::Soon, false), Verdict::Draw);
        pacer.drew();
        assert!(pacer.warm(), "a drawn beat opens a warm period");
        pacer.rest();
        assert!(!pacer.warm(), "the click's release closes it: no empty beats follow");
        // an ask that still waits keeps its beat, rest or not
        pacer.set_beating(true);
        assert_eq!(pacer.ask(WHEEL, Urgency::Soon, false), Verdict::Wait);
        pacer.rest();
        assert!(pacer.warm(), "what waits is still owed its frame");
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

    const REFRESH: Duration = Duration::from_micros(8333);

    fn ms(at: Instant, ms: f64) -> Instant {
        at + Duration::from_secs_f64(ms / 1000.0)
    }

    /// A warm pacer on a 120 Hz grid: the beat at `vsync` aims at the next
    /// refresh, and frames take half a millisecond.
    fn warm_on_a_grid(vsync: Instant) -> FramePacer {
        let pacer = FramePacer::new();
        assert_eq!(pacer.ask(WHEEL, Urgency::Soon, false), Verdict::Draw);
        pacer.drew();
        pacer.set_beating(true);
        pacer.spent(Duration::from_micros(500));
        pacer.refresh(vsync + REFRESH, REFRESH);
        assert_eq!(pacer.beat_at(false, vsync, false), Beat::Quiet);
        pacer
    }

    #[test]
    fn a_warm_event_draws_at_once_while_its_refresh_is_open() {
        let vsync = Instant::now();
        let pacer = warm_on_a_grid(vsync);
        let now = ms(vsync, 2.0);
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, now),
            Verdict::Draw,
            "no frame reaches the next refresh yet: this one does, a beat early"
        );
        pacer.drew();
        pacer.claim(ms(vsync, 2.6));
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 5.0)),
            Verdict::Wait,
            "that refresh has its frame: the next event waits"
        );
        assert_eq!(
            pacer.beat_at(false, ms(vsync, 8.4), false),
            Beat::Draw,
            "the beat after draws it, for the refresh after"
        );
        pacer.drew();
        pacer.claim(ms(vsync, 9.0));
        assert!(pacer.warm());
    }

    #[test]
    fn a_beat_holds_when_its_refresh_already_has_a_frame() {
        let vsync = Instant::now();
        let pacer = warm_on_a_grid(vsync);
        pacer.spent(Duration::from_millis(3));
        // late in the refresh: the frame reaches the one after
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 7.0)),
            Verdict::Draw
        );
        pacer.drew();
        pacer.claim(ms(vsync, 10.0));
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 10.5)),
            Verdict::Wait
        );
        pacer.refresh(ms(vsync, 16.67), REFRESH);
        assert_eq!(
            pacer.beat_at(false, ms(vsync, 8.4), false),
            Beat::Hold,
            "a second frame for the same refresh is a frame nobody sees"
        );
        assert!(pacer.warm(), "what waits keeps the beat running");
        pacer.refresh(ms(vsync, 25.0), REFRESH);
        assert_eq!(pacer.beat_at(false, ms(vsync, 16.75), false), Beat::Draw);
    }

    #[test]
    fn a_frame_that_changed_nothing_leaves_its_refresh_open() {
        let vsync = Instant::now();
        let pacer = warm_on_a_grid(vsync);
        assert_eq!(
            pacer.ask_at(WAKE, Urgency::Soon, false, ms(vsync, 1.0)),
            Verdict::Draw
        );
        pacer.drew();
        // the shell claims nothing: the picture did not change
        assert_eq!(
            pacer.ask_at(WAKE, Urgency::Soon, false, ms(vsync, 1.5)),
            Verdict::Draw,
            "the change that follows still has the refresh"
        );
    }

    #[test]
    fn without_a_fresh_grid_a_warm_event_waits_for_the_beat_as_before() {
        let vsync = Instant::now();
        let pacer = warm_on_a_grid(vsync);
        pacer.drew();
        pacer.claim(ms(vsync, 1.0));
        let later = ms(vsync, 300.0);
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, later),
            Verdict::Draw,
            "a stale grid claims nothing"
        );
        let plain = FramePacer::new();
        plain.ask(WHEEL, Urgency::Soon, false);
        plain.drew();
        plain.set_beating(true);
        assert_eq!(
            plain.ask_at(WHEEL, Urgency::Soon, false, vsync),
            Verdict::Draw,
            "no grid reported: nothing is known to be taken"
        );
    }

    #[test]
    fn a_line_that_drains_holds_events_that_could_draw() {
        let vsync = Instant::now();
        let pacer = warm_on_a_grid(vsync);
        pacer.congested();
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 1.0)),
            Verdict::Wait
        );
        assert_eq!(pacer.beat_at(false, ms(vsync, 8.4), false), Beat::Hold);
        assert_eq!(pacer.beat_at(false, ms(vsync, 16.7), false), Beat::Draw);
    }

    #[test]
    fn what_follows_the_hand_rides_its_next_frame_until_it_stops() {
        let vsync = Instant::now();
        let pacer = warm_on_a_grid(vsync);
        // a wheel turn draws at once and leads its frame
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 1.0)),
            Verdict::Draw
        );
        pacer.drew();
        pacer.claim(ms(vsync, 1.6));
        pacer.led(ms(vsync, 1.6));
        // the engine's frame for the same turn follows it
        assert_eq!(
            pacer.ask_at(WAKE, Urgency::Soon, false, ms(vsync, 2.0)),
            Verdict::Wait
        );
        pacer.refresh(ms(vsync, 16.67), REFRESH);
        assert_eq!(
            pacer.beat_at(false, ms(vsync, 8.4), true),
            Beat::Hold,
            "the hand is still drawing: the next turn carries it"
        );
        assert!(pacer.warm());
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 12.0)),
            Verdict::Draw,
            "and it does"
        );
        assert_eq!(pacer.drew().count, 2);
        pacer.claim(ms(vsync, 12.6));
        pacer.led(ms(vsync, 12.6));
        assert_eq!(
            pacer.ask_at(WAKE, Urgency::Soon, false, ms(vsync, 13.0)),
            Verdict::Wait
        );
        pacer.refresh(ms(vsync, 25.0), REFRESH);
        assert_eq!(pacer.beat_at(false, ms(vsync, 16.75), true), Beat::Hold);
        pacer.refresh(ms(vsync, 33.33), REFRESH);
        assert_eq!(
            pacer.beat_at(false, ms(vsync, 25.1), true),
            Beat::Draw,
            "the hand stopped: the beat draws what followed it"
        );
        // an ask of its own never waits for the hand
        pacer.drew();
        pacer.ask_at(WAKE, Urgency::Soon, false, ms(vsync, 25.2));
        pacer.led(ms(vsync, 25.0));
        pacer.claim(ms(vsync, 25.0));
        pacer.refresh(ms(vsync, 41.67), REFRESH);
        assert_eq!(pacer.beat_at(false, ms(vsync, 33.4), false), Beat::Draw);
    }

    #[test]
    fn a_frame_the_gpu_is_still_drawing_reaches_a_later_refresh() {
        let vsync = Instant::now();
        let pacer = warm_on_a_grid(vsync);
        pacer.gpu(Duration::from_millis(6));
        // presented early, but done on the GPU only after the next refresh
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 1.0)),
            Verdict::Draw
        );
        pacer.drew();
        pacer.claim(ms(vsync, 3.0));
        pacer.refresh(ms(vsync, 16.67), REFRESH);
        assert_eq!(
            pacer.ask_at(WHEEL, Urgency::Soon, false, ms(vsync, 8.5)),
            Verdict::Wait,
            "the refresh after is that frame's"
        );
        assert_eq!(pacer.beat_at(false, ms(vsync, 8.5), false), Beat::Hold);
    }

    #[test]
    fn the_grid_steps_to_the_first_refresh_after_a_moment() {
        let target = Instant::now();
        let grid = Grid {
            target,
            interval: REFRESH,
        };
        assert_eq!(grid.after(target - REFRESH), target);
        assert_eq!(grid.after(target), target);
        assert_eq!(grid.after(ms(target, 0.1)), target + REFRESH);
        assert_eq!(grid.after(ms(target, 17.0)), target + REFRESH * 3);
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
