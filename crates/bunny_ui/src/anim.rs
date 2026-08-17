//! Springs — the animation math and the retained animator.
//!
//! One law rules this module: an animation interpolates SEMANTIC values
//! (a color today; origins and offsets next) on their way into the
//! display list. The scene tree and the retention never hold an
//! animated value — the place phase asks the animator for "the value to
//! paint now", the tick advances the physics, and a settled track SNAPS
//! bit-exact to its target, so a finished animation repaints
//! byte-for-byte what a never-animated frame paints (the identical-frame
//! skip and the incremental oracle both depend on that equality).
//!
//! The animator lives in the `Runtime` and rides into layout through the
//! env, like the frame stamp. Keys are identity paths captured at RENDER
//! time (the cursor is gone by place time). An entry the place stopped
//! touching belongs to an unmounted node and dies on the next tick.

use std::rc::Rc;

use crate::layout::Color;
use motor::hash::FxHashMap;

/// A spring described the way a designer thinks: how fast it closes in
/// and how much it bounces. Damping 1 never overshoots; below 1 it does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spring {
    /// The response period, in seconds — smaller is faster.
    pub response: f64,
    /// The damping fraction — 1 is critical, below 1 bounces.
    pub damping: f64,
}

impl Spring {
    /// No bounce, gentle pace — the default feel.
    pub fn smooth() -> Self {
        Spring { response: 0.4, damping: 1.0 }
    }

    /// Quick, with a small bounce — controls and selections.
    pub fn snappy() -> Self {
        Spring { response: 0.3, damping: 0.85 }
    }

    /// A visible bounce — playful emphasis.
    pub fn bouncy() -> Self {
        Spring { response: 0.5, damping: 0.7 }
    }
}

/// Inside this window a track stops and snaps EXACTLY onto its target.
/// Units are logical px / color channels, so 0.05 is invisible.
const SETTLE_VALUE: f64 = 0.05;
const SETTLE_VELOCITY: f64 = 0.5;

/// One animated scalar: where it is, how fast it moves, where it goes.
#[derive(Clone, Copy, Debug)]
struct Track {
    value: f64,
    velocity: f64,
    target: f64,
}

impl Track {
    fn at(target: f64) -> Self {
        Track { value: target, velocity: 0.0, target }
    }

    fn active(&self) -> bool {
        self.value != self.target || self.velocity != 0.0
    }

    /// Advances by the analytic solution of the damped oscillator —
    /// exact for a constant target within the step, stable at any `dt`.
    fn step(&mut self, spec: Spring, dt: f64) {
        if !self.active() || dt <= 0.0 {
            return;
        }
        let omega = std::f64::consts::TAU / spec.response.max(1e-3);
        let zeta = spec.damping.max(0.0);
        let x = self.value - self.target;
        let v = self.velocity;
        let (displacement, velocity) = if zeta >= 1.0 {
            // critical damping: x(t) = (a + b·t)·e^(−ω·t)
            let a = x;
            let b = v + omega * x;
            let decay = (-omega * dt).exp();
            ((a + b * dt) * decay, (b - omega * (a + b * dt)) * decay)
        } else {
            // underdamped: a decaying sine around the target
            let damped = omega * (1.0 - zeta * zeta).sqrt();
            let decay = (-zeta * omega * dt).exp();
            let a = x;
            let b = (v + zeta * omega * x) / damped;
            let (sin, cos) = (damped * dt).sin_cos();
            (
                decay * (a * cos + b * sin),
                decay * (damped * (b * cos - a * sin) - zeta * omega * (a * cos + b * sin)),
            )
        };
        if displacement.abs() < SETTLE_VALUE && velocity.abs() < SETTLE_VELOCITY {
            // the snap: a finished animation IS the plain value
            self.value = self.target;
            self.velocity = 0.0;
        } else {
            self.value = self.target + displacement;
            self.velocity = velocity;
        }
    }

    /// A new destination mid-flight keeps position AND velocity — the
    /// motion bends instead of restarting.
    fn retarget(&mut self, target: f64) {
        self.target = target;
    }
}

/// The color slots one identity can animate — indices into the entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Channel {
    Background = 0,
    Foreground = 1,
    Border = 2,
    Shadow = 3,
}

/// The animated channels of one identity: the four color slots and the
/// origin (a visual translation, anchored to the enclosing scroll box).
struct NodeAnim {
    spec: Spring,
    colors: [Option<[Track; 4]>; 4],
    origin: Option<[Track; 2]>,
    /// The tick generation that last touched this entry at place —
    /// stops advancing when the node unmounts.
    touched: u64,
}

