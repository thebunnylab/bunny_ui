//! Own-process AppKit sheet protocol; this does not inject physical input.
#[cfg(target_os = "macos")]
mod probe {
    use std::ffi::{CStr, c_char};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use bunny_ui::prelude::*;
    use bunny_ui::window::{
        ConfirmationError, ConfirmationKind, ConfirmationResponse, ConfirmationSpec,
    };
    use bunny_ui_apple::ffi::{CGPoint, Id, Sel, class, ns_string, sel};
    use bunny_ui_macos::{App, WindowError, WindowSpec};

    #[cfg(target_arch = "aarch64")]
    type NativeBool = bool;
    #[cfg(not(target_arch = "aarch64"))]
    type NativeBool = i8;

    #[allow(
        clashing_extern_declarations,
        reason = "Each selector uses its concrete Objective-C ABI"
    )]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(value: Id, selector: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn count(value: Id, selector: Sel) -> usize;
        #[link_name = "objc_msgSend"]
        fn at(value: Id, selector: Sel, index: usize) -> Id;
        #[link_name = "objc_msgSend"]
        fn chars(value: Id, selector: Sel) -> *const c_char;
        #[link_name = "objc_msgSend"]
        fn send(value: Id, selector: Sel, sender: Id);
        #[link_name = "objc_msgSend"]
        fn is_kind(value: Id, selector: Sel, class: Id) -> NativeBool;
        #[link_name = "objc_msgSend"]
        fn number(value: Id, selector: Sel) -> isize;
        #[link_name = "objc_msgSend"]
        fn key_event(
            class: Id,
            selector: Sel,
            kind: usize,
            position: CGPoint,
            modifiers: usize,
            timestamp: f64,
            window: isize,
            context: Id,
            characters: Id,
            unmodified: Id,
            repeat: NativeBool,
            code: u16,
        ) -> Id;
    }

    fn text_of(string: Id) -> String {
        assert!(!string.is_null());
        unsafe {
            CStr::from_ptr(chars(string, sel("UTF8String")))
                .to_string_lossy()
                .into_owned()
        }
    }

    fn owner(title: &str) -> Id {
        unsafe {
            let app = object(class("NSApplication"), sel("sharedApplication"));
            let windows = object(app, sel("windows"));
            (0..count(windows, sel("count")))
                .map(|index| at(windows, sel("objectAtIndex:"), index))
                .find(|window| text_of(object(*window, sel("title"))) == title)
                .expect("owned fixture window")
        }
    }

    fn sheet(title: &str) -> Id {
        unsafe { object(owner(title), sel("attachedSheet")) }
    }

    async fn wait_sheet(title: &str, shown: bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while sheet(title).is_null() == shown {
            assert!(
                Instant::now() < deadline,
                "sheet visibility must become {shown}"
            );
            task::sleep(Duration::from_millis(10)).await;
        }
    }

    fn button(view: Id, label: &str) -> Option<Id> {
        unsafe {
            if is_kind(view, sel("isKindOfClass:"), class("NSButton")) as u8 != 0
                && text_of(object(view, sel("title"))) == label
            {
                return Some(view);
            }
            let children = object(view, sel("subviews"));
            (0..count(children, sel("count")))
                .find_map(|index| button(at(children, sel("objectAtIndex:"), index), label))
        }
    }

    fn answer(title: &str, label: &str) {
        unsafe {
            let sheet = sheet(title);
            assert!(
                !sheet.is_null(),
                "an actual owner-attached AppKit sheet exists"
            );
            let view = object(sheet, sel("contentView"));
            let button = button(view, label).expect("localized native answer button");
            send(button, sel("performClick:"), std::ptr::null_mut());
        }
    }

    fn key(title: &str, characters: &str, code: u16) {
        key_on(sheet(title), characters, code);
    }

    fn key_on(window: Id, characters: &str, code: u16) {
        unsafe {
            let event = key_event(
                class("NSEvent"),
                sel(
                    "keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
                ),
                10,
                CGPoint { x: 0.0, y: 0.0 },
                0,
                0.0,
                number(window, sel("windowNumber")),
                std::ptr::null_mut(),
                ns_string(characters),
                ns_string(characters),
                false as NativeBool,
                code,
            );
            assert!(!event.is_null());
            assert_eq!(object(event, sel("window")), window, "own window event");
            send(
                object(class("NSApplication"), sel("sharedApplication")),
                sel("sendEvent:"),
                event,
            );
        }
    }

    fn question() -> ConfirmationSpec {
        ConfirmationSpec::new(
            "Delete Árvore 🌳?",
            "This removes the selected project.\nContinue?",
        )
        .unwrap()
    }

    pub fn run() {
        eprintln!("confirmation stage: create windows");
        let app = App::new();
        app.set_locale(Some(Locale::new("pt-BR")));
        let guard = app.open(
            WindowSpec::titled("Confirmation guard"),
            Rc::new(app.runtime()),
            text("guard"),
        );
        let subject = app.open(
            WindowSpec::titled("Confirmation subject"),
            Rc::new(app.runtime()),
            text("subject"),
        );
        assert!(matches!(
            App::new().confirm(subject, question()),
            Err(ConfirmationError::Window(WindowError::StaleWindow))
        ));
        app.hide(subject).unwrap();
        assert!(matches!(
            app.confirm(subject, question()),
            Err(ConfirmationError::Window(WindowError::HiddenWindow))
        ));
        app.show(subject).unwrap();
        drop(app.confirm(subject, question()).unwrap());

        eprintln!("confirmation stage: queue probe");
        let running = app.clone();
        task::spawn(async move {
            eprintln!("confirmation stage: initial timer");
            task::sleep(Duration::from_millis(50)).await;
            assert!(
                sheet("Confirmation subject").is_null(),
                "pre-presentation drop stays closed"
            );
            for (label, expected) in [
                ("Sim", ConfirmationResponse::Yes),
                ("Não", ConfirmationResponse::No),
            ] {
                eprintln!("confirmation stage: button {label}");
                let confirmation = running.confirm(subject, question()).unwrap();
                eprintln!("confirmation stage: request queued {label}");
                assert_eq!(confirmation.kind(), ConfirmationKind::AppKitSheet);
                assert!(matches!(
                    running.confirm(subject, question()),
                    Err(ConfirmationError::AlreadyOpen)
                ));
                wait_sheet("Confirmation subject", true).await;
                eprintln!("confirmation stage: sheet attached {label}");
                unsafe {
                    let default = object(sheet("Confirmation subject"), sel("defaultButtonCell"));
                    assert_eq!(
                        text_of(object(default, sel("title"))),
                        "Não",
                        "negative default"
                    );
                }
                // This timer and an operation on another window run while the sheet is open.
                task::sleep(Duration::from_millis(40)).await;
                running.hide(guard).unwrap();
                running.show(guard).unwrap();
                eprintln!("confirmation stage: activate {label}");
                answer("Confirmation subject", label);
                assert_eq!(confirmation.await, Ok(expected));
                eprintln!("confirmation stage: completed {label}");
                wait_sheet("Confirmation subject", false).await;
            }
            eprintln!("confirmation stage: keyboard");
            for (characters, code) in [("\u{1b}", 53), ("\r", 36), ("\u{3}", 76)] {
                let confirmation = running.confirm(subject, question()).unwrap();
                wait_sheet("Confirmation subject", true).await;
                eprintln!("confirmation stage: key {code}");
                key("Confirmation subject", characters, code);
                assert_eq!(confirmation.await, Ok(ConfirmationResponse::No), "Return, Enter and Escape are negative");
                wait_sheet("Confirmation subject", false).await;
            }
            eprintln!("confirmation stage: simultaneous owners");
            let subject_answer = running.confirm(subject, question()).unwrap();
            let guard_answer = running.confirm(guard, question()).unwrap();
            wait_sheet("Confirmation subject", true).await;
            wait_sheet("Confirmation guard", true).await;
            key("Confirmation guard", "\u{1b}", 53);
            assert_eq!(guard_answer.await, Ok(ConfirmationResponse::No));
            wait_sheet("Confirmation guard", false).await;
            key_on(owner("Confirmation guard"), "\r", 36);
            task::sleep(Duration::from_millis(20)).await;
            assert!(!sheet("Confirmation subject").is_null(), "another window cannot answer this sheet");
            answer("Confirmation subject", "Sim");
            assert_eq!(subject_answer.await, Ok(ConfirmationResponse::Yes));
            wait_sheet("Confirmation subject", false).await;

            eprintln!("confirmation stage: explicit cancellation");
            let mut confirmation = running.confirm(subject, question()).unwrap();
            wait_sheet("Confirmation subject", true).await;
            assert!(confirmation.cancel());
            assert_eq!(confirmation.await, Err(ConfirmationError::Cancelled));
            wait_sheet("Confirmation subject", false).await;

            let confirmation = running.confirm(subject, question()).unwrap();
            wait_sheet("Confirmation subject", true).await;
            eprintln!("confirmation stage: active drop");
            drop(confirmation);
            wait_sheet("Confirmation subject", false).await;

            let confirmation = running.confirm(subject, question()).unwrap();
            wait_sheet("Confirmation subject", true).await;
            eprintln!("confirmation stage: hide owner");
            running.hide(subject).unwrap();
            assert_eq!(
                confirmation.await,
                Err(ConfirmationError::Window(WindowError::HiddenWindow))
            );
            running.show(subject).unwrap();
            wait_sheet("Confirmation subject", false).await;

            let confirmation = running.confirm(subject, question()).unwrap();
            wait_sheet("Confirmation subject", true).await;
            eprintln!("confirmation stage: close owner");
            running.close(subject);
            assert_eq!(confirmation.await, Err(ConfirmationError::OwnerClosed));
            assert!(matches!(
                running.confirm(subject, question()),
                Err(ConfirmationError::Window(WindowError::StaleWindow))
            ));
            println!(
                "AppKit confirmation: native Yes/No, Return/Escape, negative default, localized controls, timer progress, unrelated window, pre-presentation drop, active cancellation/drop, hide, close and repeated requests passed"
            );
            running.quit();
        }).detach();
        eprintln!("confirmation stage: enter event loop");
        app.run();
    }
}

#[cfg(target_os = "macos")]
fn main() {
    if std::env::var_os("BUNNY_CONFIRMATION_CHILD").is_some() {
        probe::run();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_CONFIRMATION_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "native confirmation fixture: {status}");
            break;
        }
        if std::time::Instant::now() >= deadline {
            // Sample only this fixture's child before the watchdog retires it.
            // Native stalls need the platform stack, not a longer timeout.
            if let Ok(sample) = std::process::Command::new("/usr/bin/sample")
                .args([&child.id().to_string(), "1", "1", "-file", "/dev/stdout"])
                .output()
            {
                eprintln!("{}", String::from_utf8_lossy(&sample.stdout));
                eprintln!("{}", String::from_utf8_lossy(&sample.stderr));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("confirmation event loop did not complete");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
