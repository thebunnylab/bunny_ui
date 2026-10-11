//! Safe scene operations over the native window adapter.

use crate::{App, WindowError, WindowId, WindowParent, WindowVisibility, ffi};

impl App {
    /// Requests activation of a live singleton under window-system policy.
    /// This is a request, not proof of foreground focus. Explicitly hidden
    /// owners must be shown first. macOS and Windows respect native modal scope;
    /// Linux currently returns `Unsupported` while a scene owns a dialog.
    ///
    /// # Errors
    /// Rejects stale/foreign or explicitly hidden lifetimes, unavailable native
    /// activation protocols and native refusal. A refused activation may already
    /// have restored a minimized window.
    pub fn focus(&self, id: WindowId) -> Result<(), WindowError> {
        let slot = self.slot(id).ok_or(WindowError::StaleWindow)?;
        let mut current = Some(id);
        while let Some(window) = current {
            let owner = self.slot(window).ok_or(WindowError::StaleWindow)?;
            if owner.lifecycle.is_hidden() {
                return Err(WindowError::HiddenWindow);
            }
            current = self
                .inner
                .relationships
                .parent(window)?
                .map(WindowParent::window);
        }
        ffi::focus_window(slot.window)
    }

    /// Reads this scene's successfully requested native parent relationship.
    /// Placement and stacking remain subject to window-manager policy.
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign lifetime.
    pub fn parent(&self, id: WindowId) -> Result<Option<WindowParent>, WindowError> {
        self.inner.relationships.parent(id)
    }

    /// Associates two separate top-level scenes using the native parent/owner
    /// mechanism, or detaches with `None`. This does not make a scene modal.
    ///
    /// Closing an owner tears down its descendants, bypassing their close
    /// policies as `quit` does. Cancel/Hide on the owner's close preserves them.
    /// Detach a child first to retain its independent lifetime.
    ///
    /// # Errors
    /// Rejects closed/foreign IDs, cycles, recursive relationship changes and
    /// native failures. A failed relationship request is not published as a
    /// parent. A later visibility error may leave the relationship committed
    /// and descendants partially updated; retry to finish their visibility.
    pub fn set_parent(&self, child: WindowId, parent: Option<WindowId>) -> Result<(), WindowError> {
        let change = self.inner.relationships.change(child, parent)?;
        let child_slot = self.slot(child).ok_or(WindowError::StaleWindow)?;
        let parent_slot = parent
            .map(|id| self.slot(id).ok_or(WindowError::StaleWindow))
            .transpose()?;
        let kind = ffi::set_window_parent(
            child_slot.window,
            parent_slot.as_ref().map(|slot| slot.window),
        )?;
        if let Err(error) = change.commit(kind) {
            // Retirement clears expired owners but can leave a previous live
            // owner intact. Restore that committed relation after reentrancy.
            if let Some(slot) = self.slot(child) {
                let previous = self
                    .parent(child)?
                    .and_then(|parent| self.slot(parent.window()));
                ffi::set_window_parent(slot.window, previous.map(|parent| parent.window))?;
            }
            return Err(error);
        }
        self.refresh_window_visibility(child)?;
        self.refresh_child_visibility(child)
    }

    pub(super) fn close_children(&self, parent: WindowId) {
        for child in self.inner.relationships.children(parent) {
            self.close(child);
        }
    }

    pub(super) fn refresh_window_visibility(&self, id: WindowId) -> Result<(), WindowError> {
        let slot = self.slot(id).ok_or(WindowError::StaleWindow)?;
        let owner_hidden = self
            .parent(id)?
            .and_then(|parent| self.slot(parent.window()))
            .is_some_and(|parent| parent.lifecycle.is_hidden());
        slot.lifecycle.set_owner_hidden(owner_hidden);
        let visibility = if slot.lifecycle.is_hidden() {
            WindowVisibility::Hidden
        } else {
            WindowVisibility::Shown
        };
        ffi::set_window_visibility(slot.window, visibility)?;
        if visibility == WindowVisibility::Shown {
            slot.runtime.request_frame();
            ffi::wake_from_any_thread();
        }
        Ok(())
    }

    pub(super) fn refresh_child_visibility(&self, id: WindowId) -> Result<(), WindowError> {
        for child in self.inner.relationships.children(id) {
            self.refresh_window_visibility(child)?;
            self.refresh_child_visibility(child)?;
        }
        Ok(())
    }
}
