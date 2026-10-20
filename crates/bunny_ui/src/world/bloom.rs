//! The bloom: light that spills around what glows.
//!
//! A frame with a bloom ([`super::frame::WorldStyle::bloom`], at
//! [`super::frame::Quality::High`]) draws a second picture beside its
//! colours: one byte a pixel, how much the pixel glows — a surface's
//! emissive light, blended as its colour is. After the pass a chain of
//! small pictures spreads the glowing colours: four steps down, each half
//! the size of the one above, then three steps back up, each its own step
//! down blended with the step below read up, so every step weighs the
//! same in the end: the near glow and the wide one alike. The chain's top
//! is laid over the picture as a screen, `c + s·(1 − c)`: it brightens,
//! and it never clips. A pixel takes the spill as far as it does not glow
//! itself: a lamp keeps its own look, and its light spills around it.
//!
//! Every step reads whole texels and sums whole codes, so the CPU's chain
//! and every tier's give the same integers. The pictures of the chain are
//! RGBA8 on a GPU: their alpha is unused.

/// How much of a pixel's glow one unit of emissive light makes: a
/// surface that gives off four times its colour glows fully.
pub const GLOW_PER_LIGHT: f32 = 0.25;

/// The steps down the chain takes, each half the size of the one above.
pub const LEVELS: usize = 4;

/// How much brighter the spill is than the chain's average: a small lamp's
/// light, spread over a wide step, still shows.
pub const GAIN: u32 = 4;

/// How much a surface glows, in `[0, 1]`, from the light it gives off
/// (linear, its strength folded in): its brightest channel.
pub fn glow(emissive: [f32; 3]) -> f32 {
    (emissive[0].max(emissive[1]).max(emissive[2]) * GLOW_PER_LIGHT).clamp(0.0, 1.0)
}

/// The size of the step below a picture of `size`.
pub fn half(size: (u32, u32)) -> (u32, u32) {
    (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1))
}

/// The sizes of the chain's steps below a picture of `size`, from the
/// largest.
pub fn sizes(size: (u32, u32)) -> [(u32, u32); LEVELS] {
    let mut out = [half(size); LEVELS];
    for level in 1..LEVELS {
        out[level] = half(out[level - 1]);
    }
    out
}

/// The weight of a texel of the 4 × 4 block a step down reads around its
/// own 2 × 2: five for the middle four, one for the ring — 32 in all.
pub fn down_weight(dx: i64, dy: i64) -> u32 {
    if (0..=1).contains(&dx) && (0..=1).contains(&dy) { 5 } else { 1 }
}

/// One picture of the chain: whole codes, alpha apart.
struct Level {
    width: u32,
    height: u32,
    rgb: Vec<[u32; 3]>,
}

impl Level {
    /// The texel at `(x, y)`, the edge repeated past it.
    fn at(&self, x: i64, y: i64) -> [u32; 3] {
        let x = x.clamp(0, self.width as i64 - 1) as u32;
        let y = y.clamp(0, self.height as i64 - 1) as u32;
        self.rgb[(y * self.width + x) as usize]
    }
}

/// A step down from a picture of `size`: each texel the weighted sum of
/// the 4 × 4 block around its own 2 × 2 above, `read` giving a texel's
/// value (the edge repeated past it), over `whole` and rounded.
fn down(size: (u32, u32), read: impl Fn(i64, i64) -> [u32; 3], whole: u32) -> Level {
    let (width, height) = half(size);
    let mut rgb = Vec::with_capacity((width * height) as usize);
    for y in 0..height as i64 {
        for x in 0..width as i64 {
            let mut sum = [0u32; 3];
            for dy in -1..=2 {
                for dx in -1..=2 {
                    let weight = down_weight(dx, dy);
                    let texel = read(2 * x + dx, 2 * y + dy);
                    for k in 0..3 {
                        sum[k] += weight * texel[k];
                    }
                }
            }
            rgb.push(sum.map(|s| (s + whole / 2) / whole));
        }
    }
    Level { width, height, rgb }
}

/// The step below read at `(x, y)` of the step above: the four texels
/// nearest, weighted 9, 3, 3 and 1 — sixteen in all, the bilinear blend
/// of a picture twice the size.
fn up16(lower: &Level, x: i64, y: i64) -> [u32; 3] {
    let (mx, my) = (x / 2, y / 2);
    let ox = if x % 2 == 0 { -1 } else { 1 };
    let oy = if y % 2 == 0 { -1 } else { 1 };
    let (near, across, below, far) = (lower.at(mx, my), lower.at(mx + ox, my), lower.at(mx, my + oy), lower.at(mx + ox, my + oy));
    [0, 1, 2].map(|k| 9 * near[k] + 3 * across[k] + 3 * below[k] + far[k])
}

/// A step up: each texel its own step down and the step below read up,
/// the step below carrying `below` steps, so each of them weighs as much
/// as the step's own: `(16·own + below·read + 8·(below + 1)) / (16·(below + 1))`.
fn up(level: &Level, lower: &Level, below: u32) -> Level {
    let whole = 16 * (below + 1);
    let mut rgb = Vec::with_capacity(level.rgb.len());
    for y in 0..level.height as i64 {
        for x in 0..level.width as i64 {
            let own = level.at(x, y);
            let read = up16(lower, x, y);
            rgb.push([0, 1, 2].map(|k| (16 * own[k] + below * read[k] + whole / 2) / whole));
        }
    }
    Level { width: level.width, height: level.height, rgb }
}

