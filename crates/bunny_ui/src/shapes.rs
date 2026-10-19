//! Basic shapes reuse the vector resource understood by every paint backend.
//!
//! ## Wiring
//! Layout supplies the final box and inherited ink. The path identity includes
//! both, so retained frames and the preview wire invalidate only changed paint.

use crate::icon::{Ink, Paint, Rule, Verb};
use crate::image_engine::ImageSource;
use crate::layout::{Color, Size};

pub(crate) fn ellipse_source(size: Size, color: Color) -> Option<ImageSource> {
    let (width, height) = (size.width as f32, size.height as f32);
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (rx, ry) = (width / 2.0, height / 2.0);
    // Four cubic arcs. Scaling the two radii independently preserves an ellipse
    // at non-square sizes; a rounded rectangle would incorrectly produce a pill.
    let kappa = 0.552_284_8;
    let (kx, ky) = (rx * kappa, ry * kappa);
    Some(ImageSource::path(
        vec![
            Verb::Move(width, ry),
            Verb::Cubic(width, ry + ky, rx + kx, height, rx, height),
            Verb::Cubic(rx - kx, height, 0.0, ry + ky, 0.0, ry),
            Verb::Cubic(0.0, ry - ky, rx - kx, 0.0, rx, 0.0),
            Verb::Cubic(rx + kx, 0.0, width, ry - ky, width, ry),
            Verb::Close,
        ],
        Paint::Fill(Rule::NonZero),
        Ink::Solid(color),
        (width, height),
    ))
}
