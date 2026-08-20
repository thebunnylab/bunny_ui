//! What the RUST side costs per official operation — the counters the
//! browser cannot see. The harness times the whole path (script plus
//! paint); this says how much work the render and the diff really did
//! for a change the app made.
//!
//! ```sh
//! cargo run --release -p bench-web --example ops_cost
//! ```

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;
use bunny_ui::stats;
use std::rc::Rc;

use bench_web::keyed::{App, RowSeed};

const SIZE: Size = Size { width: 1200.0, height: 800.0 };

fn seeds(from: usize, count: usize) -> Vec<RowSeed> {
    (0..count)
        .map(|i| RowSeed {
            id: from + i,
            label: State::new(Rc::from(format!("row {}", from + i).as_str())),
            selected: State::new(false),
        })
        .collect()
}

/// What KIND of patch the mount spends itself on — the create path's
/// bill, one line per kind.
fn histogram(patches: &[bunny_ui::dom::DomPatch]) {
    use bunny_ui::dom::DomPatch;
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for patch in patches {
        let kind = match patch {
            DomPatch::Create { .. } => "create",
            DomPatch::Remove { .. } => "remove",
            DomPatch::SetText { .. } => "text",
            DomPatch::SetStyle { .. } => "style",
            DomPatch::SetLayout { .. } => "layout",
            DomPatch::SetHints { .. } => "hints",
            DomPatch::Move { .. } => "move",
            _ => "other",
        };
        match counts.iter_mut().find(|(name, _)| *name == kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((kind, 1)),
        }
    }
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let line: Vec<String> =
        counts.iter().map(|(name, count)| format!("{name} {count}")).collect();
    println!("{:<16} {}", "  patch mix", line.join(", "));
}

fn report(label: &str, frame: stats::FrameStats, patches: usize) {
    println!(
        "{label:<16} bodies {:>5}  built {:>6}  visited {:>6}  reused {:>5}  patches {:>5}",
        frame.body_passes, frame.capture_nodes, frame.diff_visited, frame.diff_reused, patches
    );
    // where the frame's milliseconds went — the stages, in order
    use stats::Stage;
    println!(
        "{:<16} settle {:>6.3}  build {:>6.3}  diff {:>6.3}  encode {:>6.3}  = {:>6.3} ms",
        "  stages",
        frame.ms(Stage::Settle),
        frame.ms(Stage::Capture),
        frame.ms(Stage::Diff),
        frame.ms(Stage::Encode),
        frame.ms(Stage::Settle)
            + frame.ms(Stage::Capture)
            + frame.ms(Stage::Diff)
            + frame.ms(Stage::Encode),
    );
}

/// A clock for the stage timers — the engine takes a function, so the
/// host decides what "now" means (the browser hands it `performance.now`).
fn now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs_f64() * 1000.0
}

fn main() {
    stats::set_clock(Some(now_ms));
    let runtime = Runtime::new();
    let app = App {
        rows: State::new(Rc::new(Vec::new())),
        selected: State::new(None),
        next_id: State::new(1),
    };

    // create 1,000 rows
    let _ = runtime.dom_frame(&app, SIZE);
    let _ = stats::take();
    app.rows.set(Rc::new(seeds(1, 1_000)));
    let patches = runtime.dom_frame(&app, SIZE);
    report("create 1k", stats::take(), patches.len());
    histogram(&patches);

    // select one row: the row's OWN state changes
    app.rows.get()[500].selected.set(true);
    let patches = runtime.dom_frame(&app, SIZE);
    report("select row", stats::take(), patches.len());

    // update every 10th label: a hundred rows, each its own state
    for row in app.rows.get().iter().step_by(10) {
        row.label.set(Rc::from(format!("{} !!!", row.label.get()).as_str()));
    }
    let patches = runtime.dom_frame(&app, SIZE);
    report("partial update", stats::take(), patches.len());

    // swap two rows: the LIST changes, the rows do not
    let swapped = {
        let mut rows = (*app.rows.get()).clone();
        rows.swap(1, 998);
        rows
    };
    app.rows.set(Rc::new(swapped));
    let patches = runtime.dom_frame(&app, SIZE);
    report("swap rows", stats::take(), patches.len());

    // remove one row: same shape of change
    let shorter = {
        let mut rows = (*app.rows.get()).clone();
        rows.remove(1);
        rows
    };
    app.rows.set(Rc::new(shorter));
    let patches = runtime.dom_frame(&app, SIZE);
    report("remove row", stats::take(), patches.len());

    // clear
    app.rows.set(Rc::new(Vec::new()));
    let patches = runtime.dom_frame(&app, SIZE);
    report("clear rows", stats::take(), patches.len());
}