impl NodeAnim {
    fn fresh(spec: Spring, touched: u64) -> Self {
        NodeAnim { spec, colors: [None, None, None, None], origin: None, touched }
    }

    fn tracks(&mut self) -> impl Iterator<Item = &mut Track> {
        self.colors
            .iter_mut()
            .flatten()
            .flat_map(|tracks| tracks.iter_mut())
            .chain(self.origin.iter_mut().flat_map(|tracks| tracks.iter_mut()))
    }

    fn active(&self) -> bool {
        self.colors
            .iter()
            .flatten()
            .flat_map(|tracks| tracks.iter())
            .chain(self.origin.iter().flat_map(|tracks| tracks.iter()))
            .any(Track::active)
    }
}

/// A scroll offset in flight (`.scroll_target` under an animation
/// scope). Keyed by region path, exempt from the touch sweep: it
/// removes itself on settle, and the wheel cancels it.
struct ScrollAnim {
    spec: Spring,
    tracks: [Track; 2],
}

/// Every live animation, keyed by the identity captured at render.
#[derive(Default)]
pub struct Animator {
    entries: FxHashMap<Rc<str>, NodeAnim>,
    scrolls: FxHashMap<Rc<str>, ScrollAnim>,
    /// The sweep clock: bumped when a PLACE begins, never by ticks. An
    /// entry the newest place did not touch belongs to an unmounted
    /// node; ticks alone (with no frame between them) sweep nothing.
    places: u64,
    /// Accessibility: every animation completes instantly.
    reduce_motion: bool,
    /// A RESIZE is not an animation: while this pass flag is on, a
    /// changed target SNAPS instead of starting a flight. The runtime
    /// raises it for any pass whose proposal differs from the last —
    /// live-resizing a window must track the mouse, never wobble after
    /// it. Flights whose target did not change keep flying.
    snap_retargets: bool,
}

fn color_tracks(color: Color) -> [Track; 4] {
    [
        Track::at(color.r as f64),
        Track::at(color.g as f64),
        Track::at(color.b as f64),
        Track::at(color.a as f64),
    ]
}

