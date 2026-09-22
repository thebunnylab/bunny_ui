//! JPEG: the baseline and the progressive roads, decoded the way
//! libjpeg does so the pixels agree with every other shell's platform
//! within a step — the same integer inverse DCT (`jidctint`, the
//! "islow" one), the same triangle filter for the chroma upsampling,
//! the same fixed-point YCbCr tables.
//!
//! In scope: Huffman-coded 8-bit sequential (SOF0, SOF1) and
//! progressive (SOF2) frames, one or three components, any sampling
//! factors, restart intervals, the JFIF and Adobe markers. Out of
//! scope, refused BY NAME: arithmetic coding, lossless, hierarchical,
//! 12-bit samples, CMYK and YCCK. The EXIF orientation tag is read by
//! nobody — the platform decoders the other shells use leave it as
//! metadata too, and so does this one.
//!
//! The decoder holds every coefficient of the picture until the last
//! scan closed (a progressive picture needs that; a baseline one is
//! the same road with one scan), then dequantizes, transforms,
//! upsamples and converts — one plane per component, then RGBA.

use super::{fits, Image};

/// Why a JPEG was refused — a name, so the shell can say it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// Arithmetic entropy coding (SOF9…SOF11, SOF13…SOF15).
    Arithmetic,
    /// Lossless (SOF3, SOF7, SOF11, SOF15).
    Lossless,
    /// Hierarchical (SOF5…SOF7, SOF13…SOF15).
    Hierarchical,
    /// 12-bit (or 16-bit) samples.
    TwelveBit,
    /// Four components without the Adobe transform: CMYK.
    Cmyk,
    /// Four components with the Adobe transform: YCCK.
    Ycck,
    /// A picture beyond the pixel cap.
    TooLarge,
    /// Bytes that are not the JPEG they claim to be.
    Corrupt,
}

// MARK: - markers

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const DQT: u8 = 0xDB;
const DHT: u8 = 0xC4;
const DRI: u8 = 0xDD;
const APP0: u8 = 0xE0;
const APP14: u8 = 0xEE;
const RST0: u8 = 0xD0;
const RST7: u8 = 0xD7;

/// The natural-order index of each zigzag position.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
    59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

fn be16(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize)
}

/// The size out of the frame header alone.
pub fn header(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != SOI {
        return None;
    }
    let mut at = 2;
    loop {
        // markers may be padded with 0xFF fill bytes
        while *bytes.get(at)? == 0xFF && bytes.get(at + 1).is_some_and(|&b| b == 0xFF) {
            at += 1;
        }
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        at += 2;
        match marker {
            0xC0..=0xCF if marker != DHT && marker != 0xC8 && marker != 0xCC => {
                let height = be16(bytes, at + 3)? as u32;
                let width = be16(bytes, at + 5)? as u32;
                return fits(width, height).then_some((width, height));
            }
            SOS | EOI => return None,
            RST0..=RST7 | 0x01 => {}
            _ => at += be16(bytes, at)?,
        }
    }
}

// MARK: - the tables

/// A Huffman table as the spec decodes it (F.2.2.3): the codes of
/// each length, the first code of each length, and where its symbols
/// start.
#[derive(Clone, Default)]
struct Huffman {
    /// Per code length 1..=16: the largest code of that length, or -1
    /// when the length has none.
    maxcode: [i32; 18],
    /// Per code length: symbols index − first code of that length.
    valptr: [i32; 17],
    symbols: Vec<u8>,
    present: bool,
}

impl Huffman {
    fn new(counts: &[u8; 16], symbols: Vec<u8>) -> Huffman {
        let mut table = Huffman { symbols, present: true, ..Huffman::default() };
        let mut code: i32 = 0;
        let mut index: i32 = 0;
        for length in 1..=16 {
            let count = counts[length - 1] as i32;
            if count == 0 {
                table.maxcode[length] = -1;
            } else {
                table.valptr[length] = index - code;
                code += count;
                index += count;
                table.maxcode[length] = code - 1;
            }
            code <<= 1;
        }
        table.maxcode[17] = i32::MAX;
        table
    }
}

// MARK: - the bit reader

