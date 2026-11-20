//! The computed values a browser answers, read back: lengths, colors,
//! shadows, gradients and transforms, in the spellings `getComputedStyle`
//! uses.

/// A color in straight sRGB, alpha 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f64,
}

impl Rgba {
    /// Fully transparent: nothing to paint.
    #[must_use]
    pub fn invisible(self) -> bool {
        self.a <= 0.0
    }

    /// The bunny spelling.
    #[must_use]
    pub fn code(self) -> String {
        let alpha = channel(self.a * 255.0);
        if alpha == 255 {
            format!("Color::rgb({}, {}, {})", self.r, self.g, self.b)
        } else {
            format!("Color::rgba({}, {}, {}, {alpha})", self.r, self.g, self.b)
        }
    }
}

const fn channel(value: f64) -> u8 {
    // a channel is a byte by construction: clamped, then rounded
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..=255 first")]
    let byte = value.clamp(0.0, 255.0).round() as u8;
    byte
}

/// A length in CSS pixels: `12px`, `0`, `-3.5px`. Anything else — `auto`,
/// `normal`, a percentage — is not a pixel length.
#[must_use]
pub fn px(value: &str) -> Option<f64> {
    let value = value.trim();
    if value == "0" {
        return Some(0.0);
    }
    value.strip_suffix("px")?.trim().parse().ok()
}

/// A plain number: `1`, `0.5`.
#[must_use]
pub fn number(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

/// A percentage: `50%` is 0.5.
#[must_use]
pub fn percent(value: &str) -> Option<f64> {
    value.trim().strip_suffix('%')?.trim().parse::<f64>().ok().map(|value| value / 100.0)
}

/// `rgb(…)`, `rgba(…)`, `color(srgb …)` and `transparent`.
#[must_use]
pub fn color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if value == "transparent" {
        return Some(Rgba { r: 0, g: 0, b: 0, a: 0.0 });
    }
    if let Some(inner) = value.strip_prefix("rgba(").or_else(|| value.strip_prefix("rgb(")) {
        let inner = inner.strip_suffix(')')?;
        let parts: Vec<&str> = inner.split([',', ' ', '/']).filter(|part| !part.is_empty()).collect();
        let channel_at = |index: usize| parts.get(index).and_then(|part| number(part)).map(channel);
        let alpha = parts.get(3).and_then(|part| number(part).or_else(|| percent(part))).unwrap_or(1.0);
        return Some(Rgba { r: channel_at(0)?, g: channel_at(1)?, b: channel_at(2)?, a: alpha });
    }
    if let Some(inner) = value.strip_prefix("color(srgb ") {
        let inner = inner.strip_suffix(')')?;
        let (channels, alpha) = inner.split_once('/').map_or((inner, None), |(left, right)| (left, Some(right)));
        let unit: Vec<f64> = channels.split_whitespace().filter_map(number).collect();
        if unit.len() != 3 {
            return None;
        }
        let alpha = alpha.and_then(|alpha| number(alpha).or_else(|| percent(alpha))).unwrap_or(1.0);
        return Some(Rgba { r: channel(unit[0] * 255.0), g: channel(unit[1] * 255.0), b: channel(unit[2] * 255.0), a: alpha });
    }
    None
}

/// Splits at the commas that are not inside parentheses.
#[must_use]
pub fn split_top(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, character) in value.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if character == separator && depth == 0 => {
                parts.push(value[start..at].trim());
                start = at + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(value[start..].trim());
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

/// The words of a value, a function call kept whole.
fn words(value: &str) -> Vec<&str> {
    split_top(value, ' ')
}

/// One `box-shadow` layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: Rgba,
    pub x: f64,
    pub y: f64,
    pub blur: f64,
    pub spread: f64,
    pub inset: bool,
}

/// The layers of a computed `box-shadow` (`none` has none).
#[must_use]
pub fn shadows(value: &str) -> Vec<Shadow> {
    if value.trim() == "none" {
        return Vec::new();
    }
    split_top(value, ',')
        .into_iter()
        .filter_map(|layer| {
            let mut color = None;
            let mut lengths = Vec::new();
            let mut inset = false;
            for word in words(layer) {
                if word == "inset" {
                    inset = true;
                } else if let Some(length) = px(word) {
                    lengths.push(length);
                } else if let Some(found) = self::color(word) {
                    color = Some(found);
                }
            }
            let at = |index: usize| lengths.get(index).copied().unwrap_or(0.0);
            Some(Shadow { color: color?, x: at(0), y: at(1), blur: at(2), spread: at(3), inset })
        })
        .collect()
}

