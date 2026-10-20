//! Motions: an entity that walks somewhere at a steady pace.
//!
//! A [`Motion`] is where the entity set out, where it goes, when it left
//! and how fast: its place at any moment is a pure function of the engine
//! clock, so a walk writes the world once — when it starts — and every
//! frame between reads where it has got to. A box that shows the world
//! asks for frames until it arrives ([`super::store::Entity::move_to`]).

use super::math::{Quat, Transform, Vec3};

/// A walk from one place to another, a meter of `speed` a second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub from: Vec3,
    pub to: Vec3,
    /// The engine clock's time it set out at.
    pub start: f64,
    /// Meters a second.
    pub speed: f64,
    /// Whether it turns to face its way, about the world's up: its front
    /// is its own +Z, as a glTF figure's is.
    pub face: bool,
}

impl Motion {
    /// The share of the way walked at `time`, in `[0, 1]`.
    pub fn progress(&self, time: f64) -> f64 {
        let length = self.from.distance(self.to);
        if length <= 0.0 || self.speed <= 0.0 || !self.speed.is_finite() {
            return 1.0;
        }
        ((time - self.start) * self.speed / length).clamp(0.0, 1.0)
    }

    /// Where it stands at `time`.
    pub fn position(&self, time: f64) -> Vec3 {
        let share = self.progress(time);
        if share >= 1.0 {
            return self.to;
        }
        self.from + (self.to - self.from) * share
    }

    /// Whether it still walks at `time`.
    pub fn moving(&self, time: f64) -> bool {
        self.progress(time) < 1.0 && time >= self.start
    }

    /// The turn that faces its way, flat on the ground; `None` for a walk
    /// that goes nowhere across it.
    pub fn heading(&self) -> Option<Quat> {
        let way = self.to - self.from;
        let flat = Vec3 { y: 0.0, ..way };
        flat.try_normalized().map(|direction| Quat::from_arc(Vec3::Z, direction))
    }

    /// `transform` as the walk leaves it at `time`: the place the walk has
    /// reached, and, when it faces its way, that heading.
    pub fn applied(&self, transform: Transform, time: f64) -> Transform {
        let rotation = if self.face { self.heading().unwrap_or(transform.rotation) } else { transform.rotation };
        Transform { translation: self.position(time), rotation, scale: transform.scale }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::math::vec3;

    #[test]
    fn a_walk_goes_at_its_pace_and_stops_where_it_was_sent() {
        let walk = Motion { from: vec3(0.0, 0.0, 0.0), to: vec3(4.0, 0.0, 0.0), start: 10.0, speed: 2.0, face: true };
        assert_eq!(walk.position(9.0), vec3(0.0, 0.0, 0.0), "not yet left");
        assert_eq!(walk.position(11.0), vec3(2.0, 0.0, 0.0), "halfway after a second");
        assert!(walk.moving(11.0) && !walk.moving(12.0) && !walk.moving(30.0));
        assert_eq!(walk.position(30.0), vec3(4.0, 0.0, 0.0), "held where it arrived");
        // it faces +x: its +Z front turned onto +x
        let front = walk.heading().expect("a way across the ground").rotate(Vec3::Z);
        assert!((front - Vec3::X).length() < 1e-12, "{front:?}");
        // a walk straight up has no way across the ground to face
        let climb = Motion { to: vec3(0.0, 3.0, 0.0), ..walk };
        assert!(climb.heading().is_none());
        assert_eq!(climb.applied(Transform::IDENTITY, 30.0).rotation, Quat::IDENTITY);
    }
}
