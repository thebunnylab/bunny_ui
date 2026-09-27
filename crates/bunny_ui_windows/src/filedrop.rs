//! Files dragged in from the system — Explorer, the desktop, a mail
//! attachment — offered to the scene the way the mac's
//! `draggingEntered:` / `performDragOperation:` offer them.
//!
//! The scene already knows what to do with them: `Runtime::external_drag`
//! previews against the same clipped drop targets an internal drag uses,
//! `external_drop` delivers to the one that accepts, and
//! `external_drag_exited` clears the preview. This module is the platform
//! half — an OLE `IDropTarget` per window, the one Windows road that says
//! "over here, would you take these?" while the drag is still moving
//! (`WM_DROPFILES` only ever says "dropped", after the fact, and cannot
//! refuse).
//!
//! ## Production gotchas
//!
//! - **OLE, not COM.** `RegisterDragDrop` fails on a thread that only
//!   called `CoInitializeEx`; it needs `OleInitialize`, which joins the
//!   same apartment and then some. [`register`] joins once per thread.
//! - **The data is lent at Enter and at Drop only.** `DragOver` carries a
//!   point and no data, so the paths read at Enter are held for the moves
//!   that follow.
//! - **A drag that is not files is refused silently.** A browser's link or
//!   a word from an editor carries no `CF_HDROP`; the scene is not asked,
//!   so no drop target lights up for something it could never take.

use std::cell::{Cell, RefCell};
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use crate::ffi::{self, Guid, Hresult, Hwnd, UnknownVtbl};

/// Where a system file drag is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileDrag {
    /// Moving over the window: would the target under the pointer take them?
    Preview,
    /// Released: deliver to the target under the pointer.
    Drop,
    /// Left the window, or the drag was cancelled.
    Exit,
}

/// The scene's answer, routed to the window the drag is over: `(x, y)` in
/// scene points, the files, the phase — `true` when a target takes them.
type Gate = Box<dyn Fn(f64, f64, Vec<PathBuf>, FileDrag) -> bool>;

thread_local! {
    static GATE: RefCell<Option<Gate>> = const { RefCell::new(None) };
}

/// Installs the one gate — the app routes it to the addressed window.
pub(crate) fn set_gate(gate: Gate) {
    GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

fn ask(hwnd: Hwnd, x: f64, y: f64, files: Vec<PathBuf>, phase: FileDrag) -> bool {
    ffi::addressed(hwnd, || {
        GATE.with(|slot| slot.borrow().as_ref().is_some_and(|gate| gate(x, y, files, phase)))
    })
}

// MARK: - The platform's shapes

#[repr(C)]
#[derive(Clone, Copy)]
struct PointL {
    x: i32,
    y: i32,
}

/// `FORMATETC`.
#[repr(C)]
struct FormatEtc {
    format: u16,
    target_device: *mut c_void,
    aspect: u32,
    index: i32,
    medium: u32,
}

/// `STGMEDIUM`: the medium's kind, the handle (a union — ours is always an
/// `HGLOBAL`), and who releases it.
#[repr(C)]
struct StgMedium {
    medium: u32,
    handle: *mut c_void,
    release_by: *mut c_void,
}

/// `IDataObject` — only `GetData` is called.
#[repr(C)]
struct DataObjectVtbl {
    unknown: UnknownVtbl,
    get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> Hresult,
}

/// `IDropTarget`: IUnknown, then `DragEnter`, `DragOver`, `DragLeave`, `Drop`.
#[repr(C)]
struct DropTargetVtbl {
    unknown: UnknownVtbl,
    drag_enter: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, PointL, *mut u32) -> Hresult,
    drag_over: unsafe extern "system" fn(*mut c_void, u32, PointL, *mut u32) -> Hresult,
    drag_leave: unsafe extern "system" fn(*mut c_void) -> Hresult,
    drop: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, PointL, *mut u32) -> Hresult,
}

/// `IID_IUnknown` {00000000-0000-0000-C000-000000000046}.
const IID_IUNKNOWN: Guid = Guid {
    d1: 0x0000_0000,
    d2: 0x0000,
    d3: 0x0000,
    d4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};

/// `IID_IDropTarget` {00000122-0000-0000-C000-000000000046}.
const IID_IDROPTARGET: Guid = Guid {
    d1: 0x0000_0122,
    d2: 0x0000,
    d3: 0x0000,
    d4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};

