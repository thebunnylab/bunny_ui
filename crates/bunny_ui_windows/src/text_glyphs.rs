//! Native glyph production retains whole-paragraph script and bidi decisions.
//!
//! ## Wiring
//! Logical runs use the original analysis source for system font fallback.
//! Native glyph indices, cluster maps, advances and offsets stay together with
//! their owning face. The line module applies visual L1/L2 ordering without
//! changing those shaping buffers, then supplies runtime geometry and raster.
//! Native u16 cluster indices bound each shaping span; oversized spans currently
//! return an explicit preparation error rather than an arbitrary split.

use super::{
    Analyzer, Com, Factories, Guid, Hresult, Prepared, Script, Source, TextError, bidi, com_ok,
    preparation,
};
use crate::text::{FontSlot, FontSpec, IDWriteFont, IDWriteFontCollection, style_of, weight_of};
#[cfg(test)]
use std::ffi::c_void;
use std::ops::Range;

#[path = "text_glyphs_ffi.rs"]
mod ffi;

#[path = "text_glyph_raster.rs"]
mod raster;

#[path = "text_line.rs"]
mod line;
pub(super) use line::VisualLine;

const MAX_GLYPHS: usize = u16::MAX as usize + 1;
const INSUFFICIENT_BUFFER: Hresult = -2_147_024_774; // HRESULT_FROM_WIN32(122)

pub(super) struct BaseFont {
    collection: Com<IDWriteFontCollection>,
    family: Vec<u16>,
    font: Com<IDWriteFont>,
}
impl BaseFont {
    pub(super) fn new(slot: &FontSlot, spec: &FontSpec) -> Result<Self, TextError> {
        // Read the resolved collection/name, including custom-font precedence
        // and unknown-family policy, from the existing engine's font slot.
        // SAFETY: IDWriteTextFormat has the exact inherited getter slots.
        unsafe {
            let format = slot.format.as_ptr().cast::<ffi::Format>();
            let table = &*(*format).vtbl;
            let mut collection = std::ptr::null_mut();
            let status = (table.collection)(format, &raw mut collection);
            let collection = own(status, collection, "GetFontCollection")?;
            let length = (table.family_length)(format)
                .checked_add(1)
                .ok_or_else(|| preparation("font family length"))?;
            let mut family = buffer(length as usize, "font family allocation")?;
            if !com_ok((table.family)(format, family.as_mut_ptr(), length)) {
                return Err(preparation("GetFontFamilyName"));
            }
            if family.last() != Some(&0) {
                return Err(preparation("font family terminator"));
            }
            let mut index = 0;
            let mut exists = 0;
            let collection_ptr = collection.as_ptr();
            let table = &*(*collection_ptr).vtbl;
            let status = (table.find_family_name)(
                collection_ptr,
                family.as_ptr(),
                &raw mut index,
                &raw mut exists,
            );
            if !com_ok(status) || exists == 0 {
                return Err(preparation("FindFamilyName"));
            }
            let mut family_object = std::ptr::null_mut();
            let status = (table.get_font_family)(collection_ptr, index, &raw mut family_object);
            let family_object = own(status, family_object, "GetFontFamily")?;
            let mut font = std::ptr::null_mut();
            let pointer = family_object.as_ptr();
            let status = ((*(*pointer).vtbl).get_first_matching_font)(
                pointer,
                weight_of(spec.weight),
                5,
                style_of(spec.slant),
                &raw mut font,
            );
            let font = own(status, font, "GetFirstMatchingFont")?;
            Ok(Self {
                collection,
                family,
                font,
            })
        }
    }
}

pub(super) struct Shaper {
    analyzer: Com<ffi::Analyzer>,
    fallback: Com<ffi::Fallback>,
}
impl Shaper {
    pub(super) fn new(factories: &Factories) -> Result<Self, TextError> {
        let analyzer = Analyzer::new(factories)?;
        let analyzer = ffi::query(&analyzer, &ffi::ANALYZER1, "IDWriteTextAnalyzer1")?;
        let factory: Com<ffi::Factory> =
            ffi::query(&factories.dwrite, &ffi::FACTORY2, "IDWriteFactory2")?;
        // SAFETY: the queried factory owns IDWriteFactory2, including slot 26.
        let fallback = unsafe {
            let mut pointer = std::ptr::null_mut();
            let status = ((*(*factory.as_ptr()).vtbl).fallback)(factory.as_ptr(), &raw mut pointer);
            own(status, pointer, "GetSystemFontFallback")?
        };
        Ok(Self { analyzer, fallback })
    }

