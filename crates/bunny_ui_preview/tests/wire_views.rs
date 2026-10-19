use bunny_ui_preview::wire::{
    self, Message, Reader, Received, ToHost, ToIde, ViewIdentity, ViewSnapshot,
};

#[test]
fn view_snapshot_and_opt_in_round_trip_without_changing_old_payloads() {
    let snapshot = ViewSnapshot {
        instance: Some(wire::SceneInstanceId::parse("board:1").unwrap()),
        preview: "card".into(),
        variant: 2,
        generation: 3,
        seq: 4,
        nodes: vec![
            ViewIdentity {
                index: 0,
                mounted: wire::MountedViewId::parse(7).ok(),
            },
            ViewIdentity {
                index: 1,
                mounted: None,
            },
        ],
    };
    for message in [
        Message::ToIde(ToIde::ViewSnapshot(snapshot)),
        Message::ToHost(ToHost::ObserveViews { enabled: true }),
        Message::ToHost(ToHost::ObserveViews { enabled: false }),
    ] {
        let bytes = wire::encode(&message);
        let mut reader = Reader::new();
        for byte in bytes {
            reader.feed(&[byte]);
        }
        assert_eq!(reader.pull().unwrap(), Some(Received::Message(message)));
    }
    assert!(wire::MountedViewId::parse(0).is_err());
}

fn fixture() -> (ViewSnapshot, wire::Frame) {
    let frame = wire::Frame {
        preview: "card".into(),
        variant: 0,
        seq: 7,
        generation: 3,
        size: wire::Size {
            width: 100.0,
            height: 100.0,
        },
        scale: 1.0,
        background: wire::Color {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
        strings: Vec::new(),
        commands: Vec::new(),
        hosts: Vec::new(),
        metrics: wire::Metrics {
            build_ms: 0.0,
            layout_ms: 0.0,
            body_passes: 1,
            commands: 0,
        },
        sites: (0..2)
            .map(|index| wire::SiteBox {
                index,
                parent: None,
                kind: 0,
                modifier: None,
                file: None,
                name: None,
                line: 0,
                column: 0,
                path: None,
                frame: wire::Rect {
                    origin: wire::Point { x: 0.0, y: 0.0 },
                    size: wire::Size {
                        width: 100.0,
                        height: 100.0,
                    },
                },
            })
            .collect(),
    };
    let snapshot = ViewSnapshot {
        instance: None,
        preview: frame.preview.clone(),
        variant: frame.variant,
        generation: frame.generation,
        seq: frame.seq,
        nodes: (0..2)
            .map(|index| ViewIdentity {
                index,
                mounted: Some(wire::MountedViewId::parse(u64::from(index) + 1).unwrap()),
            })
            .collect(),
    };
    (snapshot, frame)
}

#[test]
fn metadata_requires_the_exact_scene_build_and_publication() {
    let (snapshot, frame) = fixture();
    assert_eq!(
        snapshot.match_frame(&frame, None).unwrap().snapshot(),
        &snapshot
    );
    let other = wire::SceneInstanceId::parse("other").unwrap();
    assert!(matches!(
        snapshot.match_frame(&frame, Some(&other)),
        Err(wire::ViewMismatch::Scene)
    ));
    let mut changed = frame.clone();
    changed.seq += 1;
    assert!(matches!(
        snapshot.match_frame(&changed, None),
        Err(wire::ViewMismatch::Frame)
    ));
    changed = frame.clone();
    changed.generation += 1;
    assert!(matches!(
        snapshot.match_frame(&changed, None),
        Err(wire::ViewMismatch::Frame)
    ));
    changed = frame.clone();
    changed.variant += 1;
    assert!(matches!(
        snapshot.match_frame(&changed, None),
        Err(wire::ViewMismatch::Scene)
    ));
    changed = frame;
    changed.preview = "other".into();
    assert!(matches!(
        snapshot.match_frame(&changed, None),
        Err(wire::ViewMismatch::Scene)
    ));
}

#[test]
fn missing_unknown_duplicate_and_unavailable_identities_refuse_the_join() {
    let (snapshot, frame) = fixture();
    let mut changed = snapshot.clone();
    changed.nodes.pop();
    assert!(matches!(
        changed.match_frame(&frame, None),
        Err(wire::ViewMismatch::MissingSite(1))
    ));
    changed = snapshot.clone();
    changed.nodes[1].index = 2;
    assert!(matches!(
        changed.match_frame(&frame, None),
        Err(wire::ViewMismatch::UnknownSite(2))
    ));
    changed = snapshot.clone();
    changed.nodes[1].index = 0;
    assert!(matches!(
        changed.match_frame(&frame, None),
        Err(wire::ViewMismatch::DuplicateSite(0))
    ));
    changed = snapshot.clone();
    changed.nodes[1].mounted = changed.nodes[0].mounted;
    assert!(matches!(
        changed.match_frame(&frame, None),
        Err(wire::ViewMismatch::DuplicateView(_))
    ));
    changed = snapshot.clone();
    changed.nodes[1].mounted = None;
    assert!(matches!(
        changed.match_frame(&frame, None),
        Err(wire::ViewMismatch::UnavailableSite(1))
    ));
    let mut duplicate_frame = frame;
    duplicate_frame.sites[1].index = 0;
    assert!(matches!(
        snapshot.match_frame(&duplicate_frame, None),
        Err(wire::ViewMismatch::DuplicateSite(0))
    ));
}

#[test]
fn zero_id_is_malformed_on_the_wire() {
    let (snapshot, _) = fixture();
    let mut bytes = wire::encode(&Message::ToIde(ToIde::ViewSnapshot(snapshot)));
    let end = bytes.len();
    bytes[end - 8..].fill(0);
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert!(matches!(
        reader.pull(),
        Err(wire::Malformed::InvalidMountedView)
    ));
}
