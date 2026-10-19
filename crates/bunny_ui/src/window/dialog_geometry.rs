//! Persist dialog placement without confusing a model identity with a native lifetime.
//!
//! The controller belongs to the application. Each host presentation holds a
//! separate lease; deferred native observations hold only weak lease/subscription
//! identities. Native adapters capture facts first and deliver after releasing
//! their platform and renderer borrows.

use super::{WindowError, WindowGeometry, WindowPlacement};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fmt,
    rc::{Rc, Weak},
};

type GeometryResult = Result<WindowGeometry, WindowError>;
type Handler = RefCell<dyn FnMut(GeometryResult)>;

struct Subscription {
    handler: Option<Rc<Handler>>,
}

struct Controller {
    initial: Option<WindowPlacement>,
    geometry: Cell<Option<WindowGeometry>>,
    error: Cell<Option<WindowError>>,
    active: RefCell<Weak<DialogGeometrySession>>,
    subscription: RefCell<Rc<Subscription>>,
    pending: RefCell<VecDeque<DialogGeometryObservation>>,
    last: RefCell<Option<DialogGeometryObservation>>,
    delivering: Cell<bool>,
}

/// Optional persistent geometry for one dialog at a time.
///
/// Attach a clone through [`crate::layout::DialogSpec::geometry`]. A saved
/// placement is an opening request; only a native observation updates
/// [`Self::geometry`]. Keep distinct controllers for independently open dialogs.
/// Callbacks run on the owning UI thread and may dismiss or reopen the dialog.
#[derive(Clone)]
pub struct DialogGeometry(Rc<Controller>);

impl DialogGeometry {
    /// Keeps the ordinary initial placement and observes subsequent native facts.
    #[must_use]
    pub fn new() -> Self {
        Self::with_initial(None)
    }

    /// Requests a saved placement when the native dialog first opens.
    /// A later reopening prefers the last actually observed placement.
    #[must_use]
    pub fn restoring(placement: WindowPlacement) -> Self {
        Self::with_initial(Some(placement))
    }

    fn with_initial(initial: Option<WindowPlacement>) -> Self {
        Self(Rc::new(Controller {
            initial,
            geometry: Cell::new(None),
            error: Cell::new(None),
            active: RefCell::new(Weak::new()),
            subscription: RefCell::new(Rc::new(Subscription { handler: None })),
            pending: RefCell::new(VecDeque::new()),
            last: RefCell::new(None),
            delivering: Cell::new(false),
        }))
    }

    /// Last native geometry, retained across dismissal for application persistence.
    /// A restoration request or a native failure cannot populate this value.
    #[must_use]
    pub fn geometry(&self) -> Option<WindowGeometry> {
        self.0.geometry.get()
    }

    /// Last failure since presentation began; successful observations do not erase it.
    #[must_use]
    pub fn last_error(&self) -> Option<WindowError> {
        self.0.error.get()
    }

    /// Replaces the callback for future captured native results, without replaying
    /// a cached value. Captures for the previous subscription are discarded.
    pub fn on_change(&self, handler: impl FnMut(GeometryResult) + 'static) {
        self.replace_subscription(Some(Rc::new(RefCell::new(handler))));
    }

    /// Removes the callback and its queued deliveries. Future native observations
    /// still update the readable geometry while the controller is attached.
    pub fn clear_handler(&self) {
        self.replace_subscription(None);
    }

    fn replace_subscription(&self, handler: Option<Rc<Handler>>) {
        let previous = self
            .0
            .subscription
            .replace(Rc::new(Subscription { handler }));
        self.0.pending.borrow_mut().clear();
        self.0.last.borrow_mut().take();
        drop(previous);
    }

    pub(crate) fn capture_error(&self, error: WindowError) -> Option<DialogGeometryObservation> {
        let session = self.0.active.borrow().upgrade()?;
        session.capture(Err(error))
    }

