use super::*;
use wire::{StateRead, StateReply, StateRequestId, StateTarget};
fn request(n: u64) -> StateRequestId {
    StateRequestId::parse(n).unwrap()
}
fn select(host: &mut Host, names: &[&str]) {
    host.handle(ToHost::ObserveViews { enabled: true });
    host.handle(ToHost::SelectInstances(
        names
            .iter()
            .map(|name| wire::InstanceSelection {
                instance: wire::SceneInstanceId::parse(name).unwrap(),
                ident: "tapper".into(),
                traits: fixed(200.0, 120.0),
            })
            .collect(),
    ));
}
fn target(seen: &Rc<RefCell<Vec<ToIde>>>, name: &str) -> (StateTarget, wire::Frame) {
    let messages = seen.borrow();
    let frame = messages
        .iter()
        .rev()
        .find_map(|m| match m {
            ToIde::InstanceFrame { instance, frame } if instance.as_str() == name => {
                Some(frame.as_ref())
            }
            _ => None,
        })
        .unwrap();
    let views = messages
        .iter()
        .find_map(|m| match m {
            ToIde::ViewSnapshot(views) if views.seq == frame.seq => Some(views),
            _ => None,
        })
        .unwrap();
    let site = frame
        .sites
        .iter()
        .find(|site| frame.strings[site.kind as usize] == "text")
        .unwrap();
    (
        StateTarget {
            instance: views.instance.clone(),
            preview: views.preview.clone(),
            variant: views.variant,
            generation: views.generation,
            mounted: views
                .nodes
                .iter()
                .find(|node| node.index == site.index)
                .unwrap()
                .mounted
                .unwrap(),
        },
        frame.clone(),
    )
}
fn snapshots(seen: &Rc<RefCell<Vec<ToIde>>>) -> Vec<wire::StateSnapshot> {
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    messages
        .iter()
        .enumerate()
        .filter_map(|(at, message)| {
            let ToIde::StateObservation(StateReply::Snapshot(snapshot)) = message else {
                return None;
            };
            let ToIde::ViewSnapshot(views) = &messages[at + 1] else {
                panic!("missing view metadata")
            };
            let (frame, instance) = match &messages[at + 2] {
                ToIde::InstanceFrame { instance, frame } => (frame.as_ref(), Some(instance)),
                ToIde::Frame(frame) => (frame, None),
                _ => panic!("missing frame"),
            };
            let proof = views.match_frame(frame, instance).unwrap();
            snapshot
                .match_views(&proof, snapshot.request, &snapshot.target)
                .unwrap();
            Some(snapshot.clone())
        })
        .collect()
}
fn value(snapshot: &wire::StateSnapshot) -> (wire::DependencyKey, i128) {
    let StateRead::Values(deps) = &snapshot.read else {
        panic!("no values: {snapshot:?}")
    };
    assert_eq!(deps.len(), 1);
    let wire::StateEvidence::State {
        value: wire::StateValue::Signed(value),
        constructor,
        ..
    } = &deps[0].evidence
    else {
        panic!("not signed state")
    };
    assert!(constructor.file.ends_with("host.rs"));
    (deps[0].key, *value)
}
fn click(host: &mut Host, target: &StateTarget, frame: &wire::Frame) {
    let button = frame
        .sites
        .iter()
        .find(|s| frame.strings[s.kind as usize] == "button")
        .unwrap();
    for kind in [wire::PointerKind::Down, wire::PointerKind::Up] {
        host.handle(ToHost::Instance {
            instance: target.instance.clone().unwrap(),
            variant: 0,
            command: wire::InstanceCommand::Pointer {
                kind,
                x: button.frame.origin.x + button.frame.size.width / 2.0,
                y: button.frame.origin.y + button.frame.size.height / 2.0,
                modifiers: 0,
            },
        });
    }
}
#[test]
fn selected_observation_is_opt_in_and_independent_between_instances() {
    let (mut host, seen) = host();
    host.hello();
    assert!(seen.borrow().iter().any(
        |m| matches!(m, ToIde::Hello { caps, .. } if caps & wire::caps::STATE_OBSERVATIONS != 0)
    ));
    select(&mut host, &["a", "b"]);
    host.tick();
    let (a, frame) = target(&seen, "a");
    let (b, _) = target(&seen, "b");
    assert!(snapshots(&seen).is_empty());
    host.handle(ToHost::ObserveState {
        request: request(1),
        target: Some(a.clone()),
    });
    host.tick();
    let first = snapshots(&seen);
    assert_eq!(first.len(), 1);
    assert_eq!(value(&first[0]).1, 0);
    click(&mut host, &a, &frame);
    host.tick();
    let next = snapshots(&seen);
    assert_eq!(next.len(), 1);
    assert_eq!(value(&next[0]), (value(&first[0]).0, 1));
    host.handle(ToHost::ObserveState {
        request: request(2),
        target: Some(b),
    });
    host.tick();
    let other = snapshots(&seen);
    assert_eq!(other.len(), 1);
    assert_eq!(value(&other[0]).1, 0);
    assert_ne!(value(&other[0]).0, value(&first[0]).0);
    assert!(!host.tick());
    assert!(snapshots(&seen).is_empty());
}
#[test]
fn cancellation_stale_requests_disable_and_remount_revoke_without_retargeting() {
    let (mut host, seen) = host();
    select(&mut host, &["a"]);
    host.tick();
    let (a, frame) = target(&seen, "a");
    seen.borrow_mut().clear();
    host.handle(ToHost::ObserveState {
        request: request(2),
        target: Some(a.clone()),
    });
    host.tick();
    snapshots(&seen);
    host.handle(ToHost::ObserveState {
        request: request(1),
        target: None,
    });
    assert!(seen.borrow().iter().any(|m| matches!(m, ToIde::StateObservation(StateReply::Refused { request: id, reason: wire::StateRefusal::StaleRequest }) if *id == request(1))));
    click(&mut host, &a, &frame);
    host.tick();
    assert_eq!(value(&snapshots(&seen)[0]).1, 1);
    host.handle(ToHost::ObserveState {
        request: request(3),
        target: None,
    });
    click(&mut host, &a, &frame);
    host.tick();
    assert!(snapshots(&seen).is_empty());
    host.handle(ToHost::ObserveState {
        request: request(4),
        target: Some(a.clone()),
    });
    host.tick();
    snapshots(&seen);
    host.handle(ToHost::ObserveViews { enabled: false });
    assert!(seen.borrow().iter().any(|m| matches!(
        m,
        ToIde::StateObservation(StateReply::Refused {
            reason: wire::StateRefusal::Disabled,
            ..
        })
    )));
    seen.borrow_mut().clear();
    select(&mut host, &[]);
    select(&mut host, &["a"]);
    host.tick();
    host.handle(ToHost::ObserveState {
        request: request(5),
        target: Some(a),
    });
    assert!(seen.borrow().iter().any(|m| matches!(
        m,
        ToIde::StateObservation(StateReply::Refused {
            reason: wire::StateRefusal::ViewMissing,
            ..
        })
    )));
    assert!(snapshots(&seen).is_empty());
}

