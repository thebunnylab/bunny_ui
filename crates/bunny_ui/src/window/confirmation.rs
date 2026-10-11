//! A confirmation belongs to one window lifetime and one awaiting consumer.
//!
//! ## Wiring
//! Desktop shells own a [`ConfirmationSlot`] and present its weak request through
//! their native adapter. Cancellation is acknowledged after native teardown; a
//! racing affirmative answer can never override an earlier cancellation.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use crate::prelude::{State, StateExt};

use super::{WindowError, WindowId};

/// Validated application text for a native Yes/No question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmationSpec {
    question: Arc<str>,
    detail: Arc<str>,
}

impl ConfirmationSpec {
    /// Constructs text that every desktop adapter can represent without truncation.
    ///
    /// # Errors
    /// The question must contain visible text. Neither string may contain NUL.
    pub fn new(
        question: impl Into<Arc<str>>,
        detail: impl Into<Arc<str>>,
    ) -> Result<Self, ConfirmationError> {
        let question = question.into();
        let detail = detail.into();
        if question.trim().is_empty() {
            return Err(ConfirmationError::InvalidText("question is empty"));
        }
        if question.contains('\0') || detail.contains('\0') {
            return Err(ConfirmationError::InvalidText("text contains NUL"));
        }
        Ok(Self { question, detail })
    }

    /// The question the person is answering, preserved verbatim.
    #[must_use]
    pub fn question(&self) -> &str {
        &self.question
    }

    /// Optional explanatory text, preserved verbatim.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// The presentation actually used; these mechanisms have different modality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmationKind {
    /// An `NSAlert` sheet attached to the owning `AppKit` window.
    AppKitSheet,
    /// A system Win32 message box that disables its owner while open.
    Win32MessageBox,
    /// A framework alert on a native Linux window, with framework input isolation.
    FrameworkAlert,
}

/// A native answer, distinct from a failed or cancelled operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmationResponse {
    /// The person explicitly affirmed the question.
    Yes,
    /// The person chose the negative answer.
    No,
    /// The native presentation was dismissed without choosing an answer.
    Dismissed,
}

/// Why a confirmation did not produce an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmationError {
    /// The window operation failed before or during presentation.
    Window(WindowError),
    /// This owner already has a request or native teardown in progress.
    AlreadyOpen,
    /// The caller cancelled the operation.
    Cancelled,
    /// The owning window retired before the native response completed.
    OwnerClosed,
    /// The question cannot be represented consistently by the native adapters.
    InvalidText(&'static str),
}

impl std::fmt::Display for ConfirmationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Window(error) => error.fmt(f),
            Self::AlreadyOpen => f.write_str("this window already has a confirmation in progress"),
            Self::Cancelled => f.write_str("confirmation cancelled"),
            Self::OwnerClosed => f.write_str("confirmation owner closed"),
            Self::InvalidText(reason) => write!(f, "invalid confirmation text: {reason}"),
        }
    }
}

impl std::error::Error for ConfirmationError {}

impl From<WindowError> for ConfirmationError {
    fn from(error: WindowError) -> Self {
        Self::Window(error)
    }
}

type Answer = Result<ConfirmationResponse, ConfirmationError>;
type Cancel = Box<dyn FnOnce()>;

enum Cancellation {
    Unregistered,
    Armed(Cancel),
    Sent,
}

enum Progress {
    Pending {
        waiter: Option<Waker>,
        cancel: Cancellation,
        requested: Option<ConfirmationError>,
    },
    Ready(Answer),
    Taken,
}

struct Shared {
    token: usize,
    spec: ConfirmationSpec,
    slot: Weak<Slot>,
    progress: RefCell<Progress>,
}

struct Slot {
    owner: WindowId,
    kind: ConfirmationKind,
    closed: Cell<bool>,
    changed: State<Option<usize>>,
    current: RefCell<Option<Rc<Shared>>>,
}

