//! A tiny multiply-rotate hasher for the INTERNAL maps — identity
//! paths, retention keys, caches. SipHash guards against adversarial
//! keys; these keys are the framework's own view paths and cache keys,
//! so the guard is pure cost on the hottest loops. Never reach for this
//! on data an outside caller controls.

use std::hash::{BuildHasherDefault, Hasher};

pub type FxHashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type FxHashSet<T> = std::collections::HashSet<T, BuildHasherDefault<FxHasher>>;

/// The classic firefox constant — one odd multiplier, good avalanche
/// for short strings.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(SEED);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(SEED);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}
