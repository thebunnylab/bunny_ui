//! The field of integers modulo p = 2^255 - 19.
//!
//! An element is five limbs of 51 bits, least significant first. No
//! operation branches on the value of an element, and no table lookup
//! uses it as an index, so the time does not change with a secret.

use std::sync::OnceLock;

const MASK: u64 = (1 << 51) - 1;

/// An element of the field, not always in its smallest form.
#[derive(Clone, Copy)]
pub struct Fe([u64; 5]);

impl Fe {
    pub const ZERO: Fe = Fe([0; 5]);
    pub const ONE: Fe = Fe([1, 0, 0, 0, 0]);

    /// The element of 32 little-endian bytes. The top bit is ignored,
    /// and a value of p or more is accepted as itself modulo p.
    pub fn from_bytes(bytes: &[u8; 32]) -> Fe {
        let load = |at: usize| {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(word)
        };
        Fe([
            load(0) & MASK,
            (load(6) >> 3) & MASK,
            (load(12) >> 6) & MASK,
            (load(19) >> 1) & MASK,
            (load(24) >> 12) & MASK,
        ])
    }

    /// The 32 little-endian bytes of the element in its smallest form.
    pub fn to_bytes(self) -> [u8; 32] {
        let mut h = self.carried().0;
        // each limb is now below 2^51 but the first, a little over: q is
        // 1 when the value is p or more, and 19q then takes p away
        let mut q = (h[0] + 19) >> 51;
        q = (h[1] + q) >> 51;
        q = (h[2] + q) >> 51;
        q = (h[3] + q) >> 51;
        q = (h[4] + q) >> 51;
        h[0] += 19 * q;
        h[1] += h[0] >> 51;
        h[0] &= MASK;
        h[2] += h[1] >> 51;
        h[1] &= MASK;
        h[3] += h[2] >> 51;
        h[2] &= MASK;
        h[4] += h[3] >> 51;
        h[3] &= MASK;
        h[4] &= MASK;

        let mut out = [0u8; 32];
        let mut acc: u128 = 0;
        let mut bits = 0;
        let mut at = 0;
        for limb in h {
            acc |= u128::from(limb) << bits;
            bits += 51;
            while bits >= 8 {
                out[at] = acc as u8;
                acc >>= 8;
                bits -= 8;
                at += 1;
            }
        }
        out[at] = acc as u8;
        out
    }

    /// The same value with each limb below 2^51 + a little.
    fn carried(self) -> Fe {
        let mut h = self.0;
        h[1] += h[0] >> 51;
        h[0] &= MASK;
        h[2] += h[1] >> 51;
        h[1] &= MASK;
        h[3] += h[2] >> 51;
        h[2] &= MASK;
        h[4] += h[3] >> 51;
        h[3] &= MASK;
        h[0] += 19 * (h[4] >> 51);
        h[4] &= MASK;
        Fe(h)
    }

