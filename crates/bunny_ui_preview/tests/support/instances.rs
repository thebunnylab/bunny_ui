use super::*;
use wire::{InstanceCommand, InstanceSelection, SceneInstanceId};

fn instance(value: &str) -> SceneInstanceId {
    SceneInstanceId::parse(value).unwrap()
}

fn selection(value: &str, ident: &str, traits: Traits) -> InstanceSelection {
    InstanceSelection {
        instance: instance(value),
        ident: ident.to_owned(),
        traits,
    }
}

fn command(value: &str, variant: u32, command: InstanceCommand) -> ToHost {
    ToHost::Instance {
        instance: instance(value),
        variant,
        command,
    }
}

fn instance_frames(seen: &Rc<RefCell<Vec<ToIde>>>) -> Vec<(SceneInstanceId, wire::Frame)> {
    seen.borrow_mut()
        .drain(..)
        .filter_map(|message| match message {
            ToIde::InstanceFrame { instance, frame } => Some((instance, *frame)),
            ToIde::Frame(_) => panic!("an instance frame lost its identity"),
            _ => None,
        })
        .collect()
}

#[test]
fn duplicate_components_keep_input_size_and_state_by_instance_when_reordered() {
    let (mut host, seen) = host();
    let a = selection("artboard:1", "tapper", fixed(200.0, 120.0));
    let b = selection("artboard:2", "tapper", fixed(300.0, 160.0));
    host.handle(ToHost::SelectInstances(vec![a.clone(), b.clone()]));
    assert!(host.tick());
    let first = instance_frames(&seen);
    assert_eq!(first.len(), 2);
    assert!(first.iter().all(|(_, frame)| says(frame, "taps 0")));
    let button = first[0]
        .1
        .sites
        .iter()
        .find(|site| first[0].1.strings[site.kind as usize] == "button")
        .unwrap();
    let (x, y) = (
        button.frame.origin.x + button.frame.size.width / 2.0,
        button.frame.origin.y + button.frame.size.height / 2.0,
    );
    for kind in [wire::PointerKind::Down, wire::PointerKind::Up] {
        host.handle(command(
            "artboard:1",
            0,
            InstanceCommand::Pointer {
                kind,
                x,
                y,
                modifiers: 0,
            },
        ));
    }
    assert!(host.tick());
    let changed = instance_frames(&seen);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0, a.instance);
    assert!(says(&changed[0].1, "taps 1"));
    host.handle(command(
        "artboard:2",
        0,
        InstanceCommand::Resize(wire::Size {
            width: 320.0,
            height: 180.0,
        }),
    ));
    host.handle(ToHost::SelectInstances(vec![b.clone(), a.clone()]));
    assert!(host.tick());
    let changed = instance_frames(&seen);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0, b.instance);
    assert!(says(&changed[0].1, "taps 0"));
    assert_eq!(
        changed[0].1.size,
        wire::Size {
            width: 320.0,
            height: 180.0
        }
    );
    host.handle(command("artboard:1", 0, InstanceCommand::RequestSnapshot));
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    assert!(
        matches!(&messages[..], [ToIde::InstanceSnapshot { instance: id, preview, variant: 0, size: (200,120), rgba }] if id == &a.instance && preview == "tapper" && rgba.len() == 200*120*4)
    );
    assert!(
        !host.needs_turn(),
        "idle instances do not invalidate each other's retained layouts"
    );
}

#[test]
fn overrides_and_variant_input_do_not_leak_to_another_instance() {
    let (mut host, seen) = host();
    host.handle(ToHost::SelectInstances(vec![
        selection(
            "a",
            "card",
            Traits {
                scheme: wire::Scheme::Both,
                ..Traits::default()
            },
        ),
        selection("b", "card", Traits::default()),
    ]));
    host.tick();
    let initial = instance_frames(&seen);
    assert_eq!(initial.len(), 3);
    let site = padding_site(&initial[0].1);
    let rest = text_x(&initial[0].1);
    host.handle(command(
        "a",
        1,
        InstanceCommand::Override {
            site,
            op: wire::OverrideOp::Replace,
            patch: wire::Patch::PaddingLength(48.0),
        },
    ));
    assert!(host.tick());
    let changed = instance_frames(&seen);
    assert_eq!(changed.len(), 1);
    assert_eq!((&changed[0].0, changed[0].1.variant), (&instance("a"), 1));
    assert!(text_x(&changed[0].1) > rest);
    host.handle(command(
        "b",
        0,
        InstanceCommand::Resize(wire::Size {
            width: 240.0,
            height: 140.0,
        }),
    ));
    host.tick();
    let changed = instance_frames(&seen);
    assert_eq!(
        text_x(&changed[0].1),
        rest,
        "the second scene did not inherit the first's padding"
    );
    host.handle(command("a", 1, InstanceCommand::ClearOverrides));
    host.tick();
    let changed = instance_frames(&seen);
    assert_eq!(text_x(&changed[0].1), rest);
    assert!(!host.needs_turn());
}

