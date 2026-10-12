//! Owned COM callbacks keep analysis buffers live and reject invalid native ranges.

use super::{
    Arc, AtomicU32, Com, FAILED, Guid, Hresult, INVALID_ARGUMENT, Levels, Mutex, NO_INTERFACE, OK,
    Output, POINTER, Script, Sink, SinkVtbl, Source, SourceVtbl, TextError, c_void, preparation,
};
use crate::ffi::UnknownVtbl;
use std::sync::atomic::{Ordering, fence};

// IUnknown, IDWriteTextAnalysisSource and IDWriteTextAnalysisSink, dwrite.h.
const UNKNOWN: Guid = Guid {
    d1: 0,
    d2: 0,
    d3: 0,
    d4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
};
const SOURCE: Guid = Guid {
    d1: 0x688e_1a58,
    d2: 0x5094,
    d3: 0x47c8,
    d4: [0xad, 0xc8, 0xfb, 0xce, 0xa6, 0x0a, 0xe9, 0x2b],
};
const SINK: Guid = Guid {
    d1: 0x5810_cd44,
    d2: 0x0ca0,
    d3: 0x4701,
    d4: [0xb3, 0xfa, 0xbe, 0xc5, 0x18, 0x2a, 0xe4, 0xf6],
};

trait Interface {
    const ID: Guid;
    fn references(&self) -> &AtomicU32;
}
impl Interface for Source {
    const ID: Guid = SOURCE;
    fn references(&self) -> &AtomicU32 {
        &self.references
    }
}
impl Interface for Sink {
    const ID: Guid = SINK;
    fn references(&self) -> &AtomicU32 {
        &self.references
    }
}

