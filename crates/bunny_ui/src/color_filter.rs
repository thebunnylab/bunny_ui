//! `SwiftUI`'s color filters, drawn as the page draws them.
//!
//! `.grayscale`, `.saturation`, `.contrast`, `.hue_rotation` and
//! `.color_invert` follow CSS Filter Effects Level 1's shorthand
//! functions, so the canvas and the page agree. Each filter is a matrix over a color's red, green and blue, and an
//! offset, read on the sRGB channels as they are stored, with straight
//! alpha: Chrome runs the shorthand functions in sRGB, never in the
//! linear light SVG's `color-interpolation-filters` starts from. Each
//! filter clamps its result to the channel's range before the next one
//! reads it, as one filter primitive's result is. Alpha never changes.
//!
//! The pixel pipelines raster the subtree once into a picture, change its
//! texels, and lay it back where it was as one [`DrawCommand::Sprite`]:
//! no backend learns a new primitive.
//!
//! [`DrawCommand::Sprite`]: crate::layout::DrawCommand::Sprite

/// One color filter, as `SwiftUI` names it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ColorFilter {
    /// `.grayscale(amount)` — 0 keeps the colors, 1 leaves only their
    /// luminance: CSS `grayscale()`.
    Grayscale(f64),
    /// `.saturation(amount)` — 0 is gray, 1 the colors as they are, past
    /// 1 richer: CSS `saturate()`.
    Saturation(f64),
    /// `.contrast(amount)` — 1 the colors as they are, 0 a flat middle
    /// gray, below 0 inverted too: CSS `contrast()`.
    Contrast(f64),
    /// `.hue_rotation(angle)` — degrees round the hue circle: CSS
    /// `hue-rotate()`.
    HueRotation(f64),
    /// `.color_invert()` — CSS `invert(1)`.
    Invert,
}

/// Three rows — red, green, blue — of weights on the three channels and
/// an offset, on channels in `0..=1`.
#[cfg(feature = "canvas")]
pub(crate) type Matrix = [[f64; 4]; 3];

impl ColorFilter {
    /// The filter a modifier asked for, as it applies: `None` for an
    /// amount that is no number, a grayscale held to `0..=1` and a
    /// saturation to zero and above, as CSS clamps them. Zero is never
    /// negative, which would print as `-0`.
    pub(crate) fn normalized(self) -> Option<ColorFilter> {
        if !self.wire().1.is_finite() {
            return None;
        }
        let zero = |amount: f64| amount + 0.0;
        Some(match self {
            ColorFilter::Grayscale(amount) => ColorFilter::Grayscale(zero(amount.clamp(0.0, 1.0))),
            ColorFilter::Saturation(amount) => ColorFilter::Saturation(zero(amount.max(0.0))),
            ColorFilter::Contrast(amount) => ColorFilter::Contrast(zero(amount)),
            ColorFilter::HueRotation(degrees) => ColorFilter::HueRotation(zero(degrees)),
            ColorFilter::Invert => ColorFilter::Invert,
        })
    }

    /// Does it leave every color as it was?
    #[must_use]
    pub fn is_identity(self) -> bool {
        const NEAR: f64 = 1e-9;
        match self {
            ColorFilter::Grayscale(amount) => amount.abs() < NEAR,
            ColorFilter::Saturation(amount) | ColorFilter::Contrast(amount) => (amount - 1.0).abs() < NEAR,
            ColorFilter::HueRotation(degrees) => {
                let turned = degrees.rem_euclid(360.0);
                turned < NEAR || 360.0 - turned < NEAR
            }
            ColorFilter::Invert => false,
        }
    }

    /// What crosses the wire: the kind (0 grayscale, 1 saturation, 2
    /// contrast, 3 hue rotation, 4 invert) and its amount, degrees for
    /// the hue and 1 for the inversion.
    pub(crate) const fn wire(self) -> (u8, f64) {
        match self {
            ColorFilter::Grayscale(amount) => (0, amount),
            ColorFilter::Saturation(amount) => (1, amount),
            ColorFilter::Contrast(amount) => (2, amount),
            ColorFilter::HueRotation(degrees) => (3, degrees),
            ColorFilter::Invert => (4, 1.0),
        }
    }

