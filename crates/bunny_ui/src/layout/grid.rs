//! Sparse cell allocation and equal-track sizing, shared by measure and place.
use super::*;

/// A child's occupied rectangle of grid tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellArea {
    /// Zero-based leading column.
    pub column: usize,
    /// Zero-based top row.
    pub row: usize,
    /// Number of columns occupied, clamped to the grid width.
    pub columns: usize,
    /// Number of rows occupied.
    pub rows: usize,
}

/// Track configuration, independent of children and the frame.
pub(super) struct Tracks {
    pub columns: usize,
    pub gap: Px,
    pub row_gap: Px,
    pub align: CrossAlign,
}

impl Tracks {
    fn width(&self, total: Px) -> Px {
        ((total - self.gap * self.columns.saturating_sub(1) as Px) / self.columns as Px).max(0.0)
    }

    fn across(&self, width: Px, span: usize) -> Px {
        width * span as Px + self.gap * span.saturating_sub(1) as Px
    }

    pub fn measure(
        &self,
        children: &[LayoutNode],
        proposal: Proposal,
        env: &LayoutEnv<'_>,
    ) -> (Size, Fit) {
        let areas = auto_place(self.columns, children.iter().map(LayoutNode::span_of));
        let column = proposal.width.map_or_else(
            || {
                children
                    .iter()
                    .zip(&areas)
                    .map(|(child, area)| {
                        let (size, _) = child.measure(Proposal::unspecified(), env);
                        ((size.width - self.gap * area.columns.saturating_sub(1) as Px)
                            / area.columns as Px)
                            .max(0.0)
                    })
                    .fold(0.0, Px::max)
            },
            |width| self.width(width),
        );
        let cells: Vec<_> = children
            .iter()
            .zip(&areas)
            .map(|(child, area)| {
                child.measure(
                    Proposal {
                        width: Some(self.across(column, area.columns)),
                        height: None,
                    },
                    env,
                )
            })
            .collect();
        let count = areas
            .iter()
            .map(|area| area.row + area.rows)
            .max()
            .unwrap_or(0);
        let mut rows = vec![0.0_f64; count];
        // Single-row children establish the ordinary tracks first. A spanning
        // cell gives only its remaining deficit to its final row.
        for (area, (size, _)) in areas.iter().zip(&cells).filter(|(area, _)| area.rows == 1) {
            rows[area.row] = rows[area.row].max(size.height);
        }
        for (area, (size, _)) in areas.iter().zip(&cells).filter(|(area, _)| area.rows > 1) {
            let end = area.row + area.rows;
            let height = rows[area.row..end].iter().sum::<Px>()
                + self.row_gap * area.rows.saturating_sub(1) as Px;
            rows[end - 1] += (size.height - height).max(0.0);
        }
        let size = Size {
            width: if children.is_empty() {
                proposal.width.unwrap_or(0.0)
            } else {
                self.across(column, self.columns)
            },
            height: rows.iter().sum::<Px>() + self.row_gap * count.saturating_sub(1) as Px,
        };
        (size, Fit::Grid { areas, rows, cells })
    }

    pub fn place(
        &self,
        children: &[LayoutNode],
        frame: Rect,
        fit: &Fit,
        env: &LayoutEnv<'_>,
        out: &mut Placement,
    ) {
        let Fit::Grid { areas, rows, cells } = fit else {
            return;
        };
        let column = self.width(frame.size.width);
        let mut starts = Vec::with_capacity(rows.len() + 1);
        starts.push(0.0);
        for row in rows {
            starts.push(starts.last().copied().unwrap_or(0.0) + row + self.row_gap);
        }
        for ((child, area), (size, fit)) in children.iter().zip(areas).zip(cells) {
            let width = self.across(column, area.columns);
            let top = starts[area.row];
            let height = starts[area.row + area.rows] - top - self.row_gap;
            let along = area.column as Px * (column + self.gap)
                + align_offset(width, size.width, self.align);
            let origin = Point {
                x: leading_x(frame, along, size.width, env.rtl()),
                y: frame.origin.y + top + align_offset(height, size.height, CrossAlign::Center),
            };
            child.place(
                Rect {
                    origin,
                    size: *size,
                },
                fit,
                env,
                out,
            );
        }
    }
}

/// CSS sparse auto-placement: the cursor never backfills a preceding hole.
fn auto_place(columns: usize, spans: impl IntoIterator<Item = (usize, usize)>) -> Vec<CellArea> {
    let mut occupied_until = vec![0usize; columns];
    let (mut row, mut column) = (0usize, 0usize);
    let mut result = Vec::new();
    for (width, height) in spans {
        let width = width.max(1).min(columns);
        let height = height.max(1);
        loop {
            if column + width > columns {
                row += 1;
                column = 0;
                // A tall cell covering every track can skip many rows at once.
                if let Some(next) = occupied_until.iter().copied().min() {
                    row = row.max(next);
                }
            }
            if let Some(blocked) = occupied_until[column..column + width]
                .iter()
                .position(|end| *end > row)
            {
                column += blocked + 1;
                continue;
            }
            occupied_until[column..column + width].fill(row + height);
            result.push(CellArea {
                column,
                row,
                columns: width,
                rows: height,
            });
            column += width;
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_placement_leaves_a_hole_before_a_wide_child() {
        let areas = auto_place(3, [(2, 1), (2, 1), (1, 1)]);
        assert_eq!((areas[1].column, areas[1].row), (0, 1));
        assert_eq!((areas[2].column, areas[2].row), (2, 1));
    }

    #[test]
    fn a_span_wider_than_the_grid_is_clamped() {
        let areas = auto_place(2, [(9, 1), (1, 1)]);
        assert_eq!(areas[0].columns, 2);
        assert_eq!(areas[1].row, 1);
    }
}
