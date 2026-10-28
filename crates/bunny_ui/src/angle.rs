//! Turns — how far a mark or a line of text is rotated.
//!
//! Two vocabularies, because the two things that turn are not alike.
//! A MARK (a glyph, a painted path) is geometry the house rasterizes,
//! so it turns by any [`Angle`]: a disclosure chevron a quarter round,
//! a loader through every degree of its cycle. A line of TEXT is a
//! raster the platform's engine shapes upright, and it turns only by a
//! [`QuarterTurn`]: those are the turns a raster survives byte for byte,
//! and they are the only ones a design asks of text — the vertical
//! label on a chart's axis, the spine of a tab.
//!
//! Both read the way CSS's `rotate()` and SwiftUI's `rotationEffect`
//! do: clockwise on the y-down screen.

use crate::layout::{Point, Rect, Size};
use crate::text_engine::LineMetrics;

/// A rotation, clockwise. Stored in radians; written in whichever unit
/// the design spoke.
///
/// ```ignore
/// icon(symbol::CHEVRON_RIGHT).rotation(Angle::degrees(90.0)) // open
/// ```
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Angle {
    radians: f64,
}

impl Angle {
    /// No turn.
    pub const ZERO: Angle = Angle { radians: 0.0 };

    /// `degrees` clockwise — CSS's `rotate(90deg)`.
    pub fn degrees(degrees: f64) -> Angle {
        Angle { radians: degrees.to_radians() }
    }

    /// `radians` clockwise — what a phase times `TAU` gives a spinner.
    pub const fn radians(radians: f64) -> Angle {
        Angle { radians }
    }

    /// The turn in radians.
    pub const fn as_radians(self) -> f64 {
        self.radians
    }

    /// The turn in degrees.
    pub fn as_degrees(self) -> f64 {
        self.radians.to_degrees()
    }

    /// Does it turn at all? A non-finite angle does not: it is a turn
    /// nobody can draw, and drawing the mark upright says so plainly
    /// instead of drawing nothing.
    pub fn is_zero(self) -> bool {
        self.radians == 0.0 || !self.radians.is_finite()
    }
}

/// A turn a line of text takes whole: a quarter, a half or three
/// quarters, clockwise.
///
/// The line is shaped and rasterized UPRIGHT, by the same engine and
/// the same caches as every other line, and the raster is turned
/// afterwards — a quarter turn moves every pixel to another pixel, so
/// nothing is resampled and the glyphs stay as crisp as they were.
/// The box it lays out in turns with it: a line turned a quarter is as
/// wide as it was tall.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum QuarterTurn {
    /// 90° — reads top to bottom, the tops of the glyphs facing right.
    Clockwise,
    /// 180° — upside down.
    Half,
    /// 270° — reads bottom to top, the tops facing left: the label on
    /// a chart's vertical axis.
    Counterclockwise,
}

impl QuarterTurn {
    const fn quarters(self) -> u8 {
        match self {
            QuarterTurn::Clockwise => 1,
            QuarterTurn::Half => 2,
            QuarterTurn::Counterclockwise => 3,
        }
    }

    /// Does the turn swap the two axes of the box?
    pub const fn swaps_axes(self) -> bool {
        !matches!(self, QuarterTurn::Half)
    }

    /// The same turn as an [`Angle`].
    pub fn angle(self) -> Angle {
        Angle::degrees(self.degrees())
    }

    /// 90, 180 or 270.
    pub const fn degrees(self) -> f64 {
        match self {
            QuarterTurn::Clockwise => 90.0,
            QuarterTurn::Half => 180.0,
            QuarterTurn::Counterclockwise => 270.0,
        }
    }

    /// This turn and then `next`: `None` when the two make a whole turn
    /// and the line stands upright again.
    pub const fn then(self, next: QuarterTurn) -> Option<QuarterTurn> {
        match (self.quarters() + next.quarters()) % 4 {
            1 => Some(QuarterTurn::Clockwise),
            2 => Some(QuarterTurn::Half),
            3 => Some(QuarterTurn::Counterclockwise),
            _ => None,
        }
    }

