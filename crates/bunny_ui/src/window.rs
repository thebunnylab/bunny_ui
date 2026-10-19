//! Window lifetime and close policy shared by native shells.
//!
//! A native address can be reused after close; a window identity cannot. Shells
//! validate this identity against their own live slots before touching the OS.
//! Hiding preserves a scene. Explicit teardown bypasses the user-close policy.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod confirmation;
mod confirmation_alert;
mod dialog_geometry;
mod geometry;
mod relationships;
pub use confirmation::{
    Confirmation, ConfirmationError, ConfirmationKind, ConfirmationResponse, ConfirmationSpec,
};
#[doc(hidden)]
pub use confirmation::{ConfirmationRequest, ConfirmationSlot};
#[doc(hidden)]
pub use confirmation_alert::FrameworkConfirmation;
pub use dialog_geometry::DialogGeometry;
#[doc(hidden)]
pub use dialog_geometry::{DialogGeometryObservation, DialogGeometrySession};
pub use geometry::{DesktopPosition, DesktopSpace, WindowGeometry, WindowPlacement, WindowSize};
#[doc(hidden)]
pub use geometry::{GeometryEvents, GeometryObservation};
#[doc(hidden)]
pub use relationships::{RelationshipChange, Relationships};

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

/// Why a desktop window operation could not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowError {
    /// The ID is closed or belongs to another application.
    StaleWindow,
    /// This window's close callback is already executing.
    CloseInProgress,
    /// The requested parent would create a cycle, including self-parenting.
    RelationshipCycle,
    /// A native relationship change is already in progress for this app.
    RelationshipInProgress,
    /// This window or an owning ancestor was explicitly hidden.
    HiddenWindow,
    /// The window system declined the activation request.
    FocusDenied,
    /// Invalid size, coordinates, scale or a mismatched desktop coordinate space.
    InvalidGeometry(&'static str),
    /// The native window has not supplied usable geometry in its current state.
    GeometryUnavailable,
    /// A dialog geometry controller is already attached to another presentation.
    GeometryInUse,
    /// A native operation failed with its platform error code.
    NativeFailure {
        /// The native operation that failed.
        operation: &'static str,
        /// The platform's error code.
        code: i64,
    },
    /// The backend cannot provide this operation in the stated situation.
    Unsupported(&'static str),
}

impl std::fmt::Display for WindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleWindow => f.write_str("window is closed or belongs to another application"),
            Self::CloseInProgress => f.write_str("window close callback is already executing"),
            Self::RelationshipCycle => f.write_str("window parent would create a cycle"),
            Self::RelationshipInProgress => {
                f.write_str("window relationship change is in progress")
            }
            Self::HiddenWindow => f.write_str("window or its owner is explicitly hidden"),
            Self::FocusDenied => f.write_str("window system declined activation"),
            Self::InvalidGeometry(reason) => write!(f, "invalid window geometry: {reason}"),
            Self::GeometryUnavailable => f.write_str("window geometry is not yet available"),
            Self::GeometryInUse => {
                f.write_str("dialog geometry already has an active presentation")
            }
            Self::NativeFailure { operation, code } => {
                write!(f, "{operation} failed: native error {code}")
            }
            Self::Unsupported(reason) => write!(f, "window operation is unsupported: {reason}"),
        }
    }
}

impl std::error::Error for WindowError {}

/// The native relationship requested for a separate top-level scene.
/// None of these relationships alone makes the window modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowParentKind {
    /// An `AppKit` child window, ordered above its parent.
    AppKitChild,
    /// A Win32 owned top-level window, not an embedded child control.
    Win32Owner,
    /// An X11 `WM_TRANSIENT_FOR` hint, subject to window-manager policy.
    X11Transient,
    /// A Wayland `xdg_toplevel.set_parent` request, subject to compositor policy.
    WaylandParent,
}

/// A successfully requested native relationship and its owning scene.
/// The window system still decides placement and focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowParent {
    window: WindowId,
    kind: WindowParentKind,
}

impl WindowParent {
    /// The owning window's lifetime.
    #[must_use]
    pub const fn window(self) -> WindowId {
        self.window
    }

    /// The native mechanism used by this backend.
    #[must_use]
    pub const fn kind(self) -> WindowParentKind {
        self.kind
    }
}

type Handler = Rc<RefCell<dyn FnMut(WindowId) -> CloseAction>>;

/// Shared backend state; applications use their shell's `App` methods instead.
#[doc(hidden)]
pub struct Lifecycle {
    id: WindowId,
    handler: RefCell<Option<Handler>>,
    deciding: Cell<bool>,
    hidden: Cell<bool>,
    owner_hidden: Cell<bool>,
    geometry_events: Rc<GeometryEvents>,
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
            geometry_events: Rc::new(GeometryEvents::new(WindowId(id))),
            handler: RefCell::new(None),
            deciding: Cell::new(false),
            hidden: Cell::new(false),
            owner_hidden: Cell::new(false),
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
    /// Native geometry observations belonging to this window lifetime.
    #[must_use]
    pub const fn geometry_events(&self) -> &Rc<GeometryEvents> {
        &self.geometry_events
    }

    /// The immutable identity belonging to this backend slot.
    #[must_use]
    pub const fn id(&self) -> WindowId {
        self.id
    }

    /// Whether this window or an owning ancestor suspended presentation.
    #[must_use]
    pub const fn is_hidden(&self) -> bool {
        self.hidden.get() || self.owner_hidden.get()
    }

    /// Updates inherited visibility without erasing an explicit hide request.
    pub fn set_owner_hidden(&self, hidden: bool) {
        self.owner_hidden.set(hidden);
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
