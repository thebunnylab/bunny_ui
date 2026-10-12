//! Native color-glyph calls from pinned `d2d1_3.h` and `dwrite_2/3.h`.

use super::{Com, Guid, Hresult, Run};
use crate::ffi::UnknownVtbl;
use crate::text::{D2dColor, D2dPoint, ID2D1Brush};
use std::ffi::c_void;
use std::marker::PhantomData;

pub(super) const FACTORY4: Guid = Guid {
    d1: 0x4b0b_5bd3,
    d2: 0x0797,
    d3: 0x4549,
    d4: [0x8a, 0xc5, 0xfe, 0x91, 0x5c, 0xc5, 0x38, 0x56],
};
pub(super) const NO_COLOR: Hresult = -2_003_283_956; // 0x8898500C

/// Borrowed `DWRITE_GLYPH_RUN`: pointer fields cannot outlive their owned run.
#[repr(C)]
pub(super) struct GlyphRun<'a> {
    pub(super) face: *mut c_void,
    pub(super) em_size: f32,
    pub(super) count: u32,
    pub(super) indices: *const u16,
    pub(super) advances: *const f32,
    pub(super) offsets: *const super::super::super::ffi::Offset,
    pub(super) sideways: i32,
    pub(super) level: u32,
    _borrow: PhantomData<&'a Run>,
}
impl<'a> GlyphRun<'a> {
    pub(super) fn from_run(run: &'a Run) -> Result<Self, super::TextError> {
        Ok(Self {
            face: run.face.as_ptr().cast(),
            em_size: run.em_size,
            count: u32::try_from(run.glyphs.len())
                .map_err(|_| super::failure("raster glyph count"))?,
            indices: run.glyphs.as_ptr(),
            advances: run.advances.as_ptr(),
            offsets: run.offsets.as_ptr(),
            sideways: 0,
            level: u32::from(run.level),
            _borrow: PhantomData,
        })
    }
}

#[repr(C)]
pub(super) struct Context7 {
    pub(super) vtbl: *const Context7Vtbl,
}
#[repr(C)]
pub(super) struct Context7Vtbl {
    _prefix_0_121: [usize; 122],
    pub(super) draw_color: unsafe extern "system" fn(
        *mut Context7,
        D2dPoint,
        *const GlyphRun<'_>,
        *const c_void,
        *mut ID2D1Brush,
        *mut c_void,
        u32,
        u32,
        u32,
    ),
}
#[repr(C)]
pub(super) struct Context4 {
    pub(super) vtbl: *const Context4Vtbl,
}
#[repr(C)]
pub(super) struct Context4Vtbl {
    _prefix_0_109: [usize; 110],
    pub(super) draw_layout: unsafe extern "system" fn(
        *mut Context4,
        D2dPoint,
        *mut crate::text::IDWriteTextLayout,
        *mut ID2D1Brush,
        *mut c_void,
        u32,
        u32,
    ),
    pub(super) draw_bitmap:
        unsafe extern "system" fn(*mut Context4, u32, D2dPoint, *const GlyphRun<'_>, u32, u32),
    pub(super) draw_svg: unsafe extern "system" fn(
        *mut Context4,
        D2dPoint,
        *const GlyphRun<'_>,
        *mut ID2D1Brush,
        *mut c_void,
        u32,
        u32,
    ),
}
#[repr(C)]
pub(super) struct Target {
    pub(super) vtbl: *const TargetVtbl,
}
#[repr(C)]
pub(super) struct TargetVtbl {
    _prefix_0_28: [usize; 29],
    pub(super) draw:
        unsafe extern "system" fn(*mut Target, D2dPoint, *const GlyphRun<'_>, *mut ID2D1Brush, u32),
}
#[repr(C)]
pub(super) struct Factory4 {
    pub(super) vtbl: *const Factory4Vtbl,
}
#[repr(C)]
pub(super) struct Factory4Vtbl {
    _prefix_0_39: [usize; 40],
    pub(super) translate: unsafe extern "system" fn(
        *mut Factory4,
        D2dPoint,
        *const GlyphRun<'_>,
        *const c_void,
        u32,
        u32,
        *const [f32; 6],
        u32,
        *mut *mut Enumerator1,
    ) -> Hresult,
}
#[repr(C)]
pub(super) struct Factory2 {
    pub(super) vtbl: *const Factory2Vtbl,
}
#[repr(C)]
pub(super) struct Factory2Vtbl {
    _prefix_0_27: [usize; 28],
    pub(super) translate: unsafe extern "system" fn(
        *mut Factory2,
        f32,
        f32,
        *const GlyphRun<'_>,
        *const c_void,
        u32,
        *const [f32; 6],
        u32,
        *mut *mut Enumerator,
    ) -> Hresult,
}

#[repr(C)]
pub(super) struct Layer<'a> {
    pub(super) run: GlyphRun<'a>,
    _description: *const c_void,
    pub(super) origin: D2dPoint,
    pub(super) color: D2dColor,
    pub(super) palette: u16,
}
#[repr(C)]
pub(super) struct Layer1<'a> {
    // C++ inheritance includes all base tail padding (88 bytes on Win64).
    // Flattening these fields would put image_format at the wrong offset.
    pub(super) base: Layer<'a>,
    pub(super) image_format: u32,
    pub(super) measuring_mode: u32,
}
#[repr(C)]
pub(super) struct Enumerator {
    pub(super) vtbl: *const EnumeratorVtbl,
}
#[repr(C)]
pub(super) struct EnumeratorVtbl {
    _unknown: UnknownVtbl,
    pub(super) next: unsafe extern "system" fn(*mut Enumerator, *mut i32) -> Hresult,
    pub(super) current:
        unsafe extern "system" fn(*mut Enumerator, *mut *const Layer<'_>) -> Hresult,
}
#[repr(C)]
pub(super) struct Enumerator1 {
    pub(super) vtbl: *const Enumerator1Vtbl,
}
#[repr(C)]
pub(super) struct Enumerator1Vtbl {
    _unknown: UnknownVtbl,
    pub(super) next: unsafe extern "system" fn(*mut Enumerator1, *mut i32) -> Hresult,
    _base_current: usize,
    pub(super) current:
        unsafe extern "system" fn(*mut Enumerator1, *mut *const Layer1<'_>) -> Hresult,
}

pub(super) enum Translation {
    Modern(Com<Factory4>),
    Layers(Com<Factory2>),
}
