use bunny_ui_preview::wire::{
    self, InstanceCommand as C, Message, Reader, Received, SceneInstanceId, ToHost, ToIde,
};

fn id() -> SceneInstanceId {
    SceneInstanceId::parse("artboard:17").unwrap()
}
fn round_trip(message: Message) {
    let bytes = wire::encode(&message);
    let mut reader = Reader::new();
    for byte in bytes {
        reader.feed(&[byte]);
    }
    assert_eq!(reader.pull().unwrap(), Some(Received::Message(message)));
    assert!(reader.pull().unwrap().is_none());
}

#[test]
fn instance_messages_preserve_identity_and_every_command_payload() {
    let site = wire::SiteKey {
        file: "src/lib.rs".into(),
        line: 3,
        column: 7,
    };
    round_trip(Message::ToIde(ToIde::InstanceFrame {
        instance: id(),
        frame: Box::new(wire::Frame {
            preview: "card".into(),
            variant: 1,
            seq: 2,
            generation: 3,
            size: wire::Size {
                width: 100.0,
                height: 200.0,
            },
            scale: 2.0,
            background: wire::Color {
                r: 1,
                g: 2,
                b: 3,
                a: 255,
            },
            strings: vec!["hello".into()],
            commands: Vec::new(),
            sites: Vec::new(),
            hosts: Vec::new(),
            metrics: wire::Metrics {
                build_ms: 1.0,
                layout_ms: 2.0,
                body_passes: 3,
                commands: 0,
            },
        }),
    }));
    let commands = [
        C::Retrait(wire::Traits::default()),
        C::Resize(wire::Size {
            width: 321.0,
            height: 456.0,
        }),
        C::Pointer {
            kind: wire::PointerKind::Down,
            x: 1.0,
            y: 2.0,
            modifiers: 3,
        },
        C::Wheel {
            x: 1.0,
            y: 2.0,
            dx: 3.0,
            dy: 4.0,
        },
        C::Key {
            key: "Enter".into(),
            modifiers: 4,
        },
        C::Text {
            text: "Olá".into()
        },
        C::Touch {
            phase: wire::TouchPhase::Moved,
            touch: 99,
            x: 1.0,
            y: 2.0,
            taps: 2,
        },
        C::Magnify {
            x: 1.0,
            y: 2.0,
            scale: 1.5,
        },
        C::Override {
            site: site.clone(),
            op: wire::OverrideOp::Append,
            patch: wire::Patch::PaddingLength(16.0),
        },
        C::Place {
            site,
            op: wire::OverrideOp::Replace,
            patch: wire::PlacePatch::Span(2, 3),
        },
        C::ClearOverrides,
        C::RequestSnapshot,
    ];
    for command in commands {
        round_trip(Message::ToHost(ToHost::Instance {
            instance: id(),
            variant: 1,
            command,
        }));
    }
    round_trip(Message::ToHost(ToHost::SelectInstances(vec![
        wire::InstanceSelection {
            instance: id(),
            ident: "card".into(),
            traits: wire::Traits::default(),
        },
    ])));
    round_trip(Message::ToIde(ToIde::InstancePanicked {
        instance: id(),
        preview: "card".into(),
        variant: 1,
        message: "broken".into(),
    }));
    round_trip(Message::ToIde(ToIde::InstanceError {
        instance: id(),
        variant: Some(3),
        message: "missing".into(),
    }));
    round_trip(Message::ToIde(ToIde::InstanceError {
        instance: id(),
        variant: None,
        message: "duplicate".into(),
    }));
    round_trip(Message::ToIde(ToIde::InstanceSnapshot {
        instance: id(),
        preview: "card".into(),
        variant: 1,
        size: (1, 1),
        rgba: vec![0, 1, 2, 255],
    }));
}

#[test]
fn malformed_instance_ids_are_refused_without_changing_old_message_kinds() {
    for invalid in ["", " ", "a\nb", " a"] {
        assert!(SceneInstanceId::parse(invalid).is_err());
    }
    let bytes = wire::encode(&Message::ToHost(ToHost::SelectInstances(Vec::new())));
    assert_eq!(bytes[4], 80);
    assert_eq!(wire::encode(&Message::ToHost(ToHost::Bye))[4], 76);
    assert_eq!(wire::WIRE_VERSION, 1);
    let mut malformed = wire::encode(&Message::ToHost(ToHost::Instance {
        instance: id(),
        variant: 0,
        command: C::ClearOverrides,
    }));
    *malformed.last_mut().unwrap() = 255;
    let mut reader = Reader::new();
    reader.feed(&malformed);
    assert!(reader.pull().is_err());
}

#[test]
fn clipboard_text_roundtrips_without_changing_ordinary_key_or_text_messages() {
    let paste = wire::ClipboardPaste {
        key: "v".into(),
        modifiers: 2,
        text: "café 🐇\r\nΩ\n".into(),
    };
    let legacy = Message::ToHost(ToHost::Paste {
        id: "scene".into(),
        paste: paste.clone(),
    });
    assert_eq!(wire::encode(&legacy)[4], 86);
    round_trip(legacy);
    round_trip(Message::ToHost(ToHost::Instance {
        instance: id(),
        variant: 1,
        command: C::Paste(paste),
    }));
    assert_eq!(
        wire::encode(&Message::ToHost(ToHost::Key {
            id: "scene".into(),
            key: "v".into(),
            modifiers: 2,
        }))[4],
        69
    );
    assert_eq!(
        wire::encode(&Message::ToHost(ToHost::Text {
            id: "scene".into(),
            text: "literal".into(),
        }))[4],
        70
    );
    for modifiers in [2, 8, 10] {
        assert!(wire::ClipboardPaste::is_shortcut("v", modifiers));
        assert!(wire::ClipboardPaste::is_shortcut("V", modifiers));
    }
    for (key, modifiers) in [("v", 0), ("v", 3), ("v", 6), ("c", 2), ("v", 258)] {
        assert!(!wire::ClipboardPaste::is_shortcut(key, modifiers));
    }
}
