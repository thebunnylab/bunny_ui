//! The arithmetic of a world: vectors, rotations, matrices, boxes, rays
//! and the six planes a camera sees through.
//!
//! The conventions, once, for every file of the module:
//!
//! - The world is right-handed, Y up, in meters.
//! - A camera looks down its own −Z.
//! - Clip space is the GPU's: X right and Y up in [−1, 1], depth in
//!   [0, 1] with 0 at the near plane. A tier whose framebuffer runs the
//!   other way corrects it in its own vertex stage, never here.
//! - A triangle whose corners turn counter-clockwise on the screen faces
//!   the camera.
//! - Matrices are column-major: `m[col * 4 + row]`, and a matrix
//!   multiplies a column vector on its right.
//!
//! Everything public is `f64`. A world can be large — a site, a city
//! block — and the GPU sees `f32` only after the frame moves each
//! entity into the camera's own neighbourhood.

use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

// MARK: - Vectors

/// A point or a direction in a plane.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// A point or a direction in the world.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A homogeneous point: what a matrix makes of a point before the
/// divide.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec4 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

/// `vec2(x, y)` — the short way to write a [`Vec2`].
pub const fn vec2(x: f64, y: f64) -> Vec2 {
    Vec2 { x, y }
}

/// `vec3(x, y, z)` — the short way to write a [`Vec3`].
pub const fn vec3(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3 { x, y, z }
}

/// `vec4(x, y, z, w)` — the short way to write a [`Vec4`].
pub const fn vec4(x: f64, y: f64, z: f64, w: f64) -> Vec4 {
    Vec4 { x, y, z, w }
}

impl Vec2 {
    pub const ZERO: Vec2 = vec2(0.0, 0.0);

    pub fn dot(self, other: Vec2) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// The z of the 3D cross product: positive when `other` turns
    /// counter-clockwise from `self`.
    pub fn cross(self, other: Vec2) -> f64 {
        self.x * other.y - self.y * other.x
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
}

impl Vec3 {
    pub const ZERO: Vec3 = vec3(0.0, 0.0, 0.0);
    pub const ONE: Vec3 = vec3(1.0, 1.0, 1.0);
    pub const X: Vec3 = vec3(1.0, 0.0, 0.0);
    pub const Y: Vec3 = vec3(0.0, 1.0, 0.0);
    pub const Z: Vec3 = vec3(0.0, 0.0, 1.0);

    /// The same value in every component.
    pub const fn splat(value: f64) -> Vec3 {
        vec3(value, value, value)
    }

    pub fn dot(self, other: Vec3) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// The right-handed cross product: `X.cross(Y) == Z`.
    pub fn cross(self, other: Vec3) -> Vec3 {
        vec3(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    pub fn distance(self, other: Vec3) -> f64 {
        (self - other).length()
    }

    /// The direction of the vector, one meter long. `None` for a vector
    /// too short to have a direction.
    pub fn try_normalized(self) -> Option<Vec3> {
        let length = self.length();
        (length > 1e-300 && length.is_finite()).then(|| self / length)
    }

    /// The direction of the vector, one meter long; the zero vector
    /// stays zero.
    pub fn normalized(self) -> Vec3 {
        self.try_normalized().unwrap_or(Vec3::ZERO)
    }

    /// The point a fraction `t` of the way from `self` to `other`.
    pub fn lerp(self, other: Vec3, t: f64) -> Vec3 {
        self + (other - self) * t
    }

    pub fn min(self, other: Vec3) -> Vec3 {
        vec3(self.x.min(other.x), self.y.min(other.y), self.z.min(other.z))
    }

    pub fn max(self, other: Vec3) -> Vec3 {
        vec3(self.x.max(other.x), self.y.max(other.y), self.z.max(other.z))
    }

    pub fn abs(self) -> Vec3 {
        vec3(self.x.abs(), self.y.abs(), self.z.abs())
    }

    /// Component by component.
    pub fn scaled(self, other: Vec3) -> Vec3 {
        vec3(self.x * other.x, self.y * other.y, self.z * other.z)
    }

    /// The largest component.
    pub fn max_element(self) -> f64 {
        self.x.max(self.y).max(self.z)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Some unit vector at a right angle to this one — the start of a
    /// frame around a direction.
    pub fn any_orthogonal(self) -> Vec3 {
        let helper = if self.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        self.cross(helper).normalized()
    }

    /// The same vector in `f32`, for the wire.
    pub fn to_f32(self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }

    pub fn extend(self, w: f64) -> Vec4 {
        vec4(self.x, self.y, self.z, w)
    }
}

impl Vec4 {
    pub fn truncate(self) -> Vec3 {
        vec3(self.x, self.y, self.z)
    }

    pub fn dot(self, other: Vec4) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }
}

macro_rules! vector_ops {
    ($vector:ident { $($field:ident),+ }) => {
        impl Add for $vector {
            type Output = $vector;
            fn add(self, other: $vector) -> $vector {
                $vector { $($field: self.$field + other.$field),+ }
            }
        }
        impl Sub for $vector {
            type Output = $vector;
            fn sub(self, other: $vector) -> $vector {
                $vector { $($field: self.$field - other.$field),+ }
            }
        }
        impl Mul<f64> for $vector {
            type Output = $vector;
            fn mul(self, scale: f64) -> $vector {
                $vector { $($field: self.$field * scale),+ }
            }
        }
        impl Mul<$vector> for f64 {
            type Output = $vector;
            fn mul(self, vector: $vector) -> $vector {
                vector * self
            }
        }
        impl Div<f64> for $vector {
            type Output = $vector;
            fn div(self, divisor: f64) -> $vector {
                $vector { $($field: self.$field / divisor),+ }
            }
        }
        impl Neg for $vector {
            type Output = $vector;
            fn neg(self) -> $vector {
                $vector { $($field: -self.$field),+ }
            }
        }
        impl AddAssign for $vector {
            fn add_assign(&mut self, other: $vector) {
                *self = *self + other;
            }
        }
        impl SubAssign for $vector {
            fn sub_assign(&mut self, other: $vector) {
                *self = *self - other;
            }
        }
        impl MulAssign<f64> for $vector {
            fn mul_assign(&mut self, scale: f64) {
                *self = *self * scale;
            }
        }
    };
}

vector_ops!(Vec2 { x, y });
vector_ops!(Vec3 { x, y, z });
vector_ops!(Vec4 { x, y, z, w });

// MARK: - Rotations

/// A rotation, as a unit quaternion. `(x, y, z)` is the axis times the
/// sine of half the angle, `w` the cosine of half the angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Default for Quat {
    fn default() -> Quat {
        Quat::IDENTITY
    }
}

impl Quat {
    /// No rotation.
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    /// A turn of `radians` around `axis`, counter-clockwise when the axis
    /// points at the eye. A zero axis gives no rotation.
    pub fn from_axis_angle(axis: Vec3, radians: f64) -> Quat {
        let Some(axis) = axis.try_normalized() else { return Quat::IDENTITY };
        let (sin, cos) = (radians * 0.5).sin_cos();
        Quat { x: axis.x * sin, y: axis.y * sin, z: axis.z * sin, w: cos }
    }