/// `CF_HDROP`, `DVASPECT_CONTENT`, `TYMED_HGLOBAL`.
const CF_HDROP: u16 = 15;
const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: u32 = 1;

/// `DROPEFFECT_NONE` / `DROPEFFECT_COPY`.
const DROPEFFECT_NONE: u32 = 0;
const DROPEFFECT_COPY: u32 = 1;

/// `E_NOINTERFACE`.
const NO_INTERFACE: Hresult = 0x8000_4002_u32 as i32;

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OleInitialize(reserved: *mut c_void) -> Hresult;
    fn RegisterDragDrop(hwnd: Hwnd, target: *mut c_void) -> Hresult;
    fn RevokeDragDrop(hwnd: Hwnd) -> Hresult;
    fn ReleaseStgMedium(medium: *mut StgMedium);
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn DragQueryFileW(drop: *mut c_void, index: u32, file: *mut u16, length: u32) -> u32;
}

// MARK: - The target

/// One window's drop target. OLE holds the reference `RegisterDragDrop`
/// takes and gives it back at `RevokeDragDrop`; the window holds nothing.
#[repr(C)]
struct Target {
    vtbl: *const DropTargetVtbl,
    /// STA-only: OLE calls on the thread that registered.
    refs: Cell<u32>,
    hwnd: Hwnd,
    /// The files read at `DragEnter`, for the moves that carry none.
    files: RefCell<Vec<PathBuf>>,
}

static TARGET_VTBL: DropTargetVtbl = DropTargetVtbl {
    unknown: UnknownVtbl {
        query_interface: target_query,
        add_ref: target_add_ref,
        release: target_release,
    },
    drag_enter: target_enter,
    drag_over: target_over,
    drag_leave: target_leave,
    drop: target_drop,
};

unsafe extern "system" fn target_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if *riid == IID_IUNKNOWN || *riid == IID_IDROPTARGET {
            target_add_ref(this);
            *out = this;
            0
        } else {
            *out = std::ptr::null_mut();
            NO_INTERFACE
        }
    }
}

unsafe extern "system" fn target_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let target = &*(this as *const Target);
        let refs = target.refs.get() + 1;
        target.refs.set(refs);
        refs
    }
}

unsafe extern "system" fn target_release(this: *mut c_void) -> u32 {
    unsafe {
        let refs = {
            let target = &*(this as *const Target);
            let refs = target.refs.get() - 1;
            target.refs.set(refs);
            refs
        };
        if refs == 0 {
            drop(Box::from_raw(this as *mut Target));
        }
        refs
    }
}

/// What the drag may do, told back to the source: a copy when the scene
/// takes the files and the source allows copying, nothing otherwise.
const fn effect_for(accepted: bool, allowed: u32) -> u32 {
    if accepted && allowed & DROPEFFECT_COPY != 0 { DROPEFFECT_COPY } else { DROPEFFECT_NONE }
}

/// Asks the scene about the held files at `point` and writes the answer
/// into `effect`. A drag of no files asks nobody.
unsafe fn answer(target: &Target, point: PointL, phase: FileDrag, effect: *mut u32) {
    let files = target.files.borrow().clone();
    let accepted = if files.is_empty() {
        false
    } else {
        let (x, y) = ffi::screen_to_layout(target.hwnd, point.x, point.y);
        ask(target.hwnd, x, y, files, phase)
    };
    if !effect.is_null() {
        unsafe {
            *effect = effect_for(accepted, *effect);
        }
    }
}

unsafe extern "system" fn target_enter(
    this: *mut c_void,
    data: *mut c_void,
    _keys: u32,
    point: PointL,
    effect: *mut u32,
) -> Hresult {
    unsafe {
        let target = &*(this as *const Target);
        *target.files.borrow_mut() = files_of(data);
        answer(target, point, FileDrag::Preview, effect);
    }
    0
}

unsafe extern "system" fn target_over(
    this: *mut c_void,
    _keys: u32,
    point: PointL,
    effect: *mut u32,
) -> Hresult {
    unsafe {
        answer(&*(this as *const Target), point, FileDrag::Preview, effect);
    }
    0
}