    /// The filter's matrix: CSS Filter Effects 1's for its shorthand
    /// function, `feColorMatrix` for the grayscale, the saturation and
    /// the hue, `feComponentTransfer` for the contrast and the inversion.
    #[cfg(feature = "canvas")]
    pub(crate) fn matrix(self) -> Matrix {
        match self {
            ColorFilter::Grayscale(amount) => toward([0.2126, 0.7152, 0.0722], 1.0 - amount),
            ColorFilter::Saturation(amount) => toward(SATURATE, amount),
            ColorFilter::Contrast(amount) => {
                let offset = 0.5f64.mul_add(-amount, 0.5);
                [[amount, 0.0, 0.0, offset], [0.0, amount, 0.0, offset], [0.0, 0.0, amount, offset]]
            }
            ColorFilter::HueRotation(degrees) => {
                let (sin, cos) = degrees.to_radians().sin_cos();
                // each row: the luminance's weights, and what the turn's
                // cosine and sine add to them
                let row = |by_cos: [f64; 3], by_sin: [f64; 3]| {
                    let [r, g, b] = [0, 1, 2].map(|at| sin.mul_add(by_sin[at], cos.mul_add(by_cos[at], SATURATE[at])));
                    [r, g, b, 0.0]
                };
                [
                    row([0.787, -0.715, -0.072], [-0.213, -0.715, 0.928]),
                    row([-0.213, 0.285, -0.072], [0.143, 0.140, -0.283]),
                    row([-0.213, -0.715, 0.928], [-0.787, 0.715, 0.072]),
                ]
            }
            ColorFilter::Invert => [[-1.0, 0.0, 0.0, 1.0], [0.0, -1.0, 0.0, 1.0], [0.0, 0.0, -1.0, 1.0]],
        }
    }
}

/// The luminance weights `feColorMatrix` saturates and turns the hue
/// about; CSS's `grayscale()` weighs with four places.
#[cfg(feature = "canvas")]
const SATURATE: [f64; 3] = [0.213, 0.715, 0.072];

/// The luminance's `weights` mixed with the identity: at `amount` 0 every
/// channel is the gray they weigh, at 1 each is itself, past 1 further
/// from the gray.
#[cfg(feature = "canvas")]
fn toward(weights: [f64; 3], amount: f64) -> Matrix {
    [0, 1, 2].map(|row| {
        let [r, g, b] = [0, 1, 2].map(|column| amount.mul_add(if row == column { 1.0 } else { 0.0 } - weights[column], weights[column]));
        [r, g, b, 0.0]
    })
}

/// The filters a view's colors go through, in the order they apply —
/// held inline, so the paint props stay `Copy`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ColorFilters {
    held: [Option<ColorFilter>; ColorFilters::MOST],
}

impl ColorFilters {
    /// The most one view holds; a longer chain nests a second one.
    pub const MOST: usize = 4;

    /// No filter.
    pub const NONE: ColorFilters = ColorFilters { held: [None; ColorFilters::MOST] };

    /// One filter alone.
    #[must_use]
    pub const fn one(filter: ColorFilter) -> ColorFilters {
        let mut held = [None; ColorFilters::MOST];
        held[0] = Some(filter);
        ColorFilters { held }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.held[0].is_none()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.held.iter().flatten().count()
    }

    /// The filters, in the order they apply.
    pub fn iter(&self) -> impl Iterator<Item = ColorFilter> + '_ {
        self.held.iter().flatten().copied()
    }

    /// These, then `outer`'s — `None` when the two do not fit in one.
    #[must_use]
    pub fn then(self, outer: ColorFilters) -> Option<ColorFilters> {
        let len = self.len();
        if len + outer.len() > ColorFilters::MOST {
            return None;
        }
        let mut held = self.held;
        for (slot, filter) in held[len..].iter_mut().zip(outer.iter()) {
            *slot = Some(filter);
        }
        Some(ColorFilters { held })
    }

    /// Do they leave every color as it was?
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.iter().all(ColorFilter::is_identity)
    }
}

