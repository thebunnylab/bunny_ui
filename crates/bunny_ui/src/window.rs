//! Window lifetime and close policy shared by native shells.
//!
//! A native address can be reused after close; a window identity cannot. Shells
//! validate this identity against their own live slots before touching the OS.
//! Hiding preserves a scene. Explicit teardown bypasses the user-close policy.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One window lifetime. Only an owning shell can associate it with a live scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(usize);

/// An application's answer to a user-requested window close.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloseAction {
    /// Destroy this window; the last window retains the shell's exit behavior.
    #[default]
    Close,
    /// Hide the native window while keeping its scene and application alive.
    Hide,
    /// Keep the window open, for example while an asynchronous save is pending.
    Cancel,
}

/// Whether a window is submitted to the native window system.
///
/// `Shown` does not promise unobscured pixels, activation or keyboard focus. On
/// Wayland, showing starts an asynchronous configure/ack/map handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowVisibility {
    /// Available to the window system, possibly occluded or minimized.
    Shown,
    /// Explicitly hidden without destroying its scene.
    Hidden,
}

/// A rejected operation leaves the identified window unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowError {
    /// The ID is closed or belongs to another application.
    StaleWindow,
    /// This window's close callback is already executing.
    CloseInProgress,
    /// The backend cannot provide this operation in the stated situation.
    Unsupported(&'static str),
}

impl std::fmt::Display for WindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleWindow => f.write_str("window is closed or belongs to another application"),
            Self::CloseInProgress => f.write_str("window close callback is already executing"),
            Self::Unsupported(reason) => write!(f, "window operation is unsupported: {reason}"),
        }
    }
}

impl std::error::Error for WindowError {}

type Handler = Rc<RefCell<dyn FnMut(WindowId) -> CloseAction>>;

/// Shared backend state; applications use their shell's `App` methods instead.
#[doc(hidden)]
pub struct Lifecycle {
    id: WindowId,
    handler: RefCell<Option<Handler>>,
    deciding: Cell<bool>,
    hidden: Cell<bool>,
}

impl Default for Lifecycle {
    /// Allocates an identity that is never recycled.
    ///
    /// # Panics
    /// Panics if the process exhausts the identity space, refusing reuse.
    fn default() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let id = reserve_identity(&NEXT)
            .expect("window identity space exhausted; identities cannot be reused");
        Self {
            id: WindowId(id),
            handler: RefCell::new(None),
            deciding: Cell::new(false),
            hidden: Cell::new(false),
        }
    }
}

// A checked compare/exchange keeps exhaustion permanent, without depending on
// the fetch_update/try_update naming transition across supported toolchains.
fn reserve_identity(next: &AtomicUsize) -> Option<usize> {
    let mut current = next.load(Ordering::Relaxed);
    loop {
        let following = current.checked_add(1)?;
        match next.compare_exchange_weak(current, following, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(current),
            Err(actual) => current = actual,
        }
    }
}

impl Lifecycle {
    /// The immutable identity belonging to this backend slot.
    #[must_use]
    pub const fn id(&self) -> WindowId {
        self.id
    }

    /// Whether the owning shell explicitly suspended native presentation.
    #[must_use]
    pub const fn is_hidden(&self) -> bool {
        self.hidden.get()
    }

    /// Records a shell visibility transition; returns the previous state.
    pub const fn set_hidden(&self, hidden: bool) -> bool {
        self.hidden.replace(hidden)
    }

    /// Replaces the policy, including from inside the previous policy.
    pub fn on_close(&self, handler: impl FnMut(WindowId) -> CloseAction + 'static) {
        self.handler.replace(Some(Rc::new(RefCell::new(handler))));
    }

    /// Restores default close behavior for the next request.
    pub fn clear(&self) {
        self.handler.replace(None);
    }

    /// Calls application code without holding the policy registry borrowed.
    /// Same-window recursion is rejected even if a callback replaces itself.
    ///
    /// # Errors
    /// Returns [`WindowError::CloseInProgress`] for same-window recursion.
    pub fn decide(&self) -> Result<CloseAction, WindowError> {
        struct Decision<'a>(&'a Cell<bool>);
        impl Drop for Decision<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        if self.deciding.replace(true) {
            return Err(WindowError::CloseInProgress);
        }
        let _decision = Decision(&self.deciding);
        let handler = self.handler.borrow().clone();
        Ok(handler.map_or(CloseAction::Close, |handler| {
            (handler.borrow_mut())(self.id)
        }))
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn exhausting_the_identity_space_never_wraps_or_reuses() {
        let next = AtomicUsize::new(usize::MAX - 1);
        assert_eq!(reserve_identity(&next), Some(usize::MAX - 1));
        assert_eq!(reserve_identity(&next), None);
        assert_eq!(reserve_identity(&next), None);
        assert_eq!(next.load(Ordering::Relaxed), usize::MAX);
    }

    #[test]
    fn concurrent_reservations_are_unique() {
        let next = AtomicUsize::new(1);
        let mut ids = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        (0..64)
                            .map(|_| reserve_identity(&next).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        ids.sort_unstable();
        assert_eq!(ids, (1..=256).collect::<Vec<_>>());
    }
}
