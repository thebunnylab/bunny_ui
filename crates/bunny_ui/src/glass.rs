//! The liquid-glass material — the one specification every pipeline
//! repeats.
//!
//! A pane of glass reads the pixels already painted behind it, blurs
//! them, bends them through a rounded lens and lays a tint over the
//! result. This module holds the whole recipe in f64: the blur pyramid
//! ([`Pyramid`]) and the per-pixel composite ([`Lens::shade`]). The CPU
//! rasterizer calls it directly; the four GPU shaders repeat it
//! textually, constant for constant, and the parity tests hold them
//! together.
//!
//! Two decisions carry the look, and both are deliberate:
//!
//! - **The blur averages in LINEAR light.** Averaging sRGB-encoded
//!   values blurs the encoding, not the image: edges lose energy and
//!   the result is the grey halo of cheap frosted glass. Every level of
//!   the pyramid decodes on read and encodes on write, which is what an
//!   `_sRGB` texture does for the GPU at no cost.
//! - **Everything after the blur is gamma space**, like the rest of the
//!   compositor. The saturation matrices this material is tuned against
//!   run on encoded values, and the same boost in linear light
//!   overshoots into candy.
//!
//! The numbers below are the material, not taste. A pane is a LENS over
//! nearly-legible content, not a frosted sheet.

use crate::layout::{Color, Corners, GlassPaint};

// MARK: - The material

/// Sigma, in device px, of level 0 of the blur pyramid. Levels compose,
/// so each one doubles the reach: 5.2, 11.6, 23.8, 47.9 device px.
///
/// This is a FLOOR: a smaller blur has no sharper level to reach, so
/// every value below it renders as level 0. The minimum glass is light
/// glass, never clear glass.
pub const SIGMA_L0: f64 = 5.2;

/// The last level of the pyramid — four levels in all.
pub const MAX_LEVEL: usize = 3;

/// How much of the rim highlight is there whatever the edge faces.
/// Nearly nothing: a strong floor reads as a painted border, and the
/// real material lets the perpendicular diagonal go quiet.
const RIM_FLOOR: f64 = 0.1;

/// Falloff of each highlight lobe around its diagonal — the curvature
/// of the rim.
const RIM_FALLOFF: f64 = 1.7;

/// Light from the top left. The highlight takes the ABSOLUTE dot
/// product, so the opposite bottom-right edge lights up too: the dual
/// lobe is the material's signature, and one lobe alone reads as a
/// shine painted on a corner.
const LIGHT_DIR: (f64, f64) = (-0.707_106_78, -0.707_106_78);

/// Rec.709 luma — the grey a saturation mixes against.
const LUMA: (f64, f64, f64) = (0.2126, 0.7152, 0.0722);

/// The counter-band at the very edge: a thinner bend opposite in sign
/// to the main lens. The interference of the two is the double contour
/// glass shows at its rim. Ratios, not absolutes, so one
/// `(band, amount)` pair still drives both.
const OUTER_AMOUNT_RATIO: f64 = 0.25;
const OUTER_HEIGHT_RATIO: f64 = 0.5;

/// The rim inherits the environment: the backdrop under it is
/// super-saturated and gained before it becomes the highlight colour,
/// so the edge reads as light from the scene and not as white paint.
const VIBRANT_SATURATION: f64 = 2.069;
const VIBRANT_GAIN: f64 = 1.45;
const VIBRANT_BIAS: f64 = 0.05;

/// The displacement field ovalises the corner: the radius that steers
/// the DIRECTION is inflated, so the field sweeps around a corner
/// instead of kinking where the arc meets the straight edge. The true
/// radius still cuts the shape — only directions soften.
const GRAD_RADIUS_FACTOR: f64 = 1.5;

/// Nine bilinear taps: a seventeen-tap gaussian at sigma 2.6 texels of
/// the DESTINATION, which is half the resolution of the source — so
/// each tap already averages a 2x2 neighbourhood and the downsample
/// rides along for free.
const TAPS: [(f64, f64); 5] = [
    (0.153_584, 0.0),
    (0.256_886, 1.444_75),
    (0.125_975, 3.373_41),
    (0.034_902, 5.307_46),
    (0.005_445, 7.248_24),
];

