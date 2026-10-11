//! Capture `AppKit` geometry at notification time and restore into live work areas.

use super::ObjcBool;
use super::{
    AppEvent, CGPoint, CGRect, CGSize, Id, Sel, WINDOWS, class, dispatch_from, msg_f64, msg_id,
    msg_rect, msg_rect_rect, msg_u64, msg_void_rect_bool, sel,
};
use bunny_ui::window::{
    DesktopPosition, DesktopSpace, DialogGeometrySession, GeometryEvents, WindowError,
    WindowGeometry, WindowId, WindowPlacement, WindowSize,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};

#[allow(
    clashing_extern_declarations,
    reason = "Objective-C selectors have concrete result and argument ABIs"
)]
#[link(name = "objc")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn size(value: Id, selector: Sel) -> CGSize;
    #[link_name = "objc_msgSend"]
    fn at(value: Id, selector: Sel, index: u64) -> Id;
}

thread_local! {
    static SOURCES: RefCell<HashMap<usize, Rc<GeometryEvents>>> = RefCell::new(HashMap::new());
    static DIALOGS: RefCell<HashMap<usize, DialogSource>> = RefCell::new(HashMap::new());
}

struct DialogSource {
    parent: usize,
    view: Id,
    session: Weak<DialogGeometrySession>,
}

pub(super) fn bind_dialog(
    window: usize,
    view: Id,
    parent: usize,
    session: Option<Rc<DialogGeometrySession>>,
) -> bool {
    let changed = DIALOGS.with(|sources| {
        let mut sources = sources.borrow_mut();
        if let Some(session) = &session {
            let weak = Rc::downgrade(session);
            if sources
                .get(&window)
                .is_some_and(|source| source.session.ptr_eq(&weak))
            {
                return false;
            }
            sources.insert(
                window,
                DialogSource {
                    parent,
                    view,
                    session: weak,
                },
            );
            true
        } else {
            sources.remove(&window);
            false
        }
    });
    if changed && let Some(session) = session {
        if let Some(placement) = session.take_restoration()
            && let Err(error) = super::lend_hand(|| restore(window, view, placement))
            && let Some(event) = session.capture(Err(error))
        {
            super::queue_dialog_geometry(event);
        }
        notify_dialog(window);
    }
    changed
}

pub(super) fn notify_dialog(window: usize) {
    let source = DIALOGS.with(|sources| {
        sources.borrow().get(&window).and_then(|source| {
            source
                .session
                .upgrade()
                .map(|session| (source.view, session))
        })
    });
    if let Some((view, session)) = source
        && let Some(event) = session.capture(geometry(window, view))
    {
        super::queue_dialog_geometry(event);
    }
}

/// Binds observations to a stable lifetime before the scene starts delivering events.
pub fn register_geometry(window: usize, events: Rc<GeometryEvents>) {
    SOURCES.with(|sources| {
        sources.borrow_mut().insert(window, events);
    });
}

/// Removes only the retiring lifetime, including when an address has been reused.
pub fn forget_geometry(window: usize, id: WindowId) {
    SOURCES.with(|sources| {
        let mut sources = sources.borrow_mut();
        if sources.get(&window).is_some_and(|events| events.id() == id) {
            sources.remove(&window);
            DIALOGS.with(|dialogs| {
                dialogs
                    .borrow_mut()
                    .retain(|_, source| source.parent != window)
            });
        }
    });
}

fn view(window: usize) -> Result<Id, WindowError> {
    WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .find(|(id, _, _)| *id == window)
            .map(|(_, view, _)| *view)
            .ok_or(WindowError::StaleWindow)
    })
}

fn primary_frame() -> Result<CGRect, WindowError> {
    let frame = unsafe {
        let screens = msg_id(class("NSScreen"), sel("screens"));
        let primary = msg_id(screens, sel("firstObject"));
        if primary.is_null() {
            return Err(WindowError::GeometryUnavailable);
        }
        msg_rect(primary, sel("frame"))
    };
    Ok(frame)
}

/// Reads the real content viewport and screen position without emitting an event.
pub fn window_geometry(window: usize) -> Result<WindowGeometry, WindowError> {
    geometry(window, view(window)?)
}

fn geometry(window: usize, view: Id) -> Result<WindowGeometry, WindowError> {
    let primary = primary_frame()?;
    unsafe {
        let bounds = msg_rect(view, sel("bounds"));
        let size = WindowSize::new(bounds.size.width, bounds.size.height)
            .map_err(|_| WindowError::GeometryUnavailable)?;
        let content = msg_rect_rect(
            window as Id,
            sel("convertRectToScreen:"),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: bounds.size,
            },
        );
        let position = DesktopPosition::new(
            content.origin.x - primary.origin.x,
            primary.origin.y + primary.size.height - content.origin.y - content.size.height,
            DesktopSpace::AppKitPoints,
        )?;
        WindowGeometry::new(
            size,
            Some(position),
            msg_f64(window as Id, sel("backingScaleFactor")),
        )
    }
}

