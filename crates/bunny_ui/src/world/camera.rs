//! Cameras: where the eye stands, where it looks, and the lens between
//! the world and the box.
//!
//! A camera is a plain `Copy` value. Keep it in a `State<Camera>` and the
//! box that paints the world repaints when it moves. A camera can FLY to
//! another one: the flight is a start time and a length, and the camera
//! a frame shows is the flight sampled at the engine clock — no state
//! is written while it flies.

use super::math::{Aabb, Mat4, Ray, Vec3, vec3};
use crate::layout::{Point, Size};

/// The lens of a camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lens {
    /// Far things are small: the eye of a person. The angle runs from
    /// the bottom of the view to its top.
    Perspective { fov_y_degrees: f64 },
    /// Far things keep their size: a drawing, a plan, an isometric
    /// view. `height` is the meters the view shows from bottom to top.
    Parallel { height: f64 },
}

/// A camera on its way from one framing to another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    pub from_eye: Vec3,
    pub from_target: Vec3,
    pub from_lens: Lens,
    /// Engine-clock seconds ([`crate::task::now`]).
    pub start: f64,
    pub seconds: f64,
}

/// An eye in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    /// Which way is up for the eye. The world's up by default.
    pub up: Vec3,
    pub lens: Lens,
    /// The nearest distance the camera sees. `None` = fit to what is in
    /// front of it, which is right almost always.
    pub near: Option<f64>,
    /// The farthest distance it sees. `None` = fit.
    pub far: Option<f64>,
    pub flight: Option<Flight>,
}

/// The field of view a camera gets when nothing says otherwise.
pub const DEFAULT_FOV_DEGREES: f64 = 45.0;

impl Camera {
    /// An eye at `eye` that looks at `target`, through a 45° lens.
    pub fn looking_at(eye: Vec3, target: Vec3) -> Camera {
        Camera {
            eye,
            target,
            up: Vec3::Y,
            lens: Lens::Perspective { fov_y_degrees: DEFAULT_FOV_DEGREES },
            near: None,
            far: None,
            flight: None,
        }
    }

    /// An eye `distance` meters from `target`, turned `yaw_degrees`
    /// around the vertical (0 = looking from +Z toward −Z, 90 = from
    /// +X) and raised `pitch_degrees` above the horizon.
    pub fn orbiting(target: Vec3, yaw_degrees: f64, pitch_degrees: f64, distance: f64) -> Camera {
        Camera::looking_at(target + direction_of(yaw_degrees.to_radians(), pitch_degrees.to_radians()) * distance, target)
    }

    pub fn perspective(self, fov_y_degrees: f64) -> Camera {
        Camera { lens: Lens::Perspective { fov_y_degrees: fov_y_degrees.clamp(1.0, 170.0) }, ..self }
    }

    /// The same view through a parallel lens. The plane through the
    /// target keeps its framing: what stood at the edge of the view still
    /// does.
    pub fn parallel(self) -> Camera {
        Camera { lens: Lens::Parallel { height: self.view_height() }, ..self }
    }

    /// A parallel lens that shows `height` meters from bottom to top.
    pub fn parallel_height(self, height: f64) -> Camera {
        Camera { lens: Lens::Parallel { height: height.max(1e-9) }, ..self }
    }

    pub fn with_up(self, up: Vec3) -> Camera {
        Camera { up, ..self }
    }

    /// Fixed near and far distances, in place of the fit.
    pub fn clip_planes(self, near: f64, far: f64) -> Camera {
        Camera { near: Some(near), far: Some(far), ..self }
    }

    pub fn distance(&self) -> f64 {
        (self.target - self.eye).length()
    }