fn channel(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

impl Animator {
    /// A layout pass is about to place — the sweep clock advances so
    /// this pass's touches are distinguishable from the last one's.
    pub(crate) fn note_place(&mut self) {
        self.places += 1;
    }

    /// Raised for a pass whose PROPOSAL changed (a resize): geometry
    /// moved because the window did, and that is not an animation.
    pub(crate) fn set_snap_retargets(&mut self, on: bool) {
        self.snap_retargets = on;
    }

    /// The place asks: which entry answers for `key`? Seeds a fresh one
    /// on the first sighting and stamps the touch either way.
    fn entry(&mut self, key: &str, spec: Spring) -> &mut NodeAnim {
        let places = self.places;
        if !self.entries.contains_key(key) {
            self.entries.insert(Rc::from(key), NodeAnim::fresh(spec, places));
        }
        let entry = self.entries.get_mut(key).expect("entry just seeded");
        entry.spec = spec;
        entry.touched = places;
        entry
    }

    /// The color to paint NOW for one of this identity's slots. The
    /// first sighting seeds silently (nothing animates on mount); a
    /// changed target retargets in place, keeping velocity.
    pub(crate) fn resolve_color(
        &mut self,
        key: &str,
        spec: Spring,
        slot: Channel,
        target: Color,
    ) -> Color {
        if self.reduce_motion {
            return target;
        }
        let snap = self.snap_retargets;
        let entry = self.entry(key, spec);
        let tracks = entry.colors[slot as usize].get_or_insert_with(|| color_tracks(target));
        let want = [target.r as f64, target.g as f64, target.b as f64, target.a as f64];
        for (track, want) in tracks.iter_mut().zip(want) {
            if track.target != want {
                if snap {
                    *track = Track::at(want);
                } else {
                    track.retarget(want);
                }
            }
        }
        Color {
            r: channel(tracks[0].value),
            g: channel(tracks[1].value),
            b: channel(tracks[2].value),
            a: channel(tracks[3].value),
        }
    }

    /// The origin to paint NOW — coordinates RELATIVE to the enclosing
    /// scroll box, so scrolling moves the content 1:1 and never bends a
    /// spring. Same lifecycle as the colors.
    pub(crate) fn resolve_origin(
        &mut self,
        key: &str,
        spec: Spring,
        target: (f64, f64),
    ) -> (f64, f64) {
        if self.reduce_motion {
            return target;
        }
        let snap = self.snap_retargets;
        let entry = self.entry(key, spec);
        let tracks = entry
            .origin
            .get_or_insert_with(|| [Track::at(target.0), Track::at(target.1)]);
        for (track, want) in tracks.iter_mut().zip([target.0, target.1]) {
            if track.target != want {
                if snap {
                    *track = Track::at(want);
                } else {
                    track.retarget(want);
                }
            }
        }
        (tracks[0].value, tracks[1].value)
    }

    /// Starts (or bends) a scroll-offset flight for a region.
    pub(crate) fn animate_scroll(
        &mut self,
        path: &str,
        from: (f64, f64),
        to: (f64, f64),
        spec: Spring,
    ) {
        if self.reduce_motion {
            return;
        }
        match self.scrolls.get_mut(path) {
            Some(flight) => {
                flight.spec = spec;
                flight.tracks[0].retarget(to.0);
                flight.tracks[1].retarget(to.1);
            }
            None => {
                let mut tracks = [Track::at(from.0), Track::at(from.1)];
                tracks[0].retarget(to.0);
                tracks[1].retarget(to.1);
                self.scrolls.insert(Rc::from(path), ScrollAnim { spec, tracks });
            }
        }
    }

    /// The wheel is sovereign: a user scroll kills the flight.
    pub(crate) fn cancel_scroll(&mut self, path: &str) {
        self.scrolls.remove(path);
    }

    /// Advances every live track by `dt` seconds. Returns whether
    /// anything moved plus the scroll offsets to write back — a settled
    /// flight delivers its final (snapped) value once and leaves.
    pub(crate) fn tick(&mut self, dt: f64) -> (bool, Vec<(Rc<str>, (f64, f64))>) {
        let places = self.places;
        let mut moved = false;
        self.entries.retain(|_, entry| {
            if entry.touched < places {
                return false;
            }
            let spec = entry.spec;
            for track in entry.tracks() {
                if track.active() {
                    moved = true;
                    track.step(spec, dt);
                }
            }
            true
        });
        let mut offsets = Vec::new();
        self.scrolls.retain(|path, flight| {
            moved = true;
            for track in &mut flight.tracks {
                track.step(flight.spec, dt);
            }
            offsets.push((Rc::clone(path), (flight.tracks[0].value, flight.tracks[1].value)));
            flight.tracks.iter().any(Track::active)
        });
        (moved, offsets)
    }

    /// Does any track still move? The shell parks its frame driver on a
    /// false.
    pub(crate) fn wants_frame(&self) -> bool {
        self.entries.values().any(NodeAnim::active) || !self.scrolls.is_empty()
    }

    /// Accessibility switch: on, every animation completes instantly
    /// and the retained motion is dropped.
    pub(crate) fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
        if on {
            self.entries.clear();
            self.scrolls.clear();
        }
    }

    /// Reduce motion, read by the runtime — an animated reveal snaps
    /// instead of flying while this is on.
    pub(crate) fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color { r: 200, g: 40, b: 40, a: 255 };
    const BLUE: Color = Color { r: 40, g: 40, b: 200, a: 255 };

    #[test]
    fn a_critical_spring_closes_in_without_overshoot() {
        let spec = Spring { response: 0.4, damping: 1.0 };
        let mut track = Track { value: 0.0, velocity: 0.0, target: 100.0 };
        let mut previous = 0.0;
        for _ in 0..240 {
            track.step(spec, 1.0 / 120.0);
            assert!(track.value >= previous - 1e-9, "never moves backwards");
            assert!(track.value <= 100.0 + 1e-9, "never overshoots");
            previous = track.value;
        }
        assert_eq!(track.value, 100.0, "the settle snaps exactly");
        assert_eq!(track.velocity, 0.0);
    }

    #[test]
    fn a_bouncy_spring_overshoots_then_settles_exactly() {
        let spec = Spring::bouncy();
        let mut track = Track { value: 0.0, velocity: 0.0, target: 100.0 };
        let mut peak: f64 = 0.0;
        for _ in 0..600 {
            track.step(spec, 1.0 / 120.0);
            peak = peak.max(track.value);
        }
        assert!(peak > 100.5, "damping below one bounces past the target");
        assert_eq!(track.value, 100.0);
        assert_eq!(track.velocity, 0.0);
    }

    #[test]
    fn the_curve_is_deterministic_for_a_fixed_step() {
        // the numeric golden: same inputs, same trajectory, forever
        let spec = Spring::smooth();
        let mut track = Track { value: 0.0, velocity: 0.0, target: 100.0 };
        let mut samples = Vec::new();
        for _ in 0..4 {
            track.step(spec, 1.0 / 60.0);
            samples.push(track.value);
        }
        let expected = [
            2.883665370585007,
            9.744317214078507,
            18.596890406371912,
            28.159783007797472,
        ];
        for (sample, expected) in samples.iter().zip(expected) {
            assert!((sample - expected).abs() < 1e-9, "{sample} vs {expected}");
        }
    }

    #[test]
    fn a_retarget_keeps_the_velocity() {
        let spec = Spring::smooth();
        let mut track = Track { value: 0.0, velocity: 0.0, target: 100.0 };
        for _ in 0..12 {
            track.step(spec, 1.0 / 120.0);
        }
        let mid_value = track.value;
        let mid_velocity = track.velocity;
        assert!(mid_velocity > 0.0);
        track.retarget(-50.0);
        assert_eq!(track.value, mid_value, "position carries over");
        assert_eq!(track.velocity, mid_velocity, "the motion bends, it does not restart");
    }

    #[test]
    fn the_animator_seeds_silently_and_then_moves() {
        let mut animator = Animator::default();
        // first sighting: the target itself, at rest
        let seeded = animator.resolve_color("chip", Spring::smooth(), Channel::Background, BLUE);
        assert_eq!(seeded, BLUE);
        assert!(!animator.wants_frame());
        // the target changes: the painted value stays put until a tick
        let held = animator.resolve_color("chip", Spring::smooth(), Channel::Background, RED);
        assert_eq!(held, BLUE);
        assert!(animator.wants_frame());
        assert!(animator.tick(1.0 / 120.0).0);
        let moving = animator.resolve_color("chip", Spring::smooth(), Channel::Background, RED);
        assert_ne!(moving, BLUE);
        assert_ne!(moving, RED);
        // run it dry: the final resolve is the target, bit-exact
        let mut guard = 0;
        while animator.wants_frame() && guard < 1000 {
            animator.tick(1.0 / 120.0);
            let _ = animator.resolve_color("chip", Spring::smooth(), Channel::Background, RED);
            guard += 1;
        }
        assert!(guard < 1000, "the spring settles");
        assert_eq!(animator.resolve_color("chip", Spring::smooth(), Channel::Background, RED), RED);
    }

    #[test]
    fn an_entry_the_newest_place_missed_dies_on_the_next_tick() {
        let mut animator = Animator::default();
        animator.note_place();
        let _ = animator.resolve_color("gone", Spring::smooth(), Channel::Background, RED);
        let _ = animator.resolve_color("gone", Spring::smooth(), Channel::Background, BLUE);
        assert!(animator.wants_frame());
        // a NEW place runs and never touches it: the node unmounted
        animator.note_place();
        animator.tick(1.0 / 120.0);
        assert_eq!(animator.len(), 0, "the sweep collected the orphan");
        assert!(!animator.wants_frame());
    }

    #[test]
    fn ticks_alone_never_sweep_a_live_flight() {
        // the regression: a burst of ticks with no frame between them
        // must not kill mounted entries (that re-seeds at the target
        // and swallows the animation whole)
        let mut animator = Animator::default();
        animator.note_place();
        let _ = animator.resolve_color("chip", Spring::smooth(), Channel::Background, BLUE);
        let _ = animator.resolve_color("chip", Spring::smooth(), Channel::Background, RED);
        animator.tick(1.0 / 120.0);
        animator.tick(1.0 / 120.0);
        animator.tick(1.0 / 120.0);
        assert!(animator.wants_frame(), "the flight survives the burst");
        let moving = animator.resolve_color("chip", Spring::smooth(), Channel::Background, RED);
        assert_ne!(moving, BLUE);
        assert_ne!(moving, RED);
    }

    #[test]
    fn reduce_motion_paints_targets_and_retains_nothing() {
        let mut animator = Animator::default();
        let _ = animator.resolve_color("chip", Spring::smooth(), Channel::Background, BLUE);
        animator.set_reduce_motion(true);
        assert_eq!(animator.resolve_color("chip", Spring::smooth(), Channel::Background, RED), RED);
        assert_eq!(animator.len(), 0);
        assert!(!animator.wants_frame());
        assert!(!animator.tick(1.0 / 120.0).0);
    }
}