unsafe extern "system" fn target_leave(this: *mut c_void) -> Hresult {
    let target = unsafe { &*(this as *const Target) };
    let held = std::mem::take(&mut *target.files.borrow_mut());
    if !held.is_empty() {
        ask(target.hwnd, 0.0, 0.0, Vec::new(), FileDrag::Exit);
    }
    0
}

unsafe extern "system" fn target_drop(
    this: *mut c_void,
    data: *mut c_void,
    _keys: u32,
    point: PointL,
    effect: *mut u32,
) -> Hresult {
    unsafe {
        let target = &*(this as *const Target);
        // the drop lends the data again: it is the truth, not what Enter saw
        *target.files.borrow_mut() = files_of(data);
        answer(target, point, FileDrag::Drop, effect);
        target.files.borrow_mut().clear();
    }
    0
}

/// The files a data object carries as `CF_HDROP` — none for a drag of
/// anything else.
unsafe fn files_of(data: *mut c_void) -> Vec<PathBuf> {
    if data.is_null() {
        return Vec::new();
    }
    let format = FormatEtc {
        format: CF_HDROP,
        target_device: std::ptr::null_mut(),
        aspect: DVASPECT_CONTENT,
        index: -1,
        medium: TYMED_HGLOBAL,
    };
    let mut medium = StgMedium {
        medium: 0,
        handle: std::ptr::null_mut(),
        release_by: std::ptr::null_mut(),
    };
    unsafe {
        let vtbl = *(data as *const *const DataObjectVtbl);
        if !ffi::com_ok(((*vtbl).get_data)(data, &format, &mut medium)) {
            return Vec::new();
        }
        let files = if medium.medium == TYMED_HGLOBAL { paths_of_hdrop(medium.handle) } else { Vec::new() };
        ReleaseStgMedium(&mut medium);
        files
    }
}

/// The paths an `HDROP` names, in its order.
unsafe fn paths_of_hdrop(drop: *mut c_void) -> Vec<PathBuf> {
    if drop.is_null() {
        return Vec::new();
    }
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
        (0..count)
            .filter_map(|index| {
                let length = DragQueryFileW(drop, index, std::ptr::null_mut(), 0);
                if length == 0 {
                    return None;
                }
                let mut wide = vec![0u16; length as usize + 1];
                let written = DragQueryFileW(drop, index, wide.as_mut_ptr(), length + 1);
                wide.truncate(written as usize);
                Some(PathBuf::from(OsString::from_wide(&wide)))
            })
            .collect()
    }
}

// MARK: - A window's registration

/// Makes `hwnd` a place files can be dropped. A thread that cannot join
/// OLE (it chose the multithreaded apartment first) says so once and takes
/// no drops, rather than failing where nobody reads it.
pub(crate) fn register(hwnd: Hwnd) -> bool {
    thread_local! {
        static JOINED: Cell<Option<bool>> = const { Cell::new(None) };
    }
    let joined = JOINED.with(|joined| {
        *joined.get().get_or_insert_with(|| {
            let ok = ffi::com_ok(unsafe { OleInitialize(std::ptr::null_mut()) });
            if !ok {
                eprintln!("bunny_ui windows: OLE refused this thread — files cannot be dropped on its windows");
            }
            ok
        })
    });
    if !joined {
        return false;
    }
    let target = Box::into_raw(Box::new(Target {
        vtbl: &raw const TARGET_VTBL,
        refs: Cell::new(1),
        hwnd,
        files: RefCell::new(Vec::new()),
    })) as *mut c_void;
    unsafe {
        let registered = ffi::com_ok(RegisterDragDrop(hwnd, target));
        // OLE took its own reference if it kept the target; ours goes
        target_release(target);
        registered
    }
}

