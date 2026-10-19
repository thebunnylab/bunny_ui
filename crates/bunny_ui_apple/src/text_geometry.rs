//! Native caret edges from the same complete `CTLine` that draws the text.

use super::{CFRelease, CTLineRef, CoreTextEngine, FontSpec, make_line};
use bunny_ui::text_geometry::{LineGeometry, TextCell};
use std::ffi::c_void;

#[derive(Debug)]
pub(super) struct NativeEdge {
    pub(super) offset: f64,
    pub(super) index: usize,
    pub(super) leading: bool,
}

#[repr(C)]
struct EdgeBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*mut Self, f64, isize, bool, *mut bool),
    descriptor: *const BlockDescriptor,
    edges: *mut c_void,
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

static EDGE_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<EdgeBlock>(),
};

unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTLineEnumerateCaretOffsets(line: CTLineRef, block: *const EdgeBlock);
    fn CTLineGetOffsetForStringIndex(line: CTLineRef, index: isize, secondary: *mut f64) -> f64;
}

extern "C" fn collect_edge(
    block: *mut EdgeBlock,
    offset: f64,
    index: isize,
    leading: bool,
    _stop: *mut bool,
) {
    if let Ok(index) = usize::try_from(index) {
        // SAFETY: enumeration invokes this scoped block synchronously, in
        // visual order. Its vector lives for the complete native call.
        unsafe {
            (*(*block).edges.cast::<Vec<NativeEdge>>()).push(NativeEdge {
                offset,
                index,
                leading,
            });
        }
    }
}

/// Enumerate native logical edges in visual order without retaining a callback.
pub(super) fn caret_edges(line: CTLineRef) -> Vec<NativeEdge> {
    let mut edges = Vec::new();
    let block = EdgeBlock {
        isa: (&raw const _NSConcreteStackBlock).cast(),
        flags: 0,
        reserved: 0,
        invoke: collect_edge,
        descriptor: &raw const EDGE_DESCRIPTOR,
        edges: (&raw mut edges).cast(),
    };
    // SAFETY: CTLine is borrowed from the owned line; its synchronous C-block
    // ABI uses double, CFIndex and C bool as declared by the installed SDK.
    unsafe {
        CTLineEnumerateCaretOffsets(line, &raw const block);
    }
    edges
}

pub(super) struct OwnedLine(pub(super) CTLineRef);
impl Drop for OwnedLine {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns exactly one Create-rule reference.
        unsafe {
            CFRelease(self.0);
        }
    }
}

/// Cells are shaped once as a complete line. Native enumeration supplies both
/// logical sides at directional boundaries; the offset API fills explicit
/// scalar positions inside native clusters that are not pointer stops.
pub(super) fn line_geometry(
    engine: &CoreTextEngine,
    text: &str,
    font: &FontSpec,
) -> Option<LineGeometry> {
    if text.is_empty() {
        return LineGeometry::new(text, Vec::new()).ok();
    }
    let raw = unsafe { make_line(text, engine.font(font), font) };
    if raw.is_null() {
        return None;
    }
    let line = OwnedLine(raw);
    let units = text.encode_utf16().count();
    let mut leading = vec![None; units];
    let mut trailing = vec![None; units];
    for edge in caret_edges(line.0) {
        let table = if edge.leading {
            &mut leading
        } else {
            &mut trailing
        };
        if let Some(slot) = table.get_mut(edge.index) {
            *slot = Some(edge.offset);
        }
    }
    let offset = |index: usize| -> Option<f64> {
        let index = isize::try_from(index).ok()?;
        // SAFETY: a live complete CTLine and a scalar-aligned UTF-16 index.
        Some(unsafe { CTLineGetOffsetForStringIndex(line.0, index, std::ptr::null_mut()) })
    };
    let mut cells = Vec::new();
    let mut at = 0;
    for (byte, character) in text.char_indices() {
        let end = at + character.len_utf16();
        let before = leading[at].or_else(|| offset(at))?;
        let after = trailing[end - 1].or_else(|| offset(end))?;
        match TextCell::new(byte..byte + character.len_utf8(), before, after) {
            Ok(cell) => cells.push(cell),
            Err(error) => {
                eprintln!("bunny_ui_apple: {error}");
                return None;
            }
        }
        at = end;
    }
    match LineGeometry::new(text, cells) {
        Ok(geometry) => Some(geometry),
        Err(error) => {
            eprintln!("bunny_ui_apple: {error}");
            None
        }
    }
}
