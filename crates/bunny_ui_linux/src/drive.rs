//! The hand that drives an example from inside: a `--drive` sheet
//! queues the events a person would make and reads what the shell
//! did with them. Hidden from the docs — a test harness, not an API.
//!
//! The queue exists because a sheet runs inside a task, and a task
//! runs inside the pump's own dispatch (`Wake` → `poll_tasks`): a
//! direct `dispatch` from there would borrow the handler twice. So
//! the sheet leaves the events here, and the pump delivers them once
//! the current turn is over, in order, one turn each.

use std::cell::RefCell;
use std::collections::VecDeque;

use crate::ffi::AppEvent;

thread_local! {
    static QUEUE: RefCell<VecDeque<AppEvent>> = const { RefCell::new(VecDeque::new()) };
}

fn push(event: AppEvent) {
    QUEUE.with(|queue| queue.borrow_mut().push_back(event));
}

/// Moves the pointer to `(x, y)` in layout points.
pub fn pointer(x: f64, y: f64) {
    push(AppEvent::MouseMoved { x, y, modifiers: bunny_ui::action::Modifiers::default() });
}

/// A click at `(x, y)`: the pointer arrives, presses, releases.
pub fn click(x: f64, y: f64) {
    pointer(x, y);
    push(AppEvent::MouseDown {
        x,
        y,
        clicks: 1,
        modifiers: bunny_ui::action::Modifiers::default(),
    });
    push(AppEvent::MouseUp { x, y });
}

/// A wheel step at `(x, y)` — the engine's sign: positive `dy` is up.
pub fn wheel(x: f64, y: f64, dx: f64, dy: f64) {
    push(AppEvent::Wheel { x, y, dx, dy });
}

/// Typed text, the way the keyboard road delivers it.
pub fn text(text: &str) {
    push(AppEvent::Text(text.to_string()));
}

/// How many frames the shell has presented since the window opened —
/// every road counts (CPU blit, GL swap, Vulkan present).
pub fn presents() -> u64 {
    if crate::ffi::is_x11() {
        crate::x11::presents()
    } else {
        crate::ffi::presents()
    }
}

/// Which door is open — `"wayland"` or `"x11"`.
pub fn backend() -> &'static str {
    if crate::ffi::is_x11() { "x11" } else { "wayland" }
}

/// Delivers what the sheet queued. Called by both pumps at the end of
/// a turn, outside any dispatch.
pub(crate) fn drain() {
    loop {
        let next = QUEUE.with(|queue| queue.borrow_mut().pop_front());
        let Some(event) = next else { break };
        crate::ffi::dispatch(event);
    }
}

/// A sheet's guard: a pump that hangs must not hang the proof. After
/// `seconds` the process exits 2 from a thread of its own.
pub fn watchdog(seconds: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        eprintln!("drive: the watchdog fired after {seconds}s");
        std::process::exit(2);
    });
}
