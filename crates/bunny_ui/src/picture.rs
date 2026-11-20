//! A piece of the display list rastered once into a picture, for the
//! effects that work on its pixels: a turn lays the picture on the scene
//! turned ([`crate::turn`]), a ramp tints the coverage of a line of text
//! ([`crate::ink`]). The picture crosses every backend as one
//! [`DrawCommand::Sprite`], which they already sample under an affine map.

use crate::image_engine::{FeedKey, ImageSource, RawImages};
use crate::layout::{Affine, Color, DisplayList, DrawCommand, LayoutEnv, Point, Rect, Size};

/// A picture past this many texels a side would hold a screen's worth
/// many times over.
const MOST: f64 = 8192.0;

/// Straight RGBA texels at the scene's device scale, over `rect`.
pub struct Picture {
    pub rect: Rect,
    pub size: (u32, u32),
    pub rgba: Vec<u8>,
    /// Device pixels per point.
    pub scale: f64,
}

/// The picture of `layer`'s ink, grown by `margin` points on each side
/// and snapped out to whole device pixels.
pub fn raster(layer: &DisplayList, env: &LayoutEnv<'_>, margin: f64) -> Option<Picture> {
    let ink = crate::raster::list_bounds(layer, env.text)?;
    let scale = env.scale.max(1.0).ceil();
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
    #[allow(clippy::cast_precision_loss, reason = "at most 8192")]
    let rect = Rect { origin: Point { x: left, y: top }, size: Size { width: width as f64 / scale, height: height as f64 / scale } };
    #[allow(clippy::cast_possible_truncation, reason = "at most 8192")]
    let size = (width as u32, height as u32);
    Some(Picture { rect, size, rgba, scale })
}

impl Picture {
    /// The picture on `slot`, laid on the scene by `transform`.
    pub fn sprite(self, slot: FeedKey, transform: Affine) -> DrawCommand {
        let generation = ImageSource::from_bytes(RawImages::encode(self.size.0, self.size.1, &self.rgba)).key();
        DrawCommand::Sprite { rect: self.rect, source: ImageSource::feed(slot, generation, self.size, self.rgba), transform, opacity: 1.0 }
    }
}
