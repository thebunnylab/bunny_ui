//! Configured Wayland geometry and the X11 geometry adapter.
use super::{AppEvent, HANDLER, dispatch_at, is_x11, window_at, window_ref, with_client};
use bunny_ui::window::{
    GeometryEvents, WindowError, WindowGeometry, WindowId, WindowPlacement, WindowSize,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
};

thread_local! {
    static SOURCES: RefCell<HashMap<usize, Rc<GeometryEvents>>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<VecDeque<(usize, bunny_ui::window::GeometryObservation)>> = const { RefCell::new(VecDeque::new()) };
    static DELIVERING: Cell<bool> = const { Cell::new(false) };
}

/// Registers the lifetime before native observations can reach the application.
pub fn register_geometry(window: usize, events: Rc<GeometryEvents>) {
    SOURCES.with(|sources| sources.borrow_mut().insert(window, events));
}

/// A reused native address cannot inherit the previous scene's observations.
pub fn forget_geometry(window: usize, id: WindowId) {
    SOURCES.with(|sources| {
        if sources
            .borrow()
            .get(&window)
            .is_some_and(|events| events.id() == id)
        {
            sources.borrow_mut().remove(&window);
        }
    });
    PENDING.with(|pending| {
        pending
            .borrow_mut()
            .retain(|(_, observation)| observation.id() != id);
    });
}

/// Reads configured/committed facts, never an uncommitted size request.
pub fn window_geometry(window: usize) -> Result<WindowGeometry, WindowError> {
    if is_x11() {
        return crate::x11::window_geometry(
            u32::try_from(window).map_err(|_| WindowError::StaleWindow)?,
        );
    }
    with_client(|client| {
        let win = window_ref(client, window).ok_or(WindowError::StaleWindow)?;
        let (width, height) = win.observed_size.ok_or(WindowError::GeometryUnavailable)?;
        WindowGeometry::new(WindowSize::new(width, height)?, None, win.factor)
    })
}

/// Captures the native fact before deferring user code past a renderer borrow.
pub(super) fn notify(window: usize) {
    let events = SOURCES.with(|sources| sources.borrow().get(&window).cloned());
    if let Some(events) = events
        && let Ok(value) = window_geometry(window)
        && let Some(observation) = events.capture(value)
    {
        PENDING.with(|pending| pending.borrow_mut().push_back((window, observation)));
        drain();
    }
}

/// Presentation can occur inside the global handler. Deliver after it returns.
pub(super) fn drain() {
    struct Delivery;
    impl Drop for Delivery {
        fn drop(&mut self) {
            DELIVERING.with(|flag| flag.set(false));
        }
    }
    if !HANDLER.with(|handler| handler.try_borrow_mut().is_ok())
        || DELIVERING.with(|flag| flag.replace(true))
    {
        return;
    }
    let _delivery = Delivery;
    loop {
        let next = PENDING.with(|pending| pending.borrow_mut().pop_front());
        let Some((window, observation)) = next else {
            break;
        };
        dispatch_at(window, AppEvent::GeometryChanged(observation));
    }
}

/// Whether an xdg configure allows the client to choose an ordinary size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SizeAuthority {
    #[default]
    Client,
    Compositor,
}

impl SizeAuthority {
    pub(super) fn from_states(states: &[u32]) -> Self {
        // xdg_toplevel: maximized, fullscreen, resizing and tiled edges.
        if states.iter().any(|state| matches!(state, 1..=3 | 5..=8)) {
            Self::Compositor
        } else {
            Self::Client
        }
    }
}

fn constrained_size(
    size: WindowSize,
    min: Option<(i32, i32)>,
    max: Option<(i32, i32)>,
) -> Result<(f64, f64), WindowError> {
    let axis = |value: f64, floor: i32, ceiling: i32| {
        let floor = f64::from(floor.max(1));
        let ceiling = if ceiling > 0 {
            f64::from(ceiling)
        } else {
            f64::from(i32::MAX)
        };
        if floor > ceiling {
            return Err(WindowError::InvalidGeometry(
                "native minimum exceeds maximum size",
            ));
        }
        Ok(value.round().clamp(floor, ceiling))
    };
    let min = min.unwrap_or((1, 1));
    let max = max.unwrap_or((0, 0));
    Ok((
        axis(size.width(), min.0, max.0)?,
        axis(size.height(), min.1, max.1)?,
    ))
}

