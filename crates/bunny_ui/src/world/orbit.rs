//! The hand on the camera: turn, pan and zoom.
//!
//! [`Orbit`] reads the events of a box and moves a `State<Camera>`:
//!
//! - a drag turns the view around the point the press landed on;
//! - a drag with shift pans: that point follows the hand;
//! - the wheel and a pinch zoom toward the point under the pointer, so
//!   what the pointer is on stays under the pointer;
//! - a press that never travels is a TAP: its release comes back
//!   ignored, and the caller answers it.
//!
//! The turn keeps the world's up: the horizon stays level, and the pitch
//! stops short of straight up and straight down.

use motor::state::State;

use super::camera::{Camera, Lens, spherical};
use super::math::{Aabb, Plane, Quat, Vec3};
use super::store::World;
use crate::custom::{ElementEvent, EventCtx, Response};
use crate::layout::{Point, Size};

/// Points the pointer may travel before a press becomes a drag.
pub const TAP_SLOP: f64 = 4.0;
/// Radians the view turns for each point of drag.
const TURN_PER_POINT: f64 = 0.008;
/// The zoom of one point of wheel: `exp(points × this)`.
const ZOOM_PER_POINT: f64 = 0.004;

/// How far the hand may take the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitLimits {
    /// Meters from the eye to the target, for a perspective lens.
    pub distance: (f64, f64),
    /// Degrees above the horizon.
    pub pitch_degrees: (f64, f64),
    /// Meters the view shows, for a parallel lens.
    pub height: (f64, f64),
    /// The target stays inside this box.
    pub bounds: Option<Aabb>,
}