/// One outstanding operation. Dropping it cancels its native presentation.
///
/// This future is intentionally local to the owning UI thread and cannot be
/// cloned or sent to a worker. It resolves after the native adapter has finished
/// teardown. Native answers, cancellation and owner retirement are distinct.
///
/// ```compile_fail
/// use bunny_ui_core::window::Confirmation;
/// fn require_send<T: Send>() {}
/// require_send::<Confirmation>();
/// ```
#[must_use = "await the confirmation or retain it; dropping it cancels the presentation"]
pub struct Confirmation {
    shared: Rc<Shared>,
    owner: WindowId,
    kind: ConfirmationKind,
}

impl Confirmation {
    /// The window lifetime this request belongs to.
    #[must_use]
    pub const fn owner(&self) -> WindowId {
        self.owner
    }

    /// The actual native presentation mechanism.
    #[must_use]
    pub const fn kind(&self) -> ConfirmationKind {
        self.kind
    }

    /// Requests cancellation once. The future resolves after native teardown.
    pub fn cancel(&mut self) -> bool {
        self.shared.cancel(ConfirmationError::Cancelled)
    }
}

impl Drop for Confirmation {
    fn drop(&mut self) {
        self.shared.cancel(ConfirmationError::Cancelled);
    }
}

impl Future for Confirmation {
    type Output = Answer;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Answer> {
        let mut progress = self.shared.progress.borrow_mut();
        match &mut *progress {
            Progress::Pending { waiter, .. } => {
                if waiter.as_ref().is_none_or(|old| !old.will_wake(cx.waker())) {
                    *waiter = Some(cx.waker().clone());
                }
                Poll::Pending
            }
            Progress::Ready(_) => {
                let Progress::Ready(answer) = std::mem::replace(&mut *progress, Progress::Taken)
                else {
                    unreachable!("the matched ready state cannot change while borrowed")
                };
                Poll::Ready(answer)
            }
            Progress::Taken => Poll::Pending,
        }
    }
}

/// Backend ownership and retained-view notification for one window lifetime.
#[doc(hidden)]
#[derive(Clone)]
pub struct ConfirmationSlot(Rc<Slot>);

impl ConfirmationSlot {
    /// Creates the adapter slot alongside its native window lifecycle.
    #[must_use]
    pub fn new(owner: WindowId, kind: ConfirmationKind) -> Self {
        Self(Rc::new(Slot {
            owner,
            kind,
            closed: Cell::new(false),
            changed: State::new(None),
            current: RefCell::new(None),
        }))
    }

    /// Reserves this owner and returns the single consumer plus its weak adapter endpoint.
    ///
    /// # Errors
    /// Refuses a retired owner or another outstanding presentation.
    ///
    /// # Panics
    /// Refuses token reuse if the process exhausts the identity space.
    pub fn start(
        &self,
        spec: ConfirmationSpec,
    ) -> Result<(Confirmation, ConfirmationRequest), ConfirmationError> {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        if self.0.closed.get() {
            return Err(ConfirmationError::OwnerClosed);
        }
        if self.0.current.borrow().is_some() {
            return Err(ConfirmationError::AlreadyOpen);
        }
        let token = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("confirmation identity space exhausted; tokens cannot be reused");
        let shared = Rc::new(Shared {
            token,
            spec,
            slot: Rc::downgrade(&self.0),
            progress: RefCell::new(Progress::Pending {
                waiter: None,
                cancel: Cancellation::Unregistered,
                requested: None,
            }),
        });
        let request = ConfirmationRequest(Rc::downgrade(&shared), token);
        self.0.current.replace(Some(Rc::clone(&shared)));
        self.0.notify();
        Ok((
            Confirmation {
                shared,
                owner: self.0.owner,
                kind: self.0.kind,
            },
            request,
        ))
    }

    /// The current request; reading it participates in retained view invalidation.
    #[must_use]
    pub fn current(&self) -> Option<ConfirmationRequest> {
        let _ = self.0.changed.get();
        self.0
            .current
            .borrow()
            .as_ref()
            .map(|shared| ConfirmationRequest(Rc::downgrade(shared), shared.token))
    }

    /// Cancels a presentation, for example before its owner is hidden.
    pub fn cancel(&self, error: ConfirmationError) {
        let current = self.0.current.borrow().clone();
        if let Some(current) = current {
            current.cancel(error);
        }
    }

