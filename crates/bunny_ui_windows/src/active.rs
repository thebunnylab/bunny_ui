//! Which scene stands in front of the person, told to `bunny_ui::app` at the
//! messages that speak of activation.
//!
//! Leaving is the message's word alone. Arriving takes a second opinion: a
//! window shown by a process in the background is activated in its own
//! thread, is sent `WM_ACTIVATE`, and is still behind another application. So
//! an arrival names the scene the FOREGROUND window belongs to, read at the
//! message, and says nothing while that is none of the app's.
//!
//! Three messages are read, because none says it all:
//!
//! - `WM_ACTIVATE` — one scene to another, and the app left with a window of
//!   this procedure active;
//! - `WM_ACTIVATEAPP` — the app left or came back while a window that is NOT
//!   this procedure's held the activation: a message box, a file dialog;
//! - `WM_NCACTIVATE`, going active — a window activated from the background
//!   is told so when it does reach the front, where `WM_ACTIVATE` may not
//!   repeat itself. Going inactive it says nothing: the caption also goes
//!   inactive under the scene's own dialog.
//!
//! All of it runs in the window procedure, before any dispatch: an event that
//! arrives while a frame holds the handler is posted as a repaint, and the
//! activation would be lost with it.
use super::{
    Hwnd, WA_INACTIVE, WM_ACTIVATE, WM_ACTIVATEAPP, WM_NCACTIVATE, geometry_ops, scene_owner,
};

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetForegroundWindow() -> Hwnd;
}

/// Which way an activation message points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    /// Something of the app's was activated: the foreground says what.
    Arrived,
    /// The activation went to a window that is none of the scene's own.
    Left,
}

/// Reads the three messages. `WM_ACTIVATE`'s high word only flags a minimized
/// window; its low word is `WA_INACTIVE`, `WA_ACTIVE` or `WA_CLICKACTIVE`.
const fn turn(msg: u32, wparam: usize) -> Option<Turn> {
    match msg {
        WM_ACTIVATE if (wparam & 0xFFFF) == WA_INACTIVE => Some(Turn::Left),
        WM_ACTIVATE => Some(Turn::Arrived),
        WM_ACTIVATEAPP if wparam == 0 => Some(Turn::Left),
        WM_ACTIVATEAPP => Some(Turn::Arrived),
        WM_NCACTIVATE if wparam != 0 => Some(Turn::Arrived),
        _ => None,
    }
}

/// One activation message, already past the dialog rule: a scene's own dialog
/// taking the activation from it never comes here as a `WM_ACTIVATE`.
pub(super) fn observe(msg: u32, wparam: usize) {
    match turn(msg, wparam) {
        Some(Turn::Arrived) => {
            let front = scene_owner(unsafe { GetForegroundWindow() });
            if let Some(scene) = geometry_ops::lifetime(front) {
                bunny_ui::app::observe_active_window(Some(scene));
            }
        }
        // one thread, one active window: the scene that was in front is the
        // one being left, and another scene of the app arrives after it —
        // Windows deactivates the old window before it activates the new
        Some(Turn::Left) => bunny_ui::app::observe_active_window(None),
        None => {}
    }
}

#[cfg(test)]
#[path = "active_tests.rs"]
mod tests;