    /// The box a line turned this way covers, from where it starts on
    /// its baseline: as long as the line along the way it reads, and
    /// its ascent and descent across, on the side the glyphs' tops and
    /// tails turned to.
    pub fn line_box(self, pivot: Point, metrics: &LineMetrics) -> Rect {
        let LineMetrics { width, ascent, descent } = *metrics;
        let (x, y, w, h) = match self {
            QuarterTurn::Clockwise => (pivot.x - descent, pivot.y, ascent + descent, width),
            QuarterTurn::Half => (pivot.x - width, pivot.y - descent, width, ascent + descent),
            QuarterTurn::Counterclockwise => {
                (pivot.x - ascent, pivot.y - width, ascent + descent, width)
            }
        };
        Rect { origin: Point { x, y }, size: Size { width: w, height: h } }
    }

    /// The inverse: where a line starts on its baseline when its turned
    /// box's top-left corner is `origin` — the point
    /// [`DrawCommand::TurnedText`](crate::layout::DrawCommand::TurnedText)
    /// carries.
    pub fn pivot_in(self, origin: Point, metrics: &LineMetrics) -> Point {
        let LineMetrics { width, ascent, descent } = *metrics;
        match self {
            QuarterTurn::Clockwise => Point { x: origin.x + descent, y: origin.y },
            QuarterTurn::Half => Point { x: origin.x + width, y: origin.y + descent },
            QuarterTurn::Counterclockwise => Point { x: origin.x + ascent, y: origin.y + width },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrees_and_radians_are_one_turn() {
        let right = Angle::degrees(90.0);
        assert!((right.as_radians() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!((Angle::radians(std::f64::consts::PI).as_degrees() - 180.0).abs() < 1e-9);
        assert!(Angle::ZERO.is_zero());
        assert!(Angle::radians(f64::NAN).is_zero(), "a turn nobody can draw stays upright");
        assert!(!right.is_zero());
    }

    #[test]
    fn a_pivot_and_its_turned_box_answer_each_other() {
        let metrics = LineMetrics { width: 40.0, ascent: 10.0, descent: 3.0 };
        let origin = Point { x: 5.0, y: 7.0 };
        for turn in [QuarterTurn::Clockwise, QuarterTurn::Half, QuarterTurn::Counterclockwise] {
            let pivot = turn.pivot_in(origin, &metrics);
            let line = turn.line_box(pivot, &metrics);
            assert_eq!(line.origin, origin, "{turn:?}");
            let (long, short) = (40.0, 13.0);
            let expected = if turn.swaps_axes() { (short, long) } else { (long, short) };
            assert_eq!((line.size.width, line.size.height), expected, "{turn:?}");
        }
        // reading upward, the line starts at the bottom, its ascent in
        assert_eq!(
            QuarterTurn::Counterclockwise.pivot_in(origin, &metrics),
            Point { x: 15.0, y: 47.0 }
        );
    }

    #[test]
    fn turns_add_up_and_a_whole_one_stands_upright() {
        assert_eq!(QuarterTurn::Clockwise.then(QuarterTurn::Clockwise), Some(QuarterTurn::Half));
        assert_eq!(QuarterTurn::Half.then(QuarterTurn::Counterclockwise), Some(QuarterTurn::Clockwise));
        assert_eq!(QuarterTurn::Clockwise.then(QuarterTurn::Counterclockwise), None);
    }

    #[test]
    fn a_quarter_swaps_the_axes_and_a_half_does_not() {
        assert!(QuarterTurn::Clockwise.swaps_axes());
        assert!(QuarterTurn::Counterclockwise.swaps_axes());
        assert!(!QuarterTurn::Half.swaps_axes());
        assert_eq!(QuarterTurn::Counterclockwise.degrees(), 270.0);
    }
}
