//! The app's life outside its window, on this platform: the application
//! delegate (a reopen, a url handed over) and the workspace's sleep and
//! wake — each one answering a door of `bunny_ui::app`. The desktop's
//! notifications are the shared half's, `bunny_ui_apple::notifications`,
//! and the same delegate object answers their center.
//!
//! A bundled app is ONE process by the system's own rule — a second
//! launch and a url both reach the running one through the delegate,
//! and land as the same event the spool delivers everywhere else.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

use bunny_ui::app::{AppEvent, emit};

use crate::ffi::{
    Id, Sel, class, class_addMethod, class_addProtocol, objc_allocateClassPair, objc_getProtocol,
    objc_registerClassPair, sel,
};

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
    fn msg_void_id_sel_id_id(obj: Id, sel: Sel, a: Id, b: Sel, c: Id, d: Id);
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
}

thread_local! {
    /// The ONE delegate — the application's, the workspace's
    /// observer and the notification center's, in one object that
    /// lives as long as the process.
    static DELEGATE: Cell<Id> = const { Cell::new(null_mut()) };
}

/// Installs the delegate and the observers, and the notifier — at
/// boot, before the application runs, so a launch that comes from a
/// notification click or a url finds the doors already open.
pub(crate) fn install() {
    let delegate = delegate();
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_id(app, sel("setDelegate:"), delegate);
        let workspace = msg_id(class("NSWorkspace"), sel("sharedWorkspace"));
        let center = msg_id(workspace, sel("notificationCenter"));
        for (name, door) in [
            ("NSWorkspaceWillSleepNotification", "bunnyWillSleep:"),
            ("NSWorkspaceDidWakeNotification", "bunnyDidWake:"),
        ] {
            msg_void_id_sel_id_id(
                center,
                sel("addObserver:selector:name:object:"),
                delegate,
                sel(door),
                ns(name),
                null_mut(),
            );
        }
        // a bundled app's notification center learns its delegate
        // now: a click on a notification can be the very thing that
        // launched the process, and the response arrives early
        bunny_ui_apple::notifications::install_delegate(delegate);
    }
    bunny_ui::app::install_notifier(bunny_ui_apple::notifications::notify);
}

/// `applicationShouldHandleReopen:hasVisibleWindows:` — the Dock icon,
/// or a second launch of a bundled app: a reopen with no arguments.
/// YES lets AppKit do its own part (unminiaturize, show).
extern "C" fn bridge_reopen(_this: Id, _sel: Sel, _app: Id, _visible: i8) -> i8 {
    emit(AppEvent::Reopened { arguments: Vec::new() });
    1
}

/// `application:openURLs:` — a url the system handed this app: each
/// one an argument of a reopen, the same event the spool delivers.
extern "C" fn bridge_open_urls(_this: Id, _sel: Sel, _app: Id, urls: Id) {
    let mut arguments = Vec::new();
    unsafe {
        let count = msg_i64(urls, sel("count")).max(0) as u64;
        for index in 0..count {
            let url = msg_id_u64(urls, sel("objectAtIndex:"), index);
            let text = to_string(msg_id(url, sel("absoluteString")));
            if !text.is_empty() {
                arguments.push(text);
            }
        }
    }
    if !arguments.is_empty() {
        emit(AppEvent::Reopened { arguments });
    }
}

extern "C" fn bridge_will_sleep(_this: Id, _sel: Sel, _notification: Id) {
    emit(AppEvent::WillSleep);
}

extern "C" fn bridge_did_wake(_this: Id, _sel: Sel, _notification: Id) {
    emit(AppEvent::DidWake);
}

/// The one delegate instance, built on first use.
fn delegate() -> Id {
    DELEGATE.with(|slot| {
        let existing = slot.get();
        if !existing.is_null() {
            return existing;
        }
        let instance = unsafe {
            let name = CString::new("BunnyAppDelegate").expect("class name");
            let bridge = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            let methods: [(&str, *const c_void, &str); 4] = [
                (
                    "applicationShouldHandleReopen:hasVisibleWindows:",
                    bridge_reopen as *const c_void,
                    "B@:@B",
                ),
                ("application:openURLs:", bridge_open_urls as *const c_void, "v@:@@"),
                ("bunnyWillSleep:", bridge_will_sleep as *const c_void, "v@:@"),
                ("bunnyDidWake:", bridge_did_wake as *const c_void, "v@:@"),
            ];
            for (selector, imp, types) in methods {
                let types = CString::new(types).expect("type encoding");
                class_addMethod(bridge, sel(selector), imp, types.as_ptr());
            }
            let protocol = CString::new("NSApplicationDelegate").expect("protocol name");
            let protocol = objc_getProtocol(protocol.as_ptr());
            if !protocol.is_null() {
                class_addProtocol(bridge, protocol);
            }
            // the notification center's doors, on the same object
            bunny_ui_apple::notifications::add_delegate_methods(bridge);
            objc_registerClassPair(bridge);
            msg_id(msg_id(bridge, sel("alloc")), sel("init"))
        };
        slot.set(instance);
        instance
    })
}

/// A borrowed NSString for the message being sent (autoreleased).
unsafe fn ns(text: &str) -> Id {
    let text = CString::new(text).unwrap_or_default();
    unsafe { msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr()) }
}

/// The NSString's bytes, copied out.
unsafe fn to_string(ns: Id) -> String {
    if ns.is_null() {
        return String::new();
    }
    unsafe {
        let utf8 = msg_id(ns, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return String::new();
        }
        CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}
