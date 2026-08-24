//! The webview tenant: WKWebView behind the native host.
//!
//! The OS already ships a browser engine; this module mounts it in
//! the hole the layout keeps (`docs/webview.md`). The engine draws,
//! scrolls and reads input itself — the shell only creates the view,
//! points it at a url, and moves the box.

use std::ffi::{CString, c_char};

use crate::ffi::{CGPoint, CGRect, CGSize, Id, Sel, class, sel};

// WebKit rides along — the classes resolve by name at runtime, and
// the link is what loads them.
#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

// The msgSend casts in the house pattern, local to the messages this
// module sends.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_init_config(obj: Id, sel: Sel, frame: CGRect, config: Id) -> Id;
}

/// Creates the engine's view, already navigating to `url`. The
/// reference comes back with ONE retain — the host's sweep releases
/// it when the box leaves the scene.
pub(crate) fn create(url: &str) -> Id {
    unsafe {
        let config =
            msg_id(msg_id(class("WKWebViewConfiguration"), sel("alloc")), sel("init"));
        let view = msg_id(class("WKWebView"), sel("alloc"));
        let view = msg_init_config(
            view,
            sel("initWithFrame:configuration:"),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            },
            config,
        );
        // the view copied what it needed from the configuration
        msg_void(config, sel("release"));
        // the engine's own inspector, where the OS offers the switch
        // (13.3+) — a webview here is a dev's window into a page, and
        // a devtool that cannot open is a quiet page with no name
        if msg_bool_sel(view, sel("respondsToSelector:"), sel("setInspectable:")) != 0 {
            msg_void_bool(view, sel("setInspectable:"), 1);
        }
        navigate(view, url);
        view
    }
}

/// Points the engine at `url` — the load is the engine's own affair,
/// asynchronous and cancellable by the next call.
pub(crate) fn navigate(view: Id, url: &str) {
    let Ok(url) = CString::new(url) else {
        // a NUL inside a url is not a url; nothing to load
        return;
    };
    unsafe {
        let string =
            msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), url.as_ptr());
        let url = msg_id_id(class("NSURL"), sel("URLWithString:"), string);
        if url.is_null() {
            // NSURL said no — an unparseable url loads nothing rather
            // than crashing the request builder
            return;
        }
        let request = msg_id_id(class("NSURLRequest"), sel("requestWithURL:"), url);
        let _ = msg_id_id(view, sel("loadRequest:"), request);
    }
}