/// A color stop: the color, and where on the gradient line, when named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub color: Rgba,
    pub at: Option<Place>,
}

/// Where a stop stands: a fraction of the gradient line, or a length
/// along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    Fraction(f64),
    Px(f64),
}

impl Place {
    fn of(value: &str) -> Option<Self> {
        percent(value).map(Self::Fraction).or_else(|| px(value).map(Self::Px))
    }

    /// The fraction of a line `length` px long.
    #[must_use]
    pub fn fraction(self, length: f64) -> f64 {
        match self {
            Self::Fraction(fraction) => fraction,
            Self::Px(px) => px / length.max(1e-9),
        }
    }
}

/// One layer of a computed `background-image`.
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    /// `linear-gradient(…)`: the angle in degrees, clockwise from up.
    Linear { degrees: f64, stops: Vec<Stop>, repeating: bool },
    /// `radial-gradient(…)`: the shape words, and the stops.
    Radial { shape: String, stops: Vec<Stop>, repeating: bool },
    /// `conic-gradient(…)`.
    Conic,
    /// `url("…")`.
    Url(String),
}

/// The layers of a computed `background-image` (`none` has none).
#[must_use]
pub fn images(value: &str) -> Vec<Image> {
    if value.trim() == "none" {
        return Vec::new();
    }
    split_top(value, ',').into_iter().filter_map(image).collect()
}

fn image(layer: &str) -> Option<Image> {
    let (name, rest) = layer.split_once('(')?;
    let inner = rest.strip_suffix(')')?;
    let repeating = name.starts_with("repeating-");
    match name.trim_start_matches("repeating-") {
        "url" => Some(Image::Url(inner.trim().trim_matches(['"', '\'']).to_owned())),
        "conic-gradient" => Some(Image::Conic),
        "linear-gradient" => {
            let parts = split_top(inner, ',');
            let (degrees, from) = parts.first().and_then(|first| direction(first)).map_or((180.0, 0), |degrees| (degrees, 1));
            Some(Image::Linear { degrees, stops: stops(&parts[from..]), repeating })
        }
        "radial-gradient" => {
            let parts = split_top(inner, ',');
            let shaped = parts.first().is_some_and(|first| color(first.split(' ').next().unwrap_or("")).is_none());
            let shape = if shaped { parts[0].to_owned() } else { String::new() };
            Some(Image::Radial { shape, stops: stops(&parts[usize::from(shaped)..]), repeating })
        }
        _ => None,
    }
}

/// `135deg`, `0.5turn`, `to right`, `to top left`: the gradient line's
/// direction in degrees, clockwise from up.
fn direction(value: &str) -> Option<f64> {
    let value = value.trim();
    if let Some(degrees) = value.strip_suffix("deg") {
        return degrees.trim().parse().ok();
    }
    if let Some(turns) = value.strip_suffix("turn") {
        return turns.trim().parse::<f64>().ok().map(|turns| turns * 360.0);
    }
    if let Some(radians) = value.strip_suffix("rad") {
        return radians.trim().parse::<f64>().ok().map(f64::to_degrees);
    }
    let sides = value.strip_prefix("to ")?;
    let (mut x, mut y) = (0.0_f64, 0.0_f64);
    for side in sides.split_whitespace() {
        match side {
            "top" => y = -1.0,
            "bottom" => y = 1.0,
            "left" => x = -1.0,
            "right" => x = 1.0,
            _ => return None,
        }
    }
    Some(x.atan2(-y).to_degrees())
}

/// The stops of a gradient; a stop that names two places (`red 20%
/// 40%`) is two stops of one color.
fn stops(parts: &[&str]) -> Vec<Stop> {
    parts
        .iter()
        .filter_map(|part| {
            let words = words(part);
            let color = color(words.first()?)?;
            let places: Vec<Place> = words[1..].iter().filter_map(|word| Place::of(word)).collect();
            Some(if places.is_empty() { vec![Stop { color, at: None }] } else { places.into_iter().map(|at| Stop { color, at: Some(at) }).collect() })
        })
        .flatten()
        .collect()
}