/// Entropy-coded bytes, one bit at a time: `0xFF 0x00` is a stuffed
/// `0xFF`, any other marker ends the segment (the reader answers
/// zeros past it, and the caller resynchronizes at the restart).
struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
    acc: u32,
    count: u32,
    /// A marker was met: the segment is over until a restart.
    ended: bool,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8], at: usize) -> Bits<'a> {
        Bits { bytes, at, acc: 0, count: 0, ended: false }
    }

    fn fill(&mut self) {
        while self.count <= 24 {
            let byte = if self.ended {
                0
            } else {
                match self.bytes.get(self.at) {
                    Some(&0xFF) => match self.bytes.get(self.at + 1) {
                        Some(&0x00) => {
                            self.at += 2;
                            0xFF
                        }
                        Some(&0xFF) => {
                            // fill bytes before a marker: skip one
                            self.at += 1;
                            continue;
                        }
                        _ => {
                            self.ended = true;
                            0
                        }
                    },
                    Some(&byte) => {
                        self.at += 1;
                        byte
                    }
                    None => {
                        self.ended = true;
                        0
                    }
                }
            };
            self.acc |= (byte as u32) << (24 - self.count);
            self.count += 8;
        }
    }

    fn bit(&mut self) -> u32 {
        self.bits(1)
    }

    fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.count < n {
            self.fill();
        }
        let out = self.acc >> (32 - n);
        self.acc <<= n;
        self.count -= n;
        out
    }

    /// The signed value of an `n`-bit magnitude category (F.2.2.1).
    fn extend(value: u32, n: u32) -> i32 {
        if n == 0 {
            0
        } else if value < (1 << (n - 1)) {
            value as i32 - (1 << n) + 1
        } else {
            value as i32
        }
    }

    fn receive_extend(&mut self, n: u32) -> i32 {
        let value = self.bits(n);
        Self::extend(value, n)
    }

    fn decode(&mut self, table: &Huffman) -> Option<u8> {
        let mut code: i32 = 0;
        for length in 1..=16 {
            code = (code << 1) | self.bit() as i32;
            if code <= table.maxcode[length] {
                let index = (table.valptr[length] + code) as usize;
                return table.symbols.get(index).copied();
            }
        }
        None
    }

    /// At a restart interval's end: drop the partial byte, expect an
    /// RSTn marker and step past it.
    fn restart(&mut self) -> bool {
        self.acc = 0;
        self.count = 0;
        self.ended = false;
        // find the marker: the next two bytes must be FF Dn
        while self.at + 1 < self.bytes.len() {
            if self.bytes[self.at] == 0xFF {
                let marker = self.bytes[self.at + 1];
                if (RST0..=RST7).contains(&marker) {
                    self.at += 2;
                    return true;
                }
                if marker == 0xFF {
                    self.at += 1;
                    continue;
                }
                return false;
            }
            self.at += 1;
        }
        false
    }

    /// Where the next marker starts, after this scan's entropy data.
    fn end_of_scan(&self) -> usize {
        let mut at = self.at;
        while at + 1 < self.bytes.len() {
            if self.bytes[at] == 0xFF && self.bytes[at + 1] != 0x00 && !(RST0..=RST7).contains(&self.bytes[at + 1]) {
                return at;
            }
            at += 1;
        }
        self.bytes.len()
    }
}

// MARK: - the frame

struct Component {
    id: u8,
    h: usize,
    v: usize,
    quant: usize,
    /// Blocks per row and rows of blocks, padded to whole MCUs.
    blocks_w: usize,
    blocks_h: usize,
    /// The coefficients of every block, natural order, 64 per block.
    coeffs: Vec<i16>,
    dc_table: usize,
    ac_table: usize,
    dc_pred: i32,
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    components: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcus_w: usize,
    mcus_h: usize,
}

struct Decoder<'a> {
    bytes: &'a [u8],
    quant: [[u16; 64]; 4],
    dc: [Huffman; 4],
    ac: [Huffman; 4],
    restart_interval: usize,
    frame: Option<Frame>,
    jfif: bool,
    adobe_transform: Option<u8>,
    eobrun: u32,
}

/// The whole picture, or the name of the refusal.
pub fn decode(bytes: &[u8]) -> Result<Image, Unsupported> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != SOI {
        return Err(Unsupported::Corrupt);
    }
    let mut decoder = Decoder {
        bytes,
        quant: [[0; 64]; 4],
        dc: Default::default(),
        ac: Default::default(),
        restart_interval: 0,
        frame: None,
        jfif: false,
        adobe_transform: None,
        eobrun: 0,
    };
    let mut at = 2;
    loop {
        while bytes.get(at) == Some(&0xFF) && bytes.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        if bytes.get(at) != Some(&0xFF) {
            return Err(Unsupported::Corrupt);
        }
        let marker = *bytes.get(at + 1).ok_or(Unsupported::Corrupt)?;
        at += 2;
        match marker {
            EOI => break,
            RST0..=RST7 | 0x01 => continue,
            _ => {}
        }
        let len = be16(bytes, at).ok_or(Unsupported::Corrupt)?;
        if len < 2 {
            return Err(Unsupported::Corrupt);
        }
        let segment = bytes.get(at + 2..at + len).ok_or(Unsupported::Corrupt)?;
        match marker {
            DQT => decoder.quant_tables(segment)?,
            DHT => decoder.huffman_tables(segment)?,
            DRI => decoder.restart_interval = be16(segment, 0).ok_or(Unsupported::Corrupt)?,
            APP0 => {
                if segment.starts_with(b"JFIF\0") {
                    decoder.jfif = true;
                }
            }
            APP14 => {
                if segment.starts_with(b"Adobe") && segment.len() >= 12 {
                    decoder.adobe_transform = Some(segment[11]);
                }
            }
            0xC0..=0xCF if marker != 0xC8 && marker != 0xCC => decoder.frame(marker, segment)?,
            SOS => {
                let end = decoder.scan(segment, at + len)?;
                at = end;
                continue;
            }
            _ => {}
        }
        at += len;
    }
    decoder.finish()
}

