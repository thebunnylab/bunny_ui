//! Public dialog placement on native windows; no physical-input or pixel qualification.
use crate::shell::{App, WindowSpec};
use bunny_ui::{
    layout::DialogSpec,
    prelude::*,
    window::{
        DesktopPosition, DesktopSpace, DialogGeometry, WindowError, WindowGeometry,
        WindowPlacement, WindowSize,
    },
};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Copy)]
struct Scene {
    open: State<bool>,
    controller: State<DialogGeometry>,
    nested: State<bool>,
    nested_controller: State<DialogGeometry>,
}
impl Component for Scene {
    fn body(self) -> impl View {
        text("parent").dialog(
            self.open.binding(),
            DialogSpec::titled("Geometry dialog")
                .min_size(280.0, 180.0)
                .geometry(self.controller.get()),
            move |_| {
                erased(
                    text("dialog content").dialog(
                        self.nested.binding(),
                        DialogSpec::titled("Nested geometry dialog")
                            .min_size(200.0, 120.0)
                            .geometry(self.nested_controller.get()),
                        |_| erased(text("nested content")),
                    ),
                )
            },
        )
    }
}

async fn until(stage: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..500 {
        if ready() {
            println!("Dialog stage passed: {stage}");
            return;
        }
        task::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        ready(),
        "native dialog geometry condition did not arrive: {stage}"
    );
}
fn placement(width: f64, height: f64, position: Option<DesktopPosition>) -> WindowPlacement {
    WindowPlacement::new(WindowSize::new(width, height).unwrap(), position)
}
fn size_is(controller: &DialogGeometry, width: f64, height: f64) -> bool {
    controller.geometry().is_some_and(|g| {
        (g.size().width() - width).abs() <= 1.0 && (g.size().height() - height).abs() <= 1.0
    })
}
fn save(path: &Path, value: WindowGeometry) {
    let (tag, x, y) = match value.position() {
        None => ("none", 0.0, 0.0),
        Some(position) => (
            match position.space() {
                DesktopSpace::AppKitPoints => "appkit",
                DesktopSpace::Win32Pixels => "win32",
                DesktopSpace::X11Pixels => "x11",
            },
            position.x(),
            position.y(),
        ),
    };
    std::fs::write(
        path,
        format!(
            "{} {} {tag} {x} {y}\n",
            value.size().width(),
            value.size().height()
        ),
    )
    .unwrap();
}
fn load(path: &Path) -> WindowPlacement {
    let raw = std::fs::read_to_string(path).unwrap();
    let words: Vec<_> = raw.split_whitespace().collect();
    assert_eq!(words.len(), 5);
    let position = match words[2] {
        "none" => None,
        tag => Some(
            DesktopPosition::new(
                words[3].parse().unwrap(),
                words[4].parse().unwrap(),
                match tag {
                    "appkit" => DesktopSpace::AppKitPoints,
                    "win32" => DesktopSpace::Win32Pixels,
                    "x11" => DesktopSpace::X11Pixels,
                    _ => panic!("unknown persisted space"),
                },
            )
            .unwrap(),
        ),
    };
    placement(
        words[0].parse().unwrap(),
        words[1].parse().unwrap(),
        position,
    )
}