/// The pyramid level a blur of `sigma` device px reads, as a fraction —
/// the composite mixes the two levels it falls between, so a blur that
/// crosses a level slides instead of snapping.
pub fn level_for(sigma: f64) -> f64 {
    (sigma.max(SIGMA_L0) / SIGMA_L0).log2().clamp(0.0, MAX_LEVEL as f64)
}

/// The deepest level a blur needs built.
pub fn levels_for(sigma: f64) -> usize {
    (level_for(sigma).ceil() as usize).min(MAX_LEVEL)
}

/// The lens profile: a quarter circle. One at the rim, falling to zero
/// one band inward, with an INFINITE slope at the rim — the bend is
/// violent in the last few pixels and dies with a flat derivative into
/// the centre. A squared bevel ramps too gently and reads as emboss.
fn circle_map(x: f64) -> f64 {
    let c = x.clamp(0.0, 1.0);
    1.0 - (1.0 - c * c).max(0.0).sqrt()
}

/// The sRGB transfer function, decoding.
pub fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// [`srgb_to_linear`] for every value a channel can hold.
///
/// The decode is a `powf`, and the pyramid runs it **seventeen taps deep per
/// destination texel, four texels a tap, three channels a texel**. For one
/// card-sized pane that is around a hundred and ninety million of them, which
/// measured at three hundred milliseconds — a frame and a half, on a surface
/// whose whole job is to feel weightless.
///
/// A stored texel is a byte, so the answer is a table rather than a faster
/// curve: it is **exact**, not an approximation, and the pane's pixels are
/// bit-for-bit what the `powf` produced.
static DECODE: std::sync::LazyLock<[f64; 256]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|byte| srgb_to_linear(byte as f64 / 255.0))
});

/// The sRGB transfer function, encoding.
pub fn linear_to_srgb(value: f64) -> f64 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// The outward normal of the rounded-rect field — the analytic
/// gradient, never a screen-space derivative: a derivative is quantised
/// to 2x2 fragment quads and shows as a stair-stepped rim.
fn field_normal(center_to_point: (f64, f64), corner_center: (f64, f64)) -> (f64, f64) {
    let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
    let (sx, sy) = (sign(center_to_point.0), sign(center_to_point.1));
    let (mx, my) = (corner_center.0.max(0.0), corner_center.1.max(0.0));
    let length = mx.hypot(my);
    if length > 1e-5 {
        // inside a corner's quarter disc: the normal is radial
        (sx * mx / length, sy * my / length)
    } else if corner_center.0 > corner_center.1 {
        // along a straight edge: the normal is the dominant axis
        (sx, 0.0)
    } else {
        (0.0, sy)
    }
}

// MARK: - The blur pyramid

/// One level of the pyramid, kept as a WINDOW of the level the whole
/// viewport would have: a pane only ever samples its own
/// neighbourhood, and the window carries the margin every tap above it
/// needs. Texels are sRGB-encoded and premultiplied — byte for byte
/// what the GPU's `_sRGB` texture holds.
struct Plane {
    /// The size this level has for the WHOLE viewport. Sampling is by
    /// uv, so this is what turns a uv into a texel.
    full: (usize, usize),
    /// The window, in this level's texels.
    x0: i64,
    y0: i64,
    width: usize,
    height: usize,
    texels: Vec<[u8; 4]>,
    /// True for a level of the pyramid, which an `_sRGB` texture holds:
    /// the hardware decodes EVERY texel and then filters. False for the
    /// scene, which no format decodes: there the filter runs on encoded
    /// values and the decode comes after. The two are not the same
    /// number, and the level-0 tap lands exactly between four texels,
    /// so the difference is visible — this flag is what keeps the
    /// rasterizer and the shaders on the same answer.
    srgb: bool,
}

impl Plane {
    /// One texel, as stored — the bytes, clamped to the plane.
    fn bytes(&self, tx: i64, ty: i64) -> [u8; 4] {
        let tx = tx.clamp(0, self.full.0 as i64 - 1) - self.x0;
        let ty = ty.clamp(0, self.full.1 as i64 - 1) - self.y0;
        let tx = tx.clamp(0, self.width as i64 - 1) as usize;
        let ty = ty.clamp(0, self.height as i64 - 1) as usize;
        self.texels[ty * self.width + tx]
    }