/// Captures before native event deferral; a later window cannot inherit this ID.
pub(super) fn notify(window: usize) {
    let events = SOURCES.with(|sources| sources.borrow().get(&window).cloned());
    if let Some(events) = events
        && let Ok(geometry) = window_geometry(window)
        && let Some(observation) = events.capture(geometry)
    {
        dispatch_from(window, AppEvent::GeometryChanged(observation));
    }
}

fn fit(frame: CGRect, work: CGRect, minimum: CGSize) -> CGRect {
    let width = frame.size.width.min(work.size.width).max(minimum.width);
    let height = frame.size.height.min(work.size.height).max(minimum.height);
    let x = frame
        .origin
        .x
        .min(work.origin.x + work.size.width - width)
        .max(work.origin.x);
    // AppKit y points up: when a minimum cannot fit, keep the caption at the top.
    let top = (frame.origin.y + frame.size.height)
        .max(work.origin.y + height)
        .min(work.origin.y + work.size.height);
    CGRect {
        origin: CGPoint { x, y: top - height },
        size: CGSize { width, height },
    }
}

fn score(frame: CGRect, screen: CGRect) -> (f64, f64) {
    let overlap_x = (frame.origin.x + frame.size.width).min(screen.origin.x + screen.size.width)
        - frame.origin.x.max(screen.origin.x);
    let overlap_y = (frame.origin.y + frame.size.height).min(screen.origin.y + screen.size.height)
        - frame.origin.y.max(screen.origin.y);
    let intersection = overlap_x.max(0.0) * overlap_y.max(0.0);
    let distance = overlap_x
        .min(0.0)
        .mul_add(overlap_x.min(0.0), overlap_y.min(0.0).powi(2));
    (intersection, -distance)
}

/// Requests a content frame fitted to a currently available screen, without activation.
pub fn restore_window_placement(
    window: usize,
    placement: WindowPlacement,
) -> Result<(), WindowError> {
    restore(window, view(window)?, placement)
}

fn restore(window: usize, view: Id, placement: WindowPlacement) -> Result<(), WindowError> {
    if placement
        .position()
        .is_some_and(|position| position.space() != DesktopSpace::AppKitPoints)
    {
        return Err(WindowError::InvalidGeometry(
            "desktop position is not in AppKit points",
        ));
    }
    let position = match placement.position() {
        Some(position) => position,
        None => geometry(window, view)?
            .position()
            .ok_or(WindowError::GeometryUnavailable)?,
    };
    let primary = primary_frame()?;
    let content = CGRect {
        origin: CGPoint {
            x: primary.origin.x + position.x(),
            y: primary.origin.y + primary.size.height - position.y() - placement.size().height(),
        },
        size: CGSize {
            width: placement.size().width(),
            height: placement.size().height(),
        },
    };
    unsafe {
        let frame = msg_rect_rect(window as Id, sel("frameRectForContentRect:"), content);
        let screens = msg_id(class("NSScreen"), sel("screens"));
        let work = (0..msg_u64(screens, sel("count")))
            .map(|index| at(screens, sel("objectAtIndex:"), index))
            .map(|screen| {
                (
                    score(frame, msg_rect(screen, sel("frame"))),
                    msg_rect(screen, sel("visibleFrame")),
                )
            })
            .max_by(|left, right| {
                left.0
                    .0
                    .total_cmp(&right.0.0)
                    .then_with(|| left.0.1.total_cmp(&right.0.1))
            })
            .map(|(_, work)| work)
            .ok_or(WindowError::GeometryUnavailable)?;
        let target = fit(frame, work, size(window as Id, sel("minSize")));
        msg_void_rect_bool(window as Id, sel("setFrame:display:"), target, ObjcBool::YES);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }

    #[test]
    fn missing_monitor_and_oversized_bounds_keep_the_caption_reachable() {
        let work = rect(-1440.0, 50.0, 1440.0, 850.0);
        let target = fit(
            rect(5000.0, 8000.0, 2000.0, 1200.0),
            work,
            CGSize {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(
            (
                target.origin.x,
                target.origin.y,
                target.size.width,
                target.size.height
            ),
            (-1440.0, 50.0, 1440.0, 850.0)
        );
        let target = fit(
            rect(5000.0, -8000.0, 2000.0, 1200.0),
            work,
            CGSize {
                width: 1600.0,
                height: 1000.0,
            },
        );
        assert_eq!(target.origin.x, -1440.0);
        assert_eq!(target.origin.y + target.size.height, 900.0);
    }

    #[test]
    fn saved_bounds_choose_intersection_before_nearest_distance() {
        let left = rect(-1440.0, 0.0, 1440.0, 900.0);
        let right = rect(0.0, 0.0, 1920.0, 1080.0);
        assert!(
            score(rect(-1200.0, 100.0, 400.0, 300.0), left).0
                > score(rect(-1200.0, 100.0, 400.0, 300.0), right).0
        );
        assert!(
            score(rect(-2200.0, 100.0, 400.0, 300.0), left).1
                > score(rect(-2200.0, 100.0, 400.0, 300.0), right).1
        );
    }
}
