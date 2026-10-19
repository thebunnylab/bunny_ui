//! Textures of a world: the picture a material wears, and the smaller
//! copies a surface far away reads.
//!
//! The base level is the platform's decode of the source, at its own
//! size. Every level below it is made here, on the CPU, by averaging four
//! texels in LINEAR light — a gamma average darkens every minified
//! texture. The CPU rasterizer samples these levels, and a GPU tier
//! uploads the same bytes: no tier makes its own.

use std::cell::RefCell;
use std::rc::Rc;

use super::shade::{decode, encode};
use crate::image_engine::{ImageEngine, ImageSource, intrinsic_of, raster_source};

/// The largest side a texture keeps; a larger picture is resampled to it.
pub const MAX_SIDE: u32 = 2048;

/// One level: straight sRGB RGBA8, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A texture and its whole chain of smaller levels, down to one texel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub levels: Vec<Level>,
}

impl Texture {
    /// The chain for a decoded picture.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Texture {
        let mut levels = vec![Level { width, height, rgba }];
        while let Some(last) = levels.last() {
            if last.width <= 1 && last.height <= 1 {
                break;
            }
            let next = half(last);
            levels.push(next);
        }
        Texture { levels }
    }

    /// The colour at `(u, v)` (repeating), in linear light with straight
    /// alpha, read across levels by `lod` — 0 is the base, each step
    /// halves the picture.
    pub fn sample(&self, u: f32, v: f32, lod: f32) -> [f32; 4] {
        let top = (self.levels.len() - 1) as f32;
        let lod = lod.clamp(0.0, top);
        let lower = lod.floor();
        let blend = lod - lower;
        let a = bilinear(&self.levels[lower as usize], u, v);
        if blend <= 0.0 {
            return a;
        }
        let b = bilinear(&self.levels[(lower as usize + 1).min(self.levels.len() - 1)], u, v);
        [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * blend)
    }
}

/// The next level: each texel the linear average of the (up to) four
/// above it.
fn half(level: &Level) -> Level {
    let (width, height) = ((level.width / 2).max(1), (level.height / 2).max(1));
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0.0f32; 4];
            let mut count = 0.0f32;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((x * 2 + dx).min(level.width - 1), (y * 2 + dy).min(level.height - 1));
                let at = ((sy * level.width + sx) * 4) as usize;
                let p = &level.rgba[at..at + 4];
                let alpha = p[3] as f32 / 255.0;
                // colour weighted by coverage: a clear texel lends no colour
                for k in 0..3 {
                    sum[k] += decode(p[k]) * alpha;
                }
                sum[3] += alpha;
                count += 1.0;
            }
            let alpha = sum[3] / count;
            let color = |k: usize| if sum[3] > 0.0 { sum[k] / sum[3] } else { 0.0 };
            rgba.extend_from_slice(&[encode(color(0)), encode(color(1)), encode(color(2)), (alpha * 255.0 + 0.5).floor() as u8]);
        }
    }
    Level { width, height, rgba }
}

fn texel(level: &Level, x: i64, y: i64) -> [f32; 4] {
    let (w, h) = (level.width as i64, level.height as i64);
    let (x, y) = (x.rem_euclid(w), y.rem_euclid(h));
    let at = ((y * w + x) * 4) as usize;
    let p = &level.rgba[at..at + 4];
    [decode(p[0]), decode(p[1]), decode(p[2]), p[3] as f32 / 255.0]
}

