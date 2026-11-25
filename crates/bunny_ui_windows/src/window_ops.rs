//! Native owner, activation, name and full-screen requests for separate top-level scenes.
use super::{
    GetMonitorInfoW, GetWindow, GetWindowRect, Hwnd, IsWindowVisible, MONITOR_DEFAULT_TO_NEAREST,
    MonitorFromWindow, MonitorInfo, Rect, SetFocus, SetWindowPos, SetWindowTextW, ShowWindow,
    is_top_level,
};
use bunny_ui::window::{WindowError, WindowParentKind};
use std::cell::RefCell;
use std::collections::HashMap;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetWindowLongPtrW(window: Hwnd, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(window: Hwnd, index: i32) -> isize;
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

const GWL_STYLE: i32 = -16;
const WS_OVERLAPPEDWINDOW: isize = 0x00CF_0000;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_NOOWNERZORDER: u32 = 0x0200;

thread_local! {
    /// The style and frame each full-screen window had before, to go back to.
    static WINDOWED: RefCell<HashMap<Hwnd, (isize, Rect)>> = RefCell::new(HashMap::new());
}

/// Renames the window: its caption, the task bar's button and Alt-Tab.
pub fn set_window_title(window: usize, title: &str) -> Result<(), WindowError> {
    let window = live(window)?;
    let wide: Vec<u16> = title
        .encode_utf16()
        .filter(|unit| *unit != 0)
        .chain([0])
        .collect();
    if unsafe { SetWindowTextW(window, wide.as_ptr()) } == 0 {
        return Err(WindowError::NativeFailure {
            operation: "SetWindowTextW",
            code: i64::from(unsafe { GetLastError() }),
        });
    }
    Ok(())
}

/// Takes the window over its whole monitor without a frame, or back to the style and place it
/// had.
pub fn set_window_fullscreen(window: usize, fullscreen: bool) -> Result<(), WindowError> {
    let window = live(window)?;
    let saved = WINDOWED.with(|windowed| windowed.borrow().get(&window).copied());
    match (fullscreen, saved) {
        (true, None) => unsafe {
            let style = GetWindowLongPtrW(window, GWL_STYLE);
            let mut frame = Rect::default();
            GetWindowRect(window, &mut frame);
            let mut info = MonitorInfo {
                size: size_of::<MonitorInfo>() as u32,
                monitor: Rect::default(),
                work: Rect::default(),
                flags: 0,
            };
            let monitor = MonitorFromWindow(window, MONITOR_DEFAULT_TO_NEAREST);
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return Err(WindowError::NativeFailure {
                    operation: "GetMonitorInfoW",
                    code: i64::from(GetLastError()),
                });
            }
            WINDOWED.with(|windowed| windowed.borrow_mut().insert(window, (style, frame)));
            SetWindowLongPtrW(window, GWL_STYLE, style & !WS_OVERLAPPEDWINDOW);
            let screen = info.monitor;
            SetWindowPos(
                window,
                0,
                screen.left,
                screen.top,
                screen.right - screen.left,
                screen.bottom - screen.top,
                SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
            );
        },
        (false, Some((style, frame))) => unsafe {
            WINDOWED.with(|windowed| windowed.borrow_mut().remove(&window));
            SetWindowLongPtrW(window, GWL_STYLE, style);
            SetWindowPos(
                window,
                0,
                frame.left,
                frame.top,
                frame.right - frame.left,
                frame.bottom - frame.top,
                SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
            );
        },
        _ => {}
    }
    Ok(())
}