    /// A turn around the vertical axis.
    pub fn from_yaw(radians: f64) -> Quat {
        Quat::from_axis_angle(Vec3::Y, radians)
    }

    /// The shortest rotation that takes the direction `from` onto the
    /// direction `to`.
    pub fn from_arc(from: Vec3, to: Vec3) -> Quat {
        let (Some(from), Some(to)) = (from.try_normalized(), to.try_normalized()) else {
            return Quat::IDENTITY;
        };
        let cos = from.dot(to);
        if cos < -1.0 + 1e-12 {
            // opposite directions: half a turn around any perpendicular
            return Quat::from_axis_angle(from.any_orthogonal(), std::f64::consts::PI);
        }
        let axis = from.cross(to);
        Quat { x: axis.x, y: axis.y, z: axis.z, w: 1.0 + cos }.normalized()
    }

    pub fn dot(self, other: Quat) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// The same rotation with a length of exactly one. A quaternion of
    /// zero length gives no rotation.
    pub fn normalized(self) -> Quat {
        let length = self.length();
        if length <= 1e-300 || !length.is_finite() {
            return Quat::IDENTITY;
        }
        Quat { x: self.x / length, y: self.y / length, z: self.z / length, w: self.w / length }
    }

    /// The opposite rotation (for a unit quaternion).
    pub fn inverse(self) -> Quat {
        Quat { x: -self.x, y: -self.y, z: -self.z, w: self.w }
    }

    /// Turns a vector.
    pub fn rotate(self, vector: Vec3) -> Vec3 {
        // v + 2w(q × v) + 2 q × (q × v)
        let q = vec3(self.x, self.y, self.z);
        let t = q.cross(vector) * 2.0;
        vector + t * self.w + q.cross(t)
    }

    /// The rotation part of a turn: the angle in radians, in [0, π].
    pub fn angle(self) -> f64 {
        2.0 * self.w.abs().min(1.0).acos()
    }

    /// Interpolates on the sphere, `t` in [0, 1], along the SHORT way:
    /// `q` and `−q` are the same rotation, and the path takes the one
    /// closer to `self`.
    pub fn slerp(self, other: Quat, t: f64) -> Quat {
        let mut cos = self.dot(other);
        let mut other = other;
        if cos < 0.0 {
            other = Quat { x: -other.x, y: -other.y, z: -other.z, w: -other.w };
            cos = -cos;
        }
        if cos > 1.0 - 1e-9 {
            // too close for the sine to divide by: the straight line is exact enough
            return Quat {
                x: self.x + (other.x - self.x) * t,
                y: self.y + (other.y - self.y) * t,
                z: self.z + (other.z - self.z) * t,
                w: self.w + (other.w - self.w) * t,
            }
            .normalized();
        }
        let angle = cos.acos();
        let sin = angle.sin();
        let a = ((1.0 - t) * angle).sin() / sin;
        let b = (t * angle).sin() / sin;
        Quat {
            x: self.x * a + other.x * b,
            y: self.y * a + other.y * b,
            z: self.z * a + other.z * b,
            w: self.w * a + other.w * b,
        }
    }
}

impl Mul for Quat {
    type Output = Quat;