impl<'a> Decoder<'a> {
    fn quant_tables(&mut self, mut segment: &[u8]) -> Result<(), Unsupported> {
        while !segment.is_empty() {
            let precision = segment[0] >> 4;
            let id = (segment[0] & 0x0F) as usize;
            if id > 3 {
                return Err(Unsupported::Corrupt);
            }
            let size = if precision == 0 { 64 } else { 128 };
            let values = segment.get(1..1 + size).ok_or(Unsupported::Corrupt)?;
            for k in 0..64 {
                let value = if precision == 0 {
                    values[k] as u16
                } else {
                    u16::from_be_bytes([values[2 * k], values[2 * k + 1]])
                };
                // stored in zigzag order; kept in natural order
                self.quant[id][ZIGZAG[k]] = value;
            }
            segment = &segment[1 + size..];
        }
        Ok(())
    }

    fn huffman_tables(&mut self, mut segment: &[u8]) -> Result<(), Unsupported> {
        while segment.len() >= 17 {
            let class = segment[0] >> 4;
            let id = (segment[0] & 0x0F) as usize;
            if id > 3 || class > 1 {
                return Err(Unsupported::Corrupt);
            }
            let mut counts = [0u8; 16];
            counts.copy_from_slice(&segment[1..17]);
            let total: usize = counts.iter().map(|&c| c as usize).sum();
            let symbols = segment.get(17..17 + total).ok_or(Unsupported::Corrupt)?.to_vec();
            let table = Huffman::new(&counts, symbols);
            if class == 0 {
                self.dc[id] = table;
            } else {
                self.ac[id] = table;
            }
            segment = &segment[17 + total..];
        }
        Ok(())
    }

