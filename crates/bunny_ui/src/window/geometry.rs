//! Validated placement values and ordered, reentrancy-safe native observations.
//!
//! Shell adapters own native coordinates and capture observations. This module
//! keeps invalid persisted values and retired subscriptions out of that boundary.

use super::{WindowError, WindowId};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::{Rc, Weak},
};

/// The native desktop coordinate space of a content-area position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopSpace {
    /// `AppKit` desktop points, converted to a top-left origin and downward y.
    AppKitPoints,
    /// Win32 virtual-screen physical pixels under per-monitor DPI awareness.
    Win32Pixels,
    /// X11 root-window pixels, without assuming per-monitor fractional scaling.
    X11Pixels,
}

/// A finite, positive content size in the scene's logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSize {
    width: f64,
    height: f64,
}

impl WindowSize {
    /// Validates both dimensions before a persisted value reaches a native API.
    ///
    /// # Errors
    /// Rejects non-finite, non-positive or signed-32-bit-unrepresentable sizes.
    pub fn new(width: f64, height: f64) -> Result<Self, WindowError> {
        if [width, height]
            .into_iter()
            .all(|value| value.is_finite() && value > 0.0 && value <= f64::from(i32::MAX))
        {
            Ok(Self { width, height })
        } else {
            Err(WindowError::InvalidGeometry(
                "content size must be finite, positive and representable",
            ))
        }
    }

    /// The logical content width.
    #[must_use]
    pub const fn width(self) -> f64 {
        self.width
    }

    /// The logical content height.
    #[must_use]
    pub const fn height(self) -> f64 {
        self.height
    }
}

/// Content top-left in the tagged desktop space; negative origins are valid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DesktopPosition {
    x: f64,
    y: f64,
    space: DesktopSpace,
}

impl DesktopPosition {
    /// Constructs a position without guessing a monitor's coordinate scale.
    ///
    /// # Errors
    /// Rejects non-finite coordinates and values outside the signed32-bit range.
    pub fn new(x: f64, y: f64, space: DesktopSpace) -> Result<Self, WindowError> {
        if [x, y].into_iter().all(|value| {
            value.is_finite() && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value)
        }) {
            Ok(Self { x, y, space })
        } else {
            Err(WindowError::InvalidGeometry(
                "desktop position must be finite and representable",
            ))
        }
    }

    /// Distance right of the primary desktop origin in the tagged units.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    /// Distance down from the primary desktop origin in the tagged units.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }

    /// The native coordinate system; it must match the restoring backend.
    #[must_use]
    pub const fn space(self) -> DesktopSpace {
        self.space
    }
}

/// A placement request. `None` leaves global positioning to the window system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPlacement {
    size: WindowSize,
    position: Option<DesktopPosition>,
}

impl WindowPlacement {
    /// Combines already-validated content size and optional desktop position.
    #[must_use]
    pub const fn new(size: WindowSize, position: Option<DesktopPosition>) -> Self {
        Self { size, position }
    }

    /// The requested content size in logical points.
    #[must_use]
    pub const fn size(self) -> WindowSize {
        self.size
    }

    /// The requested content top-left, if the caller requests global placement.
    #[must_use]
    pub const fn position(self) -> Option<DesktopPosition> {
        self.position
    }
}

/// Native/configured content geometry, not proof of visible pixels or focus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    placement: WindowPlacement,
    scale: f64,
}

impl WindowGeometry {
    /// Validates an adapter's exact scale along with its geometry value.
    ///
    /// # Errors
    /// Rejects non-finite or non-positive scale factors.
    pub fn new(
        size: WindowSize,
        position: Option<DesktopPosition>,
        scale: f64,
    ) -> Result<Self, WindowError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(WindowError::InvalidGeometry(
                "scale factor must be finite and positive",
            ));
        }
        Ok(Self {
            placement: WindowPlacement::new(size, position),
            scale,
        })
    }

    /// The observed logical content size.
    #[must_use]
    pub const fn size(self) -> WindowSize {
        self.placement.size
    }

    /// The observed global content position; Wayland does not expose one.
    #[must_use]
    pub const fn position(self) -> Option<DesktopPosition> {
        self.placement.position
    }

    /// The exact output scale, including fractional scales.
    #[must_use]
    pub const fn scale_factor(self) -> f64 {
        self.scale
    }

    /// Values suitable for persistence; restoring them is a separate request.
    #[must_use]
    pub const fn placement(self) -> WindowPlacement {
        self.placement
    }
}