    /// `a * b` turns by `b` first, then by `a`.
    fn mul(self, b: Quat) -> Quat {
        let a = self;
        Quat {
            x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
            w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        }
    }
}

// MARK: - Matrices

/// A 4×4 matrix, column-major: `m[col * 4 + row]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub m: [f64; 16],
}

impl Default for Mat4 {
    fn default() -> Mat4 {
        Mat4::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4 {
        m: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    };

    /// The entry at `row`, `col`.
    pub fn at(&self, row: usize, col: usize) -> f64 {
        self.m[col * 4 + row]
    }

    pub fn from_cols(c0: Vec4, c1: Vec4, c2: Vec4, c3: Vec4) -> Mat4 {
        Mat4 {
            m: [
                c0.x, c0.y, c0.z, c0.w, c1.x, c1.y, c1.z, c1.w, c2.x, c2.y, c2.z, c2.w, c3.x,
                c3.y, c3.z, c3.w,
            ],
        }
    }

    pub fn col(&self, col: usize) -> Vec4 {
        let c = col * 4;
        vec4(self.m[c], self.m[c + 1], self.m[c + 2], self.m[c + 3])
    }

    pub fn row(&self, row: usize) -> Vec4 {
        vec4(self.m[row], self.m[4 + row], self.m[8 + row], self.m[12 + row])
    }

    pub fn translation(offset: Vec3) -> Mat4 {
        let mut matrix = Mat4::IDENTITY;
        matrix.m[12] = offset.x;
        matrix.m[13] = offset.y;
        matrix.m[14] = offset.z;
        matrix
    }

    pub fn scale(factors: Vec3) -> Mat4 {
        let mut matrix = Mat4::IDENTITY;
        matrix.m[0] = factors.x;
        matrix.m[5] = factors.y;
        matrix.m[10] = factors.z;
        matrix
    }

    pub fn rotation(rotation: Quat) -> Mat4 {
        let q = rotation.normalized();
        let (x2, y2, z2) = (q.x + q.x, q.y + q.y, q.z + q.z);
        let (xx, xy, xz) = (q.x * x2, q.x * y2, q.x * z2);
        let (yy, yz, zz) = (q.y * y2, q.y * z2, q.z * z2);
        let (wx, wy, wz) = (q.w * x2, q.w * y2, q.w * z2);
        Mat4 {
            m: [
                1.0 - (yy + zz),
                xy + wz,
                xz - wy,
                0.0,
                xy - wz,
                1.0 - (xx + zz),
                yz + wx,
                0.0,
                xz + wy,
                yz - wx,
                1.0 - (xx + yy),
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ],
        }
    }

    /// Scale, then rotation, then translation — what a [`Transform`]
    /// does to a point.
    pub fn from_trs(translation: Vec3, rotation: Quat, scale: Vec3) -> Mat4 {
        let mut matrix = Mat4::rotation(rotation);
        for (col, factor) in [scale.x, scale.y, scale.z].into_iter().enumerate() {
            for row in 0..3 {
                matrix.m[col * 4 + row] *= factor;
            }
        }
        matrix.m[12] = translation.x;
        matrix.m[13] = translation.y;
        matrix.m[14] = translation.z;
        matrix
    }

    /// The view matrix of an eye at `eye` that looks at `target`, its
    /// up toward `up`: the world as the camera sees it, the camera at
    /// the origin looking down −Z. When `up` runs along the line of
    /// sight, a perpendicular stands in for it.
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
        let forward = (target - eye).try_normalized().unwrap_or(-Vec3::Z);
        let side = forward.cross(up).try_normalized().unwrap_or_else(|| forward.any_orthogonal());
        let up = side.cross(forward);
        Mat4 {
            m: [
                side.x,
                up.x,
                -forward.x,
                0.0,
                side.y,
                up.y,
                -forward.y,
                0.0,
                side.z,
                up.z,
                -forward.z,
                0.0,
                -side.dot(eye),
                -up.dot(eye),
                forward.dot(eye),
                1.0,
            ],
        }
    }

    /// A perspective projection: `fov_y` radians from the bottom of the
    /// view to its top, `aspect` = width ÷ height. A point on the near
    /// plane lands at depth 0, a point on the far plane at depth 1.
    pub fn perspective(fov_y: f64, aspect: f64, near: f64, far: f64) -> Mat4 {
        let focal = 1.0 / (fov_y * 0.5).tan();
        let mut matrix = Mat4 { m: [0.0; 16] };
        matrix.m[0] = focal / aspect;
        matrix.m[5] = focal;
        matrix.m[10] = far / (near - far);
        matrix.m[11] = -1.0;
        matrix.m[14] = near * far / (near - far);
        matrix
    }

    /// A parallel projection of the box from (`left`, `bottom`) to
    /// (`right`, `top`), seen from `near` to `far` meters in front of
    /// the eye. Depth runs from 0 at `near` to 1 at `far`.
    pub fn orthographic(left: f64, right: f64, bottom: f64, top: f64, near: f64, far: f64) -> Mat4 {
        let mut matrix = Mat4::IDENTITY;
        matrix.m[0] = 2.0 / (right - left);
        matrix.m[5] = 2.0 / (top - bottom);
        matrix.m[10] = -1.0 / (far - near);
        matrix.m[12] = -(right + left) / (right - left);
        matrix.m[13] = -(top + bottom) / (top - bottom);
        matrix.m[14] = -near / (far - near);
        matrix
    }