    fn frame(&mut self, marker: u8, segment: &[u8]) -> Result<(), Unsupported> {
        match marker {
            0xC0 | 0xC1 | 0xC2 => {}
            0xC3 | 0xC7 | 0xCB | 0xCF => return Err(Unsupported::Lossless),
            0xC5 | 0xC6 | 0xCD | 0xCE => return Err(Unsupported::Hierarchical),
            0xC9 | 0xCA => return Err(Unsupported::Arithmetic),
            _ => return Err(Unsupported::Corrupt),
        }
        if self.frame.is_some() {
            return Err(Unsupported::Corrupt);
        }
        let precision = *segment.first().ok_or(Unsupported::Corrupt)?;
        if precision != 8 {
            return Err(Unsupported::TwelveBit);
        }
        let height = be16(segment, 1).ok_or(Unsupported::Corrupt)?;
        let width = be16(segment, 3).ok_or(Unsupported::Corrupt)?;
        let count = *segment.get(5).ok_or(Unsupported::Corrupt)? as usize;
        if width == 0 || height == 0 {
            // a zero height (DNL later) is not a road this decoder walks
            return Err(Unsupported::Corrupt);
        }
        if !fits(width as u32, height as u32) {
            return Err(Unsupported::TooLarge);
        }
        match count {
            1 | 3 => {}
            4 => {
                return Err(if self.adobe_transform == Some(2) {
                    Unsupported::Ycck
                } else {
                    Unsupported::Cmyk
                })
            }
            _ => return Err(Unsupported::Corrupt),
        }
        let mut components = Vec::with_capacity(count);
        for index in 0..count {
            let spec = segment.get(6 + index * 3..9 + index * 3).ok_or(Unsupported::Corrupt)?;
            let (h, v) = ((spec[1] >> 4) as usize, (spec[1] & 0x0F) as usize);
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) || spec[2] > 3 {
                return Err(Unsupported::Corrupt);
            }
            components.push(Component {
                id: spec[0],
                h,
                v,
                quant: spec[2] as usize,
                blocks_w: 0,
                blocks_h: 0,
                coeffs: Vec::new(),
                dc_table: 0,
                ac_table: 0,
                dc_pred: 0,
            });
        }
        let hmax = components.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = components.iter().map(|c| c.v).max().unwrap_or(1);
        let mcus_w = width.div_ceil(8 * hmax);
        let mcus_h = height.div_ceil(8 * vmax);
        for component in &mut components {
            component.blocks_w = mcus_w * component.h;
            component.blocks_h = mcus_h * component.v;
            component.coeffs = vec![0; component.blocks_w * component.blocks_h * 64];
        }
        self.frame = Some(Frame {
            width,
            height,
            progressive: marker == 0xC2,
            components,
            hmax,
            vmax,
            mcus_w,
            mcus_h,
        });
        Ok(())
    }

    /// One scan: its header, then the entropy-coded segment. Answers
    /// where the next marker begins.
    fn scan(&mut self, segment: &[u8], data_at: usize) -> Result<usize, Unsupported> {
        let count = *segment.first().ok_or(Unsupported::Corrupt)? as usize;
        if count == 0 || count > 4 {
            return Err(Unsupported::Corrupt);
        }
        let frame = self.frame.as_mut().ok_or(Unsupported::Corrupt)?;
        let mut members: Vec<usize> = Vec::with_capacity(count);
        for index in 0..count {
            let spec = segment.get(1 + index * 2..3 + index * 2).ok_or(Unsupported::Corrupt)?;
            let which = frame
                .components
                .iter()
                .position(|c| c.id == spec[0])
                .ok_or(Unsupported::Corrupt)?;
            let component = &mut frame.components[which];
            component.dc_table = (spec[1] >> 4) as usize;
            component.ac_table = (spec[1] & 0x0F) as usize;
            if component.dc_table > 3 || component.ac_table > 3 {
                return Err(Unsupported::Corrupt);
            }
            members.push(which);
        }
        let tail = segment.get(1 + count * 2..4 + count * 2).ok_or(Unsupported::Corrupt)?;
        let (ss, se) = (tail[0] as usize, tail[1] as usize);
        let (ah, al) = ((tail[2] >> 4) as u32, (tail[2] & 0x0F) as u32);
        if se > 63 || ss > se || al > 13 {
            return Err(Unsupported::Corrupt);
        }
        let progressive = frame.progressive;
        if !progressive && (ss != 0 || se != 63 || ah != 0 || al != 0) {
            return Err(Unsupported::Corrupt);
        }
        for &which in &members {
            frame.components[which].dc_pred = 0;
        }
        self.eobrun = 0;
        let mut bits = Bits::new(self.bytes, data_at);
        let restart = self.restart_interval;
        let frame = self.frame.as_mut().ok_or(Unsupported::Corrupt)?;
        // an interleaved scan walks MCUs; a single-component scan walks
        // that component's own blocks, unpadded
        if members.len() == 1 {
            let which = members[0];
            let (blocks_w, blocks_h) = {
                let c = &frame.components[which];
                (
                    (frame.width * c.h).div_ceil(frame.hmax).div_ceil(8),
                    (frame.height * c.v).div_ceil(frame.vmax).div_ceil(8),
                )
            };
            let mut since_restart = 0usize;
            for by in 0..blocks_h {
                for bx in 0..blocks_w {
                    if restart > 0 && since_restart == restart {
                        if !bits.restart() {
                            return Err(Unsupported::Corrupt);
                        }
                        frame.components[which].dc_pred = 0;
                        self.eobrun = 0;
                        since_restart = 0;
                    }
                    let component = &mut frame.components[which];
                    let block = by * component.blocks_w + bx;
                    decode_block(
                        &mut bits,
                        component,
                        block,
                        &self.dc,
                        &self.ac,
                        progressive,
                        ss,
                        se,
                        ah,
                        al,
                        &mut self.eobrun,
                    )?;
                    since_restart += 1;
                }
            }
        } else {
            let mut since_restart = 0usize;
            for my in 0..frame.mcus_h {
                for mx in 0..frame.mcus_w {
                    if restart > 0 && since_restart == restart {
                        if !bits.restart() {
                            return Err(Unsupported::Corrupt);
                        }
                        for &which in &members {
                            frame.components[which].dc_pred = 0;
                        }
                        self.eobrun = 0;
                        since_restart = 0;
                    }
                    for &which in &members {
                        let component = &mut frame.components[which];
                        for v in 0..component.v {
                            for h in 0..component.h {
                                let block =
                                    (my * component.v + v) * component.blocks_w + mx * component.h + h;
                                decode_block(
                                    &mut bits,
                                    component,
                                    block,
                                    &self.dc,
                                    &self.ac,
                                    progressive,
                                    ss,
                                    se,
                                    ah,
                                    al,
                                    &mut self.eobrun,
                                )?;
                            }
                        }
                    }
                    since_restart += 1;
                }
            }
        }
        Ok(bits.end_of_scan())
    }

    /// Every scan closed: dequantize, transform, upsample, convert.
    fn finish(self) -> Result<Image, Unsupported> {
        let frame = self.frame.ok_or(Unsupported::Corrupt)?;
        let (width, height) = (frame.width, frame.height);
        // one 8-bit plane per component, at the component's own size
        let mut planes: Vec<Plane> = Vec::with_capacity(frame.components.len());
        for component in &frame.components {
            let quant = &self.quant[component.quant];
            let stride = component.blocks_w * 8;
            let mut samples = vec![0u8; stride * component.blocks_h * 8];
            let mut block = [0i32; 64];
            for by in 0..component.blocks_h {
                for bx in 0..component.blocks_w {
                    let base = (by * component.blocks_w + bx) * 64;
                    for k in 0..64 {
                        block[k] = component.coeffs[base + k] as i32 * quant[k] as i32;
                    }
                    idct_islow(&block, &mut samples, (by * 8) * stride + bx * 8, stride);
                }
            }
            planes.push(Plane {
                samples,
                stride,
                width: (width * component.h).div_ceil(frame.hmax),
                height: (height * component.v).div_ceil(frame.vmax),
                h: component.h,
                v: component.v,
            });
        }
        let mut rgba = vec![0u8; width * height * 4];
        if planes.len() == 1 {
            let plane = &planes[0];
            for y in 0..height {
                for x in 0..width {
                    let s = plane.samples[y * plane.stride + x];
                    rgba[(y * width + x) * 4..][..4].copy_from_slice(&[s, s, s, 255]);
                }
            }
            return Ok(Image { width: width as u32, height: height as u32, rgba });
        }
        // the chroma planes to full size
        let full: Vec<Vec<u8>> = planes
            .iter()
            .map(|plane| upsample(plane, frame.hmax, frame.vmax, width, height))
            .collect();
        // which color space: JFIF says YCbCr; Adobe says by its flag;
        // component ids spelling R, G, B say RGB; else YCbCr
        let ids: Vec<u8> = frame.components.iter().map(|c| c.id).collect();
        let ycbcr = if self.jfif {
            true
        } else if let Some(transform) = self.adobe_transform {
            transform != 0
        } else {
            ids != [b'R', b'G', b'B']
        };
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                let (a, b, c) = (full[0][i], full[1][i], full[2][i]);
                let (r, g, bl) = if ycbcr { ycbcr_to_rgb(a, b, c) } else { (a, b, c) };
                rgba[i * 4..][..4].copy_from_slice(&[r, g, bl, 255]);
            }
        }
        Ok(Image { width: width as u32, height: height as u32, rgba })
    }
}

