//! DirectWrite glyph/fallback ABI, pinned to the same SDK as paragraph analysis.

use super::{Com, Guid, Hresult, Script, Source, TextError, com_ok, preparation};
use crate::ffi::UnknownVtbl;
use crate::text::{IDWriteFont, IDWriteFontCollection};
use std::ffi::c_void;

pub(super) const FACTORY2: Guid = Guid {
    d1: 0x0439_fc60,
    d2: 0xca44,
    d3: 0x4994,
    d4: [0x8d, 0xee, 0x3a, 0x9a, 0xf7, 0xb7, 0x32, 0xec],
};
pub(super) const ANALYZER1: Guid = Guid {
    d1: 0x80da_d800,
    d2: 0xe21f,
    d3: 0x4e83,
    d4: [0x96, 0xce, 0xbf, 0xcc, 0xe5, 0x00, 0xdb, 0x7c],
};

// All callers supply an owned native interface. The queried interface carries
// its own reference, including when the original owner goes away.
pub(super) fn query<T, U>(
    owner: &Com<T>,
    id: &Guid,
    operation: &'static str,
) -> Result<Com<U>, TextError> {
    query_optional(owner, id, operation)?.ok_or_else(|| preparation(operation))
}

pub(super) fn query_optional<T, U>(
    owner: &Com<T>,
    id: &Guid,
    operation: &'static str,
) -> Result<Option<Com<U>>, TextError> {
    // SAFETY: all callers supply real COM owners and exact SDK interface IDs.
    unsafe {
        let pointer = owner.as_ptr().cast::<c_void>();
        let table = &**pointer.cast::<*const UnknownVtbl>();
        let mut output = std::ptr::null_mut();
        let status = (table.query_interface)(pointer, id, &raw mut output);
        let result = Com::from_raw(output.cast::<U>());
        if status == super::super::NO_INTERFACE {
            return Ok(None);
        }
        if !com_ok(status) {
            return Err(preparation(operation));
        }
        result.map(Some).ok_or_else(|| preparation(operation))
    }
}

#[repr(C)]
pub(super) struct Factory {
    pub(super) vtbl: *const FactoryVtbl,
}
#[repr(C)]
pub(super) struct FactoryVtbl {
    _unknown: UnknownVtbl,
    _pad_3_25: [usize; 23],
    pub(super) fallback: unsafe extern "system" fn(*mut Factory, *mut *mut Fallback) -> Hresult,
}
#[repr(C)]
pub(super) struct Fallback {
    pub(super) vtbl: *const FallbackVtbl,
}
#[repr(C)]
pub(super) struct FallbackVtbl {
    _unknown: UnknownVtbl,
    pub(super) map: unsafe extern "system" fn(
        *mut Fallback,
        *mut Source,
        u32,
        u32,
        *mut IDWriteFontCollection,
        *const u16,
        u32,
        u32,
        u32,
        *mut u32,
        *mut *mut IDWriteFont,
        *mut f32,
    ) -> Hresult,
}

#[repr(C)]
pub(super) struct Format {
    pub(super) vtbl: *const FormatVtbl,
}
#[repr(C)]
pub(super) struct FormatVtbl {
    _unknown: UnknownVtbl,
    _pad_3_18: [usize; 16],
    pub(super) collection:
        unsafe extern "system" fn(*mut Format, *mut *mut IDWriteFontCollection) -> Hresult,
    pub(super) family_length: unsafe extern "system" fn(*mut Format) -> u32,
    pub(super) family: unsafe extern "system" fn(*mut Format, *mut u16, u32) -> Hresult,
}
#[repr(C)]
pub(super) struct Font {
    pub(super) vtbl: *const FontVtbl,
}
#[repr(C)]
pub(super) struct FontVtbl {
    _unknown: UnknownVtbl,
    _pad_3_12: [usize; 10],
    pub(super) face: unsafe extern "system" fn(*mut Font, *mut *mut Face) -> Hresult,
}
#[repr(C)]
pub(super) struct Face {
    _vtbl: *const UnknownVtbl,
}

// DWRITE_SHAPING_TEXT_PROPERTIES and DWRITE_SHAPING_GLYPH_PROPERTIES
// are each one UINT16 bitfield, stored opaquely. DWRITE_GLYPH_OFFSET is
// two FLOATs, not integral font-design units.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(super) struct Offset {
    pub(super) advance: f32,
    pub(super) ascender: f32,
}
#[repr(C)]
pub(super) struct Analyzer {
    pub(super) vtbl: *const AnalyzerVtbl,
}
#[repr(C)]
pub(super) struct AnalyzerVtbl {
    _unknown: UnknownVtbl,
    _pad_3_6: [usize; 4],
    pub(super) glyphs: unsafe extern "system" fn(
        *mut Analyzer,
        *const u16,
        u32,
        *mut Face,
        i32,
        i32,
        *const Script,
        *const u16,
        *mut c_void,
        *const *const c_void,
        *const u32,
        u32,
        u32,
        *mut u16,
        *mut u16,
        *mut u16,
        *mut u16,
        *mut u32,
    ) -> Hresult,
    pub(super) placements: unsafe extern "system" fn(
        *mut Analyzer,
        *const u16,
        *const u16,
        *mut u16,
        u32,
        *const u16,
        *const u16,
        u32,
        *mut Face,
        f32,
        i32,
        i32,
        *const Script,
        *const u16,
        *const *const c_void,
        *const u32,
        u32,
        *mut f32,
        *mut Offset,
    ) -> Hresult,
    _gdi_placements: usize,
    pub(super) spacing: unsafe extern "system" fn(
        *mut Analyzer,
        f32,
        f32,
        f32,
        u32,
        u32,
        *const u16,
        *const f32,
        *const Offset,
        *const u16,
        *mut f32,
        *mut Offset,
    ) -> Hresult,
    // IDWriteTextAnalyzer1 slots 11..14 precede GetTextComplexity at 15.
    _pad_11_14: [usize; 4],
    pub(super) complexity: unsafe extern "system" fn(
        *mut Analyzer,
        *const u16,
        u32,
        *mut Face,
        *mut i32,
        *mut u32,
        *mut u16,
    ) -> Hresult,
}
