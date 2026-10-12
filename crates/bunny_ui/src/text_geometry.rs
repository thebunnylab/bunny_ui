//! Editable coordinates must come from the complete line that paints the glyphs.
//!
//! ## Wiring
//! Native `TextEngine` adapters supply directed scalar cells. The validated
//! line owns caret affinity, grapheme hit stops and disjoint selection spans;
//! layout adds only the field origin and scrolling. No platform object crosses
//! this boundary, and logical UTF-8 remains unchanged.

use crate::layout::Px;
use std::ops::Range;

/// Which logical side supplies a boundary's visual insertion position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaretAffinity {
    /// Before the following logical scalar, or after the last at end of text.
    #[default]
    Downstream,
    /// After the preceding logical scalar, or before the first at start.
    Upstream,
}

/// A logical insertion point and the visual side selected by the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaretHit {
    /// UTF-8 byte offset on an extended grapheme boundary.
    pub byte: usize,
    /// The side to keep when the same byte has two visual positions.
    pub affinity: CaretAffinity,
}

/// A malformed native geometry result, rejected before layout can consume it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryError {
    /// A scalar cell is empty or reversed.
    InvalidRange,
    /// A coordinate is NaN or infinite.
    NonFiniteCoordinate,
    /// Cells do not cover each source scalar exactly once in logical order.
    SourceCoverage,
}

impl std::fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRange => "text geometry has an empty or reversed scalar range",
            Self::NonFiniteCoordinate => "text geometry has a non-finite coordinate",
            Self::SourceCoverage => "text geometry does not cover the logical source scalars",
        })
    }
}
impl std::error::Error for GeometryError {}

/// Directed advance edges of one logical scalar in a complete shaped line.
/// Coordinates are relative to the visual left origin, not a logical prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct TextCell {
    bytes: Range<usize>,
    leading: Px,
    trailing: Px,
}

impl TextCell {
    /// Validate one scalar cell; [`LineGeometry::new`] checks its source range.
    ///
    /// # Errors
    /// Rejects an empty/reversed range or a non-finite edge.
    pub const fn new(
        bytes: Range<usize>,
        leading: Px,
        trailing: Px,
    ) -> Result<Self, GeometryError> {
        if bytes.start >= bytes.end {
            return Err(GeometryError::InvalidRange);
        }
        if !leading.is_finite() || !trailing.is_finite() {
            return Err(GeometryError::NonFiniteCoordinate);
        }
        Ok(Self {
            bytes,
            leading,
            trailing,
        })
    }

    /// The original logical UTF-8 scalar range.
    #[must_use]
    pub fn bytes(&self) -> Range<usize> {
        self.bytes.clone()
    }
    /// The edge before this scalar in logical order.
    #[must_use]
    pub const fn leading(&self) -> Px {
        self.leading
    }
    /// The edge after this scalar in logical order.
    #[must_use]
    pub const fn trailing(&self) -> Px {
        self.trailing
    }
}

#[derive(Clone, Copy, Debug)]
struct Stop {
    hit: CaretHit,
    x: Px,
}

/// Validated cells of one complete shaped line, with cached grapheme hit stops.
#[derive(Clone, Debug)]
pub struct LineGeometry {
    cells: Vec<TextCell>,
    stops: Vec<Stop>,
    bytes: usize,
}