    pub fn transpose(&self) -> Mat4 {
        let mut out = Mat4 { m: [0.0; 16] };
        for row in 0..4 {
            for col in 0..4 {
                out.m[row * 4 + col] = self.m[col * 4 + row];
            }
        }
        out
    }

    /// The full product with a homogeneous point.
    pub fn transform(&self, point: Vec4) -> Vec4 {
        let m = &self.m;
        vec4(
            m[0] * point.x + m[4] * point.y + m[8] * point.z + m[12] * point.w,
            m[1] * point.x + m[5] * point.y + m[9] * point.z + m[13] * point.w,
            m[2] * point.x + m[6] * point.y + m[10] * point.z + m[14] * point.w,
            m[3] * point.x + m[7] * point.y + m[11] * point.z + m[15] * point.w,
        )
    }

    /// A point through an AFFINE matrix: the bottom row is ignored.
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        let m = &self.m;
        vec3(
            m[0] * point.x + m[4] * point.y + m[8] * point.z + m[12],
            m[1] * point.x + m[5] * point.y + m[9] * point.z + m[13],
            m[2] * point.x + m[6] * point.y + m[10] * point.z + m[14],
        )
    }

    /// A point through any matrix, divided by its `w` — a projection.
    /// `None` when the point lands at `w` = 0.
    pub fn project_point(&self, point: Vec3) -> Option<Vec3> {
        let clip = self.transform(point.extend(1.0));
        (clip.w.abs() > 1e-300).then(|| clip.truncate() / clip.w)
    }

    /// A direction through the matrix: the translation does not move it.
    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        let m = &self.m;
        vec3(
            m[0] * vector.x + m[4] * vector.y + m[8] * vector.z,
            m[1] * vector.x + m[5] * vector.y + m[9] * vector.z,
            m[2] * vector.x + m[6] * vector.y + m[10] * vector.z,
        )
    }

    /// Where the matrix puts the origin.
    pub fn translation_part(&self) -> Vec3 {
        vec3(self.m[12], self.m[13], self.m[14])
    }

    pub fn determinant(&self) -> f64 {
        self.cofactors().1
    }

    /// The matrix that undoes this one. `None` for a matrix that
    /// collapses space (a zero scale, a degenerate projection).
    pub fn inverse(&self) -> Option<Mat4> {
        let (adjugate, det) = self.cofactors();
        if det.abs() <= f64::MIN_POSITIVE || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        let mut out = Mat4 { m: [0.0; 16] };
        for (slot, value) in out.m.iter_mut().zip(adjugate.iter()) {
            *slot = value * inv;
        }
        Some(out)
    }

    /// The adjugate (transposed cofactors, column-major) and the
    /// determinant, by 2×2 sub-determinants.
    fn cofactors(&self) -> ([f64; 16], f64) {
        let m = &self.m;
        // rows of the matrix: a = row 0 … d = row 3, read column-major
        let (a0, a1, a2, a3) = (m[0], m[4], m[8], m[12]);
        let (b0, b1, b2, b3) = (m[1], m[5], m[9], m[13]);
        let (c0, c1, c2, c3) = (m[2], m[6], m[10], m[14]);
        let (d0, d1, d2, d3) = (m[3], m[7], m[11], m[15]);

        let s0 = a0 * b1 - b0 * a1;
        let s1 = a0 * b2 - b0 * a2;
        let s2 = a0 * b3 - b0 * a3;
        let s3 = a1 * b2 - b1 * a2;
        let s4 = a1 * b3 - b1 * a3;
        let s5 = a2 * b3 - b2 * a3;

        let c5 = c2 * d3 - d2 * c3;
        let c4 = c1 * d3 - d1 * c3;
        let c3_ = c1 * d2 - d1 * c2;
        let c2_ = c0 * d3 - d0 * c3;
        let c1_ = c0 * d2 - d0 * c2;
        let c0_ = c0 * d1 - d0 * c1;

        let det = s0 * c5 - s1 * c4 + s2 * c3_ + s3 * c2_ - s4 * c1_ + s5 * c0_;

        // the inverse's rows (r0 … r3), each from the cofactors
        let r0 = [
            b1 * c5 - b2 * c4 + b3 * c3_,
            -a1 * c5 + a2 * c4 - a3 * c3_,
            d1 * s5 - d2 * s4 + d3 * s3,
            -c1 * s5 + c2 * s4 - c3 * s3,
        ];
        let r1 = [
            -b0 * c5 + b2 * c2_ - b3 * c1_,
            a0 * c5 - a2 * c2_ + a3 * c1_,
            -d0 * s5 + d2 * s2 - d3 * s1,
            c0 * s5 - c2 * s2 + c3 * s1,
        ];
        let r2 = [
            b0 * c4 - b1 * c2_ + b3 * c0_,
            -a0 * c4 + a1 * c2_ - a3 * c0_,
            d0 * s4 - d1 * s2 + d3 * s0,
            -c0 * s4 + c1 * s2 - c3 * s0,
        ];
        let r3 = [
            -b0 * c3_ + b1 * c1_ - b2 * c0_,
            a0 * c3_ - a1 * c1_ + a2 * c0_,
            -d0 * s3 + d1 * s1 - d2 * s0,
            c0 * s3 - c1 * s1 + c2 * s0,
        ];
        // back to column-major: entry (row, col) at m[col * 4 + row]
        let mut adjugate = [0.0; 16];
        for (row, values) in [r0, r1, r2, r3].iter().enumerate() {
            for (col, value) in values.iter().enumerate() {
                adjugate[col * 4 + row] = *value;
            }
        }
        (adjugate, det)
    }

    /// The matrix for normals: the inverse transposed of the upper 3×3.
    /// It keeps a normal at a right angle to its surface under a scale
    /// that is not the same on each axis. Rows of the result: `[row0,
    /// row1, row2]`, each three numbers.
    pub fn normal_matrix(&self) -> [[f64; 3]; 3] {
        let a = |row: usize, col: usize| self.m[col * 4 + row];
        // cofactor matrix of the 3×3 = det · inverse-transposed
        let c00 = a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1);
        let c01 = a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2);
        let c02 = a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0);
        let c10 = a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2);
        let c11 = a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0);
        let c12 = a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1);
        let c20 = a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1);
        let c21 = a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2);
        let c22 = a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0);
        let det = a(0, 0) * c00 + a(0, 1) * c01 + a(0, 2) * c02;
        // the sign of the determinant keeps mirrored instances facing out;
        // the magnitude does not matter, the shader normalizes
        let sign = if det < 0.0 { -1.0 } else { 1.0 };
        [
            [c00 * sign, c01 * sign, c02 * sign],
            [c10 * sign, c11 * sign, c12 * sign],
            [c20 * sign, c21 * sign, c22 * sign],
        ]
    }

    /// The matrix in `f32`, column-major, for the wire.
    pub fn to_f32(&self) -> [f32; 16] {
        let mut out = [0.0f32; 16];
        for (slot, value) in out.iter_mut().zip(self.m.iter()) {
            *slot = *value as f32;
        }
        out
    }
}

