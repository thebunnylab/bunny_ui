//! Which window the keyboard rests on, told to `bunny_ui::app` once a turn.
//!
//! Both doors already keep the window that holds the keyboard — a toplevel for
//! itself, a dialog or a popover for its owner — and let go of it when that
//! window closes. It is read at the END of a pump turn, not at each `enter`,
//! `leave`, `FocusIn` and `FocusOut`: a keyboard passing from a window to its
//! own dialog leaves and arrives in one read, and the turn ends where it
//! began. Nothing was left, and the app hears nothing.
use super::geometry_ops;

/// `focus` is the native window holding the keyboard, 0 for none. A window the
/// app no longer holds open answers for nobody, so one closed with the
/// keyboard on it is left in the turn that closed it.
pub(crate) fn settle(focus: usize) {
    bunny_ui::app::observe_active_window(geometry_ops::lifetime(focus));
}

#[cfg(test)]
#[path = "active_tests.rs"]
mod tests;