fn run(path: &Path, fresh: bool) {
    let app = App::new();
    let runtime = Rc::new(app.runtime());
    let requested = if fresh {
        load(path)
    } else {
        placement(470.0, 310.0, None)
    };
    let first = DialogGeometry::restoring(requested);
    let scene = Scene {
        open: State::new(true),
        controller: State::new(first.clone()),
        nested: State::new(false),
        nested_controller: State::new(DialogGeometry::restoring(placement(340.0, 220.0, None))),
    };
    let seen = Rc::new(RefCell::new(Vec::new()));
    first.on_change({
        let seen = Rc::clone(&seen);
        move |value| {
            println!("Dialog geometry: {value:?}");
            seen.borrow_mut().push(value);
        }
    });
    let parent = app.open(
        WindowSpec::titled("Dialog geometry parent").size(800.0, 600.0),
        Rc::clone(&runtime),
        scene,
    );
    assert!(
        first.geometry().is_none(),
        "capture never synchronously invokes application code during opening"
    );
    let complete = Rc::new(Cell::new(false));
    let path = path.to_path_buf();
    app.open(WindowSpec::titled("Dialog geometry driver").size(160.0, 100.0), Rc::new(app.runtime()), text("driver").task({
        let app = app.clone();
        let complete = Rc::clone(&complete);
        move || {
            let app = app.clone();
            let first = first.clone();
            let runtime = Rc::clone(&runtime);
            let seen = Rc::clone(&seen);
            let complete = Rc::clone(&complete);
            let path = path.clone();
            async move {
                until("opening restore observed", || size_is(&first, requested.size().width(), requested.size().height())).await;
                assert!(first.last_error().is_none());
                if fresh {
                    assert_eq!(first.geometry().unwrap().placement(), requested, "a fresh process restores the persisted native content frame");
                    println!("Fresh-process native dialog placement round trip passed");
                    app.quit();
                    return;
                }
                let saved = first.geometry().unwrap();
                scene.open.set(false);
                runtime.request_frame();
                until("dismissed", || runtime.overlays().is_empty()).await;
                assert_eq!(first.geometry(), Some(saved));
                let before = seen.borrow().len();
                scene.open.set(true);
                runtime.request_frame();
                until("reopened", || seen.borrow().len() > before).await;
                assert_eq!(first.geometry().unwrap().placement(), saved.placement());

                let nested = scene.nested_controller.get();
                scene.nested.set(true);
                runtime.request_frame();
                until("nested dialog independently observed", || size_is(&nested, 340.0, 220.0)).await;
                assert_eq!(first.geometry(), Some(saved));
                scene.nested.set(false);
                runtime.request_frame();
                until("nested dialog dismissed alone", || runtime.overlays().len() == 1).await;
                assert_eq!(first.geometry(), Some(saved));
                assert!(nested.begin().is_ok());

                let independent = DialogGeometry::restoring(placement(390.0, 260.0, None));
                let independent_runtime = Rc::new(app.runtime());
                let independent_id = app.open(
                    WindowSpec::titled("Independent dialog parent").size(800.0, 600.0),
                    Rc::clone(&independent_runtime),
                    Scene {
                        open: State::new(true), controller: State::new(independent.clone()),
                        nested: State::new(false), nested_controller: State::new(DialogGeometry::new()),
                    },
                );
                until("independent scene's dialog observed", || size_is(&independent, 390.0, 260.0)).await;
                app.close(independent_id);
                assert!(independent.begin().is_ok());
                assert_eq!(first.geometry(), Some(saved));

                let moved = saved.position().map(|p| DesktopPosition::new(p.x() + 16.0, p.y() + 16.0, p.space()).unwrap());
                let second = DialogGeometry::restoring(placement(580.0, 380.0, moved));
                scene.controller.set(second.clone());
                runtime.request_frame();
                until("live controller replacement", || size_is(&second, 580.0, 380.0)).await;
                assert_eq!(first.geometry(), Some(saved), "old controller cannot receive the replacement's events");
                if let Some(moved) = moved {
                    assert_eq!(second.geometry().unwrap().position(), Some(moved));
                }
                let actual = second.geometry().unwrap();
                save(&path, actual);
                let wrong_space = match actual.position().map(DesktopPosition::space) {
                    Some(DesktopSpace::Win32Pixels) => DesktopSpace::AppKitPoints,
                    _ => DesktopSpace::Win32Pixels,
                };
                let invalid = DialogGeometry::restoring(placement(777.0, 888.0, Some(DesktopPosition::new(10.0, 10.0, wrong_space).unwrap())));
                scene.controller.set(invalid.clone());
                runtime.request_frame();
                until("foreign desktop position rejected", || invalid.last_error().is_some() && invalid.geometry().is_some()).await;
                assert!(matches!(invalid.last_error(), Some(WindowError::InvalidGeometry(_) | WindowError::Unsupported(_))));
                assert_eq!(invalid.geometry().unwrap(), actual, "rejection precedes native size mutation");

                let tiny = DialogGeometry::restoring(placement(10.0, 10.0, None));
                scene.controller.set(tiny.clone());
                runtime.request_frame();
                until("minimum respected", || size_is(&tiny, 280.0, 180.0)).await;
                let minimum = tiny.geometry().unwrap().size();
                assert!(minimum.width() >= 279.0 && minimum.height() >= 179.0, "actual minimum: {minimum:?}");
                if let Some(origin) = actual.position() {
                    let missing = DesktopPosition::new(30_000.0, 30_000.0, origin.space()).unwrap();
                    let fitted = DialogGeometry::restoring(placement(360.0, 240.0, Some(missing)));
                    scene.controller.set(fitted.clone());
                    runtime.request_frame();
                    until("removed display fitted", || size_is(&fitted, 360.0, 240.0) && fitted.geometry().unwrap().position() != Some(missing)).await;
                    assert!(fitted.last_error().is_none());
                }
                let closing = DialogGeometry::restoring(placement(450.0, 290.0, None));
                closing.on_change({
                    let app = app.clone();
                    let complete = Rc::clone(&complete);
                    let weak = Rc::downgrade(&runtime);
                    move |value| {
                        if value.is_ok_and(|g| (g.size().width() - 450.0).abs() <= 1.0 && (g.size().height() - 290.0).abs() <= 1.0) {
                            let runtime = weak.upgrade().unwrap();
                            let path = runtime.overlays()[0].path.clone();
                            let lease = runtime.dialog_geometry_session(&path).unwrap();
                            app.close(parent);
                            assert!(lease.capture(value).is_none(), "destroyed parent invalidates even a retained runtime");
                            complete.set(true);
                        }
                    }
                });
                scene.controller.set(closing.clone());
                runtime.request_frame();
                until("parent closed inside native callback", || complete.get()).await;
                closing.clear_handler();
                assert!(closing.begin().is_ok());
                println!("Native dialog lifecycle, validation and persistence passed; physical input/pixels remain unqualified");
                app.quit();
            }
        }
    }));
    app.run();
    assert!(complete.get() || fresh);
}

fn child(path: &Path, phase: &str) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_DIALOG_GEOMETRY_CHILD", phase)
        .env("BUNNY_DIALOG_GEOMETRY_FILE", path)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "dialog geometry {phase}: {status}");
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("dialog geometry {phase} timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
pub fn main() {
    if let Ok(phase) = std::env::var("BUNNY_DIALOG_GEOMETRY_CHILD") {
        run(
            Path::new(&std::env::var_os("BUNNY_DIALOG_GEOMETRY_FILE").unwrap()),
            phase == "fresh",
        );
        return;
    }
    let directory =
        std::env::temp_dir().join(format!("bunny-dialog-geometry-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("placement.txt");
    child(&path, "lifecycle");
    assert!(
        path.is_file(),
        "the native lifecycle wrote an observed placement"
    );
    child(&path, "fresh");
    std::fs::remove_dir_all(directory).unwrap();
}