impl Default for OrbitLimits {
    fn default() -> OrbitLimits {
        OrbitLimits { distance: (0.05, 1.0e6), pitch_degrees: (-89.0, 89.0), height: (0.01, 1.0e6), bounds: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Turn,
    Pan,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    down: Point,
    last: Point,
    mode: Mode,
    pivot: Vec3,
    travelled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
struct OrbitGesture {
    press: Option<Press>,
}

/// The hand on a camera. `Copy`: keep it in a view's field.
#[derive(Clone, Copy)]
pub struct Orbit {
    state: State<OrbitGesture>,
    limits: OrbitLimits,
}

impl Default for Orbit {
    fn default() -> Orbit {
        Orbit::new()
    }
}

impl Orbit {
    pub fn new() -> Orbit {
        Orbit { state: State::new(OrbitGesture::default()), limits: OrbitLimits::default() }
    }

    pub fn limits(self, limits: OrbitLimits) -> Orbit {
        Orbit { limits, ..self }
    }

    /// Keeps the gesture's bookkeeping WITHOUT a write: no view paints
    /// it, so a press that moves no camera asks for no frame.
    fn keep(&self, gesture: OrbitGesture) {
        self.state.update_if(|held| {
            *held = gesture;
            (false, ())
        });
    }

    /// Is a drag moving the camera right now?
    pub fn is_dragging(&self) -> bool {
        self.state.with(|gesture| gesture.press.is_some_and(|press| press.travelled))
    }

    /// Moves `camera` for `event`. A release that ends a press that
    /// never travelled comes back IGNORED: that was a tap, and the caller
    /// decides what a tap does.
    pub fn event(&self, event: &ElementEvent, ctx: &EventCtx, camera: State<Camera>, world: World) -> Response {
        self.handle(event, ctx.size(), camera, world)
    }

    pub(crate) fn handle(&self, event: &ElementEvent, size: Size, camera: State<Camera>, world: World) -> Response {
        let now = motor::task::now();
        match *event {
            ElementEvent::PointerDown { at, modifiers, .. } => {
                let current = camera.with(|camera| camera.at(now));
                let mode = if modifiers.shift { Mode::Pan } else { Mode::Turn };
                let pivot = pivot_at(&current, at, size, world);
                self.keep(OrbitGesture { press: Some(Press { down: at, last: at, mode, pivot, travelled: false }) });
                Response::handled()
            }
            ElementEvent::PointerMoved { at, pressed: true, .. } => {
                let Some(mut press) = self.state.with(|gesture| gesture.press) else { return Response::ignored() };
                if !press.travelled && (at.x - press.down.x).hypot(at.y - press.down.y) < TAP_SLOP {
                    return Response::handled();
                }
                let (dx, dy) = if press.travelled { (at.x - press.last.x, at.y - press.last.y) } else { (at.x - press.down.x, at.y - press.down.y) };
                press.travelled = true;
                press.last = at;
                self.keep(OrbitGesture { press: Some(press) });
                let current = camera.with(|camera| camera.at(now));
                let moved = match press.mode {
                    Mode::Turn => turn(current, press.pivot, -dx * TURN_PER_POINT, dy * TURN_PER_POINT, &self.limits),
                    Mode::Pan => pan(current, press.pivot, dx, dy, size, &self.limits),
                };
                camera.set_if_changed(moved);
                Response::handled()
            }
            ElementEvent::PointerUp { .. } | ElementEvent::PointerCancelled { .. } => {
                let travelled = self.state.with(|gesture| gesture.press.is_some_and(|press| press.travelled));
                let had_press = self.state.with(|gesture| gesture.press.is_some());
                if had_press {
                    self.keep(OrbitGesture::default());
                }
                if travelled { Response::handled() } else { Response::ignored() }
            }
            ElementEvent::Wheel { at, dx, dy, modifiers, .. } => {
                let current = camera.with(|camera| camera.at(now));
                let moved = if modifiers.shift {
                    let pivot = pivot_at(&current, at, size, world);
                    pan(current, pivot, -dx, -dy, size, &self.limits)
                } else {
                    zoom(current, at, (dy * ZOOM_PER_POINT).exp(), size, world, &self.limits)
                };
                camera.set_if_changed(moved);
                Response::handled()
            }
            ElementEvent::Magnify { at, scale } => {
                if scale <= 0.0 || !scale.is_finite() {
                    return Response::handled();
                }
                let current = camera.with(|camera| camera.at(now));
                camera.set_if_changed(zoom(current, at, 1.0 / scale, size, world, &self.limits));
                Response::handled()
            }
            _ => Response::ignored(),
        }
    }
}

/// The world point under the pointer: the surface there, or else the
/// plane through the target, facing the eye.
fn pivot_at(camera: &Camera, at: Point, size: Size, world: World) -> Vec3 {
    if let Some(hit) = world.pick(*camera, at, size) {
        return hit.point;
    }
    let ray = camera.ray(at, size);
    let plane = Plane::new(camera.target, -camera.forward());
    match camera.lens {
        Lens::Parallel { .. } => {
            // parallel rays start on the plane of the eye: meet the plane either way
            let facing = plane.normal.dot(ray.direction);
            if facing.abs() < 1e-12 {
                return camera.target;
            }
            ray.at(-plane.signed_distance(ray.origin) / facing)
        }
        Lens::Perspective { .. } => ray.hits_plane(&plane).map_or(camera.target, |t| ray.at(t)),
    }
}

/// The camera turned around `pivot`: `yaw` radians around the world's
/// up, `pitch` radians around the eye's right, the pitch held inside
/// the limits.
fn turn(camera: Camera, pivot: Vec3, yaw: f64, pitch: f64, limits: &OrbitLimits) -> Camera {
    let Some((_, current, _)) = spherical(camera.eye - camera.target) else { return camera };
    let (low, high) = (limits.pitch_degrees.0.to_radians(), limits.pitch_degrees.1.to_radians());
    let wanted = (current + pitch).clamp(low.min(high), high.max(low));
    let pitch = wanted - current;
    // up and down around the eye's right, then around the vertical
    let rotation = Quat::from_yaw(yaw) * Quat::from_axis_angle(camera.right(), -pitch);
    let eye = pivot + rotation.rotate(camera.eye - pivot);
    let target = pivot + rotation.rotate(camera.target - pivot);
    keep_inside(Camera { eye, target, flight: None, ..camera }, limits)
}

/// The camera slid so that `pivot` follows a hand that moved `(dx, dy)`
/// points.
fn pan(camera: Camera, pivot: Vec3, dx: f64, dy: f64, size: Size, limits: &OrbitLimits) -> Camera {
    let depth = (pivot - camera.eye).dot(camera.forward());
    let meters = camera.meters_per_point(depth, size.height);
    let shift = camera.right() * (-dx * meters) + camera.upward() * (dy * meters);
    keep_inside(Camera { eye: camera.eye + shift, target: camera.target + shift, flight: None, ..camera }, limits)
}

/// The camera brought `factor` times farther (below one: closer), the
/// point under the pointer held under the pointer.
fn zoom(camera: Camera, at: Point, factor: f64, size: Size, world: World, limits: &OrbitLimits) -> Camera {
    let anchor = pivot_at(&camera, at, size, world);
    let moved = match camera.lens {
        Lens::Perspective { .. } => {
            let distance = camera.distance();
            let wanted = (distance * factor).clamp(limits.distance.0, limits.distance.1);
            let factor = if distance > 0.0 { wanted / distance } else { 1.0 };
            Camera {
                eye: anchor + (camera.eye - anchor) * factor,
                target: anchor + (camera.target - anchor) * factor,
                flight: None,
                ..camera
            }
        }
        Lens::Parallel { height } => {
            let wanted = (height * factor).clamp(limits.height.0, limits.height.1);
            let factor = wanted / height;
            // the anchor's offset from the middle of the view shrinks with the view
            let offset = anchor - camera.target;
            let across = camera.right() * offset.dot(camera.right()) + camera.upward() * offset.dot(camera.upward());
            let shift = across * (1.0 - factor);
            Camera {
                eye: camera.eye + shift,
                target: camera.target + shift,
                lens: Lens::Parallel { height: wanted },
                flight: None,
                ..camera
            }
        }
    };
    keep_inside(moved, limits)
}

/// The target held inside the limits' box; the eye moves with it.
fn keep_inside(camera: Camera, limits: &OrbitLimits) -> Camera {
    let Some(bounds) = limits.bounds else { return camera };
    let target = camera.target.max(bounds.min).min(bounds.max);
    let shift = target - camera.target;
    Camera { eye: camera.eye + shift, target, ..camera }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Modifiers;
    use crate::custom::WheelPhase;
    use crate::world::math::vec3;

    fn size() -> Size {
        Size { width: 800.0, height: 600.0 }
    }

    fn down(x: f64, y: f64) -> ElementEvent {
        ElementEvent::PointerDown { at: Point { x, y }, clicks: 1, modifiers: Modifiers::NONE }
    }

    fn drag(x: f64, y: f64) -> ElementEvent {
        ElementEvent::PointerMoved { at: Point { x, y }, pressed: true, modifiers: Modifiers::NONE }
    }

    fn up(x: f64, y: f64) -> ElementEvent {
        ElementEvent::PointerUp { at: Point { x, y } }
    }

    fn wheel(x: f64, y: f64, dy: f64) -> ElementEvent {
        ElementEvent::Wheel { at: Point { x, y }, dx: 0.0, dy, modifiers: Modifiers::NONE, phase: WheelPhase::Changed }
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6
    }

    #[test]
    fn a_press_without_travel_is_a_tap() {
        let (world, orbit) = (World::new(), Orbit::new());
        let camera = State::new(Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 10.0));
        let before = camera.with(|c| *c);
        assert!(orbit.handle(&down(100.0, 100.0), size(), camera, world).handled);
        orbit.handle(&drag(102.0, 101.0), size(), camera, world);
        let release = orbit.handle(&up(102.0, 101.0), size(), camera, world);
        assert!(!release.handled, "the release of a tap comes back ignored");
        assert_eq!(camera.with(|c| *c), before, "a tap moves nothing");
        // and a drag is no tap
        orbit.handle(&down(100.0, 100.0), size(), camera, world);
        orbit.handle(&drag(160.0, 100.0), size(), camera, world);
        assert!(orbit.handle(&up(160.0, 100.0), size(), camera, world).handled);
        assert_ne!(camera.with(|c| *c), before);
    }

    #[test]
    fn an_orbit_keeps_the_pivot_on_screen() {
        let (world, orbit) = (World::new(), Orbit::new());
        let camera = State::new(Camera::orbiting(vec3(1.0, 0.0, 2.0), 20.0, 30.0, 12.0));
        let at = Point { x: 520.0, y: 210.0 };
        let pivot = pivot_at(&camera.with(|c| *c), at, size(), world);
        orbit.handle(&down(at.x, at.y), size(), camera, world);
        for (x, y) in [(560.0, 230.0), (640.0, 180.0), (400.0, 260.0)] {
            orbit.handle(&drag(x, y), size(), camera, world);
            let seen = camera.with(|c| c.project(pivot, size())).expect("in front");
            assert!(close(seen, at), "the pivot stays where it was pressed: {seen:?}");
        }
    }

    #[test]
    fn pitch_and_distance_stay_within_limits() {
        let world = World::new();
        let limits = OrbitLimits { distance: (2.0, 30.0), pitch_degrees: (5.0, 60.0), ..OrbitLimits::default() };
        let orbit = Orbit::new().limits(limits);
        let camera = State::new(Camera::orbiting(Vec3::ZERO, 0.0, 30.0, 10.0));
        orbit.handle(&down(400.0, 300.0), size(), camera, world);
        for step in 1..40 {
            orbit.handle(&drag(400.0, 300.0 + step as f64 * 50.0), size(), camera, world);
        }
        let (_, pitch, _) = spherical(camera.with(|c| c.eye - c.target)).unwrap();
        assert!((pitch.to_degrees() - 60.0).abs() < 1e-9, "{}", pitch.to_degrees());
        orbit.handle(&up(400.0, 2300.0), size(), camera, world);
        for _ in 0..50 {
            orbit.handle(&wheel(400.0, 300.0, 400.0), size(), camera, world);
        }
        assert!((camera.with(|c| c.distance()) - 30.0).abs() < 1e-6);
        for _ in 0..80 {
            orbit.handle(&wheel(400.0, 300.0, -400.0), size(), camera, world);
        }
        assert!((camera.with(|c| c.distance()) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let world = World::new();
        let base = Camera::looking_at(vec3(3.0, 5.0, 9.0), vec3(0.0, 1.0, 0.0));
        for start in [base, base.parallel()] {
            let orbit = Orbit::new();
            let camera = State::new(start);
            let at = Point { x: 620.0, y: 140.0 };
            let anchor = pivot_at(&start, at, size(), world);
            for dy in [-120.0, -80.0, 200.0] {
                orbit.handle(&wheel(at.x, at.y, dy), size(), camera, world);
                let seen = camera.with(|c| c.project(anchor, size())).expect("in front");
                assert!(close(seen, at), "{start:?}: the anchor drifted to {seen:?}");
            }
            assert_ne!(camera.with(|c| *c), start);
        }
    }

    #[test]
    fn a_pan_carries_the_pressed_point_with_the_hand() {
        let world = World::new();
        let orbit = Orbit::new();
        let start = Camera::orbiting(Vec3::ZERO, 45.0, 30.0, 10.0);
        for camera in [State::new(start), State::new(start.parallel())] {
            let at = Point { x: 300.0, y: 260.0 };
            let pivot = pivot_at(&camera.with(|c| *c), at, size(), world);
            let shift = ElementEvent::PointerDown { at, clicks: 1, modifiers: Modifiers::SHIFT };
            orbit.handle(&shift, size(), camera, world);
            orbit.handle(&drag(380.0, 300.0), size(), camera, world);
            let seen = camera.with(|c| c.project(pivot, size())).expect("in front");
            assert!(close(seen, Point { x: 380.0, y: 300.0 }), "{seen:?}");
            orbit.handle(&up(380.0, 300.0), size(), camera, world);
        }
    }
}
