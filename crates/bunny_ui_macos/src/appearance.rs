//! Observe application appearance without changing the user's system preferences.
//!
//! Application KVO callbacks enqueue a broadcast for the existing main-loop wake.
//! They never enter application scenes synchronously inside an `AppKit` mutation.
use super::{
    AppEvent, Id, Sel, class, class_addMethod, msg_id, msg_id_arg, msg_observe, msg_void_id,
    ns_string, objc_allocateClassPair, objc_registerClassPair, sel, text_argument_to_string,
    wake_from_any_thread,
};
use bunny_ui::theme::Appearance;
use std::{cell::RefCell, collections::VecDeque, ffi::c_void, sync::Once};

thread_local! {
    static PENDING: RefCell<VecDeque<Option<Appearance>>> = const { RefCell::new(VecDeque::new()) };
}

/// Drain only at the native wake boundary, never during synchronous operations.
pub(super) fn deliver() {
    let pending = PENDING.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
    for appearance in pending {
        super::dispatch_all(AppEvent::AppearanceChanged(appearance));
    }
}

/// The application-effective scheme, including accessibility appearance variants.
pub fn os_appearance() -> Option<Appearance> {
    // AppKit is initialized and accessed only on the shell's main thread.
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        let appearance = msg_id(app, sel("effectiveAppearance"));
        if appearance.is_null() {
            return None;
        }
        let names = msg_id(class("NSMutableArray"), sel("array"));
        // Match accessibility variants explicitly instead of relying on their
        // ancestry mapping through the two ordinary Aqua appearances.
        for name in [
            "NSAppearanceNameAccessibilityHighContrastDarkAqua",
            "NSAppearanceNameAccessibilityHighContrastAqua",
            "NSAppearanceNameDarkAqua",
            "NSAppearanceNameAqua",
        ] {
            msg_void_id(names, sel("addObject:"), ns_string(name));
        }
        let best = msg_id_arg(appearance, sel("bestMatchFromAppearancesWithNames:"), names);
        if best.is_null() {
            return None;
        }
        match text_argument_to_string(best).as_str() {
            "NSAppearanceNameAqua" | "NSAppearanceNameAccessibilityHighContrastAqua" => {
                Some(Appearance::Light)
            }
            "NSAppearanceNameDarkAqua" | "NSAppearanceNameAccessibilityHighContrastDarkAqua" => {
                Some(Appearance::Dark)
            }
            _ => None,
        }
    }
}

/// One observer retained for the process-lifetime native application. Window
/// overrides and teardown cannot stop application-level appearance delivery.
pub(super) fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| unsafe {
        let observer_class =
            objc_allocateClassPair(class("NSObject"), c"BunnyAppearanceObserver".as_ptr(), 0);
        class_addMethod(
            observer_class,
            sel("observeValueForKeyPath:ofObject:change:context:"),
            changed as *const c_void,
            c"v@:@@@^v".as_ptr(),
        );
        objc_registerClassPair(observer_class);
        // Own this one native observer as long as NSApplication itself. Neither
        // Rust TLS teardown nor a window closing can release it under KVO.
        let observer = msg_id(msg_id(observer_class, sel("alloc")), sel("init"));
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_observe(
            app,
            sel("addObserver:forKeyPath:options:context:"),
            observer,
            ns_string("effectiveAppearance"),
            0,
            std::ptr::null_mut(),
        );
    });
}

extern "C" fn changed(
    _this: Id,
    _selector: Sel,
    _key: Id,
    _object: Id,
    _change: Id,
    _context: *mut c_void,
) {
    let appearance = os_appearance();
    // Unlike synthetic input, this observation survives lend_hand (e.g. a new
    // native window). Deferral also avoids re-entering a mutably borrowed scene.
    PENDING.with(|queue| {
        let mut queue = queue.borrow_mut();
        if queue.back() != Some(&appearance) {
            queue.push_back(appearance);
        }
    });
    wake_from_any_thread();
}
