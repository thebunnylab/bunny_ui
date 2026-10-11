//! AppKit close-request and retained-window protocol witness.
#[cfg(target_os = "macos")]
use bunny_ui_macos as shell;
#[cfg(target_os = "macos")]
#[path = "../../bunny_ui/tests/support/window_lifecycle.rs"]
mod contract;
#[cfg(target_os = "macos")]
fn main() {
    if std::env::var("BUNNY_LIFECYCLE_CHILD").is_ok_and(|mode| mode == "children") {
        contract::child_surfaces(visible_children);
    } else {
        contract::run(native_close);
        if std::env::var_os("BUNNY_LIFECYCLE_CHILD").is_none() {
            contract::run_child("children");
        }
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn native_close(_app: &shell::App, _id: shell::WindowId, title: &str) {
    use bunny_ui_apple::ffi::{Id, Sel, class, sel};
    use std::ffi::{CStr, c_char};
    #[allow(
        clashing_extern_declarations,
        reason = "Each Objective-C selector uses its concrete ABI"
    )]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(value: Id, selector: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn count(value: Id, selector: Sel) -> usize;
        #[link_name = "objc_msgSend"]
        fn at(value: Id, selector: Sel, index: usize) -> Id;
        #[link_name = "objc_msgSend"]
        fn chars(value: Id, selector: Sel) -> *const c_char;
        #[link_name = "objc_msgSend"]
        fn send(value: Id, selector: Sel, sender: Id);
    }
    unsafe {
        let app = object(class("NSApplication"), sel("sharedApplication"));
        let windows = object(app, sel("windows"));
        let window = (0..count(windows, sel("count")))
            .map(|i| at(windows, sel("objectAtIndex:"), i))
            .find(|window| {
                let name = object(*window, sel("title"));
                CStr::from_ptr(chars(name, sel("UTF8String"))).to_bytes() == title.as_bytes()
            })
            .expect("owned native window");
        send(window, sel("performClose:"), std::ptr::null_mut());
    }
}

#[cfg(target_os = "macos")]
fn visible_children(title: &str) -> usize {
    use bunny_ui_apple::ffi::{Id, Sel, class, sel};
    use std::ffi::{CStr, c_char};
    #[allow(
        clashing_extern_declarations,
        reason = "Each Objective-C selector uses its concrete ABI"
    )]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(value: Id, selector: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn count(value: Id, selector: Sel) -> usize;
        #[link_name = "objc_msgSend"]
        fn at(value: Id, selector: Sel, index: usize) -> Id;
        #[link_name = "objc_msgSend"]
        fn chars(value: Id, selector: Sel) -> *const c_char;
        #[link_name = "objc_msgSend"]
        fn boolean(value: Id, selector: Sel) -> i8;
    }
    unsafe fn descendants(window: Id) -> usize {
        unsafe {
            let children = object(window, sel("childWindows"));
            (0..count(children, sel("count")))
                .map(|i| {
                    let child = at(children, sel("objectAtIndex:"), i);
                    usize::from(boolean(child, sel("isVisible")) != 0) + descendants(child)
                })
                .sum()
        }
    }
    unsafe {
        let app = object(class("NSApplication"), sel("sharedApplication"));
        let windows = object(app, sel("windows"));
        let window = (0..count(windows, sel("count")))
            .map(|i| at(windows, sel("objectAtIndex:"), i))
            .find(|window| {
                let name = object(*window, sel("title"));
                CStr::from_ptr(chars(name, sel("UTF8String"))).to_bytes() == title.as_bytes()
            })
            .expect("owned native parent");
        descendants(window)
    }
}
