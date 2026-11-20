//! Points of the twisted Edwards curve -x^2 + y^2 = 1 + d x^2 y^2, in
//! extended coordinates (X : Y : Z : T) with x = X/Z, y = Y/Z, xy = T/Z.

use super::field::{Fe, constants};
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    pub const IDENTITY: Point = Point { x: Fe::ZERO, y: Fe::ONE, z: Fe::ONE, t: Fe::ZERO };

    /// The base point B of Ed25519: y = 4/5, x even.
    pub fn base() -> Point {
        static BASE: OnceLock<Point> = OnceLock::new();
        *BASE.get_or_init(|| {
            let mut bytes = [0x66; 32];
            bytes[0] = 0x58;
            Point::decompress(&bytes).unwrap_or(Point::IDENTITY)
        })
    }

    /// The sum of two points. The formula is complete for this curve: it
    /// is correct for every pair, a point and itself too, with no branch.
    pub fn add(self, other: Point) -> Point {
        let a = self.y.sub(self.x).mul(other.y.sub(other.x));
        let b = self.y.add(self.x).mul(other.y.add(other.x));
        let c = self.t.mul(constants().d2).mul(other.t);
        let d = self.z.add(self.z).mul(other.z);
        let (e, f, g, h) = (b.sub(a), d.sub(c), d.add(c), b.add(a));
        Point { x: e.mul(f), y: g.mul(h), z: f.mul(g), t: e.mul(h) }
    }

    pub fn neg(self) -> Point {
        Point { x: self.x.neg(), y: self.y, z: self.z, t: self.t.neg() }
    }

    fn select(choice: u8, a: Point, b: Point) -> Point {
        Point {
            x: Fe::select(choice, a.x, b.x),
            y: Fe::select(choice, a.y, b.y),
            z: Fe::select(choice, a.z, b.z),
            t: Fe::select(choice, a.t, b.t),
        }
    }

    /// `scalar` (32 little-endian bytes) times the point.
    ///
    /// Each of the 256 steps doubles and adds, and a mask then keeps the
    /// sum or the double: the same work for every scalar, so a secret
    /// scalar does not show in the time.
    pub fn mul(self, scalar: &[u8; 32]) -> Point {
        let mut out = Point::IDENTITY;
        for bit in (0..256).rev() {
            out = out.add(out);
            let sum = out.add(self);
            out = Point::select(scalar[bit / 8] >> (bit % 8) & 1, sum, out);
        }
        out
    }

    /// The 32-byte form: y, with the sign of x in the top bit.
    pub fn compress(self) -> [u8; 32] {
        let z = self.z.invert();
        let (x, y) = (self.x.mul(z), self.y.mul(z));
        let mut out = y.to_bytes();
        out[31] ^= u8::from(x.is_negative()) << 7;
        out
    }

    /// The point of a 32-byte form, or `None` when no point has it.
    ///
    /// The same rules as ed25519-dalek: a y of p or more is taken modulo
    /// p, and a sign bit on x = 0 is accepted. This branches on the
    /// bytes, which are public: a key or a signature.
    pub fn decompress(bytes: &[u8; 32]) -> Option<Point> {
        let y = Fe::from_bytes(bytes);
        let yy = y.square();
        let u = yy.sub(Fe::ONE);
        let v = yy.mul(constants().d).add(Fe::ONE);
        let mut x = Fe::sqrt_ratio(u, v)?;
        if bytes[31] >> 7 == 1 {
            x = x.neg();
        }
        Some(Point { x, y, z: Fe::ONE, t: x.mul(y) })
    }
}