    /// A bilinear sample in uv, answering in LINEAR light — the filter
    /// the hardware runs, decoding on the side the format decodes on.
    fn sample(&self, u: f64, v: f64) -> [f64; 4] {
        let fx = u * self.full.0 as f64 - 0.5;
        let fy = v * self.full.1 as f64 - 0.5;
        let (ix, iy) = (fx.floor(), fy.floor());
        let (rx, ry) = (fx - ix, fy - iy);
        let (ix, iy) = (ix as i64, iy as i64);
        let mut out = [0.0; 4];
        for (dx, dy, weight) in [
            (0, 0, (1.0 - rx) * (1.0 - ry)),
            (1, 0, rx * (1.0 - ry)),
            (0, 1, (1.0 - rx) * ry),
            (1, 1, rx * ry),
        ] {
            if weight == 0.0 {
                continue;
            }
            let raw = self.bytes(ix + dx, iy + dy);
            // Decoding a STORED byte is a table lookup, and the table holds
            // exactly what the transfer function answers — the arithmetic is
            // the same, the `powf` is not run a hundred and ninety million
            // times a frame.
            let texel = if self.srgb {
                [
                    DECODE[raw[0] as usize],
                    DECODE[raw[1] as usize],
                    DECODE[raw[2] as usize],
                    f64::from(raw[3]) / 255.0,
                ]
            } else {
                [
                    f64::from(raw[0]) / 255.0,
                    f64::from(raw[1]) / 255.0,
                    f64::from(raw[2]) / 255.0,
                    f64::from(raw[3]) / 255.0,
                ]
            };
            for channel in 0..4 {
                out[channel] += texel[channel] * weight;
            }
        }
        if !self.srgb {
            // colour only: a transfer function never applies to alpha
            for channel in &mut out[..3] {
                *channel = srgb_to_linear(*channel);
            }
        }
        out
    }
}

/// The blur pyramid of a pane's neighbourhood.
///
/// Level 0 is the scene at half resolution, blurred to sigma 5.2 device
/// px. Every level after it halves the resolution again and composes
/// another 5.2, so the four levels reach 5.2, 11.6, 23.8 and 47.9. The
/// downsample is FUSED into the horizontal pass: it writes the smaller
/// destination while sampling the larger source, and each bilinear tap
/// averages a 2x2 neighbourhood on the way.
pub struct Pyramid {
    levels: Vec<Plane>,
}

/// A rect in device pixels, `[x0, y0, x1, y1)`.
pub type Area = (i64, i64, i64, i64);

/// The margin, in device px, a level needs around the area the
/// composite reads: enough for every tap of every level ABOVE it.
///
/// One pass reads nine taps at up to 7.25 destination texels, plus a
/// texel of bilinear footprint. Going down a level doubles what a texel
/// is worth, so the margins compose from the top.
fn margin_for(level: usize, max_level: usize) -> i64 {
    let mut margin = 2 * (1 << (max_level + 1)) as i64;
    let mut current = max_level;
    while current > level {
        // the vertical pass (+9 texels of this level) and the
        // horizontal one (+16 texels of the level below)
        margin += 34 * (1 << current) as i64;
        current -= 1;
    }
    margin
}

fn inflate(area: Area, by: i64) -> Area {
    (area.0 - by, area.1 - by, area.2 + by, area.3 + by)
}

impl Pyramid {
    /// Builds the pyramid over `area` (device px) of a scene held as
    /// packed `0xRRGGBBAA` pixels — the rasterizer's own buffer.
    pub fn build(
        pixels: &[u32],
        width: usize,
        height: usize,
        area: Area,
        max_level: usize,
    ) -> Pyramid {
        let max_level = max_level.min(MAX_LEVEL);
        let scene_window = inflate(area, margin_for(0, max_level) + 34);
        let scene = Plane::from_scene(pixels, width, height, scene_window);
        let base = (width.div_ceil(2).max(1), height.div_ceil(2).max(1));
        let mut levels: Vec<Plane> = Vec::with_capacity(max_level + 1);
        for level in 0..=max_level {
            let full = ((base.0 >> level).max(1), (base.1 >> level).max(1));
            let texel_size = (1 << (level + 1)) as f64;
            let window = texels_of(inflate(area, margin_for(level, max_level)), texel_size, full);
            let source = levels.last().unwrap_or(&scene);
            // the horizontal pass carries the downsample and keeps the
            // extra rows the vertical one reads
            let wide = inflate_texels(window, 9, full);
            let horizontal = blur_pass(source, full, wide, (1.0, 0.0));
            levels.push(blur_pass(&horizontal, full, window, (0.0, 1.0)));
        }
        Pyramid { levels }
    }

