//! `.rotation_effect(…)` in the pixel pipelines.
//!
//! The subtree's commands are rastered once into a picture, upright, and
//! the picture is laid on the scene turned — one [`DrawCommand::Sprite`],
//! which every backend already samples under an affine map, its edges
//! fading over a texel. No backend learns a new primitive: a turned text,
//! a turned image and a turned rounded box are the same picture.

#[cfg(feature = "canvas")]
use crate::{
    image_engine::{FeedKey, ImageSource, RawImages},
    layout::{Affine, Color, DisplayList, DrawCommand, LayoutEnv, Point, Rect, Size},
};

/// The map that turns the plane `degrees` clockwise about `pivot` (the
/// screen's y points down, so a positive angle turns clockwise, as CSS
/// and `SwiftUI` turn).
#[must_use]
pub fn about(degrees: f64, pivot: crate::layout::Point) -> crate::layout::Affine {
    let (sin, cos) = degrees.to_radians().sin_cos();
    crate::layout::Affine {
        a: cos,
        b: sin,
        c: -sin,
        d: cos,
        e: sin.mul_add(pivot.y, (-cos).mul_add(pivot.x, pivot.x)),
        f: (-cos).mul_add(pivot.y, (-sin).mul_add(pivot.x, pivot.y)),
    }
}

/// The box `rect` covers once turned by `map`: its four corners' bounds.
#[must_use]
pub fn bounds_of(rect: crate::layout::Rect, map: crate::layout::Affine) -> crate::layout::Rect {
    let corners = [
        (rect.origin.x, rect.origin.y),
        (rect.origin.x + rect.size.width, rect.origin.y),
        (rect.origin.x, rect.origin.y + rect.size.height),
        (rect.origin.x + rect.size.width, rect.origin.y + rect.size.height),
    ]
    .map(|(x, y)| (map.a.mul_add(x, map.c.mul_add(y, map.e)), map.b.mul_add(x, map.d.mul_add(y, map.f))));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (x, y) in corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    crate::layout::Rect {
        origin: crate::layout::Point { x: x0, y: y0 },
        size: crate::layout::Size { width: x1 - x0, height: y1 - y0 },
    }
}

/// A picture past this many texels a side would hold a screen's worth
/// many times over.
#[cfg(feature = "canvas")]
const MOST: f64 = 8192.0;

/// The picture of `layer`, upright, on `slot`, laid on the scene by `map`.
/// The picture keeps a transparent texel around its ink, so the turned
/// edge fades instead of stepping.
#[cfg(feature = "canvas")]
pub(crate) fn paint(layer: &DisplayList, map: Affine, env: &LayoutEnv<'_>, slot: FeedKey) -> Option<DrawCommand> {
    let ink = crate::raster::list_bounds(layer, env.text)?;
    let scale = env.scale.max(1.0).ceil();
    let margin = 1.0 / scale;
    let left = ((ink.origin.x - margin) * scale).floor() / scale;
    let top = ((ink.origin.y - margin) * scale).floor() / scale;
    let right = ink.origin.x + ink.size.width + margin;
    let bottom = ink.origin.y + ink.size.height + margin;
    if ![left, top, right, bottom].iter().all(|value| value.is_finite()) {
        return None;
    }
    let width = ((right - left) * scale).ceil().clamp(0.0, MOST);
    let height = ((bottom - top) * scale).ceil().clamp(0.0, MOST);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..=8192 first")]
    let (width, height, factor) = (width as usize, height as usize, scale as usize);
    if width == 0 || height == 0 {
        return None;
    }
    let shifted = layer.translated_slice((0, layer.len()), -left, -top);
    let bitmap = crate::raster::rasterize_with(&shifted, width, height, factor, Color::rgba(0, 0, 0, 0), env.text, env.images);
    // the raster's transparent canvas is premultiplied; a picture crosses
    // every backend as straight alpha
    let rgba: Vec<u8> = bitmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let [red, green, blue, alpha] = pixel.to_be_bytes();
            if alpha == 0 {
                return [0; 4];
            }
            let straight = |channel: u8| u8::try_from((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).unwrap_or(u8::MAX);
            [straight(red), straight(green), straight(blue), alpha]
        })
        .collect();
    #[allow(clippy::cast_possible_truncation, reason = "at most 8192")]
    let size = (width as u32, height as u32);
    let generation = ImageSource::from_bytes(RawImages::encode(size.0, size.1, &rgba)).key();
    #[allow(clippy::cast_precision_loss, reason = "at most 8192")]
    let rect = Rect { origin: Point { x: left, y: top }, size: Size { width: width as f64 / scale, height: height as f64 / scale } };
    Some(DrawCommand::Sprite { rect, source: ImageSource::feed(slot, generation, size, rgba), transform: map, opacity: 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Point, Rect, Size};

    #[test]
    fn a_quarter_turn_about_the_centre_swaps_the_box() {
        let rect = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: 40.0, height: 10.0 } };
        let turned = bounds_of(rect, about(90.0, Point { x: 20.0, y: 5.0 }));
        assert!((turned.origin.x - 15.0).abs() < 1e-9 && (turned.origin.y + 15.0).abs() < 1e-9, "{turned:?}");
        assert!((turned.size.width - 10.0).abs() < 1e-9 && (turned.size.height - 40.0).abs() < 1e-9, "{turned:?}");
    }

    #[test]
    fn a_positive_angle_turns_clockwise() {
        // the top-right corner of a box about its centre goes down
        let map = about(90.0, Point { x: 0.0, y: 0.0 });
        let (x, y) = (map.a + map.e, map.b + map.f);
        assert!(x.abs() < 1e-9 && (y - 1.0).abs() < 1e-9, "({x}, {y})");
    }
}
