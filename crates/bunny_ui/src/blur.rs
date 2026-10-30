//! Foreground effects composite paint without changing the placed interaction tree.
//!
//! Pixel targets paint a picture through the feed transport: each blurred
//! subtree keeps one slot ([`FeedKey::effect`]), whose texture a GPU tier
//! replaces in place when the pixels change. A blur that animates mints new
//! pixels every frame; as atlas pictures, every one of them was a fresh
//! picture on the shared shelves — too big for a shelf, a texture of its own —
//! and the atlas drained nearly every frame (T2-BUNNY-386). Element targets keep
//! their native controls and apply a CSS filter. Three separable box filters
//! approximate a Gaussian on premultiplied RGBA, including transparent edges.
//!
//! Each box is an extended one ([`Pass`]): its two outermost samples weigh a
//! fraction, so its variance follows sigma continuously. Boxes of whole
//! widths gave a blur 41 strengths between 14 points and none, the last
//! ones a third of a pixel apart. A word condensing out of a blur held each
//! strength for two or three frames and then jumped to the next, and it
//! snapped sharp from 0.8 pixels (T2-BUNNY-393).
//!
//! The six passes are most of a blur's cost — a screen blurring in is a
//! megapixel and more, every frame of the move, and it held FlexSuite's
//! set-up to half the display's rate (T2-BUNNY-388). Each pass runs
//! row-major, the vertical one sliding a sum per column down the rows; the
//! box's mean is a multiply and a shift, not a division ([`Mean`]); and a
//! picture big enough to pay for it splits its rows into bands a few
//! workers take in turn, so a slow core takes fewer. The sums and their
//! rounding are the same in every split — a band starts from the window
//! its first row would have — so the pixels never depend on how many cores
//! ran, and they are the pixels the one-core division gave.

pub(crate) fn radius(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then(|| value.min(128.0))
}

#[cfg(feature = "canvas")]
use crate::{
    image_engine::{FeedKey, ImageSource, RawImages},
    layout::{Color, DisplayList, DrawCommand, LayoutEnv, Point, Rect, Size},
};

