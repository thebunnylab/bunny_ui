//! Keep a framework confirmation pending until its native alert window is gone.
//!
//! ## Wiring
//! Linux wraps its retained scene in [`FrameworkConfirmation::host`]. The shell
//! marks the token presented when it creates the window and acknowledges the
//! same token after teardown. Closing the view alone cannot resolve the future.

use std::cell::RefCell;
use std::rc::Rc;

use crate::action::ALERT_DEFAULT;
use crate::layout::AlertSpec;
use crate::prelude::*;
use crate::words::{Word, Words};

use super::{
    Confirmation, ConfirmationError, ConfirmationKind, ConfirmationRequest, ConfirmationResponse,
    ConfirmationSlot, ConfirmationSpec, WindowId,
};

#[derive(Clone, Copy)]
enum Phase {
    Queued,
    Presented,
    Closing(ConfirmationResponse),
}

struct Presentation {
    request: ConfirmationRequest,
    phase: Phase,
}

struct Presenter {
    slot: ConfirmationSlot,
    changed: State<Option<(usize, bool)>>,
    current: RefCell<Option<Presentation>>,
}

impl Drop for Presenter {
    fn drop(&mut self) {
        self.slot.owner_closed();
        if let Some(presentation) = self.current.get_mut().take() {
            let _ = presentation
                .request
                .complete(Err(ConfirmationError::OwnerClosed));
        }
    }
}

/// Retained alert adapter with explicit native presentation acknowledgement.
#[doc(hidden)]
#[derive(Clone)]
pub struct FrameworkConfirmation(Rc<Presenter>);

impl FrameworkConfirmation {
    /// Creates the presenter alongside a native window lifetime.
    #[must_use]
    pub fn new(owner: WindowId) -> Self {
        Self(Rc::new(Presenter {
            slot: ConfirmationSlot::new(owner, ConfirmationKind::FrameworkAlert),
            changed: State::new(None),
            current: RefCell::new(None),
        }))
    }

    /// Wraps a retained root without changing its shape between requests.
    pub fn host(&self, root: impl View) -> impl View {
        Host {
            presenter: self.clone(),
            root,
        }
    }

    /// Reserves this owner and asks its retained view to show the question.
    ///
    /// # Errors
    /// Rejects a retired owner or another outstanding native presentation.
    pub fn start(&self, spec: ConfirmationSpec) -> Result<Confirmation, ConfirmationError> {
        let (confirmation, request) = self.0.slot.start(spec)?;
        let token = request.token();
        let weak = Rc::downgrade(&self.0);
        self.0.current.replace(Some(Presentation {
            request: request.clone(),
            phase: Phase::Queued,
        }));
        request.on_cancel(move || {
            if let Some(presenter) = weak.upgrade() {
                Self(presenter).cancel_request(token);
            }
        });
        self.notify();
        Ok(confirmation)
    }

    /// The current never-reused request token, including pending teardown.
    #[must_use]
    pub fn current_token(&self) -> Option<usize> {
        self.0
            .current
            .borrow()
            .as_ref()
            .map(|current| current.request.token())
    }

    /// Records that the shell has created this token's native alert window.
    #[must_use]
    pub fn native_presented(&self, token: usize) -> bool {
        let mut current = self.0.current.borrow_mut();
        if let Some(current) = current.as_mut()
            && current.request.token() == token
            && matches!(current.phase, Phase::Queued)
            && current.request.can_present()
        {
            current.phase = Phase::Presented;
            return true;
        }
        false
    }

    /// Acknowledges native teardown once, ignoring stale windows and callbacks.
    #[must_use]
    pub fn native_closed(&self, token: usize) -> bool {
        self.finish(token, None)
    }

    /// Reports a failed native creation, distinct from a negative answer.
    #[must_use]
    pub fn native_failed(&self, token: usize, error: ConfirmationError) -> bool {
        self.finish(token, Some(error))
    }

    /// Requests cancellation; an existing native window still needs teardown.
    pub fn cancel(&self, error: ConfirmationError) {
        self.0.slot.cancel(error);
    }

    /// Retires the owner and requests teardown before its handle is released.
    pub fn owner_closed(&self) {
        self.0.slot.owner_closed();
    }