// MARK: - one block

/// One block's coefficients for this scan — the baseline road, or
/// one of the four progressive roads (DC first, DC refine, AC first,
/// AC refine), the way libjpeg's `jdhuff` and `jdphuff` walk them.
#[allow(clippy::too_many_arguments)]
fn decode_block(
    bits: &mut Bits,
    component: &mut Component,
    block: usize,
    dc_tables: &[Huffman; 4],
    ac_tables: &[Huffman; 4],
    progressive: bool,
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
    eobrun: &mut u32,
) -> Result<(), Unsupported> {
    let dc = &dc_tables[component.dc_table];
    let ac = &ac_tables[component.ac_table];
    let base = block * 64;
    let coeffs = &mut component.coeffs[base..base + 64];
    if !progressive {
        // baseline: DC then the 63 AC in one go
        if !dc.present || !ac.present {
            return Err(Unsupported::Corrupt);
        }
        let t = bits.decode(dc).ok_or(Unsupported::Corrupt)? as u32;
        let diff = bits.receive_extend(t);
        component.dc_pred += diff;
        coeffs[0] = component.dc_pred as i16;
        let mut k = 1;
        while k < 64 {
            let rs = bits.decode(ac).ok_or(Unsupported::Corrupt)?;
            let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
            if s == 0 {
                if r == 15 {
                    k += 16;
                    continue;
                }
                break;
            }
            k += r;
            if k > 63 {
                return Err(Unsupported::Corrupt);
            }
            coeffs[ZIGZAG[k]] = bits.receive_extend(s) as i16;
            k += 1;
        }
        return Ok(());
    }
    if ss == 0 {
        // a DC scan (Se is 0 by the spec)
        if ah == 0 {
            if !dc.present {
                return Err(Unsupported::Corrupt);
            }
            let t = bits.decode(dc).ok_or(Unsupported::Corrupt)? as u32;
            let diff = bits.receive_extend(t);
            component.dc_pred += diff;
            coeffs[0] = (component.dc_pred << al) as i16;
        } else if bits.bit() != 0 {
            coeffs[0] |= (1 << al) as i16;
        }
        return Ok(());
    }
    if !ac.present {
        return Err(Unsupported::Corrupt);
    }
    if ah == 0 {
        // AC first: the band's coefficients, EOB runs across blocks
        if *eobrun > 0 {
            *eobrun -= 1;
            return Ok(());
        }
        let mut k = ss;
        while k <= se {
            let rs = bits.decode(ac).ok_or(Unsupported::Corrupt)?;
            let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
            if s != 0 {
                k += r;
                if k > 63 {
                    return Err(Unsupported::Corrupt);
                }
                coeffs[ZIGZAG[k]] = (bits.receive_extend(s) << al) as i16;
                k += 1;
            } else if r == 15 {
                k += 16;
            } else {
                *eobrun = 1 << r;
                if r > 0 {
                    *eobrun += bits.bits(r as u32);
                }
                *eobrun -= 1;
                break;
            }
        }
        return Ok(());
    }
    // AC refine: one more bit for every coefficient already nonzero,
    // and the new ones of this bit's magnitude — jdphuff's road
    let p1: i16 = 1 << al;
    let m1: i16 = -1 << al;
    let mut k = ss;
    if *eobrun == 0 {
        while k <= se {
            let rs = bits.decode(ac).ok_or(Unsupported::Corrupt)?;
            let (mut r, s) = ((rs >> 4) as i32, (rs & 15) as u32);
            let mut value: i16 = 0;
            if s != 0 {
                if s != 1 {
                    return Err(Unsupported::Corrupt);
                }
                value = if bits.bit() != 0 { p1 } else { m1 };
            } else if r != 15 {
                *eobrun = 1 << r;
                if r > 0 {
                    *eobrun += bits.bits(r as u32);
                }
                break;
            }
            // advance over the coefficients already nonzero (each takes
            // a correction bit) and `r` still-zero ones
            while k <= se {
                let coef = &mut coeffs[ZIGZAG[k]];
                if *coef != 0 {
                    if bits.bit() != 0 && (*coef & p1) == 0 {
                        *coef = if *coef >= 0 { *coef + p1 } else { *coef + m1 };
                    }
                } else {
                    if r == 0 {
                        break;
                    }
                    r -= 1;
                }
                k += 1;
            }
            if value != 0 && k <= se {
                coeffs[ZIGZAG[k]] = value;
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        // the rest of the band: correction bits only
        while k <= se {
            let coef = &mut coeffs[ZIGZAG[k]];
            if *coef != 0 && bits.bit() != 0 && (*coef & p1) == 0 {
                *coef = if *coef >= 0 { *coef + p1 } else { *coef + m1 };
            }
            k += 1;
        }
        *eobrun -= 1;
    }
    Ok(())
}

// MARK: - the inverse DCT (jidctint, "islow")

const CONST_BITS: i32 = 13;
const PASS1_BITS: i32 = 2;
const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433;
const FIX_0_765366865: i32 = 6270;
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633;
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137;
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;

fn descale(x: i32, n: i32) -> i32 {
    (x + (1 << (n - 1))) >> n
}

fn clamp_sample(x: i32) -> u8 {
    (x + 128).clamp(0, 255) as u8
}

/// The accurate integer inverse DCT of the IJG code, bit for bit —
/// dequantized coefficients in natural order in, 64 samples out at
/// `out[at + row * stride + col]`.
fn idct_islow(block: &[i32; 64], out: &mut [u8], at: usize, stride: usize) {
    let mut ws = [0i32; 64];
    // pass 1: columns, into the workspace scaled up by PASS1_BITS
    for col in 0..8 {
        let c = |row: usize| block[row * 8 + col];
        if c(1) == 0 && c(2) == 0 && c(3) == 0 && c(4) == 0 && c(5) == 0 && c(6) == 0 && c(7) == 0 {
            let dc = c(0) << PASS1_BITS;
            for row in 0..8 {
                ws[row * 8 + col] = dc;
            }
            continue;
        }
        let z2 = c(2);
        let z3 = c(6);
        let z1 = (z2 + z3) * FIX_0_541196100;
        let tmp2 = z1 + z3 * (-FIX_1_847759065);
        let tmp3 = z1 + z2 * FIX_0_765366865;
        let z2 = c(0);
        let z3 = c(4);
        let tmp0 = (z2 + z3) << CONST_BITS;
        let tmp1 = (z2 - z3) << CONST_BITS;
        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;
        let tmp0 = c(7);
        let tmp1 = c(5);
        let tmp2 = c(3);
        let tmp3 = c(1);
        let z1 = tmp0 + tmp3;
        let z2 = tmp1 + tmp2;
        let z3 = tmp0 + tmp2;
        let z4 = tmp1 + tmp3;
        let z5 = (z3 + z4) * FIX_1_175875602;
        let tmp0 = tmp0 * FIX_0_298631336;
        let tmp1 = tmp1 * FIX_2_053119869;
        let tmp2 = tmp2 * FIX_3_072711026;
        let tmp3 = tmp3 * FIX_1_501321110;
        let z1 = z1 * (-FIX_0_899976223);
        let z2 = z2 * (-FIX_2_562915447);
        let z3 = z3 * (-FIX_1_961570560) + z5;
        let z4 = z4 * (-FIX_0_390180644) + z5;
        let tmp0 = tmp0 + z1 + z3;
        let tmp1 = tmp1 + z2 + z4;
        let tmp2 = tmp2 + z2 + z3;
        let tmp3 = tmp3 + z1 + z4;
        let n = CONST_BITS - PASS1_BITS;
        ws[col] = descale(tmp10 + tmp3, n);
        ws[7 * 8 + col] = descale(tmp10 - tmp3, n);
        ws[8 + col] = descale(tmp11 + tmp2, n);
        ws[6 * 8 + col] = descale(tmp11 - tmp2, n);
        ws[2 * 8 + col] = descale(tmp12 + tmp1, n);
        ws[5 * 8 + col] = descale(tmp12 - tmp1, n);
        ws[3 * 8 + col] = descale(tmp13 + tmp0, n);
        ws[4 * 8 + col] = descale(tmp13 - tmp0, n);
    }
    // pass 2: rows, out of the workspace into samples
    for row in 0..8 {
        let w = |col: usize| ws[row * 8 + col];
        let line = &mut out[at + row * stride..at + row * stride + 8];
        if w(1) == 0 && w(2) == 0 && w(3) == 0 && w(4) == 0 && w(5) == 0 && w(6) == 0 && w(7) == 0 {
            let dc = clamp_sample(descale(w(0), PASS1_BITS + 3));
            line.fill(dc);
            continue;
        }
        let z2 = w(2);
        let z3 = w(6);
        let z1 = (z2 + z3) * FIX_0_541196100;
        let tmp2 = z1 + z3 * (-FIX_1_847759065);
        let tmp3 = z1 + z2 * FIX_0_765366865;
        let tmp0 = (w(0) + w(4)) << CONST_BITS;
        let tmp1 = (w(0) - w(4)) << CONST_BITS;
        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;
        let tmp0 = w(7);
        let tmp1 = w(5);
        let tmp2 = w(3);
        let tmp3 = w(1);
        let z1 = tmp0 + tmp3;
        let z2 = tmp1 + tmp2;
        let z3 = tmp0 + tmp2;
        let z4 = tmp1 + tmp3;
        let z5 = (z3 + z4) * FIX_1_175875602;
        let tmp0 = tmp0 * FIX_0_298631336;
        let tmp1 = tmp1 * FIX_2_053119869;
        let tmp2 = tmp2 * FIX_3_072711026;
        let tmp3 = tmp3 * FIX_1_501321110;
        let z1 = z1 * (-FIX_0_899976223);
        let z2 = z2 * (-FIX_2_562915447);
        let z3 = z3 * (-FIX_1_961570560) + z5;
        let z4 = z4 * (-FIX_0_390180644) + z5;
        let tmp0 = tmp0 + z1 + z3;
        let tmp1 = tmp1 + z2 + z4;
        let tmp2 = tmp2 + z2 + z3;
        let tmp3 = tmp3 + z1 + z4;
        let n = CONST_BITS + PASS1_BITS + 3;
        line[0] = clamp_sample(descale(tmp10 + tmp3, n));
        line[7] = clamp_sample(descale(tmp10 - tmp3, n));
        line[1] = clamp_sample(descale(tmp11 + tmp2, n));
        line[6] = clamp_sample(descale(tmp11 - tmp2, n));
        line[2] = clamp_sample(descale(tmp12 + tmp1, n));
        line[5] = clamp_sample(descale(tmp12 - tmp1, n));
        line[3] = clamp_sample(descale(tmp13 + tmp0, n));
        line[4] = clamp_sample(descale(tmp13 - tmp0, n));
    }
}

// MARK: - upsampling and color

/// One component's samples at its own resolution.
struct Plane {
    samples: Vec<u8>,
    stride: usize,
    /// The real (unpadded) size of the plane.
    width: usize,
    height: usize,
    h: usize,
    v: usize,
}

impl Plane {
    fn at(&self, x: usize, y: usize) -> i32 {
        let x = x.min(self.width - 1);
        let y = y.min(self.height - 1);
        self.samples[y * self.stride + x] as i32
    }
}

/// The plane at the picture's size: libjpeg's "fancy" triangle filter
/// for the 2:1 ratios (horizontal, vertical, or both), plain
/// replication for any other — cropped to `width × height`.
fn upsample(plane: &Plane, hmax: usize, vmax: usize, width: usize, height: usize) -> Vec<u8> {
    let (fx, fy) = (hmax / plane.h, vmax / plane.v);
    let mut out = vec![0u8; width * height];
    if fx == 1 && fy == 1 {
        for y in 0..height {
            for x in 0..width {
                out[y * width + x] = plane.at(x, y) as u8;
            }
        }
        return out;
    }
    if fx == 2 && fy == 1 && hmax % plane.h == 0 {
        // h2v1: out[2i] = (3·in[i] + in[i−1] + 1) >> 2, out[2i+1] = (3·in[i] + in[i+1] + 2) >> 2
        for y in 0..height {
            for i in 0..plane.width {
                let this = plane.at(i, y);
                let left = if i == 0 { this } else { plane.at(i - 1, y) };
                let right = if i + 1 >= plane.width { this } else { plane.at(i + 1, y) };
                let (a, b) = if i == 0 {
                    (this, (this * 3 + right + 2) >> 2)
                } else if i + 1 >= plane.width {
                    ((this * 3 + left + 1) >> 2, this)
                } else {
                    ((this * 3 + left + 1) >> 2, (this * 3 + right + 2) >> 2)
                };
                if 2 * i < width {
                    out[y * width + 2 * i] = a as u8;
                }
                if 2 * i + 1 < width {
                    out[y * width + 2 * i + 1] = b as u8;
                }
            }
        }
        return out;
    }
    if fx == 1 && fy == 2 && vmax % plane.v == 0 {
        // h1v2: the same filter down the columns
        for j in 0..plane.height {
            for x in 0..width {
                let this = plane.at(x, j);
                let above = if j == 0 { this } else { plane.at(x, j - 1) };
                let below = if j + 1 >= plane.height { this } else { plane.at(x, j + 1) };
                let upper = (this * 3 + above + 1) >> 2;
                let lower = (this * 3 + below + 2) >> 2;
                if 2 * j < height {
                    out[(2 * j) * width + x] = upper as u8;
                }
                if 2 * j + 1 < height {
                    out[(2 * j + 1) * width + x] = lower as u8;
                }
            }
        }
        return out;
    }
    if fx == 2 && fy == 2 && hmax % plane.h == 0 && vmax % plane.v == 0 {
        // h2v2: vertical 3:1 sums first, then the horizontal filter on
        // the sums with the +8/+7 rounding — jdsample's h2v2_fancy_upsample
        for j in 0..plane.height {
            for (half, bias_row) in [(0usize, -1isize), (1, 1)] {
                let oy = 2 * j + half;
                if oy >= height {
                    continue;
                }
                let near = (j as isize + bias_row).clamp(0, plane.height as isize - 1) as usize;
                let colsum = |i: usize| plane.at(i, j) * 3 + plane.at(i, near);
                for i in 0..plane.width {
                    let this = colsum(i);
                    let last = if i == 0 { this } else { colsum(i - 1) };
                    let next = if i + 1 >= plane.width { this } else { colsum(i + 1) };
                    let (a, b) = if i == 0 {
                        ((this * 4 + 8) >> 4, (this * 3 + next + 7) >> 4)
                    } else if i + 1 >= plane.width {
                        ((this * 3 + last + 8) >> 4, (this * 4 + 7) >> 4)
                    } else {
                        ((this * 3 + last + 8) >> 4, (this * 3 + next + 7) >> 4)
                    };
                    if 2 * i < width {
                        out[oy * width + 2 * i] = a as u8;
                    }
                    if 2 * i + 1 < width {
                        out[oy * width + 2 * i + 1] = b as u8;
                    }
                }
            }
        }
        return out;
    }
    // any other ratio: replicate
    for y in 0..height {
        for x in 0..width {
            out[y * width + x] = plane.at(x / fx.max(1), y / fy.max(1)) as u8;
        }
    }
    out
}

const SCALEBITS: i32 = 16;
const ONE_HALF: i32 = 1 << (SCALEBITS - 1);

fn fix(x: f64) -> i32 {
    (x * (1 << SCALEBITS) as f64 + 0.5) as i32
}

/// JFIF YCbCr → RGB with libjpeg's fixed-point tables (jdcolor).
fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
    let (y, cb, cr) = (y as i32, cb as i32 - 128, cr as i32 - 128);
    let r = y + ((fix(1.40200) * cr + ONE_HALF) >> SCALEBITS);
    let g = y + ((-fix(0.34414) * cb - fix(0.71414) * cr + ONE_HALF) >> SCALEBITS);
    let b = y + ((fix(1.77200) * cb + ONE_HALF) >> SCALEBITS);
    (r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame header of the given kind, enough for `header` and for
    /// the refusals to name themselves.
    fn frame_of(sof: u8, precision: u8, components: u8) -> Vec<u8> {
        let mut bytes = vec![0xFF, SOI];
        let mut body = vec![precision, 0, 21, 0, 33, components];
        for id in 0..components {
            body.extend_from_slice(&[id + 1, 0x11, 0]);
        }
        bytes.extend_from_slice(&[0xFF, sof]);
        bytes.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        bytes.extend_from_slice(&body);
        bytes.extend_from_slice(&[0xFF, SOS, 0, 2]);
        bytes
    }

    #[test]
    fn the_header_reads_the_size_through_the_markers() {
        let mut bytes = vec![0xFF, SOI, 0xFF, APP0, 0, 4, 0, 0];
        bytes.extend_from_slice(&frame_of(0xC0, 8, 3)[2..]);
        assert_eq!(header(&bytes), Some((33, 21)));
        assert_eq!(header(b"\xFF\xD8\xFF\xD9"), None);
        assert_eq!(header(b"PNG"), None);
    }

    #[test]
    fn every_refusal_names_itself() {
        assert_eq!(decode(&frame_of(0xC3, 8, 3)).unwrap_err(), Unsupported::Lossless);
        assert_eq!(decode(&frame_of(0xC5, 8, 3)).unwrap_err(), Unsupported::Hierarchical);
        assert_eq!(decode(&frame_of(0xC9, 8, 3)).unwrap_err(), Unsupported::Arithmetic);
        assert_eq!(decode(&frame_of(0xC0, 12, 3)).unwrap_err(), Unsupported::TwelveBit);
        assert_eq!(decode(&frame_of(0xC0, 8, 4)).unwrap_err(), Unsupported::Cmyk);
        assert_eq!(decode(b"\xFF\xD8\xFF").unwrap_err(), Unsupported::Corrupt);
        assert_eq!(decode(b"not a jpeg").unwrap_err(), Unsupported::Corrupt);
    }

    #[test]
    fn a_size_beyond_the_cap_is_refused_at_the_frame() {
        let mut bytes = frame_of(0xC0, 8, 3);
        // 65535 × 65535
        bytes[7] = 0xFF;
        bytes[8] = 0xFF;
        bytes[9] = 0xFF;
        bytes[10] = 0xFF;
        assert_eq!(decode(&bytes).unwrap_err(), Unsupported::TooLarge);
        assert_eq!(header(&bytes), None);
    }

    #[test]
    fn the_idct_of_a_dc_only_block_is_flat() {
        let mut block = [0i32; 64];
        block[0] = 8 * 40; // DC of 40 after the 1/8 scale
        let mut out = vec![0u8; 64];
        idct_islow(&block, &mut out, 0, 8);
        assert!(out.iter().all(|&s| s == 168), "128 + 40 everywhere: {out:?}");
    }

    #[test]
    fn the_color_tables_match_the_jfif_law() {
        assert_eq!(ycbcr_to_rgb(128, 128, 128), (128, 128, 128));
        assert_eq!(ycbcr_to_rgb(255, 128, 128), (255, 255, 255));
        assert_eq!(ycbcr_to_rgb(0, 128, 128), (0, 0, 0));
        // pure red in YCbCr: Y 76, Cb 85, Cr 255
        let (r, g, b) = ycbcr_to_rgb(76, 85, 255);
        assert!(r >= 253 && g <= 2 && b <= 2, "{r} {g} {b}");
    }

    #[test]
    fn extend_signs_the_magnitude_categories() {
        assert_eq!(Bits::extend(0, 0), 0);
        assert_eq!(Bits::extend(1, 1), 1);
        assert_eq!(Bits::extend(0, 1), -1);
        assert_eq!(Bits::extend(0b10, 2), 2);
        assert_eq!(Bits::extend(0b01, 2), -2);
        assert_eq!(Bits::extend(0b111, 3), 7);
        assert_eq!(Bits::extend(0b000, 3), -7);
    }
}
