//! Foreground effects composite paint without changing the placed interaction tree.
//!
//! Pixel targets use the existing immutable image transport; element targets keep
//! their native controls and apply a CSS filter. Three separable box filters
//! approximate a Gaussian on premultiplied RGBA, including transparent edges.

pub(crate) fn radius(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then(|| value.min(128.0))
}

#[cfg(feature = "canvas")]
use crate::{
    image_engine::{ImageSource, RawImages},
    layout::{Color, DisplayList, DrawCommand, LayoutEnv, Point, Rect, Size},
};

#[cfg(feature = "canvas")]
pub(crate) fn paint(
    layer: &DisplayList,
    beneath: &DisplayList,
    sigma: f64,
    viewport: Rect,
    env: &LayoutEnv<'_>,
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
    // A box of width w has variance (w² - 1) / 12. Mix the two
    // neighbouring odd widths so three boxes approximate sigma².
    let physical_sigma = sigma * factor;
    let ideal = (4.0 * physical_sigma * physical_sigma + 1.0).sqrt();
    let lower = (ideal.floor() as usize).max(1) | 1;
    let lower = if lower as f64 > ideal {
        lower.saturating_sub(2).max(1)
    } else {
        lower
    };
    let upper = lower + 2;
    let low_count = ((12.0 * physical_sigma * physical_sigma
        - 3.0 * (lower * lower) as f64
        - 12.0 * lower as f64
        - 9.0)
        / (-4.0 * lower as f64 - 4.0))
        .round()
        .clamp(0.0, 3.0) as usize;
    for pass in 0..3 {
        let radius = (if pass < low_count { lower } else { upper }) / 2;
        box_pass(&pixels, &mut scratch, width, height, radius);
        box_pass(&scratch, &mut pixels, height, width, radius);
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
    let key = ImageSource::from_bytes(RawImages::encode(width as u32, height as u32, &rgba)).key();
    Some(DrawCommand::Image {
        rect: Rect {
            origin: Point { x, y },
            size: Size {
                width: width as f64 / factor,
                height: height as f64 / factor,
            },
        },
        source: ImageSource::rgba(key, (width as u32, height as u32), rgba),
        // the layer was rastered on the grid already
        placement: crate::layout::ImagePlacement::Pixel,
    })
}

/// A sliding box, transposed on output so the vertical pass has the same
/// contiguous reads. Samples outside the layer are transparent, never clamped.
#[cfg(feature = "canvas")]
fn box_pass(
    source: &[[u16; 4]],
    target: &mut [[u16; 4]],
    width: usize,
    height: usize,
    radius: usize,
) {
    let divisor = (radius * 2 + 1) as u32;
    for y in 0..height {
        let row = &source[y * width..(y + 1) * width];
        let mut sum = [0u32; 4];
        for pixel in row.iter().take(radius + 1) {
            for channel in 0..4 {
                sum[channel] += u32::from(pixel[channel]);
            }
        }
        for x in 0..width {
            target[x * height + y] = sum.map(|channel| ((channel + divisor / 2) / divisor) as u16);
            if x >= radius {
                for channel in 0..4 {
                    sum[channel] -= u32::from(row[x - radius][channel]);
                }
            }
            if let Some(next) = row.get(x + radius + 1) {
                for channel in 0..4 {
                    sum[channel] += u32::from(next[channel]);
                }
            }
        }
    }
}
