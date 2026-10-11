//! Dialog placement separates persistent model state from each native presentation.
#![forbid(unsafe_code)]

use bunny_ui_core::layout::DialogSpec;
use bunny_ui_core::window::{
    DialogGeometry, DialogGeometrySession, WindowError, WindowGeometry, WindowPlacement, WindowSize,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

fn geometry(width: f64) -> WindowGeometry {
    WindowGeometry::new(WindowSize::new(width, 300.0).unwrap(), None, 2.0).unwrap()
}

fn observed(session: &Rc<DialogGeometrySession>, width: f64) {
    session.capture(Ok(geometry(width))).unwrap().deliver();
}

#[test]
fn restoration_is_once_per_presentation_and_is_not_an_observation() {
    let requested = WindowPlacement::new(WindowSize::new(900.0, 300.0).unwrap(), None);
    let controller = DialogGeometry::restoring(requested);
    let first = controller.begin().unwrap();
    assert_eq!(first.take_restoration(), Some(requested));
    assert_eq!(first.take_restoration(), None);
    assert_eq!(controller.geometry(), None);
    let queued = first.capture(Ok(geometry(800.0))).unwrap();
    assert_eq!(
        controller.geometry(),
        None,
        "capture cannot credit a request"
    );
    queued.deliver();
    assert_eq!(controller.geometry(), Some(geometry(800.0)));
    first.retire();
    let reopened = controller.begin().unwrap();
    assert_eq!(
        reopened.take_restoration(),
        Some(geometry(800.0).placement())
    );
}

#[test]
fn one_controller_cannot_steal_another_active_presentation() {
    let controller = DialogGeometry::new();
    let first = controller.begin().unwrap();
    assert!(matches!(
        controller.begin(),
        Err(WindowError::GeometryInUse)
    ));
    assert_eq!(controller.last_error(), Some(WindowError::GeometryInUse));
    observed(&first, 640.0);
    assert_eq!(controller.geometry(), Some(geometry(640.0)));
    assert!(DialogGeometry::new().begin().is_ok());
}

#[test]
fn retirement_and_reopening_reject_old_native_events() {
    let controller = DialogGeometry::new();
    let old = controller.begin().unwrap();
    let captured = old.capture(Ok(geometry(999.0))).unwrap();
    old.retire();
    assert!(old.capture(Ok(geometry(998.0))).is_none());
    let current = controller.begin().unwrap();
    observed(&current, 600.0);
    captured.deliver();
    assert_eq!(controller.geometry(), Some(geometry(600.0)));
    drop(old);
    assert!(matches!(
        controller.begin(),
        Err(WindowError::GeometryInUse)
    ));
}

#[test]
fn dropping_the_presentation_releases_its_lease() {
    let controller = DialogGeometry::new();
    let captured = {
        let session = controller.begin().unwrap();
        session.capture(Ok(geometry(999.0))).unwrap()
    };
    let current = controller.begin().unwrap();
    captured.deliver();
    assert_eq!(controller.geometry(), None);
    observed(&current, 600.0);
}

#[test]
fn callback_replacement_and_removal_invalidate_captured_delivery() {
    let controller = DialogGeometry::new();
    let count = Rc::new(Cell::new(0));
    controller.on_change(|_| panic!("replaced callback"));
    let session = controller.begin().unwrap();
    let old = session.capture(Ok(geometry(999.0))).unwrap();
    controller.on_change({
        let count = Rc::clone(&count);
        move |_| count.set(count.get() + 1)
    });
    old.deliver();
    assert_eq!(count.get(), 0);
    assert_eq!(controller.geometry(), None);
    observed(&session, 600.0);
    assert_eq!(count.get(), 1);
    let old = session.capture(Ok(geometry(999.0))).unwrap();
    controller.clear_handler();
    old.deliver();
    assert_eq!(controller.geometry(), Some(geometry(600.0)));
    observed(&session, 700.0);
    assert_eq!(controller.geometry(), Some(geometry(700.0)));
    assert_eq!(count.get(), 1);
}

#[test]
fn reentrant_callbacks_preserve_a_b_a_and_suppress_only_consecutive_duplicates() {
    let controller = DialogGeometry::new();
    let session = controller.begin().unwrap();
    let weak = Rc::downgrade(&session);
    let seen = Rc::new(RefCell::new(Vec::new()));
    controller.on_change({
        let seen = Rc::clone(&seen);
        let nested = Cell::new(false);
        move |value| {
            seen.borrow_mut().push(value.unwrap());
            if !nested.replace(true) {
                let session = weak.upgrade().unwrap();
                observed(&session, 700.0);
                observed(&session, 700.0);
                observed(&session, 600.0);
            }
        }
    });
    observed(&session, 600.0);
    assert_eq!(
        *seen.borrow(),
        vec![geometry(600.0), geometry(700.0), geometry(600.0)]
    );
}

#[test]
fn reopening_inside_a_callback_does_not_reborrow_the_callback() {
    let controller = DialogGeometry::new();
    let session = controller.begin().unwrap();
    let current = Rc::new(RefCell::new(Rc::clone(&session)));
    let seen = Rc::new(RefCell::new(Vec::new()));
    controller.on_change({
        let controller = controller.clone();
        let current = Rc::clone(&current);
        let seen = Rc::clone(&seen);
        let nested = Cell::new(false);
        move |value| {
            seen.borrow_mut().push(value.unwrap());
            if !nested.replace(true) {
                current.borrow().retire();
                let next = controller.begin().unwrap();
                *current.borrow_mut() = Rc::clone(&next);
                observed(&next, 700.0);
            }
        }
    });
    observed(&session, 600.0);
    assert_eq!(*seen.borrow(), vec![geometry(600.0), geometry(700.0)]);
    controller.clear_handler();
}

#[test]
fn native_errors_never_replace_last_successful_geometry() {
    let controller = DialogGeometry::new();
    let seen = Rc::new(RefCell::new(Vec::new()));
    controller.on_change({
        let seen = Rc::clone(&seen);
        move |value| seen.borrow_mut().push(value)
    });
    let session = controller.begin().unwrap();
    observed(&session, 600.0);
    let error = WindowError::Unsupported("global position is unavailable");
    session.capture(Err(error)).unwrap().deliver();
    assert_eq!(controller.geometry(), Some(geometry(600.0)));
    assert_eq!(controller.last_error(), Some(error));
    assert_eq!(*seen.borrow(), vec![Ok(geometry(600.0)), Err(error)]);
}

#[test]
fn queued_events_do_not_keep_application_callbacks_alive() {
    let marker = Rc::new(());
    let weak = Rc::downgrade(&marker);
    let controller = DialogGeometry::new();
    controller.on_change(move |_| {
        let _keep = &marker;
    });
    let session = controller.begin().unwrap();
    let queued = session.capture(Ok(geometry(600.0))).unwrap();
    drop(controller);
    assert!(weak.upgrade().is_none());
    queued.deliver();
}

#[test]
fn dialog_specs_compare_controller_identity_not_current_observations() {
    let controller = DialogGeometry::new();
    let spec = DialogSpec::titled("Edit").geometry(controller.clone());
    let clone = spec.clone();
    let session = controller.begin().unwrap();
    observed(&session, 600.0);
    assert_eq!(spec, clone);
    assert_ne!(
        spec,
        DialogSpec::titled("Edit").geometry(DialogGeometry::new())
    );
}

#[test]
fn runtime_retires_removed_presentations_even_when_the_host_keeps_a_lease() {
    use bunny_ui_core::{
        layout::{Proposal, Size},
        prelude::*,
        runtime::Runtime,
    };
    let runtime = Runtime::new();
    runtime.set_dialog_geometry_dispatch(|event| event.deliver());
    let controller = DialogGeometry::new();
    let open = State::new(true);
    let view = text("parent").dialog(
        open.binding(),
        DialogSpec::titled("child").geometry(controller.clone()),
        |_| erased(text("content")),
    );
    let proposal = Proposal::exact(Size {
        width: 1000.0,
        height: 800.0,
    });
    let layout = runtime.settled_layout(&view, proposal);
    let path = layout.overlays[0].path.clone();
    let old = runtime.dialog_geometry_session(&path).unwrap();
    observed(&old, 600.0);
    let late = old.capture(Ok(geometry(999.0))).unwrap();
    open.set(false);
    runtime.settled_layout(&view, proposal);
    assert!(runtime.dialog_geometry_session(&path).is_none());
    late.deliver();
    assert_eq!(controller.geometry(), Some(geometry(600.0)));
    open.set(true);
    runtime.settled_layout(&view, proposal);
    let reopened = runtime.dialog_geometry_session(&path).unwrap();
    assert!(!Rc::ptr_eq(&old, &reopened));
    assert_eq!(
        reopened.take_restoration(),
        Some(geometry(600.0).placement())
    );
    runtime.retire_dialog_geometry();
    assert!(reopened.capture(Ok(geometry(999.0))).is_none());
    assert!(
        controller.begin().is_ok(),
        "a surviving runtime must not retain a destroyed parent"
    );
}

#[test]
fn scene_only_host_reports_unsupported_on_the_next_executor_turn() {
    use bunny_ui_core::{
        layout::{Proposal, Size},
        prelude::*,
        runtime::Runtime,
    };
    let runtime = Rc::new(Runtime::new());
    let controller = DialogGeometry::new();
    let seen = Rc::new(RefCell::new(Vec::new()));
    controller.on_change({
        let seen = Rc::clone(&seen);
        let weak = Rc::downgrade(&runtime);
        move |value| {
            seen.borrow_mut().push(value);
            weak.upgrade().unwrap().retire_dialog_geometry();
        }
    });
    let view = text("parent").dialog(
        State::new(true).binding(),
        DialogSpec::titled("child").geometry(controller.clone()),
        |_| erased(text("content")),
    );
    runtime.settled_layout(
        &view,
        Proposal::exact(Size {
            width: 1000.0,
            height: 800.0,
        }),
    );
    assert!(
        seen.borrow().is_empty(),
        "layout must never invoke application callbacks"
    );
    runtime.poll_tasks();
    assert!(matches!(
        seen.borrow().as_slice(),
        [Err(WindowError::Unsupported(_))]
    ));
    assert_eq!(controller.geometry(), None);
}

#[test]
fn dom_fallback_keeps_the_lease_across_retained_frames_and_retires_on_dismissal() {
    use bunny_ui_core::{layout::Size, prelude::*, runtime::Runtime};
    let runtime = Runtime::new();
    let controller = DialogGeometry::new();
    let seen = Rc::new(Cell::new(0));
    controller.on_change({
        let seen = Rc::clone(&seen);
        move |value| {
            assert!(value.is_err());
            seen.set(seen.get() + 1);
        }
    });
    let open = State::new(true);
    let view = text("parent").dialog(
        open.binding(),
        DialogSpec::titled("child").geometry(controller.clone()),
        |_| erased(text("content")),
    );
    let size = Size {
        width: 800.0,
        height: 600.0,
    };
    runtime.dom_frame(&view, size);
    runtime.poll_tasks();
    assert_eq!(seen.get(), 1);
    runtime.dom_frame(&view, size);
    runtime.poll_tasks();
    assert_eq!(
        seen.get(),
        1,
        "a retained DOM frame is not a new presentation"
    );
    open.set(false);
    runtime.dom_frame(&view, size);
    assert!(controller.begin().is_ok());
}

#[cfg(feature = "canvas")]
#[test]
fn dom_exact_and_canvas_fallbacks_report_no_native_geometry() {
    use bunny_ui_core::{
        layout::{LayoutMode, Rendering, Size},
        prelude::*,
        runtime::Runtime,
    };
    for canvas in [false, true] {
        let runtime = Runtime::new();
        let controller = DialogGeometry::new();
        let seen = Rc::new(Cell::new(0));
        controller.on_change({
            let seen = Rc::clone(&seen);
            move |value| {
                assert!(matches!(value, Err(WindowError::Unsupported(_))));
                seen.set(seen.get() + 1);
            }
        });
        let view = text("parent").dialog(
            State::new(true).binding(),
            DialogSpec::titled("child").geometry(controller.clone()),
            |_| erased(text("content")),
        );
        let view = if canvas {
            erased(view.rendering(Rendering::Gpu))
        } else {
            erased(view.layout(LayoutMode::Exact))
        };
        runtime.dom_frame(
            &view,
            Size {
                width: 800.0,
                height: 600.0,
            },
        );
        runtime.poll_tasks();
        assert_eq!(seen.get(), 1, "canvas={canvas}");
        assert_eq!(controller.geometry(), None);
    }
}
