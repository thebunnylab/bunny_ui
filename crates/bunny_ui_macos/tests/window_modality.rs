//! Main-thread native eligibility witness, not a pointer/focus qualification.
#[cfg(target_os = "macos")]
mod native {
    use bunny_ui::layout::DialogSpec;
    use bunny_ui::prelude::*;
    use bunny_ui_apple::ffi::{Id, Sel, class, sel};
    use bunny_ui_macos::{App, WindowSpec};
    use std::{
        ffi::{CStr, c_char},
        rc::Rc,
        time::Duration,
    };

    #[allow(
        clashing_extern_declarations,
        reason = "Objective-C selectors have distinct ABIs"
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
        fn boolean(value: Id, selector: Sel) -> i8;
        #[link_name = "objc_msgSend"]
        fn button(value: Id, selector: Sel, kind: u64) -> Id;
    }

    fn window(title: &str) -> Id {
        unsafe {
            let app = object(class("NSApplication"), sel("sharedApplication"));
            let windows = object(app, sel("windows"));
            (0..count(windows, sel("count")))
                .map(|i| at(windows, sel("objectAtIndex:"), i))
                .find(|window| {
                    let title_value = object(*window, sel("title"));
                    CStr::from_ptr(chars(title_value, sel("UTF8String"))).to_bytes()
                        == title.as_bytes()
                })
                .unwrap_or_else(|| panic!("native window missing: {title}"))
        }
    }

    fn eligible(title: &str, expected: bool) {
        for name in ["canBecomeKeyWindow", "canBecomeMainWindow"] {
            let actual = unsafe { boolean(window(title), sel(name)) != 0 };
            println!("{title}: {name}={actual}, expected={expected}");
            assert_eq!(actual, expected, "{title}: {name}");
        }
    }

    fn visible(title: &str) -> bool {
        unsafe { boolean(window(title), sel("isVisible")) != 0 }
    }

    fn buttons(title: &str) -> [Option<bool>; 3] {
        [0, 1, 2].map(|kind| unsafe {
            let control = button(window(title), sel("standardWindowButton:"), kind);
            (!control.is_null()).then(|| boolean(control, sel("isEnabled")) != 0)
        })
    }

    async fn await_visibility(title: &str, expected: bool) {
        // A bounded event-loop wait, not a timing assertion or performance sample.
        for _ in 0..200 {
            if visible(title) == expected {
                return;
            }
            task::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            visible(title),
            expected,
            "native visibility did not settle: {title}"
        );
    }

    pub fn run() {
        let app = App::new();
        let other = app.open(
            WindowSpec::titled("Unrelated"),
            Rc::new(app.runtime()),
            text("unrelated"),
        );
        let first = State::new(true);
        let nested = State::new(true);
        let runtime = Rc::new(app.runtime());
        let parent = app.open(
            WindowSpec::titled("First parent"),
            Rc::clone(&runtime),
            text("first").dialog(
                first.binding(),
                DialogSpec::titled("First dialog"),
                move |_| {
                    erased(text("dialog").dialog(
                        nested.binding(),
                        DialogSpec::titled("Nested dialog"),
                        |_| erased(text("nested")),
                    ))
                },
            ),
        );
        eligible("First parent", false);
        eligible("Unrelated", true);
        eligible("First dialog", false);
        eligible("Nested dialog", true);
        let second = State::new(true);
        let second_runtime = Rc::new(app.runtime());
        let second_parent = app.open(
            WindowSpec::titled("Second parent").no_minimize(),
            Rc::clone(&second_runtime),
            text("second").dialog(
                second.binding(),
                DialogSpec::titled("Second dialog"),
                |_| erased(text("second dialog")),
            ),
        );
        eligible("Second parent", false);
        eligible("Second dialog", true);
        eligible("Nested dialog", true);
        eligible("Unrelated", true);
        let ordinary_dialog_buttons = buttons("Second dialog");
        assert_eq!(
            ordinary_dialog_buttons[1],
            Some(false),
            "dialogs cannot minimize"
        );
        assert_eq!(
            buttons("First dialog"),
            [Some(false); 3],
            "lower dialog buttons are blocked"
        );
        let running = app.clone();
        task::spawn(async move {
            nested.set(false);
            runtime.request_frame();
            await_visibility("Nested dialog", false).await;
            eligible("First parent", false);
            eligible("First dialog", true);
            assert_eq!(buttons("First dialog"), ordinary_dialog_buttons, "nested teardown restores original button state");
            eligible("Second parent", false);
            eligible("Second dialog", true);
            for _ in 0..3 {
                first.set(false);
                runtime.request_frame();
                await_visibility("First dialog", false).await;
                eligible("First parent", true);
                eligible("Second parent", false);
                eligible("Unrelated", true);
                first.set(true);
                runtime.request_frame();
                await_visibility("First dialog", true).await;
                eligible("First parent", false);
                eligible("First dialog", true);
                eligible("Second parent", false);
            }
            running.hide(parent).unwrap();
            eligible("Unrelated", true);
            eligible("Second parent", false);
            running.show(parent).unwrap();
            await_visibility("First dialog", true).await;
            eligible("First parent", false);
            nested.set(true);
            runtime.request_frame();
            await_visibility("Nested dialog", true).await;
            eligible("First dialog", false);
            eligible("Nested dialog", true);
            running.close(parent);
            eligible("Unrelated", true);
            eligible("Second parent", false);
            eligible("Second dialog", true);
            second.set(false);
            second_runtime.request_frame();
            await_visibility("Second dialog", false).await;
            eligible("Second parent", true);
            assert_eq!(buttons("Second parent")[1], Some(false), "modality must not enable a prohibited minimize button");
            running.close(second_parent);
            eligible("Unrelated", true);
            println!("AppKit eligibility passed: two modal scopes, nesting, reopen, hide/show and parent teardown. Actual input/focus/pixels remain unqualified.");
            running.close(other);
        }).detach();
        app.run();
    }
}

#[cfg(target_os = "macos")]
fn main() {
    if std::env::var_os("BUNNY_MODALITY_CHILD").is_some() {
        native::run();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_MODALITY_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "native modality witness failed: {status}");
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native modality witness did not terminate");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {}
