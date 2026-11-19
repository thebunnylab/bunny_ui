//! Integers modulo the group order l = 2^252 + 27742317777372353535851937790883648493.
//!
//! A scalar is four 64-bit limbs, least significant first. The reduction
//! goes one bit at a time and takes l away under a mask, so the time
//! does not change with a secret value.

/// l, the order of the base point.
const L: [u64; 4] = [0x5812_631a_5cf5_d3ed, 0x14de_f9de_a2f7_9cd6, 0, 0x1000_0000_0000_0000];

fn limbs<const N: usize>(bytes: &[u8]) -> [u64; N] {
    let mut out = [0u64; N];
    for (limb, chunk) in out.iter_mut().zip(bytes.as_chunks::<8>().0) {
        *limb = u64::from_le_bytes(*chunk);
    }
    out
}

fn bytes_of(limbs: [u64; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (chunk, limb) in out.as_chunks_mut::<8>().0.iter_mut().zip(limbs) {
        chunk.copy_from_slice(&limb.to_le_bytes());
    }
    out
}

/// `a - b` and whether it went below zero.
fn sub(a: [u64; 4], b: [u64; 4]) -> ([u64; 4], bool) {
    let mut out = [0u64; 4];
    let mut borrow = false;
    for i in 0..4 {
        let (step, low) = a[i].overflowing_sub(b[i]);
        let (step, carry) = step.overflowing_sub(u64::from(borrow));
        out[i] = step;
        borrow = low | carry;
    }
    (out, borrow)
}

/// `wide` (little-endian 64-bit limbs) modulo l.
fn reduce<const N: usize>(wide: [u64; N]) -> [u64; 4] {
    let mut acc = [0u64; 4];
    for bit in (0..N * 64).rev() {
        // acc < l < 2^253, so twice it plus one still fits in 256 bits
        let next = (wide[bit / 64] >> (bit % 64)) & 1;
        acc = [
            acc[0] << 1 | next,
            acc[1] << 1 | acc[0] >> 63,
            acc[2] << 1 | acc[1] >> 63,
            acc[3] << 1 | acc[2] >> 63,
        ];
        let (less, borrow) = sub(acc, L);
        // keep `less` when no borrow: acc was l or more
        let mask = u64::from(borrow).wrapping_sub(1);
        for i in 0..4 {
            acc[i] = (less[i] & mask) | (acc[i] & !mask);
        }
    }
    acc
}

/// A 64-byte hash, taken as a little-endian integer, modulo l.
pub fn from_hash(hash: &[u8; 64]) -> [u8; 32] {
    bytes_of(reduce(limbs::<8>(hash)))
}

/// (r + k a) modulo l, with each input 32 little-endian bytes.
pub fn mul_add(k: &[u8; 32], a: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
    let (k, a, r) = (limbs::<4>(k), limbs::<4>(a), limbs::<4>(r));
    let mut wide = [0u64; 8];
    for i in 0..4 {
        let mut carry: u128 = 0;
        for j in 0..4 {
            let sum = u128::from(wide[i + j]) + u128::from(k[i]) * u128::from(a[j]) + carry;
            wide[i + j] = sum as u64;
            carry = sum >> 64;
        }
        wide[i + 4] = carry as u64;
    }
    let mut carry = 0u128;
    for (i, limb) in wide.iter_mut().enumerate() {
        let sum = u128::from(*limb) + u128::from(*r.get(i).unwrap_or(&0)) + carry;
        *limb = sum as u64;
        carry = sum >> 64;
    }
    bytes_of(reduce(wide))
}

/// Whether 32 little-endian bytes are an integer below l: the only form
/// of s a signature can have.
pub fn is_canonical(bytes: &[u8; 32]) -> bool {
    sub(limbs::<4>(bytes), L).1
}