    pub(super) fn shape(
        &self,
        prepared: &Prepared,
        range: Range<usize>,
        base: &BaseFont,
        font: &FontSpec,
    ) -> Result<Vec<Run>, TextError> {
        let size = native_float(font.size)?;
        let tracking = native_float(font.tracking)?;
        if !size.is_finite() || size <= 0.0 || !tracking.is_finite() {
            return Err(TextError::InvalidMetrics {
                engine: super::ENGINE,
            });
        }
        let text = prepared.utf16();
        validate_range(text, &range)?;
        let mut runs = Vec::new();
        let mut position = range.start;
        while position < range.end {
            let script = prepared.scripts[position];
            let level = prepared.levels[position].resolved;
            let end = (position + 1..range.end)
                .find(|&index| {
                    prepared.scripts[index] != script || prepared.levels[index].resolved != level
                })
                .unwrap_or(range.end);
            while position < end {
                let (face, scale, mapped) = self.map(prepared, position..end, base, font)?;
                let source = position..position + mapped;
                validate_range(text, &source)?;
                let em_size = size * scale;
                if !em_size.is_finite() || em_size <= 0.0 {
                    return Err(TextError::InvalidMetrics {
                        engine: super::ENGINE,
                    });
                }
                // SAFETY: prepared owns the immutable source for this whole call.
                let locale = unsafe { &(*prepared.source.as_ptr()).locale };
                let input = Input {
                    text: &text[source.clone()],
                    script,
                    level,
                    face: &face,
                    em_size,
                    locale,
                };
                let capacity = input.text.len().saturating_mul(3) / 2 + 16;
                let output = self.produce(&input, tracking, capacity.min(MAX_GLYPHS))?;
                runs.try_reserve(1)
                    .map_err(|_| preparation("glyph run allocation"))?;
                runs.push(Run {
                    source,
                    face,
                    em_size,
                    level,
                    visibility: if script.shapes & 1 == 0 {
                        Visibility::Ink
                    } else {
                        Visibility::Control
                    },
                    clusters: output.clusters,
                    glyphs: output.glyphs,
                    advances: output.advances,
                    offsets: output.offsets,
                });
                position += mapped;
            }
        }
        Ok(runs)
    }

    fn map(
        &self,
        prepared: &Prepared,
        range: Range<usize>,
        base: &BaseFont,
        spec: &FontSpec,
    ) -> Result<(Com<ffi::Face>, f32, usize), TextError> {
        let position =
            u32::try_from(range.start).map_err(|_| preparation("font fallback position"))?;
        let length = u32::try_from(range.len()).map_err(|_| preparation("font fallback length"))?;
        let mut mapped = 0;
        let mut font = std::ptr::null_mut();
        let mut scale = 0.0;
        // SAFETY: native fallback sees the original complete source, a checked
        // requested range and live collection/name. Every output is initialized.
        let status = unsafe {
            ((*(*self.fallback.as_ptr()).vtbl).map)(
                self.fallback.as_ptr(),
                prepared.source.as_ptr(),
                position,
                length,
                base.collection.as_ptr(),
                base.family.as_ptr(),
                weight_of(spec.weight),
                style_of(spec.slant),
                5,
                &raw mut mapped,
                &raw mut font,
                &raw mut scale,
            )
        };
        let mapped_font = Com::from_raw(font);
        if !com_ok(status) {
            return Err(preparation("MapCharacters"));
        }
        if mapped == 0 || mapped > length {
            return Err(preparation("font fallback range"));
        }
        // NULL is the documented no-known-font result, not a native failure.
        // Retain missing-glyph rendering in the selected primary font.
        let (font, scale) = mapped_font
            .as_ref()
            .map_or((&base.font, 1.0), |font| (font, scale));
        if !scale.is_finite() || scale <= 0.0 {
            return Err(TextError::InvalidMetrics {
                engine: super::ENGINE,
            });
        }
        let face = create_face(font)?;
        Ok((face, scale, mapped as usize))
    }