impl Mul for Mat4 {
    type Output = Mat4;

    /// `a * b` applies `b` first, then `a`.
    fn mul(self, other: Mat4) -> Mat4 {
        let mut out = Mat4 { m: [0.0; 16] };
        for col in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += self.m[k * 4 + row] * other.m[col * 4 + k];
                }
                out.m[col * 4 + row] = sum;
            }
        }
        out
    }
}

// MARK: - Placement

/// Where an entity stands: a scale, then a rotation, then a
/// translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Transform {
        Transform::IDENTITY
    }
}

impl Transform {
    /// In place, unturned, at its own size.
    pub const IDENTITY: Transform =
        Transform { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    pub fn from_translation(translation: Vec3) -> Transform {
        Transform { translation, ..Transform::IDENTITY }
    }

    pub fn with_rotation(self, rotation: Quat) -> Transform {
        Transform { rotation, ..self }
    }

    pub fn with_scale(self, scale: Vec3) -> Transform {
        Transform { scale, ..self }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_trs(self.translation, self.rotation, self.scale)
    }

    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        self.rotation.rotate(point.scaled(self.scale)) + self.translation
    }

    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        self.rotation.rotate(vector.scaled(self.scale))
    }
}

// MARK: - Boxes, rays, planes

/// An axis-aligned box. The empty box has `min` above `max`, and grows
/// into a real one with the first point it includes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Aabb {
    fn default() -> Aabb {
        Aabb::EMPTY
    }
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: Vec3::splat(f64::INFINITY),
        max: Vec3::splat(f64::NEG_INFINITY),
    };

    pub fn new(min: Vec3, max: Vec3) -> Aabb {
        Aabb { min: min.min(max), max: min.max(max) }
    }

    /// The smallest box around the points (empty for no points).
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Aabb {
        points.into_iter().fold(Aabb::EMPTY, Aabb::including)
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    pub fn including(self, point: Vec3) -> Aabb {
        Aabb { min: self.min.min(point), max: self.max.max(point) }
    }

    pub fn union(self, other: Aabb) -> Aabb {
        if other.is_empty() {
            return self;
        }
        Aabb { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Half the size on each axis.
    pub fn half_extents(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            vec3(a.x, a.y, a.z),
            vec3(b.x, a.y, a.z),
            vec3(a.x, b.y, a.z),
            vec3(b.x, b.y, a.z),
            vec3(a.x, a.y, b.z),
            vec3(b.x, a.y, b.z),
            vec3(a.x, b.y, b.z),
            vec3(b.x, b.y, b.z),
        ]
    }

    pub fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }

    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    /// The box around this box after an affine matrix — exact for the
    /// corners, so a turned box grows to hold its own corners.
    pub fn transformed(&self, matrix: &Mat4) -> Aabb {
        if self.is_empty() {
            return Aabb::EMPTY;
        }
        // Arvo's method: per output axis, the min/max of each column term
        let mut min = matrix.translation_part();
        let mut max = min;
        for col in 0..3 {
            for row in 0..3 {
                let factor = matrix.m[col * 4 + row];
                let (lo, hi) = (factor * axis(self.min, col), factor * axis(self.max, col));
                let (lo, hi) = if lo < hi { (lo, hi) } else { (hi, lo) };
                let (low, high) = (axis(min, row) + lo, axis(max, row) + hi);
                set_axis(&mut min, row, low);
                set_axis(&mut max, row, high);
            }
        }
        Aabb { min, max }
    }
}

