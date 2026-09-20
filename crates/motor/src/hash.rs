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
        // whole words first: a slice of EXACTLY eight bytes becomes one
        // load. A slice of "up to eight" is a copy of unknown length, and
        // the compiler calls `memmove` for it — once for each word of every
        // path this hasher is given, which is the hottest loop it has.
        let mut words = bytes.chunks_exact(8);
        for chunk in &mut words {
            let word = u64::from_le_bytes(chunk.try_into().expect("a chunk of eight"));
            self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED);
        }
        // the tail, zero-filled: the same word the loop above would make
        let tail = words.remainder();
        if !tail.is_empty() {
            let mut word = [0u8; 8];
            for (slot, byte) in word.iter_mut().zip(tail) {
                *slot = *byte;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The hasher as it was first written: a zero-filled word for every
    /// chunk of up to eight bytes. Keys that reach a golden are hashed by
    /// this rule, so a faster `write` must answer the very same number.
    fn reference(bytes: &[u8]) -> u64 {
        let mut state = 0u64;
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            state = (state.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(SEED);
        }
        state
    }

    #[test]
    fn the_word_loop_answers_what_the_byte_copy_answered() {
        let text = "w0/Workbench/#1/#1/[board]/#0/#0/[panel-3]/Panel/#1/#1/#0/#17/[legend-16]";
        // every length from empty to past several words, so every tail
        // length is met after every count of whole words
        for end in 0..=text.len() {
            let bytes = &text.as_bytes()[..end];
            let mut hasher = FxHasher::default();
            hasher.write(bytes);
            assert_eq!(hasher.finish(), reference(bytes), "length {end}");
        }
    }
}
