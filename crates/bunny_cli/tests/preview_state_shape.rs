//! A structural edit must not give a new primitive State an old sibling's slot.
//! Run with `cargo test -p bunny-cli --test preview_state_shape -- --ignored`.

use bunny_cli::preview::{Event, Live, Options, Preview, Step};
use bunny_ui_preview::wire::{
    Frame, InstanceCommand, InstanceSelection, PointerKind, SceneInstanceId, ToHost, ToIde,
};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

const BEFORE: &str = r#"use bunny_ui::prelude::*;
#[derive(Clone, Copy)]
struct Post { likes: State<i32> }
impl Component for Post {
    fn body(self) -> impl View {
        vstack!(text!("Likes: {}", self.likes), button(text("Like"), move || self.likes.add(1)))
    }
}
fn first() -> Post { Post { likes: State::new(7) } }
fn second() -> Post { Post { likes: State::new(11) } }
fn root() -> impl View { vstack!(first(), second()).padding() }
bunny_ui::app!(root);
"#;

fn expect<T>(live: &mut Preview<Live>, mut pick: impl FnMut(&ToIde) -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(30);
    let mut events = Vec::new();
    loop {
        assert!(
            live.wait(Duration::from_millis(20), &mut events),
            "host ended"
        );
        for event in events.drain(..) {
            match event {
                Event::Host(message) => {
                    if let Some(found) = pick(&message) {
                        return found;
                    }
                }
                Event::Crashed { code } => panic!("host crashed: {code:?}"),
                Event::BuildFailed { diagnostics } => panic!("build failed: {diagnostics:?}"),
                _ => {}
            }
        }
        assert!(Instant::now() < until, "host reply timed out");
    }
}

fn event_summary(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| !matches!(event, Event::Sources { .. }))
        .collect()
}

fn start(step: Step, events: &[Event]) -> Preview<Live> {
    match step {
        Step::Live(live) => live,
        Step::Failed(failed) => panic!("{}: {:?}", failed.why(), event_summary(events)),
    }
}

fn select(live: &mut Preview<Live>) -> BTreeMap<SceneInstanceId, Frame> {
    let app = expect(live, |message| match message {
        ToIde::Ready { previews, .. } => previews.iter().find(|p| p.ident == "app").cloned(),
        _ => None,
    });
    live.send(&ToHost::SelectInstances(
        ["first", "second"]
            .into_iter()
            .map(|key| InstanceSelection {
                instance: SceneInstanceId::parse(key).unwrap(),
                ident: app.ident.clone(),
                traits: app.traits.clone(),
            })
            .collect(),
    ))
    .unwrap();
    frames(live, 1)
}

fn frames(live: &mut Preview<Live>, generation: u32) -> BTreeMap<SceneInstanceId, Frame> {
    let mut result = BTreeMap::new();
    expect(live, |message| {
        if let ToIde::InstanceFrame { instance, frame } = message
            && frame.generation >= generation
        {
            result.insert(instance.clone(), frame.as_ref().clone());
        }
        (result.len() == 2).then_some(())
    });
    result
}

fn says(frame: &Frame, text: &str) -> bool {
    frame.strings.iter().any(|s| s == text)
}

fn click(live: &mut Preview<Live>, instance: &SceneInstanceId, frame: &Frame) {
    let button = frame
        .sites
        .iter()
        .find(|site| frame.strings[site.kind as usize] == "button")
        .unwrap();
    let x = button.frame.origin.x + button.frame.size.width / 2.0;
    let y = button.frame.origin.y + button.frame.size.height / 2.0;
    for kind in [PointerKind::Down, PointerKind::Up] {
        live.send(&ToHost::Instance {
            instance: instance.clone(),
            variant: 0,
            command: InstanceCommand::Pointer {
                kind,
                x,
                y,
                modifiers: 0,
            },
        })
        .unwrap();
    }
}

