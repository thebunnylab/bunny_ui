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
    InvalidRange,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::IndexOverflow => "text exceeds the native index range",
            Self::ResolutionFailed => "native paragraph resolution failed",
            Self::InvalidRange => "line range is not on source UTF-8 boundaries",
        })
    }
}

/// Immutable paragraph analysis, reusable for overlapping candidate lines.
/// Native L1 resets affect a temporary line window, never this retained state.
pub(super) struct ResolvedText {
    storage: Storage,
}

enum Storage {
    Ascii(usize),
    Native(NativeText),
}

struct NativeText {
    bytes: Vec<usize>,
    types: Vec<c_uint>,
    levels: Vec<i8>,
    paragraphs: Vec<Paragraph>,
}

struct Paragraph {
    scalars: Range<usize>,
    base: c_uint,
}

impl ResolvedText {
    pub(super) fn new(text: &str) -> Result<Self, Error> {
        // HarfBuzz also uses signed byte lengths, so reject before allocating.
        c_int::try_from(text.len()).map_err(|_| Error::IndexOverflow)?;
        if text.is_ascii() {
            return Ok(Self {
                storage: Storage::Ascii(text.len()),
            });
        }
        let (mut bytes, scalars): (Vec<_>, Vec<_>) = text
            .char_indices()
            .map(|(byte, character)| (byte, u32::from(character)))
            .unzip();
        bytes.push(text.len());
        let length = c_int::try_from(scalars.len()).map_err(|_| Error::IndexOverflow)?;
        let mut types = vec![0; scalars.len()];
        let mut brackets = vec![0; scalars.len()];
        let mut levels = vec![0; scalars.len()];
        // SAFETY: equally sized uint32_t arrays; calls retain no pointers.
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
        let mut paragraphs = Vec::new();
        let mut start = 0;
        for paragraph in types.split_inclusive(|kind| *kind == separator) {
            let end = start + paragraph.len();
            let length = c_int::try_from(paragraph.len()).map_err(|_| Error::IndexOverflow)?;
            let mut base = neutral;
            // SAFETY: this complete paragraph's type, bracket and level windows
            // have the same length; native resolution writes only its window.
            let resolved = unsafe {
                fribidi_get_par_embedding_levels_ex(
                    paragraph.as_ptr(),
                    brackets[start..end].as_ptr(),
                    length,
                    &raw mut base,
                    levels[start..end].as_mut_ptr(),
                )
            };
            if resolved == 0 {
                return Err(Error::ResolutionFailed);
            }
            paragraphs.push(Paragraph {
                scalars: start..end,
                base,
            });
            start = end;
        }
        Ok(Self {
            storage: Storage::Native(NativeText {
                bytes,
                types,
                levels,
                paragraphs,
            }),
        })
    }

    /// Visual runs retain byte offsets in the original source. Invalid or
    /// mid-scalar boundaries fail explicitly rather than clamping silently.
    pub(super) fn line_runs(&self, range: Range<usize>) -> Result<Vec<Run>, Error> {
        if range.start > range.end {
            return Err(Error::InvalidRange);
        }
        let native = match &self.storage {
            Storage::Ascii(length) => {
                if range.end > *length {
                    return Err(Error::InvalidRange);
                }
                return Ok(if range.is_empty() {
                    Vec::new()
                } else {
                    vec![Run {
                        bytes: range,
                        direction: Direction::LeftToRight,
                    }]
                });
            }
            Storage::Native(native) => native,
        };
        let start = native
            .bytes
            .binary_search(&range.start)
            .map_err(|_| Error::InvalidRange)?;
        let end = native
            .bytes
            .binary_search(&range.end)
            .map_err(|_| Error::InvalidRange)?;
        if start == end {
            return Ok(Vec::new());
        }
        let first = native
            .paragraphs
            .partition_point(|p| p.scalars.end <= start);
        let mut result = Vec::new();
        for paragraph in native.paragraphs[first..]
            .iter()
            .take_while(|p| p.scalars.start < end)
        {
            let a = start.max(paragraph.scalars.start);
            let b = end.min(paragraph.scalars.end);
            result.extend(reordered_runs(
                &native.types[a..b],
                &native.levels[a..b],
                &native.bytes[a..=b],
                paragraph.base,
            )?);
        }
        Ok(result)
    }
}

/// Legacy whole-line entry, using the same immutable paragraph analysis.
pub(super) fn runs(text: &str) -> Result<Vec<Run>, Error> {
    ResolvedText::new(text)?.line_runs(0..text.len())
}

fn reordered_runs(
    types: &[c_uint],
    paragraph_levels: &[i8],
    bytes: &[usize],
    base: c_uint,
) -> Result<Vec<Run>, Error> {
    let length = c_int::try_from(types.len()).map_err(|_| Error::IndexOverflow)?;
    let mut levels = paragraph_levels.to_vec();
    let mut visual_to_logical: Vec<c_int> = (0..length).collect();
    // SAFETY: the pinned native implementation accesses only [off, off+len).
    // These line windows use off=0 with the original resolved paragraph base.
    // L1 mutates only the copied levels; null visual is supported and flags=0
    // leave mirroring/mark shaping to HarfBuzz. The map starts as identity.
    let resolved = unsafe {
        fribidi_reorder_line(
            0,
            types.as_ptr(),
            length,
            0,
            base,
            levels.as_mut_ptr(),
            std::ptr::null_mut(),
            visual_to_logical.as_mut_ptr(),
        )
    };
    if resolved == 0 {
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
