//! The display's refresh, for an app that draws on a clock of its own.
//!
//! A browser engine runs its animation frames, its `requestAnimationFrame`
//! callbacks and its video presentation on another thread. With a timer of
//! its own those frames drift against the display: one lands twice in a
//! refresh, the next misses one. The shell's display link knows when every
//! refresh happens. An app asks for the next one with [`request`], and the
//! shell keeps its link running until it comes, then hands it to every
//! listener ([`listen`]) — on the main thread, at the refresh.
//!
//! ```ignore
//! bunny_ui::vsync::listen(move |beat| {
//!     engine.send(Vsync { target: beat.target, interval: beat.interval });
//!     still_wanted
//! });
//! bunny_ui::vsync::request();
//! ```
//!
//! One request is one beat, like `requestAnimationFrame`: an app that
//! stops asking lets the link rest, so a still page costs nothing.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// One refresh of the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayBeat {
    /// When the frame drawn for this refresh reaches the glass.
    pub target: Instant,
    /// The time between two refreshes.
    pub interval: Duration,
}

type Listener = Box<dyn FnMut(DisplayBeat) -> bool>;

thread_local! {
    static REQUESTED: Cell<bool> = const { Cell::new(false) };
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
}

/// Asks the shell on this thread for the next display refresh: its link
/// runs until the refresh comes and the listeners hear of it.
pub fn request() {
    REQUESTED.with(|requested| requested.set(true));
}

/// Whether a refresh was asked for and has not come yet. The shell runs
/// its display link while it is.
pub fn requested() -> bool {
    REQUESTED.with(Cell::get)
}

/// Hears every display refresh on this thread from now on, for as long as
/// it answers `true`.
pub fn listen(listener: impl FnMut(DisplayBeat) -> bool + 'static) {
    LISTENERS.with(|listeners| listeners.borrow_mut().push(Box::new(listener)));
}

/// The shell's half: a refresh came. The request is answered, and every
/// listener hears it; one may ask for the next refresh from inside.
pub fn deliver(beat: DisplayBeat) {
    REQUESTED.with(|requested| requested.set(false));
    let mut running = LISTENERS.with(|listeners| std::mem::take(&mut *listeners.borrow_mut()));
    running.retain_mut(|listener| listener(beat));
    LISTENERS.with(|listeners| {
        let mut listeners = listeners.borrow_mut();
        let added = std::mem::take(&mut *listeners);
        *listeners = running;
        listeners.extend(added);
    });
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    fn beat() -> DisplayBeat {
        DisplayBeat {
            target: Instant::now(),
            interval: Duration::from_micros(8333),
        }
    }

    #[test]
    fn a_request_lasts_until_the_next_refresh_and_listeners_hear_each_one() {
        let heard = Rc::new(Cell::new(0));
        let counted = Rc::clone(&heard);
        listen(move |_| {
            counted.set(counted.get() + 1);
            counted.get() < 2
        });
        assert!(!requested());
        request();
        assert!(requested());
        deliver(beat());
        assert!(!requested(), "one request is one refresh");
        deliver(beat());
        deliver(beat());
        assert_eq!(
            heard.get(),
            2,
            "a listener that answers false hears no more"
        );
    }

    #[test]
    fn a_listener_may_ask_for_the_next_refresh_and_add_another() {
        listen(|_| {
            request();
            listen(|_| false);
            false
        });
        deliver(beat());
        assert!(requested());
        deliver(beat());
        assert!(LISTENERS.with(|listeners| listeners.borrow().is_empty()));
    }
}
