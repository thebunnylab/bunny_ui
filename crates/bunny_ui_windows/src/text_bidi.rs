//! Per-line UAX #9 L1/L2 after native whole-paragraph resolution and shaping.
//!
//! ## Wiring
//! Original Unicode classes govern resets, including retained controls (section
//! 5.2). These levels order clusters; they must never split logical shaping
//! input at a joiner or replace the original native shaping direction.

#[path = "text_bidi_data.rs"]
mod data;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Other,
    Separator,
    Reset,
    Retained,
}
impl Class {
    pub(super) fn of(character: char) -> Self {
        let scalar = u32::from(character);
        let index = data::CLASSES.partition_point(|&(_, end, _)| end < scalar);
        data::CLASSES
            .get(index)
            .filter(|&&(start, _, _)| start <= scalar)
            .map_or(Self::Other, |&(_, _, class)| class)
    }
}

/// The caller supplies equal-length original classes and copied resolved levels.
/// `preceding` is the nearest preceding non-retained level in the paragraph.
/// Empty/trailing reset sequences are valid. Paragraph base direction is LTR,
/// matching the immutable native source and the engine's TextFormat.
pub(super) fn reset_line(classes: &[Class], levels: &mut [u8], preceding: u8) {
    let mut previous = preceding;
    for (&class, level) in classes.iter().zip(levels.iter_mut()) {
        if class == Class::Retained {
            *level = previous;
        }
        previous = *level;
    }
    let mut sequence = 0;
    for (index, &class) in classes.iter().enumerate() {
        match class {
            Class::Other => sequence = index + 1,
            Class::Separator => {
                levels[sequence..=index].fill(0);
                sequence = index + 1;
            }
            Class::Reset | Class::Retained => {}
        }
    }
    levels[sequence..].fill(0);
}

/// Reorder atomic glyph clusters, never individual UTF-16 units or glyphs.
pub(super) fn reorder<T>(items: &mut [T], level: impl Fn(&T) -> u8) {
    let Some(lowest_odd) = items
        .iter()
        .map(&level)
        .filter(|value| value % 2 == 1)
        .min()
    else {
        return;
    };
    let highest = items.iter().map(&level).max().unwrap_or(lowest_odd);
    for threshold in (lowest_odd..=highest).rev() {
        let mut start = 0;
        while start < items.len() {
            if level(&items[start]) < threshold {
                start += 1;
                continue;
            }
            let end = (start..items.len())
                .find(|&index| level(&items[index]) < threshold)
                .unwrap_or(items.len());
            items[start..end].reverse();
            start = end;
        }
    }
}

#[cfg(test)]
#[path = "text_bidi_tests.rs"]
mod tests;
