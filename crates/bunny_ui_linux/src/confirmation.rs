//! Framework alerts resolve after native teardown and outside dispatch borrows.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

use bunny_ui::layout::OverlaySurface;
use bunny_ui::window::{Confirmation, ConfirmationError, ConfirmationSpec, FrameworkConfirmation};

use crate::{App, DialogWindow, WindowError, WindowId, WindowVisibility, ffi};

thread_local! {
    static CLOSED: RefCell<VecDeque<Receipt>> = const { RefCell::new(VecDeque::new()) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

struct Receipt {
    presenter: FrameworkConfirmation,
    token: usize,
    result: Result<(), ConfirmationError>,
}

pub fn closed(presenter: &FrameworkConfirmation, token: usize) {
    receipt(presenter, token, Ok(()));
}

pub fn failed(presenter: &FrameworkConfirmation, token: usize, error: ConfirmationError) {
    receipt(presenter, token, Err(error));
}

fn receipt(presenter: &FrameworkConfirmation, token: usize, result: Result<(), ConfirmationError>) {
    CLOSED.with(|queue| {
        queue.borrow_mut().push_back(Receipt {
            presenter: presenter.clone(),
            token,
            result,
        });
    });
    ffi::wake_from_any_thread();
}

struct Draining;
impl Drop for Draining {
    fn drop(&mut self) {
        DRAINING.set(false);
    }
}

pub fn drain() {
    if DRAINING.replace(true) {
        return;
    }
    let _draining = Draining;
    loop {
        let receipt = CLOSED.with(|queue| queue.borrow_mut().pop_front());
        let Some(receipt) = receipt else { break };
        match receipt.result {
            Ok(()) => {
                let _ = receipt.presenter.native_closed(receipt.token);
            }
            Err(error) => {
                let _ = receipt.presenter.native_failed(receipt.token, error);
            }
        }
    }
}

/// Hide/close can bypass drawing; retire the matching native window explicitly.
pub fn dismiss(
    presenter: &FrameworkConfirmation,
    dialogs: &RefCell<HashMap<String, DialogWindow>>,
) {
    let Some(token) = presenter.current_token() else {
        return;
    };
    let retired = {
        let mut dialogs = dialogs.borrow_mut();
        dialogs
            .extract_if(|_, dialog| dialog.confirmation == Some(token))
            .map(|(_, dialog)| dialog)
            .collect::<Vec<_>>()
    };
    for dialog in retired {
        dialog.handle.close_panel();
        closed(presenter, token);
    }
}

impl App {
    /// Shows a themed Yes/No alert on a native Linux window owned by this scene.
    /// The framework isolates the owner's input; this is not a GTK message box
    /// or a Wayland OS-modal protocol. The future resolves after native teardown.
    ///
    /// # Errors
    /// Rejects stale, foreign or hidden owners and existing modal presentations.
    pub fn confirm(
        &self,
        id: WindowId,
        spec: ConfirmationSpec,
    ) -> Result<Confirmation, ConfirmationError> {
        let slot = self.slot(id).ok_or(WindowError::StaleWindow)?;
        if slot.lifecycle.is_hidden()
            || ffi::window_visibility(slot.window)? == WindowVisibility::Hidden
        {
            return Err(WindowError::HiddenWindow.into());
        }
        if slot
            .runtime
            .overlays()
            .iter()
            .any(|overlay| match &overlay.surface {
                OverlaySurface::Layer => false,
                OverlaySurface::Window(_) => true,
                OverlaySurface::Alert(spec) => spec.confirmation_token.is_none(),
            })
        {
            return Err(ConfirmationError::AlreadyOpen);
        }
        let confirmation = slot.confirmation.start(spec)?;
        slot.runtime.request_frame();
        ffi::wake_from_any_thread();
        Ok(confirmation)
    }
}

#[cfg(test)]
#[path = "confirmation_tests.rs"]
mod tests;
