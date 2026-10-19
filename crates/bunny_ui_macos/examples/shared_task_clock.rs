//! Measures a scoped sleep with one, two, three, then two native windows.
//!
//! `cargo run --release -p bunny-ui-macos --example shared_task_clock`
//! prints every unrounded elapsed duration and exits nonzero if any 600ms
//! deadline fires before 580ms. No keyboard input or benchmark ranking is involved.

#[cfg(target_os = "macos")]
fn main() {
    use bunny_ui::prelude::*;
    use bunny_ui_macos::{App, CoreTextEngine, WindowSpec};
    use std::{
        rc::Rc,
        time::{Duration, Instant},
    };

    fn open(app: &App, title: &'static str) -> bunny_ui_macos::WindowId {
        let runtime = Rc::new(app.runtime().text_engine(Rc::new(CoreTextEngine::new())));
        app.open(
            WindowSpec::titled(title).size(360.0, 180.0),
            runtime,
            vstack!(
                text(title).font(Font::Title),
                text("Each sleeper waits the same real interval.")
            )
            .padding(),
        )
    }
    let app = App::new();
    open(&app, "Shared task clock · one");
    let checking = app.clone();
    task::spawn(async move {
        task::sleep(Duration::from_millis(200)).await;
        let mut passed = true;
        for count in [1, 2, 3, 2] {
            while checking.windows().len() < count {
                open(&checking, "Shared task clock · additional");
            }
            while checking.windows().len() > count {
                if let Some(id) = checking.windows().last().copied() {
                    checking.close(id);
                }
            }
            let started = Instant::now();
            task::sleep(Duration::from_millis(600)).await;
            let elapsed = started.elapsed();
            let ok = elapsed >= Duration::from_millis(580);
            passed &= ok;
            println!(
                "SLEEP windows={count} requested_ms=600 elapsed_ms={:.6} passed={ok}",
                elapsed.as_secs_f64() * 1000.0
            );
        }
        // This executable is the process-level regression: all owned native
        // resources are reclaimed by the OS on either result.
        std::process::exit(i32::from(!passed));
    })
    .detach();
    app.run();
}
#[cfg(not(target_os = "macos"))]
fn main() {}
