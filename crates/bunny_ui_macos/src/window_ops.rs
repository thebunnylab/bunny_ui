//! Native requests for registered, independently routed scenes.
use super::ObjcBool;
use super::{
    Id, MODAL_WINDOWS, WINDOWS, class, lend_hand, msg_bool, msg_id, msg_id_arg, msg_id_cstr,
    msg_id_u64_u64, msg_void_bool, msg_void_id, msg_void_id_i64, sel,
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

/// Names a live window: what Mission Control, the Window menu and the Dock call it, whether
/// or not its title bar shows the name. A NUL in `title` is dropped.
pub fn set_window_title(window: usize, title: &str) -> Result<(), WindowError> {
    let window = live(window)?;
    let title = std::ffi::CString::new(title.replace('\0', "")).unwrap_or_default();
    lend_hand(|| unsafe {
        let name = msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), title.as_ptr());
        msg_void_id(window, sel("setTitle:"), name);
    });
    Ok(())
}

/// Shows the PNG or TIFF in `image` as the application's icon in the Dock and the app
/// switcher; `false` when AppKit cannot read it.
pub fn set_application_icon(image: &[u8]) -> bool {
    lend_hand(|| unsafe {
        let data = msg_id_u64_u64(
            class("NSData"),
            sel("dataWithBytes:length:"),
            image.as_ptr() as u64,
            image.len() as u64,
        );
        let picture = msg_id_arg(msg_id(class("NSImage"), sel("alloc")), sel("initWithData:"), data);
        if picture.is_null() {
            return false;
        }
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_id(app, sel("setApplicationIconImage:"), picture);
        true
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
            if msg_bool(ancestor, sel("isMiniaturized")).get() {
                msg_void_id(ancestor, sel("deminiaturize:"), std::ptr::null_mut());
            }
            ancestor = msg_id(ancestor, sel("parentWindow"));
        }
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_bool(app, sel("activateIgnoringOtherApps:"), ObjcBool::NO);
        msg_void_id(target, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
        msg_void_id(
            target,
            sel("makeFirstResponder:"),
            msg_id(target, sel("contentView")),
        );
    });
    Ok(())
}
