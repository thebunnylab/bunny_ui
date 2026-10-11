//! Observe Win32 client geometry and restore it within current monitor work areas.

use super::{
    AdjustWindowRectExForDpi, AppEvent, ClientToScreen, GetClientRect, GetDpiForWindow,
    GetMonitorInfoW, GetWindowLongW, HANDLER, Handle, Hwnd, MonitorInfo, Point, Rect,
    SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, content_floor, dispatch_at, fit_to_work_area,
    is_top_level, wears_scene_chrome,
};
use bunny_ui::window::{
    DesktopPosition, DesktopSpace, GeometryEvents, WindowError, WindowGeometry, WindowId,
    WindowPlacement, WindowSize,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
};

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn MonitorFromRect(rect: *const Rect, flags: u32) -> Handle;
}
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

thread_local! { static SOURCES: RefCell<HashMap<usize, Rc<GeometryEvents>>> = RefCell::new(HashMap::new()); }
thread_local! {
    static PENDING: RefCell<VecDeque<(Hwnd, bunny_ui::window::GeometryObservation)>> = const { RefCell::new(VecDeque::new()) };
    static DELIVERING: Cell<bool> = const { Cell::new(false) };
    static MUTATING: Cell<bool> = const { Cell::new(false) };
}

/// Associates future observations with a non-recycled scene identity.
pub fn register_geometry(window: usize, events: Rc<GeometryEvents>) {
    SOURCES.with(|sources| {
        sources.borrow_mut().insert(window, events);
    });
}

/// Retires the observation source only if it still names this window lifetime.
pub fn forget_geometry(window: usize, id: WindowId) {
    SOURCES.with(|sources| {
        let mut sources = sources.borrow_mut();
        if sources.get(&window).is_some_and(|events| events.id() == id) {
            sources.remove(&window);
        }
    });
    PENDING.with(|pending| {
        pending
            .borrow_mut()
            .retain(|(_, observation)| observation.id() != id);
    });
}

fn live(window: usize) -> Result<Hwnd, WindowError> {
    let hwnd = Hwnd::from_ne_bytes(window.to_ne_bytes());
    is_top_level(hwnd)
        .then_some(hwnd)
        .ok_or(WindowError::StaleWindow)
}

fn failure(operation: &'static str) -> WindowError {
    WindowError::NativeFailure {
        operation,
        code: i64::from(unsafe { GetLastError() }),
    }
}

/// Reads client pixels, content origin and exact DPI without inventing geometry.
pub fn window_geometry(window: usize) -> Result<WindowGeometry, WindowError> {
    let hwnd = live(window)?;
    let mut rect = Rect::default();
    let mut origin = Point { x: 0, y: 0 };
    unsafe {
        if GetClientRect(hwnd, &raw mut rect) == 0 {
            return Err(failure("GetClientRect"));
        }
        if ClientToScreen(hwnd, &raw mut origin) == 0 {
            return Err(failure("ClientToScreen"));
        }
        let dpi = GetDpiForWindow(hwnd);
        if dpi == 0 {
            return Err(WindowError::GeometryUnavailable);
        }
        let scale = f64::from(dpi) / 96.0;
        let size = WindowSize::new(
            (f64::from(rect.right) - f64::from(rect.left)) / scale,
            (f64::from(rect.bottom) - f64::from(rect.top)) / scale,
        )
        .map_err(|_| WindowError::GeometryUnavailable)?;
        let position = DesktopPosition::new(
            f64::from(origin.x),
            f64::from(origin.y),
            DesktopSpace::Win32Pixels,
        )?;
        WindowGeometry::new(size, Some(position), scale)
    }
}

/// Captures the native fact before the dispatcher can defer its delivery.
pub(super) fn notify(hwnd: Hwnd) {
    let window = usize::from_ne_bytes(hwnd.to_ne_bytes());
    let events = SOURCES.with(|sources| sources.borrow().get(&window).cloned());
    if let Some(events) = events
        && let Ok(geometry) = window_geometry(window)
        && let Some(observation) = events.capture(geometry)
    {
        PENDING.with(|pending| pending.borrow_mut().push_back((hwnd, observation)));
        drain();
    }
}

/// Reentrant Win32 messages must not turn an observed geometry fact into a repaint.
pub(super) fn drain() {
    struct Delivery;
    impl Drop for Delivery {
        fn drop(&mut self) {
            DELIVERING.with(|flag| flag.set(false));
        }
    }
    if MUTATING.with(Cell::get)
        || !HANDLER.with(|handler| handler.try_borrow_mut().is_ok())
        || DELIVERING.with(|flag| flag.replace(true))
    {
        return;
    }
    let _delivery = Delivery;
    loop {
        let next = PENDING.with(|pending| pending.borrow_mut().pop_front());
        let Some((hwnd, observation)) = next else {
            break;
        };
        dispatch_at(hwnd, AppEvent::GeometryChanged(observation));
    }
}

fn valid_frame(rect: Rect) -> Result<Rect, WindowError> {
    if rect
        .right
        .checked_sub(rect.left)
        .is_some_and(|width| width > 0)
        && rect
            .bottom
            .checked_sub(rect.top)
            .is_some_and(|height| height > 0)
    {
        Ok(rect)
    } else {
        Err(WindowError::InvalidGeometry(
            "native frame dimensions are not representable",
        ))
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "The rounded finite value is checked against both i32 bounds before conversion"
)]
fn pixel(value: f64) -> Result<i32, WindowError> {
    let value = value.round();
    if value.is_finite() && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value) {
        Ok(value as i32)
    } else {
        Err(WindowError::InvalidGeometry(
            "scaled bounds exceed native coordinates",
        ))
    }
}

