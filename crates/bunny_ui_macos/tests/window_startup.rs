//! Main-thread startup and structure-return regression against checked AppKit geometry.
#![forbid(unsafe_code)]

#[cfg(target_os = "macos")]
const COMPLETE: &str =
    "Native startup, initial viewport, rectangle conversion, resize and teardown passed.";

#[cfg(target_os = "macos")]
fn run() {
    use bunny_ui::prelude::*;
    use bunny_ui_macos::{App, WindowPlacement, WindowSize, WindowSpec};
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use std::rc::Rc;

    let main = MainThreadMarker::new().unwrap();
    let app = App::new();
    let runtime = Rc::new(app.runtime());
    let id = app.open(
        WindowSpec::titled("Native startup geometry").size(640.0, 400.0),
        Rc::clone(&runtime),
        text("Initial layout and paint must complete"),
    );
    let window = NSApplication::sharedApplication(main)
        .windows()
        .iter()
        .find(|window| window.title().to_string() == "Native startup geometry")
        .unwrap();
    let view = window.contentView().unwrap();
    let check = |width: f64, height: f64| {
        // objc2 selects the native message ABI independently of the shell's FFI.
        let bounds = view.bounds();
        let content = window.contentRectForFrameRect(window.frame());
        let screen = window.convertRectToScreen(bounds);
        let geometry = app.geometry(id).unwrap();
        let viewport = runtime.last_viewport().unwrap();
        for (actual, expected) in [
            (bounds.size.width, width),
            (bounds.size.height, height),
            (content.size.width, width),
            (content.size.height, height),
            (screen.size.width, width),
            (screen.size.height, height),
            (geometry.size().width(), width),
            (geometry.size().height(), height),
            (viewport.width, width),
            (viewport.height, height),
        ] {
            assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
        }
        assert!(screen.origin.x.is_finite() && screen.origin.y.is_finite());
        assert!(geometry.position().is_some());
    };
    check(640.0, 400.0);
    app.restore_placement(
        id,
        WindowPlacement::new(WindowSize::new(560.0, 360.0).unwrap(), None),
    )
    .unwrap();
    check(560.0, 360.0);
    // Retain another lifetime so closing the tested window cannot terminate
    // the process before the teardown assertions have run.
    let keeper = app.open(
        WindowSpec::titled("Startup lifetime keeper").size(160.0, 100.0),
        Rc::new(app.runtime()),
        text("keeper"),
    );
    app.close(id);
    assert_eq!(app.windows(), vec![keeper]);
    println!("{COMPLETE}");
    app.close(keeper);
}

#[cfg(target_os = "macos")]
fn main() {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    if std::env::var_os("BUNNY_STARTUP_CHILD").is_some() {
        run();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_STARTUP_CHILD", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "native startup regression: {status}");
            let output = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
            assert!(
                output.contains(COMPLETE),
                "startup exited before completing its assertions"
            );
            print!("{output}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native startup regression timed out");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
