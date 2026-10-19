//! Public ownership/visibility contract. Focus receipts are requests, not input proof.
use crate::shell::{App, CloseAction, WindowError, WindowSpec, WindowVisibility};
use bunny_ui::prelude::*;
use std::rc::Rc;

pub fn run(native_parent: fn(&str, Option<&str>)) {
    let app = App::new();
    let guard = app.open(
        WindowSpec::titled("Relationship guard"),
        Rc::new(app.runtime()),
        text("guard"),
    );
    let parent = app.open(
        WindowSpec::titled("Relationship parent"),
        Rc::new(app.runtime()),
        text("parent"),
    );
    let child = app.open(
        WindowSpec::titled("Relationship child"),
        Rc::new(app.runtime()),
        text("child"),
    );
    let grandchild = app.open(
        WindowSpec::titled("Relationship grandchild"),
        Rc::new(app.runtime()),
        text("grandchild"),
    );
    assert_eq!(app.parent(child), Ok(None));
    assert_eq!(
        app.set_parent(child, Some(child)),
        Err(WindowError::RelationshipCycle)
    );
    let foreign = App::new();
    assert_eq!(
        foreign.set_parent(child, Some(parent)),
        Err(WindowError::StaleWindow)
    );
    assert_eq!(foreign.focus(child), Err(WindowError::StaleWindow));
    assert_eq!(foreign.parent(child), Err(WindowError::StaleWindow));
    app.set_parent(child, Some(parent)).unwrap();
    app.set_parent(grandchild, Some(child)).unwrap();
    native_parent("Relationship child", Some("Relationship parent"));
    native_parent("Relationship grandchild", Some("Relationship child"));
    assert_eq!(app.parent(child).unwrap().unwrap().window(), parent);
    assert_eq!(
        app.set_parent(parent, Some(grandchild)),
        Err(WindowError::RelationshipCycle)
    );
    for _ in 0..3 {
        app.set_parent(child, Some(parent)).unwrap();
    }
    match app.focus(child) {
        Ok(()) => println!("Native activation request submitted; actual focus not qualified"),
        Err(WindowError::FocusDenied) => println!("Native activation declined by OS policy"),
        Err(WindowError::Unsupported(reason)) => {
            println!("Native activation unavailable: {reason}")
        }
        other => panic!("unexpected activation result: {other:?}"),
    }
    app.hide(parent).unwrap();
    for id in [parent, child, grandchild] {
        assert_eq!(app.visibility(id), Ok(WindowVisibility::Hidden));
        assert_eq!(app.focus(id), Err(WindowError::HiddenWindow));
    }
    app.show(child).unwrap();
    assert_eq!(app.visibility(child), Ok(WindowVisibility::Hidden));
    app.hide(grandchild).unwrap();
    app.show(parent).unwrap();
    assert_eq!(app.visibility(child), Ok(WindowVisibility::Shown));
    assert_eq!(app.visibility(grandchild), Ok(WindowVisibility::Hidden));
    app.show(grandchild).unwrap();
    native_parent("Relationship child", Some("Relationship parent"));
    native_parent("Relationship grandchild", Some("Relationship child"));
    app.on_close_requested(parent, |_| CloseAction::Cancel)
        .unwrap();
    assert_eq!(app.request_close(parent), Ok(CloseAction::Cancel));
    assert!(app.windows().contains(&grandchild));
    app.on_close_requested(parent, |_| CloseAction::Hide)
        .unwrap();
    assert_eq!(app.request_close(parent), Ok(CloseAction::Hide));
    assert!(app.windows().contains(&grandchild));
    app.show(parent).unwrap();
    app.set_parent(grandchild, None).unwrap();
    native_parent("Relationship grandchild", None);
    assert_eq!(app.parent(grandchild), Ok(None));
    app.on_close_requested(child, |_| CloseAction::Cancel)
        .unwrap();
    assert_eq!(app.request_close(child), Ok(CloseAction::Cancel));
    app.clear_close_handler(parent).unwrap();
    assert_eq!(app.request_close(parent), Ok(CloseAction::Close));
    assert_eq!(app.windows(), vec![guard, grandchild]);
    for closed in [parent, child] {
        assert_eq!(app.parent(closed), Err(WindowError::StaleWindow));
        assert_eq!(app.focus(closed), Err(WindowError::StaleWindow));
        assert_eq!(
            app.set_parent(grandchild, Some(closed)),
            Err(WindowError::StaleWindow)
        );
    }
    println!(
        "Ownership API passed: native relationship requests, cycles, foreign/stale IDs, inherited visibility, cancellation, detachment and descendant teardown. Input/pixels/DPI unqualified."
    );
    app.quit();
}
