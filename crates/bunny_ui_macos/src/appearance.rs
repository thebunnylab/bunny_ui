//! Observe application appearance without changing the user's system preferences.
//!
//! Native view callbacks enqueue a broadcast for the existing main-loop wake.
//! They never enter application scenes synchronously inside an `AppKit` mutation.
use super::{
    AppEvent, Id, ObjcSuper, Sel, class, class_addMethod, msg_id, msg_id_arg, msg_super_void,
    msg_void_id, ns_string, sel, text_argument_to_string, wake_from_any_thread,
};
use bunny_ui::theme::Appearance;
use std::{cell::RefCell, collections::VecDeque, ffi::c_void};

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
        msg_void_id(names, sel("addObject:"), ns_string("NSAppearanceNameAqua"));
        msg_void_id(
            names,
            sel("addObject:"),
            ns_string("NSAppearanceNameDarkAqua"),
        );
        let best = msg_id_arg(appearance, sel("bestMatchFromAppearancesWithNames:"), names);
        if best.is_null() {
            return None;
        }
        match text_argument_to_string(best).as_str() {
            "NSAppearanceNameAqua" => Some(Appearance::Light),
            "NSAppearanceNameDarkAqua" => Some(Appearance::Dark),
            _ => None,
        }
    }
}

pub(super) unsafe fn register(view: Id) {
    // The registered selector takes no arguments and returns void on both ABIs.
    unsafe {
        class_addMethod(
            view,
            sel("viewDidChangeEffectiveAppearance"),
            changed as *const c_void,
            c"v@:".as_ptr(),
        );
    }
}

extern "C" fn changed(this: Id, selector: Sel) {
    // Preserve NSView's appearance bookkeeping before capturing the app snapshot.
    unsafe {
        let parent = ObjcSuper {
            receiver: this,
            class: class("NSView"),
        };
        msg_super_void(&raw const parent, selector);
    }
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
