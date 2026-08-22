//! The platform's own file dialogs, through the house FFI.
//!
//! A picker is not a view. It is a window the SYSTEM owns, with the
//! reader's Quick Access, their recent places, their search and their
//! idea of what a folder looks like — an app that draws its own is a
//! worse one.
//!
//! **These calls block, and they belong on the thread that owns the
//! window.** The panel runs the platform's own modal loop while it is
//! up: the application stays alive and keeps drawing, and this call
//! returns when the reader answers — see the macOS twin for the shape.

use std::ffi::c_void;
use std::path::PathBuf;

use crate::ffi::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, Com, Guid, Hresult, Hwnd, UnknownVtbl, com_init,
    com_ok, main_window, wide,
};

// MARK: - Interfaces (vtables in header order, slot indexes cited)

/// `IFileOpenDialog` — slots verified against shobjidl_core.h. The
/// chain is `IUnknown` → `IModalWindow` → `IFileDialog` →
/// `IFileOpenDialog`, and the derived slots continue the base's
/// numbering, so the padding below is the part this module never asks
/// for.
#[repr(C)]
struct FileOpenDialog {
    vtbl: *const FileOpenDialogVtbl,
}

#[repr(C)]
struct FileOpenDialogVtbl {
    unknown: UnknownVtbl, // 0..=2
    // 3 IModalWindow::Show — the owner window, or null for none
    show: unsafe extern "system" fn(*mut FileOpenDialog, Hwnd) -> Hresult,
    // 4 SetFileTypes, 5 SetFileTypeIndex, 6 GetFileTypeIndex,
    // 7 Advise, 8 Unadvise
    _pad_4_8: [usize; 5],
    // 9 SetOptions
    set_options: unsafe extern "system" fn(*mut FileOpenDialog, u32) -> Hresult,
    // 10 GetOptions
    get_options: unsafe extern "system" fn(*mut FileOpenDialog, *mut u32) -> Hresult,
    // 11 SetDefaultFolder, 12 SetFolder, 13 GetFolder,
    // 14 GetCurrentSelection, 15 SetFileName, 16 GetFileName
    _pad_11_16: [usize; 6],
    // 17 SetTitle
    set_title: unsafe extern "system" fn(*mut FileOpenDialog, *const u16) -> Hresult,
    // 18 SetOkButtonLabel, 19 SetFileNameLabel
    _pad_18_19: [usize; 2],
    // 20 GetResult
    get_result:
        unsafe extern "system" fn(*mut FileOpenDialog, *mut *mut ShellItem) -> Hresult,
    // 21 AddPlace … 26 SetFilter, then IFileOpenDialog's own two
    _pad_21_28: [usize; 8],
}

/// `IShellItem` — slots verified against shobjidl_core.h.
#[repr(C)]
struct ShellItem {
    vtbl: *const ShellItemVtbl,
}

#[repr(C)]
struct ShellItemVtbl {
    unknown: UnknownVtbl,   // 0..=2
    _pad_3_4: [usize; 2],   // 3 BindToHandler, 4 GetParent
    // 5 GetDisplayName — the string comes back on the task allocator
    get_display_name:
        unsafe extern "system" fn(*mut ShellItem, u32, *mut *mut u16) -> Hresult,
    _pad_6_7: [usize; 2], // 6 GetAttributes, 7 Compare
}

