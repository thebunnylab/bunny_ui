//! Framework alert contract, including native teardown acknowledgement.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll, Waker};

use bunny_ui::accessibility::{Action, Role};
use bunny_ui::prelude::*;
use bunny_ui::window::{
    Confirmation, ConfirmationError, ConfirmationResponse, ConfirmationSpec, FrameworkConfirmation,
    Lifecycle,
};
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 800.0,
    height: 600.0,
};

fn poll(future: &mut Confirmation) -> Poll<Result<ConfirmationResponse, ConfirmationError>> {
    Pin::new(future).poll(&mut TaskContext::from_waker(Waker::noop()))
}

fn question() -> ConfirmationSpec {
    ConfirmationSpec::new(
        "Delete project?",
        "This action removes the selected project.",
    )
    .unwrap()
}

fn press(runtime: &Runtime, key: Key) -> bool {
    let pattern = KeyPattern::key(key);
    match runtime.chord(pattern) {
        bunny_ui::action::KeyMatch::Action(action) => runtime.dispatch_action(action),
        bunny_ui::action::KeyMatch::Pending => true,
        bunny_ui::action::KeyMatch::None => runtime.key_stroke(pattern).handled,
    }
}

#[test]
fn framework_confirmation_localizes_blocks_parent_restores_focus_and_waits_for_teardown() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.set_locale(Some(Locale::new("pt-BR")));
    let host = FrameworkConfirmation::new(Lifecycle::default().id());
    let value = State::new(String::from("draft"));
    let presses = State::new(0);
    let view = host.host(vstack!(
        text_field("Draft", value.binding()).id("draft"),
        button(text("Parent action"), move || presses.add(1)).id("parent"),
    ));
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("draft"));
    let previous = runtime.focused();
    let mut future = host.start(question()).unwrap();
    runtime.display_frame(&view, SIZE);
    let token = host.current_token().unwrap();
    assert!(host.native_presented(token));
    assert!(!runtime.focus_named("parent"));
    let tree = runtime.accessibility_tree();
    for label in ["Sim", "Não"] {
        assert!(
            tree.nodes()
                .iter()
                .any(|node| node.role == Role::Button && node.label.as_ref() == label)
        );
    }
    let no = tree
        .nodes()
        .iter()
        .find(|node| node.role == Role::Button && node.label.as_ref() == "Não")
        .unwrap();
    runtime.accessibility_action(no.id, Action::Focus).unwrap();
    assert!(press(&runtime, Key::Tab));
    assert!(
        runtime
            .accessibility_tree()
            .nodes()
            .iter()
            .any(|node| node.focused && node.label.as_ref() == "Sim")
    );
    assert!(press(&runtime, Key::Enter));
    runtime.display_frame(&view, SIZE);
    assert!(runtime.overlays().is_empty());
    assert_eq!(
        poll(&mut future),
        Poll::Pending,
        "a button cannot acknowledge native teardown"
    );
    assert!(matches!(
        host.start(question()),
        Err(ConfirmationError::AlreadyOpen)
    ));
    assert!(host.native_closed(token));
    assert_eq!(poll(&mut future), Poll::Ready(Ok(ConfirmationResponse::No)));
    assert_eq!(runtime.focused(), previous);
    assert_eq!(presses.get(), 0);
    assert_eq!(value.get(), "draft");
}

#[test]
fn framework_confirmation_keyboard_pointer_and_cancellation_keep_requests_separate() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let host = FrameworkConfirmation::new(Lifecycle::default().id());
    let view = host.host(text("owner"));
    let mut first = host.start(question()).unwrap();
    runtime.display_frame(&view, SIZE);
    let old = host.current_token().unwrap();
    assert!(host.native_presented(old));
    let tree = runtime.accessibility_tree();
    let yes = tree
        .nodes()
        .iter()
        .find(|node| node.role == Role::Button && node.label.as_ref() == "Yes")
        .unwrap();
    runtime
        .accessibility_action(yes.id, Action::Activate)
        .unwrap();
    runtime.display_frame(&view, SIZE);
    assert!(host.native_closed(old));
    assert_eq!(poll(&mut first), Poll::Ready(Ok(ConfirmationResponse::Yes)));

    let mut second = host.start(question()).unwrap();
    runtime.display_frame(&view, SIZE);
    let next = host.current_token().unwrap();
    assert_ne!(next, old);
    assert!(host.native_presented(next));
    assert!(
        !host.native_closed(old),
        "a stale window close cannot answer a new request"
    );
    assert!(press(&runtime, Key::Escape));
    runtime.display_frame(&view, SIZE);
    assert!(host.native_closed(next));
    assert_eq!(
        poll(&mut second),
        Poll::Ready(Ok(ConfirmationResponse::Dismissed))
    );

    let mut cancelled = host.start(question()).unwrap();
    runtime.display_frame(&view, SIZE);
    let token = host.current_token().unwrap();
    assert!(host.native_presented(token));
    assert!(cancelled.cancel());
    runtime.display_frame(&view, SIZE);
    assert!(runtime.overlays().is_empty());
    assert_eq!(poll(&mut cancelled), Poll::Pending);
    assert!(host.native_closed(token));
    assert_eq!(
        poll(&mut cancelled),
        Poll::Ready(Err(ConfirmationError::Cancelled))
    );
}