/// Requests a client-chosen ordinary size; global placement is unavailable.
pub fn restore_window_placement(
    window: usize,
    placement: WindowPlacement,
) -> Result<(), WindowError> {
    if is_x11() {
        return crate::x11::restore_window_placement(
            u32::try_from(window).map_err(|_| WindowError::StaleWindow)?,
            placement,
        );
    }
    // The GPU post hook currently also runs after VK_ERROR_OUT_OF_DATE_KHR.
    // T2-ECO-16 supplies real presentation receipts. Until that integration,
    // never turn this ambiguous hook into a successful placement observation.
    let ambiguous_present = crate::vk::active(window);
    with_client(|client| {
        let win = window_at(client, window).ok_or(WindowError::StaleWindow)?;
        if placement.position().is_some() {
            return Err(WindowError::Unsupported(
                "Wayland does not expose global window position",
            ));
        }
        if ambiguous_present {
            return Err(WindowError::Unsupported(
                "restoring client-chosen Wayland size is unavailable with this presenter",
            ));
        }
        if win.size_authority == SizeAuthority::Compositor {
            return Err(WindowError::Unsupported(
                "the compositor currently controls this window's size",
            ));
        }
        // The next actual buffer commit observes this size. Merely changing the
        // layout target is not an observed result and must not notify persistence.
        win.logical = constrained_size(placement.size(), win.min_size, win.max_size)?;
        Ok(())
    })
}

/// The committed CPU buffer determines the surface size, independently of the
/// mutable layout request. Wayland buffer dimensions and scale are native ints.
pub(super) fn committed_size(width: usize, height: usize, scale: usize) -> Option<(f64, f64)> {
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    let scale = u32::try_from(scale).ok()?;
    if width == 0 || height == 0 || scale == 0 {
        return None;
    }
    Some((
        f64::from(width) / f64::from(scale),
        f64::from(height) / f64::from(scale),
    ))
}

/// A zero configure axis delegates sizing to the client; it does not confirm
/// that the current layout request has reached the compositor. Preserve only
/// a previously observed axis until an actual buffer commit supplies it.
pub(super) fn configured_size(
    staged: Option<(i32, i32)>,
    observed: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    let Some((width, height)) = staged else {
        return observed;
    };
    let width = if width > 0 {
        Some(f64::from(width))
    } else {
        observed.map(|size| size.0)
    }?;
    let height = if height > 0 {
        Some(f64::from(height))
    } else {
        observed.map(|size| size.1)
    }?;
    Some((width, height))
}

/// OpenGL calls this only after a successful buffer swap, with its actual buffer.
pub(super) fn observe_buffer(window: usize, physical: (usize, usize), scale: usize) {
    let observed = committed_size(physical.0, physical.1, scale);
    let changed = with_client(|client| {
        let Some(win) = window_at(client, window) else {
            return false;
        };
        let previous = std::mem::replace(&mut win.observed_size, observed);
        previous != observed
    });
    if changed {
        notify(window);
    }
}

/// Both exact scale and raster lattice matter; 1.25 -> 1.5 retains lattice 2.
pub(super) fn set_scale(lattice: &mut usize, exact: &mut f64, scale: usize, factor: f64) -> bool {
    let changed = (*lattice, *exact) != (scale, factor);
    *lattice = scale;
    *exact = factor;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_size_comes_from_buffer_pixels_and_buffer_scale() {
        assert_eq!(committed_size(721, 480, 2), Some((360.5, 240.0)));
        assert_eq!(committed_size(721, 480, 1), Some((721.0, 480.0)));
        assert_eq!(committed_size(0, 480, 2), None);
        assert_eq!(committed_size(721, 480, 0), None);
    }

    #[test]
    fn client_chosen_configure_axes_do_not_confirm_an_uncommitted_request() {
        assert_eq!(configured_size(None, None), None);
        assert_eq!(configured_size(Some((0, 0)), None), None);
        assert_eq!(configured_size(Some((400, 0)), None), None);
        assert_eq!(
            configured_size(Some((400, 260)), None),
            Some((400.0, 260.0))
        );
        let prior = Some((320.0, 200.0));
        assert_eq!(configured_size(None, prior), prior);
        assert_eq!(configured_size(Some((0, 0)), prior), prior);
        assert_eq!(configured_size(Some((400, 0)), prior), Some((400.0, 200.0)));
        assert_eq!(configured_size(Some((0, 260)), prior), Some((320.0, 260.0)));
    }

    #[test]
    fn exact_scale_change_notifies_without_a_lattice_change() {
        let (mut lattice, mut exact) = (2, 1.25);
        assert!(set_scale(&mut lattice, &mut exact, 2, 1.5));
        assert_eq!((lattice, exact), (2, 1.5));
        assert!(!set_scale(&mut lattice, &mut exact, 2, 1.5));
        assert!(set_scale(&mut lattice, &mut exact, 1, 1.0));
    }

    #[test]
    fn compositor_states_reject_client_size_changes() {
        assert_eq!(SizeAuthority::from_states(&[4, 9]), SizeAuthority::Client);
        for state in [1, 2, 3, 5, 6, 7, 8] {
            assert_eq!(
                SizeAuthority::from_states(&[4, state]),
                SizeAuthority::Compositor
            );
        }
    }

    #[test]
    fn client_size_obeys_both_axes_of_native_limits() {
        let size = WindowSize::new(720.4, 480.6).unwrap();
        assert_eq!(constrained_size(size, None, None).unwrap(), (720.0, 481.0));
        assert_eq!(
            constrained_size(size, Some((800, 100)), Some((0, 400))).unwrap(),
            (800.0, 400.0)
        );
        assert!(constrained_size(size, Some((800, 100)), Some((700, 400))).is_err());
    }
}