fn frame(hwnd: Hwnd, size: WindowSize, dpi: u32) -> Result<Rect, WindowError> {
    if dpi == 0 {
        return Err(WindowError::GeometryUnavailable);
    }
    let scale = f64::from(dpi) / 96.0;
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: pixel(size.width() * scale)?.max(1),
        bottom: pixel(size.height() * scale)?.max(1),
    };
    if !wears_scene_chrome(hwnd) {
        let (style, extended) = unsafe { (GetWindowLongW(hwnd, -16), GetWindowLongW(hwnd, -20)) };
        if unsafe {
            AdjustWindowRectExForDpi(
                &raw mut rect,
                u32::from_ne_bytes(style.to_ne_bytes()),
                0,
                u32::from_ne_bytes(extended.to_ne_bytes()),
                dpi,
            )
        } == 0
        {
            return Err(failure("AdjustWindowRectExForDpi"));
        }
    }
    valid_frame(rect)
}

fn positioned(frame: Rect, position: DesktopPosition) -> Result<Rect, WindowError> {
    Ok(Rect {
        left: pixel(position.x() + f64::from(frame.left))?,
        top: pixel(position.y() + f64::from(frame.top))?,
        right: pixel(position.x() + f64::from(frame.right))?,
        bottom: pixel(position.y() + f64::from(frame.bottom))?,
    })
}

fn monitor_work(rect: &Rect) -> Result<Rect, WindowError> {
    let monitor = unsafe { MonitorFromRect(rect, 2) }; // MONITOR_DEFAULTTONEAREST
    if monitor == 0 {
        return Err(WindowError::GeometryUnavailable);
    }
    let mut info = MonitorInfo {
        size: u32::try_from(std::mem::size_of::<MonitorInfo>())
            .map_err(|_| WindowError::GeometryUnavailable)?,
        monitor: Rect::default(),
        work: Rect::default(),
        flags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &raw mut info) } == 0 {
        return Err(failure("GetMonitorInfoW"));
    }
    Ok(info.work)
}

fn apply(
    hwnd: Hwnd,
    size: WindowSize,
    position: DesktopPosition,
    dpi: u32,
    work: Rect,
) -> Result<(), WindowError> {
    let rect = positioned(frame(hwnd, size, dpi)?, position)?;
    let minimum = content_floor(hwnd).unwrap_or((1.0, 1.0));
    let floor = frame(
        hwnd,
        WindowSize::new(minimum.0.max(1.0), minimum.1.max(1.0))?,
        dpi,
    )?;
    let rect = fit_to_work_area(rect, work, (floor.width(), floor.height()));
    if unsafe {
        SetWindowPos(
            hwnd,
            0,
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    } == 0
    {
        return Err(failure("SetWindowPos"));
    }
    Ok(())
}

/// Requests placement, fitting a removed monitor's bounds to a current work area.
pub fn restore_window_placement(
    window: usize,
    placement: WindowPlacement,
) -> Result<(), WindowError> {
    struct Mutation(bool);
    impl Drop for Mutation {
        fn drop(&mut self) {
            MUTATING.with(|flag| flag.set(self.0));
        }
    }
    let previous = MUTATING.with(|flag| flag.replace(true));
    let mutation = Mutation(previous);
    let result = restore(window, placement);
    drop(mutation);
    drain();
    result
}

fn restore(window: usize, placement: WindowPlacement) -> Result<(), WindowError> {
    let hwnd = live(window)?;
    if placement
        .position()
        .is_some_and(|position| position.space() != DesktopSpace::Win32Pixels)
    {
        return Err(WindowError::InvalidGeometry(
            "desktop position is not in Win32 pixels",
        ));
    }
    let current = window_geometry(window)?;
    let position = placement
        .position()
        .or_else(|| current.position())
        .ok_or(WindowError::GeometryUnavailable)?;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let work = monitor_work(&positioned(frame(hwnd, placement.size(), dpi)?, position)?)?;
    apply(hwnd, placement.size(), position, dpi, work)?;
    // A cross-monitor move may synchronously change DPI. Recompute content
    // dimensions once at the granted DPI, retaining the selected work area.
    let hwnd = live(window)?;
    let granted_dpi = unsafe { GetDpiForWindow(hwnd) };
    if granted_dpi != 0 && granted_dpi != dpi {
        apply(hwnd, placement.size(), position, granted_dpi, work)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_frame_rejects_overflow_and_empty_dimensions() {
        assert!(
            valid_frame(Rect {
                left: -20,
                top: -30,
                right: 400,
                bottom: 250
            })
            .is_ok()
        );
        assert!(
            valid_frame(Rect {
                left: -20,
                top: 0,
                right: i32::MAX,
                bottom: 100
            })
            .is_err()
        );
        assert!(
            valid_frame(Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 100
            })
            .is_err()
        );
        assert!(pixel(f64::from(i32::MAX) + 1.0).is_err());
        assert_eq!(pixel(-1800.0).unwrap(), -1800);
    }
}
