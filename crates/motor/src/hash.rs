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

    // The small integers are one word each. Left to the default, a flag,
    // a color or the terminator of every `str` went through `write` and
    // its tail loop — a byte copy per field of every look a row hashes.
    // The word is the one that loop made: the value's little-endian
    // bytes, zero-filled (a `u32` IS its zero-extended `u64` there).
    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(u16::from_le_bytes(value.to_ne_bytes())));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(u32::from_le_bytes(value.to_ne_bytes())));
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

    /// A flag, a short and a word hash as their bytes always did: every
    /// key that reached a golden keeps its number.
    #[test]
    fn the_small_integers_answer_what_their_bytes_answered() {
        for value in [0u32, 1, 0xff, 0x1234, 0xdead_beef, u32::MAX] {
            let mut word = FxHasher::default();
            word.write_u32(value);
            assert_eq!(word.finish(), reference(&value.to_ne_bytes()), "u32 {value:#x}");
            let short = value as u16;
            let mut half = FxHasher::default();
            half.write_u16(short);
            assert_eq!(half.finish(), reference(&short.to_ne_bytes()), "u16 {short:#x}");
            let byte = value as u8;
            let mut one = FxHasher::default();
            one.write_u8(byte);
            assert_eq!(one.finish(), reference(&[byte]), "u8 {byte:#x}");
        }
        // and a string, whose terminator is a byte
        let mut text = FxHasher::default();
        std::hash::Hash::hash("App/#0/[12]", &mut text);
        let expected = (reference(b"App/#0/[12]").rotate_left(5) ^ 0xff).wrapping_mul(SEED);
        assert_eq!(text.finish(), expected, "a str hashes its bytes, then its 0xff");
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
