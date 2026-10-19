//! Native dialog observations use panel leases, never ordinary scene identities.
use super::{geometry_ops, is_x11, with_client};
use bunny_ui::window::{
    DialogGeometrySession, WindowError, WindowGeometry, WindowPlacement, WindowSize,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};

thread_local! {
    static SOURCES: RefCell<HashMap<usize, (usize, Weak<DialogGeometrySession>)>> = RefCell::new(HashMap::new());
}

pub(super) fn bind(index: usize, owner: usize, session: Option<Rc<DialogGeometrySession>>) -> bool {
    let changed = SOURCES.with(|sources| {
        let mut sources = sources.borrow_mut();
        if let Some(session) = &session {
            let weak = Rc::downgrade(session);
            if sources
                .get(&index)
                .is_some_and(|(_, old)| old.ptr_eq(&weak))
            {
                return false;
            }
            sources.insert(index, (owner, weak));
            true
        } else {
            sources.remove(&index);
            false
        }
    });
    if changed && let Some(session) = session {
        if let Some(placement) = session.take_restoration()
            && let Err(error) = restore(index, placement)
            && let Some(event) = session.capture(Err(error))
        {
            geometry_ops::queue_dialog_geometry(event);
        }
        notify(index);
    }
    changed
}

pub(super) fn forget(index: usize) {
    let source = SOURCES.with(|sources| sources.borrow_mut().remove(&index));
    if let Some((_, session)) = source
        && let Some(session) = session.upgrade()
    {
        session.retire();
    }
}

pub(super) fn forget_parent(owner: usize) {
    let indices: Vec<_> = SOURCES.with(|sources| {
        sources
            .borrow()
            .iter()
            .filter_map(|(index, (parent, _))| (*parent == owner).then_some(*index))
            .collect()
    });
    for index in indices {
        forget(index);
    }
}

pub(super) fn notify(index: usize) {
    let session = SOURCES.with(|sources| {
        sources
            .borrow()
            .get(&index)
            .and_then(|(_, session)| session.upgrade())
    });
    if let Some(session) = session {
        // A Wayland surface has no observation until an actual buffer commit.
        // Waiting for its first configure is not an application failure.
        let result = geometry(index);
        if result != Err(WindowError::GeometryUnavailable)
            && let Some(event) = session.capture(result)
        {
            geometry_ops::queue_dialog_geometry(event);
        }
    }
}

fn geometry(index: usize) -> Result<WindowGeometry, WindowError> {
    if is_x11() {
        return crate::x11::dialog_geometry(index);
    }
    with_client(|client| {
        let dialog = client
            .panels
            .get(index)
            .and_then(Option::as_ref)
            .and_then(|panel| panel.role.dialog())
            .ok_or(WindowError::StaleWindow)?;
        dialog.observed.ok_or(WindowError::GeometryUnavailable)
    })
}

fn restore(index: usize, placement: WindowPlacement) -> Result<(), WindowError> {
    if is_x11() {
        return crate::x11::restore_dialog_placement(index, placement);
    }
    if placement.position().is_some() {
        return Err(WindowError::Unsupported(
            "Wayland does not expose global dialog position",
        ));
    }
    with_client(|client| {
        let dialog = client
            .panels
            .get_mut(index)
            .and_then(Option::as_mut)
            .and_then(|panel| panel.role.dialog_mut())
            .ok_or(WindowError::StaleWindow)?;
        if dialog.size_authority == geometry_ops::SizeAuthority::Compositor {
            return Err(WindowError::Unsupported(
                "the compositor currently controls this dialog's size",
            ));
        }
        let width = placement
            .size()
            .width()
            .round()
            .max(dialog.min.0.ceil())
            .min(f64::from(i32::MAX));
        let height = placement
            .size()
            .height()
            .round()
            .max(dialog.min.1.ceil())
            .min(f64::from(i32::MAX));
        let size = WindowSize::new(width, height)?;
        dialog.requested = Some((size.width(), size.height()));
        Ok(())
    })
}