    /// The way the eye looks.
    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye).try_normalized().unwrap_or(-Vec3::Z)
    }

    /// The eye's right hand.
    pub fn right(&self) -> Vec3 {
        let forward = self.forward();
        forward.cross(self.up).try_normalized().unwrap_or_else(|| forward.any_orthogonal())
    }

    /// The eye's own up, at a right angle to where it looks.
    pub fn upward(&self) -> Vec3 {
        self.right().cross(self.forward())
    }

    /// The meters the view shows from bottom to top at the target.
    pub fn view_height(&self) -> f64 {
        match self.lens {
            Lens::Perspective { fov_y_degrees } => 2.0 * self.distance() * (fov_y_degrees.to_radians() * 0.5).tan(),
            Lens::Parallel { height } => height,
        }
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at(self.eye, self.target, self.up)
    }

    /// The projection for a box of `aspect` (width ÷ height), seeing from
    /// `near` to `far` meters.
    pub fn projection(&self, aspect: f64, near: f64, far: f64) -> Mat4 {
        match self.lens {
            Lens::Perspective { fov_y_degrees } => Mat4::perspective(fov_y_degrees.to_radians(), aspect, near, far),
            Lens::Parallel { height } => {
                let (half_h, half_w) = (height * 0.5, height * 0.5 * aspect);
                Mat4::orthographic(-half_w, half_w, -half_h, half_h, near, far)
            }
        }
    }

    // MARK: flights

    /// The camera a moment of its flight shows. Past the end of the
    /// flight it is the destination itself, with no flight left.
    pub fn at(&self, time: f64) -> Camera {
        let Some(flight) = self.flight else { return *self };
        let t = if flight.seconds <= 0.0 { 1.0 } else { ((time - flight.start) / flight.seconds).clamp(0.0, 1.0) };
        if t >= 1.0 {
            return Camera { flight: None, ..*self };
        }
        let s = t * t * (3.0 - 2.0 * t);
        let target = flight.from_target.lerp(self.target, s);
        let eye = match (spherical(flight.from_eye - flight.from_target), spherical(self.eye - self.target)) {
            (Some(a), Some(b)) if self.up == Vec3::Y => {
                // around the target, the short way: a turn stays a turn
                let mut yaw = b.0 - a.0;
                if yaw > std::f64::consts::PI {
                    yaw -= std::f64::consts::TAU;
                } else if yaw < -std::f64::consts::PI {
                    yaw += std::f64::consts::TAU;
                }
                let distance = (a.2.ln() + (b.2.ln() - a.2.ln()) * s).exp();
                target + direction_of(a.0 + yaw * s, a.1 + (b.1 - a.1) * s) * distance
            }
            _ => flight.from_eye.lerp(self.eye, s),
        };
        let lens = match (flight.from_lens, self.lens) {
            (Lens::Perspective { fov_y_degrees: a }, Lens::Perspective { fov_y_degrees: b }) => {
                Lens::Perspective { fov_y_degrees: a + (b - a) * s }
            }
            (Lens::Parallel { height: a }, Lens::Parallel { height: b }) => {
                Lens::Parallel { height: (a.ln() + (b.ln() - a.ln()) * s).exp() }
            }
            // a lens of another kind changes at the end
            (from, _) => from,
        };
        Camera { eye, target, lens, ..*self }
    }

    /// Is the camera still on its way at `time`?
    pub fn is_flying(&self, time: f64) -> bool {
        self.flight.is_some_and(|flight| time < flight.start + flight.seconds)
    }

    /// `to`, reached in `seconds` from where this camera stands now. The
    /// path curves around the target, so a turn of the view stays a turn.
    pub fn fly_to(self, to: Camera, seconds: f64) -> Camera {
        let now = motor::task::now();
        let from = self.at(now);
        Camera {
            flight: Some(Flight {
                from_eye: from.eye,
                from_target: from.target,
                from_lens: from.lens,
                start: now,
                seconds: seconds.max(0.0),
            }),
            ..to
        }
    }

    // MARK: the box

    /// Where `point` lands in a box of `size`, in the box's own points
    /// (origin top-left, y down). `None` behind a perspective eye. A
    /// flying camera answers where the flight stands now.
    pub fn project(&self, point: Vec3, size: Size) -> Option<Point> {
        let camera = self.at(motor::task::now());
        let seen = camera.view().transform_point(point);
        let aspect = aspect_of(size);
        let (x, y) = match camera.lens {
            Lens::Perspective { fov_y_degrees } => {
                if seen.z > -1e-9 {
                    return None;
                }
                let focal = 1.0 / (fov_y_degrees.to_radians() * 0.5).tan();
                (seen.x / -seen.z * focal / aspect, seen.y / -seen.z * focal)
            }
            Lens::Parallel { height } => (seen.x / (height * 0.5 * aspect), seen.y / (height * 0.5)),
        };
        Some(Point { x: (x + 1.0) * 0.5 * size.width, y: (1.0 - y) * 0.5 * size.height })
    }

    /// The ray from the eye through the point `at` of a box of `size`.
    /// A parallel camera's rays run side by side, each from the plane of
    /// the eye.
    pub fn ray(&self, at: Point, size: Size) -> Ray {
        let camera = self.at(motor::task::now());
        let aspect = aspect_of(size);
        let x = at.x / size.width.max(1e-9) * 2.0 - 1.0;
        let y = 1.0 - at.y / size.height.max(1e-9) * 2.0;
        let (forward, right, up) = (camera.forward(), camera.right(), camera.upward());
        match camera.lens {
            Lens::Perspective { fov_y_degrees } => {
                let tan = (fov_y_degrees.to_radians() * 0.5).tan();
                let direction = (forward + right * (x * tan * aspect) + up * (y * tan)).normalized();
                Ray::new(camera.eye, direction)
            }
            Lens::Parallel { height } => {
                let origin = camera.eye + right * (x * height * 0.5 * aspect) + up * (y * height * 0.5);
                Ray::new(origin, forward)
            }
        }
    }

    /// The same view direction and lens, moved so that `bounds` fills a
    /// box of `size` with a margin.
    pub fn framing(self, bounds: Aabb, size: Size) -> Camera {
        if bounds.is_empty() {
            return self;
        }
        let aspect = aspect_of(size);
        let center = bounds.center();
        let radius = bounds.half_extents().length().max(1e-6) * 1.05;
        let forward = self.forward();
        match self.lens {
            Lens::Perspective { fov_y_degrees } => {
                let half_y = fov_y_degrees.to_radians() * 0.5;
                let half_x = (half_y.tan() * aspect).atan();
                let distance = radius / half_y.min(half_x).sin();
                Camera { eye: center - forward * distance, target: center, flight: None, ..self }
            }
            Lens::Parallel { .. } => {
                let height = 2.0 * radius * (1.0 / aspect).max(1.0);
                Camera {
                    eye: center - forward * (radius * 2.0),
                    target: center,
                    lens: Lens::Parallel { height },
                    flight: None,
                    ..self
                }
            }
        }
    }

    /// The meters one point of the box spans at `depth` meters in front
    /// of the eye, in a box `height` points tall.
    pub fn meters_per_point(&self, depth: f64, height: f64) -> f64 {
        let view = match self.lens {
            Lens::Perspective { fov_y_degrees } => 2.0 * depth.max(0.0) * (fov_y_degrees.to_radians() * 0.5).tan(),
            Lens::Parallel { height: meters } => meters,
        };
        view / height.max(1e-9)
    }
}

