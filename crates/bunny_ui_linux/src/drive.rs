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

/// One move of the hand: an event for the handler, or a click that
/// walks the door's own press road (the crown answers first there —
/// a bar, a band, a control — and only then the scene).
enum Hand {
    Event(AppEvent),
    Click(f64, f64),
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Hand>> = const { RefCell::new(VecDeque::new()) };
}

fn push(event: AppEvent) {
    QUEUE.with(|queue| queue.borrow_mut().push_back(Hand::Event(event)));
}

/// Moves the pointer to `(x, y)` in layout points.
pub fn pointer(x: f64, y: f64) {
    push(AppEvent::MouseMoved { x, y, modifiers: bunny_ui::action::Modifiers::default() });
}

/// A click at `(x, y)`: the pointer arrives, presses, releases — through
/// the door's press road, so a window control or a drag region answers
/// as it would to a real hand.
pub fn click(x: f64, y: f64) {
    QUEUE.with(|queue| queue.borrow_mut().push_back(Hand::Click(x, y)));
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
    crate::trace::presents()
}

/// The first window's size in layout points, as granted: a tiling
/// compositor (niri, sway, Hyprland) answers the asked size with a
/// tile of its own, so a sheet that aims from the bottom or the right
/// edge measures here instead of trusting its own request.
pub fn window_size() -> (f64, f64) {
    crate::ffi::first_window_size()
}

/// Which door is open — `"wayland"` or `"x11"`.
pub fn backend() -> &'static str {
    if crate::ffi::is_x11() { "x11" } else { "wayland" }
}

/// Who draws the frame: `"server"`, `"client"` or `"unknown"` (no
/// `xdg-decoration` on this compositor — the house bar stands in).
pub fn decoration() -> &'static str {
    match crate::ffi::decoration(crate::ffi::first_window_address()) {
        crate::ffi::Decoration::ServerSide => "server",
        crate::ffi::Decoration::ClientSide => "client",
        crate::ffi::Decoration::Unknown => "unknown",
    }
}

/// A property of the window as 32-bit words, by atom name — what the
/// x11 door wrote, read back through the server. Empty on wayland.
pub fn x11_property(name: &str) -> Vec<u32> {
    if crate::ffi::is_x11() {
        crate::x11::read_property_u32(name)
    } else {
        Vec::new()
    }
}

/// Delivers what the sheet queued. Called by both pumps at the end of
/// a turn, outside any dispatch.
pub(crate) fn drain() {
    loop {
        let next = QUEUE.with(|queue| queue.borrow_mut().pop_front());
        match next {
            Some(Hand::Event(event)) => crate::ffi::dispatch(event),
            Some(Hand::Click(x, y)) => crate::ffi::drive_click(x, y),
            None => break,
        }
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