    /// Trilinear: the two levels the blur falls between, mixed.
    fn sample(&self, level: f64, u: f64, v: f64) -> [f64; 4] {
        let top = (self.levels.len() - 1) as f64;
        let level = level.clamp(0.0, top);
        let low = level.floor();
        let high = (low + 1.0).min(top);
        let ratio = level - low;
        let near = self.levels[low as usize].sample(u, v);
        if ratio <= 0.0 || high == low {
            return near;
        }
        let far = self.levels[high as usize].sample(u, v);
        [
            near[0] + (far[0] - near[0]) * ratio,
            near[1] + (far[1] - near[1]) * ratio,
            near[2] + (far[2] - near[2]) * ratio,
            near[3] + (far[3] - near[3]) * ratio,
        ]
    }
}

/// The texel window of a device-px area at a level of `texel_size`
/// device px, clamped to the level.
fn texels_of(area: Area, texel_size: f64, full: (usize, usize)) -> (i64, i64, i64, i64) {
    let low = |value: i64| (value as f64 / texel_size).floor() as i64;
    let high = |value: i64| (value as f64 / texel_size).ceil() as i64 + 1;
    (
        low(area.0).max(0),
        low(area.1).max(0),
        high(area.2).min(full.0 as i64),
        high(area.3).min(full.1 as i64),
    )
}

fn inflate_texels(window: (i64, i64, i64, i64), by: i64, full: (usize, usize)) -> (i64, i64, i64, i64) {
    (
        (window.0 - by).max(0),
        (window.1 - by).max(0),
        (window.2 + by).min(full.0 as i64),
        (window.3 + by).min(full.1 as i64),
    )
}

impl Plane {
    /// The scene itself as a plane: already sRGB-encoded and
    /// premultiplied, which is what every level holds.
    fn from_scene(pixels: &[u32], width: usize, height: usize, window: Area) -> Plane {
        let x0 = window.0.max(0);
        let y0 = window.1.max(0);
        let x1 = window.2.min(width as i64).max(x0);
        let y1 = window.3.min(height as i64).max(y0);
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut texels = Vec::with_capacity(w * h);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = pixels[y as usize * width + x as usize];
                texels.push([
                    (pixel >> 24) as u8,
                    (pixel >> 16) as u8,
                    (pixel >> 8) as u8,
                    pixel as u8,
                ]);
            }
        }
        Plane { full: (width, height), x0, y0, width: w, height: h, texels, srgb: false }
    }
}

/// One separable pass: nine bilinear taps along `direction`, in the
/// DESTINATION's texels. The destination is `full` for the whole
/// viewport and this call fills `window` of it.
fn blur_pass(
    source: &Plane,
    full: (usize, usize),
    window: (i64, i64, i64, i64),
    direction: (f64, f64),
) -> Plane {
    let (x0, y0) = (window.0, window.1);
    let (width, height) = ((window.2 - x0).max(0) as usize, (window.3 - y0).max(0) as usize);
    let inv = (1.0 / full.0 as f64, 1.0 / full.1 as f64);
    let step = (direction.0 * inv.0, direction.1 * inv.1);
    let mut texels = Vec::with_capacity(width * height);
    for ty in 0..height {
        for tx in 0..width {
            let u = (x0 + tx as i64) as f64 * inv.0 + inv.0 * 0.5;
            let v = (y0 + ty as i64) as f64 * inv.1 + inv.1 * 0.5;
            let mut sum = [0.0f64; 4];
            for (index, (weight, offset)) in TAPS.iter().enumerate() {
                let mut add = |du: f64, dv: f64| {
                    let tap = source.sample(u + du, v + dv);
                    for channel in 0..4 {
                        sum[channel] += tap[channel] * weight;
                    }
                };
                if index == 0 {
                    add(0.0, 0.0);
                } else {
                    add(step.0 * offset, step.1 * offset);
                    add(-step.0 * offset, -step.1 * offset);
                }
            }
            texels.push([
                encode(sum[0]),
                encode(sum[1]),
                encode(sum[2]),
                (sum[3].clamp(0.0, 1.0) * 255.0).round() as u8,
            ]);
        }
    }
    Plane { full, x0, y0, width, height, texels, srgb: true }
}

