//! Inflate — RFC 1951 by hand, the whole of it a PNG needs: stored,
//! fixed and dynamic blocks, the 32K window, the adler check on the
//! zlib wrapper. Safe Rust from the first byte: this is the road the
//! Linux shell walks because its platform has no codec and the C ones
//! speak setjmp.


struct BitReader<'a> {
    bytes: &'a [u8],
    at: usize,
    bit: u32,
    value: u32,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> BitReader<'a> {
        BitReader { bytes, at: 0, bit: 0, value: 0 }
    }

    fn take(&mut self, count: u32) -> Option<u32> {
        while self.bit < count {
            let byte = *self.bytes.get(self.at)? as u32;
            self.at += 1;
            self.value |= byte << self.bit;
            self.bit += 8;
        }
        let out = self.value & ((1 << count) - 1);
        self.value >>= count;
        self.bit -= count;
        Some(out)
    }

    fn align(&mut self) {
        self.value = 0;
        self.bit = 0;
    }
}

/// A canonical Huffman table: code lengths in, symbol lookup out.
struct Huffman {
    /// counts[len] and offsets into `symbols`, the canonical walk.
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Huffman {
        let mut counts = [0u16; 16];
        for &length in lengths {
            counts[length as usize] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0u16; 16];
        for length in 1..16 {
            offsets[length] = offsets[length - 1] + counts[length - 1];
        }
        let mut symbols = vec![0u16; lengths.iter().filter(|&&l| l != 0).count()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                symbols[offsets[length as usize] as usize] = symbol as u16;
                offsets[length as usize] += 1;
            }
        }
        Huffman { counts, symbols }
    }

    /// One symbol off the stream — deflate codes arrive MSB-first
    /// inside the LSB-first bit soup, so the walk goes bit by bit.
    fn decode(&self, bits: &mut BitReader) -> Option<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for length in 1..16 {
            code |= bits.take(1)? as i32;
            let count = self.counts[length] as i32;
            if code - first < count {
                return Some(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];

/// zlib in, raw bytes out. `None` on any malformation — the caller
/// remembers the failure and never walks the bytes again.
pub fn inflate_zlib(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 6 {
        return None;
    }
    let cmf = bytes[0] as u32;
    let flg = bytes[1] as u32;
    // deflate method, window sane, header checksum, no preset dict
    if cmf & 0x0F != 8 || (cmf * 256 + flg) % 31 != 0 || flg & 0x20 != 0 {
        return None;
    }
    let deflate = &bytes[2..];
    let mut bits = BitReader::new(deflate);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = bits.take(1)?;
        match bits.take(2)? {
            0 => {
                // stored: aligned, LEN + one's complement
                bits.align();
                let at = bits.at;
                let len = u16::from_le_bytes([*deflate.get(at)?, *deflate.get(at + 1)?]) as usize;
                let nlen =
                    u16::from_le_bytes([*deflate.get(at + 2)?, *deflate.get(at + 3)?]) as usize;
                if len != !nlen & 0xFFFF {
                    return None;
                }
                let data = deflate.get(at + 4..at + 4 + len)?;
                out.extend_from_slice(data);
                bits.at = at + 4 + len;
            }
            kind @ (1 | 2) => {
                let (literals, distances);
                if kind == 1 {
                    // the fixed trees, straight from the RFC
                    let mut lengths = [0u8; 288];
                    lengths[..144].fill(8);
                    lengths[144..256].fill(9);
                    lengths[256..280].fill(7);
                    lengths[280..].fill(8);
                    literals = Huffman::new(&lengths);
                    distances = Huffman::new(&[5u8; 30]);
                } else {
                    const ORDER: [usize; 19] =
                        [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let hlit = bits.take(5)? as usize + 257;
                    let hdist = bits.take(5)? as usize + 1;
                    let hclen = bits.take(4)? as usize + 4;
                    let mut code_lengths = [0u8; 19];
                    for &slot in ORDER.iter().take(hclen) {
                        code_lengths[slot] = bits.take(3)? as u8;
                    }
                    let decoder = Huffman::new(&code_lengths);
                    let mut lengths = vec![0u8; hlit + hdist];
                    let mut at = 0;
                    while at < lengths.len() {
                        let symbol = decoder.decode(&mut bits)?;
                        match symbol {
                            0..=15 => {
                                lengths[at] = symbol as u8;
                                at += 1;
                            }
                            16 => {
                                let previous = *lengths.get(at.checked_sub(1)?)?;
                                for _ in 0..bits.take(2)? + 3 {
                                    *lengths.get_mut(at)? = previous;
                                    at += 1;
                                }
                            }
                            17 => at += bits.take(3)? as usize + 3,
                            18 => at += bits.take(7)? as usize + 11,
                            _ => return None,
                        }
                    }
                    if at > lengths.len() {
                        return None;
                    }
                    literals = Huffman::new(&lengths[..hlit]);
                    distances = Huffman::new(&lengths[hlit..]);
                }
                loop {
                    let symbol = literals.decode(&mut bits)?;
                    match symbol {
                        0..=255 => out.push(symbol as u8),
                        256 => break,
                        257..=285 => {
                            let slot = symbol as usize - 257;
                            let length = LENGTH_BASE[slot] as usize
                                + bits.take(LENGTH_EXTRA[slot] as u32)? as usize;
                            let dist_symbol = distances.decode(&mut bits)? as usize;
                            if dist_symbol >= 30 {
                                return None;
                            }
                            let distance = DIST_BASE[dist_symbol] as usize
                                + bits.take(DIST_EXTRA[dist_symbol] as u32)? as usize;
                            let start = out.len().checked_sub(distance)?;
                            // the window copy may overlap itself — the
                            // repeat IS the feature
                            for offset in 0..length {
                                let byte = out[start + offset];
                                out.push(byte);
                            }
                        }
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
        if last == 1 {
            break;
        }
    }
    // the adler tail seals the stream
    let at = 2 + bits.at + if bits.bit >= 8 { 0 } else { 0 };
    let tail = bytes.get(at..at + 4);
    if let Some(tail) = tail {
        let want = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
        if adler32(&out) != want {
            return None;
        }
    }
    Some(out)
}

pub fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}