#[test]
fn framework_confirmation_drop_before_presentation_needs_no_native_acknowledgement() {
    let host = FrameworkConfirmation::new(Lifecycle::default().id());
    drop(host.start(question()).unwrap());
    assert!(host.current_token().is_none());
    let mut request = host.start(question()).unwrap();
    host.owner_closed();
    assert_eq!(
        poll(&mut request),
        Poll::Ready(Err(ConfirmationError::OwnerClosed))
    );
    assert!(matches!(
        host.start(question()),
        Err(ConfirmationError::OwnerClosed)
    ));
}

#[test]
fn framework_confirmation_native_failure_and_presenter_retirement_are_errors() {
    let host = FrameworkConfirmation::new(Lifecycle::default().id());
    let mut request = host.start(question()).unwrap();
    let token = host.current_token().unwrap();
    let error = ConfirmationError::Window(bunny_ui::window::WindowError::NativeFailure {
        operation: "test alert surface",
        code: 5,
    });
    assert!(host.native_failed(token, error.clone()));
    assert_eq!(poll(&mut request), Poll::Ready(Err(error)));
    let mut request = host.start(question()).unwrap();
    assert!(host.native_presented(host.current_token().unwrap()));
    drop(host);
    assert_eq!(
        poll(&mut request),
        Poll::Ready(Err(ConfirmationError::OwnerClosed))
    );
}

#[test]
fn open_confirmation_repaints_and_relocalizes_without_replacing_its_request() {
    use bunny_ui::layout::{DisplayList, DrawCommand};
    use bunny_ui::theme::{self, Theme};

    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.set_locale(Some(Locale::new("en")));
    let host = FrameworkConfirmation::new(Lifecycle::default().id());
    let draft = State::new(String::from("unchanged draft"));
    let view = host.host(text_field("Draft", draft.binding()));
    let mut answer = host.start(question()).unwrap();
    let token = host.current_token().unwrap();
    let ink = |display: &DisplayList| {
        display
            .iter()
            .find_map(|command| match command {
                DrawCommand::TextLine { content, color, .. }
                    if content.as_ref() == "Delete project?" =>
                {
                    Some(*color)
                }
                _ => None,
            })
            .expect("the confirmation question is painted")
    };
    theme::install(Theme::light());
    assert_eq!(ink(&runtime.display_frame(&view, SIZE)), Theme::light().fg);
    assert!(host.native_presented(token));
    theme::install(Theme::dark());
    runtime.set_locale(Some(Locale::new("pt-BR")));
    assert_eq!(ink(&runtime.display_frame(&view, SIZE)), Theme::dark().fg);
    assert_eq!(host.current_token(), Some(token));
    assert_eq!(poll(&mut answer), Poll::Pending);
    assert_eq!(draft.get(), "unchanged draft");
    let tree = runtime.accessibility_tree();
    let no = tree
        .nodes()
        .iter()
        .find(|node| node.role == Role::Button && node.label.as_ref() == "Não")
        .unwrap();
    assert!(!tree.nodes().iter().any(|node| node.label.as_ref() == "Yes"));
    runtime
        .accessibility_action(no.id, Action::Activate)
        .unwrap();
    runtime.display_frame(&view, SIZE);
    assert!(host.native_closed(token));
    assert_eq!(poll(&mut answer), Poll::Ready(Ok(ConfirmationResponse::No)));
    theme::install(Theme::light());
}