fn edit(live: &mut Preview<Live>, lib: &Path, source: &str) -> Vec<Event> {
    fs::write(lib, source).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while live.saved().is_none() {
        assert!(Instant::now() < until, "edit was not observed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut events = Vec::new();
    live.reload(&mut events);
    events
}

#[test]
#[ignore = "builds and reloads a real Bunny preview host"]
fn adding_a_same_type_field_restarts_without_reassigning_live_state() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap();
    let here = repo.join(format!("target/state-shape-fixture-{}", std::process::id()));
    fs::create_dir_all(&here).unwrap();
    let created = Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(["new", "state_shape", "--org", "io.bunny"])
        .arg("--bunny-ui-path")
        .arg(repo.join("crates/bunny_ui_facade"))
        .current_dir(&here)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let app = here.join("state_shape");
    let manifest = app.join("Cargo.toml");
    let package = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, format!("{package}\n[workspace]\n")).unwrap();
    fs::create_dir_all(app.join(".cargo")).unwrap();
    fs::write(
        app.join(".cargo/config.toml"),
        format!(
            "[build]\ntarget-dir = {:?}\n",
            repo.join("target/bunny-preview-e2e").to_str().unwrap()
        ),
    )
    .unwrap();
    let lib = app.join("src/lib.rs");
    fs::write(&lib, BEFORE).unwrap();
    let mut events = Vec::new();
    let mut live = start(
        Preview::new(&app, Options::default())
            .unwrap()
            .start(&mut events),
        &events,
    );
    let first = SceneInstanceId::parse("first").unwrap();
    let second = SceneInstanceId::parse("second").unwrap();
    let initial = select(&mut live);
    for frame in initial.values() {
        assert!(
            says(frame, "Likes: 7") && says(frame, "Likes: 11"),
            "{:?}",
            frame.strings
        );
    }
    click(&mut live, &first, &initial[&first]);
    expect(&mut live, |message| {
        matches!(message, ToIde::InstanceFrame { instance, frame } if instance == &first && says(frame, "Likes: 8")).then_some(())
    });

    // A body/initializer edit still preserves each mounted value.
    let body_edit = BEFORE.replace("State::new(7)", "State::new(70)");
    let events = edit(&mut live, &lib, &body_edit);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Reloaded {
                new_types: false,
                ..
            }
        )),
        "{:?}",
        event_summary(&events)
    );
    let kept = frames(&mut live, 2);
    assert!(says(&kept[&first], "Likes: 8") && says(&kept[&first], "Likes: 11"));
    assert!(says(&kept[&second], "Likes: 7") && says(&kept[&second], "Likes: 11"));

    // A failed structural build must not install code or stop the old host.
    let invalid = body_edit.replace(
        "struct Post { likes: State<i32> }",
        "struct Post { likes: State<i32>, score: MissingType }",
    );
    let events = edit(&mut live, &lib, &invalid);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BuildFailed { .. })),
        "{:?}",
        event_summary(&events)
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::RestartNeeded(_) | Event::Reloaded { .. })),
        "{:?}",
        event_summary(&events)
    );
    live.send(&ToHost::Ping { nonce: 135 }).unwrap();
    expect(&mut live, |m| {
        matches!(m, ToIde::Pong { nonce: 135 }).then_some(())
    });

    let structural = body_edit
        .replace(
            "struct Post { likes: State<i32> }",
            "struct Post { likes: State<i32>, score: State<i32> }",
        )
        .replace(
            "text!(\"Likes: {}\", self.likes)",
            "text!(\"Likes: {} Score: {}\", self.likes, self.score)",
        )
        .replace("State::new(70) }", "State::new(70), score: State::new(5) }")
        .replace("State::new(11) }", "State::new(11), score: State::new(5) }");
    let events = edit(&mut live, &lib, &structural);
    let restart = events.iter().any(
        |e| matches!(e, Event::RestartNeeded(why) if why.contains("type") && why.contains("state")),
    );
    if !restart {
        let wrong = frames(&mut live, 3);
        for (instance, frame) in wrong {
            eprintln!("ambiguous live state in {instance:?}: {:?}", frame.strings);
        }
    }
    assert!(
        restart,
        "structural state change must request restart before installing a generation: {:?}",
        event_summary(&events)
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::Reloaded { .. })),
        "{:?}",
        event_summary(&events)
    );
    let mut events = Vec::new();
    let mut live = start(live.restart(&mut events), &events);
    let fresh = select(&mut live);
    for frame in fresh.values() {
        assert!(
            says(frame, "Likes: 70 Score: 5") && says(frame, "Likes: 11 Score: 5"),
            "{:?}",
            frame.strings
        );
    }
    click(&mut live, &first, &fresh[&first]);
    expect(&mut live, |message| {
        matches!(message, ToIde::InstanceFrame { instance, frame } if instance == &first && says(frame, "Likes: 71 Score: 5")).then_some(())
    });
    live.send(&ToHost::Instance {
        instance: second.clone(),
        variant: 0,
        command: InstanceCommand::Resize(bunny_ui_preview::wire::Size {
            width: 421.0,
            height: 640.0,
        }),
    })
    .unwrap();
    expect(&mut live, |message| {
        matches!(message, ToIde::InstanceFrame { instance, frame } if instance == &second && says(frame, "Likes: 70 Score: 5") && says(frame, "Likes: 11 Score: 5")).then_some(())
    });
    live.stop();
    fs::remove_dir_all(here).unwrap();
}
