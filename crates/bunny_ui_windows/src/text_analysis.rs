//! Whole-paragraph DirectWrite analysis before visual line shaping.
//!
//! ## Wiring
//! Native source/sink callbacks retain the original UTF-16 and engine locale.
//! Script and bidi output must cover every code unit before it is published.
//! This boundary is evaluated independently of the existing line-only renderer;
//! it is not yet connected to the contextual `TextEngine` entry points.

use super::{Com, Factories, Guid, Hresult, com_ok};
use bunny_ui::text_error::TextError;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, atomic::AtomicU32};

#[path = "text_analysis_callbacks.rs"]
mod callbacks;

#[path = "text_glyphs.rs"]
mod glyphs;

#[path = "text_bidi.rs"]
mod bidi;

#[path = "text_line.rs"]
mod line;

const ENGINE: &str = "DirectWrite";
const OK: Hresult = 0;
const NO_INTERFACE: Hresult = -2_147_467_262; // E_NOINTERFACE
const POINTER: Hresult = -2_147_467_261; // E_POINTER
const FAILED: Hresult = -2_147_467_259; // E_FAIL
const INVALID_ARGUMENT: Hresult = -2_147_024_809; // E_INVALIDARG

const fn preparation(operation: &'static str) -> TextError {
    TextError::Preparation {
        engine: ENGINE,
        operation,
    }
}

// Exact SDK layouts from microsoft/win32metadata dwrite.h at
// c41ef7458f008ba4ccea85ee33163be71b9112bb. Script shapes is a 32-bit enum,
// not a second UINT16; the padded DWRITE_SCRIPT_ANALYSIS is eight bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Script {
    script: u16,
    shapes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Levels {
    explicit: u8,
    resolved: u8,
}

#[repr(C)]
struct FactoryVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_20: [usize; 18],
    create_text_analyzer: unsafe extern "system" fn(*mut Factory, *mut *mut Analyzer) -> Hresult,
}
#[repr(C)]
struct Factory {
    vtbl: *const FactoryVtbl,
}

type Analyze =
    unsafe extern "system" fn(*mut Analyzer, *mut Source, u32, u32, *mut Sink) -> Hresult;

#[repr(C)]
struct AnalyzerVtbl {
    unknown: crate::ffi::UnknownVtbl,
    analyze_script: Analyze, // 3
    analyze_bidi: Analyze,   // 4
}
#[repr(C)]
struct Analyzer {
    vtbl: *const AnalyzerVtbl,
}

#[repr(C)]
struct SourceVtbl {
    unknown: crate::ffi::UnknownVtbl,
    text_at: unsafe extern "system" fn(*mut Source, u32, *mut *const u16, *mut u32) -> Hresult,
    text_before: unsafe extern "system" fn(*mut Source, u32, *mut *const u16, *mut u32) -> Hresult,
    reading_direction: unsafe extern "system" fn(*mut Source) -> u32,
    locale: unsafe extern "system" fn(*mut Source, u32, *mut u32, *mut *const u16) -> Hresult,
    number_substitution:
        unsafe extern "system" fn(*mut Source, u32, *mut u32, *mut *mut c_void) -> Hresult,
}
#[repr(C)]
struct Source {
    vtbl: *const SourceVtbl,
    references: AtomicU32,
    text: Arc<[u16]>,
    locale: Arc<[u16]>,
    length: u32,
}

#[repr(C)]
struct SinkVtbl {
    unknown: crate::ffi::UnknownVtbl,
    script: unsafe extern "system" fn(*mut Sink, u32, u32, *const Script) -> Hresult,
    breakpoints: unsafe extern "system" fn(*mut Sink, u32, u32, *const u8) -> Hresult,
    bidi: unsafe extern "system" fn(*mut Sink, u32, u32, u8, u8) -> Hresult,
    number_substitution: unsafe extern "system" fn(*mut Sink, u32, u32, *mut c_void) -> Hresult,
}
#[derive(Default)]
struct Output {
    scripts: Vec<Option<Script>>,
    levels: Vec<Option<Levels>>,
}
#[repr(C)]
struct Sink {
    vtbl: *const SinkVtbl,
    references: AtomicU32,
    output: Mutex<Output>,
}