impl LineGeometry {
    /// Bind native cells to their unchanged logical UTF-8 source.
    ///
    /// # Errors
    /// Rejects gaps, duplicates, out-of-order cells and non-scalar ranges.
    pub fn new(text: &str, cells: Vec<TextCell>) -> Result<Self, GeometryError> {
        let mut actual = cells.iter();
        for (byte, character) in text.char_indices() {
            let expected = byte..byte + character.len_utf8();
            if actual.next().is_none_or(|cell| cell.bytes != expected) {
                return Err(GeometryError::SourceCoverage);
            }
        }
        if actual.next().is_some() {
            return Err(GeometryError::SourceCoverage);
        }
        let boundaries = crate::grapheme::boundaries(text);
        let mut stops = Vec::new();
        for cell in &cells {
            if boundaries.binary_search(&cell.bytes.start).is_ok() {
                stops.push(Stop {
                    hit: CaretHit {
                        byte: cell.bytes.start,
                        affinity: CaretAffinity::Downstream,
                    },
                    x: cell.leading,
                });
            }
            if boundaries.binary_search(&cell.bytes.end).is_ok() {
                stops.push(Stop {
                    hit: CaretHit {
                        byte: cell.bytes.end,
                        affinity: CaretAffinity::Upstream,
                    },
                    x: cell.trailing,
                });
            }
        }
        stops.sort_by(|a, b| {
            a.x.total_cmp(&b.x)
                .then(a.hit.byte.cmp(&b.hit.byte))
                .then(a.hit.affinity.cmp(&b.hit.affinity))
        });
        if stops.is_empty() {
            stops.push(Stop {
                hit: CaretHit::default(),
                x: 0.0,
            });
        }
        Ok(Self {
            cells,
            stops,
            bytes: text.len(),
        })
    }

    /// Native scalar cells in their original logical order.
    #[must_use]
    pub fn cells(&self) -> &[TextCell] {
        &self.cells
    }

    /// Position of a byte boundary, preserving the selected visual affinity.
    /// Out-of-range indices clamp; an index inside a scalar clamps to its start.
    #[must_use]
    pub fn caret_x(&self, byte: usize, affinity: CaretAffinity) -> Px {
        let index = self.cells.partition_point(|cell| cell.bytes.end <= byte);
        match affinity {
            CaretAffinity::Downstream => self.cells.get(index).map_or_else(
                || self.cells.last().map_or(0.0, |cell| cell.trailing),
                |cell| cell.leading,
            ),
            CaretAffinity::Upstream => index
                .checked_sub(1)
                .and_then(|i| self.cells.get(i))
                .map_or_else(
                    || self.cells.first().map_or(0.0, |cell| cell.leading),
                    |cell| cell.trailing,
                ),
        }
    }

    /// Nearest legal grapheme hit. Affinity makes its visual position stable
    /// when logical neighbors live on different sides of a directional run.
    #[must_use]
    pub fn hit_test(&self, x: Px) -> CaretHit {
        if x.is_nan() {
            return CaretHit::default();
        }
        let right = self.stops.partition_point(|stop| stop.x < x);
        let Some(after) = self.stops.get(right) else {
            return self.stops[self.stops.len() - 1].hit;
        };
        let Some(before) = right.checked_sub(1).and_then(|index| self.stops.get(index)) else {
            return after.hit;
        };
        if x - before.x <= after.x - x {
            before.hit
        } else {
            after.hit
        }
    }

    /// Visual advance spans for a logical selection, sorted and merged only
    /// where they touch. Mixed-direction holes remain separate rectangles.
    #[must_use]
    pub fn selection_spans(&self, bytes: Range<usize>) -> Vec<Range<Px>> {
        let end = bytes.end.min(self.bytes);
        let start = bytes.start.min(end);
        if start == end {
            return Vec::new();
        }
        let first = self.cells.partition_point(|cell| cell.bytes.end <= start);
        let mut spans: Vec<_> = self.cells[first..]
            .iter()
            .take_while(|cell| cell.bytes.start < end)
            .map(|cell| cell.leading.min(cell.trailing)..cell.leading.max(cell.trailing))
            .collect();
        spans.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut merged: Vec<Range<Px>> = Vec::new();
        for span in spans {
            match merged.last_mut() {
                Some(previous) if span.start <= previous.end + 1e-6 => {
                    previous.end = previous.end.max(span.end);
                }
                _ => merged.push(span),
            }
        }
        merged
    }
}

#[cfg(test)]
#[path = "text_geometry_tests.rs"]
mod tests;