    fn glyph_indices(&self, input: &Input<'_>, capacity: usize) -> Result<Output, TextError> {
        let length =
            u32::try_from(input.text.len()).map_err(|_| preparation("glyph text length"))?;
        let mut output = Output::new(input.text.len())?;
        let mut capacity = capacity.clamp(1, MAX_GLYPHS);
        let mut actual = 0;
        // SAFETY: this owner is a queried IDWriteTextAnalyzer1 with the exact ABI.
        let table = unsafe { &*(*self.analyzer.as_ptr()).vtbl };
        loop {
            output.glyphs = buffer(capacity, "glyph index allocation")?;
            output.glyph_properties = buffer(capacity, "glyph property allocation")?;
            // SAFETY: input is one script/level/font span; every output buffer
            // has its documented text/glyph capacity. No synthetic controls.
            let status = unsafe {
                (table.glyphs)(
                    self.analyzer.as_ptr(),
                    input.text.as_ptr(),
                    length,
                    input.face.as_ptr(),
                    0,
                    i32::from(!input.level.is_multiple_of(2)),
                    &raw const input.script,
                    input.locale.as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    u32::try_from(capacity).map_err(|_| preparation("glyph capacity"))?,
                    output.clusters.as_mut_ptr(),
                    output.text_properties.as_mut_ptr(),
                    output.glyphs.as_mut_ptr(),
                    output.glyph_properties.as_mut_ptr(),
                    &raw mut actual,
                )
            };
            if status == INSUFFICIENT_BUFFER && capacity < MAX_GLYPHS {
                // actualGlyphCount is undefined on this failure; grow from our
                // known capacity, never from an untrusted partially written out.
                capacity = (capacity * 2).min(MAX_GLYPHS);
                continue;
            }
            if !com_ok(status) {
                return Err(preparation("GetGlyphs"));
            }
            break;
        }
        let count = actual as usize;
        if count == 0
            || count > capacity
            || output
                .clusters
                .iter()
                .any(|&cluster| usize::from(cluster) >= count)
        {
            return Err(preparation("glyph output coverage"));
        }
        output.glyphs.truncate(count);
        output.glyph_properties.truncate(count);
        Ok(output)
    }

    fn produce(
        &self,
        input: &Input<'_>,
        tracking: f32,
        capacity: usize,
    ) -> Result<Output, TextError> {
        let mut output = self.glyph_indices(input, capacity)?;
        let count = output.glyphs.len();
        let actual = u32::try_from(count).map_err(|_| preparation("glyph count"))?;
        let length =
            u32::try_from(input.text.len()).map_err(|_| preparation("glyph text length"))?;
        // SAFETY: this owner is a queried IDWriteTextAnalyzer1 with the exact ABI.
        let table = unsafe { &*(*self.analyzer.as_ptr()).vtbl };
        output.advances = buffer(count, "glyph advance allocation")?;
        output.offsets = buffer(count, "glyph offset allocation")?;
        // SAFETY: the cluster/glyph arrays came from the successful call above,
        // are validated, and retain their input/font/script/locale identity.
        let status = unsafe {
            (table.placements)(
                self.analyzer.as_ptr(),
                input.text.as_ptr(),
                output.clusters.as_ptr(),
                output.text_properties.as_mut_ptr(),
                length,
                output.glyphs.as_ptr(),
                output.glyph_properties.as_ptr(),
                actual,
                input.face.as_ptr(),
                input.em_size,
                0,
                i32::from(!input.level.is_multiple_of(2)),
                &raw const input.script,
                input.locale.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                output.advances.as_mut_ptr(),
                output.offsets.as_mut_ptr(),
            )
        };
        if !com_ok(status) {
            return Err(preparation("GetGlyphPlacements"));
        }
        if tracking != 0.0 {
            // Match TextLayout's placement of spacing in the glyph buffer:
            // right-to-left runs need the offset on the opposite edge. Widths
            // alone cannot detect an incorrectly shifted but equally wide run.
            let (leading, trailing) = if input.level.is_multiple_of(2) {
                (0.0, tracking)
            } else {
                (tracking, 0.0)
            };
            // SAFETY: the SDK permits in-place advance/offset adjustment. The
            // native operation preserves diacritic and ligature cluster shape.
            let status = unsafe {
                (table.spacing)(
                    self.analyzer.as_ptr(),
                    leading,
                    trailing,
                    0.0,
                    length,
                    actual,
                    output.clusters.as_ptr(),
                    output.advances.as_ptr(),
                    output.offsets.as_ptr(),
                    output.glyph_properties.as_ptr(),
                    output.advances.as_mut_ptr(),
                    output.offsets.as_mut_ptr(),
                )
            };
            if !com_ok(status) {
                return Err(preparation("ApplyCharacterSpacing"));
            }
        }
        output.validate()?;
        Ok(output)
    }
}

