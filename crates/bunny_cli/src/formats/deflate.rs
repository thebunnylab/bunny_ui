//! DEFLATE (RFC 1951), written: what a `.zip` entry and a `.tar.gz`
//! carry. One block with the fixed Huffman codes, its matches found by
//! a hash chain over the last 32 KB — not zlib's ratio, but most of it,
//! and every inflater reads it.

/// The shortest and the longest match DEFLATE codes, and how far back.
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const WINDOW: usize = 32 * 1024;
/// How many earlier places with the same three bytes a match tries.
const CHAIN: usize = 96;
const HASH_BITS: u32 = 15;

/// `bytes`, compressed: one final block of fixed Huffman codes.
pub fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut out = Bits::default();
    out.put(1, 1); // the last block
    out.put(0b01, 2); // fixed Huffman codes
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut prev = vec![usize::MAX; WINDOW];
    let hash = |at: usize| {
        let value = u32::from(bytes[at]) << 16 | u32::from(bytes[at + 1]) << 8 | u32::from(bytes[at + 2]);
        (value.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    };
    let insert = |at: usize, head: &mut Vec<usize>, prev: &mut Vec<usize>| {
        if at + MIN_MATCH <= bytes.len() {
            let key = hash(at);
            prev[at % WINDOW] = head[key];
            head[key] = at;
        }
    };
    let mut at = 0;
    while at < bytes.len() {
        let (length, distance) = longest(bytes, at, &head, &prev, hash);
        if length >= MIN_MATCH {
            out.length(length);
            out.distance(distance);
            for step in at..at + length {
                insert(step, &mut head, &mut prev);
            }
            at += length;
        } else {
            out.literal(bytes[at]);
            insert(at, &mut head, &mut prev);
            at += 1;
        }
    }
    out.symbol(256); // the end of the block
    out.finish()
}

/// The longest earlier match of the bytes at `at`: its length and how
/// far back it starts — a length under 3 when there is none.
fn longest(bytes: &[u8], at: usize, head: &[usize], prev: &[usize], hash: impl Fn(usize) -> usize) -> (usize, usize) {
    if at + MIN_MATCH > bytes.len() {
        return (0, 0);
    }
    let limit = MAX_MATCH.min(bytes.len() - at);
    let (mut best, mut distance) = (0, 0);
    let mut candidate = head[hash(at)];
    for _ in 0..CHAIN {
        if candidate == usize::MAX || candidate >= at || at - candidate > WINDOW {
            break;
        }
        let mut length = 0;
        while length < limit && bytes[candidate + length] == bytes[at + length] {
            length += 1;
        }
        if length > best {
            best = length;
            distance = at - candidate;
            if length == limit {
                break;
            }
        }
        let next = prev[candidate % WINDOW];
        if next == usize::MAX || next >= candidate {
            break;
        }
        candidate = next;
    }
    (best, distance)
}

/// Bits, least significant first, the order DEFLATE packs them in.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    pending: u64,
    count: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        self.pending |= u64::from(value) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.bytes.push(self.pending as u8);
            self.pending >>= 8;
            self.count -= 8;
        }
    }

    /// A Huffman code: its bits go most significant first.
    fn code(&mut self, code: u32, length: u32) {
        self.put(code.reverse_bits() >> (32 - length), length);
    }

    /// A literal or length symbol, in the fixed code.
    fn symbol(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xC0 + symbol - 280, 8),
        }
    }

    fn literal(&mut self, byte: u8) {
        self.symbol(u32::from(byte));
    }

    fn length(&mut self, length: usize) {
        let (symbol, base, extra) = LENGTHS.iter().rev().find(|(_, base, _)| *base as usize <= length).copied().unwrap_or((257, 3, 0));
        self.symbol(symbol);
        self.put(length as u32 - base, extra);
    }

    fn distance(&mut self, distance: usize) {
        let (code, base, extra) = DISTANCES.iter().rev().find(|(_, base, _)| *base as usize <= distance).copied().unwrap_or((0, 1, 0));
        self.code(code, 5);
        self.put(distance as u32 - base, extra);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.pending as u8);
        }
        self.bytes
    }
}

