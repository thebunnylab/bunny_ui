//! `CancelBag` — Combine's `Cancellable` collection (`subscriptions`).

use std::cell::RefCell;
use std::rc::Rc;

/// Combine's `Cancellable`.
pub trait Cancellable {
    fn cancel(&self);
}

/// `final class CancelBag` — shared storage, cloned bags see the same subscriptions.
#[derive(Clone)]
pub struct CancelBag {
    subscriptions: Rc<RefCell<Vec<Rc<dyn Cancellable>>>>,
    equalToAny: bool,
}

impl CancelBag {
    pub fn new() -> Self {
        CancelBag { subscriptions: Rc::new(RefCell::new(Vec::new())), equalToAny: false }
    }

    /// `CancelBag(equalToAny: true)` — used by unit tests.
    pub fn newEqualToAny() -> Self {
        CancelBag { subscriptions: Rc::new(RefCell::new(Vec::new())), equalToAny: true }
    }

    /// `cancellable.store(in: bag)`
    pub fn store(&self, cancellable: Rc<dyn Cancellable>) {
        self.subscriptions.borrow_mut().push(cancellable);
    }

    /// `bag.cancel()`
    pub fn cancel(&self) {
        self.subscriptions.borrow_mut().clear();
    }

    /// `bag.isEqual(to: other)`
    pub fn isEqual(&self, other: &CancelBag) -> bool {
        Rc::ptr_eq(&self.subscriptions, &other.subscriptions)
            || self.equalToAny
            || other.equalToAny
    }
}

impl Default for CancelBag {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CancelBag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CancelBag({} subscriptions)", self.subscriptions.borrow().len())
    }
}

/// A fake `Task` handle — cancelling flags a shared bool.
pub struct TaskHandle {
    cancelled: Rc<std::cell::Cell<bool>>,
}

impl TaskHandle {
    pub fn new() -> (Rc<TaskHandle>, Rc<std::cell::Cell<bool>>) {
        let cancelled = Rc::new(std::cell::Cell::new(false));
        (Rc::new(TaskHandle { cancelled: cancelled.clone() }), cancelled)
    }

    pub fn isCancelled(&self) -> bool {
        self.cancelled.get()
    }
}

impl Cancellable for TaskHandle {
    fn cancel(&self) {
        self.cancelled.set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_follows_swift_semantics() {
        let bag = CancelBag::new();
        assert!(bag.isEqual(&bag.clone()));
        assert!(CancelBag::new().isEqual(&CancelBag::newEqualToAny()));
        assert!(!CancelBag::new().isEqual(&CancelBag::new()));
    }
}
