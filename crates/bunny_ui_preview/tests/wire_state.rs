//! State observations must join the exact selected occurrence and frame.
use bunny_ui_preview::wire::{self, *};

fn request(n: u64) -> StateRequestId {
    StateRequestId::parse(n).unwrap()
}
fn target() -> StateTarget {
    StateTarget {
        instance: Some(SceneInstanceId::parse("board").unwrap()),
        preview: "likes".into(),
        variant: 0,
        generation: 3,
        mounted: MountedViewId::parse(9).unwrap(),
    }
}
fn observation(value: StateValue) -> StateDependency {
    StateDependency {
        key: DependencyKey::State(4),
        evidence: StateEvidence::State {
            type_name: "i32".into(),
            constructor: StateConstructor {
                file: "src/card.rs".into(),
                line: 8,
                column: 3,
            },
            version: 2,
            value,
        },
    }
}
fn roundtrip(message: Message) {
    let mut reader = Reader::new();
    for byte in wire::encode(&message) {
        reader.feed(&[byte]);
    }
    assert_eq!(reader.pull().unwrap(), Some(Received::Message(message)));
}
#[test]
fn all_values_and_request_outcomes_roundtrip_without_loss() {
    for value in [
        StateValue::Signed(i128::MIN),
        StateValue::Unsigned(u128::MAX),
        StateValue::FloatBits(f64::NAN.to_bits()),
        StateValue::FloatBits((-0.0_f64).to_bits()),
        StateValue::Bool(true),
        StateValue::Char('🦀'),
        StateValue::Text("Likes: café".into()),
        StateValue::Opaque,
        StateValue::TooLarge { bytes: 4097 },
        StateValue::Updating,
    ] {
        roundtrip(Message::ToIde(ToIde::StateObservation(
            StateReply::Snapshot(StateSnapshot {
                request: request(1),
                target: target(),
                seq: 7,
                read: StateRead::Values(vec![observation(value)]),
            }),
        )));
    }
    roundtrip(Message::ToHost(ToHost::ObserveState {
        request: request(2),
        target: Some(target()),
    }));
    roundtrip(Message::ToHost(ToHost::ObserveState {
        request: request(3),
        target: None,
    }));
    roundtrip(Message::ToIde(ToIde::StateObservation(
        StateReply::Stopped {
            request: request(3),
        },
    )));
    roundtrip(Message::ToIde(ToIde::StateObservation(
        StateReply::Refused {
            request: request(4),
            reason: StateRefusal::StaleRequest,
        },
    )));
    assert!(StateRequestId::parse(0).is_err());
}
#[test]
fn zero_request_and_oversized_metadata_are_rejected_at_decode() {
    let mut bytes = wire::encode(&Message::ToHost(ToHost::ObserveState {
        request: request(1),
        target: None,
    }));
    bytes[5..13].fill(0);
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert!(matches!(
        reader.pull(),
        Err(Malformed::InvalidStateObservation)
    ));
    let mut long = target();
    long.preview = "x".repeat(STATE_TEXT_LIMIT + 1);
    let mut reader = Reader::new();
    reader.feed(&wire::encode(&Message::ToHost(ToHost::ObserveState {
        request: request(1),
        target: Some(long),
    })));
    assert!(matches!(
        reader.pull(),
        Err(Malformed::StateObservationLimit)
    ));
}
#[test]
fn duplicate_dependencies_and_limits_refuse_a_proof() {
    let dep = observation(StateValue::Signed(0));
    assert!(
        StateRead::Values(vec![dep.clone(), dep.clone()])
            .validate()
            .is_err()
    );
    assert!(
        StateRead::Values(vec![dep; STATE_DEPENDENCY_LIMIT + 1])
            .validate()
            .is_err()
    );
    let mut reader = Reader::new();
    reader.feed(&wire::encode(&Message::ToIde(ToIde::StateObservation(
        StateReply::Snapshot(StateSnapshot {
            request: request(1),
            target: target(),
            seq: 7,
            read: StateRead::Values(vec![observation(StateValue::Text(
                "x".repeat(STATE_TEXT_LIMIT + 1),
            ))]),
        }),
    ))));
    assert!(matches!(
        reader.pull(),
        Err(Malformed::StateObservationLimit)
    ));
}

