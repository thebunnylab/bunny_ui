//! `git log` in a window: a worker thread reads the process and the
//! rows arrive one by one, while the scene stays live — scroll, hover
//! and the reload button all answer during the load.
//!
//! The framework opens no process. The example does, inside its own
//! `.task`, and hands the lines over a channel; the engine only learns
//! the result. Closing the window (or reloading) cancels the task, the
//! reader dies, and the next `send` tells the worker to stop.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example git_window
//! ```

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui_macos::Chrome;

const BAR_H: f64 = 44.0;
const LIGHTS_W: f64 = 78.0;
const ROW_H: f64 = 30.0;
/// The demo paces the stream so the crossing is VISIBLE. A real panel
/// sends as fast as the process writes.
const PACE: Duration = Duration::from_millis(6);

#[derive(Clone, Debug)]
struct Commit {
    hash: String,
    author: String,
    subject: String,
}

#[derive(Clone, Copy)]
struct App {
    commits: State<Rc<Vec<Commit>>>,
    reloads: State<usize>,
    reading: State<bool>,
}

impl Component for App {
    fn body(self, _ctx: &Context) -> impl View {
        let commits = self.commits;
        let reloads = self.reloads;
        let reading = self.reading;
        let rows = commits.get();
        let count = rows.len();

        let title = hstack!(
            spacer().frame(LIGHTS_W, 1.0),
            text("git log").bold(),
            text(if reading.get() { "reading…" } else { "done" })
                .font_size(11.0)
                .foreground_color(theme::fg_faint()),
            spacer(),
            text(format!("{count} commits"))
                .font_size(11.0)
                .foreground_color(theme::fg_secondary()),
            text("reload")
                .font_size(11.0)
                .foreground_color(theme::fg_secondary())
                .foreground_hovered(Color::WHITE)
                .padding_edge(Edge::Leading, 10.0)
                .padding_edge(Edge::Trailing, 10.0)
                .padding_edge(Edge::Top, 5.0)
                .padding_edge(Edge::Bottom, 5.0)
                .background_color(theme::control())
                .background_hovered(theme::row_hover())
                .corner_radius(7.0)
                .on_click(move || reloads.update(|n| *n += 1)),
        )
        .spacing(10.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Trailing, 12.0)
        .frame_max(f64::INFINITY, BAR_H, Alignment::Leading)
        .background_color(theme::panel())
        .window_drag_region();

        let id_rows = Rc::clone(&rows);
        let list = virtual_list(
            count,
            move |index| id_rows[index].hash.clone(),
            move |index| {
                let commit = &rows[index];
                hstack!(
                    text(commit.hash.clone())
                        .monospaced()
                        .font_size(11.0)
                        .foreground_color(theme::accent()),
                    text(commit.subject.clone()),
                    spacer(),
                    text(commit.author.clone()).font_size(11.0),
                )
                .spacing(10.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Leading, 14.0)
                .padding_edge(Edge::Trailing, 14.0)
                .frame_max(f64::INFINITY, ROW_H, Alignment::Leading)
                // the ink comes from the row: faint at rest, bright
                // under the pointer
                .foreground_color(theme::fg_secondary())
                .foreground_hovered(theme::fg())
                .background_hovered(theme::row_hover())
            },
        );

        vstack!(
            title,
            spacer().frame(1.0, 1.0).background_color(theme::divider()),
            zstack!(spacer().background_color(theme::canvas()), list),
        )
        // the id is the reload count: pressing reload cancels the read
        // in flight and starts a fresh one
        .task_id(reloads.get(), move || async move {
            commits.set(Rc::new(Vec::new()));
            reading.set(true);
            let (lines, reader) = task::channel::<Commit>();
            // the APP owns the process and the thread
            std::thread::spawn(move || read_the_log(&lines));
            let mut all = Vec::new();
            while let Some(commit) = reader.recv().await {
                all.push(commit);
                commits.set(Rc::new(all.clone()));
            }
            reading.set(false);
        })
    }
}

/// Runs `git log` and sends one commit per line. An `Err` from send
/// means the view is gone: stop reading, and let the process go.
fn read_the_log(lines: &task::Sender<Commit>) {
    let child = Command::new("git")
        .args(["log", "--max-count=400", "--pretty=format:%h\u{1}%an\u{1}%s"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return };
    let Some(output) = child.stdout.take() else { return };
    for line in BufReader::new(output).lines().map_while(Result::ok) {
        let mut parts = line.split('\u{1}');
        let commit = Commit {
            hash: parts.next().unwrap_or_default().to_string(),
            author: parts.next().unwrap_or_default().to_string(),
            subject: parts.next().unwrap_or_default().to_string(),
        };
        std::thread::sleep(PACE);
        if lines.send(commit).is_err() {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn main() {
    theme::install(Theme::dark());
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_macos::CoreTextEngine::new()))
        .image_engine(Rc::new(bunny_ui_macos::CoreGraphicsImageEngine::new()));
    bunny_ui_macos::run_window_chrome(
        "bunny — git log",
        Size { width: 900.0, height: 620.0 },
        Chrome::Scene,
        runtime,
        App {
            commits: State::new(Rc::new(Vec::new())),
            reloads: State::new(0),
            reading: State::new(false),
        },
    );
}