    /// Acquires a native presentation lease; desktop hosts own and retire it.
    ///
    /// # Errors
    /// Returns [`WindowError::GeometryInUse`] if a different presentation is active.
    #[doc(hidden)]
    pub fn begin(&self) -> Result<Rc<DialogGeometrySession>, WindowError> {
        if self.0.active.borrow().upgrade().is_some() {
            self.0.error.set(Some(WindowError::GeometryInUse));
            return Err(WindowError::GeometryInUse);
        }
        let session = Rc::new(DialogGeometrySession {
            controller: Rc::downgrade(&self.0),
            retired: Cell::new(false),
            restoration: Cell::new(
                self.geometry()
                    .map(WindowGeometry::placement)
                    .or(self.0.initial),
            ),
        });
        *self.0.active.borrow_mut() = Rc::downgrade(&session);
        self.0.error.set(None);
        self.0.last.borrow_mut().take();
        Ok(session)
    }
}

impl Default for DialogGeometry {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for DialogGeometry {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DialogGeometry {}

impl fmt::Debug for DialogGeometry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialogGeometry")
            .field("geometry", &self.geometry())
            .field("last_error", &self.last_error())
            .finish_non_exhaustive()
    }
}

/// One host-owned presentation; it cannot be reused for a reopened native window.
#[doc(hidden)]
pub struct DialogGeometrySession {
    controller: Weak<Controller>,
    retired: Cell<bool>,
    restoration: Cell<Option<WindowPlacement>>,
}

impl DialogGeometrySession {
    /// Takes the opening request once, without claiming that it was applied.
    #[must_use]
    pub fn take_restoration(&self) -> Option<WindowPlacement> {
        if self.retired.get() {
            None
        } else {
            self.restoration.take()
        }
    }

    /// Captures a native result without retaining application state or running code.
    #[must_use]
    pub fn capture(self: &Rc<Self>, result: GeometryResult) -> Option<DialogGeometryObservation> {
        let controller = self.controller.upgrade()?;
        if self.retired.get() || !controller.active.borrow().ptr_eq(&Rc::downgrade(self)) {
            return None;
        }
        let subscription = Rc::downgrade(&controller.subscription.borrow());
        Some(DialogGeometryObservation {
            session: Rc::downgrade(self),
            subscription,
            result,
        })
    }

    /// Invalidates deferred events before pooling, destruction or controller replacement.
    pub fn retire(&self) {
        self.retired.set(true);
        self.restoration.take();
        if let Some(controller) = self.controller.upgrade()
            && std::ptr::eq(controller.active.borrow().as_ptr(), self)
        {
            *controller.active.borrow_mut() = Weak::new();
        }
    }
}

impl Drop for DialogGeometrySession {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Deferred native fact; delivery requires its original live lease and subscription.
#[doc(hidden)]
#[derive(Clone)]
pub struct DialogGeometryObservation {
    session: Weak<DialogGeometrySession>,
    subscription: Weak<Subscription>,
    result: GeometryResult,
}

impl DialogGeometryObservation {
    /// Delivers at a host boundary with no native, layout or renderer borrows held.
    /// Callback reentrancy, including closing/reopening, is queued in native order.
    pub fn deliver(self) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let Some(controller) = session.controller.upgrade() else {
            return;
        };
        if !controller.accepts(&self) {
            return;
        }
        let duplicate = controller.last.borrow().as_ref().is_some_and(|last| {
            last.session.ptr_eq(&self.session)
                && last.subscription.ptr_eq(&self.subscription)
                && last.result == self.result
        });
        if duplicate {
            return;
        }
        *controller.last.borrow_mut() = Some(self.clone());
        controller.pending.borrow_mut().push_back(self);
        controller.drain();
    }
}

impl Controller {
    fn accepts(&self, observation: &DialogGeometryObservation) -> bool {
        observation
            .session
            .upgrade()
            .is_some_and(|session| !session.retired.get())
            && self.active.borrow().ptr_eq(&observation.session)
            && observation
                .subscription
                .ptr_eq(&Rc::downgrade(&self.subscription.borrow()))
    }

    fn drain(&self) {
        struct Delivery<'a>(&'a Cell<bool>);
        impl Drop for Delivery<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        if self.delivering.replace(true) {
            return;
        }
        let _delivery = Delivery(&self.delivering);
        loop {
            let next = self.pending.borrow_mut().pop_front();
            let Some(observation) = next else { break };
            if !self.accepts(&observation) {
                continue;
            }
            match observation.result {
                Ok(geometry) => self.geometry.set(Some(geometry)),
                Err(error) => self.error.set(Some(error)),
            }
            let subscription = self.subscription.borrow().clone();
            if let Some(handler) = &subscription.handler {
                (handler.borrow_mut())(observation.result);
            }
        }
    }
}