#[test]
fn same_formatted_pixels_still_publish_changed_storage_and_write_version() {
    let (mut host, seen) = host();
    host.handle(ToHost::ObserveViews { enabled: true });
    host.handle(ToHost::SelectInstances(vec![wire::InstanceSelection {
        instance: wire::SceneInstanceId::parse("zoom").unwrap(),
        ident: "zoomer".into(),
        traits: fixed(200.0, 200.0),
    }]));
    host.tick();
    let (selected, frame) = target(&seen, "zoom");
    seen.borrow_mut().clear();
    host.handle(ToHost::ObserveState {
        request: request(1),
        target: Some(selected.clone()),
    });
    host.tick();
    let first = snapshots(&seen);
    // The 100-point gesture element starts below the label, as in the input probe.
    host.handle(ToHost::Instance {
        instance: selected.instance.clone().unwrap(),
        variant: 0,
        command: wire::InstanceCommand::Magnify {
            x: 50.0,
            y: 70.0,
            scale: 1.01,
        },
    });
    assert!(host.needs_turn());
    assert!(host.tick());
    let (_, changed_frame) = target(&seen, "zoom");
    let next = snapshots(&seen);
    assert_eq!(next.len(), 1);
    assert_eq!(frame.commands, changed_frame.commands);
    assert_eq!(frame.strings, changed_frame.strings);
    let (StateRead::Values(before), StateRead::Values(after)) = (&first[0].read, &next[0].read)
    else {
        panic!("no values")
    };
    let wire::StateEvidence::State {
        version: old_version,
        ..
    } = before[0].evidence
    else {
        panic!("no state")
    };
    let wire::StateEvidence::State {
        version,
        value: wire::StateValue::FloatBits(bits),
        ..
    } = after[0].evidence
    else {
        panic!("not float state")
    };
    assert!(version > old_version);
    assert_eq!(bits, 1.01_f64.to_bits());
    assert!(!host.needs_turn());
}
#[test]
fn invalid_replacement_clears_previous_watch_and_removal_reports_revocation() {
    let (mut host, seen) = host();
    select(&mut host, &["a"]);
    host.tick();
    let (a, frame) = target(&seen, "a");
    seen.borrow_mut().clear();
    host.handle(ToHost::ObserveState {
        request: request(1),
        target: Some(a.clone()),
    });
    host.tick();
    snapshots(&seen);
    let mut old_build = a.clone();
    old_build.generation += 1;
    host.handle(ToHost::ObserveState {
        request: request(2),
        target: Some(old_build),
    });
    assert!(seen.borrow().iter().any(|m| matches!(
        m,
        ToIde::StateObservation(StateReply::Refused {
            reason: wire::StateRefusal::Generation,
            ..
        })
    )));
    click(&mut host, &a, &frame);
    host.tick();
    assert!(snapshots(&seen).is_empty());
    host.handle(ToHost::ObserveState {
        request: request(3),
        target: Some(a),
    });
    host.tick();
    snapshots(&seen);
    select(&mut host, &[]);
    assert!(seen.borrow().iter().any(|m| matches!(
        m,
        ToIde::StateObservation(StateReply::Refused {
            reason: wire::StateRefusal::SceneMissing,
            ..
        })
    )));
}
#[test]
fn ambiguous_legacy_scenes_never_pick_the_first_matching_preview() {
    let (mut host, seen) = host();
    host.handle(ToHost::ObserveViews { enabled: true });
    host.handle(ToHost::Select(
        ["one", "two"]
            .into_iter()
            .map(|id| Selection {
                id: id.into(),
                ident: "tapper".into(),
                traits: fixed(200.0, 120.0),
            })
            .collect(),
    ));
    host.tick();
    let selected = seen
        .borrow()
        .iter()
        .find_map(|m| {
            if let ToIde::ViewSnapshot(views) = m {
                Some(StateTarget {
                    instance: None,
                    preview: views.preview.clone(),
                    variant: views.variant,
                    generation: views.generation,
                    mounted: views.nodes[0].mounted.unwrap(),
                })
            } else {
                None
            }
        })
        .unwrap();
    seen.borrow_mut().clear();
    host.handle(ToHost::ObserveState {
        request: request(1),
        target: Some(selected),
    });
    assert!(matches!(
        &seen.borrow()[0],
        ToIde::StateObservation(StateReply::Refused {
            reason: wire::StateRefusal::AmbiguousScene,
            ..
        })
    ));
}
