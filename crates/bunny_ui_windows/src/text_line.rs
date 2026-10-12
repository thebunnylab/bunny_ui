//! Immutable line-specific levels leave the native logical shaping context intact.

use super::{Prepared, TextError, bidi, glyphs};
use std::ops::Range;

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

#[cfg(test)]
#[path = "text_line_tests.rs"]
mod tests;
