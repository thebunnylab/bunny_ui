//! Main-thread checked-consumer regression for Objective-C BOOL, including modal gating.
#![forbid(unsafe_code)]

#[cfg(target_os = "macos")]
mod native {
    use bunny_ui::layout::DialogSpec;
    use bunny_ui::prelude::*;
    use bunny_ui_macos::{App, CloseAction, WindowSpec};
    use objc2::{
        MainThreadMarker,
        encode::Encode,
        rc::Retained,
        runtime::{AnyClass, Bool, Sel},
    };
    use objc2_app_kit::{NSApplication, NSWindow};
    use std::{cell::Cell, ffi::CString, rc::Rc};

    fn window(main: MainThreadMarker, title: &str) -> Retained<NSWindow> {
        NSApplication::sharedApplication(main)
            .windows()
            .iter()
            .find(|window| window.title().to_string() == title)
            .unwrap_or_else(|| panic!("missing native window {title}"))
    }

    fn registrations() {
        let encoding = Bool::ENCODING.to_string();
        for (name, selectors) in [
            (
                "BunnyWindow",
                &["canBecomeKeyWindow", "canBecomeMainWindow"][..],
            ),
            ("BunnyDelegate", &["windowShouldClose:"][..]),
            ("BunnyDialogDelegate", &["windowShouldClose:"][..]),
            (
                "BunnyView",
                &[
                    "acceptsFirstResponder",
                    "hasMarkedText",
                    "performKeyEquivalent:",
                    "performDragOperation:",
                    "validateMenuItem:",
                    "isAccessibilityElement",
                ][..],
            ),
            ("BunnyMenuTarget", &["validateMenuItem:"][..]),
            (
                "BunnyAccessibilityElement",
                &[
                    "isAccessibilityElement",
                    "isAccessibilityEnabled",
                    "isAccessibilityFocused",
                    "accessibilityPerformPress",
                    "isAccessibilitySelectorAllowed:",
                ][..],
            ),
        ] {
            let name = CString::new(name).unwrap();
            let class = AnyClass::get(&name).unwrap();
            for selector in selectors {
                let selector = CString::new(*selector).unwrap();
                let method = class.instance_method(Sel::register(&selector)).unwrap();
                assert_eq!(
                    method.return_type().to_str().unwrap(),
                    encoding,
                    "{name:?} {selector:?}"
                );
            }
        }
        let class = AnyClass::get(c"BunnyAccessibilityElement").unwrap();
        let method = class
            .instance_method(Sel::register(c"setAccessibilityFocused:"))
            .unwrap();
        assert_eq!(method.argument_type(2).unwrap().to_str().unwrap(), encoding);
        println!(
            "All 16 BOOL returns and the accessibility BOOL argument match the native target ({encoding})."
        );
    }

    fn delegate_registration() {
        let class = AnyClass::get(c"BunnyAppDelegate").unwrap();
        let method = class
            .instance_method(Sel::register(
                c"applicationShouldHandleReopen:hasVisibleWindows:",
            ))
            .unwrap();
        let encoding = Bool::ENCODING.to_string();
        assert_eq!(method.return_type().to_str().unwrap(), encoding);
        assert_eq!(method.argument_type(3).unwrap().to_str().unwrap(), encoding);
        println!(
            "Application delegate BOOL result and argument match the native target ({encoding})."
        );
    }

    pub fn run() {
        let main = MainThreadMarker::new().unwrap();
        let app = App::new();
        delegate_registration();
        if std::env::args().any(|argument| argument == "--delegate-only") {
            return;
        }
        let unrelated = app.open(
            WindowSpec::titled("BOOL unrelated"),
            Rc::new(app.runtime()),
            text("unrelated"),
        );
        let live = window(main, "BOOL unrelated");
        // These safe generated bindings enable objc2's debug signature checks.
        assert!(live.canBecomeKeyWindow());
        assert!(live.canBecomeMainWindow());
        assert!(live.contentView().unwrap().acceptsFirstResponder());
        registrations();
        let requested = Rc::new(Cell::new(false));
        let observed = Rc::clone(&requested);
        app.on_close_requested(unrelated, move |_| {
            observed.set(true);
            CloseAction::Cancel
        })
        .unwrap();
        live.performClose(None);
        assert!(requested.get(), "safe native close must reach the delegate");
        assert!(app.windows().contains(&unrelated), "Cancel must veto close");
        let shown = State::new(true);
        let parent = app.open(
            WindowSpec::titled("BOOL parent"),
            Rc::new(app.runtime()),
            text("parent").dialog(shown.binding(), DialogSpec::titled("BOOL dialog"), |_| {
                erased(text("dialog"))
            }),
        );
        assert!(!window(main, "BOOL parent").canBecomeKeyWindow());
        assert!(!window(main, "BOOL parent").canBecomeMainWindow());
        assert!(window(main, "BOOL dialog").canBecomeKeyWindow());
        assert!(live.canBecomeKeyWindow(), "unrelated window stays eligible");
        app.close(parent);
        assert!(live.canBecomeKeyWindow());
        println!(
            "Checked BOOL consumers passed: eligible/blocked windows, view, native close veto and scoped teardown."
        );
        app.close(unrelated);
    }
}

#[cfg(target_os = "macos")]
fn main() {
    if std::env::var_os("BUNNY_BOOL_CHILD").is_some() {
        native::run();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(std::env::args_os().skip(1))
        .env("BUNNY_BOOL_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "checked BOOL consumer failed: {status}");
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("checked BOOL consumer did not terminate");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {}
