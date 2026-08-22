//! The platform's own file dialogs, through the house FFI.
//!
//! A picker is not a view. It is a window the SYSTEM owns, with the
//! reader's sidebar, their recent places, their search and their idea
//! of what a folder looks like — an app that draws its own is a worse
//! one, and it is also the wrong one, because the sandbox grants
//! access through the reader's choice in the platform's panel and
//! nowhere else.
//!
//! **These calls block, and they belong on the MAIN thread.** The
//! panel runs the platform's own modal loop while it is up: the
//! application stays alive and keeps drawing, and this call returns
//! when the reader answers. It is the one place in this shell where
//! blocking is the correct behaviour — there is nothing to do until
//! the answer arrives.
//!
//! ```ignore
//! // in the handler for ⌘O
//! if let Some(folder) = dialog::open_folder("Open a project") {
//!     roots.update(|roots| roots.repoint(folder));
//! }
//! ```

use std::ffi::{CString, c_char, c_void};
use std::path::PathBuf;

use crate::ffi::{Id, Sel, class, sel};

// The panel bridge — msgSend casts in the house pattern, local to the
// messages this module sends.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
    #[link_name = "objc_msgSend"]
    fn msg_u64(obj: Id, sel: Sel) -> u64;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

/// `NSModalResponseOK` — the reader chose. Everything else (cancel,
/// the window closed) is the same answer here: nothing was chosen.
const MODAL_OK: i64 = 1;

/// The reader picks ONE folder. `None` = they cancelled, which is an
/// answer and not a failure.
///
/// `prompt` is the line above the file list — the sentence that says
/// what the folder is FOR ("Open a project", "Choose where to export").
/// An empty one leaves the panel with its own wording.
pub fn open_folder(prompt: &str) -> Option<PathBuf> {
    run(prompt, true)
}

/// The same panel, for ONE file. `None` = cancelled.
pub fn open_file(prompt: &str) -> Option<PathBuf> {
    run(prompt, false)
}

fn run(prompt: &str, directories: bool) -> Option<PathBuf> {
    // the panel makes a great many temporaries, and this call may be
    // the only thing on the stack: its own pool, drained on the way out
    let pool = unsafe { objc_autoreleasePoolPush() };
    let picked = unsafe { run_panel(prompt, directories) };
    unsafe { objc_autoreleasePoolPop(pool) };
    picked
}

unsafe fn run_panel(prompt: &str, directories: bool) -> Option<PathBuf> {
    unsafe {
        let panel = msg_id(class("NSOpenPanel"), sel("openPanel"));
        if panel.is_null() {
            return None;
        }
        // exactly one of the two, and one at a time: an app that wanted
        // several would want a different answer type than this one
        msg_void_bool(panel, sel("setCanChooseDirectories:"), directories as i8);
        msg_void_bool(panel, sel("setCanChooseFiles:"), (!directories) as i8);
        msg_void_bool(panel, sel("setAllowsMultipleSelection:"), 0);
        // the reader can always REACH a folder that is not there yet,
        // which is what makes this the picker a project opener wants
        msg_void_bool(panel, sel("setCanCreateDirectories:"), directories as i8);
        if !prompt.is_empty()
            && let Ok(text) = CString::new(prompt)
        {
            let message =
                msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr());
            msg_void_id(panel, sel("setMessage:"), message);
        }
        // the platform's own modal loop: the app keeps drawing behind
        // the panel and this returns when the reader answers
        if msg_i64(panel, sel("runModal")) != MODAL_OK {
            return None;
        }
        let urls = msg_id(panel, sel("URLs"));
        if urls.is_null() || msg_u64(urls, sel("count")) == 0 {
            return None;
        }
        let url = msg_id_u64(urls, sel("objectAtIndex:"), 0);
        path_of(url)
    }
}

/// The file-system path behind an `NSURL`. A URL that names something
/// other than a file has none, and answers `None`.
unsafe fn path_of(url: Id) -> Option<PathBuf> {
    unsafe {
        if url.is_null() {
            return None;
        }
        let string = msg_id(url, sel("path"));
        if string.is_null() {
            return None;
        }
        let utf8 = msg_id(string, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return None;
        }
        let path = std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "objc", kind = "dylib")]
    #[allow(clashing_extern_declarations)]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
    }

    /// Every message this module sends, asked of the REAL AppKit on
    /// this machine. A hand-written border has no compiler checking
    /// the spelling of a selector, and a misspelled one is silent
    /// until a reader presses the key — so the spelling is the test.
    #[test]
    fn the_panel_answers_every_message_the_module_sends() {
        unsafe {
            let panel = class("NSOpenPanel");
            assert!(!panel.is_null(), "AppKit is linked and NSOpenPanel is there");
            assert_eq!(
                msg_bool_sel(panel, sel("respondsToSelector:"), sel("openPanel")),
                1,
                "the class makes one",
            );
            for name in [
                "setCanChooseDirectories:",
                "setCanChooseFiles:",
                "setAllowsMultipleSelection:",
                "setCanCreateDirectories:",
                "setMessage:",
                "runModal",
                "URLs",
            ] {
                assert_eq!(
                    msg_bool_sel(panel, sel("instancesRespondToSelector:"), sel(name)),
                    1,
                    "a panel answers `{name}`",
                );
            }
            let url = class("NSURL");
            assert_eq!(
                msg_bool_sel(url, sel("instancesRespondToSelector:"), sel("path")),
                1,
                "a URL answers `path`",
            );
        }
    }
}
