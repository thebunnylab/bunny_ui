//! Resolve visual runs without rewriting the logical text passed to `HarfBuzz`.
//!
//! `FriBidi` owns paragraph levels, brackets and line reordering; `HarfBuzz` owns
//! shaping, mirroring and marks inside each logical run. All native indices
//! count Unicode scalars here, converted to UTF-8 byte ranges before leaving.
//! This is the existing Linux FFI boundary, not a second text engine.

use std::ffi::{c_int, c_uint};
use std::ops::Range;

#[link(name = "fribidi")]
unsafe extern "C" {
    // FriBidi 1.0: Char/CharType/BracketType/ParType/Flags are uint32_t;
    // StrIndex is int and Level is signed char. See lib/fribidi-{types,
    // bidi-types,bidi,brackets}.h at b54871c339dabb7434718da3fed2fa63320997e5.
    fn fribidi_get_bidi_type(character: u32) -> c_uint;
    fn fribidi_get_bidi_types(text: *const u32, length: c_int, types: *mut c_uint);
    fn fribidi_get_bracket_types(
        text: *const u32,
        length: c_int,
        types: *const c_uint,
        brackets: *mut c_uint,
    );
    fn fribidi_get_par_embedding_levels_ex(
        types: *const c_uint,
        brackets: *const c_uint,
        length: c_int,
        base: *mut c_uint,
        levels: *mut i8,
    ) -> i8;
    fn fribidi_reorder_line(
        flags: c_uint,
        types: *const c_uint,
        length: c_int,
        offset: c_int,
        base: c_uint,
        levels: *mut i8,
        visual: *mut u32,
        map: *mut c_int,
    ) -> i8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction {
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Run {
    pub(super) bytes: Range<usize>,
    pub(super) direction: Direction,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Error {
    IndexOverflow,
    ResolutionFailed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::IndexOverflow => "text exceeds the native index range",
            Self::ResolutionFailed => "native paragraph resolution failed",
        })
    }
}

/// Logical UTF-8 ranges in visual order. Each paragraph chooses its own first
/// strong direction. A line is normally one paragraph; explicit paragraph
/// separators reset resolution rather than leaking one paragraph's controls.
pub(super) fn runs(text: &str) -> Result<Vec<Run>, Error> {
    // HarfBuzz also consumes signed byte lengths, so reject that boundary before
    // allocating the scalar maps, even when the scalar count would fit.
    c_int::try_from(text.len()).map_err(|_| Error::IndexOverflow)?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    // ASCII has no RTL or embedding controls. Its single LTR run preserves
    // the former shaping path without allocating scalar and level maps.
    if text.is_ascii() {
        return Ok(vec![Run {
            bytes: 0..text.len(),
            direction: Direction::LeftToRight,
        }]);
    }
    let (mut bytes, scalars): (Vec<_>, Vec<_>) = text
        .char_indices()
        .map(|(byte, character)| (byte, u32::from(character)))
        .unzip();
    bytes.push(text.len());
    let length = c_int::try_from(scalars.len()).map_err(|_| Error::IndexOverflow)?;
    let mut types = vec![0; scalars.len()];
    let mut brackets = vec![0; scalars.len()];
    // SAFETY: the three arrays have `length` elements of the header's uint32_t
    // types. Native calls borrow them for their duration and retain nothing.
    let (neutral, separator) = unsafe {
        fribidi_get_bidi_types(scalars.as_ptr(), length, types.as_mut_ptr());
        fribidi_get_bracket_types(
            scalars.as_ptr(),
            length,
            types.as_ptr(),
            brackets.as_mut_ptr(),
        );
        (
            fribidi_get_bidi_type(u32::from('!')),
            fribidi_get_bidi_type(u32::from('\n')),
        )
    };
    let mut result = Vec::new();
    let mut start = 0;
    for paragraph in types.split_inclusive(|kind| *kind == separator) {
        let end = start + paragraph.len();
        result.extend(paragraph_runs(
            paragraph,
            &brackets[start..end],
            &bytes[start..=end],
            neutral,
        )?);
        start = end;
    }
    Ok(result)
}

fn paragraph_runs(
    types: &[c_uint],
    brackets: &[c_uint],
    bytes: &[usize],
    mut base: c_uint,
) -> Result<Vec<Run>, Error> {
    let length = c_int::try_from(types.len()).map_err(|_| Error::IndexOverflow)?;
    let mut levels = vec![0; types.len()];
    let mut visual_to_logical: Vec<c_int> = (0..length).collect();
    // SAFETY: equal-length typed arrays, an initialized identity map, and no
    // retained pointers. Null visual text is explicitly supported. Flags zero
    // leave mirroring and combining-mark shaping to HarfBuzz, which receives
    // the original logical substring rather than a reordered scalar string.
    let resolved = unsafe {
        fribidi_get_par_embedding_levels_ex(
            types.as_ptr(),
            brackets.as_ptr(),
            length,
            &raw mut base,
            levels.as_mut_ptr(),
        ) != 0
            && fribidi_reorder_line(
                0,
                types.as_ptr(),
                length,
                0,
                base,
                levels.as_mut_ptr(),
                std::ptr::null_mut(),
                visual_to_logical.as_mut_ptr(),
            ) != 0
    };
    if !resolved {
        return Err(Error::ResolutionFailed);
    }
    let mut logical_to_visual = vec![0; types.len()];
    for (visual, logical) in visual_to_logical.into_iter().enumerate() {
        let logical = usize::try_from(logical).map_err(|_| Error::ResolutionFailed)?;
        let slot = logical_to_visual
            .get_mut(logical)
            .ok_or(Error::ResolutionFailed)?;
        *slot = visual;
    }
    let mut ordered = Vec::new();
    let mut start = 0;
    while start < levels.len() {
        let level = levels[start];
        let end = start
            + levels[start..]
                .iter()
                .take_while(|&&next| next == level)
                .count();
        // Every homogeneous level span stays contiguous under L2. Its lowest
        // visual index is its position among runs; marks stay inside the span.
        let first = logical_to_visual[start..end]
            .iter()
            .copied()
            .min()
            .ok_or(Error::ResolutionFailed)?;
        ordered.push((
            first,
            Run {
                bytes: bytes[start]..bytes[end],
                direction: if level & 1 == 0 {
                    Direction::LeftToRight
                } else {
                    Direction::RightToLeft
                },
            },
        ));
        start = end;
    }
    ordered.sort_unstable_by_key(|(first, _)| *first);
    Ok(ordered.into_iter().map(|(_, run)| run).collect())
}
