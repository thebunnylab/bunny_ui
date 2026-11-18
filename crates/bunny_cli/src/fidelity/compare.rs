//! Two captures, compared: box by box first, then pixel by pixel.
//!
//! The boxes say where a layout went wrong. The pixels under text keep a
//! bucket of their own, so two glyph rasterizers that never agree byte
//! for byte hide nothing else.

use super::capture::{Rect, Shot};

/// Two boxes closer than this agree: the browser keeps its geometry in
/// sixty-fourths of a pixel, and a layout in `f64` lands within one.
pub const TOLERANCE: f64 = 1.0 / 64.0 + 1e-6;

/// How far text's pixels reach past its box, in CSS pixels: a glyph's
/// antialiasing and its overhangs.
const TEXT_REACH: f64 = 1.0;

/// One named box that does not agree.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeDiff {
    pub id: String,
    pub first: Option<Rect>,
    pub second: Option<Rect>,
}

impl NodeDiff {
    /// The largest difference along any edge, in CSS pixels; infinite
    /// for a box one side does not have.
    #[must_use]
    pub fn worst(&self) -> f64 {
        match (self.first, self.second) {
            (Some(a), Some(b)) => [b.x - a.x, b.y - a.y, b.w - a.w, b.h - a.h].into_iter().map(f64::abs).fold(0.0, f64::max),
            _ => f64::INFINITY,
        }
    }

    /// "moved +0.50, +0.00 px; larger +1.25, +0.00 px" — or which side
    /// lacks it.
    #[must_use]
    pub fn describe(&self) -> String {
        match (self.first, self.second) {
            (Some(a), Some(b)) => {
                let mut parts = Vec::new();
                let (dx, dy, dw, dh) = (b.x - a.x, b.y - a.y, b.w - a.w, b.h - a.h);
                if dx.abs() > TOLERANCE || dy.abs() > TOLERANCE {
                    parts.push(format!("moved {dx:+.2}, {dy:+.2} px"));
                }
                if dw.abs() > TOLERANCE || dh.abs() > TOLERANCE {
                    parts.push(format!("{} {dw:+.2}, {dh:+.2} px", if dw + dh >= 0.0 { "larger" } else { "smaller" }));
                }
                parts.join("; ")
            }
            (Some(_), None) => String::from("missing on the second side"),
            (None, Some(_)) => String::from("missing on the first side"),
            (None, None) => String::new(),
        }
    }
}

/// A connected run of differing pixels, its box in CSS pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub bounds: Rect,
    pub pixels: usize,
    pub in_text: bool,
}

/// Everything one pair of captures disagrees on.
#[derive(Clone, Debug)]
pub struct Comparison {
    pub nodes: Vec<NodeDiff>,
    /// Differing pixels outside every text box.
    pub outside: usize,
    /// Differing pixels inside a text box.
    pub in_text: usize,
    /// The largest per-channel difference among the differing pixels.
    pub max_delta: u8,
    pub regions: Vec<Region>,
    /// The picture of the difference, RGBA, the captures' size.
    pub image: Vec<u8>,
}

impl Comparison {
    /// The three numbers a baseline keeps.
    #[must_use]
    pub const fn score(&self) -> [usize; 3] {
        [self.nodes.len(), self.outside, self.in_text]
    }
}

fn agrees(a: Rect, b: Rect) -> bool {
    [b.x - a.x, b.y - a.y, b.w - a.w, b.h - a.h].into_iter().all(|delta| delta.abs() <= TOLERANCE)
}

/// The named boxes that disagree, in the first side's order; a name only
/// one side has disagrees too.
#[must_use]
pub fn compare_nodes(first: &[(String, Rect)], second: &[(String, Rect)]) -> Vec<NodeDiff> {
    let lookup = |boxes: &[(String, Rect)], id: &str| boxes.iter().find(|(name, _)| name == id).map(|(_, rect)| *rect);
    let mut diffs = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for (id, _) in first.iter().chain(second) {
        if seen.contains(&id.as_str()) {
            continue;
        }
        seen.push(id);
        let (a, b) = (lookup(first, id), lookup(second, id));
        let same = match (a, b) {
            (Some(a), Some(b)) => agrees(a, b),
            _ => false,
        };
        if !same {
            diffs.push(NodeDiff { id: id.clone(), first: a, second: b });
        }
    }
    diffs
}