/// A computed `transform`: `matrix(a, b, c, d, e, f)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    /// The rotation, in degrees clockwise.
    #[must_use]
    pub fn degrees(self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }

    /// The scale along x and y.
    #[must_use]
    pub fn scale(self) -> (f64, f64) {
        (self.a.hypot(self.b), self.c.hypot(self.d))
    }

    /// The shear left after the rotation and the scale: zero for a plain
    /// rotate-scale-translate.
    #[must_use]
    pub fn shear(self) -> f64 {
        self.a.mul_add(self.c, self.b * self.d)
    }
}

/// `none` is none; a 2D `matrix(…)` is read; a `matrix3d(…)` is `Err`
/// with its text.
///
/// # Errors
///
/// A transform with depth, which no flat matrix holds.
pub fn transform(value: &str) -> Result<Option<Matrix>, String> {
    let value = value.trim();
    if value == "none" {
        return Ok(None);
    }
    let Some(inner) = value.strip_prefix("matrix(").and_then(|rest| rest.strip_suffix(')')) else {
        return Err(value.to_owned());
    };
    let values: Vec<f64> = inner.split(',').filter_map(number).collect();
    match values.as_slice() {
        &[a, b, c, d, translate_x, translate_y] => Ok(Some(Matrix { a, b, c, d, e: translate_x, f: translate_y })),
        _ => Err(value.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_read_in_every_spelling_the_browser_answers() {
        assert_eq!(color("rgb(52, 120, 246)"), Some(Rgba { r: 52, g: 120, b: 246, a: 1.0 }));
        assert_eq!(color("rgba(0, 0, 0, 0.25)").map(|c| c.a), Some(0.25));
        assert_eq!(color("color(srgb 1 0.5 0 / 0.5)"), Some(Rgba { r: 255, g: 128, b: 0, a: 0.5 }));
        assert!(color("transparent").is_some_and(Rgba::invisible));
        assert_eq!(color("rgba(0, 0, 0, 0.25)").map(Rgba::code).as_deref(), Some("Color::rgba(0, 0, 0, 64)"));
    }

    #[test]
    fn a_shadow_list_reads_layer_by_layer() {
        let layers = shadows("rgba(0, 0, 0, 0.2) 0px 20px 40px 0px, rgb(255, 255, 255) 0px 0px 0px 2px inset");
        assert_eq!(layers.len(), 2);
        assert_eq!((layers[0].y, layers[0].blur), (20.0, 40.0));
        assert!(layers[1].inset && (layers[1].spread - 2.0).abs() < 1e-9);
    }

    #[test]
    fn gradients_read_their_direction_and_stops() {
        let layers = images("linear-gradient(135deg, rgb(255, 0, 0) 0%, rgb(0, 0, 255) 100%), url(\"/assets/a.png\")");
        assert_eq!(layers.len(), 2);
        match &layers[0] {
            Image::Linear { degrees, stops, .. } => {
                assert!((degrees - 135.0).abs() < 1e-9);
                assert_eq!(stops.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(layers[1], Image::Url("/assets/a.png".to_owned()));
        assert!(matches!(images("linear-gradient(to right, rgb(0, 0, 0), rgb(1, 1, 1))")[0], Image::Linear { degrees, .. } if (degrees - 90.0).abs() < 1e-9));
        let Image::Linear { stops, .. } = &images("linear-gradient(rgb(0, 0, 0), rgb(255, 0, 0) 20% 40%, rgb(1, 1, 1) 30px)")[0] else { panic!("a line") };
        let places: Vec<Option<Place>> = stops.iter().map(|stop| stop.at).collect();
        assert_eq!(places, [None, Some(Place::Fraction(0.2)), Some(Place::Fraction(0.4)), Some(Place::Px(30.0))]);
    }

    #[test]
    fn a_rotation_reads_from_its_matrix() {
        let turned = transform("matrix(0.978148, 0.207912, -0.207912, 0.978148, 0, 0)").ok().flatten().expect("a matrix");
        assert!((turned.degrees() - 12.0).abs() < 0.01);
        assert!(turned.shear().abs() < 1e-6);
        assert!(transform("matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1)").is_err());
    }
}