type Handler = Rc<RefCell<dyn FnMut(WindowId, WindowGeometry)>>;

/// Backend subscription state belonging to one non-recycled window lifetime.
#[doc(hidden)]
pub struct GeometryEvents {
    id: WindowId,
    retired: Cell<bool>,
    delivering: Cell<bool>,
    handler: RefCell<Option<Handler>>,
    pending: RefCell<VecDeque<WindowGeometry>>,
    last: Cell<Option<WindowGeometry>>,
}

/// A native snapshot bound to the scene and subscription present at capture.
#[doc(hidden)]
#[derive(Clone)]
pub struct GeometryObservation {
    id: WindowId,
    geometry: WindowGeometry,
    handler: Weak<RefCell<dyn FnMut(WindowId, WindowGeometry)>>,
}

impl GeometryObservation {
    /// The non-recycled scene identity used by deferred native routing.
    #[must_use]
    pub const fn id(&self) -> WindowId {
        self.id
    }
}

impl GeometryEvents {
    /// The lifetime registered by this observation source.
    #[must_use]
    pub const fn id(&self) -> WindowId {
        self.id
    }

    /// Captures a subscription without retaining its user callback.
    #[must_use]
    pub fn capture(&self, geometry: WindowGeometry) -> Option<GeometryObservation> {
        if self.retired.get() {
            return None;
        }
        self.handler
            .borrow()
            .as_ref()
            .map(|handler| GeometryObservation {
                id: self.id,
                geometry,
                handler: Rc::downgrade(handler),
            })
    }

    /// Rejects observations captured for a replaced or retired subscription.
    pub fn observe_captured(&self, observation: &GeometryObservation) {
        let current = observation.id == self.id
            && self
                .handler
                .borrow()
                .as_ref()
                .is_some_and(|handler| observation.handler.ptr_eq(&Rc::downgrade(handler)));
        if current {
            self.observe(observation.geometry);
        }
    }

    pub(super) const fn new(id: WindowId) -> Self {
        Self {
            id,
            retired: Cell::new(false),
            delivering: Cell::new(false),
            handler: RefCell::new(None),
            pending: RefCell::new(VecDeque::new()),
            last: Cell::new(None),
        }
    }

    /// Replaces a subscription, discarding observations queued for its predecessor.
    ///
    /// # Errors
    /// A retired subscription cannot be revived under its old window identity.
    pub fn set_handler(
        &self,
        handler: impl FnMut(WindowId, WindowGeometry) + 'static,
    ) -> Result<(), WindowError> {
        if self.retired.get() {
            return Err(WindowError::StaleWindow);
        }
        self.clear_handler();
        *self.handler.borrow_mut() = Some(Rc::new(RefCell::new(handler)));
        Ok(())
    }

    /// Stops notification and discards pending events without holding callback borrows.
    pub fn clear_handler(&self) {
        let previous = self.handler.borrow_mut().take();
        self.pending.borrow_mut().clear();
        self.last.set(None);
        drop(previous);
    }

    /// Delivers snapshots in native order; reentrant observations are queued.
    pub fn observe(&self, geometry: WindowGeometry) {
        struct Delivery<'a>(&'a Cell<bool>);
        impl Drop for Delivery<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        if self.retired.get()
            || self.last.replace(Some(geometry)) == Some(geometry)
            || self.handler.borrow().is_none()
        {
            return;
        }
        self.pending.borrow_mut().push_back(geometry);
        if self.delivering.replace(true) {
            return;
        }
        let _delivery = Delivery(&self.delivering);
        loop {
            let next = self.pending.borrow_mut().pop_front();
            let Some(value) = next else { break };
            let handler = self.handler.borrow().clone();
            if let Some(handler) = handler {
                (handler.borrow_mut())(self.id, value);
            }
        }
    }

    /// Permanently retires the subscription before its native address can be reused.
    pub fn retire(&self) {
        self.retired.set(true);
        self.clear_handler();
    }
}