/// The blurred picture of `layer` (over `beneath` when the layer reads a
/// backdrop), as an image command on `slot`: its pixels' identity is the
/// feed's generation, so a blur that holds still uploads nothing.
#[cfg(feature = "canvas")]
pub(crate) fn paint(
    layer: &DisplayList,
    beneath: &DisplayList,
    sigma: f64,
    viewport: Rect,
    env: &LayoutEnv<'_>,
    slot: FeedKey,
) -> Option<DrawCommand> {
    let ink = crate::raster::list_bounds(layer, env.text)?;
    let expand = |rect: Rect, reach: f64| Rect {
        origin: Point {
            x: rect.origin.x - reach,
            y: rect.origin.y - reach,
        },
        size: Size {
            width: rect.size.width + reach * 2.0,
            height: rect.size.height + reach * 2.0,
        },
    };
    // Keep the kernel's entire support outside the visible surface: pixels
    // beyond the viewport can still contribute to its first visible column.
    let reach = sigma * 4.0 + 3.0;
    let bounds = expand(ink, reach).intersection(expand(viewport, reach))?;
    if ![
        bounds.origin.x,
        bounds.origin.y,
        bounds.size.width,
        bounds.size.height,
        env.scale,
    ]
    .iter()
    .all(|value| value.is_finite())
    {
        return None;
    }
    let scale = env.scale.max(1.0).ceil() as usize;
    let factor = scale as f64;
    let x = (bounds.origin.x * factor).floor() / factor;
    let y = (bounds.origin.y * factor).floor() / factor;
    let width = ((bounds.origin.x + bounds.size.width - x) * factor).ceil() as usize;
    let height = ((bounds.origin.y + bounds.size.height - y) * factor).ceil() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let shifted = layer.translated_slice((0, layer.len()), -x, -y);
    let under = (!beneath.is_empty()).then(|| {
        let under = beneath.translated_slice((0, beneath.len()), -x, -y);
        crate::raster::rasterize_with(
            &under,
            width,
            height,
            scale,
            Color::rgba(0, 0, 0, 0),
            env.text,
            env.images,
        )
    });
    let bitmap = crate::raster::rasterize_over(
        &shifted,
        width,
        height,
        scale,
        Color::rgba(0, 0, 0, 0),
        env.text,
        env.images,
        under.as_ref(),
    );
    let mut pixels: Vec<[u16; 4]> = bitmap
        .pixels()
        .iter()
        .map(|pixel| pixel.to_be_bytes().map(|channel| u16::from(channel) * 256))
        .collect();
    let mut scratch = vec![[0; 4]; pixels.len()];
    let split = Split::for_picture(width, height);
    let pass = Pass::for_sigma(sigma * factor);
    for _ in 0..3 {
        across_rows(&pixels, &mut scratch, width, pass, split);
        down_columns(&scratch, &mut pixels, width, height, pass, split);
    }
    // The raster's transparent canvas is premultiplied. Images cross every
    // backend as straight alpha, so unpremultiply once, after all six passes.
    let rgba: Vec<u8> = pixels
        .into_iter()
        .flat_map(|pixel| {
            let alpha = u32::from(pixel[3]);
            if alpha == 0 {
                return [0; 4];
            }
            let straight =
                |channel: u16| ((u32::from(channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            [
                straight(pixel[0]),
                straight(pixel[1]),
                straight(pixel[2]),
                ((alpha + 128) / 256).min(255) as u8,
            ]
        })
        .collect();
    // These pixels are already decoded. RawImages supplies the existing
    // content-and-size identity, but its envelope is not a platform codec.
    let generation = ImageSource::from_bytes(RawImages::encode(width as u32, height as u32, &rgba)).key();
    Some(DrawCommand::Image {
        rect: Rect {
            origin: Point { x, y },
            size: Size {
                width: width as f64 / factor,
                height: height as f64 / factor,
            },
        },
        source: ImageSource::feed(slot, generation, (width as u32, height as u32), rgba),
        // the layer was rastered on the grid already
        placement: crate::layout::ImagePlacement::Pixel,
    })
}

/// Pixels below this a single core blurs faster than threads can start.
#[cfg(feature = "canvas")]
const PARALLEL_FROM: usize = 1 << 16;
/// The most workers a blur takes: past it the passes are memory's, not
/// the cores'.
#[cfg(feature = "canvas")]
const WORKERS: usize = 8;
/// Bands per worker: small enough that a fast core takes the bands a slow
/// one has not reached.
#[cfg(feature = "canvas")]
const BANDS_PER_WORKER: usize = 4;
/// The fewest rows a band holds.
#[cfg(feature = "canvas")]
const BAND_ROWS: usize = 16;

/// How the passes over one picture divide: `workers` threads taking bands
/// of `rows` rows in turn. One worker on the web, where a page has no
/// threads to give, and for a picture too small to pay for them.
#[cfg(feature = "canvas")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Split {
    workers: usize,
    rows: usize,
}

#[cfg(feature = "canvas")]
impl Split {
    fn for_picture(width: usize, height: usize) -> Split {
        let one = Split { workers: 1, rows: height.max(1) };
        if cfg!(target_arch = "wasm32") || width * height < PARALLEL_FROM {
            return one;
        }
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(WORKERS);
        let rows = height.div_ceil(workers * BANDS_PER_WORKER).max(BAND_ROWS);
        if workers < 2 || rows >= height {
            return one;
        }
        Split { workers, rows }
    }

    /// Runs `work` over every band, the bands taken in turn by the
    /// workers. Each band is its index and whatever `bands` paired with it.
    fn run<B: Send>(self, bands: impl Iterator<Item = B>, work: impl Fn(B) + Sync) {
        if self.workers == 1 {
            return bands.for_each(work);
        }
        let queue = std::sync::Mutex::new(bands.collect::<Vec<_>>().into_iter());
        let next = || queue.lock().map_or(None, |mut bands| bands.next());
        std::thread::scope(|scope| {
            for _ in 0..self.workers {
                scope.spawn(|| {
                    while let Some(band) = next() {
                        work(band);
                    }
                });
            }
        });
    }
}

/// One of the blur's three passes: an extended box. The `2 * radius + 1`
/// samples around a pixel weigh `unit` each, and the sample just past each
/// end weighs `edge`. For `α = edge / unit` the pass's variance,
/// `(r(r + 1)(2r + 1) / 3 + 2α(r + 1)²) / (2r + 1 + 2α)`, runs
/// continuously from a plain box of radius `r` (α = 0) to one of radius
/// `r + 1` (α = 1). Three of them, each carrying a third of sigma², follow
/// a Gaussian as closely as three plain boxes do, at any sigma (Gwosdek,
/// Grewenig, Bruhn and Weickert, *Theoretical foundations of Gaussian
/// convolution by extended box filtering*, 2011).
#[cfg(feature = "canvas")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pass {
    radius: usize,
    edge: u32,
    unit: u32,
}

#[cfg(feature = "canvas")]
impl Pass {
    /// The finest a sample's weight divides: 256ths, where the lightest
    /// edge blurs by 0.15 of a pixel.
    const FINEST: u32 = 256;

    /// The pass that, run three times, blurs by `sigma` pixels.
    fn for_sigma(sigma: f64) -> Pass {
        let third = sigma * sigma / 3.0;
        // the widest plain box whose variance, r(r + 1) / 3, is no more
        // than a third of sigma² (the loops settle the square root's last bit)
        let mut radius = (((12.0 * third + 1.0).sqrt() - 1.0) / 2.0).max(0.0) as usize;
        while radius > 0 && (radius * (radius + 1)) as f64 > 3.0 * third {
            radius -= 1;
        }
        while ((radius + 1) * (radius + 2)) as f64 <= 3.0 * third {
            radius += 1;
        }
        let r = radius as f64;
        // solves the variance above for α; below (r + 1)(r + 2) / 3 the
        // denominator stays positive and α below one
        let alpha = (2.0f64.mul_add(r, 1.0) * (third - r * (r + 1.0) / 3.0))
            / (2.0 * (r + 1.0).mul_add(r + 1.0, -third));
        let unit = Self::unit(radius);
        let edge = (alpha * f64::from(unit)).round() as u32;
        if edge >= unit {
            Pass { radius: radius + 1, edge: 0, unit: Self::unit(radius + 1) }
        } else {
            Pass { radius, edge, unit }
        }
    }

    /// A whole sample's weight in a box this wide: the finest that keeps
    /// the weights' total below 2¹⁶, so a weighted sum of sixteen-bit
    /// channels fits 32 bits. Boxes up to 125 pixels a side keep 256ths;
    /// the widest a blur asks for (radius 383: 128 points at three pixels
    /// a point) weighs its edges in 85ths.
    fn unit(radius: usize) -> u32 {
        (usize::from(u16::MAX) / (2 * radius + 3)).min(Self::FINEST as usize) as u32
    }

    /// The weights' total, which the weighted sum is divided by.
    fn divisor(self) -> u32 {
        self.unit * (2 * self.radius as u32 + 1) + 2 * self.edge
    }

    /// The weighted sum around a pixel: `inner` the sum of the box's own
    /// samples, `before` and `after` the samples just past its two ends.
    fn weigh(self, inner: [u32; 4], before: [u16; 4], after: [u16; 4]) -> [u32; 4] {
        std::array::from_fn(|channel| {
            self.unit * inner[channel] + self.edge * (u32::from(before[channel]) + u32::from(after[channel]))
        })
    }
}

/// A pass's rounded mean as a multiply and a shift, not a division: for
/// `n = sum + d / 2` and `m = ⌈2⁴⁸ / d⌉ = (2⁴⁸ + e) / d` with `0 ≤ e < d`,
/// `n·m / 2⁴⁸ = ⌊n / d⌋ + r/d + n·e / (d·2⁴⁸)` where `r = n mod d`, so the
/// floor is exact while `n·e < 2⁴⁸`. Sixteen-bit channels under weights
/// that total `d`, plus `d / 2`, keep `n < 2¹⁶·d`, and `e < d`, so every
/// divisor below 2¹⁶ is exact — a pass's always is ([`Pass::unit`]) — and
/// `n·m` stays below 2⁶⁴.
#[cfg(feature = "canvas")]
#[derive(Clone, Copy)]
struct Mean {
    half: u32,
    scale: u64,
}

#[cfg(feature = "canvas")]
impl Mean {
    const SHIFT: u32 = 48;

    fn over(divisor: u32) -> Mean {
        Mean { half: divisor / 2, scale: (1u64 << Self::SHIFT).div_ceil(u64::from(divisor)) }
    }

    fn of(self, sum: [u32; 4]) -> [u16; 4] {
        sum.map(|channel| ((u64::from(channel + self.half) * self.scale) >> Self::SHIFT) as u16)
    }
}

#[cfg(feature = "canvas")]
fn add(sum: &mut [u32; 4], pixel: [u16; 4]) {
    for (total, channel) in sum.iter_mut().zip(pixel) {
        *total += u32::from(channel);
    }
}

#[cfg(feature = "canvas")]
fn take(sum: &mut [u32; 4], pixel: [u16; 4]) {
    for (total, channel) in sum.iter_mut().zip(pixel) {
        *total -= u32::from(channel);
    }
}

/// The horizontal pass: each pixel the rounded mean of the pass's
/// weights around it in its row.
#[cfg(feature = "canvas")]
fn across_rows(source: &[[u16; 4]], target: &mut [[u16; 4]], width: usize, pass: Pass, split: Split) {
    let band = split.rows * width;
    split.run(source.chunks(band).zip(target.chunks_mut(band)), |(from, to)| {
        box_rows(from, to, width, pass);
    });
}

/// The vertical pass: each pixel the rounded mean of the pass's
/// weights around it in its column.
#[cfg(feature = "canvas")]
fn down_columns(
    source: &[[u16; 4]],
    target: &mut [[u16; 4]],
    width: usize,
    height: usize,
    pass: Pass,
    split: Split,
) {
    split.run(target.chunks_mut(split.rows * width).enumerate(), |(at, rows)| {
        box_columns(source, rows, width, height, at * split.rows, pass);
    });
}

/// A sliding extended box along each row of `source` into the same row
/// of `target`. Samples outside the layer are transparent, never clamped:
/// each row is copied between `radius + 1` transparent samples a side, so
/// the samples a pixel's window reads are three slices of the copy, offset
/// from one another, with no bounds to test.
#[cfg(feature = "canvas")]
fn box_rows(source: &[[u16; 4]], target: &mut [[u16; 4]], width: usize, pass: Pass) {
    let mean = Mean::over(pass.divisor());
    let reach = pass.radius + 1;
    let mut padded = vec![[0u16; 4]; width + 2 * reach];
    for (row, out) in source.chunks_exact(width).zip(target.chunks_exact_mut(width)) {
        padded[reach..reach + width].copy_from_slice(row);
        // pixel x's box is padded[x + 1..=x + 2r + 1]: its edges are
        // padded[x] and padded[x + 2r + 2], and padded[x + 1] leaves it next
        let mut sum = [0u32; 4];
        for &pixel in &padded[1..2 * reach] {
            add(&mut sum, pixel);
        }
        let before = &padded[..width];
        let gone = &padded[1..=width];
        let after = &padded[2 * reach..2 * reach + width];
        for (slot, ((&first, &left), &last)) in out.iter_mut().zip(before.iter().zip(gone).zip(after)) {
            *slot = mean.of(pass.weigh(sum, first, last));
            take(&mut sum, left);
            add(&mut sum, last);
        }
    }
}

/// A sliding extended box down each column of `source`, for the rows of
/// `target`, the first of which is row `first`: a sum per column starts
/// from that row's own window and slides down a row at a time, in the
/// sweep that writes the row. Samples outside the layer are transparent,
/// never clamped.
#[cfg(feature = "canvas")]
fn box_columns(
    source: &[[u16; 4]],
    target: &mut [[u16; 4]],
    width: usize,
    height: usize,
    first: usize,
    pass: Pass,
) {
    let mean = Mean::over(pass.divisor());
    let radius = pass.radius;
    let clear = vec![[0u16; 4]; width];
    let row = |y: Option<usize>| {
        y.filter(|&y| y < height)
            .map_or(clear.as_slice(), |y| &source[y * width..(y + 1) * width])
    };
    let mut sums = vec![[0u32; 4]; width];
    for y in first.saturating_sub(radius)..=(first + radius).min(height - 1) {
        for (sum, &pixel) in sums.iter_mut().zip(row(Some(y))) {
            add(sum, pixel);
        }
    }
    for (offset, out) in target.chunks_exact_mut(width).enumerate() {
        let y = first + offset;
        let before = row(y.checked_sub(radius + 1));
        let gone = row(y.checked_sub(radius));
        let after = row(Some(y + radius + 1));
        let columns = sums.iter_mut().zip(before).zip(gone).zip(after);
        for (slot, (((sum, &up), &left), &down)) in out.iter_mut().zip(columns) {
            *slot = mean.of(pass.weigh(*sum, up, down));
            take(sum, left);
            add(sum, down);
        }
    }
}

#[cfg(all(test, feature = "canvas"))]
#[path = "blur_tests.rs"]
mod tests;
