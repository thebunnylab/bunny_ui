//! Which window is the active one, read after `AppKit` has finished moving the key.
//!
//! `AppKit` moves the key in steps, and says each one synchronously: the old
//! window resigns before the new one becomes key, and between the two the
//! application has no key window at all. Opening a window's own dialog, ending
//! its sheet, hiding it and showing another are all that same pair. A report
//! made inside either notification would tell the app its window was left and
//! found again within one call. The notifications therefore only knock, and
//! the answer is read once, at the main loop's next wake.
//!
//! The observer hears every window of the process, not only this shell's: a
//! sheet or a system panel holding the key when the app steps back is the only
//! window that says so. Like the appearance observer, it survives `lend_hand`.
use super::{
    Id, Sel, class, class_addMethod, msg_bool, msg_id, ns_string, objc_allocateClassPair,
    objc_registerClassPair, owning_window, sel, wake_from_any_thread,
};
use bunny_ui::window::WindowId;
use std::{cell::Cell, ffi::c_void, sync::Once};

#[allow(
    clashing_extern_declarations,
    reason = "Objective-C selectors have concrete result and argument ABIs"
)]
#[link(name = "objc")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn observe(center: Id, selector: Sel, observer: Id, door: Sel, name: Id, object: Id);
}

thread_local! {
    /// The key or the application's own standing moved since the last look.
    static KNOCKED: Cell<bool> = const { Cell::new(false) };
}

/// Reports where the key came to rest, at the native wake boundary. A wake
/// nothing knocked for asks `AppKit` nothing.
pub(super) fn settle() {
    if KNOCKED.with(|knocked| knocked.replace(false)) {
        bunny_ui::app::observe_active_window(holder());
    }
}

/// The window of this shell that is active now: the application is the active
/// one and its key window is that window, a dialog it owns, or a sheet on it.
fn holder() -> Option<WindowId> {
    // AppKit is initialized and accessed only on the shell's main thread.
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        if !msg_bool(app, sel("isActive")).get() {
            return None;
        }
        let key = msg_id(app, sel("keyWindow"));
        if key.is_null() {
            return None;
        }
        // A sheet is no child window: it names the window it is attached to
        // itself, and its `parentWindow` is nil.
        let host = msg_id(key, sel("sheetParent"));
        super::geometry_ops::lifetime(owning_window(if host.is_null() { key } else { host }))
    }
}

/// One observer retained for the process-lifetime native application.
pub(super) fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| unsafe {
        let observer_class =
            objc_allocateClassPair(class("NSObject"), c"BunnyActiveObserver".as_ptr(), 0);
        class_addMethod(
            observer_class,
            sel("bunnyActiveMoved:"),
            moved as *const c_void,
            c"v@:@".as_ptr(),
        );
        objc_registerClassPair(observer_class);
        let observer = msg_id(msg_id(observer_class, sel("alloc")), sel("init"));
        let center = msg_id(class("NSNotificationCenter"), sel("defaultCenter"));
        for name in [
            "NSWindowDidBecomeKeyNotification",
            "NSWindowDidResignKeyNotification",
            "NSApplicationDidBecomeActiveNotification",
            "NSApplicationDidResignActiveNotification",
        ] {
            observe(
                center,
                sel("addObserver:selector:name:object:"),
                observer,
                sel("bunnyActiveMoved:"),
                ns_string(name),
                std::ptr::null_mut(),
            );
        }
        // A window made key while it was being built became so before this
        // observer existed: the first wake reads where things stand.
        knock();
    });
}

fn knock() {
    KNOCKED.with(|knocked| knocked.set(true));
    wake_from_any_thread();
}

extern "C" fn moved(_this: Id, _selector: Sel, _notification: Id) {
    knock();
}
