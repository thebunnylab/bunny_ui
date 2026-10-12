//! Independent full-layout glyph records used for native glyph and pixel oracles.

use super::{Com, GlyphRun, Guid, Hresult};
use crate::ffi::UnknownVtbl;
use crate::text::{DwriteTextMetrics, IDWriteTextLayout, com_ok};
use std::cell::RefCell;
use std::ffi::c_void;
use std::ops::Range;
use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
struct Renderer {
    vtbl: *const Vtable,
    references: AtomicU32,
    scale: f32,
    trace: bool,
    callbacks: AtomicU32,
    empty_callbacks: AtomicU32,
    error: RefCell<Option<&'static str>>,
    records: RefCell<Vec<NativeRun>>,
}
// DWRITE_GLYPH_RUN_DESCRIPTION from the same pinned dwrite.h. Text positions
// are original UTF-16 offsets; the cluster map is relative to this glyph run.
#[repr(C)]
struct Description {
    locale: *const u16,
    text: *const u16,
    length: u32,
    clusters: *const u16,
    position: u32,
}

#[derive(Debug)]
pub(in crate::text::analysis::glyphs) struct NativeRun {
    pub(in crate::text::analysis::glyphs) source: Range<usize>,
    pub(in crate::text::analysis::glyphs) em_size: f32,
    pub(in crate::text::analysis::glyphs) level: u32,
    pub(in crate::text::analysis::glyphs) clusters: Vec<u16>,
    pub(in crate::text::analysis::glyphs) glyphs: Vec<u16>,
    pub(in crate::text::analysis::glyphs) advances: Vec<f32>,
    pub(in crate::text::analysis::glyphs) offsets: Vec<super::super::super::ffi::Offset>,
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
    this: *mut Renderer,
    _: *mut c_void,
    x: f32,
    y: f32,
    mode: u32,
    run: *const GlyphRun<'_>,
    description: *const c_void,
    _: *mut c_void,
) -> Hresult {
    // SAFETY: the native caller holds this synchronous renderer alive.
    unsafe {
        (*this).callbacks.fetch_add(1, Ordering::Relaxed);
    }
    // SAFETY: native arrays remain borrowed only throughout this callback.
    let record = unsafe { snapshot(run, description) };
    // SAFETY: the caller retains this renderer for synchronous drawing.
    unsafe {
        match record {
            Ok(None) => {
                (*this).empty_callbacks.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Some(record)) => {
                if (*this).trace {
                    eprintln!("native layout run: origin=({x},{y}), mode={mode}, {record:?}");
                }
                (*this).records.borrow_mut().push(record);
            }
            Err(error) => {
                (*this).error.borrow_mut().get_or_insert(error);
                return -2_147_467_259; // E_FAIL; also retained if Draw ignores it.
            }
        }
    }
    0
}

unsafe fn snapshot(
    run: *const GlyphRun<'_>,
    description: *const c_void,
) -> Result<Option<NativeRun>, &'static str> {
    // SAFETY: the callback's native pointers are checked before dereferencing;
    // non-null arrays are supplied for their declared lengths by TextLayout.
    unsafe {
        let run = run.as_ref().ok_or("missing glyph run")?;
        if run.count == 0 {
            return Ok(None);
        }
        let description = description
            .cast::<Description>()
            .as_ref()
            .ok_or("missing glyph description")?;
        eprintln!(
            "native glyph arrays: count={}, units={}, indices={}, advances={}, offsets={}, clusters={}",
            run.count,
            description.length,
            !run.indices.is_null(),
            !run.advances.is_null(),
            !run.offsets.is_null(),
            !description.clusters.is_null()
        );
        if run.indices.is_null() {
            return Err("missing required glyph indices");
        }
        if run.advances.is_null() {
            return Err("native reference omitted advances");
        }
        if description.length == 0 {
            return Err("empty native source description");
        }
        if description.clusters.is_null() {
            return Err("native reference omitted cluster map");
        }
        let start = description.position as usize;
        let end = start
            .checked_add(description.length as usize)
            .ok_or("source range overflow")?;
        // The pinned SDK marks glyphOffsets optional. An absent array means no
        // displacement, not an invalid native run; preserve explicit arrays.
        let offsets = if run.offsets.is_null() {
            vec![super::super::super::ffi::Offset::default(); run.count as usize]
        } else {
            std::slice::from_raw_parts(run.offsets, run.count as usize).to_vec()
        };
        Ok(Some(NativeRun {
            source: start..end,
            em_size: run.em_size,
            level: run.level,
            clusters: std::slice::from_raw_parts(description.clusters, description.length as usize)
                .to_vec(),
            glyphs: std::slice::from_raw_parts(run.indices, run.count as usize).to_vec(),
            advances: std::slice::from_raw_parts(run.advances, run.count as usize).to_vec(),
            offsets,
        }))
    }
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
    draw(layout, scale, true);
}

pub(in crate::text::analysis::glyphs) fn collect(
    layout: &Com<IDWriteTextLayout>,
) -> Vec<NativeRun> {
    let mut metrics = DwriteTextMetrics::default();
    // SAFETY: the independently created layout owns its original text and font.
    // Realize native layout before collecting its synchronous draw callbacks.
    unsafe {
        assert!(com_ok(((*(*layout.as_ptr()).vtbl).get_metrics)(
            layout.as_ptr(),
            &raw mut metrics
        )));
    }
    let records = draw(layout, 1.0, false);
    eprintln!(
        "native glyph reference: width={}, height={}, lines={}, runs={}",
        metrics.width_including_trailing_whitespace,
        metrics.height,
        metrics.line_count,
        records.len()
    );
    records
}

fn draw(layout: &Com<IDWriteTextLayout>, scale: f32, trace: bool) -> Vec<NativeRun> {
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
        vtbl: &raw const VTABLE,
        references: AtomicU32::new(1),
        scale,
        trace,
        callbacks: AtomicU32::new(0),
        empty_callbacks: AtomicU32::new(0),
        error: RefCell::new(None),
        records: RefCell::new(Vec::new()),
    })))
    .unwrap();
    // SAFETY: exact IDWriteTextLayout::Draw slot and an owned text renderer.
    unsafe {
        let pointer = layout.as_ptr().cast::<Layout>();
        let status =
            ((*(*pointer).vtbl).draw)(pointer, std::ptr::null_mut(), renderer.as_ptr(), 8.0, 8.0);
        eprintln!(
            "native draw: thread={:?}, status={status:#x}, callbacks={}, empty={}",
            std::thread::current().name(),
            (*renderer.as_ptr()).callbacks.load(Ordering::Relaxed),
            (*renderer.as_ptr()).empty_callbacks.load(Ordering::Relaxed)
        );
        let error = (*renderer.as_ptr()).error.take();
        assert!(
            error.is_none(),
            "native renderer callback rejected data: {error:?}"
        );
        assert!(com_ok(status));
        assert_eq!((*renderer.as_ptr()).references.load(Ordering::Acquire), 1);
        (*renderer.as_ptr()).records.take()
    }
}