fn axis(vector: Vec3, index: usize) -> f64 {
    match index {
        0 => vector.x,
        1 => vector.y,
        _ => vector.z,
    }
}

fn set_axis(vector: &mut Vec3, index: usize, value: f64) {
    match index {
        0 => vector.x = value,
        1 => vector.y = value,
        _ => vector.z = value,
    }
}

/// A half line: from `origin`, along `direction`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, direction: Vec3) -> Ray {
        Ray { origin, direction }
    }

    pub fn at(&self, t: f64) -> Vec3 {
        self.origin + self.direction * t
    }

    /// Where the ray enters and leaves the box, as distances along it:
    /// `Some((enter, leave))` with `leave ≥ max(enter, 0)`. A ray that
    /// starts inside enters at a negative distance.
    pub fn hits_box(&self, aabb: &Aabb) -> Option<(f64, f64)> {
        let mut enter = f64::NEG_INFINITY;
        let mut leave = f64::INFINITY;
        for index in 0..3 {
            let origin = axis(self.origin, index);
            let direction = axis(self.direction, index);
            let (lo, hi) = (axis(aabb.min, index), axis(aabb.max, index));
            if direction.abs() < 1e-300 {
                if origin < lo || origin > hi {
                    return None;
                }
                continue;
            }
            let inv = 1.0 / direction;
            let (mut t0, mut t1) = ((lo - origin) * inv, (hi - origin) * inv);
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            enter = enter.max(t0);
            leave = leave.min(t1);
            if enter > leave {
                return None;
            }
        }
        (leave >= 0.0).then_some((enter, leave))
    }

    /// The distance along the ray where it meets the plane, ahead of
    /// the origin. `None` for a ray parallel to the plane or one that
    /// points away from it.
    pub fn hits_plane(&self, plane: &Plane) -> Option<f64> {
        let facing = plane.normal.dot(self.direction);
        if facing.abs() < 1e-300 {
            return None;
        }
        let t = -plane.signed_distance(self.origin) / facing;
        (t >= 0.0).then_some(t)
    }
}

/// The plane of the points `p` with `normal · p + d = 0`. The side the
/// normal points to is in FRONT.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub d: f64,
}

impl Plane {
    /// The plane through `point`, facing along `normal`.
    pub fn new(point: Vec3, normal: Vec3) -> Plane {
        let normal = normal.normalized();
        Plane { normal, d: -normal.dot(point) }
    }

    /// Positive in front of the plane, negative behind, in meters when
    /// the normal has unit length.
    pub fn signed_distance(&self, point: Vec3) -> f64 {
        self.normal.dot(point) + self.d
    }

    /// The same plane with a unit normal.
    pub fn normalized(self) -> Plane {
        let length = self.normal.length();
        if length <= 1e-300 {
            return self;
        }
        Plane { normal: self.normal / length, d: self.d / length }
    }
}