fn encode(linear: f64) -> u8 {
    (linear_to_srgb(linear.clamp(0.0, 1.0)) * 255.0).round().clamp(0.0, 255.0) as u8
}

// MARK: - The composite

/// One pane of glass, everything already in device pixels — the form
/// the rasterizer and the four shaders all evaluate.
pub struct Lens {
    /// The snapped box, `[x0, y0, x1, y1)`.
    pub rect: (f64, f64, f64, f64),
    /// The four radii, already clamped to the box.
    pub radii: Corners,
    /// The material, already scaled.
    pub glass: GlassPaint,
    /// The surface, for the uv the pyramid is sampled by.
    pub viewport: (f64, f64),
}

impl Lens {
    /// How deep the pyramid must go for this pane.
    pub fn levels(&self) -> usize {
        levels_for(self.glass.blur)
    }

    /// Everything about this pane that changes what it answers — the box,
    /// the corners and every knob of the material — as bytes a memo can
    /// compare.
    #[must_use]
    pub fn signature(&self, band: (i64, i64, i64, i64)) -> Vec<u8> {
        let mut out = Vec::with_capacity(160);
        for value in [self.rect.0, self.rect.1, self.rect.2, self.rect.3] {
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        for value in [
            self.radii.top_left,
            self.radii.top_right,
            self.radii.bottom_right,
            self.radii.bottom_left,
        ] {
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        for value in [self.viewport.0, self.viewport.1] {
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        for value in [band.0, band.1, band.2, band.3] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.glass.signature());
        out
    }

    /// The device-px area the pane reads — its own box plus the reach
    /// of the lens.
    pub fn area(&self) -> Area {
        let reach = (self.glass.refraction_amount.abs()
            * (1.0 + self.glass.chromatic.max(0.0)))
        .ceil() as i64
            + 2;
        (
            self.rect.0.floor() as i64 - reach,
            self.rect.1.floor() as i64 - reach,
            self.rect.2.ceil() as i64 + reach,
            self.rect.3.ceil() as i64 + reach,
        )
    }

    /// The colour one pixel of the pane becomes, and how much of the
    /// pixel the pane covers. `None` means the pane does not reach this
    /// pixel at all.
    pub fn shade(&self, pyramid: &Pyramid, x: i64, y: i64) -> Option<(Color, f64)> {
        let point = (x as f64 + 0.5, y as f64 + 0.5);
        let half = (
            (self.rect.2 - self.rect.0) / 2.0,
            (self.rect.3 - self.rect.1) / 2.0,
        );
        let center_to_point = (
            point.0 - self.rect.0 - half.0,
            point.1 - self.rect.1 - half.1,
        );
        let radius = corner_at(center_to_point, self.radii);
        let corner_to_point = (
            center_to_point.0.abs() - half.0,
            center_to_point.1.abs() - half.1,
        );
        let corner_center = (corner_to_point.0 + radius, corner_to_point.1 + radius);
        let outside = corner_center.0.max(0.0).hypot(corner_center.1.max(0.0));
        let inside = corner_center.0.max(corner_center.1).min(0.0);
        let sdf = outside + inside - radius;
        let coverage = (0.5 - sdf).clamp(0.0, 1.0);
        if coverage <= 0.0 {
            return None;
        }
        let depth = (-sdf).max(0.0);

        // the direction field, ovalised so a corner sweeps instead of
        // kinking. The true radius still cut the shape above
        let grad_radius = (radius * GRAD_RADIUS_FACTOR).min(half.0.min(half.1));
        let normal = field_normal(
            center_to_point,
            (corner_to_point.0 + grad_radius, corner_to_point.1 + grad_radius),
        );

        // Two opposed bands on the same quarter-circle profile. The
        // main one samples INWARD — the rim MAGNIFIES what is under the
        // glass, a convex lens. Sampling outward pinches, and a pinch
        // is the loudest tell of a fake.
        let band = self.glass.refraction_band.max(1.0);
        let inner = circle_map(1.0 - depth / band);
        let outer = circle_map(1.0 - depth / (band * OUTER_HEIGHT_RATIO));
        let profile = inner - outer * OUTER_AMOUNT_RATIO;
        let bend = -self.glass.refraction_amount * profile;
        let displace = (normal.0 * bend, normal.1 * bend);

        // sharper where the lens works, frosted on the face: the blur
        // backs off by up to one level across the band, which is how
        // the material keeps the bent content legible at the rim
        let sharpen = 1.0 - (depth / band).clamp(0.0, 1.0);
        let level = (level_for(self.glass.blur) - sharpen).max(0.0);
        let inv = (1.0 / self.viewport.0, 1.0 / self.viewport.1);
        let base = (point.0 * inv.0, point.1 * inv.1);
        let at = |scale: f64| {
            pyramid.sample(
                level,
                base.0 + displace.0 * scale * inv.0,
                base.1 + displace.1 * scale * inv.1,
            )
        };
        let sampled = if self.glass.chromatic > 0.0 {
            let spread = self.glass.chromatic;
            let red = at(1.0 - spread);
            let green = at(1.0);
            let blue = at(1.0 + spread);
            [red[0], green[1], blue[2], green[3]]
        } else {
            at(1.0)
        };

        // back to the engine's colour space FIRST: the vibrancy and the
        // saturation this material is tuned against run on ENCODED
        // values, unlike the blur, which must average in linear light
        let alpha = sampled[3].max(1e-4);
        let mut rgb = [
            linear_to_srgb(sampled[0] / alpha),
            linear_to_srgb(sampled[1] / alpha),
            linear_to_srgb(sampled[2] / alpha),
        ];
        let luma = rgb[0] * LUMA.0 + rgb[1] * LUMA.1 + rgb[2] * LUMA.2;
        for channel in &mut rgb {
            *channel = (luma + (*channel - luma) * self.glass.saturation) * self.glass.brightness;
        }
        let mut color = [rgb[0], rgb[1], rgb[2], sampled[3]];

        // the tint, over
        let tint = self.glass.tint;
        let tint_alpha = tint.a as f64 / 255.0;
        for (channel, value) in [tint.r, tint.g, tint.b].into_iter().enumerate() {
            let tint_channel = value as f64 / 255.0;
            color[channel] += (tint_channel - color[channel]) * tint_alpha;
        }
        color[3] = tint_alpha + color[3] * (1.0 - tint_alpha);

        // the specular rim: a thin band lit along BOTH diagonals, in
        // the colour of the scene under it, added instead of painted
        let rim = 1.0 - (depth / self.glass.highlight_band.max(1.0)).clamp(0.0, 1.0);
        let axis = (normal.0 * LIGHT_DIR.0 + normal.1 * LIGHT_DIR.1).abs();
        let ring = RIM_FLOOR + (1.0 - RIM_FLOOR) * axis.powf(RIM_FALLOFF);
        let highlight = self.glass.highlight;
        let strength = self.glass.highlight_intensity
            * rim
            * rim
            * ring
            * (highlight.a as f64 / 255.0);
        if strength > 0.0 {
            let grey = color[0] * LUMA.0 + color[1] * LUMA.1 + color[2] * LUMA.2;
            for (channel, value) in [highlight.r, highlight.g, highlight.b].into_iter().enumerate()
            {
                let vibrant = ((grey + (color[channel] - grey) * VIBRANT_SATURATION) * VIBRANT_GAIN
                    + VIBRANT_BIAS)
                    .clamp(0.0, 1.0);
                color[channel] =
                    (color[channel] + vibrant * (value as f64 / 255.0) * strength).clamp(0.0, 1.0);
            }
        }

        // the touch: a flat wash plus a pool of light under the finger,
        // both additive and both zero by default
        let spot = if self.glass.spot_alpha > 0.0 && self.glass.spot_radius > 0.0 {
            let distance = (point.0 - self.glass.spot_center.x)
                .hypot(point.1 - self.glass.spot_center.y);
            let fall = 1.0 - (distance / self.glass.spot_radius).clamp(0.0, 1.0);
            self.glass.spot_alpha * fall * fall
        } else {
            0.0
        };
        let touch = (self.glass.sheen + spot).clamp(0.0, 1.0);
        if touch > 0.0 {
            for channel in 0..3 {
                color[channel] = (color[channel] + touch).clamp(0.0, 1.0);
            }
        }

        let byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some((
            Color { r: byte(color[0]), g: byte(color[1]), b: byte(color[2]), a: byte(color[3]) },
            coverage,
        ))
    }
}

/// Which of the four radii a point answers to: the box's own midpoint
/// splits it in quarters, and a point far from every corner reads the
/// same coverage whichever radius it picked.
fn corner_at(center_to_point: (f64, f64), radii: Corners) -> f64 {
    match (center_to_point.0 < 0.0, center_to_point.1 < 0.0) {
        (true, true) => radii.top_left,
        (false, true) => radii.top_right,
        (false, false) => radii.bottom_right,
        (true, false) => radii.bottom_left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four GPU tiers repeat this material textually, constant for
    /// constant, and a shader is a string until a device compiles it —
    /// so a number that drifts here drifts SILENTLY on every machine
    /// that cannot run one of them.
    ///
    /// This is the vulkan tier's re-bake-and-compare gate, applied to a
    /// string: the numbers below are the ones this module resolves in
    /// f64, and the shader has to be carrying the same digits.
    #[cfg(feature = "gpu")]
    #[test]
    fn the_shaders_carry_this_material_constant_for_constant() {
        let glass = crate::gpu::shaders::GLASS_FRAG_BODY;
        for (name, spelled) in [
            ("GLASS_SIGMA_L0", format!("{SIGMA_L0}")),
            ("GLASS_MAX_LEVEL", format!("{}.0", MAX_LEVEL)),
            ("GLASS_RIM_FLOOR", format!("{RIM_FLOOR}")),
            ("GLASS_RIM_FALLOFF", format!("{RIM_FALLOFF}")),
            ("GLASS_OUTER_AMOUNT_RATIO", format!("{OUTER_AMOUNT_RATIO}")),
            ("GLASS_OUTER_HEIGHT_RATIO", format!("{OUTER_HEIGHT_RATIO}")),
            ("GLASS_VIBRANT_SATURATION", format!("{VIBRANT_SATURATION}")),
            ("GLASS_VIBRANT_GAIN", format!("{VIBRANT_GAIN}")),
            ("GLASS_VIBRANT_BIAS", format!("{VIBRANT_BIAS}")),
            ("GLASS_GRAD_RADIUS_FACTOR", format!("{GRAD_RADIUS_FACTOR}")),
        ] {
            let declaration = format!("const float {name} = {spelled};");
            assert!(
                glass.contains(&declaration),
                "the glass shader must declare `{declaration}` — this module says {spelled}",
            );
        }

        // the nine taps: five weights and five offsets, the centre and
        // four mirrored pairs. The shader writes them without the
        // underscores this module groups its digits with.
        let blur = crate::gpu::shaders::BLUR_FRAG;
        // GLSL wants a float to LOOK like one: Rust prints 0.0 as `0`
        let plain = |value: f64| match format!("{value}") {
            spelled if spelled.contains('.') => spelled,
            whole => format!("{whole}.0"),
        };
        let weights: Vec<String> = TAPS.iter().map(|(weight, _)| plain(*weight)).collect();
        let offsets: Vec<String> = TAPS.iter().map(|(_, offset)| plain(*offset)).collect();
        assert!(
            blur.contains(&format!("const float BLUR_W[5] = float[5]({});", weights.join(", "))),
            "the blur weights drifted from the taps this module resolves",
        );
        assert!(
            blur.contains(&format!("const float BLUR_O[5] = float[5]({});", offsets.join(", "))),
            "the blur offsets drifted from the taps this module resolves",
        );
    }
}