fn frame_views(target: &StateTarget, seq: u64) -> (Frame, ViewSnapshot) {
    let frame = Frame {
        preview: target.preview.clone(),
        variant: target.variant,
        seq,
        generation: target.generation,
        size: Size::default(),
        scale: 1.0,
        background: Color::default(),
        strings: Vec::new(),
        commands: Vec::new(),
        hosts: Vec::new(),
        metrics: Metrics {
            build_ms: 0.0,
            layout_ms: 0.0,
            body_passes: 0,
            commands: 0,
        },
        sites: vec![SiteBox {
            index: 2,
            parent: None,
            kind: 0,
            modifier: None,
            file: None,
            name: None,
            line: 0,
            column: 0,
            frame: Rect::default(),
            path: None,
        }],
    };
    let views = ViewSnapshot {
        instance: target.instance.clone(),
        preview: target.preview.clone(),
        variant: target.variant,
        generation: target.generation,
        seq,
        nodes: vec![ViewIdentity {
            index: 2,
            mounted: Some(target.mounted),
        }],
    };
    (frame, views)
}
#[test]
fn observations_require_current_request_target_and_exact_validated_frame() {
    let selected = target();
    let (frame, views) = frame_views(&selected, 7);
    let proof = views
        .match_frame(&frame, selected.instance.as_ref())
        .unwrap();
    let snapshot = StateSnapshot {
        request: request(1),
        target: selected.clone(),
        seq: 7,
        read: StateRead::Values(vec![observation(StateValue::Signed(5))]),
    };
    assert_eq!(
        snapshot
            .match_views(&proof, request(1), &selected)
            .unwrap()
            .snapshot(),
        &snapshot
    );
    assert!(matches!(
        snapshot.match_views(&proof, request(2), &selected),
        Err(StateMismatch::Request)
    ));
    let mut another = selected.clone();
    another.mounted = MountedViewId::parse(10).unwrap();
    assert!(matches!(
        snapshot.match_views(&proof, request(1), &another),
        Err(StateMismatch::Target)
    ));
    for field in 0..6 {
        let mut changed = snapshot.clone();
        match field {
            0 => changed.seq += 1,
            1 => changed.target.generation += 1,
            2 => changed.target.variant += 1,
            3 => changed.target.instance = None,
            4 => changed.target.preview.push('x'),
            _ => changed.target.mounted = MountedViewId::parse(88).unwrap(),
        }
        assert!(
            changed
                .match_views(&proof, request(1), &changed.target)
                .is_err(),
            "mismatched field {field}"
        );
    }
    let mut duplicate = snapshot;
    duplicate.read = StateRead::Values(vec![observation(StateValue::Signed(1)); 2]);
    assert!(matches!(
        duplicate.match_views(&proof, request(1), &selected),
        Err(StateMismatch::DuplicateDependency(_))
    ));
}
#[test]
fn explicit_unavailable_outcomes_roundtrip_and_every_truncation_fails() {
    let mut messages = vec![Message::ToHost(ToHost::ObserveState {
        request: request(1),
        target: Some(target()),
    })];
    for read in [
        StateRead::NoBinding,
        StateRead::UnavailableBinding,
        StateRead::Borrowed,
        StateRead::Limit,
        StateRead::Values(vec![
            StateDependency {
                key: DependencyKey::State(1),
                evidence: StateEvidence::Missing,
            },
            StateDependency {
                key: DependencyKey::Store(1),
                evidence: StateEvidence::Unsupported,
            },
        ]),
    ] {
        messages.push(Message::ToIde(ToIde::StateObservation(
            StateReply::Snapshot(StateSnapshot {
                request: request(1),
                target: target(),
                seq: 7,
                read,
            }),
        )));
    }
    for reason in [
        StateRefusal::Disabled,
        StateRefusal::SceneMissing,
        StateRefusal::AmbiguousScene,
        StateRefusal::Generation,
        StateRefusal::NotPublished,
        StateRefusal::ViewMissing,
        StateRefusal::Poisoned,
        StateRefusal::StaleRequest,
        StateRefusal::Limit,
    ] {
        messages.push(Message::ToIde(ToIde::StateObservation(
            StateReply::Refused {
                request: request(1),
                reason,
            },
        )));
    }
    for message in messages {
        let bytes = wire::encode(&message);
        for end in 5..bytes.len() {
            assert!(wire::decode(bytes[4], &bytes[5..end]).is_err());
        }
        let mut trailing = bytes[5..].to_vec();
        trailing.push(0);
        assert!(matches!(
            wire::decode(bytes[4], &trailing),
            Err(Malformed::Trailing { .. })
        ));
        roundtrip(message);
    }
}
#[test]
fn invalid_scalar_duplicate_dependency_and_large_count_are_malformed() {
    let message = |read| {
        Message::ToIde(ToIde::StateObservation(StateReply::Snapshot(
            StateSnapshot {
                request: request(1),
                target: target(),
                seq: 7,
                read,
            },
        )))
    };
    let bytes = wire::encode(&message(StateRead::Values(vec![
        observation(
            StateValue::Signed(0)
        );
        2
    ])));
    assert!(matches!(
        wire::decode(bytes[4], &bytes[5..]),
        Err(Malformed::InvalidStateObservation)
    ));
    let mut bytes = wire::encode(&message(StateRead::Values(Vec::new())));
    let len = bytes.len();
    bytes[len - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        wire::decode(bytes[4], &bytes[5..]),
        Err(Malformed::StateObservationLimit)
    ));
    let mut bytes = wire::encode(&message(StateRead::Values(vec![observation(
        StateValue::Char('a'),
    )])));
    let len = bytes.len();
    bytes[len - 4..].copy_from_slice(&0xd800_u32.to_le_bytes());
    assert!(matches!(
        wire::decode(bytes[4], &bytes[5..]),
        Err(Malformed::InvalidStateObservation)
    ));
}

#[test]
fn additive_control_messages_have_fixed_kinds_and_bytes() {
    assert_eq!(caps::STATE_OBSERVATIONS, 128);
    assert_eq!(
        wire::encode(&Message::ToHost(ToHost::ObserveState {
            request: request(1),
            target: None
        })),
        vec![10, 0, 0, 0, 85, 1, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        wire::encode(&Message::ToIde(ToIde::StateObservation(
            StateReply::Stopped {
                request: request(1)
            }
        ))),
        vec![10, 0, 0, 0, 16, 1, 1, 0, 0, 0, 0, 0, 0, 0]
    );
}