    pub fn add(self, other: Fe) -> Fe {
        let (a, b) = (self.0, other.0);
        Fe([a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3], a[4] + b[4]]).carried()
    }

    pub fn sub(self, other: Fe) -> Fe {
        // 16p is added first so that no limb goes below zero
        const P16: [u64; 5] = [
            36_028_797_018_963_664,
            36_028_797_018_963_952,
            36_028_797_018_963_952,
            36_028_797_018_963_952,
            36_028_797_018_963_952,
        ];
        let (a, b) = (self.carried().0, other.carried().0);
        Fe([
            (a[0] + P16[0]) - b[0],
            (a[1] + P16[1]) - b[1],
            (a[2] + P16[2]) - b[2],
            (a[3] + P16[3]) - b[3],
            (a[4] + P16[4]) - b[4],
        ])
        .carried()
    }

    pub fn neg(self) -> Fe {
        Fe::ZERO.sub(self)
    }

    pub fn mul(self, other: Fe) -> Fe {
        let (a, b) = (self.carried().0, other.carried().0);
        let m = |x: u64, y: u64| u128::from(x) * u128::from(y);
        let (b1, b2, b3, b4) = (b[1] * 19, b[2] * 19, b[3] * 19, b[4] * 19);
        let mut c = [
            m(a[0], b[0]) + m(a[4], b1) + m(a[3], b2) + m(a[2], b3) + m(a[1], b4),
            m(a[1], b[0]) + m(a[0], b[1]) + m(a[4], b2) + m(a[3], b3) + m(a[2], b4),
            m(a[2], b[0]) + m(a[1], b[1]) + m(a[0], b[2]) + m(a[4], b3) + m(a[3], b4),
            m(a[3], b[0]) + m(a[2], b[1]) + m(a[1], b[2]) + m(a[0], b[3]) + m(a[4], b4),
            m(a[4], b[0]) + m(a[3], b[1]) + m(a[2], b[2]) + m(a[1], b[3]) + m(a[0], b[4]),
        ];
        let mask = u128::from(MASK);
        c[1] += c[0] >> 51;
        c[0] &= mask;
        c[2] += c[1] >> 51;
        c[1] &= mask;
        c[3] += c[2] >> 51;
        c[2] &= mask;
        c[4] += c[3] >> 51;
        c[3] &= mask;
        c[0] += (c[4] >> 51) * 19;
        c[4] &= mask;
        c[1] += c[0] >> 51;
        c[0] &= mask;
        Fe([c[0] as u64, c[1] as u64, c[2] as u64, c[3] as u64, c[4] as u64])
    }

    pub fn square(self) -> Fe {
        self.mul(self)
    }

    /// The element to the power `exponent`, 32 little-endian bytes. The
    /// exponent is public: the steps depend on its bits.
    fn pow(self, exponent: &[u8; 32]) -> Fe {
        let mut out = Fe::ONE;
        for bit in (0..256).rev() {
            out = out.square();
            if exponent[bit / 8] >> (bit % 8) & 1 == 1 {
                out = out.mul(self);
            }
        }
        out
    }

    /// 1 / self, by Fermat: self^(p - 2). Zero gives zero.
    pub fn invert(self) -> Fe {
        let mut exponent = [0xFF; 32];
        exponent[0] = 0xEB;
        exponent[31] = 0x7F;
        self.pow(&exponent)
    }

    /// self^((p - 5) / 8) = self^(2^252 - 3), the step of a square root.
    fn pow_p58(self) -> Fe {
        let mut exponent = [0xFF; 32];
        exponent[0] = 0xFD;
        exponent[31] = 0x0F;
        self.pow(&exponent)
    }

    pub fn equals(self, other: Fe) -> bool {
        self.to_bytes() == other.to_bytes()
    }

    #[cfg(test)]
    pub fn is_zero(self) -> bool {
        self.to_bytes() == [0; 32]
    }

    /// Whether the smallest form is odd: the "negative" half of the field.
    pub fn is_negative(self) -> bool {
        self.to_bytes()[0] & 1 == 1
    }

    /// `a` when `choice` is 1, `b` when it is 0, with no branch.
    pub fn select(choice: u8, a: Fe, b: Fe) -> Fe {
        let mask = 0u64.wrapping_sub(u64::from(choice & 1));
        let mut out = [0u64; 5];
        for (place, (x, y)) in out.iter_mut().zip(a.0.iter().zip(b.0.iter())) {
            *place = (x & mask) | (y & !mask);
        }
        Fe(out)
    }

    /// The non-negative square root of u / v, when there is one.
    ///
    /// The answer for u = 0 is zero, a root. When v is zero and u is not,
    /// there is no root. This branches on its values: it reads a point
    /// out of public bytes, never a secret.
    pub fn sqrt_ratio(u: Fe, v: Fe) -> Option<Fe> {
        let v3 = v.square().mul(v);
        let v7 = v3.square().mul(v);
        let mut r = u.mul(v3).mul(u.mul(v7).pow_p58());
        let check = v.mul(r.square());
        let correct = check.equals(u);
        let flipped = check.equals(u.neg());
        let flipped_i = check.equals(u.neg().mul(constants().sqrt_m1));
        if flipped || flipped_i {
            r = r.mul(constants().sqrt_m1);
        }
        if r.is_negative() {
            r = r.neg();
        }
        (correct || flipped).then_some(r)
    }
}

/// The constants of the curve, worked out once from their definitions.
pub struct Constants {
    /// d = -121665 / 121666, of the curve -x^2 + y^2 = 1 + d x^2 y^2.
    pub d: Fe,
    /// 2d, which the point addition uses.
    pub d2: Fe,
    /// A square root of -1: 2^((p - 1) / 4).
    pub sqrt_m1: Fe,
}

pub fn constants() -> &'static Constants {
    static CONSTANTS: OnceLock<Constants> = OnceLock::new();
    CONSTANTS.get_or_init(|| {
        let small = |value: u64| Fe([value, 0, 0, 0, 0]);
        let d = small(121_665).neg().mul(small(121_666).invert());
        let mut quarter = [0xFF; 32];
        quarter[0] = 0xFB;
        quarter[31] = 0x1F;
        Constants { d, d2: d.add(d), sqrt_m1: small(2).pow(&quarter) }
    })
}