    fn finish(&self, token: usize, failure: Option<ConfirmationError>) -> bool {
        let presentation = {
            let mut current = self.0.current.borrow_mut();
            if current
                .as_ref()
                .is_none_or(|current| current.request.token() != token)
            {
                return false;
            }
            current.take()
        };
        let Some(presentation) = presentation else {
            return false;
        };
        let response = match presentation.phase {
            Phase::Closing(response) => response,
            Phase::Queued | Phase::Presented => ConfirmationResponse::Dismissed,
        };
        self.notify();
        presentation
            .request
            .complete(failure.map_or(Ok(response), Err))
    }

    fn cancel_request(&self, token: usize) {
        let queued = self.0.current.borrow().as_ref().is_some_and(|current| {
            current.request.token() == token && matches!(current.phase, Phase::Queued)
        });
        if queued {
            let _ = self.native_closed(token);
        } else {
            self.answer(token, ConfirmationResponse::Dismissed);
        }
    }

    fn answer(&self, token: usize, response: ConfirmationResponse) {
        let changed = {
            let mut current = self.0.current.borrow_mut();
            if let Some(current) = current.as_mut()
                && current.request.token() == token
                && !matches!(current.phase, Phase::Closing(_))
            {
                current.phase = Phase::Closing(response);
                true
            } else {
                false
            }
        };
        if changed {
            self.notify();
        }
    }

    fn wants(&self, token: Option<usize>) -> bool {
        let _ = self.0.changed.get();
        self.0.current.borrow().as_ref().is_some_and(|current| {
            Some(current.request.token()) == token
                && !matches!(current.phase, Phase::Closing(_))
                && current.request.can_present()
        })
    }

    fn notify(&self) {
        let state = self.0.current.borrow().as_ref().map(|current| {
            (
                current.request.token(),
                !matches!(current.phase, Phase::Closing(_)),
            )
        });
        self.0.changed.set(state);
    }
}

#[derive(Clone)]
struct Host<V> {
    presenter: FrameworkConfirmation,
    root: V,
}

impl<V: View> Component for Host<V> {
    fn body(self) -> impl View {
        let _ = self.presenter.0.changed.get();
        let question = self
            .presenter
            .0
            .current
            .borrow()
            .as_ref()
            .and_then(|current| {
                current
                    .request
                    .spec()
                    .map(|spec| (current.request.token(), spec))
            });
        let token = question.as_ref().map(|(token, _)| *token);
        let title = question
            .as_ref()
            .map_or("", |(_, spec)| spec.question())
            .to_owned();
        let getter = self.presenter.clone();
        let dismiss = self.presenter.clone();
        let content = self.presenter;
        let mut spec = AlertSpec::new(title, 420.0);
        // The allocator starts at one and refuses wraparound, so every
        // allocated request can be represented as a nonzero token.
        spec.confirmation_token = token.map(|token| {
            std::num::NonZeroUsize::new(token).expect("confirmation tokens start at one")
        });
        tuple((self.root,)).alert(
            Binding::new(
                move || getter.wants(token),
                move |open| {
                    if !open && let Some(token) = token {
                        dismiss.answer(token, ConfirmationResponse::Dismissed);
                    }
                },
            ),
            spec,
            move |context| {
                let Some((token, spec)) = question.as_ref() else {
                    return erased(text(""));
                };
                let token = *token;
                let words = Words::for_locale(&context.environment::<Locale>());
                let yes = content.clone();
                let no = content.clone();
                let default = content.clone();
                erased(
                    crate::vstack!(
                        text(spec.question())
                            .font_size(18.0)
                            .foreground_color(crate::theme::fg()),
                        text(spec.detail()).foreground_color(crate::theme::fg_secondary()),
                        crate::hstack!(
                            button(text(words.get(Word::No).into_owned()), move || no
                                .answer(token, ConfirmationResponse::No)),
                            button(text(words.get(Word::Yes).into_owned()), move || yes
                                .answer(token, ConfirmationResponse::Yes)),
                        )
                        .spacing(12.0),
                    )
                    .spacing(16.0)
                    .padding_length(24.0)
                    .on_action(ALERT_DEFAULT, move || {
                        default.answer(token, ConfirmationResponse::No);
                    }),
                )
            },
        )
    }
}
