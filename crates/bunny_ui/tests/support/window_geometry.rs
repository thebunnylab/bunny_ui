//! Native geometry requests and observations through the public shell API.
use crate::shell::{
    App, DesktopPosition, DesktopSpace, WindowError, WindowGeometry, WindowPlacement, WindowSize,
    WindowSpec,
};
use bunny_ui::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

fn placement(width: f64, height: f64) -> WindowPlacement {
    WindowPlacement::new(WindowSize::new(width, height).unwrap(), None)
}

fn matches_size(value: WindowGeometry, width: f64, height: f64) -> bool {
    (value.size().width() - width).abs() <= 1.0 && (value.size().height() - height).abs() <= 1.0
}

async fn until(mut ready: impl FnMut() -> bool) {
    // Bounded event-loop progress, not a guessed delay followed by an assertion.
    for _ in 0..500 {
        if ready() {
            return;
        }
        task::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        ready(),
        "native geometry condition did not arrive; see preceding observation stage"
    );
}

fn run() {
    let app = App::new();
    let parent_runtime = Rc::new(app.runtime());
    let parent = app.open(
        WindowSpec::titled("Geometry parent").size(480.0, 320.0),
        Rc::clone(&parent_runtime),
        text("parent"),
    );
    let child_runtime = Rc::new(app.runtime());
    let child_id = Rc::new(Cell::new(None));
    let completed = Rc::new(Cell::new(false));
    let child = app.open(
        WindowSpec::titled("Geometry child").size(320.0, 200.0),
        Rc::clone(&child_runtime),
        text("child"),
    );
    child_id.set(Some(child));
    app.open(WindowSpec::titled("Geometry driver").size(160.0, 100.0), Rc::new(app.runtime()), text("driver").task({
        let app = app.clone();
        let child_id = Rc::clone(&child_id);
        let child_runtime = Rc::clone(&child_runtime);
        let completed = Rc::clone(&completed);
        move || {
            let app = app.clone();
            let child_id = Rc::clone(&child_id);
            let parent_runtime = Rc::clone(&parent_runtime);
            let child_runtime = Rc::clone(&child_runtime);
            let completed = Rc::clone(&completed);
            async move {
                until(|| child_id.get().is_some()).await;
                let child = child_id.get().unwrap();
                app.set_parent(child, Some(parent)).unwrap();
                until(|| app.geometry(child).is_ok()).await;
                let initial = app.geometry(child).unwrap();
                let parent_before = parent_runtime.last_viewport().unwrap();
                let seen = Rc::new(RefCell::new(Vec::new()));
                app.on_geometry_changed(child, {
                    let seen = Rc::clone(&seen);
                    move |id, value| { assert_eq!(id, child); seen.borrow_mut().push(value); }
                }).unwrap();
                assert_eq!(app.geometry(child).unwrap(), initial);
                assert!(seen.borrow().is_empty(), "query is not a notification");
                let foreign = App::new();
                assert_eq!(foreign.geometry(child), Err(WindowError::StaleWindow));
                assert_eq!(foreign.restore_placement(child, placement(360.0, 240.0)), Err(WindowError::StaleWindow));
                assert_eq!(foreign.on_geometry_changed(child, |_, _| {}), Err(WindowError::StaleWindow));
                assert_eq!(foreign.clear_geometry_handler(child), Err(WindowError::StaleWindow));
                let wrong_space = match initial.position().map(DesktopPosition::space) {
                    Some(DesktopSpace::Win32Pixels) => DesktopSpace::AppKitPoints,
                    _ => DesktopSpace::Win32Pixels,
                };
                let wrong = WindowPlacement::new(WindowSize::new(999.0, 888.0).unwrap(), Some(DesktopPosition::new(10.0, 10.0, wrong_space).unwrap()));
                assert!(matches!(app.restore_placement(child, wrong), Err(WindowError::InvalidGeometry(_) | WindowError::Unsupported(_))));
                assert_eq!(app.geometry(child).unwrap(), initial, "rejection must precede size mutation");
                app.restore_placement(child, placement(360.0, 240.0)).unwrap();
                println!("First restore readback: {:?}", app.geometry(child));
                until(|| seen.borrow().iter().any(|value| matches_size(*value, 360.0, 240.0))).await;
                until(|| child_runtime.last_viewport().is_some_and(|size| (size.width - 360.0).abs() <= 1.0)).await;
                assert_eq!(parent_runtime.last_viewport().unwrap(), parent_before);
                assert!(matches_size(app.geometry(child).unwrap(), 360.0, 240.0));
                println!("Native content size observed independently of its parent");
                if let Some(origin) = app.geometry(child).unwrap().position() {
                    let moved = DesktopPosition::new(origin.x() + 16.0, origin.y() + 16.0, origin.space()).unwrap();
                    app.restore_placement(child, WindowPlacement::new(WindowSize::new(360.0, 240.0).unwrap(), Some(moved))).unwrap();
                    until(|| seen.borrow().iter().any(|value| value.position() == Some(moved) && matches_size(*value, 360.0, 240.0))).await;
                    println!("Move-only native observation received without resizing content");
                }


                seen.borrow_mut().clear();
                app.on_geometry_changed(child, {
                    let app = app.clone();
                    let seen = Rc::clone(&seen);
                    let requested = Cell::new(false);
                    move |id, value| {
                        println!("Reentrant observation: {value:?}");
                        seen.borrow_mut().push(value);
                        if matches_size(value, 400.0, 260.0) && !requested.replace(true) {
                            app.restore_placement(id, placement(430.0, 280.0)).unwrap();
                        }
                    }
                }).unwrap();
                app.restore_placement(child, placement(400.0, 260.0)).unwrap();
                until(|| seen.borrow().iter().any(|value| matches_size(*value, 430.0, 280.0))).await;
                let first = seen.borrow().iter().position(|value| matches_size(*value, 400.0, 260.0)).unwrap();
                let second = seen.borrow().iter().position(|value| matches_size(*value, 430.0, 280.0)).unwrap();
                assert!(first < second, "reentrant native observations retain order");
                println!("Reentrant restoration complete");
                app.clear_geometry_handler(child).unwrap();
                app.hide(parent).unwrap();
                app.show(parent).unwrap();
                println!("Parent remap requested: {:?}", app.geometry(child));
                until(|| app.geometry(child).is_ok_and(|value| matches_size(value, 430.0, 280.0))).await;
                if let Some(position) = initial.position() {
                    let missing = DesktopPosition::new(30_000.0, 30_000.0, position.space()).unwrap();
                    app.restore_placement(child, WindowPlacement::new(WindowSize::new(380.0, 250.0).unwrap(), Some(missing))).unwrap();
                    until(|| app.geometry(child).is_ok_and(|value| matches_size(value, 380.0, 250.0) && value.position() != Some(missing))).await;
                    let saved = app.geometry(child).unwrap().placement();
                    app.restore_placement(child, saved).unwrap();
                    until(|| app.geometry(child).is_ok_and(|value| value.placement() == saved)).await;
                    println!("Off-screen restore and content-position round trip observed");
                } else {
                    println!("Wayland global position explicitly unavailable");
                }
                let closed = Rc::new(Cell::new(false));
                app.on_geometry_changed(child, {
                    let app = app.clone();
                    let closed = Rc::clone(&closed);
                    move |id, value| {
                        if matches_size(value, 450.0, 290.0) {
                            app.close(id);
                            closed.set(true);
                        }
                    }
                }).unwrap();
                let result = app.restore_placement(child, placement(450.0, 290.0));
                assert!(matches!(result, Ok(()) | Err(WindowError::StaleWindow)));
                until(|| closed.get()).await;
                assert_eq!(app.geometry(child), Err(WindowError::StaleWindow));
                assert_eq!(app.clear_geometry_handler(child), Err(WindowError::StaleWindow));
                println!("Native geometry contract passed; physical input, visible pixels and mixed-DPI remain unqualified");
                completed.set(true);
                app.quit();
            }
        }
    }));
    child_id.set(Some(child));
    app.run();
    assert!(
        completed.get(),
        "fixture exited before completing assertions"
    );
    println!(
        "Native geometry contract passed; physical input, visible pixels and mixed-DPI remain unqualified"
    );
}

pub fn main() {
    if std::env::var_os("BUNNY_GEOMETRY_CHILD").is_some() {
        run();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_GEOMETRY_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(result) = child.try_wait().unwrap() {
            assert!(result.success(), "native geometry fixture: {result}");
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native geometry fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
