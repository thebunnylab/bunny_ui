//! A window that takes the whole screen, and the news of its trips there.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use super::{WindowError, WindowId};

/// How a window stands on its screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FullscreenMode {
    /// An ordinary window among the others.
    #[default]
    Windowed,
    /// The platform's own full screen: the window fills a screen of its
    /// own (a Space on the mac) and the system's bars hide until the
    /// pointer reaches the top edge.
    Fullscreen,
    /// Full screen for one thing — a video, a slide. The toolbar hides
    /// with the menu bar, nothing of the window's frame stays on the
    /// screen, and Escape takes the window back, as a player's does. The
    /// app draws its one element over the whole window.
    Presentation,
}

impl FullscreenMode {
    /// Does the window fill the screen?
    #[must_use]
    pub const fn is_fullscreen(self) -> bool {
        !matches!(self, Self::Windowed)
    }
}

/// One step of a trip into or out of full screen, from whoever asked: the
/// app, the green button, Escape, the system's own menu item. A trip is
/// animated, so it starts and ends in two events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FullscreenEvent {
    /// The window starts filling the screen.
    WillEnter,
    /// The window fills the screen.
    Entered,
    /// The window starts going back among the others.
    WillExit,
    /// The window is an ordinary window again.
    Exited,
    /// The system declined to enter full screen; the window stays as it was.
    EnterFailed,
    /// The system declined to leave full screen; the window stays full.
    ExitFailed,
}

type Handler = Rc<RefCell<dyn FnMut(WindowId, FullscreenEvent)>>;

/// Backend subscription state belonging to one non-recycled window lifetime.
#[doc(hidden)]
pub struct FullscreenEvents {
    id: WindowId,
    retired: Cell<bool>,
    delivering: Cell<bool>,
    handler: RefCell<Option<Handler>>,
    pending: RefCell<VecDeque<FullscreenEvent>>,
}

impl FullscreenEvents {
    pub(super) const fn new(id: WindowId) -> Self {
        Self {
            id,
            retired: Cell::new(false),
            delivering: Cell::new(false),
            handler: RefCell::new(None),
            pending: RefCell::new(VecDeque::new()),
        }
    }

    /// Replaces the subscription, discarding events queued for the old one.
    ///
    /// # Errors
    /// A retired subscription cannot be revived under its old window identity.
    pub fn set_handler(
        &self,
        handler: impl FnMut(WindowId, FullscreenEvent) + 'static,
    ) -> Result<(), WindowError> {
        if self.retired.get() {
            return Err(WindowError::StaleWindow);
        }
        self.clear_handler();
        *self.handler.borrow_mut() = Some(Rc::new(RefCell::new(handler)));
        Ok(())
    }

    pub fn clear_handler(&self) {
        let previous = self.handler.borrow_mut().take();
        self.pending.borrow_mut().clear();
        drop(previous);
    }

    /// Delivers events in native order; one raised by a handler waits for
    /// the handler to return.
    pub fn observe(&self, event: FullscreenEvent) {
        struct Delivery<'a>(&'a Cell<bool>);
        impl Drop for Delivery<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        if self.retired.get() || self.handler.borrow().is_none() {
            return;
        }
        self.pending.borrow_mut().push_back(event);
        if self.delivering.replace(true) {
            return;
        }
        let _delivery = Delivery(&self.delivering);
        loop {
            let next = self.pending.borrow_mut().pop_front();
            let Some(event) = next else { break };
            let handler = self.handler.borrow().clone();
            if let Some(handler) = handler {
                (handler.borrow_mut())(self.id, event);
            }
        }
    }

    /// Permanently retires the subscription before its native address can be reused.
    pub fn retire(&self) {
        self.retired.set(true);
        self.clear_handler();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> (Rc<FullscreenEvents>, Rc<RefCell<Vec<FullscreenEvent>>>) {
        let events = Rc::new(FullscreenEvents::new(WindowId(7)));
        let heard = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&heard);
        events
            .set_handler(move |id, event| {
                assert_eq!(id, WindowId(7));
                log.borrow_mut().push(event);
            })
            .expect("live");
        (events, heard)
    }

    #[test]
    fn a_trip_arrives_in_the_order_the_system_tells_it() {
        let (events, heard) = events();
        for event in [
            FullscreenEvent::WillEnter,
            FullscreenEvent::Entered,
            FullscreenEvent::WillExit,
            FullscreenEvent::Exited,
        ] {
            events.observe(event);
        }
        assert_eq!(
            *heard.borrow(),
            [
                FullscreenEvent::WillEnter,
                FullscreenEvent::Entered,
                FullscreenEvent::WillExit,
                FullscreenEvent::Exited
            ]
        );
    }

    #[test]
    fn an_event_raised_by_the_handler_waits_for_it_to_return() {
        let events = Rc::new(FullscreenEvents::new(WindowId(1)));
        let heard = Rc::new(RefCell::new(Vec::new()));
        let (log, inner) = (Rc::clone(&heard), Rc::clone(&events));
        events
            .set_handler(move |_, event| {
                log.borrow_mut().push((event, "start"));
                if event == FullscreenEvent::WillEnter {
                    inner.observe(FullscreenEvent::Entered);
                }
                log.borrow_mut().push((event, "end"));
            })
            .expect("live");
        events.observe(FullscreenEvent::WillEnter);
        assert_eq!(
            *heard.borrow(),
            [
                (FullscreenEvent::WillEnter, "start"),
                (FullscreenEvent::WillEnter, "end"),
                (FullscreenEvent::Entered, "start"),
                (FullscreenEvent::Entered, "end"),
            ]
        );
    }

    #[test]
    fn a_handler_may_replace_itself_and_the_next_event_reaches_the_new_one() {
        let events = Rc::new(FullscreenEvents::new(WindowId(2)));
        let heard = Rc::new(RefCell::new(Vec::new()));
        let (log, inner) = (Rc::clone(&heard), Rc::clone(&events));
        events
            .set_handler(move |_, event| {
                log.borrow_mut().push(("old", event));
                let log = Rc::clone(&log);
                inner
                    .set_handler(move |_, event| log.borrow_mut().push(("new", event)))
                    .expect("live");
            })
            .expect("live");
        events.observe(FullscreenEvent::WillExit);
        events.observe(FullscreenEvent::Exited);
        assert_eq!(
            *heard.borrow(),
            [
                ("old", FullscreenEvent::WillExit),
                ("new", FullscreenEvent::Exited)
            ]
        );
    }

    #[test]
    fn a_retired_window_hears_nothing_and_takes_no_handler() {
        let (events, heard) = events();
        events.retire();
        events.observe(FullscreenEvent::WillEnter);
        assert!(heard.borrow().is_empty());
        assert_eq!(events.set_handler(|_, _| {}), Err(WindowError::StaleWindow));
    }

    #[test]
    fn only_a_windowed_window_is_not_full_screen() {
        assert!(!FullscreenMode::Windowed.is_fullscreen());
        assert!(FullscreenMode::Fullscreen.is_fullscreen());
        assert!(FullscreenMode::Presentation.is_fullscreen());
        assert_eq!(FullscreenMode::default(), FullscreenMode::Windowed);
    }
}
