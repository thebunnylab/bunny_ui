//! Events raised by hand — a test instrument, not an app's API.
//!
//! A shell's event path is most of what a user feels: the handler, the
//! frame pacing, the display beat, the present. None of it runs in a
//! headless test. An example that wants to measure it (`scroll_window
//! --drive`) raises the events itself, through the same door the window
//! system's callbacks use, and reads the present tape afterwards.
//!
//! An event raised here is NOT the window system's: nothing moved the
//! real pointer, and the event queue of the process never backed up the
//! way it does under a real hand. It measures the shell, not the system.

use crate::ffi::{self, AppEvent};

/// One wheel step at a point of the key window, in layout points. A
/// positive delta reveals content above, as the window system's own does.
///
/// Call it on the main thread. Inside an event handler — a task that a
/// wake polls — the event waits for the handler to return, like any event
/// raised from inside one.
pub fn wheel(x: f64, y: f64, dx: f64, dy: f64) {
    ffi::dispatch(AppEvent::Wheel {
        x,
        y,
        dx,
        dy,
        modifiers: bunny_ui::action::Modifiers::NONE,
        phase: bunny_ui::custom::WheelPhase::Changed,
    });
}

/// The pointer at a point of the key window, with nothing held.
pub fn pointer(x: f64, y: f64) {
    ffi::dispatch(AppEvent::MouseMoved {
        x,
        y,
        modifiers: bunny_ui::action::Modifiers::NONE,
    });
}
