//! A snapshot against its golden: two pictures of the same preview
//! compared pixel by pixel, the difference said as a count and the box
//! that holds it — what `bunny preview --snapshot` prints per preview,
//! and what a test asserts on. Pixels only: reading and writing the
//! files is the tool's.

use std::fmt;

/// A picture: `width × height` pixels of straight RGBA, rows top down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Picture<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

impl<'a> Picture<'a> {
    /// A picture over `rgba`, when the bytes are `width × height` pixels.
    pub fn new(width: u32, height: u32, rgba: &'a [u8]) -> Option<Picture<'a>> {
        (rgba.len() == width as usize * height as usize * 4).then_some(Picture { width, height, rgba })
    }
}

/// The box the differing pixels lie in, inclusive of its last row and
/// column — where to look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What a comparison found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every channel of every pixel within the tolerance.
    Same,
    /// The pictures are not the same size: nothing to compare pixel by
    /// pixel.
    SizeDiffers { expected: (u32, u32), actual: (u32, u32) },
    /// This many pixels differ, inside this box.
    Differs { pixels: usize, bounds: Bounds },
}

impl Verdict {
    /// Is it the same picture?
    pub fn is_same(&self) -> bool {
        matches!(self, Verdict::Same)
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Same => write!(f, "same"),
            Verdict::SizeDiffers { expected, actual } => write!(f, "{}×{} where the golden is {}×{}", actual.0, actual.1, expected.0, expected.1),
            Verdict::Differs { pixels, bounds } => {
                write!(f, "{pixels} pixel(s) differ in a {}×{} box at ({}, {})", bounds.width, bounds.height, bounds.x, bounds.y)
            }
        }
    }
}

/// `actual` against `expected`: a pixel differs when any of its four
/// channels is more than `tolerance` away — 0 is exact.
pub fn compare(expected: Picture<'_>, actual: Picture<'_>, tolerance: u8) -> Verdict {
    if (expected.width, expected.height) != (actual.width, actual.height) {
        return Verdict::SizeDiffers { expected: (expected.width, expected.height), actual: (actual.width, actual.height) };
    }
    let width = expected.width as usize;
    let mut pixels = 0usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let (wanted, _) = expected.rgba.as_chunks::<4>();
    let (got, _) = actual.rgba.as_chunks::<4>();
    for (index, (want, got)) in wanted.iter().zip(got).enumerate() {
        if want.iter().zip(got).all(|(a, b)| a.abs_diff(*b) <= tolerance) {
            continue;
        }
        pixels += 1;
        let (x, y) = (index % width, index / width);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if pixels == 0 {
        return Verdict::Same;
    }
    Verdict::Differs {
        pixels,
        bounds: Bounds { x: x0 as u32, y: y0 as u32, width: (x1 - x0 + 1) as u32, height: (y1 - y0 + 1) as u32 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: u32, height: u32, fill: [u8; 4]) -> Vec<u8> {
        fill.repeat(width as usize * height as usize)
    }

    /// The same bytes are the same; a pixel moved is counted and boxed.
    #[test]
    fn a_difference_is_counted_and_boxed() {
        let golden = picture(4, 3, [10, 20, 30, 255]);
        let mut now = golden.clone();
        // pixels (1, 1) and (3, 2) move
        let at = |x: usize, y: usize| (y * 4 + x) * 4;
        now[at(1, 1)] = 200;
        now[at(3, 2) + 2] = 0;
        let expected = Picture::new(4, 3, &golden).unwrap();
        assert_eq!(compare(expected, expected, 0), Verdict::Same);
        let verdict = compare(expected, Picture::new(4, 3, &now).unwrap(), 0);
        assert_eq!(verdict, Verdict::Differs { pixels: 2, bounds: Bounds { x: 1, y: 1, width: 3, height: 2 } });
        assert_eq!(verdict.to_string(), "2 pixel(s) differ in a 3×2 box at (1, 1)");
    }

    /// Within the tolerance is the same; a size that moved is said as such.
    #[test]
    fn tolerance_and_size() {
        let golden = picture(2, 2, [100, 100, 100, 255]);
        let close = picture(2, 2, [103, 98, 100, 255]);
        let expected = Picture::new(2, 2, &golden).unwrap();
        assert!(compare(expected, Picture::new(2, 2, &close).unwrap(), 3).is_same());
        assert!(!compare(expected, Picture::new(2, 2, &close).unwrap(), 2).is_same());
        let taller = picture(2, 3, [100, 100, 100, 255]);
        let verdict = compare(expected, Picture::new(2, 3, &taller).unwrap(), 0);
        assert_eq!(verdict, Verdict::SizeDiffers { expected: (2, 2), actual: (2, 3) });
        assert_eq!(verdict.to_string(), "2×3 where the golden is 2×2");
        assert!(Picture::new(2, 2, &taller).is_none(), "the bytes must be the size");
    }
}
