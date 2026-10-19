//! Inflate — RFC 1951 by hand, the whole of it a PNG needs: stored,
//! fixed and dynamic blocks, the 32K window, the adler check on the
//! zlib wrapper. The framework's own codec walks the same road; this
//! copy keeps the lane std-only.

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

    /// One symbol off the stream — deflate codes arrive MSB-first inside
    /// the LSB-first bit soup, so the walk goes bit by bit.
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

const LENGTH_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] =
    [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// zlib in, raw bytes out. `None` on any malformation, the adler tail
/// included.
pub fn inflate_zlib(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 6 {
        return None;
    }
    let cmf = bytes[0] as u32;
    let flg = bytes[1] as u32;
    // deflate method, window sane, header checksum, no preset dict
    if cmf & 0x0F != 8 || !(cmf * 256 + flg).is_multiple_of(31) || flg & 0x20 != 0 {
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
                let nlen = u16::from_le_bytes([*deflate.get(at + 2)?, *deflate.get(at + 3)?]) as usize;
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
                    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
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
                            let length = LENGTH_BASE[slot] as usize + bits.take(LENGTH_EXTRA[slot] as u32)? as usize;
                            let dist_symbol = distances.decode(&mut bits)? as usize;
                            if dist_symbol >= 30 {
                                return None;
                            }
                            let distance = DIST_BASE[dist_symbol] as usize + bits.take(DIST_EXTRA[dist_symbol] as u32)? as usize;
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
    let at = 2 + bits.at;
    if let Some(tail) = bytes.get(at..at + 4) {
        let want = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
        if super::png::adler32(&out) != want {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the lane's own deflate wrote comes back whole.
    #[test]
    fn the_lanes_own_stream_inflates() {
        let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog, ".repeat(40);
        let mut zlib = vec![0x78, 0x01];
        zlib.extend(super::super::deflate::deflate(&text));
        zlib.extend_from_slice(&super::super::png::adler32(&text).to_be_bytes());
        assert_eq!(inflate_zlib(&zlib).as_deref(), Some(&text[..]));
        // a tail that lies is refused
        let last = zlib.len() - 1;
        zlib[last] ^= 0xFF;
        assert_eq!(inflate_zlib(&zlib), None);
    }

    /// A dynamic-Huffman stream and a stored one, as zlib writes them.
    #[test]
    fn zlibs_dynamic_and_stored_streams_inflate() {
        let text: Vec<u8> = (0..1200u32).map(|i| (97 + (i * 7 + i / 13) % 26) as u8).chain(b" the quick brown fox jumps over the lazy dog ".repeat(30)).collect();
        let dynamic: &[u8] = &[0x78, 0xDA, 0xED, 0xCE, 0xCB, 0x0D, 0x80, 0x20, 0x10, 0x84, 0xE1, 0x56, 0xB6, 0x35, 0x04, 0xE4, 0x21, 0xBA, 0x02, 0x82, 0x68, 0xF5, 0x26, 0x9E, 0x26, 0xB6, 0xE0, 0xDE, 0xBE, 0x64, 0xFE, 0xC3, 0x28, 0xCF, 0x5D, 0xC7, 0x3C, 0x6C, 0xAA, 0xB7, 0x03, 0x6E, 0x0D, 0x38, 0x05, 0xE0, 0x7E, 0x02, 0xCD, 0x02, 0x2C, 0x17, 0x70, 0x5E, 0x81, 0x87, 0x02, 0x7A, 0x06, 0x76, 0x0D, 0x8C, 0x19, 0x38, 0x2C, 0x30, 0x55, 0x25, 0x47, 0xE5, 0xA8, 0x1C, 0xFD, 0xFB, 0x51, 0x3A, 0xBC, 0xA5, 0xDC, 0x82, 0x5E, 0x68, 0x2A, 0x7C, 0x6E, 0x34, 0xF3, 0xA0, 0xD8, 0xD6, 0xBD, 0x12, 0x77, 0x5B, 0xDE, 0x39, 0xA9, 0xFB, 0x22, 0xC3, 0x8E, 0x24, 0x96, 0x58, 0x62, 0x89, 0x25, 0xFE, 0xC4, 0x0F, 0x1C, 0xB6, 0xE7, 0xEC];
        assert_eq!((dynamic[2] >> 1) & 3, 2, "the vector's first block has its own trees");
        assert_eq!(inflate_zlib(dynamic).as_deref(), Some(&text[..]));
        let stored: &[u8] = &[0x78, 0x01, 0x01, 0x03, 0x00, 0xFC, 0xFF, 0x61, 0x62, 0x63, 0x02, 0x4D, 0x01, 0x27];
        assert_eq!(inflate_zlib(stored).as_deref(), Some(&b"abc"[..]));
        assert_eq!(inflate_zlib(&stored[..4]), None, "cut short is malformed");
    }
}
