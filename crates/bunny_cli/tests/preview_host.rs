//! The preview road end to end, with its clock: a project `bunny new`
//! writes, built hot with its preview entries and started as a host; the
//! template's own `home_preview` selected and framed; a click on its
//! button answered with a new frame; a save reloaded with the count kept.
//! Every step prints `@@bunny-timing <step> <ms>`, and under
//! `BUNNY_TIMING_STRICT=1` the budgets of NFR-STUDIO-1 are asserted:
//! a warm start to the first frame within 3 s, a save to its frame within
//! 800 ms, a click to its frame within 20 ms.
//!
//! Slow — it builds the framework as a shared library — so it runs on
//! request: `cargo test -p bunny-cli --test preview_host -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bunny_cli::preview::{Event, Options, Preview, Step};
use bunny_ui_preview::wire::{Frame, PointerKind, Selection, ToHost, ToIde, Traits};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf()
}

fn strict() -> bool {
    std::env::var_os("BUNNY_TIMING_STRICT").is_some()
}

/// Prints a step's time, and holds it to `budget` in strict mode.
fn timing(step: &str, took: Duration, budget: Option<Duration>) {
    let ms = took.as_secs_f64() * 1000.0;
    println!("@@bunny-timing {step} {ms:.1}");
    if let Some(budget) = budget
        && strict()
    {
        assert!(took <= budget, "{step} took {ms:.1} ms, over {} ms", budget.as_millis());
    }
}

/// Waits up to `timeout` for a message `pick` accepts, polling the lane.
fn expect<T>(live: &mut Preview<bunny_cli::preview::Live>, timeout: Duration, mut pick: impl FnMut(&ToIde) -> Option<T>) -> T {
    let until = Instant::now() + timeout;
    let mut events = Vec::new();
    loop {
        let alive = live.wait(Duration::from_millis(20), &mut events);
        for event in events.drain(..) {
            match &event {
                Event::Host(message) => {
                    if let Some(found) = pick(message) {
                        return found;
                    }
                }
                Event::Crashed { code } => panic!("the host ended: {code:?}"),
                Event::Note(note) => println!("note: {note}"),
                Event::BuildFailed { diagnostics } => {
                    for diagnostic in diagnostics {
                        print!("{}", diagnostic.rendered);
                    }
                    panic!("the build failed");
                }
                _ => {}
            }
        }
        assert!(alive, "the host is gone");
        assert!(Instant::now() < until, "nothing matched within {} s", timeout.as_secs());
    }
}

fn frame_of(ident: &str) -> impl FnMut(&ToIde) -> Option<Frame> + '_ {
    move |message| match message {
        ToIde::Frame(frame) if frame.preview == ident => Some(frame.clone()),
        _ => None,
    }
}

fn says(frame: &Frame, text: &str) -> bool {
    frame.strings.iter().any(|s| s.contains(text))
}

fn start(lane: Preview<bunny_cli::preview::Idle>) -> Preview<bunny_cli::preview::Live> {
    let mut events = Vec::new();
    match lane.start(&mut events) {
        Step::Live(live) => {
            for event in &events {
                match event {
                    Event::Note(note) => println!("note: {note}"),
                    Event::Diagnostic(diagnostic) if diagnostic.level == "error" => print!("{}", diagnostic.rendered),
                    _ => {}
                }
            }
            live
        }
        Step::Failed(failed) => {
            for event in &events {
                if let Event::BuildFailed { diagnostics } = event {
                    for diagnostic in diagnostics {
                        print!("{}", diagnostic.rendered);
                    }
                }
            }
            panic!("the lane did not start: {}", failed.why());
        }
    }
}

#[test]
#[ignore = "builds the framework as a shared library; run with --ignored"]
fn the_preview_road_frames_clicks_and_survives_a_swap() {
    let here = std::env::temp_dir().join(format!("bunny-preview-{}", std::process::id()));
    let _ = fs::remove_dir_all(&here);
    fs::create_dir_all(&here).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(["new", "preview_app", "--org", "io.bunny"])
        .arg("--bunny-ui-path")
        .arg(repo().join("crates/bunny_ui_facade"))
        .current_dir(&here)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let app = here.join("preview_app");
    // one target directory for the road, shared across runs of this test
    fs::create_dir_all(app.join(".cargo")).unwrap();
    fs::write(
        app.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = {:?}\n", repo().join("target/bunny-preview-e2e").to_string_lossy()),
    )
    .unwrap();

    // the cold start builds the framework; the warm one is the budgeted one
    let began = Instant::now();
    let live = start(Preview::new(&app, Options::default()).unwrap());
    timing("cold_build", began.elapsed(), None);
    live.stop();

    let began = Instant::now();
    let mut live = start(Preview::new(&app, Options::default()).unwrap());
    let previews = expect(&mut live, Duration::from_secs(30), |message| match message {
        ToIde::Ready { previews, .. } => Some(previews.clone()),
        _ => None,
    });
    let home = previews.iter().find(|preview| preview.ident == "home_preview").expect("the template's preview is listed");
    live.send(&ToHost::Select(vec![Selection { id: home.id.clone(), ident: home.ident.clone(), traits: Traits::default() }])).unwrap();
    let frame = expect(&mut live, Duration::from_secs(30), frame_of("home_preview"));
    timing("warm_start", began.elapsed(), Some(Duration::from_secs(3)));
    assert!(says(&frame, "You tapped 0 times"), "{:?}", frame.strings);

    // a click on the button, found by its site
    let button = frame
        .sites
        .iter()
        .find(|site| frame.strings.get(site.kind as usize).is_some_and(|kind| kind == "button"))
        .expect("the button has a site");
    let (x, y) = (button.frame.origin.x + button.frame.size.width / 2.0, button.frame.origin.y + button.frame.size.height / 2.0);
    let began = Instant::now();
    live.send(&ToHost::Pointer { id: home.id.clone(), kind: PointerKind::Down, x, y, modifiers: 0 }).unwrap();
    live.send(&ToHost::Pointer { id: home.id.clone(), kind: PointerKind::Up, x, y, modifiers: 0 }).unwrap();
    let frame = expect(&mut live, Duration::from_secs(10), |message| match message {
        ToIde::Frame(frame) if frame.preview == "home_preview" && says(frame, "You tapped 1 times") => Some(frame.clone()),
        _ => None,
    });
    timing("click", began.elapsed(), Some(Duration::from_millis(20)));
    assert!(frame.seq >= 2);

    // a save: the label changes, the count stays
    let lib = app.join("src/lib.rs");
    let source = fs::read_to_string(&lib).unwrap();
    fs::write(&lib, source.replace("\"Tap me\"", "\"Tap again\"")).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while live.saved().is_none() {
        assert!(Instant::now() < until, "the save was not noticed");
        std::thread::sleep(Duration::from_millis(50));
    }
    let began = Instant::now();
    let mut events = Vec::new();
    live.reload(&mut events);
    assert!(events.iter().any(|event| matches!(event, Event::Reloaded { .. })), "{events:?}");
    let frame = expect(&mut live, Duration::from_secs(10), |message| match message {
        ToIde::Frame(frame) if frame.preview == "home_preview" && says(frame, "Tap again") => Some(frame.clone()),
        _ => None,
    });
    timing("save", began.elapsed(), Some(Duration::from_millis(800)));
    assert!(says(&frame, "You tapped 1 times"), "the state survived the swap: {:?}", frame.strings);
    assert!(frame.generation >= 2);

    live.stop();
    let _ = fs::remove_dir_all(&here);
}