struct Input<'a> {
    text: &'a [u16],
    script: Script,
    level: u8,
    face: &'a Com<ffi::Face>,
    em_size: f32,
    locale: &'a [u16],
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Ink,
    Control,
}
pub(super) struct Run {
    source: Range<usize>,
    face: Com<ffi::Face>,
    em_size: f32,
    level: u8,
    visibility: Visibility,
    clusters: Vec<u16>,
    glyphs: Vec<u16>,
    advances: Vec<f32>,
    offsets: Vec<ffi::Offset>,
}
impl Run {
    #[cfg(test)]
    fn width(&self) -> f64 {
        if self.visibility == Visibility::Control {
            return 0.0;
        }
        self.advances
            .iter()
            .map(|&advance| f64::from(advance))
            .sum()
    }
}
struct Output {
    clusters: Vec<u16>,
    text_properties: Vec<u16>,
    glyph_properties: Vec<u16>,
    glyphs: Vec<u16>,
    advances: Vec<f32>,
    offsets: Vec<ffi::Offset>,
}
impl Output {
    fn new(length: usize) -> Result<Self, TextError> {
        Ok(Self {
            clusters: buffer(length, "cluster allocation")?,
            text_properties: buffer(length, "text property allocation")?,
            glyph_properties: Vec::new(),
            glyphs: Vec::new(),
            advances: Vec::new(),
            offsets: Vec::new(),
        })
    }
    fn validate(&self) -> Result<(), TextError> {
        let width: f64 = self
            .advances
            .iter()
            .map(|&advance| f64::from(advance))
            .sum();
        if !width.is_finite()
            || width < 0.0
            || self.advances.iter().any(|value| !value.is_finite())
            || self
                .offsets
                .iter()
                .any(|value| !value.advance.is_finite() || !value.ascender.is_finite())
        {
            return Err(TextError::InvalidMetrics {
                engine: super::ENGINE,
            });
        }
        Ok(())
    }
}

pub(super) fn validate_range(text: &[u16], range: &Range<usize>) -> Result<(), TextError> {
    let boundary = |index| {
        index == text.len()
            || text
                .get(index)
                .is_some_and(|unit| !(0xdc00..=0xdfff).contains(unit))
    };
    if range.start > range.end || !boundary(range.start) || !boundary(range.end) {
        return Err(preparation("glyph source range"));
    }
    Ok(())
}
fn create_face(font: &Com<IDWriteFont>) -> Result<Com<ffi::Face>, TextError> {
    // SAFETY: IDWriteFont slot 13 is CreateFontFace; own the returned reference.
    unsafe {
        let font = font.as_ptr().cast::<ffi::Font>();
        let mut face = std::ptr::null_mut();
        let status = ((*(*font).vtbl).face)(font, &raw mut face);
        own(status, face, "CreateFontFace")
    }
}
fn own<T>(status: Hresult, pointer: *mut T, operation: &'static str) -> Result<Com<T>, TextError> {
    let value = Com::from_raw(pointer);
    if !com_ok(status) {
        return Err(preparation(operation));
    }
    value.ok_or_else(|| preparation(operation))
}
fn buffer<T: Default + Clone>(length: usize, operation: &'static str) -> Result<Vec<T>, TextError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| preparation(operation))?;
    output.resize(length, T::default());
    Ok(output)
}

#[cfg(test)]
#[path = "text_glyphs_tests.rs"]
pub(super) mod tests;

/// DirectWrite FLOATs round layout precision at the native boundary. Reject
/// non-finite/out-of-range inputs before that intentional precision conversion.
#[allow(
    clippy::cast_possible_truncation,
    reason = "DirectWrite requires FLOAT; finite range is checked before IEEE-754 precision rounding"
)]
pub(super) fn native_float(value: f64) -> Result<f32, TextError> {
    if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
        return Err(TextError::InvalidMetrics {
            engine: super::ENGINE,
        });
    }
    Ok(value as f32)
}
