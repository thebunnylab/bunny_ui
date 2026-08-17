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

/// The animated channels of one identity. Today: the background color.
struct NodeAnim {
    spec: Spring,
    background: Option<[Track; 4]>,
    /// The tick generation that last touched this entry at place —
    /// stops advancing when the node unmounts.
    touched: u64,
}

/// Every live animation, keyed by the identity captured at render.
#[derive(Default)]
pub struct Animator {
    entries: FxHashMap<Rc<str>, NodeAnim>,
    /// The sweep clock — bumped once per tick.
    generation: u64,
    /// Accessibility: every animation completes instantly.
    reduce_motion: bool,
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
    /// The value to paint NOW for this identity's background. The first
    /// sighting seeds silently (nothing animates on mount); a changed
    /// target retargets the springs in place, keeping velocity; the
    /// return is the current interpolated color.
    pub(crate) fn resolve_background(&mut self, key: &str, spec: Spring, target: Color) -> Color {
        if self.reduce_motion {
            return target;
        }
        let generation = self.generation;
        if !self.entries.contains_key(key) {
            self.entries.insert(
                Rc::from(key),
                NodeAnim { spec, background: Some(color_tracks(target)), touched: generation },
            );
            return target;
        }
        let entry = self.entries.get_mut(key).expect("entry just checked");
        entry.spec = spec;
        entry.touched = generation;
        let tracks = entry.background.get_or_insert_with(|| color_tracks(target));
        let want = [target.r as f64, target.g as f64, target.b as f64, target.a as f64];
        for (track, want) in tracks.iter_mut().zip(want) {
            if track.target != want {
                track.retarget(want);
            }
        }
        Color {
            r: channel(tracks[0].value),
            g: channel(tracks[1].value),
            b: channel(tracks[2].value),
            a: channel(tracks[3].value),
        }
    }

    /// Advances every live track by `dt` seconds. `true` = something
    /// moved and the frame must repaint. Also the sweep: an entry no
    /// place touched since the previous tick belongs to an unmounted
    /// node and is dropped.
    pub(crate) fn tick(&mut self, dt: f64) -> bool {
        self.generation += 1;
        let generation = self.generation;
        let mut moved = false;
        self.entries.retain(|_, entry| {
            if entry.touched + 1 < generation {
                return false;
            }
            if let Some(tracks) = &mut entry.background {
                for track in tracks {
                    if track.active() {
                        moved = true;
                        track.step(entry.spec, dt);
                    }
                }
            }
            true
        });
        moved
    }

    /// Does any track still move? The shell parks its frame driver on a
    /// false.
    pub(crate) fn wants_frame(&self) -> bool {
        self.entries.values().any(|entry| {
            entry
                .background
                .as_ref()
                .is_some_and(|tracks| tracks.iter().any(Track::active))
        })
    }

    /// Accessibility switch: on, every animation completes instantly
    /// and the retained motion is dropped.
    pub(crate) fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
        if on {
            self.entries.clear();
        }
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
        let seeded = animator.resolve_background("chip", Spring::smooth(), BLUE);
        assert_eq!(seeded, BLUE);
        assert!(!animator.wants_frame());
        // the target changes: the painted value stays put until a tick
        let held = animator.resolve_background("chip", Spring::smooth(), RED);
        assert_eq!(held, BLUE);
        assert!(animator.wants_frame());
        assert!(animator.tick(1.0 / 120.0));
        let moving = animator.resolve_background("chip", Spring::smooth(), RED);
        assert_ne!(moving, BLUE);
        assert_ne!(moving, RED);
        // run it dry: the final resolve is the target, bit-exact
        let mut guard = 0;
        while animator.wants_frame() && guard < 1000 {
            animator.tick(1.0 / 120.0);
            let _ = animator.resolve_background("chip", Spring::smooth(), RED);
            guard += 1;
        }
        assert!(guard < 1000, "the spring settles");
        assert_eq!(animator.resolve_background("chip", Spring::smooth(), RED), RED);
    }

    #[test]
    fn an_untouched_entry_dies_on_the_next_tick() {
        let mut animator = Animator::default();
        let _ = animator.resolve_background("gone", Spring::smooth(), RED);
        let _ = animator.resolve_background("gone", Spring::smooth(), BLUE);
        assert!(animator.wants_frame());
        // two ticks with no place in between: the node unmounted
        animator.tick(1.0 / 120.0);
        animator.tick(1.0 / 120.0);
        assert_eq!(animator.len(), 0, "the sweep collected the orphan");
        assert!(!animator.wants_frame());
    }

    #[test]
    fn reduce_motion_paints_targets_and_retains_nothing() {
        let mut animator = Animator::default();
        let _ = animator.resolve_background("chip", Spring::smooth(), BLUE);
        animator.set_reduce_motion(true);
        assert_eq!(animator.resolve_background("chip", Spring::smooth(), RED), RED);
        assert_eq!(animator.len(), 0);
        assert!(!animator.wants_frame());
        assert!(!animator.tick(1.0 / 120.0));
    }
}