/// The six planes that bound what a camera sees, normals pointing IN.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    /// The planes of a clip-from-world matrix, with depth in [0, 1].
    pub fn from_matrix(clip_from_world: &Mat4) -> Frustum {
        let r0 = clip_from_world.row(0);
        let r1 = clip_from_world.row(1);
        let r2 = clip_from_world.row(2);
        let r3 = clip_from_world.row(3);
        let plane = |v: Vec4| Plane { normal: v.truncate(), d: v.w }.normalized();
        Frustum {
            planes: [
                plane(r3 + r0), // left:   x ≥ −w
                plane(r3 - r0), // right:  x ≤ w
                plane(r3 + r1), // bottom: y ≥ −w
                plane(r3 - r1), // top:    y ≤ w
                plane(r2),      // near:   z ≥ 0
                plane(r3 - r2), // far:    z ≤ w
            ],
        }
    }

    /// `false` only when the box lies wholly behind one plane: a box
    /// this says `true` for may still be out of sight, but a box it says
    /// `false` for never is.
    pub fn may_see(&self, aabb: &Aabb) -> bool {
        if aabb.is_empty() {
            return false;
        }
        let center = aabb.center();
        let half = aabb.half_extents();
        self.planes.iter().all(|plane| {
            let reach = half.dot(plane.normal.abs());
            plane.signed_distance(center) >= -reach
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= EPS * (1.0 + a.abs().max(b.abs()))
    }

    fn close3(a: Vec3, b: Vec3) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.z, b.z)
    }

    fn close_mat(a: &Mat4, b: &Mat4) -> bool {
        a.m.iter().zip(b.m.iter()).all(|(x, y)| close(*x, *y))
    }

    /// A seeded xorshift: the same numbers on every machine.
    struct Seed(u64);

    impl Seed {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    #[test]
    fn a_matrix_and_its_inverse_compose_to_identity() {
        let mut seed = Seed(0x9E37_79B9_7F4A_7C15);
        for _ in 0..200 {
            let rotation = Quat::from_axis_angle(
                vec3(seed.next(), seed.next(), seed.next()),
                seed.next() * 3.0,
            );
            let matrix = Mat4::from_trs(
                vec3(seed.next() * 50.0, seed.next() * 50.0, seed.next() * 50.0),
                rotation,
                vec3(0.5 + seed.next().abs(), 0.5 + seed.next().abs(), 0.5 + seed.next().abs()),
            );
            let inverse = matrix.inverse().expect("a scale above zero inverts");
            assert!(close_mat(&(matrix * inverse), &Mat4::IDENTITY), "{:?}", matrix * inverse);
            assert!(close_mat(&(inverse * matrix), &Mat4::IDENTITY));
        }
        let projection = Mat4::perspective(1.0, 1.5, 0.1, 100.0);
        let inverse = projection.inverse().expect("a projection inverts");
        assert!(close_mat(&(projection * inverse), &Mat4::IDENTITY));
        assert_eq!(Mat4::scale(vec3(1.0, 0.0, 1.0)).inverse(), None, "a flat scale has no inverse");
    }

    #[test]
    fn look_at_sees_the_target_at_the_center() {
        let eye = vec3(4.0, 3.0, 5.0);
        let target = vec3(-1.0, 0.5, 2.0);
        let view = Mat4::look_at(eye, target, Vec3::Y);
        let seen = view.transform_point(target);
        assert!(close(seen.x, 0.0) && close(seen.y, 0.0), "on the line of sight: {seen:?}");
        assert!(seen.z < 0.0, "in front of the eye, down −Z: {seen:?}");
        assert!(close(-seen.z, eye.distance(target)), "at its distance: {seen:?}");
        assert!(close3(view.transform_point(eye), Vec3::ZERO), "the eye is the origin");
        // up stays up: a point above the target lands above the center
        assert!(view.transform_point(target + Vec3::Y).y > 0.0);
        // straight down the vertical: up runs along the sight, a stand-in steps in
        let down = Mat4::look_at(vec3(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::Y);
        assert!(down.m.iter().all(|v| v.is_finite()), "no NaN from a vertical sight");
        assert!(close(down.transform_point(Vec3::ZERO).z, -10.0));
    }

    #[test]
    fn the_projection_maps_near_to_zero_and_far_to_one() {
        let (near, far) = (0.25, 400.0);
        let perspective = Mat4::perspective(60f64.to_radians(), 16.0 / 9.0, near, far);
        let depth = |m: &Mat4, z: f64| m.project_point(vec3(0.0, 0.0, -z)).unwrap().z;
        assert!(close(depth(&perspective, near), 0.0));
        assert!(close(depth(&perspective, far), 1.0));
        assert!(depth(&perspective, 10.0) > depth(&perspective, 5.0), "farther is deeper");
        // the top edge of the field of view lands at y = 1
        let half = 30f64.to_radians();
        let top = perspective.project_point(vec3(0.0, 7.0 * half.tan(), -7.0)).unwrap();
        assert!(close(top.y, 1.0), "{top:?}");

        let parallel = Mat4::orthographic(-4.0, 4.0, -3.0, 3.0, near, far);
        assert!(close(depth(&parallel, near), 0.0));
        assert!(close(depth(&parallel, far), 1.0));
        let corner = parallel.project_point(vec3(4.0, 3.0, -1.0)).unwrap();
        assert!(close(corner.x, 1.0) && close(corner.y, 1.0), "{corner:?}");
    }

    #[test]
    fn slerp_takes_the_short_way() {
        let a = Quat::from_yaw(10f64.to_radians());
        let b = Quat::from_yaw(350f64.to_radians());
        // the same rotation as b written the long way around
        let negated = Quat { x: -b.x, y: -b.y, z: -b.z, w: -b.w };
        for other in [b, negated] {
            let mid = a.slerp(other, 0.5);
            let forward = mid.rotate(Vec3::Z);
            assert!(close3(forward, Vec3::Z), "halfway between ±10° is 0°, not 180°: {forward:?}");
        }
        assert!(close3(a.slerp(b, 0.0).rotate(Vec3::X), a.rotate(Vec3::X)));
        assert!(close3(a.slerp(b, 1.0).rotate(Vec3::X), b.rotate(Vec3::X)));
        // two quarter turns about Y make a half turn
        let quarter = Quat::from_yaw(std::f64::consts::FRAC_PI_2);
        assert!(close3((quarter * quarter).rotate(Vec3::X), -Vec3::X));
        assert!(close3(quarter.rotate(Vec3::X), -Vec3::Z), "counter-clockwise seen from above");
    }

    #[test]
    fn a_rotation_matrix_agrees_with_the_quaternion() {
        let mut seed = Seed(42);
        for _ in 0..100 {
            let q = Quat::from_axis_angle(vec3(seed.next(), seed.next(), seed.next()), seed.next() * 3.0);
            let v = vec3(seed.next(), seed.next(), seed.next()) * 10.0;
            assert!(close3(Mat4::rotation(q).transform_vector(v), q.rotate(v)));
        }
        let arc = Quat::from_arc(Vec3::X, Vec3::Y);
        assert!(close3(arc.rotate(Vec3::X), Vec3::Y));
        let flip = Quat::from_arc(Vec3::Z, -Vec3::Z);
        assert!(close3(flip.rotate(Vec3::Z), -Vec3::Z), "opposite directions still turn");
    }

    #[test]
    fn a_transform_and_its_matrix_place_points_alike() {
        let transform = Transform::from_translation(vec3(1.0, 2.0, 3.0))
            .with_rotation(Quat::from_axis_angle(vec3(1.0, 1.0, 0.0), 0.7))
            .with_scale(vec3(2.0, 0.5, 3.0));
        let point = vec3(0.3, -1.2, 4.0);
        assert!(close3(transform.matrix().transform_point(point), transform.transform_point(point)));
    }

    #[test]
    fn the_normal_matrix_keeps_normals_at_right_angles() {
        // a squashed box: the plane through x = y still meets its normal at 90°
        let matrix = Mat4::scale(vec3(1.0, 4.0, 1.0));
        let tangent = matrix.transform_vector(vec3(1.0, 1.0, 0.0));
        let n = matrix.normal_matrix();
        let normal = vec3(1.0, -1.0, 0.0);
        let turned = vec3(
            n[0][0] * normal.x + n[0][1] * normal.y + n[0][2] * normal.z,
            n[1][0] * normal.x + n[1][1] * normal.y + n[1][2] * normal.z,
            n[2][0] * normal.x + n[2][1] * normal.y + n[2][2] * normal.z,
        );
        assert!(close(turned.dot(tangent), 0.0), "{turned:?} · {tangent:?}");
        // a mirror keeps the normal pointing out
        let mirror = Mat4::scale(vec3(-1.0, 1.0, 1.0)).normal_matrix();
        assert!(mirror[0][0] < 0.0, "the outward x normal follows the flip: {mirror:?}");
    }

    #[test]
    fn a_box_outside_the_frustum_is_culled() {
        let view = Mat4::look_at(vec3(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y);
        let clip = Mat4::perspective(50f64.to_radians(), 1.0, 0.5, 50.0) * view;
        let frustum = Frustum::from_matrix(&clip);
        let unit = |center: Vec3| Aabb::new(center - Vec3::splat(0.5), center + Vec3::splat(0.5));
        assert!(frustum.may_see(&unit(Vec3::ZERO)), "dead ahead");
        assert!(!frustum.may_see(&unit(vec3(0.0, 0.0, 20.0))), "behind the eye");
        assert!(!frustum.may_see(&unit(vec3(40.0, 0.0, 0.0))), "far to the side");
        assert!(!frustum.may_see(&unit(vec3(0.0, 0.0, -60.0))), "beyond the far plane");
        assert!(frustum.may_see(&unit(vec3(4.6, 0.0, 0.0))), "straddling the edge counts");
        assert!(!frustum.may_see(&Aabb::EMPTY), "nothing is never seen");
    }

    #[test]
    fn a_turned_box_holds_its_corners() {
        let aabb = Aabb::new(vec3(-1.0, -2.0, -0.5), vec3(3.0, 1.0, 2.0));
        let matrix = Mat4::from_trs(
            vec3(5.0, -1.0, 2.0),
            Quat::from_axis_angle(vec3(0.2, 1.0, 0.4), 1.1),
            vec3(1.5, 0.5, 2.0),
        );
        let moved = aabb.transformed(&matrix);
        let tight = Aabb::from_points(aabb.corners().map(|c| matrix.transform_point(c)));
        assert!(close3(moved.min, tight.min) && close3(moved.max, tight.max), "{moved:?} vs {tight:?}");
    }

    #[test]
    fn a_ray_meets_boxes_and_planes_where_they_are() {
        let aabb = Aabb::new(vec3(-1.0, -1.0, -1.0), vec3(1.0, 1.0, 1.0));
        let ray = Ray::new(vec3(-5.0, 0.0, 0.0), Vec3::X);
        assert_eq!(ray.hits_box(&aabb), Some((4.0, 6.0)));
        assert_eq!(Ray::new(vec3(-5.0, 2.0, 0.0), Vec3::X).hits_box(&aabb), None);
        assert_eq!(Ray::new(vec3(5.0, 0.0, 0.0), Vec3::X).hits_box(&aabb), None, "behind the origin");
        let inside = Ray::new(Vec3::ZERO, Vec3::Y).hits_box(&aabb).unwrap();
        assert!(inside.0 < 0.0 && close(inside.1, 1.0));

        let floor = Plane::new(Vec3::ZERO, Vec3::Y);
        let down = Ray::new(vec3(2.0, 3.0, -1.0), vec3(0.0, -1.0, 0.0));
        assert_eq!(down.hits_plane(&floor), Some(3.0));
        assert!(close3(down.at(3.0), vec3(2.0, 0.0, -1.0)));
        assert_eq!(Ray::new(vec3(0.0, 3.0, 0.0), Vec3::Y).hits_plane(&floor), None, "it points away");
    }
}