/// `CLSID_FileOpenDialog` — {DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7}.
const CLSID_FILE_OPEN_DIALOG: Guid = Guid {
    d1: 0xDC1C_5A9C,
    d2: 0xE88A,
    d3: 0x4DDE,
    d4: [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
};

/// `IID_IFileOpenDialog` — {D57C7288-D4AD-4768-BE02-9D969532D960}.
const IID_FILE_OPEN_DIALOG: Guid = Guid {
    d1: 0xD57C_7288,
    d2: 0xD4AD,
    d3: 0x4768,
    d4: [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
};

/// `FOS_PICKFOLDERS` — the panel chooses a container, not a file.
const FOS_PICK_FOLDERS: u32 = 0x0000_0020;
/// `FOS_FORCEFILESYSTEM` — only what has a real path comes back, so
/// the answer is always a `PathBuf` and never a virtual place.
const FOS_FORCE_FILE_SYSTEM: u32 = 0x0000_0040;
/// `FOS_PATHMUSTEXIST` + `FOS_FILEMUSTEXIST`.
const FOS_PATH_MUST_EXIST: u32 = 0x0000_0800;
const FOS_FILE_MUST_EXIST: u32 = 0x0000_1000;
/// `FOS_NOCHANGEDIR` — the panel must not move the process's working
/// directory out from under the app.
const FOS_NO_CHANGE_DIR: u32 = 0x0000_0008;

/// `SIGDN_FILESYSPATH`.
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoTaskMemFree(block: *mut c_void);
}

/// The reader picks ONE folder. `None` = they cancelled, which is an
/// answer and not a failure.
///
/// `prompt` becomes the panel's title — the sentence that says what
/// the folder is FOR ("Open a project"). An empty one leaves the panel
/// with its own wording.
pub fn open_folder(prompt: &str) -> Option<PathBuf> {
    run(prompt, true)
}

/// The same panel, for ONE file. `None` = cancelled.
pub fn open_file(prompt: &str) -> Option<PathBuf> {
    run(prompt, false)
}

fn run(prompt: &str, directories: bool) -> Option<PathBuf> {
    com_init();
    let mut raw: *mut c_void = std::ptr::null_mut();
    let created = unsafe {
        CoCreateInstance(
            &CLSID_FILE_OPEN_DIALOG,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_FILE_OPEN_DIALOG,
            &mut raw,
        )
    };
    if !com_ok(created) {
        return None;
    }
    let dialog = Com::from_raw(raw as *mut FileOpenDialog)?;
    let dialog = dialog.as_ptr();
    unsafe {
        let vtbl = (*dialog).vtbl;
        // the panel's own defaults stand except for what this asks
        // for: read them, add, write back
        let mut options = 0u32;
        if !com_ok(((*vtbl).get_options)(dialog, &mut options)) {
            return None;
        }
        options |= FOS_FORCE_FILE_SYSTEM | FOS_NO_CHANGE_DIR | FOS_PATH_MUST_EXIST;
        options |= match directories {
            true => FOS_PICK_FOLDERS,
            false => FOS_FILE_MUST_EXIST,
        };
        if !com_ok(((*vtbl).set_options)(dialog, options)) {
            return None;
        }
        if !prompt.is_empty() {
            let title = wide(prompt);
            ((*vtbl).set_title)(dialog, title.as_ptr());
        }
        // the platform's own modal loop. A cancel answers a failing
        // HRESULT, which is the same "nothing was chosen" as any
        // other refusal here
        if !com_ok(((*vtbl).show)(dialog, main_window())) {
            return None;
        }
        let mut item: *mut ShellItem = std::ptr::null_mut();
        if !com_ok(((*vtbl).get_result)(dialog, &mut item)) {
            return None;
        }
        let item = Com::from_raw(item)?;
        path_of(item.as_ptr())
    }
}

/// The file-system path behind an `IShellItem`. The string comes back
/// on the task allocator and is this side's to free.
unsafe fn path_of(item: *mut ShellItem) -> Option<PathBuf> {
    unsafe {
        let mut wide: *mut u16 = std::ptr::null_mut();
        let named =
            ((*(*item).vtbl).get_display_name)(item, SIGDN_FILESYSPATH, &mut wide);
        if !com_ok(named) || wide.is_null() {
            return None;
        }
        let mut length = 0usize;
        while *wide.add(length) != 0 {
            length += 1;
        }
        let path = String::from_utf16_lossy(std::slice::from_raw_parts(wide, length));
        CoTaskMemFree(wide as *mut c_void);
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-written vtable has no compiler checking that a slot
    /// index is the one the header names, and a wrong index calls the
    /// WRONG METHOD — silently, until it corrupts something. The total
    /// slot count is the arithmetic that can be checked here: it holds
    /// only if every padding run between the named slots is right.
    #[test]
    fn the_vtables_hold_exactly_the_slots_their_headers_declare() {
        let slot = std::mem::size_of::<usize>();
        // IUnknown 3 + IModalWindow 1 + IFileDialog 23 + IFileOpenDialog 2
        assert_eq!(std::mem::size_of::<FileOpenDialogVtbl>(), 29 * slot);
        // IUnknown 3 + BindToHandler, GetParent, GetDisplayName,
        // GetAttributes, Compare
        assert_eq!(std::mem::size_of::<ShellItemVtbl>(), 8 * slot);
    }
}
