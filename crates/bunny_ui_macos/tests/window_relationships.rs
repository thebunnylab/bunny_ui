//! Main-thread AppKit relationship and independently addressed resize witness.
#[cfg(target_os = "macos")]
use bunny_ui_macos as shell;
#[cfg(target_os = "macos")]
#[path = "../../bunny_ui/tests/support/window_relationships.rs"]
mod contract;
#[cfg(target_os = "macos")]
mod native {
    use bunny_ui::prelude::*;
    use bunny_ui_apple::ffi::{CGSize, Id, Sel, class, sel};
    use std::{
        ffi::{CStr, c_char},
        rc::Rc,
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
        fn size(value: Id, selector: Sel, size: CGSize);
    }
    fn window(title: &str) -> Id {
        unsafe {
            let windows = object(
                object(class("NSApplication"), sel("sharedApplication")),
                sel("windows"),
            );
            (0..count(windows, sel("count")))
                .map(|i| at(windows, sel("objectAtIndex:"), i))
                .find(|&window| {
                    CStr::from_ptr(chars(object(window, sel("title")), sel("UTF8String")))
                        .to_bytes()
                        == title.as_bytes()
                })
                .expect("this process's native window")
        }
    }
    pub fn parent(child: &str, expected: Option<&str>) {
        let parent = unsafe { object(window(child), sel("parentWindow")) };
        assert_eq!(parent, expected.map_or(std::ptr::null_mut(), window));
    }
    pub fn routing() {
        let app = crate::shell::App::new();
        let main = Rc::new(app.runtime());
        let inspector = Rc::new(app.runtime());
        let parent = app.open(
            crate::shell::WindowSpec::titled("Routing parent").size(400.0, 300.0),
            Rc::clone(&main),
            text("main"),
        );
        let child = app.open(
            crate::shell::WindowSpec::titled("Routing inspector").size(200.0, 180.0),
            Rc::clone(&inspector),
            text("inspector"),
        );
        app.set_parent(child, Some(parent)).unwrap();
        let parent_before = main.last_viewport().unwrap();
        unsafe {
            size(
                window("Routing inspector"),
                sel("setContentSize:"),
                CGSize {
                    width: 321.0,
                    height: 145.0,
                },
            );
        }
        assert_eq!(
            inspector.last_viewport().unwrap().width,
            321.0,
            "native child resize must reach the child's scene"
        );
        assert_eq!(
            main.last_viewport().unwrap(),
            parent_before,
            "parent scene is independent"
        );
        println!("AppKit child resize reaches its own scene");
        app.quit();
    }
}
#[cfg(target_os = "macos")]
fn main() {
    match std::env::var("BUNNY_RELATIONSHIP_CHILD").as_deref() {
        Ok("routing") => native::routing(),
        Ok("contract") => contract::run(native::parent),
        _ => {
            for mode in ["contract", "routing"] {
                let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                    .env("BUNNY_RELATIONSHIP_CHILD", mode)
                    .spawn()
                    .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
                loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        assert!(status.success(), "{mode} native witness failed: {status}");
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        child.kill().unwrap();
                        child.wait().unwrap();
                        panic!("{mode} native witness timed out");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {}
