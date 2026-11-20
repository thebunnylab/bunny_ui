//! `.foreground_gradient(…)` in the pixel pipelines.
//!
//! A line of text under a ramp is rastered once, in white, and each texel
//! of its coverage takes the ramp's color at its place: one picture,
//! laid where the line was, as a [`DrawCommand::Sprite`]. No backend
//! learns a new primitive, and the letters cut one gradient out of the
//! box the ramp was laid across, as CSS's `background-clip: text` does.

use crate::image_engine::FeedKey;
use crate::layout::{Affine, Color, DisplayList, DrawCommand, GradientPaint, LayoutEnv, Point};

/// The line `command` inked by `ramp`, `alpha` of the way opaque (what a
/// fade did to the line since it was drawn); `None` when it is no line
/// or shows nothing.
pub fn paint(command: &DrawCommand, ramp: &GradientPaint, alpha: f64, env: &LayoutEnv<'_>, slot: FeedKey) -> Option<DrawCommand> {
    const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };
    let white = match command.clone() {
        DrawCommand::TextLine { origin, content, range, font, .. } => DrawCommand::TextLine { origin, content, range, color: WHITE, font },
        DrawCommand::TurnedText { pivot, content, range, font, turn, .. } => {
            DrawCommand::TurnedText { pivot, content, range, color: WHITE, font, turn }
        }
        _ => return None,
    };
    // a transparent texel around the letters: a sprite's outer texels
    // fade over its edge
    let texel = 1.0 / env.scale.max(1.0).ceil();
    let mut picture = crate::picture::raster(&DisplayList::from(vec![white]), env, texel)?;
    let width = picture.size.0 as usize;
    let (left, top, scale) = (picture.rect.origin.x, picture.rect.origin.y, picture.scale);
    let (texels, _) = picture.rgba.as_chunks_mut::<4>();
    for (index, texel) in texels.iter_mut().enumerate() {
        let coverage = texel[3];
        if coverage == 0 {
            continue;
        }
        #[allow(clippy::cast_precision_loss, reason = "at most 8192 a side")]
        let at = Point { x: left + ((index % width) as f64 + 0.5) / scale, y: top + ((index / width) as f64 + 0.5) / scale };
        let ink = ramp.at(at);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a product of fractions of a byte")]
        let strength = (f64::from(coverage) * f64::from(ink.a) / 255.0 * alpha.clamp(0.0, 1.0)).round() as u8;
        *texel = [ink.r, ink.g, ink.b, strength];
    }
    Some(picture.sprite(slot, Affine::IDENTITY))
}
