//! Native requests for registered, independently routed scenes.
use super::{
    Id, MODAL_WINDOWS, WINDOWS, class, lend_hand, msg_bool, msg_id, msg_void_bool, msg_void_id,
    msg_void_id_i64, sel,
};
use bunny_ui::window::{WindowError, WindowParentKind};

fn live(window: usize) -> Result<Id, WindowError> {
    WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .any(|(id, _, _)| *id == window)
            .then_some(window as Id)
            .ok_or(WindowError::StaleWindow)
    })
}

/// Requests the native relationship after validating both live endpoints.
pub fn set_window_parent(
    window: usize,
    parent: Option<usize>,
) -> Result<WindowParentKind, WindowError> {
    let window = live(window)?;
    let parent = parent.map(live).transpose()?;
    lend_hand(|| unsafe {
        let old = msg_id(window, sel("parentWindow"));
        if old == parent.unwrap_or(std::ptr::null_mut()) {
            return;
        }
        if !old.is_null() {
            msg_void_id(old, sel("removeChildWindow:"), window);
        }
        if let Some(parent) = parent {
            msg_void_id_i64(parent, sel("addChildWindow:ordered:"), window, 1);
        }
    });
    Ok(WindowParentKind::AppKitChild)
}

/// Requests activation under platform policy, preserving modal eligibility.
pub fn focus_window(window: usize) -> Result<(), WindowError> {
    let live = live(window)?;
    let target = MODAL_WINDOWS
        .with(|state| state.borrow().scopes.top(window))
        .map_or(live, |dialog| dialog as Id);
    lend_hand(|| unsafe {
        let mut ancestor = live;
        while !ancestor.is_null() {
            if msg_bool(ancestor, sel("isMiniaturized")) != 0 {
                msg_void_id(ancestor, sel("deminiaturize:"), std::ptr::null_mut());
            }
            ancestor = msg_id(ancestor, sel("parentWindow"));
        }
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_bool(app, sel("activateIgnoringOtherApps:"), 0);
        msg_void_id(target, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
        msg_void_id(
            target,
            sel("makeFirstResponder:"),
            msg_id(target, sel("contentView")),
        );
    });
    Ok(())
}
