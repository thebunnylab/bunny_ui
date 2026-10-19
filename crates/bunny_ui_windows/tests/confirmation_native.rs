//! Own-process Win32 message-box protocol; no global input is injected.
#[cfg(target_os = "windows")]
mod probe {
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use bunny_ui::prelude::*;
    use bunny_ui::window::{
        ConfirmationError, ConfirmationKind, ConfirmationResponse, ConfirmationSpec,
    };
    use bunny_ui_windows::{App, WindowError, WindowSpec};

    #[link(name = "user32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn GetWindow(window: isize, relation: u32) -> isize;
        fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
        fn IsWindowEnabled(window: isize) -> i32;
        fn IsWindow(window: isize) -> i32;
        fn SendMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> isize;
        fn PostMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain([0]).collect()
    }

    fn owned(window: isize) -> isize {
        assert_ne!(window, 0, "native fixture window exists");
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(window, &raw mut process) };
        assert_eq!(
            process,
            std::process::id(),
            "only this fixture's windows are addressed"
        );
        window
    }

    fn window(title: &str) -> isize {
        owned(unsafe { FindWindowW(std::ptr::null(), wide(title).as_ptr()) })
    }

    async fn message_box(owner: isize) -> isize {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let dialog = unsafe {
                FindWindowW(wide("#32770").as_ptr(), wide("Delete Árvore 🌳?").as_ptr())
            };
            if dialog != 0 && unsafe { GetWindow(dialog, 4) } == owner {
                return owned(dialog);
            }
            assert!(
                Instant::now() < deadline,
                "an owner-bound native message box must appear"
            );
            task::sleep(Duration::from_millis(10)).await;
        }
    }

    fn post(window: isize, message: u32, value: usize) {
        owned(window);
        assert_ne!(unsafe { PostMessageW(window, message, value, 0) }, 0);
    }

    fn question() -> ConfirmationSpec {
        ConfirmationSpec::new("Delete Árvore 🌳?", "Remove the selected project?").unwrap()
    }

    pub fn run() {
        let app = App::new();
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
        let owner = window("Confirmation subject");
        let other = window("Confirmation guard");
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

        let running = app.clone();
        task::spawn(async move {
            task::sleep(Duration::from_millis(30)).await;
            for (command, response) in [(6, ConfirmationResponse::Yes), (7, ConfirmationResponse::No)] {
                let answer = running.confirm(subject, question()).unwrap();
                assert_eq!(answer.kind(), ConfirmationKind::Win32MessageBox);
                let dialog = message_box(owner).await;
                assert_eq!(unsafe { IsWindowEnabled(owner) }, 0);
                assert_ne!(unsafe { IsWindowEnabled(other) }, 0);
                assert_eq!(unsafe { SendMessageW(dialog, 0x0400, 0, 0) } & 0xffff, 7, "No is the default button");
                assert!(matches!(running.confirm(subject, question()), Err(ConfirmationError::AlreadyOpen)));
                task::sleep(Duration::from_millis(30)).await;
                running.hide(guard).unwrap();
                running.show(guard).unwrap();
                post(dialog, 0x0111, command);
                assert_eq!(answer.await, Ok(response));
                assert_eq!(unsafe { IsWindow(dialog) }, 0, "completion follows native teardown");
                assert_ne!(unsafe { IsWindowEnabled(owner) }, 0);
            }
            for key in [13, 27] {
                let answer = running.confirm(subject, question()).unwrap();
                let dialog = message_box(owner).await;
                post(dialog, 0x0100, key);
                assert_eq!(answer.await, Ok(ConfirmationResponse::No));
            }
            let active = running.confirm(subject, question()).unwrap();
            let dialog = message_box(owner).await;
            let mut queued = running.confirm(guard, question()).unwrap();
            assert!(queued.cancel());
            assert_eq!(queued.await, Err(ConfirmationError::Cancelled), "queued cancellation completes while the first box remains open");
            assert_ne!(unsafe { IsWindow(dialog) }, 0);
            post(dialog, 0x0111, 6);
            assert_eq!(active.await, Ok(ConfirmationResponse::Yes));

            let mut answer = running.confirm(subject, question()).unwrap();
            let dialog = message_box(owner).await;
            assert!(answer.cancel());
            assert_eq!(answer.await, Err(ConfirmationError::Cancelled));
            assert_eq!(unsafe { IsWindow(dialog) }, 0);

            let answer = running.confirm(subject, question()).unwrap();
            message_box(owner).await;
            running.hide(subject).unwrap();
            assert_eq!(answer.await, Err(ConfirmationError::Window(WindowError::HiddenWindow)));
            running.show(subject).unwrap();
            let answer = running.confirm(subject, question()).unwrap();
            message_box(owner).await;
            running.close(subject);
            assert_eq!(answer.await, Err(ConfirmationError::OwnerClosed));
            println!("Win32 confirmation: owner-disabled native box, Yes/No, negative default, Return/Escape, timer progress, unrelated window, queued cancellation, active cancellation, hide, close and repeated requests passed");
            running.quit();
        }).detach();
        app.run();
    }
}

#[cfg(target_os = "windows")]
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
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("confirmation event loop did not complete");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {}