struct Prepared {
    source: Com<Source>,
    scripts: Vec<Script>,
    levels: Vec<Levels>,
    classes: Vec<bidi::Class>,
}
impl Prepared {
    fn new(factories: &Factories, text: &str) -> Result<Self, TextError> {
        // UTF-16 never has more units than valid UTF-8 has bytes. Check the
        // native bound before allocating either native input or output arrays.
        u32::try_from(text.len()).map_err(|_| TextError::IndexOverflow { engine: ENGINE })?;
        let classes = text
            .chars()
            .flat_map(|c| std::iter::repeat_n(bidi::Class::of(c), c.len_utf16()))
            .collect();
        let text: Arc<[u16]> = text.encode_utf16().collect::<Vec<_>>().into();
        let source = Source::new(text, factories.locale.clone().into())?;
        let (scripts, levels) = analyze(factories, &source)?;
        Ok(Self {
            source,
            scripts,
            levels,
            classes,
        })
    }

    fn utf16(&self) -> &[u16] {
        // SAFETY: this Prepared owns the source's live COM reference.
        unsafe { &(*self.source.as_ptr()).text }
    }
}

impl Analyzer {
    fn new(factories: &Factories) -> Result<Com<Self>, TextError> {
        // SAFETY: IDWriteFactory slot 21 is CreateTextAnalyzer. All referenced
        // objects outlive these synchronous native calls; output is initialized.
        unsafe {
            let factory = factories.dwrite.as_ptr().cast::<Factory>();
            let mut pointer = std::ptr::null_mut();
            let status = ((*(*factory).vtbl).create_text_analyzer)(factory, &raw mut pointer);
            let owned = Com::from_raw(pointer);
            if !com_ok(status) {
                return Err(preparation("CreateTextAnalyzer"));
            }
            owned.ok_or_else(|| preparation("CreateTextAnalyzer result"))
        }
    }
}

fn analyze(
    factories: &Factories,
    source: &Com<Source>,
) -> Result<(Vec<Script>, Vec<Levels>), TextError> {
    // SAFETY: source is our live, immutable COM object. Its constructor checks
    // length representability and gives every native callback stable storage.
    let input = unsafe { &*source.as_ptr() };
    if input.length == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let sink = Sink::new(input.text.len())?;
    let analyzer = Analyzer::new(factories)?;
    // SAFETY: the native analyzer has the documented IUnknown/vtable prefix.
    let vtbl = unsafe { &*(*analyzer.as_ptr()).vtbl };
    for (call, operation) in [
        (vtbl.analyze_script, "AnalyzeScript"),
        (vtbl.analyze_bidi, "AnalyzeBidi"),
    ] {
        // SAFETY: analyze the original whole paragraph, not a visual slice.
        let status = unsafe {
            call(
                analyzer.as_ptr(),
                source.as_ptr(),
                0,
                input.length,
                sink.as_ptr(),
            )
        };
        if !com_ok(status) {
            return Err(preparation(operation));
        }
    }
    // SAFETY: the native calls have returned; the owning reference still holds
    // the sink. The mutex also makes callback mutation explicit and serialized.
    let output = unsafe { &(*sink.as_ptr()).output };
    let output = {
        let mut held = output
            .lock()
            .map_err(|_| preparation("analysis sink lock"))?;
        std::mem::take(&mut *held)
    };
    output.finish()
}

impl Output {
    fn finish(self) -> Result<(Vec<Script>, Vec<Levels>), TextError> {
        let scripts = self
            .scripts
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| preparation("script coverage"))?;
        let levels = self
            .levels
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| preparation("bidi coverage"))?;
        Ok((scripts, levels))
    }
}

#[cfg(test)]
#[path = "text_analysis_tests.rs"]
mod tests;
