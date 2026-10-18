//! The preview road end to end, with its clock: a project `bunny new`
//! writes, built hot with its preview entries and started as a host; the
//! template's real `app!` root selected and framed; a click on its
//! button answered with a new frame; a save reloaded with the count kept;
//! a save in a sibling crate's body reloaded too, and a type moved there
//! a restart, said.
//! Then `bunny preview --png` takes a picture of the same preview through
//! the host's snapshot door — and, on the Mac, of a preview holding a
//! page, the page's own pixels where its box stood. Every step prints
//! `@@bunny-timing <step> <ms>`, and under
//! `BUNNY_TIMING_STRICT=1` the budgets of NFR-STUDIO-1 are asserted:
//! a warm start to the first frame within 3 s, a save to its frame within
//! 800 ms, a click and an override to their frames within 20 ms each.
//!
//! Slow — it builds the framework as a shared library — so it runs on
//! request: `cargo test -p bunny-cli --test preview_host -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bunny_cli::preview::{Event, Options, Preview, Step};
use bunny_ui_preview::wire::{Frame, OverrideOp, Patch, PointerKind, Selection, SiteKey, ToHost, ToIde};

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
            assert!(
                events.iter().any(
                    |event| matches!(event, Event::Sources { generation: 1, sources: Some(sources) }
                if sources.files().keys().any(|path| path.ends_with("src/main.rs"))
                    && sources.files().keys().any(|path| path.ends_with("src/lib.rs")))
                ),
                "startup provenance: {events:?}"
            );
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