/// The color `rgb` comes out as, on channels in `0..=1`: each matrix in
/// turn, clamped before the next.
#[cfg(feature = "canvas")]
fn through(matrices: &[Matrix], rgb: [f64; 3]) -> [f64; 3] {
    matrices.iter().fold(rgb, |[red, green, blue], matrix| {
        matrix.map(|[r, g, b, offset]| r.mul_add(red, g.mul_add(green, b.mul_add(blue, offset))).clamp(0.0, 1.0))
    })
}

/// The picture of `layer` with its colors through `filters`, laid where
/// it was. The picture keeps a transparent texel around its ink: a
/// sprite's outer texels fade over its edge.
#[cfg(feature = "canvas")]
pub(crate) fn paint(
    layer: &crate::layout::DisplayList,
    filters: &ColorFilters,
    env: &crate::layout::LayoutEnv<'_>,
    slot: crate::image_engine::FeedKey,
) -> Option<crate::layout::DrawCommand> {
    let texel = 1.0 / env.scale.max(1.0).ceil();
    let mut picture = crate::picture::raster(layer, env, texel)?;
    let matrices: Vec<Matrix> = filters.iter().map(ColorFilter::matrix).collect();
    let (texels, _) = picture.rgba.as_chunks_mut::<4>();
    for texel in texels.iter_mut().filter(|texel| texel[3] != 0) {
        let rgb = through(&matrices, [texel[0], texel[1], texel[2]].map(|channel| f64::from(channel) / 255.0));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..=1 first")]
        let [red, green, blue] = rgb.map(|channel| (channel * 255.0).round() as u8);
        *texel = [red, green, blue, texel[3]];
    }
    Some(picture.sprite(slot, crate::layout::Affine::IDENTITY))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows of the gray and the saturation each weigh to one: a gray
    /// stays the same gray under every amount.
    #[test]
    #[cfg(feature = "canvas")]
    fn a_gray_stays_gray() {
        for filter in [ColorFilter::Grayscale(0.3), ColorFilter::Saturation(2.5), ColorFilter::HueRotation(73.0)] {
            let [red, green, blue] = through(&[filter.matrix()], [0.4, 0.4, 0.4]);
            assert!((red - 0.4).abs() < 1e-3 && (green - 0.4).abs() < 1e-3 && (blue - 0.4).abs() < 1e-3, "{filter:?}");
        }
    }

    /// A chain longer than one view holds does not fold.
    #[test]
    fn a_chain_folds_while_it_fits() {
        let gray = ColorFilters::one(ColorFilter::Grayscale(1.0));
        let three = gray.then(gray).and_then(|two| two.then(gray)).expect("three fit");
        assert_eq!(three.len(), 3);
        assert_eq!(three.then(gray).map(|four| four.len()), Some(4));
        assert_eq!(three.then(gray).and_then(|four| four.then(gray)), None);
    }

    /// An amount that is no number asks for no filter; CSS's ranges hold
    /// the rest, and a zero is never written `-0`.
    #[test]
    fn an_amount_is_held_to_what_css_draws() {
        assert_eq!(ColorFilter::Saturation(f64::NAN).normalized(), None);
        assert_eq!(ColorFilter::Grayscale(3.0).normalized(), Some(ColorFilter::Grayscale(1.0)));
        assert_eq!(ColorFilter::Saturation(-1.0).normalized(), Some(ColorFilter::Saturation(0.0)));
        assert_eq!(ColorFilter::Contrast(-1.0).normalized(), Some(ColorFilter::Contrast(-1.0)), "SwiftUI inverts");
        assert!(ColorFilter::Contrast(-0.0).normalized().is_some_and(|filter| filter.wire().1.is_sign_positive()));
    }
}