/// A coordinate in CSS pixels as a device pixel's index, held to
/// `0..=limit`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to the picture before the cast")]
fn device(value: f64, limit: usize) -> usize {
    let limit_f = f64::from(u32::try_from(limit).unwrap_or(u32::MAX));
    value.clamp(0.0, limit_f) as usize
}

/// A device pixel's index in CSS pixels.
fn css(index: usize, ratio: f64) -> f64 {
    f64::from(u32::try_from(index).unwrap_or(u32::MAX)) / ratio
}

/// Which device pixels lie under text on either side.
fn text_mask(first: &Shot, second: &Shot, ratio: f64) -> Vec<bool> {
    let (width, height) = (first.width as usize, first.height as usize);
    let mut mask = vec![false; width * height];
    for rect in first.texts.iter().chain(&second.texts) {
        let left = device(((rect.x - TEXT_REACH) * ratio).floor(), width);
        let top = device(((rect.y - TEXT_REACH) * ratio).floor(), height);
        let right = device(((rect.x + rect.w + TEXT_REACH) * ratio).ceil(), width);
        let bottom = device(((rect.y + rect.h + TEXT_REACH) * ratio).ceil(), height);
        for y in top..bottom {
            mask[y * width + left.min(right)..y * width + right].fill(true);
        }
    }
    mask
}

/// Compares `second` against `first`, both captured at `ratio`.
#[must_use]
pub fn compare(first: &Shot, second: &Shot, ratio: u32) -> Comparison {
    let nodes = compare_nodes(&first.boxes, &second.boxes);
    let (width, height) = (first.width as usize, first.height as usize);
    let same_size = first.width == second.width && first.height == second.height;
    let mask = text_mask(first, second, f64::from(ratio));
    let mut differs = vec![0u8; width * height];
    let (mut outside, mut in_text, mut max_delta) = (0, 0, 0u8);
    for at in 0..width * height {
        let delta = if same_size {
            (0..4).map(|channel| first.rgba[at * 4 + channel].abs_diff(second.rgba[at * 4 + channel])).max().unwrap_or(0)
        } else {
            255
        };
        if delta == 0 {
            continue;
        }
        max_delta = max_delta.max(delta);
        if mask[at] {
            in_text += 1;
            differs[at] = 2;
        } else {
            outside += 1;
            differs[at] = 1;
        }
    }
    let regions = regions(&differs, width, height, f64::from(ratio));
    let image = picture(first, &differs, &nodes, f64::from(ratio));
    Comparison { nodes, outside, in_text, max_delta, regions, image }
}

