//! Observation and restoration of a live scene's native content geometry.
use crate::{App, WindowError, WindowGeometry, WindowId, WindowPlacement, ffi};

impl App {
    /// Reads native/configured geometry independently of any placement request.
    /// This does not prove that the window is visible or focused.
    ///
    /// # Errors
    /// Rejects stale/foreign IDs, unavailable geometry and native query failures.
    pub fn geometry(&self, id: WindowId) -> Result<WindowGeometry, WindowError> {
        let slot = self.slot(id).ok_or(WindowError::StaleWindow)?;
        ffi::window_geometry(slot.window)
    }

    /// Requests content placement under the window system's constraints.
    /// The observed result arrives separately. Native desktop spaces must match;
    /// Wayland rejects global positions; client size restoration currently requires
    /// its CPU or OpenGL presenter. Vulkan restoration awaits reliable presentation receipts.
    /// This does not request activation or
    /// promise to restore maximized/fullscreen state. Native failure may follow
    /// a partially applied request, whose actual geometry remains observable.
    ///
    /// # Errors
    /// Rejects stale/foreign IDs, invalid/cross-space or unsupported placement,
    /// and native failures. Missing saved monitors are handled by the adapter.
    pub fn restore_placement(
        &self,
        id: WindowId,
        placement: WindowPlacement,
    ) -> Result<(), WindowError> {
        let slot = self.slot(id).ok_or(WindowError::StaleWindow)?;
        ffi::restore_window_placement(slot.window, placement)?;
        slot.runtime.request_frame();
        ffi::wake_from_any_thread();
        Ok(())
    }

    /// Subscribes to future native geometry changes without polling.
    /// Callbacks may restore placement, replace this handler or close the scene.
    /// Reentrant observations retain their order; retirement cancels delivery.
    /// Query `geometry` separately if an initial snapshot is needed.
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign lifetime.
    pub fn on_geometry_changed(
        &self,
        id: WindowId,
        handler: impl FnMut(WindowId, WindowGeometry) + 'static,
    ) -> Result<(), WindowError> {
        self.slot(id)
            .ok_or(WindowError::StaleWindow)?
            .lifecycle
            .geometry_events()
            .set_handler(handler)
    }

    /// Removes the observer and discards any observations queued for it.
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign lifetime.
    pub fn clear_geometry_handler(&self, id: WindowId) -> Result<(), WindowError> {
        self.slot(id)
            .ok_or(WindowError::StaleWindow)?
            .lifecycle
            .geometry_events()
            .clear_handler();
        Ok(())
    }
}
