//! Native owner and activation requests for separate top-level scenes.
use super::{GetWindow, Hwnd, IsWindowVisible, SetFocus, ShowWindow, is_top_level};
use bunny_ui::window::{WindowError, WindowParentKind};

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetWindowLongPtrW(window: Hwnd, index: i32, value: isize) -> isize;
    fn SetForegroundWindow(window: Hwnd) -> i32;
    fn GetLastActivePopup(window: Hwnd) -> Hwnd;
    fn IsIconic(window: Hwnd) -> i32;
    fn IsWindowEnabled(window: Hwnd) -> i32;
}
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(error: u32);
    fn GetLastError() -> u32;
}

fn live(window: usize) -> Result<Hwnd, WindowError> {
    let hwnd = Hwnd::from_ne_bytes(window.to_ne_bytes());
    is_top_level(hwnd)
        .then_some(hwnd)
        .ok_or(WindowError::StaleWindow)
}

/// Requests the native relationship after validating both live endpoints.
pub fn set_window_parent(
    window: usize,
    parent: Option<usize>,
) -> Result<WindowParentKind, WindowError> {
    let window = live(window)?;
    let parent = parent.map(live).transpose()?.unwrap_or(0);
    unsafe {
        SetLastError(0);
        if SetWindowLongPtrW(window, -8, parent) == 0 {
            let code = GetLastError();
            if code != 0 {
                return Err(WindowError::NativeFailure {
                    operation: "SetWindowLongPtrW(owner)",
                    code: i64::from(code),
                });
            }
        }
    }
    Ok(WindowParentKind::Win32Owner)
}

/// Requests activation under platform policy, preserving modal eligibility.
pub fn focus_window(window: usize) -> Result<(), WindowError> {
    let window = live(window)?;
    unsafe {
        // Owned windows are hidden with their minimized owner. Restore the
        // ownership chain before asking the OS to activate this scene.
        let mut ancestor = window;
        while ancestor != 0 {
            if IsIconic(ancestor) != 0 {
                ShowWindow(ancestor, 9);
            } // SW_RESTORE
            ancestor = GetWindow(ancestor, 4); // GW_OWNER
        }
        let popup = GetLastActivePopup(window);
        let target = if popup != 0 && IsWindowVisible(popup) != 0 && IsWindowEnabled(popup) != 0 {
            popup
        } else {
            window
        };
        if IsWindowEnabled(target) == 0 || SetForegroundWindow(target) == 0 {
            return Err(WindowError::FocusDenied);
        }
        SetFocus(target);
    }
    Ok(())
}