fn aspect_of(size: Size) -> f64 {
    if size.height > 0.0 && size.width > 0.0 { size.width / size.height } else { 1.0 }
}

/// The unit direction from a target toward an eye at `yaw` around the
/// vertical and `pitch` above the horizon.
pub(crate) fn direction_of(yaw: f64, pitch: f64) -> Vec3 {
    vec3(pitch.cos() * yaw.sin(), pitch.sin(), pitch.cos() * yaw.cos())
}

/// `(yaw, pitch, distance)` of an offset from a target; `None` for no
/// offset at all.
pub(crate) fn spherical(offset: Vec3) -> Option<(f64, f64, f64)> {
    let distance = offset.length();
    if distance <= 1e-12 || !distance.is_finite() {
        return None;
    }
    Some((offset.x.atan2(offset.z), (offset.y / distance).clamp(-1.0, 1.0).asin(), distance))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> Size {
        Size { width: 800.0, height: 500.0 }
    }

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn project_and_ray_round_trip() {
        let base = Camera::looking_at(vec3(3.0, 4.0, 6.0), vec3(0.0, 0.5, 0.0));
        for camera in [base, base.parallel()] {
            for point in [vec3(0.3, 0.2, -0.4), vec3(-1.0, 1.5, 0.7), vec3(0.0, 0.5, 0.0)] {
                let at = camera.project(point, size()).expect("in front of the eye");
                let ray = camera.ray(at, size());
                // the point lies on the ray
                let along = (point - ray.origin).dot(ray.direction);
                let off = (ray.at(along) - point).length();
                assert!(off < 1e-9, "{camera:?}: {point:?} is {off} off its ray");
            }
            let center = camera.project(camera.target, size()).unwrap();
            assert!(close(center.x, 400.0, 1e-9) && close(center.y, 250.0, 1e-9), "the target is the middle");
        }
        let behind = base.project(base.eye + base.forward() * -2.0, size());
        assert_eq!(behind, None, "behind a perspective eye there is nothing");
    }

    #[test]
    fn parallel_frames_what_perspective_framed() {
        let camera = Camera::looking_at(vec3(0.0, 2.0, 10.0), vec3(0.0, 2.0, 0.0));
        let flat = camera.parallel();
        // a point on the plane of the target lands on the same spot
        let point = vec3(1.7, 3.1, 0.0);
        let a = camera.project(point, size()).unwrap();
        let b = flat.project(point, size()).unwrap();
        assert!(close(a.x, b.x, 1e-9) && close(a.y, b.y, 1e-9), "{a:?} vs {b:?}");
        assert!(close(flat.view_height(), camera.view_height(), 1e-12));
    }

    #[test]
    fn a_flight_lands_exactly_on_its_target() {
        motor::task::advance(0.0);
        let start = motor::task::now();
        let from = Camera::orbiting(Vec3::ZERO, 10.0, 30.0, 10.0);
        let to = Camera::orbiting(Vec3::ZERO, 100.0, 30.0, 20.0);
        let flying = from.fly_to(to, 0.4);
        assert!(flying.is_flying(start));
        let mid = flying.at(start + 0.2);
        // around the target, not through it
        assert!(close(mid.distance(), (10.0f64 * 20.0).sqrt(), 1e-9), "{}", mid.distance());
        let (yaw, pitch, _) = spherical(mid.eye - mid.target).unwrap();
        assert!(close(yaw.to_degrees(), 55.0, 1e-9) && close(pitch.to_degrees(), 30.0, 1e-9));
        let landed = flying.at(start + 0.4);
        assert_eq!(landed, to, "the end of a flight is the destination, exactly");
        assert!(!flying.is_flying(start + 0.4));
        // a turn across the back takes the short way
        let back = Camera::orbiting(Vec3::ZERO, 170.0, 30.0, 10.0).fly_to(Camera::orbiting(Vec3::ZERO, -170.0, 30.0, 10.0), 1.0);
        let (yaw, _, _) = spherical(back.at(start + 0.5).eye).unwrap();
        assert!(close(yaw.abs().to_degrees(), 180.0, 1e-9), "{}", yaw.to_degrees());
    }

    #[test]
    fn framing_holds_the_box_in_view() {
        let bounds = Aabb::new(vec3(-2.0, 0.0, -1.0), vec3(6.0, 3.0, 1.0));
        for camera in [Camera::looking_at(vec3(1.0, 5.0, 9.0), Vec3::ZERO), Camera::looking_at(vec3(1.0, 5.0, 9.0), Vec3::ZERO).parallel()] {
            let framed = camera.framing(bounds, size());
            for corner in bounds.corners() {
                let at = framed.project(corner, size()).expect("in front");
                assert!(at.x >= 0.0 && at.x <= 800.0 && at.y >= 0.0 && at.y <= 500.0, "{corner:?} at {at:?}");
            }
            assert!((framed.forward() - camera.forward()).length() < 1e-12, "the direction stays");
        }
    }
}
