//! The arena's scaffolding: the script a worker thread paces with a real
//! clock, the fixtures, and the `FIRST_FRAME` print every framework's app
//! makes so the launch is measured the same way everywhere.
//!
//! The engine's own clock is the frame tick; a script that slept on it would
//! pace itself by the thing it measures. So the worker keeps the time and
//! hands each step to a task on the main thread, which raises it through the
//! shell's own door (`bunny_ui_macos::drive`) — no CGEvent, no Accessibility.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bunny_ui::prelude::*;

/// Wheel steps a second — faster than any display, which is the point.
pub const STEPS_PER_SECOND: u64 = 240;
/// The window every arena app opens: the workbench's own size.
pub const WINDOW: (f64, f64) = (1280.0, 800.0);

/// The face shared by the text-bearing macOS scenes. Naming the face is
/// part of the fixture: a platform's default would change glyph widths,
/// wrapping and raster work between measurements. It must be installed.
pub const FONT_FAMILY: &str = "Menlo";

/// What the script asked for, from the command line.
#[derive(Clone, Debug)]
pub struct Args {
    pub script: String,
    pub secs: f64,
    pub rows: usize,
    pub lines: usize,
}

impl Args {
    /// `--script S --secs N --rows N --lines N`, with the arena's defaults.
    #[must_use]
    pub fn parse() -> Args {
        let mut args = Args {
            script: "rest".to_owned(),
            secs: 30.0,
            rows: 10_000,
            lines: 400,
        };
        let mut it = std::env::args().skip(1);
        while let Some(flag) = it.next() {
            let value = it.next();
            match (flag.as_str(), value) {
                ("--script", Some(v)) => args.script = v,
                ("--secs", Some(v)) => args.secs = v.parse().unwrap_or(args.secs),
                ("--rows", Some(v)) => args.rows = v.parse().unwrap_or(args.rows),
                ("--lines", Some(v)) => args.lines = v.parse().unwrap_or(args.lines),
                _ => {}
            }
        }
        args
    }
}

/// The fixtures directory: `ARENA_FIXTURES`, or the repository's private one.
#[must_use]
pub fn fixtures() -> PathBuf {
    std::env::var_os("ARENA_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../gitignore/arena/fixtures"),
        PathBuf::from,
    )
}

/// The rows of `rows-10k.tsv`, the first `count` of them, six cells each.
#[must_use]
pub fn rows(count: usize) -> Vec<[String; 6]> {
    let text = std::fs::read_to_string(fixtures().join("rows-10k.tsv")).unwrap_or_default();
    text.lines()
        .skip(1)
        .take(count)
        .map(|line| {
            let mut cells = line.split('\t').map(str::to_owned);
            std::array::from_fn(|_| cells.next().unwrap_or_default())
        })
        .collect()
}

/// The editor's text: `lines-400.txt` or `lines-30k.txt`.
#[must_use]
pub fn lines(count: usize) -> String {
    let file = if count > 400 {
        "lines-30k.txt"
    } else {
        "lines-400.txt"
    };
    std::fs::read_to_string(fixtures().join(file)).unwrap_or_default()
}

/// The wall clock in milliseconds since the epoch — the one clock the
/// orchestrator and every app share.
#[must_use]
pub fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Says the first frame happened, once. Called from a `.task` on the root,
/// which runs after the root's first pass — the frame that shows it is a
/// beat away, the same beat every framework's first-frame callback is.
pub fn first_frame() {
    println!("FIRST_FRAME {}", unix_ms());
}

/// One step of a script, from the worker's clock to the main thread.
#[derive(Clone, Copy, Debug)]
pub enum Step {
    Wheel { x: f64, y: f64, dy: f64 },
    Type(char),
    Backspace,
    Append,
    Done,
}

/// The scripts, as the worker plays them. `rest` is `secs` of nothing;
/// `wheel` is 240 steps a second over `(x, y)`, down for half of `secs`
/// and back up; `type` alternates a character and a backspace ten times a
/// second; `append` types a character ten times a second, so the text is
/// new on every stroke; `stream` appends every 33 ms; `loop` is `rest`
/// with a loop on screen; `soak` is `rest` for a long while.
pub fn play(args: &Args, over: (f64, f64), send: impl Fn(Step) -> bool) {
    let secs = args.secs.max(0.1);
    // the moment the steps begin, on the shared clock: a measuring run
    // anchors its window here rather than on the first frame, which a
    // framework may report late
    let started = || println!("SCRIPT_START {}", unix_ms());
    match args.script.as_str() {
        "wheel" => {
            std::thread::sleep(Duration::from_secs(1));
            started();
            let steps = (secs * STEPS_PER_SECOND as f64) as u64;
            let pause = Duration::from_micros(1_000_000 / STEPS_PER_SECOND);
            for step in 0..steps {
                let dy = if step < steps / 2 { -6.0 } else { 6.0 };
                if !send(Step::Wheel {
                    x: over.0,
                    y: over.1,
                    dy,
                }) {
                    return;
                }
                std::thread::sleep(pause);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        "type" | "append" => {
            std::thread::sleep(Duration::from_secs(1));
            started();
            let strokes = (secs * 10.0) as u64;
            let append = args.script == "append";
            for stroke in 0..strokes {
                let step = if append || stroke % 2 == 0 {
                    Step::Type('x')
                } else {
                    Step::Backspace
                };
                if !send(step) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        "stream" => {
            std::thread::sleep(Duration::from_secs(1));
            started();
            let appends = (secs * 1000.0 / 33.0) as u64;
            for _ in 0..appends {
                if !send(Step::Append) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(33));
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        _ => std::thread::sleep(Duration::from_secs_f64(secs)),
    }
    let _ = send(Step::Done);
}

/// Raises a step through the shell's door, on the main thread.
pub fn raise(step: Step) {
    match step {
        Step::Wheel { x, y, dy } => bunny_ui_macos::drive::wheel(x, y, 0.0, dy),
        Step::Type(c) => bunny_ui_macos::drive::text(&c.to_string()),
        Step::Backspace => bunny_ui_macos::drive::backspace(),
        Step::Append | Step::Done => {}
    }
}

/// The script, mounted on a view: the worker starts when the view first
/// appears, the first frame is announced, and `Done` ends the process.
/// `on_step` sees every step before it is raised — a stream appends there.
pub fn scripted<V: View<Arity = Single>>(
    view: V,
    args: Args,
    over: (f64, f64),
    on_step: impl Fn(Step) + 'static,
) -> impl View {
    let on_step = std::rc::Rc::new(on_step);
    view.task(move || {
        let args = args.clone();
        let on_step = on_step.clone();
        async move {
            first_frame();
            let (sender, receiver) = task::channel::<Step>();
            std::thread::spawn(move || play(&args, over, |step| sender.send(step).is_ok()));
            while let Some(step) = receiver.recv().await {
                on_step(step);
                if matches!(step, Step::Done) {
                    std::process::exit(0);
                }
                raise(step);
            }
        }
    })
}