const fn unknown<T: Interface>() -> UnknownVtbl {
    UnknownVtbl {
        query_interface: query::<T>,
        add_ref: add_ref::<T>,
        release: release::<T>,
    }
}
unsafe extern "system" fn query<T: Interface>(
    this: *mut c_void,
    id: *const Guid,
    result: *mut *mut c_void,
) -> Hresult {
    if result.is_null() {
        return POINTER;
    }
    // SAFETY: COM supplies writable output and a live this pointer. Null
    // optional/bad input pointers are rejected before they are dereferenced.
    unsafe {
        *result = std::ptr::null_mut();
        if this.is_null() || id.is_null() {
            return POINTER;
        }
        if *id != UNKNOWN && *id != T::ID {
            return NO_INTERFACE;
        }
        add_ref::<T>(this);
        *result = this;
    }
    OK
}
unsafe extern "system" fn add_ref<T: Interface>(this: *mut c_void) -> u32 {
    // SAFETY: one live COM reference guarantees a valid T until release.
    unsafe {
        (*this.cast::<T>())
            .references()
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}
unsafe extern "system" fn release<T: Interface>(this: *mut c_void) -> u32 {
    // SAFETY: every successful QueryInterface/AddRef has one matching release;
    // the final release owns the Box, after earlier writes become visible.
    unsafe {
        let count = (*this.cast::<T>())
            .references()
            .fetch_sub(1, Ordering::Release)
            - 1;
        if count == 0 {
            fence(Ordering::Acquire);
            drop(Box::from_raw(this.cast::<T>()));
        }
        count
    }
}

impl Source {
    pub(super) fn new(text: Arc<[u16]>, locale: Arc<[u16]>) -> Result<Com<Self>, TextError> {
        let length = u32::try_from(text.len()).map_err(|_| TextError::IndexOverflow {
            engine: super::ENGINE,
        })?;
        if locale.last() != Some(&0) {
            return Err(preparation("locale terminator"));
        }
        let source = Box::new(Self {
            vtbl: &raw const SOURCE_VTBL,
            references: AtomicU32::new(1),
            text,
            locale,
            length,
        });
        Com::from_raw(Box::into_raw(source)).ok_or_else(|| preparation("analysis source"))
    }
}

pub(super) const fn source_vtbl() -> SourceVtbl {
    SourceVtbl {
        unknown: unknown::<Source>(),
        text_at,
        text_before,
        reading_direction,
        locale,
        number_substitution,
    }
}
static SOURCE_VTBL: SourceVtbl = source_vtbl();

unsafe extern "system" fn text_at(
    this: *mut Source,
    position: u32,
    text: *mut *const u16,
    length: *mut u32,
) -> Hresult {
    if this.is_null() || text.is_null() || length.is_null() {
        return POINTER;
    }
    // SAFETY: the source owns its stable buffer; return only in-bounds windows.
    unsafe {
        let source = &*this;
        *text = std::ptr::null();
        *length = 0;
        if position < source.length {
            let Ok(offset) = usize::try_from(position) else {
                return INVALID_ARGUMENT;
            };
            *text = source.text.as_ptr().add(offset);
            *length = source.length - position;
        }
    }
    OK
}
unsafe extern "system" fn text_before(
    this: *mut Source,
    position: u32,
    text: *mut *const u16,
    length: *mut u32,
) -> Hresult {
    if this.is_null() || text.is_null() || length.is_null() {
        return POINTER;
    }
    // SAFETY: native positions count code units, so a position between the
    // halves of a surrogate pair still denotes a valid buffer window here.
    unsafe {
        let source = &*this;
        *text = std::ptr::null();
        *length = 0;
        if position > 0 && position <= source.length {
            *text = source.text.as_ptr();
            *length = position;
        }
    }
    OK
}
unsafe extern "system" fn reading_direction(_this: *mut Source) -> u32 {
    // Match IDWriteTextFormat's existing default LTR paragraph base; embedded
    // controls and implicit directions are resolved by AnalyzeBidi itself.
    0
}
unsafe extern "system" fn locale(
    this: *mut Source,
    position: u32,
    length: *mut u32,
    name: *mut *const u16,
) -> Hresult {
    if this.is_null() || length.is_null() || name.is_null() {
        return POINTER;
    }
    // SAFETY: locale is terminated and owned for the entire analysis call.
    unsafe {
        let source = &*this;
        *length = source.length.saturating_sub(position);
        *name = source.locale.as_ptr();
    }
    OK
}
unsafe extern "system" fn number_substitution(
    this: *mut Source,
    position: u32,
    length: *mut u32,
    substitution: *mut *mut c_void,
) -> Hresult {
    if this.is_null() || length.is_null() || substitution.is_null() {
        return POINTER;
    }
    // SAFETY: null substitution selects nominal digits, matching the existing
    // engine, and transfers no native reference to the caller.
    unsafe {
        *length = (*this).length.saturating_sub(position);
        *substitution = std::ptr::null_mut();
    }
    OK
}

impl Sink {
    pub(super) fn new(length: usize) -> Result<Com<Self>, TextError> {
        let sink = Box::new(Self {
            vtbl: &raw const SINK_VTBL,
            references: AtomicU32::new(1),
            output: Mutex::new(Output {
                scripts: vec![None; length],
                levels: vec![None; length],
            }),
        });
        Com::from_raw(Box::into_raw(sink)).ok_or_else(|| preparation("analysis sink"))
    }
}
static SINK_VTBL: SinkVtbl = SinkVtbl {
    unknown: unknown::<Sink>(),
    script,
    breakpoints,
    bidi,
    number_substitution: sink_number_substitution,
};

fn window(start: u32, length: u32) -> Option<std::ops::Range<usize>> {
    let end = usize::try_from(start.checked_add(length)?).ok()?;
    Some(usize::try_from(start).ok()?..end)
}
unsafe extern "system" fn script(
    this: *mut Sink,
    position: u32,
    length: u32,
    analysis: *const Script,
) -> Hresult {
    if this.is_null() || analysis.is_null() {
        return POINTER;
    }
    let Some(range) = window(position, length) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: native passes a live sink and one SDK-layout script value.
    unsafe {
        let Ok(mut output) = (*this).output.lock() else {
            return FAILED;
        };
        let Some(cells) = output.scripts.get_mut(range) else {
            return INVALID_ARGUMENT;
        };
        cells.fill(Some(*analysis));
    }
    OK
}
unsafe extern "system" fn bidi(
    this: *mut Sink,
    position: u32,
    length: u32,
    explicit: u8,
    resolved: u8,
) -> Hresult {
    if this.is_null() {
        return POINTER;
    }
    let Some(range) = window(position, length) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: native passes a live sink; all writes are confined to its window.
    unsafe {
        let Ok(mut output) = (*this).output.lock() else {
            return FAILED;
        };
        let Some(cells) = output.levels.get_mut(range) else {
            return INVALID_ARGUMENT;
        };
        cells.fill(Some(Levels { explicit, resolved }));
    }
    OK
}
unsafe extern "system" fn breakpoints(
    _this: *mut Sink,
    _position: u32,
    _length: u32,
    _breakpoints: *const u8,
) -> Hresult {
    // The core's Unicode wrap contract owns legal break opportunities. This
    // adapter requests script/bidi analysis, not native line-break analysis.
    OK
}
unsafe extern "system" fn sink_number_substitution(
    _this: *mut Sink,
    _position: u32,
    _length: u32,
    _substitution: *mut c_void,
) -> Hresult {
    // Our source supplies no substitution objects to retain.
    OK
}