/// Bilinear in linear light, repeating, texel centres at half-texels.
fn bilinear(level: &Level, u: f32, v: f32) -> [f32; 4] {
    let x = u * level.width as f32 - 0.5;
    let y = v * level.height as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (texel(level, x0, y0), texel(level, x0 + 1, y0), texel(level, x0, y0 + 1), texel(level, x0 + 1, y0 + 1));
    [0, 1, 2, 3].map(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// `log2(x)` for `x > 0`, by the exponent bits and a fixed polynomial on
/// the mantissa — the same number on every machine, within 6·10⁻⁶.
pub fn log2(x: f32) -> f32 {
    if x <= 0.0 || x.is_nan() {
        return -126.0;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127;
    let mantissa = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000) - 1.0;
    let f = mantissa;
    let fraction = 5.0381295e-06 + f * (1.442396 + f * (-0.71699065 + f * (0.45386693 + f * (-0.27236766 + f * (0.117914304 + f * -0.024827661)))));
    exponent as f32 + fraction
}

// MARK: - The cache

/// How many textures stay decoded.
const KEEP: usize = 32;

thread_local! {
    static TEXTURES: RefCell<Vec<(u64, Rc<Texture>)>> = const { RefCell::new(Vec::new()) };
}

/// The texture of `source`, decoded once by `engine` and kept. `None`
/// while the platform has not decoded it (or never will).
pub fn texture_of(engine: &dyn ImageEngine, source: &ImageSource) -> Option<Rc<Texture>> {
    let key = source.key();
    if let Some(hit) = TEXTURES.with(|cache| cache.borrow().iter().find(|(held, _)| *held == key).map(|(_, texture)| Rc::clone(texture))) {
        return Some(hit);
    }
    let (width, height) = intrinsic_of(engine, source)?;
    let scale = (MAX_SIDE as f64 / width.max(height).max(1) as f64).min(1.0);
    let (width, height) = (((width as f64 * scale) as u32).max(1), ((height as f64 * scale) as u32).max(1));
    let raster = raster_source(engine, source, width as usize, height as usize)?;
    let texture = Rc::new(Texture::from_rgba(raster.width as u32, raster.height as u32, raster.rgba.clone()));
    TEXTURES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= KEEP {
            cache.remove(0);
        }
        cache.push((key, Rc::clone(&texture)));
    });
    Some(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(side: u32) -> Vec<u8> {
        (0..side * side)
            .flat_map(|i| {
                let (x, y) = (i % side, i / side);
                if (x + y) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }
            })
            .collect()
    }

    #[test]
    fn mips_average_in_linear_light() {
        let texture = Texture::from_rgba(8, 8, checker(8));
        assert_eq!(texture.levels.len(), 4, "8, 4, 2, 1");
        let one = &texture.levels[3];
        assert_eq!((one.width, one.height), (1, 1));
        // half white and half black is half the LIGHT — code 188, not 128
        assert_eq!(one.rgba[0], encode(0.5));
        assert_eq!(one.rgba[0], 188);
    }

    #[test]
    fn a_clear_texel_lends_no_colour() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 0, 255, 0, 0, 255, 0, 255, 0, 0];
        let texture = Texture::from_rgba(2, 2, rgba);
        let one = &texture.levels[1];
        assert_eq!(&one.rgba[..3], &[255, 0, 0], "the clear green texels add no green");
        assert_eq!(one.rgba[3], 128);
    }

    #[test]
    fn sampling_repeats_and_reads_the_levels() {
        let texture = Texture::from_rgba(8, 8, checker(8));
        let base = texture.sample(0.5 / 8.0, 0.5 / 8.0, 0.0);
        assert_eq!(base, [1.0, 1.0, 1.0, 1.0], "a texel centre is that texel");
        let wrapped = texture.sample(1.0 + 0.5 / 8.0, 0.5 / 8.0, 0.0);
        assert_eq!(wrapped, base, "u past one comes round");
        let far = texture.sample(0.3, 0.7, 3.0);
        assert!((far[0] - 0.5).abs() < 0.01, "the last level is the average: {far:?}");
    }

    #[test]
    fn the_fixed_log2_follows_the_true_one() {
        for i in 1..2000 {
            let x = i as f32 * 0.37;
            assert!((log2(x) - x.log2()).abs() < 1e-5, "log2({x})");
        }
        assert_eq!(log2(1.0), 5.0381295e-06);
    }
}
