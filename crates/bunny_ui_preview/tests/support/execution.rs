use super::*;
use bunny_ui::preview::{ExecutionContext, ExecutionMode, ExecutionSubscription};

thread_local! {
    static FAIL_FACTORY: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Vec<(&'static str, ExecutionContext)>> = const { RefCell::new(Vec::new()) };
}

fn record(event: &'static str) {
    let context = ExecutionContext::current().expect("a host callback must have a scene context");
    EVENTS.with(|events| events.borrow_mut().push((event, context)));
}

#[derive(Clone)]
struct ExecutionCard {
    label: State<String>,
    subscription: Rc<ExecutionSubscription>,
}
impl Component for ExecutionCard {
    fn body(self) -> impl View {
        button(text(self.label.get()), move || {
            let _keep = &self.subscription;
            record("input");
        })
        .on_mount(|| record("mount"))
    }
}

fn make_execution() -> Output {
    record("factory");
    assert!(
        !FAIL_FACTORY.with(|fail| fail.replace(false)),
        "factory failed"
    );
    let context = ExecutionContext::current().unwrap();
    let label = State::new(format!("{:?}", context.mode()));
    let status = label;
    let subscription = context.subscribe(move |mode| {
        record("change");
        status.set(format!("{mode:?}"));
    });
    Output::of(ExecutionCard {
        label,
        subscription: Rc::new(subscription),
    })
}

static EXECUTION: PreviewEntry = PreviewEntry::new(
    Meta {
        ident: "execution",
        name: "Execution",
        scope: "Execution",
        example: "execution",
        module: "host",
        file: "src/lib.rs",
        line: 1,
    },
    make_execution,
);

#[unsafe(export_name = "bunny_preview__execution")]
pub fn execution_entry() -> &'static PreviewEntry {
    &EXECUTION
}

fn selection(id: &str, scheme: wire::Scheme) -> wire::InstanceSelection {
    wire::InstanceSelection {
        instance: wire::SceneInstanceId::parse(id).unwrap(),
        ident: "execution".into(),
        traits: Traits {
            scheme,
            ..fixed(200.0, 120.0)
        },
    }
}

fn mode(id: &str, variant: u32, mode: wire::ExecutionMode) -> ToHost {
    ToHost::SetExecution {
        instance: wire::SceneInstanceId::parse(id).unwrap(),
        variant,
        mode,
    }
}

