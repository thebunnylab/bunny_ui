use super::*;

fn pairs(seen: &Rc<RefCell<Vec<ToIde>>>) -> Vec<(wire::ViewSnapshot, wire::Frame)> {
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for (at, message) in messages.iter().enumerate() {
        if let ToIde::ViewSnapshot(snapshot) = message {
            let (instance, frame) = match &messages[at + 1] {
                ToIde::Frame(frame) => (None, frame),
                ToIde::InstanceFrame { instance, frame } => (Some(instance), frame.as_ref()),
                other => panic!("snapshot must immediately precede its frame: {other:?}"),
            };
            snapshot
                .match_frame(frame, instance)
                .expect("complete unique identities");
            pairs.push((snapshot.clone(), frame.clone()));
        }
    }
    pairs
}

#[test]
fn view_observation_is_opt_in_and_forces_a_current_frame_pair() {
    let (mut host, seen) = host();
    host.hello();
    assert!(seen.borrow().iter().any(|message| matches!(message, ToIde::Hello { caps, .. } if caps & wire::caps::VIEW_IDENTITIES != 0)));
    host.handle(ToHost::Select(vec![Selection {
        id: "card".into(),
        ident: "card".into(),
        traits: Traits::default(),
    }]));
    host.tick();
    assert!(
        pairs(&seen).is_empty(),
        "old peers receive no extra messages"
    );
    host.handle(ToHost::ObserveViews { enabled: true });
    host.tick();
    let initial = pairs(&seen);
    assert_eq!(initial.len(), 1);
    assert!(!initial[0].0.nodes.is_empty());
    host.handle(ToHost::Resize {
        id: "card".into(),
        size: wire::Size {
            width: 500.0,
            height: 500.0,
        },
    });
    host.tick();
    let resized = pairs(&seen);
    assert_eq!(resized.len(), 1);
    assert_eq!(initial[0].0.nodes, resized[0].0.nodes);
    assert!(resized[0].0.seq > initial[0].0.seq);
    host.handle(ToHost::ObserveViews { enabled: false });
    host.tick();
    assert!(pairs(&seen).is_empty());
}

#[test]
fn instance_remount_retires_ids_even_when_the_picture_is_identical() {
    let (mut host, seen) = host();
    let instance = wire::SceneInstanceId::parse("one").unwrap();
    let selection = wire::InstanceSelection {
        instance: instance.clone(),
        ident: "card".into(),
        traits: Traits::default(),
    };
    host.handle(ToHost::ObserveViews { enabled: true });
    host.handle(ToHost::SelectInstances(vec![selection.clone()]));
    host.tick();
    let first = pairs(&seen);
    assert_eq!(first.len(), 1);
    host.handle(ToHost::SelectInstances(Vec::new()));
    host.handle(ToHost::SelectInstances(vec![selection]));
    host.tick();
    let second = pairs(&seen);
    assert_eq!(second.len(), 1);
    let old: std::collections::HashSet<_> = first[0]
        .0
        .nodes
        .iter()
        .map(|n| n.mounted.unwrap())
        .collect();
    assert!(
        second[0]
            .0
            .nodes
            .iter()
            .all(|n| !old.contains(&n.mounted.unwrap()))
    );
    assert_eq!(first[0].1.commands, second[0].1.commands);
    assert!(
        second[0]
            .0
            .match_frame(&first[0].1, Some(&instance))
            .is_err(),
        "a previous publication must not join a new mount"
    );
}
