//! The app's life outside its window, on this platform: the application
//! delegate (a reopen, a url handed over), the workspace's sleep and
//! wake — each one answering a door of `bunny_ui::app` — and the
//! system's word that its languages moved, which is the shell's own
//! event. The desktop's notifications are the shared half's,
//! `bunny_ui_apple::notifications`, and the same delegate object
//! answers their center.
//!
//! A bundled app is ONE process by the system's own rule — a second
//! launch and a url both reach the running one through the delegate,
//! and land as the same event the spool delivers everywhere else.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

use bunny_ui::app::{AppEvent, emit};

use crate::objc_bool::ObjcBool;
use crate::ffi::{
    Id, Sel, class, class_addMethod, class_addProtocol, objc_allocateClassPair, objc_getProtocol,
    objc_registerClassPair, sel,
};

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSUserActivityTypeBrowsingWeb: Id;
}

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
        // the system's languages: the DEFAULT center posts the change
        // (the workspace's does not), and the windows hear it as an
        // event of the shell's own, not one of `bunny_ui::app`'s — no
        // app has a say in it, every window re-reads the list
        let defaults = msg_id(class("NSNotificationCenter"), sel("defaultCenter"));
        msg_void_id_sel_id_id(
            defaults,
            sel("addObserver:selector:name:object:"),
            delegate,
            sel("bunnyLocaleChanged:"),
            ns("NSCurrentLocaleDidChangeNotification"),
            null_mut(),
        );
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
extern "C" fn bridge_reopen(_this: Id, _sel: Sel, _app: Id, _visible: ObjcBool) -> ObjcBool {
    emit(AppEvent::Reopened { arguments: Vec::new() });
    ObjcBool::YES
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

/// Universal links arrive as browsing activities, not `openURLs:`. Forward the
/// web URL through the same memory-only app event; the application must validate
/// its trusted origin and parameters before acting. Other Handoff activities
/// remain AppKit's responsibility. No restorable responder is created here, so
/// the optional restoration handler is neither retained nor called.
extern "C" fn bridge_continue_activity(
    _this: Id,
    _sel: Sel,
    _app: Id,
    activity: Id,
    _restoration: Id,
) -> i8 {
    if activity.is_null() {
        return 0;
    }
    let argument = unsafe {
        if to_string(msg_id(activity, sel("activityType")))
            != to_string(NSUserActivityTypeBrowsingWeb)
        {
            return 0;
        }
        let url = msg_id(activity, sel("webpageURL"));
        if url.is_null()
            || !matches!(
                to_string(msg_id(url, sel("scheme"))).as_str(),
                "http" | "https"
            )
        {
            return 0;
        }
        to_string(msg_id(url, sel("absoluteString")))
    };
    if argument.is_empty() {
        return 0;
    }
    i8::from(
        emit(AppEvent::Reopened {
            arguments: vec![argument],
        }) > 0,
    )
}

// AppKit's BOOL is C++ bool on Apple Silicon and signed char on Intel.
// Both callbacks return 0/1 through i8; their Objective-C metadata must still
// describe the platform's actual BOOL, including for protocol introspection.
const CONTINUE_ACTIVITY_TYPES: &str = if cfg!(target_arch = "x86_64") {
    "c@:@@@?"
} else {
    "B@:@@@?"
};

extern "C" fn bridge_will_sleep(_this: Id, _sel: Sel, _notification: Id) {
    emit(AppEvent::WillSleep);
}

extern "C" fn bridge_did_wake(_this: Id, _sel: Sel, _notification: Id) {
    emit(AppEvent::DidWake);
}

/// `NSCurrentLocaleDidChangeNotification` — the person changed the
/// system's languages or region: a beat every window shares, and the
/// app's menu bar is worded again after them.
extern "C" fn bridge_locale_changed(_this: Id, _sel: Sel, _notification: Id) {
    crate::ffi::dispatch_all(crate::ffi::AppEvent::Locale);
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
            let methods: [(&str, *const c_void, &str); 6] = [
                (
                    "applicationShouldHandleReopen:hasVisibleWindows:",
                    bridge_reopen as *const c_void,
                    if cfg!(target_arch = "aarch64") { "B@:@B" } else { "c@:@c" },
                ),
                (
                    "application:openURLs:",
                    bridge_open_urls as *const c_void,
                    "v@:@@",
                ),
                (
                    "application:continueUserActivity:restorationHandler:",
                    bridge_continue_activity as *const c_void,
                    CONTINUE_ACTIVITY_TYPES,
                ),
                (
                    "bunnyWillSleep:",
                    bridge_will_sleep as *const c_void,
                    "v@:@",
                ),
                ("bunnyDidWake:", bridge_did_wake as *const c_void, "v@:@"),
                (
                    "bunnyLocaleChanged:",
                    bridge_locale_changed as *const c_void,
                    "v@:@",
                ),
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
    unsafe {
        msg_id_cstr(
            class("NSString"),
            sel("stringWithUTF8String:"),
            text.as_ptr(),
        )
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct MethodDescription {
        name: Sel,
        types: *const c_char,
    }

    #[allow(clashing_extern_declarations)]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn msg_id_id(obj: Id, selector: Sel, value: Id) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_bool_ids(obj: Id, selector: Sel, app: Id, activity: Id, handler: Id) -> i8;
        fn protocol_getMethodDescription(
            protocol: Id,
            selector: Sel,
            required: bool,
            instance: bool,
        ) -> MethodDescription;
        fn class_getInstanceMethod(class: Id, selector: Sel) -> Id;
        fn method_getTypeEncoding(method: Id) -> *const c_char;
    }

    struct Pool(*mut c_void);
    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe { crate::ffi::objc_autoreleasePoolPop(self.0) };
        }
    }

    struct Activity(Id);
    impl Drop for Activity {
        fn drop(&mut self) {
            unsafe { msg_id(self.0, sel("release")) };
        }
    }

    /// Exercise the registered Objective-C method with real Foundation objects,
    /// including an untouched URL fragment. This is delegate delivery evidence,
    /// not evidence that a signed app/domain association has been installed.
    #[test]
    fn browsing_activity_delivers_only_web_urls_through_the_registered_delegate() {
        let _pool = Pool(unsafe { crate::ffi::objc_autoreleasePoolPush() });
        let (sender, receiver) = bunny_ui::task::channel();
        bunny_ui::app::subscribe(sender);
        let door = unsafe { sel("application:continueUserActivity:restorationHandler:") };
        let delegate = delegate();
        unsafe {
            let name = CString::new("NSApplicationDelegate").expect("protocol name");
            let protocol = objc_getProtocol(name.as_ptr());
            assert!(!protocol.is_null());
            let native = protocol_getMethodDescription(protocol, door, false, true);
            assert!(!native.types.is_null());
            let native_types = CStr::from_ptr(native.types).to_bytes();
            assert_eq!(
                native_types.first(),
                CONTINUE_ACTIVITY_TYPES.as_bytes().first()
            );
            let method = class_getInstanceMethod(msg_id(delegate, sel("class")), door);
            assert!(!method.is_null());
            let registered = method_getTypeEncoding(method);
            assert!(!registered.is_null());
            assert_eq!(
                CStr::from_ptr(registered).to_bytes(),
                CONTINUE_ACTIVITY_TYPES.as_bytes()
            );
        }
        assert_eq!(
            unsafe { msg_bool_ids(delegate, door, null_mut(), null_mut(), null_mut()) },
            0
        );
        for (kind, url, accepted) in [
            (
                "NSUserActivityTypeBrowsingWeb",
                Some("https://example.test/join-commercial/123#token=synthetic_only"),
                true,
            ),
            (
                "NSUserActivityTypeBrowsingWeb",
                Some("http://example.test/page?x=1#fragment"),
                true,
            ),
            (
                "example.other-activity",
                Some("https://example.test/page"),
                false,
            ),
            ("NSUserActivityTypeBrowsingWeb", None, false),
        ] {
            let activity = Activity(unsafe {
                msg_id_id(
                    msg_id(class("NSUserActivity"), sel("alloc")),
                    sel("initWithActivityType:"),
                    ns(kind),
                )
            });
            assert!(!activity.0.is_null());
            if let Some(url) = url {
                unsafe {
                    let url = msg_id_id(class("NSURL"), sel("URLWithString:"), ns(url));
                    msg_void_id(activity.0, sel("setWebpageURL:"), url);
                }
            }
            assert_eq!(
                unsafe { msg_bool_ids(delegate, door, null_mut(), activity.0, null_mut()) },
                i8::from(accepted)
            );
            if accepted {
                let AppEvent::Reopened { arguments } = receiver.try_recv().expect("one URL event")
                else {
                    panic!("wrong application event");
                };
                assert_eq!(arguments, vec![url.expect("accepted URL")]);
            }
            assert!(
                receiver.try_recv().is_none(),
                "no unrelated or duplicate event"
            );
        }
    }
}
