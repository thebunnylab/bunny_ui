//! Continue native shaping only at finalized, cluster-safe dependency boundaries.
//!
//! A complex span keeps its complete native call whenever supported. Oversized
//! input may use successful bounded probes, but substitution flags alone are
//! insufficient: placement can invalidate a cut because of kerning or marks.

use super::{
    CHUNK_UNITS, Input, MAX_GLYPHS, Output, Run, Shaper, TextError, Visibility, preparation,
    validate_range,
};
use std::ops::Range;

enum Cuts {
    Simple,
    Native,
}

impl Shaper {
    pub(super) fn shape_span(
        &self,
        input: &Input<'_>,
        tracking: f32,
        source: Range<usize>,
        visibility: Visibility,
        runs: &mut Vec<Run>,
    ) -> Result<(), TextError> {
        let size = self.shaping_chunk_size(input)?;
        let cuts = if size == input.text.len() {
            match self.produce(input, tracking, capacity(input.text.len())) {
                Ok(output) => return emit(input, source, visibility, output, runs),
                Err(error)
                    if input.text.len() > CHUNK_UNITS
                        && matches!(
                            error,
                            TextError::Preparation {
                                operation: "GetGlyphs",
                                ..
                            }
                        ) =>
                {
                    Cuts::Native
                }
                Err(error) => return Err(error),
            }
        } else {
            Cuts::Simple
        };
        let mut position = 0;
        while position < input.text.len() {
            let mut end = (position + CHUNK_UNITS).min(input.text.len());
            if validate_range(input.text, &(position..end)).is_err() {
                end -= 1;
            }
            validate_range(input.text, &(position..end))?;
            let probe = Input {
                text: &input.text[position..end],
                ..*input
            };
            // `produce` includes placement's in/out property updates and the
            // native tracking adjustment. No failed output reaches cut selection.
            let mut output = self.produce(&probe, tracking, capacity(probe.text.len()))?;
            if matches!(cuts, Cuts::Native) && end < input.text.len() {
                let cut = (1..probe.text.len())
                    .rev()
                    .find(|&cut| {
                        output.text_properties[cut - 1] & 4 != 0
                            && output.clusters[cut - 1] != output.clusters[cut]
                            && validate_range(input.text, &(position..position + cut)).is_ok()
                    })
                    .ok_or_else(|| preparation("glyph continuation boundary"))?;
                end = position + cut;
                let retained = Input {
                    text: &input.text[position..end],
                    ..*input
                };
                output = self.produce(&retained, tracking, capacity(retained.text.len()))?;
            }
            emit(
                input,
                source.start + position..source.start + end,
                visibility,
                output,
                runs,
            )?;
            position = end;
        }
        Ok(())
    }
}

fn capacity(length: usize) -> usize {
    (length.saturating_mul(3) / 2 + 16).min(MAX_GLYPHS)
}

fn emit(
    input: &Input<'_>,
    source: Range<usize>,
    visibility: Visibility,
    output: Output,
    runs: &mut Vec<Run>,
) -> Result<(), TextError> {
    runs.try_reserve(1)
        .map_err(|_| preparation("glyph run allocation"))?;
    runs.push(Run {
        source,
        face: input.face.clone(),
        em_size: input.em_size,
        level: input.level,
        visibility,
        clusters: output.clusters,
        glyphs: output.glyphs,
        advances: output.advances,
        offsets: output.offsets,
    });
    Ok(())
}
