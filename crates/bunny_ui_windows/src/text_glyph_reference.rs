//! Independent full-layout glyph records used to diagnose native pixel parity.

use super::{Com, GlyphRun, Guid, Hresult};
use crate::ffi::UnknownVtbl;
use crate::text::{IDWriteTextLayout, com_ok};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
struct Renderer {
    vtbl: *const Vtable,
    references: AtomicU32,
    scale: f32,
}
type Decoration = unsafe extern "system" fn(
    *mut Renderer,
    *mut c_void,
    f32,
    f32,
    *const c_void,
    *mut c_void,
) -> Hresult;
#[repr(C)]
struct Vtable {
    unknown: UnknownVtbl,
    disabled: unsafe extern "system" fn(*mut Renderer, *mut c_void, *mut i32) -> Hresult,
    transform: unsafe extern "system" fn(*mut Renderer, *mut c_void, *mut [f32; 6]) -> Hresult,
    scale: unsafe extern "system" fn(*mut Renderer, *mut c_void, *mut f32) -> Hresult,
    glyphs: unsafe extern "system" fn(
        *mut Renderer,
        *mut c_void,
        f32,
        f32,
        u32,
        *const GlyphRun<'_>,
        *const c_void,
        *mut c_void,
    ) -> Hresult,
    underline: Decoration,
    strike: Decoration,
    inline: unsafe extern "system" fn(
        *mut Renderer,
        *mut c_void,
        f32,
        f32,
        *mut c_void,
        i32,
        i32,
        *mut c_void,
    ) -> Hresult,
}
static VTABLE: Vtable = Vtable {
    unknown: UnknownVtbl {
        query_interface: query,
        add_ref: retain,
        release,
    },
    disabled,
    transform,
    scale,
    glyphs,
    underline: decoration,
    strike: decoration,
    inline,
};
unsafe extern "system" fn query(
    this: *mut c_void,
    id: *const Guid,
    output: *mut *mut c_void,
) -> Hresult {
    if id.is_null() || output.is_null() {
        return -2_147_467_261;
    }
    // SAFETY: valid COM parameters and this renderer's live allocation.
    unsafe {
        *output = std::ptr::null_mut();
        let id = &*id;
        let key = (id.d1, id.d2, id.d3, id.d4);
        if matches!(
            key,
            (0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46])
                | (
                    0xeaf3_a2da,
                    0xecf4,
                    0x4d24,
                    [0xb6, 0x44, 0xb3, 0x4f, 0x68, 0x42, 0x02, 0x4b]
                )
                | (
                    0xef8a_8135,
                    0x5cc6,
                    0x45fe,
                    [0x88, 0x25, 0xc5, 0xa0, 0x72, 0x4e, 0xb8, 0x19]
                )
        ) {
            retain(this);
            *output = this;
            0
        } else {
            -2_147_467_262
        }
    }
}
unsafe extern "system" fn retain(this: *mut c_void) -> u32 {
    // SAFETY: the caller owns a live reference.
    unsafe {
        (*this.cast::<Renderer>())
            .references
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}
unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    // SAFETY: the final release consumes exactly the original Box allocation.
    unsafe {
        let remaining = (*this.cast::<Renderer>())
            .references
            .fetch_sub(1, Ordering::AcqRel)
            - 1;
        if remaining == 0 {
            drop(Box::from_raw(this.cast::<Renderer>()));
        }
        remaining
    }
}
unsafe extern "system" fn disabled(_: *mut Renderer, _: *mut c_void, output: *mut i32) -> Hresult {
    if output.is_null() {
        return -2_147_467_261;
    }
    // SAFETY: the native caller supplies its BOOL output.
    unsafe {
        *output = 0;
    }
    0
}
unsafe extern "system" fn transform(
    _: *mut Renderer,
    _: *mut c_void,
    output: *mut [f32; 6],
) -> Hresult {
    if output.is_null() {
        return -2_147_467_261;
    }
    // SAFETY: the native caller supplies its DWRITE_MATRIX output.
    unsafe {
        *output = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    0
}
unsafe extern "system" fn scale(this: *mut Renderer, _: *mut c_void, output: *mut f32) -> Hresult {
    if output.is_null() {
        return -2_147_467_261;
    }
    // SAFETY: the renderer owns its scale, and the native output is valid.
    unsafe {
        *output = (*this).scale;
    }
    0
}
unsafe extern "system" fn glyphs(
    _: *mut Renderer,
    _: *mut c_void,
    x: f32,
    y: f32,
    mode: u32,
    run: *const GlyphRun<'_>,
    _: *const c_void,
    _: *mut c_void,
) -> Hresult {
    if run.is_null() {
        return -2_147_467_261;
    }
    // SAFETY: IDWriteTextLayout owns these arrays throughout its synchronous
    // DrawGlyphRun callback. Nothing borrowed survives this callback.
    unsafe {
        let run = &*run;
        if run.count == 0 {
            return 0;
        }
        if run.indices.is_null() || run.advances.is_null() || run.offsets.is_null() {
            return -2_147_467_261;
        }
        eprintln!(
            "native layout run: origin=({x},{y}), mode={mode}, em={}, level={}, glyphs={:?}, advances={:?}, offsets={:?}",
            run.em_size,
            run.level,
            std::slice::from_raw_parts(run.indices, run.count as usize),
            std::slice::from_raw_parts(run.advances, run.count as usize),
            std::slice::from_raw_parts(run.offsets, run.count as usize)
        );
    }
    0
}
unsafe extern "system" fn decoration(
    _: *mut Renderer,
    _: *mut c_void,
    _: f32,
    _: f32,
    _: *const c_void,
    _: *mut c_void,
) -> Hresult {
    -2_147_467_263
}
unsafe extern "system" fn inline(
    _: *mut Renderer,
    _: *mut c_void,
    _: f32,
    _: f32,
    _: *mut c_void,
    _: i32,
    _: i32,
    _: *mut c_void,
) -> Hresult {
    -2_147_467_263
}

pub(super) fn trace(layout: &Com<IDWriteTextLayout>, scale: f32) {
    #[repr(C)]
    struct Layout {
        vtbl: *const LayoutTable,
    }
    #[repr(C)]
    struct LayoutTable {
        prefix: [usize; 58],
        draw:
            unsafe extern "system" fn(*mut Layout, *mut c_void, *mut Renderer, f32, f32) -> Hresult,
    }
    let renderer = Com::from_raw(Box::into_raw(Box::new(Renderer {
        vtbl: &VTABLE,
        references: AtomicU32::new(1),
        scale,
    })))
    .unwrap();
    // SAFETY: exact IDWriteTextLayout::Draw slot and an owned text renderer.
    unsafe {
        let pointer = layout.as_ptr().cast::<Layout>();
        assert!(com_ok(((*(*pointer).vtbl).draw)(
            pointer,
            std::ptr::null_mut(),
            renderer.as_ptr(),
            8.0,
            8.0
        )));
        assert_eq!((*renderer.as_ptr()).references.load(Ordering::Acquire), 1);
    }
}