    /// Retires the owner permanently and cancels its current presentation.
    pub fn owner_closed(&self) {
        self.0.closed.set(true);
        self.cancel(ConfirmationError::OwnerClosed);
    }
}

impl Slot {
    fn notify(&self) {
        self.changed
            .set(self.current.borrow().as_ref().map(|request| request.token));
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(shared) = self.current.get_mut().take() {
            shared.cancel(ConfirmationError::OwnerClosed);
        }
    }
}

/// A weak native response endpoint; it cannot keep the consumer alive.
#[doc(hidden)]
#[derive(Clone)]
pub struct ConfirmationRequest(Weak<Shared>, usize);

impl ConfirmationRequest {
    /// A never-reused opaque token, suitable for a POD native callback capture.
    #[must_use]
    pub const fn token(&self) -> usize {
        self.1
    }

    /// Application text while the request is still waiting for native completion.
    #[must_use]
    pub fn spec(&self) -> Option<ConfirmationSpec> {
        let shared = self.0.upgrade()?;
        let pending = matches!(*shared.progress.borrow(), Progress::Pending { .. });
        pending.then(|| shared.spec.clone())
    }

    /// Whether a native answer or teardown acknowledgement is still outstanding.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.0
            .upgrade()
            .is_some_and(|shared| matches!(*shared.progress.borrow(), Progress::Pending { .. }))
    }

    /// Whether this queued request still wants a presentation to be opened.
    #[must_use]
    pub fn can_present(&self) -> bool {
        self.0.upgrade().is_some_and(|shared| {
            matches!(
                *shared.progress.borrow(),
                Progress::Pending {
                    requested: None,
                    ..
                }
            )
        })
    }

    /// Installs one native cancellation operation, called outside registry borrows.
    /// Returns false if completion already happened or a canceller was installed.
    pub fn on_cancel(&self, callback: impl FnOnce() + 'static) -> bool {
        let Some(shared) = self.0.upgrade() else {
            return false;
        };
        let mut progress = shared.progress.borrow_mut();
        let Progress::Pending {
            cancel, requested, ..
        } = &mut *progress
        else {
            return false;
        };
        if !matches!(cancel, Cancellation::Unregistered) {
            return false;
        }
        if requested.is_some() {
            *cancel = Cancellation::Sent;
            drop(progress);
            callback();
        } else {
            *cancel = Cancellation::Armed(Box::new(callback));
        }
        true
    }

    /// Acknowledges native teardown and delivers at most one result.
    /// A previously requested cancellation takes precedence over this answer.
    #[must_use]
    pub fn complete(&self, answer: Answer) -> bool {
        self.0
            .upgrade()
            .is_some_and(|shared| shared.complete(answer))
    }
}

impl Shared {
    fn cancel(&self, error: ConfirmationError) -> bool {
        let callback = {
            let mut progress = self.progress.borrow_mut();
            let Progress::Pending {
                cancel, requested, ..
            } = &mut *progress
            else {
                return false;
            };
            if requested.is_some() {
                return false;
            }
            *requested = Some(error.clone());
            std::mem::replace(cancel, Cancellation::Sent)
        };
        if let Cancellation::Armed(callback) = callback {
            callback();
        } else {
            self.complete(Err(error));
        }
        true
    }

    fn complete(&self, answer: Answer) -> bool {
        let (waiter, cancel) = {
            let mut progress = self.progress.borrow_mut();
            if !matches!(*progress, Progress::Pending { .. }) {
                return false;
            }
            let Progress::Pending {
                waiter,
                cancel,
                requested,
            } = std::mem::replace(&mut *progress, Progress::Taken)
            else {
                unreachable!("the pending state is held exclusively")
            };
            *progress = Progress::Ready(requested.map_or(answer, Err));
            (waiter, cancel)
        };
        if let Some(slot) = self.slot.upgrade() {
            let removed = {
                let mut current = slot.current.borrow_mut();
                if current
                    .as_ref()
                    .is_some_and(|current| current.token == self.token)
                {
                    current.take()
                } else {
                    None
                }
            };
            if removed.is_some() && !slot.closed.get() {
                slot.notify();
            }
        }
        drop(cancel);
        if let Some(waiter) = waiter {
            waiter.wake();
        }
        true
    }
}
