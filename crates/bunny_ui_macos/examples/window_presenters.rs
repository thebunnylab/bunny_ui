//! Native pixel regression for independent window presenters.
//!
//! Run with `python3 crates/bunny_ui_macos/tests/window_presenters.py` on macOS.
//! The driver captures only this process's windows; it sends no user input.
#![forbid(unsafe_code)]

#[cfg(target_os = "macos")]
fn main() {
    use bunny_ui::prelude::*;
    use bunny_ui_macos::{App, WindowSpec};
    use std::{path::PathBuf, rc::Rc, time::Duration};

    #[derive(Clone, Copy)]
    struct Pane(State<Color>);
    impl Component for Pane {
        fn body(self) -> impl View {
            spacer().background_color(self.0.get())
        }
    }
    fn publish(folder: &std::path::Path, stage: &str) {
        if let Err(error) = std::fs::write(folder.join("stage"), stage) {
            eprintln!("Cannot publish stage: {error}");
            std::process::exit(2);
        }
    }
    async fn observed(folder: &std::path::Path, stage: &str) {
        publish(folder, stage);
        loop {
            if std::fs::read_to_string(folder.join("continue")).is_ok_and(|value| value == stage) {
                break;
            }
            task::sleep(Duration::from_millis(25)).await;
        }
    }
    let Some(folder) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("Pass an existing control directory");
        std::process::exit(2);
    };
    let app = App::new();
    let primary = Pane(State::new(Color::hex(0xff0000)));
    let first = app.open(
        WindowSpec::titled("Presenter primary").size(260.0, 180.0),
        Rc::new(app.runtime()),
        primary,
    );
    let test_app = app.clone();
    task::spawn(async move {
        task::sleep(Duration::from_millis(300)).await;
        observed(&folder, "initial").await;
        let secondary = Pane(State::new(Color::hex(0x0000ff)));
        let second = test_app.open(
            WindowSpec::titled("Presenter secondary").size(260.0, 180.0),
            Rc::new(test_app.runtime()),
            secondary,
        );
        primary.0.set(Color::hex(0x00ff00));
        secondary.0.set(Color::hex(0xffff00));
        task::sleep(Duration::from_millis(600)).await;
        observed(&folder, "dual").await;
        test_app.close(second);
        primary.0.set(Color::hex(0x00ffff));
        task::sleep(Duration::from_millis(600)).await;
        observed(&folder, "survivor").await;
        for index in 0..3 {
            let next = test_app.open(
                WindowSpec::titled("Presenter secondary").size(260.0, 180.0),
                Rc::new(test_app.runtime()),
                Pane(State::new(Color::hex(0xff00ff))),
            );
            task::sleep(Duration::from_millis(100)).await;
            test_app.close(next);
            primary.0.set(if index == 2 {
                Color::hex(0xffffff)
            } else {
                Color::hex(0x0000ff)
            });
            task::sleep(Duration::from_millis(250)).await;
        }
        observed(&folder, "repeated").await;
        let survivor = Pane(State::new(Color::hex(0xff00ff)));
        let last = test_app.open(
            WindowSpec::titled("Presenter secondary").size(260.0, 180.0),
            Rc::new(test_app.runtime()),
            survivor,
        );
        test_app.close(first);
        survivor.0.set(Color::hex(0x0000ff));
        task::sleep(Duration::from_millis(600)).await;
        observed(&folder, "reverse-survivor").await;
        publish(&folder, "closing-last");
        test_app.close(last);
    })
    .detach();
    app.run();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