/// Two copies of the real app keep their state, sizes and overrides through a swap.
fn exercise_instances(
    live: &mut Preview<bunny_cli::preview::Live>,
    home: &bunny_ui_preview::wire::PreviewInfo,
    lib: &std::path::Path,
) {
    use bunny_ui_preview::wire::{InstanceCommand as C, InstanceSelection, SceneInstanceId, Size};
    let a = SceneInstanceId::parse("artboard:1").unwrap();
    let b = SceneInstanceId::parse("artboard:2").unwrap();
    live.send(&ToHost::SelectInstances(vec![
        InstanceSelection {
            instance: a.clone(),
            ident: home.ident.clone(),
            traits: home.traits.clone(),
        },
        InstanceSelection {
            instance: b.clone(),
            ident: home.ident.clone(),
            traits: home.traits.clone(),
        },
    ]))
    .unwrap();
    let mut first = std::collections::BTreeMap::new();
    expect(live, Duration::from_secs(10), |message| {
        if let ToIde::InstanceFrame { instance, frame } = message {
            first.insert(instance.clone(), frame.as_ref().clone());
        }
        (first.len() == 2).then_some(())
    });
    assert!(
        first
            .values()
            .all(|frame| says(frame, "You tapped 0 times"))
    );
    let initial = &first[&a];
    let button = initial
        .sites
        .iter()
        .find(|site| initial.strings[site.kind as usize] == "button")
        .unwrap();
    let (x, y) = (
        button.frame.origin.x + button.frame.size.width / 2.0,
        button.frame.origin.y + button.frame.size.height / 2.0,
    );
    let began = Instant::now();
    for kind in [PointerKind::Down, PointerKind::Up] {
        live.send(&ToHost::Instance {
            instance: a.clone(),
            variant: 0,
            command: C::Pointer {
                kind,
                x,
                y,
                modifiers: 0,
            },
        })
        .unwrap();
    }
    let frame = expect(live, Duration::from_secs(10), |message| match message {
        ToIde::InstanceFrame { instance, frame }
            if instance == &a && says(frame, "You tapped 1 times") =>
        {
            Some(frame.as_ref().clone())
        }
        _ => None,
    });
    timing(
        "instance_click",
        began.elapsed(),
        Some(Duration::from_millis(20)),
    );
    let padding = frame
        .sites
        .iter()
        .find(|site| {
            site.modifier
                .is_some_and(|name| frame.strings[name as usize] == "padding")
        })
        .unwrap();
    let site = SiteKey {
        file: frame.strings[padding.file.unwrap() as usize].clone(),
        line: padding.line,
        column: padding.column,
    };
    let text_x = |frame: &Frame| {
        frame
            .commands
            .iter()
            .find_map(|command| match command {
                bunny_ui_preview::wire::Draw::TextLine { origin, .. } => Some(origin.x),
                _ => None,
            })
            .unwrap()
    };
    let before = text_x(&frame);
    live.send(&ToHost::Instance {
        instance: b.clone(),
        variant: 0,
        command: C::Resize(Size {
            width: 600.0,
            height: 700.0,
        }),
    })
    .unwrap();
    let before_b = expect(live, Duration::from_secs(10), |message| match message {
        ToIde::InstanceFrame { instance, frame } if instance == &b && frame.size.width == 600.0 => {
            Some(text_x(frame))
        }
        _ => None,
    });
    live.send(&ToHost::Instance {
        instance: a.clone(),
        variant: 0,
        command: C::Override {
            site,
            op: OverrideOp::Replace,
            patch: Patch::PaddingLength(64.0),
        },
    })
    .unwrap();
    expect(live, Duration::from_secs(10), |message| match message {
        ToIde::InstanceFrame { instance, frame } if instance == &a && text_x(frame) > before => {
            Some(())
        }
        _ => None,
    });
    live.send(&ToHost::Instance {
        instance: b.clone(),
        variant: 0,
        command: C::Resize(Size {
            width: 600.0,
            height: 701.0,
        }),
    })
    .unwrap();
    let other = expect(live, Duration::from_secs(10), |message| match message {
        ToIde::InstanceFrame { instance, frame }
            if instance == &b && frame.size.height == 701.0 =>
        {
            Some(frame.as_ref().clone())
        }
        _ => None,
    });
    assert!(says(&other, "You tapped 0 times"));
    assert_eq!(
        text_x(&other),
        before_b,
        "another instance's padding did not leak"
    );
    live.send(&ToHost::Instance {
        instance: a.clone(),
        variant: 0,
        command: C::ClearOverrides,
    })
    .unwrap();
    expect(live, Duration::from_secs(10), |message| match message {
        ToIde::InstanceFrame { instance, frame } if instance == &a && text_x(frame) == before => {
            Some(())
        }
        _ => None,
    });
    let source = fs::read_to_string(lib).unwrap();
    let broken = source.replace("text(\"Tap again\")", "text(missing_reload_value)");
    assert_ne!(source, broken);
    fs::write(lib, &broken).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while live.saved().is_none() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut failed = Vec::new();
    live.reload(&mut failed);
    let diagnostics = match failed.as_slice() {
        [Event::BuildStarted, Event::BuildFailed { diagnostics }] => diagnostics,
        _ => panic!("a failed body build must keep the live host: {failed:?}"),
    };
    let problem = diagnostics
        .iter()
        .find(|problem| problem.level == "error" && problem.message.contains("missing_reload_value"))
        .expect("the failed reload must preserve the compiler diagnostic");
    assert_eq!(problem.code.as_deref(), Some("E0425"));
    assert_eq!(problem.file.as_deref().map(Path::new), Some(Path::new("src/lib.rs")));
    let line = broken.lines().nth(problem.line.unwrap() as usize - 1).unwrap();
    assert!(line[problem.column.unwrap() as usize - 1..].starts_with("missing_reload_value"));
    for instance in [&a, &b] {
        live.send(&ToHost::Instance {
            instance: instance.clone(),
            variant: 0,
            command: C::Resize(Size {
                width: if instance == &a { 420.0 } else { 600.0 },
                height: 702.0,
            }),
        })
        .unwrap();
        let retained = expect(live, Duration::from_secs(10), |message| match message {
            ToIde::InstanceFrame { instance: current, frame }
                if current == instance && frame.size.height == 702.0 =>
            {
                Some(frame.as_ref().clone())
            }
            _ => None,
        });
        assert_eq!(retained.generation, other.generation);
        assert!(says(&retained, "Tap again"));
        assert!(says(
            &retained,
            if instance == &a { "You tapped 1 times" } else { "You tapped 0 times" }
        ));
    }
    fs::write(lib, source.replace("Tap again", "Tap together")).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while live.saved().is_none() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(50));
    }
    let began = Instant::now();
    let mut events = Vec::new();
    live.reload(&mut events);
    let mut updated = std::collections::BTreeMap::new();
    expect(live, Duration::from_secs(10), |message| {
        if let ToIde::InstanceFrame { instance, frame } = message
            && says(frame, "Tap together")
        {
            updated.insert(instance.clone(), frame.as_ref().clone());
        }
        (updated.len() == 2).then_some(())
    });
    timing(
        "instance_save",
        began.elapsed(),
        Some(Duration::from_millis(800)),
    );
    assert!(says(&updated[&a], "You tapped 1 times"));
    assert!(says(&updated[&b], "You tapped 0 times"));
    assert_eq!(updated[&a].size.width, 420.0);
    assert_eq!(updated[&b].size.width, 600.0);
    assert!(updated.values().all(|frame| events.iter().any(|event| matches!(event, Event::Sources { generation, sources: Some(_) } if *generation == frame.generation))));
    live.send(&ToHost::Instance {
        instance: b.clone(),
        variant: 0,
        command: C::RequestSnapshot,
    })
    .unwrap();
    expect(live, Duration::from_secs(10), |message| match message {
        ToIde::InstanceSnapshot {
            instance,
            size: (600, 702),
            ..
        } if instance == &b => Some(()),
        _ => None,
    });
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
    // a sibling crate beside the app: the greeting comes from it
    let kit = here.join("ui_kit");
    fs::create_dir_all(kit.join("src")).unwrap();
    fs::write(kit.join("Cargo.toml"), "[package]\nname = \"ui_kit\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n").unwrap();
    fs::write(kit.join("src/lib.rs"), "pub fn greeting() -> &'static str {\n    \"Hello from bunny-ui\"\n}\n").unwrap();
    let manifest = app.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    assert!(text.contains("[dependencies]\n"), "{text}");
    fs::write(&manifest, text.replacen("[dependencies]\n", "[dependencies]\nui_kit = { path = \"../ui_kit\" }\n", 1)).unwrap();
    let lib = app.join("src/lib.rs");
    let source = fs::read_to_string(&lib).unwrap();
    assert!(source.contains("text(\"Hello from bunny-ui\")"), "{source}");
    fs::write(&lib, source.replace("text(\"Hello from bunny-ui\")", "text(ui_kit::greeting())")).unwrap();
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
    let home = previews.first().expect("the template's app is listed");
    assert_eq!(home.ident, "app");
    assert_eq!(home.traits.size, bunny_ui_preview::wire::Sizing::Fixed(bunny_ui_preview::wire::Size { width: 420.0, height: 640.0 }));
    live.send(&ToHost::Select(vec![Selection { id: home.id.clone(), ident: home.ident.clone(), traits: home.traits.clone() }])).unwrap();
    let frame = expect(&mut live, Duration::from_secs(30), frame_of("app"));
    timing("warm_start", began.elapsed(), Some(Duration::from_secs(3)));
    timing("app_frame", began.elapsed(), Some(Duration::from_secs(3)));
    assert_eq!(
        frame.size,
        bunny_ui_preview::wire::Size {
            width: 420.0,
            height: 640.0
        }
    );
    assert!(
        frame.sites.iter().any(|site| site
            .file
            .is_some_and(|file| frame.strings[file as usize].replace('\\', "/").starts_with("src/"))),
        "app source sites are recorded"
    );
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
        ToIde::Frame(frame) if frame.preview == "app" && says(frame, "You tapped 1 times") => Some(frame.clone()),
        _ => None,
    });
    timing("click", began.elapsed(), Some(Duration::from_millis(20)));
    assert!(frame.seq >= 2);

    // an override: the stack's padding replaced, the picture moving before any build
    let padding = frame
        .sites
        .iter()
        .find(|site| site.modifier.is_some_and(|name| frame.strings.get(name as usize).is_some_and(|name| name == "padding")))
        .expect("the padding has a site");
    let site = SiteKey { file: frame.strings[padding.file.expect("a recorded site") as usize].clone(), line: padding.line, column: padding.column };
    let text_x = |frame: &Frame| {
        frame.commands.iter().find_map(|command| match command {
            bunny_ui_preview::wire::Draw::TextLine { origin, .. } => Some(origin.x),
            _ => None,
        })
    };
    let before = text_x(&frame).expect("a text line");
    let began = Instant::now();
    live.send(&ToHost::Override { id: home.id.clone(), site: site.clone(), op: OverrideOp::Replace, patch: Patch::PaddingLength(64.0) }).unwrap();
    let frame = expect(&mut live, Duration::from_secs(10), |message| match message {
        ToIde::Frame(frame) if frame.preview == "app" && text_x(frame).is_some_and(|x| x != before) => Some(frame.clone()),
        _ => None,
    });
    timing("override", began.elapsed(), Some(Duration::from_millis(20)));
    assert!(text_x(&frame).unwrap() > before, "the text moved right under the wider padding");
    live.send(&ToHost::ClearOverrides { id: home.id.clone() }).unwrap();
    let frame = expect(&mut live, Duration::from_secs(10), |message| match message {
        ToIde::Frame(frame) if frame.preview == "app" && text_x(frame) == Some(before) => Some(frame.clone()),
        _ => None,
    });
    assert!(says(&frame, "You tapped 1 times"), "the count survived the override: {:?}", frame.strings);

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
        ToIde::Frame(frame) if frame.preview == "app" && says(frame, "Tap again") => Some(frame.clone()),
        _ => None,
    });
    assert!(events.iter().any(|event| matches!(event, Event::Sources { generation, sources: Some(sources) }
        if *generation == frame.generation && sources.files().iter().any(|(path, source)| fs::canonicalize(path).is_ok_and(|path| path == fs::canonicalize(&lib).unwrap()) && source.contains("Tap again")))), "frame generation {}, path {:?}, receipts {:?}", frame.generation, lib, events.iter().filter_map(|event| match event { Event::Sources { generation, sources } => Some((*generation, sources.as_ref().map(|sources| sources.files().keys().filter(|path| path.ends_with("src/lib.rs")).cloned().collect::<Vec<_>>()))), _ => None }).collect::<Vec<_>>());
    timing("save", began.elapsed(), Some(Duration::from_millis(800)));
    assert!(says(&frame, "You tapped 1 times"), "the state survived the swap: {:?}", frame.strings);
    assert!(frame.generation >= 2);

    // a save in the sibling's body: the crate is built again, the
    // generation that links it loads, the count stays
    let kit_lib = kit.join("src/lib.rs");
    let kit_source = fs::read_to_string(&kit_lib).unwrap();
    fs::write(&kit_lib, kit_source.replace("Hello from bunny-ui", "Hello from ui_kit")).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while live.saved().is_none() {
        assert!(Instant::now() < until, "the sibling's save was not noticed");
        std::thread::sleep(Duration::from_millis(50));
    }
    let began = Instant::now();
    let mut events = Vec::new();
    live.reload(&mut events);
    assert!(events.iter().any(|event| matches!(event, Event::Reloaded { .. })), "a body edit in a sibling reloads: {events:?}");
    let frame = expect(&mut live, Duration::from_secs(10), |message| match message {
        ToIde::Frame(frame) if frame.preview == "app" && says(frame, "Hello from ui_kit") => Some(frame.clone()),
        _ => None,
    });
    timing("sibling_save", began.elapsed(), Some(Duration::from_millis(800)));
    assert!(says(&frame, "You tapped 1 times"), "the state survived the sibling's swap: {:?}", frame.strings);

    exercise_instances(&mut live, home, &lib);

    // a type moved in the sibling: no salt renames its types, so the
    // road asks for a restart and says whose
    fs::write(&kit_lib, format!("{}\npub struct Greeting;\n", fs::read_to_string(&kit_lib).unwrap())).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while live.saved().is_none() {
        assert!(Instant::now() < until, "the sibling's second save was not noticed");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut events = Vec::new();
    live.reload(&mut events);
    assert!(
        events.iter().any(|event| matches!(event, Event::RestartNeeded(why) if why.contains("ui_kit"))),
        "a type in a sibling asks for a restart by name: {events:?}"
    );

    // Registration wakes queued work; restart keeps the same notification.
    let (wake, awake) = std::sync::mpsc::channel();
    live.set_waker(Some(std::sync::Arc::new(move || { let _ = wake.send(()); })));
    awake.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut events = Vec::new();
    let mut live = match live.restart(&mut events) {
        Step::Live(live) => live,
        Step::Failed(failed) => panic!("restart: {}", failed.why()),
    };
    awake.recv_timeout(Duration::from_secs(1)).expect("the retained hook wakes the restarted owner");
    expect(&mut live, Duration::from_secs(30), |message| matches!(message, ToIde::Ready { .. }).then_some(()));
    while awake.try_recv().is_ok() {}
    live.send(&ToHost::Ping { nonce: 111 }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        awake.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("a reply after restart wakes the owner without polling");
        let mut received = Vec::new();
        assert!(live.poll(&mut received));
        if received.iter().any(|event| matches!(event, Event::Host(ToIde::Pong { nonce: 111 }))) { break; }
    }
    live.set_waker(None);
    live.stop();

    // a picture: `bunny preview --png` starts the host again on the warm
    // target, asks for the preview's pixels and stops
    let pictures = here.join("pictures");
    let began = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(["preview", "--png"])
        .arg(&pictures)
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    timing("png", began.elapsed(), None);
    let png = fs::read(pictures.join("home_preview.png")).expect("the picture was written");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let app_png = fs::read(pictures.join("app.png")).expect("no idents includes the whole app");
    assert_eq!(
        &app_png[16..24],
        &[0, 0, 1, 164, 0, 0, 2, 128],
        "420x640 at scale 1"
    );

    // a page: on the Mac the host's shell pictures a webview through an
    // offscreen desk, and the picture paints the page where the
    // placeholder was — red, as the document says
    if cfg!(target_os = "macos") {
        let source = fs::read_to_string(&lib).unwrap();
        fs::write(
            &lib,
            format!(
                "{source}\n/// A page from memory, in a box.\n#[bunny_ui::preview]\nfn page_preview() -> impl View {{\n    vstack!(webview_html(\"<body style=\\\"margin:0;background:#ff0000\\\"></body>\", \"\", NetworkPolicy::Deny).frame(200.0, 100.0))\n}}\n"
            ),
        )
        .unwrap();
        let began = Instant::now();
        let out = Command::new(env!("CARGO_BIN_EXE_bunny"))
            .args(["preview", "--png"])
            .arg(&pictures)
            .args(["--size", "200x100", "page_preview"])
            .current_dir(&app)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        timing("page_png", began.elapsed(), None);
        let png = fs::read(pictures.join("page_preview.png")).expect("the page's picture was written");
        let (width, height, rgba) = bunny_cli::formats::png::decode(&png).expect("a png the house wrote");
        assert_eq!((width, height), (200, 100));
        let at = ((50 * 200) + 100) * 4;
        assert_eq!(&rgba[at..at + 4], &[255, 0, 0, 255], "the page's red where the box stood");
    }

    // goldens: the first `--snapshot` writes one, the second finds it the
    // same, and a padding moved fails by name with the box
    let goldens = here.join("goldens");
    let snapshot = |label: &str| {
        let began = Instant::now();
        let out = Command::new(env!("CARGO_BIN_EXE_bunny"))
            .args(["preview", "--snapshot"])
            .arg(&goldens)
            .args(["--engine", "house", "--size", "320x200", "home_preview"])
            .current_dir(&app)
            .output()
            .unwrap();
        timing(label, began.elapsed(), None);
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let (ok, said) = snapshot("snapshot_new");
    assert!(ok && said.contains("new, kept as the golden"), "{said}");
    let (ok, said) = snapshot("snapshot_same");
    assert!(ok && said.contains("same"), "{said}");
    let source = fs::read_to_string(&lib).unwrap();
    assert!(source.contains(".padding()"), "{source}");
    fs::write(&lib, source.replace(".padding()", ".padding_length(40.0)")).unwrap();
    let (ok, said) = snapshot("snapshot_differs");
    assert!(!ok, "{said}");
    assert!(said.contains("home_preview.png") && said.contains("pixel(s) differ in a") && said.contains("box at ("), "{said}");
    let _ = fs::remove_dir_all(&here);
}
