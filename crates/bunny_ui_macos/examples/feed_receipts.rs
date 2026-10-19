//! Synthetic frame IDs for an independent window capture; no camera or input.
//! Pass an existing control directory. See `tests/feed_receipts.py`.
#![forbid(unsafe_code)]

#[cfg(target_os = "macos")]
fn main() {
    use bunny_ui::image_engine::ImageFeed;
    use bunny_ui::prelude::*;
    use bunny_ui_macos::{App, WindowSpec};
    use std::{path::PathBuf, rc::Rc, time::Duration};

    #[derive(Clone)]
    struct Pane {
        feed: ImageFeed,
        repaint: State<u64>,
    }
    impl Component for Pane {
        fn body(self) -> impl View {
            let _ = self.repaint.get();
            image(&self.feed).resizable().frame(320.0, 160.0)
        }
    }
    fn pixels(marker: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 * 4);
        for bit in 0..8 {
            let value = if marker & (1 << bit) == 0 { 0 } else { 255 };
            bytes.extend_from_slice(&[value, value, value, 255]);
        }
        bytes
    }
    async fn checkpoint(folder: &std::path::Path, stage: &str) {
        std::fs::write(folder.join("stage"), stage).expect("write stage");
        for _ in 0..600 {
            if std::fs::read_to_string(folder.join("continue")).is_ok_and(|value| value == stage) {
                return;
            }
            task::sleep(Duration::from_millis(25)).await;
        }
        panic!("external capture did not acknowledge {stage}");
    }
    let folder = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("control directory");
    let app = App::new();
    let feed = ImageFeed::new();
    feed.push((8, 1), pixels(0));
    let repaint = State::new(0);
    let window = app.open(
        WindowSpec::titled("Feed receipts").size(320.0, 160.0),
        Rc::new(app.runtime()),
        Pane {
            feed: feed.clone(),
            repaint,
        },
    );
    let observer = app
        .observe_feed(window, feed.key(), 64)
        .expect("open window");
    println!(
        "SESSION {} FEED {}",
        observer.session(),
        observer.feed().raw()
    );
    let checking = app.clone();
    task::spawn(async move {
        for (stage, marker) in [
            ("frame-2", 85),
            ("repeat-2", 85),
            ("frame-4", 170),
            ("frame-5", 240),
        ] {
            if stage != "repeat-2" {
                if stage == "frame-4" {
                    feed.push((8, 1), pixels(15));
                }
                let generation = feed.push((8, 1), pixels(marker));
                println!("PUSH {generation} MARKER {marker}");
            }
            repaint.add(1);
            task::sleep(Duration::from_millis(400)).await;
            checkpoint(&folder, stage).await;
            for event in observer.drain() {
                println!("EVENT {stage} {event:?}");
            }
            println!("SNAPSHOT {stage} {:?}", observer.snapshot());
        }
        // Keep the run loop alive while observing the first window's teardown.
        let sentinel = checking.open(
            WindowSpec::titled("Receipt teardown").size(100.0, 80.0),
            Rc::new(checking.runtime()),
            spacer(),
        );
        checking.close(window);
        println!("CLOSED {:?}", observer.snapshot());
        for event in observer.drain() {
            println!("EVENT closed {event:?}");
        }
        checking.close(sentinel);
    })
    .detach();
    app.run();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
