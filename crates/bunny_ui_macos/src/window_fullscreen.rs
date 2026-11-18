//! A scene's window on a screen of its own: native full screen, and
//! presentation for the one thing an app shows over all of it.
use crate::{App, FullscreenEvent, FullscreenMode, WindowError, WindowId, ffi};

impl App {
    /// Asks the window into a mode: the platform's full screen (a Space of
    /// its own, the system's bars hidden until the pointer reaches the
    /// top), presentation (the same, with the toolbar hidden too and
    /// Escape as the way back), or back among the other windows.
    ///
    /// The trip is animated and its events say when it lands; an ask that
    /// arrives while the window is still moving is answered when it does.
    /// A window already in the asked mode hears nothing, and switching
    /// between full screen and presentation needs no trip at all.
    ///
    /// ```ignore
    /// // a video element asked for the whole screen
    /// app.set_fullscreen(id, FullscreenMode::Presentation)?;
    /// ```
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign ID and `HiddenWindow`
    /// for a hidden one.
    pub fn set_fullscreen(&self, id: WindowId, mode: FullscreenMode) -> Result<(), WindowError> {
        let slot = self.slot(id).ok_or(WindowError::StaleWindow)?;
        if slot.lifecycle.is_hidden() && mode.is_fullscreen() {
            return Err(WindowError::HiddenWindow);
        }
        ffi::set_window_fullscreen(slot.window, mode)
    }

    /// How the window stands now. A window on its way in already reports
    /// the mode it is travelling to; one on its way out, `Windowed`.
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign ID.
    pub fn fullscreen(&self, id: WindowId) -> Result<FullscreenMode, WindowError> {
        let slot = self.slot(id).ok_or(WindowError::StaleWindow)?;
        ffi::window_fullscreen(slot.window)
    }

    /// Subscribes to the window's trips into and out of full screen,
    /// whoever asked: the app, the green button, Escape in a presentation,
    /// the View menu's item. Callbacks may ask for another mode, replace
    /// this handler or close the window; an event raised meanwhile waits
    /// for the callback to return.
    ///
    /// ```ignore
    /// app.on_fullscreen_changed(id, move |_, event| match event {
    ///     FullscreenEvent::Entered => page.fullscreen_entered(),
    ///     FullscreenEvent::Exited => page.fullscreen_exited(),
    ///     _ => {}
    /// })?;
    /// ```
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign lifetime.
    pub fn on_fullscreen_changed(
        &self,
        id: WindowId,
        handler: impl FnMut(WindowId, FullscreenEvent) + 'static,
    ) -> Result<(), WindowError> {
        self.slot(id)
            .ok_or(WindowError::StaleWindow)?
            .lifecycle
            .fullscreen_events()
            .set_handler(handler)
    }

    /// Removes the observer and discards any events queued for it.
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign lifetime.
    pub fn clear_fullscreen_handler(&self, id: WindowId) -> Result<(), WindowError> {
        self.slot(id)
            .ok_or(WindowError::StaleWindow)?
            .lifecycle
            .fullscreen_events()
            .clear_handler();
        Ok(())
    }
}