#[test]
fn invalid_instance_selection_is_atomic_and_legacy_commands_cannot_alias_it() {
    let (mut host, seen) = host();
    let a = selection("same", "card", fixed(120.0, 60.0));
    host.handle(ToHost::SelectInstances(vec![a.clone()]));
    host.tick();
    instance_frames(&seen);
    host.handle(ToHost::SelectInstances(vec![a.clone(), a]));
    assert!(
        seen.borrow()
            .iter()
            .any(|event| matches!(event, ToIde::InstanceError { .. }))
    );
    host.handle(ToHost::Resize {
        id: "same".into(),
        size: wire::Size {
            width: 999.0,
            height: 999.0,
        },
    });
    assert!(
        !host.tick(),
        "legacy addressing cannot mutate an instance with the same spelling"
    );
    host.handle(command("same", 7, InstanceCommand::RequestSnapshot));
    assert!(seen.borrow().iter().any(|event| matches!(
        event,
        ToIde::InstanceError {
            variant: Some(7),
            ..
        }
    )));
    host.handle(command("same", 0, InstanceCommand::RequestSnapshot));
    assert!(seen.borrow().iter().any(|event| matches!(
        event,
        ToIde::InstanceSnapshot {
            size: (120, 60),
            ..
        }
    )));
}

#[test]
fn an_instance_panic_names_only_its_instance_and_other_scenes_survive() {
    let (mut host, seen) = host();
    host.handle(ToHost::SelectInstances(vec![
        selection("broken", "boom", Traits::default()),
        selection("good", "card", Traits::default()),
    ]));
    host.tick();
    assert!(seen.borrow().iter().any(|event| matches!(event, ToIde::InstancePanicked { instance: id, preview, variant: 0, .. } if id == &instance("broken") && preview == "boom")));
    assert!(seen.borrow().iter().any(|event| matches!(event, ToIde::InstanceFrame { instance: id, .. } if id == &instance("good"))));
}

#[test]
fn a_variant_has_its_own_input_and_recreated_instances_start_fresh() {
    let (mut host, seen) = host();
    let a = selection(
        "a",
        "tapper",
        Traits {
            scheme: wire::Scheme::Both,
            ..fixed(200.0, 120.0)
        },
    );
    host.handle(ToHost::SelectInstances(vec![a.clone()]));
    host.tick();
    let initial = instance_frames(&seen);
    let dark = &initial
        .iter()
        .find(|(_, frame)| frame.variant == 1)
        .unwrap()
        .1;
    let button = dark
        .sites
        .iter()
        .find(|site| dark.strings[site.kind as usize] == "button")
        .unwrap();
    let (x, y) = (
        button.frame.origin.x + button.frame.size.width / 2.0,
        button.frame.origin.y + button.frame.size.height / 2.0,
    );
    for kind in [wire::PointerKind::Down, wire::PointerKind::Up] {
        host.handle(command(
            "a",
            1,
            InstanceCommand::Pointer {
                kind,
                x,
                y,
                modifiers: 0,
            },
        ));
    }
    host.tick();
    let changed = instance_frames(&seen);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].1.variant, 1);
    assert!(says(&changed[0].1, "taps 1"));
    host.handle(ToHost::SelectInstances(Vec::new()));
    host.handle(ToHost::SelectInstances(vec![a]));
    host.tick();
    let new = instance_frames(&seen);
    assert_eq!(new.len(), 2);
    assert!(new.iter().all(|(_, frame)| says(frame, "taps 0")));
}

#[test]
fn a_held_snapshot_cannot_reattach_to_a_recreated_instance() {
    let (host, seen) = host();
    let door = Door::default();
    door.busy.set(true);
    let mut host = host.with_pictures(Box::new(door.clone()));
    let selected = selection("a", "page", fixed(200.0, 140.0));
    host.handle(ToHost::SelectInstances(vec![selected.clone()]));
    host.tick();
    instance_frames(&seen);
    host.handle(command("a", 0, InstanceCommand::RequestSnapshot));
    host.handle(ToHost::SelectInstances(Vec::new()));
    host.handle(ToHost::SelectInstances(vec![selected]));
    door.busy.set(false);
    host.tick();
    assert!(seen.borrow().iter().any(|event| matches!(event, ToIde::InstanceError { instance: id, variant: Some(0), .. } if id == &instance("a"))));
    assert!(
        !seen
            .borrow()
            .iter()
            .any(|event| matches!(event, ToIde::InstanceSnapshot { .. }))
    );
}