/// (symbol, the shortest length it codes, its extra bits).
const LENGTHS: [(u32, u32, u32); 29] = [
    (257, 3, 0), (258, 4, 0), (259, 5, 0), (260, 6, 0), (261, 7, 0), (262, 8, 0), (263, 9, 0), (264, 10, 0),
    (265, 11, 1), (266, 13, 1), (267, 15, 1), (268, 17, 1), (269, 19, 2), (270, 23, 2), (271, 27, 2), (272, 31, 2),
    (273, 35, 3), (274, 43, 3), (275, 51, 3), (276, 59, 3), (277, 67, 4), (278, 83, 4), (279, 99, 4), (280, 115, 4),
    (281, 131, 5), (282, 163, 5), (283, 195, 5), (284, 227, 5), (285, 258, 0),
];

/// (code, the shortest distance it codes, its extra bits).
const DISTANCES: [(u32, u32, u32); 30] = [
    (0, 1, 0), (1, 2, 0), (2, 3, 0), (3, 4, 0), (4, 5, 1), (5, 7, 1), (6, 9, 2), (7, 13, 2), (8, 17, 3), (9, 25, 3),
    (10, 33, 4), (11, 49, 4), (12, 65, 5), (13, 97, 5), (14, 129, 6), (15, 193, 6), (16, 257, 7), (17, 385, 7),
    (18, 513, 8), (19, 769, 8), (20, 1025, 9), (21, 1537, 9), (22, 2049, 10), (23, 3073, 10), (24, 4097, 11),
    (25, 6145, 11), (26, 8193, 12), (27, 12289, 12), (28, 16385, 13), (29, 24577, 13),
];

/// A gzip member (RFC 1952) around `bytes`: what `.tar.gz` is.
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    out.extend(deflate(bytes));
    out.extend_from_slice(&super::zip::crc32(bytes).to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small inflater for the one block shape this writes — enough to
    /// read every output back.
    fn inflate(data: &[u8]) -> Vec<u8> {
        let mut bit = 0usize;
        let mut take = |count: u32| {
            let mut value = 0u32;
            for index in 0..count {
                value |= u32::from(data[bit / 8] >> (bit % 8) & 1) << index;
                bit += 1;
            }
            value
        };
        assert_eq!(take(1), 1, "one final block");
        assert_eq!(take(2), 1, "fixed codes");
        let mut out = Vec::new();
        loop {
            // read the fixed code most significant bit first
            let mut code = 0u32;
            let mut length = 0;
            let symbol = loop {
                code = code << 1 | take(1);
                length += 1;
                match (length, code) {
                    (7, 0..=0x17) => break code + 256,
                    (8, 0x30..=0xBF) => break code - 0x30,
                    (8, 0xC0..=0xC7) => break code - 0xC0 + 280,
                    (9, 0x190..=0x1FF) => break code - 0x190 + 144,
                    _ => {}
                }
            };
            match symbol {
                0..=255 => out.push(symbol as u8),
                256 => return out,
                _ => {
                    let (_, base, extra) = LENGTHS[(symbol - 257) as usize];
                    let length = (base + take(extra)) as usize;
                    let mut code = 0;
                    for _ in 0..5 {
                        code = code << 1 | take(1);
                    }
                    let (_, base, extra) = DISTANCES[code as usize];
                    let distance = (base + take(extra)) as usize;
                    for _ in 0..length {
                        out.push(out[out.len() - distance]);
                    }
                }
            }
        }
    }

    #[test]
    fn what_is_compressed_inflates_back() {
        let text = b"the quick brown fox jumps over the lazy dog; the quick brown fox jumps again".repeat(40);
        let mut mixed: Vec<u8> = (0..70_000u32).map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        mixed.extend_from_slice(&text);
        for input in [Vec::new(), b"a".to_vec(), b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec(), text.clone(), mixed] {
            let packed = deflate(&input);
            assert_eq!(inflate(&packed), input);
        }
        assert!(deflate(&text).len() < text.len() / 10, "repetition compresses");
    }

    #[test]
    fn the_empty_stream_is_the_standard_one() {
        // a final fixed block holding only its end: 03 00
        assert_eq!(deflate(b""), [0x03, 0x00]);
    }

    /// The system's own gzip reads what this writes, where there is one.
    #[cfg(unix)]
    #[test]
    fn gzip_reads_it() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let Ok(mut child) = Command::new("gzip").arg("-dc").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else {
            return;
        };
        let input = b"bunny bunny bunny ui ui ui \x00\x01\x02".repeat(500);
        child.stdin.take().unwrap().write_all(&gzip(&input)).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, input);
    }
}
