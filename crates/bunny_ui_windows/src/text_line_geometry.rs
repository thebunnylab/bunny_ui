//! Scalar editing cells refer to the same indivisible native glyph clusters.

use super::{TextError, VisualLine, preparation};
use bunny_ui::text_geometry::{LineGeometry, TextCell};

impl VisualLine {
    pub(in crate::text::analysis) fn geometry(
        &self,
        text: &str,
    ) -> Result<LineGeometry, TextError> {
        let mut logical: Vec<_> = self.clusters.iter().collect();
        logical.sort_by_key(|cluster| cluster.source.start);
        let mut cells: Vec<TextCell> = Vec::new();
        let mut unit = self.source.start;
        let mut index = 0;
        for (byte, character) in text.char_indices() {
            while logical
                .get(index)
                .is_some_and(|cluster| cluster.source.end <= unit)
            {
                index += 1;
            }
            let cluster = logical
                .get(index)
                .filter(|cluster| cluster.source.contains(&unit))
                .ok_or_else(|| preparation("scalar glyph cluster coverage"))?;
            let end = unit + character.len_utf16();
            if end > cluster.source.end {
                return Err(preparation("scalar glyph cluster boundary"));
            }
            let (leading, trailing) = if self.runs[cluster.run].visibility
                == super::glyphs::Visibility::Control
                && let Some(previous) = cells.last()
            {
                (previous.trailing(), previous.trailing())
            } else if cluster.level.is_multiple_of(2) {
                (cluster.left, cluster.left + cluster.width)
            } else {
                (cluster.left + cluster.width, cluster.left)
            };
            cells.push(TextCell::new(
                byte..byte + character.len_utf8(),
                leading,
                trailing,
            )?);
            unit = end;
        }
        if unit != self.source.end {
            return Err(preparation("scalar source length"));
        }
        Ok(LineGeometry::new(text, cells)?)
    }
}

#[cfg(test)]
#[path = "text_line_geometry_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "text_paragraph_selection_tests.rs"]
mod selection_tests;