#[test]
fn modes_input_and_mount_are_scoped_per_instance_and_variant() {
    let (mut host, seen) = host();
    let a = selection("execution-a", wire::Scheme::Both);
    let b = selection("execution-b", wire::Scheme::Light);
    host.handle(ToHost::SelectInstances(vec![a.clone(), b.clone()]));
    assert!(host.tick());
    let initial = seen
        .borrow_mut()
        .drain(..)
        .filter_map(|message| match message {
            ToIde::InstanceFrame { instance, frame } => Some((instance, frame)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(initial.len(), 3);
    assert!(initial.iter().all(|(_, frame)| says(frame, "Some(Design)")));
    let captured = EVENTS.with(|events| events.borrow_mut().drain(..).collect::<Vec<_>>());
    assert_eq!(
        captured.iter().filter(|(kind, _)| *kind == "mount").count(),
        3
    );
    assert!(
        captured
            .iter()
            .all(|(_, context)| context.mode() == Some(ExecutionMode::Design))
    );
    assert!(ExecutionContext::current().is_none());
    host.handle(mode("execution-a", 1, wire::ExecutionMode::Play));
    assert!(host.tick());
    let changed = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    assert!(
        matches!(&changed[..], [ToIde::InstanceFrame { instance, frame }]
        if instance == &a.instance && frame.variant == 1 && says(frame, "Some(Play)"))
    );
    host.handle(mode("execution-a", 1, wire::ExecutionMode::Play));
    assert!(
        !host.needs_turn(),
        "an idempotent assignment must not dirty a scene"
    );
    assert!(!host.tick());
    let button = initial[1]
        .1
        .sites
        .iter()
        .find(|site| initial[1].1.strings[site.kind as usize] == "button")
        .unwrap();
    for kind in [wire::PointerKind::Down, wire::PointerKind::Up] {
        host.handle(ToHost::Instance {
            instance: a.instance.clone(),
            variant: 1,
            command: wire::InstanceCommand::Pointer {
                kind,
                x: button.frame.origin.x + 2.0,
                y: button.frame.origin.y + 2.0,
                modifiers: 0,
            },
        });
    }
    EVENTS.with(|events| {
        assert!(
            events
                .borrow()
                .iter()
                .any(|(kind, context)| *kind == "input"
                    && context.mode() == Some(ExecutionMode::Play))
        )
    });
    host.handle(ToHost::SelectInstances(vec![b, a]));
    assert_eq!(
        captured
            .iter()
            .filter(|(_, context)| context.mode() == Some(ExecutionMode::Play))
            .count(),
        2
    );
    host.handle(ToHost::SelectInstances(Vec::new()));
    assert!(captured.iter().all(|(_, context)| context.mode().is_none()));
    host.handle(ToHost::SelectInstances(vec![selection(
        "execution-a",
        wire::Scheme::Light,
    )]));
    host.tick();
    assert!(seen.borrow().iter().any(|message| matches!(message, ToIde::InstanceFrame { frame, .. } if says(frame, "Some(Design)"))));
    assert!(ExecutionContext::current().is_none());
}

#[test]
fn legacy_callbacks_use_design_and_missing_instance_mode_reports_an_error() {
    let (mut host, seen) = host();
    host.handle(select("execution", fixed(200.0, 120.0)));
    assert!(host.tick());
    let first = frames(&seen);
    assert_eq!(first.len(), 1);
    assert!(says(&first[0], "Some(Design)"));
    host.handle(mode("absent", 4, wire::ExecutionMode::Play));
    assert!(
        matches!(&seen.borrow()[..], [ToIde::InstanceError { variant: Some(4), message, .. }] if message.contains("not selected"))
    );
    let button = first[0]
        .sites
        .iter()
        .find(|site| first[0].strings[site.kind as usize] == "button")
        .unwrap();
    for kind in [wire::PointerKind::Down, wire::PointerKind::Up] {
        host.handle(ToHost::Pointer {
            id: "src/lib.rs#execution".into(),
            kind,
            x: button.frame.origin.x + 2.0,
            y: button.frame.origin.y + 2.0,
            modifiers: 0,
        });
    }
    EVENTS.with(|events| {
        assert!(events.borrow().iter().any(
            |(kind, context)| *kind == "input" && context.mode() == Some(ExecutionMode::Design)
        ))
    });
}

#[test]
fn discovery_uses_a_temporary_design_context_and_closes_it() {
    let dir =
        std::env::temp_dir().join(format!("bunny-execution-discovery-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "#[preview] fn execution() -> impl View { text(\"fixture\") }",
    )
    .unwrap();
    let mut host = Host::new(
        Engines::house(),
        Config {
            package: "discovery".into(),
            manifest_dir: dir,
            framework: "test",
        },
        Box::new(|_| {}),
    );
    host.hello();
    EVENTS.with(|events| {
        let events = events.borrow();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "factory");
        assert!(events[0].1.mode().is_none());
    });
    assert!(ExecutionContext::current().is_none());
}

#[test]
fn failed_scene_closes_its_captured_context_without_poisoning_other_scenes() {
    let (mut host, seen) = host();
    host.handle(ToHost::SelectInstances(vec![
        selection("failed", wire::Scheme::Light),
        wire::InstanceSelection {
            instance: wire::SceneInstanceId::parse("survivor").unwrap(),
            ident: "card".into(),
            traits: Traits::default(),
        },
    ]));
    FAIL_FACTORY.with(|fail| fail.set(true));
    assert!(host.tick());
    assert!(seen.borrow().iter().any(|event| matches!(event, ToIde::InstancePanicked { instance, .. } if instance.as_str() == "failed")));
    assert!(seen.borrow().iter().any(|event| matches!(event, ToIde::InstanceFrame { instance, .. } if instance.as_str() == "survivor")));
    EVENTS.with(|events| {
        let events = events.borrow();
        assert_eq!(events.len(), 1);
        assert!(events[0].1.mode().is_none());
    });
    assert!(ExecutionContext::current().is_none());
}
