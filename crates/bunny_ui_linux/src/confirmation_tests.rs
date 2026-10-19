//! Native surface witness; answer actions are protocol calls, not physical input.
use std::rc::Rc;
use std::time::{Duration, Instant};

use bunny_ui::accessibility::{Action, Role};
use bunny_ui::prelude::*;
use bunny_ui::window::{
    ConfirmationError, ConfirmationKind, ConfirmationResponse, ConfirmationSpec,
};

use crate::{App, WindowError, WindowId, WindowSpec, ffi};

fn question() -> ConfirmationSpec {
    ConfirmationSpec::new("Delete Árvore 🌳?", "Remove the selected project?").unwrap()
}

fn activate(runtime: &Runtime, label: &str) {
    let tree = runtime.accessibility_tree();
    let node = tree
        .nodes()
        .iter()
        .find(|node| node.role == Role::Button && node.label.as_ref() == label)
        .expect("localized accessible confirmation button");
    runtime
        .accessibility_action(node.id, Action::Activate)
        .unwrap();
}

#[test]
#[ignore = "requires an isolated X11 server or Wayland compositor and session bus"]
fn confirmation_native_protocol() {
    let app = App::new();
    app.set_locale(Some(Locale::new("pt-BR")));
    let guard = app.open(
        WindowSpec::titled("Confirmation guard"),
        Rc::new(app.runtime()),
        text("guard"),
    );
    let runtime = Rc::new(app.runtime());
    let subject = app.open(
        WindowSpec::titled("Confirmation subject"),
        Rc::clone(&runtime),
        text("subject"),
    );
    runtime.set_accessibility_enabled(true);
    // The native fixture waits on its own UI executor.
    let surface = |app: App, owner: WindowId| async move {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let handle = app.slot(owner).and_then(|slot| {
                slot.confirmation_dialogs
                    .borrow()
                    .values()
                    .find(|dialog| dialog.confirmation.is_some())
                    .map(|dialog| dialog.handle)
            });
            if let Some(handle) = handle
                && ffi::confirmation_geometry(handle).is_ok()
            {
                return handle;
            }
            assert!(
                Instant::now() < deadline,
                "native alert must acquire observed geometry"
            );
            task::sleep(Duration::from_millis(10)).await;
        }
    };
    let running = app.clone();
    task::spawn(async move {
        drop(running.confirm(subject, question()).unwrap());
        task::sleep(Duration::from_millis(20)).await;
        assert!(running.slot(subject).unwrap().confirmation_dialogs.borrow().is_empty());
        for (label, expected) in [("Sim", ConfirmationResponse::Yes), ("Não", ConfirmationResponse::No)] {
            let answer = running.confirm(subject, question()).unwrap();
            assert_eq!(answer.kind(), ConfirmationKind::FrameworkAlert);
            let native = surface(running.clone(), subject).await;
            assert!(matches!(running.confirm(subject, question()), Err(ConfirmationError::AlreadyOpen)));
            task::sleep(Duration::from_millis(30)).await;
            running.hide(guard).unwrap();
            running.show(guard).unwrap();
            activate(&runtime, label);
            assert_eq!(answer.await, Ok(expected));
            assert_eq!(ffi::confirmation_geometry(native), Err(WindowError::StaleWindow));
        }
        let mut answer = running.confirm(subject, question()).unwrap();
        let native = surface(running.clone(), subject).await;
        assert!(answer.cancel());
        assert_eq!(answer.await, Err(ConfirmationError::Cancelled));
        assert_eq!(ffi::confirmation_geometry(native), Err(WindowError::StaleWindow));

        let answer = running.confirm(subject, question()).unwrap();
        let native = surface(running.clone(), subject).await;
        running.hide(subject).unwrap();
        assert_eq!(answer.await, Err(ConfirmationError::Window(WindowError::HiddenWindow)));
        assert_eq!(ffi::confirmation_geometry(native), Err(WindowError::StaleWindow));
        running.show(subject).unwrap();
        let answer = running.confirm(subject, question()).unwrap();
        let native = surface(running.clone(), subject).await;
        running.close(subject);
        assert_eq!(answer.await, Err(ConfirmationError::OwnerClosed));
        assert_eq!(ffi::confirmation_geometry(native), Err(WindowError::StaleWindow));
        println!("Linux confirmation: observed native alert geometry, localized Yes/No, timer progress, unrelated window, queued drop, duplicate rejection, cancellation, hide, close and repeated requests passed");
        running.quit();
    }).detach();
    app.run();
}
