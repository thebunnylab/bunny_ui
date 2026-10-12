//! Immutable line-specific levels leave the native logical shaping context intact.

use super as glyphs;
use super::{Prepared, TextError, bidi, preparation};
use std::ops::Range;

#[path = "text_line_geometry.rs"]
mod geometry;

impl Prepared {
    fn line_levels(&self, range: Range<usize>) -> Result<Vec<u8>, TextError> {
        glyphs::validate_range(self.utf16(), &range)?;
        let preceding = self.classes[..range.start]
            .iter()
            .rposition(|&class| class != bidi::Class::Retained)
            .map_or(0, |index| self.levels[index].resolved);
        let mut levels: Vec<_> = self.levels[range.clone()]
            .iter()
            .map(|level| level.resolved)
            .collect();
        bidi::reset_line(&self.classes[range], &mut levels, preceding);
        Ok(levels)
    }
}

/// Owned logical glyphs plus atomic clusters placed in visual order. Shaping
/// levels remain on Run; line-reset levels are used solely for L2 placement.
pub(super) struct VisualLine {
    source: Range<usize>,
    pub(super) runs: Vec<glyphs::Run>,
    pub(super) clusters: Vec<Cluster>,
    pub(super) width: f64,
}
#[derive(Clone, Debug)]
pub(super) struct Cluster {
    pub(super) run: usize,
    pub(super) source: Range<usize>,
    pub(super) glyphs: Range<usize>,
    pub(super) level: u8,
    pub(super) left: f64,
    pub(super) width: f64,
}
impl VisualLine {
    /// Coalesce only adjacent clusters that are contiguous in the original
    /// glyph direction. L1-reset whitespace may visually reverse that order.
    pub(super) fn spans(&self) -> Vec<Cluster> {
        let mut spans: Vec<Cluster> = Vec::new();
        for cluster in &self.clusters {
            if let Some(previous) = spans.last_mut()
                && previous.run == cluster.run
                && previous.level == cluster.level
            {
                let rtl = !self.runs[cluster.run].level.is_multiple_of(2);
                if rtl
                    && cluster.glyphs.end == previous.glyphs.start
                    && cluster.source.end == previous.source.start
                {
                    previous.source.start = cluster.source.start;
                    previous.glyphs.start = cluster.glyphs.start;
                    previous.width += cluster.width;
                    continue;
                }
                if !rtl
                    && cluster.glyphs.start == previous.glyphs.end
                    && cluster.source.start == previous.source.end
                {
                    previous.source.end = cluster.source.end;
                    previous.glyphs.end = cluster.glyphs.end;
                    previous.width += cluster.width;
                    continue;
                }
            }
            spans.push(cluster.clone());
        }
        spans
    }
    pub(super) fn new(
        prepared: &Prepared,
        range: Range<usize>,
        runs: Vec<glyphs::Run>,
    ) -> Result<Self, TextError> {
        let levels = prepared.line_levels(range.clone())?;
        let mut clusters = Vec::new();
        let mut position = range.start;
        for (index, run) in runs.iter().enumerate() {
            if run.source.start != position || run.source.end > range.end || run.source.is_empty() {
                return Err(preparation("line glyph source coverage"));
            }
            append_clusters(&mut clusters, prepared, &range, &levels, index, run)?;
            position = run.source.end;
        }
        if position != range.end {
            return Err(preparation("line glyph source coverage"));
        }
        bidi::reorder(&mut clusters, |cluster| cluster.level);
        let mut width = 0.0;
        for cluster in &mut clusters {
            cluster.left = width;
            width += cluster.width;
        }
        if !width.is_finite() || width < 0.0 {
            return Err(preparation("line visual width"));
        }
        Ok(Self {
            source: range,
            runs,
            clusters,
            width,
        })
    }
}

fn append_clusters(
    output: &mut Vec<Cluster>,
    prepared: &Prepared,
    line: &Range<usize>,
    levels: &[u8],
    index: usize,
    run: &glyphs::Run,
) -> Result<(), TextError> {
    let mut starts = run.clusters.clone();
    starts.sort_unstable();
    starts.dedup();
    if starts.first() != Some(&0) || run.clusters.len() != run.source.len() {
        return Err(preparation("line glyph cluster coverage"));
    }
    let mut visited = vec![false; starts.len()];
    let mut position = 0;
    while position < run.clusters.len() {
        let glyph = run.clusters[position];
        let end = (position + 1..run.clusters.len())
            .find(|&unit| run.clusters[unit] != glyph)
            .unwrap_or(run.clusters.len());
        let ordinal = starts
            .binary_search(&glyph)
            .map_err(|_| preparation("line glyph cluster index"))?;
        if visited[ordinal] {
            return Err(preparation("non-contiguous logical glyph cluster"));
        }
        visited[ordinal] = true;
        let glyph_end = starts
            .get(ordinal + 1)
            .map_or(run.glyphs.len(), |&start| usize::from(start));
        let glyphs = usize::from(glyph)..glyph_end;
        let advances = run
            .advances
            .get(glyphs.clone())
            .ok_or_else(|| preparation("line glyph cluster bounds"))?;
        let width: f64 = if run.visibility == glyphs::Visibility::Control {
            0.0
        } else {
            advances.iter().map(|&value| f64::from(value)).sum()
        };
        let source = run.source.start + position..run.source.start + end;
        // A retained joiner may share its glyph with a visible letter. Reorder
        // their indivisible native cluster by that letter, never cut its shape.
        let anchor = source
            .clone()
            .find(|&unit| prepared.classes[unit] == bidi::Class::Other)
            .unwrap_or(source.start);
        output.push(Cluster {
            run: index,
            source,
            glyphs,
            level: levels[anchor - line.start],
            left: 0.0,
            width,
        });
        position = end;
    }
    Ok(())
}

#[cfg(test)]
#[path = "text_line_tests.rs"]
mod tests;