/// The 4×4 ordered dither's index at a pixel, `0..16`.
pub fn bayer_index(x: u32, y: u32) -> u32 {
    const B: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    B[(y & 3) as usize][(x & 3) as usize]
}

/// The bloom's codes at a pixel, from the chain's top read up (sixteen
/// times its codes): scaled by `strength` (256ths) and the [`GAIN`], taken
/// as far as the pixel does not glow itself, and dithered to whole codes —
/// `⌊v + (b + ½)/16⌋` for the dither's index `b`, in whole numbers.
pub fn spill(up16: [u32; 3], glow: u32, strength: u32, x: u32, y: u32) -> [u32; 3] {
    let dither = 65280 * bayer_index(x, y) + 32640;
    up16.map(|value| ((strength * GAIN * value * (255 - glow) + dither) / 1_044_480).min(255))
}

/// A code `c` under a screen of `s`: `c + s·(1 − c)`, rounded.
pub fn screen(c: u32, s: u32) -> u32 {
    (255 * c + s * (255 - c) + 127) / 255
}

/// Spreads the glow of a picture over it: `color` premultiplied codes,
/// `glow` one byte a pixel, `strength` in 256ths.
pub fn apply(color: &mut [[u8; 4]], glow: &[u8], size: (u32, u32), strength: u32) {
    let (width, height) = size;
    if width == 0 || height == 0 || color.len() != (width * height) as usize || glow.len() != color.len() {
        return;
    }
    // the glowing colours, a step down: each texel's codes times its glow
    let read = |x: i64, y: i64| {
        let i = (y.clamp(0, height as i64 - 1) as u32 * width + x.clamp(0, width as i64 - 1) as u32) as usize;
        let (c, g) = (color[i], glow[i] as u32);
        [c[0] as u32 * g, c[1] as u32 * g, c[2] as u32 * g]
    };
    let mut steps = vec![down(size, read, 32 * 255)];
    for _ in 1..LEVELS {
        let above = steps.last().expect("a step");
        let next = down((above.width, above.height), |x, y| above.at(x, y), 32);
        steps.push(next);
    }
    // back up: the smallest step read into the one above it, and so on
    let mut top = steps.pop().expect("a step");
    let mut below = 1;
    while let Some(level) = steps.pop() {
        top = up(&level, &top, below);
        below += 1;
    }
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) as usize;
            let s = spill(up16(&top, x as i64, y as i64), glow[i] as u32, strength, x, y);
            let pixel = &mut color[i];
            for k in 0..3 {
                pixel[k] = screen(pixel[k] as u32, s[k]) as u8;
            }
            pixel[3] = screen(pixel[3] as u32, s[0].max(s[1]).max(s[2])) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_that_does_not_glow_is_left_alone() {
        let size = (37, 23);
        let mut color: Vec<[u8; 4]> = (0..size.0 * size.1).map(|i| [(i * 7 % 256) as u8, (i * 13 % 256) as u8, 90, 255]).collect();
        let before = color.clone();
        apply(&mut color, &vec![0; (size.0 * size.1) as usize], size, 256);
        assert_eq!(color, before);
    }

    #[test]
    fn a_glowing_point_spills_around_itself() {
        let size = (64, 48);
        let mut color = vec![[20u8, 20, 30, 255]; (size.0 * size.1) as usize];
        let mut glow = vec![0u8; color.len()];
        for y in 22..26 {
            for x in 30..34 {
                color[y * 64 + x] = [255, 200, 120, 255];
                glow[y * 64 + x] = 255;
            }
        }
        apply(&mut color, &glow, size, 154);
        let at = |x: usize, y: usize| color[y * 64 + x];
        assert!(at(29, 24)[0] > 40, "the light spills next door: {:?}", at(29, 24));
        assert!(at(26, 24)[0] > at(20, 24)[0], "and fades with the distance");
        assert!(at(26, 24)[0] - 20 > at(26, 24)[2] - 30, "in the light's own colour: {:?}", at(26, 24));
        assert_eq!(at(0, 0), [20, 20, 30, 255], "a far corner keeps its colour");
        assert!(color.iter().all(|pixel| pixel[3] == 255), "an opaque picture stays opaque");
    }

    #[test]
    fn every_size_is_drawn() {
        for size in [(1, 1), (2, 1), (3, 5), (17, 2)] {
            let count = (size.0 * size.1) as usize;
            let mut color = vec![[200u8, 100, 50, 255]; count];
            apply(&mut color, &vec![255; count], size, 256);
            assert!(color.iter().all(|pixel| pixel[0] >= 200));
        }
        assert_eq!(sizes((1920, 1080)), [(960, 540), (480, 270), (240, 135), (120, 68)]);
    }

    #[test]
    fn a_screen_brightens_and_never_clips() {
        assert_eq!(screen(0, 0), 0);
        assert_eq!(screen(255, 255), 255);
        assert_eq!(screen(100, 0), 100);
        assert_eq!(screen(0, 77), 77);
        assert!((0..=255).all(|c| (0..=255).all(|s| screen(c, s) >= c.max(s) && screen(c, s) <= 255)));
    }
}