/// The connected runs of differing pixels (eight neighbours, one bucket
/// each), largest first.
fn regions(differs: &[u8], width: usize, height: usize, ratio: f64) -> Vec<Region> {
    let mut seen = vec![false; differs.len()];
    let mut found = Vec::new();
    let mut stack = Vec::new();
    for start in 0..differs.len() {
        if differs[start] == 0 || seen[start] {
            continue;
        }
        let bucket = differs[start];
        let (mut left, mut top, mut right, mut bottom, mut pixels) = (usize::MAX, usize::MAX, 0, 0, 0);
        seen[start] = true;
        stack.push(start);
        while let Some(at) = stack.pop() {
            let (x, y) = (at % width, at / width);
            pixels += 1;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let next = ny * width + nx;
                    if !seen[next] && differs[next] == bucket {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        found.push(Region {
            bounds: Rect { x: css(left, ratio), y: css(top, ratio), w: css(right + 1 - left, ratio), h: css(bottom + 1 - top, ratio) },
            pixels,
            in_text: bucket == 2,
        });
    }
    found.sort_by_key(|region| std::cmp::Reverse(region.pixels));
    found
}

/// The first capture faded to a pale gray, the pixels outside text that
/// differ in red, those under text in blue, and each disagreeing box
/// outlined where the first side put it (green) and the second (magenta).
fn picture(first: &Shot, differs: &[u8], nodes: &[NodeDiff], ratio: f64) -> Vec<u8> {
    let (width, height) = (first.width as usize, first.height as usize);
    let mut out = Vec::with_capacity(width * height * 4);
    for (at, pixel) in first.rgba.as_chunks::<4>().0.iter().enumerate() {
        let gray = (u32::from(pixel[0]) * 3 + u32::from(pixel[1]) * 6 + u32::from(pixel[2])) / 10;
        let pale = u8::try_from(200 + gray * 55 / 255).unwrap_or(u8::MAX);
        out.extend_from_slice(&match differs[at] {
            1 => [230, 20, 20, 255],
            2 => [20, 90, 230, 255],
            _ => [pale, pale, pale, 255],
        });
    }
    let mut outline = |rect: Rect, color: [u8; 4]| {
        if width == 0 || height == 0 {
            return;
        }
        let left = device((rect.x * ratio).round(), width - 1);
        let top = device((rect.y * ratio).round(), height - 1);
        let right = device(((rect.x + rect.w) * ratio).round() - 1.0, width - 1);
        let bottom = device(((rect.y + rect.h) * ratio).round() - 1.0, height - 1);
        let mut put = |x: usize, y: usize| {
            let at = (y * width + x) * 4;
            out[at..at + 4].copy_from_slice(&color);
        };
        for x in left..=right {
            put(x, top);
            put(x, bottom);
        }
        for y in top..=bottom {
            put(left, y);
            put(right, y);
        }
    };
    for node in nodes {
        if let Some(rect) = node.first {
            outline(rect, [20, 160, 60, 255]);
        }
        if let Some(rect) = node.second {
            outline(rect, [200, 30, 200, 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    fn shot(width: u32, height: u32, color: [u8; 4]) -> Shot {
        Shot {
            width,
            height,
            rgba: color.repeat((width * height) as usize),
            boxes: Vec::new(),
            texts: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// A box within a sixty-fourth agrees; one that moved, grew or exists
    /// on one side only does not, and says how.
    #[test]
    fn boxes_disagree_by_more_than_a_sixty_fourth() {
        let first = vec![("a".to_owned(), rect(0.0, 0.0, 10.0, 10.0)), ("b".to_owned(), rect(0.0, 10.0, 10.0, 10.0)), ("c".to_owned(), rect(0.0, 0.0, 1.0, 1.0))];
        let second = vec![("a".to_owned(), rect(0.01, 0.0, 10.0, 10.0)), ("b".to_owned(), rect(0.5, 10.0, 11.25, 10.0)), ("d".to_owned(), rect(0.0, 0.0, 1.0, 1.0))];
        let diffs = compare_nodes(&first, &second);
        assert_eq!(diffs.iter().map(|diff| diff.id.as_str()).collect::<Vec<_>>(), ["b", "c", "d"]);
        assert_eq!(diffs[0].describe(), "moved +0.50, +0.00 px; larger +1.25, +0.00 px");
        assert!((diffs[0].worst() - 1.25).abs() < 1e-9);
        assert_eq!(diffs[1].describe(), "missing on the second side");
        assert_eq!(diffs[2].describe(), "missing on the first side");
    }

    /// A differing pixel under a text box (or within its reach) counts in
    /// the text bucket; one elsewhere counts outside; each run is a
    /// region with its box in CSS pixels.
    #[test]
    fn pixels_fall_in_two_buckets_and_runs_become_regions() {
        let first = shot(20, 10, [255, 255, 255, 255]);
        let mut second = shot(20, 10, [255, 255, 255, 255]);
        second.texts.push(rect(1.0, 1.0, 2.0, 2.0));
        let mut paint = |x: usize, y: usize| {
            let at = (y * 20 + x) * 4;
            second.rgba[at] = 0;
        };
        paint(3, 3);
        paint(4, 4);
        paint(15, 2);
        paint(16, 2);
        paint(16, 3);
        let comparison = compare(&first, &second, 2);
        assert_eq!((comparison.outside, comparison.in_text, comparison.max_delta), (3, 2, 255));
        assert_eq!(comparison.regions.len(), 2);
        assert_eq!(comparison.regions[0], Region { bounds: rect(7.5, 1.0, 1.0, 1.0), pixels: 3, in_text: false });
        assert!(comparison.regions[1].in_text);
        assert_eq!(comparison.score(), [0, 3, 2]);
        assert_eq!(comparison.image.len(), 20 * 10 * 4);
    }
}
