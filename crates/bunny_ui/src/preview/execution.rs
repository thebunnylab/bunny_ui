//! A preview's execution intent and lifetime, independent of application services.
//!
//! ## Wiring
//! The host owns one [`ExecutionScope`] per scene. Application adapters capture
//! [`ExecutionContext::current`] during construction and retain a subscription
//! to switch fixtures/connections or cancel work when the scope closes. The
//! framework carries no transport, executor or credentials.
//!
//! ## Production gotchas
//! Context is thread-local and synchronous: an asynchronous task must capture a
//! handle explicitly. Handles do not keep a scope open. Notifications describe
//! current state, not an event log; a reentrant change supersedes an undelivered
//! state. Callbacks must not oscillate modes indefinitely. A callback panic is
//! rethrown after notifying other listeners; during an existing unwind, secondary
//! callback panics are suppressed so scope closure preserves the original panic.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::rc::{Rc, Weak};

/// Whether a scene is being designed or explicitly executed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Application adapters use fixtures and avoid live effects.
    #[default]
    Design,
    /// Application adapters may use their explicitly configured live services.
    Play,
}

/// The sole owner of a scene's execution lifetime; dropping it closes all handles.
pub struct ExecutionScope {
    context: ExecutionContext,
}

impl ExecutionScope {
    /// Open a new independent scope in `mode`.
    pub fn new(mode: ExecutionMode) -> Self {
        Self {
            context: ExecutionContext(Rc::new(Core {
                mode: Cell::new(Some(mode)),
                listeners: RefCell::new(Vec::new()),
            })),
        }
    }

    /// A handle that can observe, but cannot extend, this lifetime.
    pub fn context(&self) -> ExecutionContext {
        self.context.clone()
    }

    /// Run with this scene's context, restoring the previous one on return or panic.
    pub fn with<R>(&self, run: impl FnOnce() -> R) -> R {
        self.context.with(run)
    }

    /// Change intent and notify subscribers under this context. Returns whether
    /// a transition started; a reentrant callback may change the mode again.
    pub fn set_mode(&self, mode: ExecutionMode) -> bool {
        self.with(|| self.context.0.publish(Some(mode)))
    }
}

impl Drop for ExecutionScope {
    fn drop(&mut self) {
        self.with(|| self.context.0.publish(None));
    }
}

/// A captured scene context. `mode() == None` means its owner has closed.
#[derive(Clone)]
pub struct ExecutionContext(Rc<Core>);

impl ExecutionContext {
    /// The enclosing scope, or `None` outside a scoped synchronous call.
    pub fn current() -> Option<Self> {
        CURRENT.with(|current| current.borrow().clone())
    }

    /// The current intent, or `None` after the owning scope has closed forever.
    pub fn mode(&self) -> Option<ExecutionMode> {
        self.0.mode.get()
    }

    /// Re-enter this captured context for a synchronous callback. This does not
    /// reopen a closed context; adapters must still check [`Self::mode`].
    pub fn with<R>(&self, run: impl FnOnce() -> R) -> R {
        let _restore = Restore(CURRENT.with(|current| current.replace(Some(self.clone()))));
        run()
    }

    /// Observe subsequent changes, including `None` on closure. Read [`Self::mode`]
    /// for the initial value. Retain the returned subscription to keep listening.
    pub fn subscribe(
        &self,
        notify: impl Fn(Option<ExecutionMode>) + 'static,
    ) -> ExecutionSubscription {
        let listener = Rc::new(Listener {
            active: Cell::new(true),
            seen: Cell::new(self.mode()),
            notify: Box::new(notify),
        });
        let mut listeners = self.0.listeners.borrow_mut();
        listeners.retain(|listener| listener.strong_count() != 0);
        listeners.push(Rc::downgrade(&listener));
        ExecutionSubscription(listener)
    }
}

/// A notification registration. Dropping it detaches immediately, even during a
/// notification already in progress. It does not retain the scope or context.
pub struct ExecutionSubscription(Rc<Listener>);

impl Drop for ExecutionSubscription {
    fn drop(&mut self) {
        self.0.active.set(false);
    }
}

struct Core {
    mode: Cell<Option<ExecutionMode>>,
    listeners: RefCell<Vec<Weak<Listener>>>,
}

impl Core {
    fn publish(&self, mode: Option<ExecutionMode>) -> bool {
        if self.mode.replace(mode) == mode {
            return false;
        }
        let listeners: Vec<_> = self
            .listeners
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        let unwinding = std::thread::panicking();
        let mut panic = None;
        for listener in listeners {
            // A previous listener can remove this one or change the mode. Never
            // deliver the stale outer transition after that nested notification.
            let current = self.mode.get();
            if listener.active.get()
                && listener.seen.replace(current) != current
                && let Err(payload) = catch_unwind(AssertUnwindSafe(|| (listener.notify)(current)))
                && panic.is_none()
            {
                panic = Some(payload);
            }
        }
        if !unwinding && let Some(payload) = panic {
            resume_unwind(payload);
        }
        true
    }
}

struct Listener {
    active: Cell<bool>,
    seen: Cell<Option<ExecutionMode>>,
    notify: Box<dyn Fn(Option<ExecutionMode>)>,
}

thread_local! {
    static CURRENT: RefCell<Option<ExecutionContext>> = const { RefCell::new(None) };
}

struct Restore(Option<ExecutionContext>);
impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.0.take()));
    }
}