/// Undoes [`register`] as the window goes: OLE lets go of the target.
pub(crate) fn revoke(hwnd: Hwnd) {
    unsafe {
        RevokeDragDrop(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "user32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn DestroyWindow(hwnd: Hwnd) -> i32;
    }

    // the clipboard's own declarations' shapes (`ffi.rs`): one symbol, one
    // signature — a handle is an `isize` here as there
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
        fn GlobalLock(handle: isize) -> *mut c_void;
        fn GlobalUnlock(handle: isize) -> i32;
        fn GlobalFree(handle: isize) -> isize;
    }

    /// An `HDROP` as Explorer builds one: a `DROPFILES` header, then each
    /// path as UTF-16, NUL-ended, and one more NUL to end the list.
    fn hdrop(paths: &[&str]) -> isize {
        const GMEM_MOVEABLE: u32 = 0x0002;
        const GMEM_ZEROINIT: u32 = 0x0040;
        // DROPFILES: pFiles (u32), pt (2×i32), fNC (i32), fWide (i32)
        const HEADER: usize = 20;
        let mut wide: Vec<u16> = Vec::new();
        for path in paths {
            wide.extend(path.encode_utf16());
            wide.push(0);
        }
        wide.push(0);
        let bytes = HEADER + wide.len() * 2;
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes);
            assert!(memory != 0);
            let base = GlobalLock(memory) as *mut u8;
            (base as *mut u32).write_unaligned(HEADER as u32);
            (base.add(16) as *mut i32).write_unaligned(1);
            std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, base.add(HEADER), wide.len() * 2);
            GlobalUnlock(memory);
            memory
        }
    }

    #[test]
    fn an_hdrop_reads_back_as_its_paths_in_order() {
        let paths = [r"C:\Users\reader\notes.md", r"D:\work\ação — relatório.xlsx"];
        let drop = hdrop(&paths);
        let read = unsafe { paths_of_hdrop(drop as *mut c_void) };
        unsafe {
            GlobalFree(drop);
        }
        let expected: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        assert_eq!(read, expected, "every path, whole, in order — non-ASCII included");
        assert!(unsafe { paths_of_hdrop(std::ptr::null_mut()) }.is_empty());
    }

    #[test]
    fn the_source_hears_copy_only_when_the_scene_takes_the_files() {
        const MOVE: u32 = 2;
        const LINK: u32 = 4;
        assert_eq!(effect_for(true, DROPEFFECT_COPY | MOVE | LINK), DROPEFFECT_COPY);
        assert_eq!(effect_for(false, DROPEFFECT_COPY | MOVE), DROPEFFECT_NONE);
        // a source that forbids copying is never told it happened
        assert_eq!(effect_for(true, MOVE), DROPEFFECT_NONE);
    }

    #[test]
    fn a_window_takes_drops_and_its_target_answers_through_the_gate() {
        let window = ffi::create_window("bunny drop", 200.0, 150.0, false, true, true);
        let hwnd = window.raw_window() as Hwnd;
        // `create_window` made it a drop target: OLE refuses a second
        // registration (and says so, never a panic)
        assert!(!register(hwnd), "the window was not registered at birth");
        revoke(hwnd);
        assert!(register(hwnd), "RegisterDragDrop on a live window");

        // the target itself, driven as OLE drives it
        let seen: std::rc::Rc<RefCell<Vec<(FileDrag, usize)>>> = std::rc::Rc::default();
        set_gate(Box::new({
            let seen = std::rc::Rc::clone(&seen);
            move |_, _, files, phase| {
                seen.borrow_mut().push((phase, files.len()));
                phase != FileDrag::Exit
            }
        }));
        let target = Box::into_raw(Box::new(Target {
            vtbl: &raw const TARGET_VTBL,
            refs: Cell::new(1),
            hwnd,
            files: RefCell::new(vec![PathBuf::from(r"C:\a.txt")]),
        }));
        let this = target as *mut c_void;
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            target_over(this, 0, PointL { x: 10, y: 10 }, &mut effect);
        }
        assert_eq!(effect, DROPEFFECT_COPY, "the scene took the held file");
        unsafe {
            target_leave(this);
        }
        // a drop of no data object: nothing to take, nobody asked
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            target_drop(this, std::ptr::null_mut(), 0, PointL { x: 10, y: 10 }, &mut effect);
        }
        assert_eq!(effect, DROPEFFECT_NONE);
        assert_eq!(
            *seen.borrow(),
            [(FileDrag::Preview, 1), (FileDrag::Exit, 0)],
            "a preview with the held file, then the exit — and no drop of nothing"
        );
        unsafe {
            target_release(this);
        }
        GATE.with(|slot| slot.borrow_mut().take());
        // WM_DESTROY revokes the registration on the way out
        unsafe {
            DestroyWindow(hwnd);
        }
    }
}
