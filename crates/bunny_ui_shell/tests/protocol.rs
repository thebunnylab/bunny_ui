//! The shell protocol as bytes: what a Trinity shell and a block built
//! with any engine version must keep agreeing on.

use bunny_ui_shell::protocol::{
    self, AccessAction, Capabilities, Malformed, Message, Modifiers, PointerButton, PointerPhase, Reader, Received,
    Role, Scheme, SemanticNode, ToGuest, ToHost, Versions, Viewport,
};

fn round_trip(message: Message) {
    let bytes = protocol::encode(&message);
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert_eq!(reader.pull(), Ok(Some(Received::Message(message))));
    assert_eq!(reader.pull(), Ok(None));
}

fn viewport() -> Viewport {
    Viewport { width: 320.0, height: 120.0, scale: 2.0, scheme: Scheme::Light }
}

#[test]
fn every_message_survives_its_bytes() {
    let messages = vec![
        Message::ToGuest(ToGuest::Offer {
            versions: Versions::THIS,
            capabilities: Capabilities::KNOWN,
            viewport: viewport(),
        }),
        Message::ToGuest(ToGuest::Resize(Viewport { scheme: Scheme::Dark, ..viewport() })),
        Message::ToGuest(ToGuest::Pointer {
            phase: PointerPhase::Down,
            x: 10.5,
            y: 20.25,
            button: PointerButton::Primary,
            clicks: 2,
            modifiers: Modifiers::SHIFT | Modifiers::COMMAND,
        }),
        Message::ToGuest(ToGuest::Wheel { x: 1.0, y: 2.0, dx: -3.0, dy: 40.0, modifiers: Modifiers::NONE }),
        Message::ToGuest(ToGuest::Key { key: "ArrowLeft".into(), typed: String::new(), modifiers: Modifiers::OPTION }),
        Message::ToGuest(ToGuest::Key { key: "a".into(), typed: "A".into(), modifiers: Modifiers::SHIFT }),
        Message::ToGuest(ToGuest::Text { text: "olá, mundo".into() }),
        Message::ToGuest(ToGuest::Focus { focused: true }),
        Message::ToGuest(ToGuest::Accessibility { node: 7, action: AccessAction::SetText("x".into()) }),
        Message::ToGuest(ToGuest::Accessibility { node: 8, action: AccessAction::SetValue(0.5) }),
        Message::ToGuest(ToGuest::Accessibility { node: 9, action: AccessAction::Activate }),
        Message::ToGuest(ToGuest::Tick { dt_ms: 16.0 }),
        Message::ToHost(ToHost::Accept {
            version: 1,
            capabilities: Capabilities::SCENE | Capabilities::POINTER,
            engine: "bunny-ui 0.5.0".into(),
        }),
        Message::ToHost(ToHost::Refuse { versions: Versions { oldest: 3, newest: 4 }, reason: "too new".into() }),
        Message::ToHost(ToHost::Semantics {
            seq: 4,
            nodes: vec![SemanticNode {
                id: 11,
                role: Role::Button,
                label: "Add".into(),
                value: None,
                bounds: (1.0, 2.0, 30.0, 20.0),
                focused: false,
                enabled: true,
                checked: None,
                expanded: None,
                level: None,
                multiline: false,
            }],
        }),
        Message::ToHost(ToHost::Pace { animating: true }),
        Message::ToHost(ToHost::Fault { reason: "kind 64 ended early".into() }),
    ];
    for message in messages {
        round_trip(message);
    }
}

/// The handshake's first bytes never move: a host can tell a block from
/// anything else before it reads one field.
#[test]
fn the_offer_is_pinned_byte_for_byte() {
    let bytes = protocol::encode(&Message::ToGuest(ToGuest::Offer {
        versions: Versions { oldest: 1, newest: 1 },
        capabilities: Capabilities::SCENE,
        viewport: Viewport { width: 1.0, height: 2.0, scale: 1.0, scheme: Scheme::Dark },
    }));
    let mut expected = Vec::new();
    expected.extend_from_slice(&46u32.to_le_bytes()); // the kind and the payload
    expected.push(0x40);
    expected.extend_from_slice(b"BNYS");
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend_from_slice(&1u64.to_le_bytes());
    expected.extend_from_slice(&1f64.to_le_bytes());
    expected.extend_from_slice(&2f64.to_le_bytes());
    expected.extend_from_slice(&1f64.to_le_bytes());
    expected.push(1);
    assert_eq!(bytes, expected);
    assert_eq!(protocol::PROTOCOL_VERSION, 1);
}

#[test]
fn the_newest_common_version_is_chosen() {
    let host = Versions { oldest: 1, newest: 3 };
    assert_eq!(host.negotiate(Versions { oldest: 1, newest: 1 }), Some(1));
    assert_eq!(host.negotiate(Versions { oldest: 2, newest: 5 }), Some(3));
    assert_eq!(host.negotiate(Versions { oldest: 4, newest: 5 }), None);
    assert_eq!(Versions { oldest: 2, newest: 2 }.negotiate(Versions { oldest: 1, newest: 1 }), None);
}

/// A newer peer may grant a capability this build has no name for: it
/// survives the bytes, and the intersection a guest answers with drops it.
#[test]
fn unknown_capabilities_are_kept_on_the_wire_and_dropped_by_the_answer() {
    let newer = Capabilities::from_bits(Capabilities::KNOWN.bits() | 1 << 40);
    assert_eq!(newer.bits() & 1 << 40, 1 << 40);
    assert_eq!(newer & Capabilities::KNOWN, Capabilities::KNOWN);
    assert!(!(newer & Capabilities::KNOWN).contains(Capabilities::from_bits(1 << 40)));
}

/// A kind this build does not know is skipped by its length, and the
/// reader carries on — the protocol grows by new kinds, never by a bump.
#[test]
fn an_unknown_kind_is_skipped_by_its_length() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.push(0x7e);
    bytes.extend_from_slice(&[1, 2, 3]);
    bytes.extend(protocol::encode(&Message::ToGuest(ToGuest::Focus { focused: false })));
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert_eq!(reader.pull(), Ok(Some(Received::Unknown { kind: 0x7e, len: 3 })));
    assert_eq!(reader.pull(), Ok(Some(Received::Message(Message::ToGuest(ToGuest::Focus { focused: false })))));
}

/// A newer peer appends fields to a message it already sends: an older
/// reader takes the fields it knows and leaves the rest.
#[test]
fn fields_appended_by_a_newer_peer_are_left_unread() {
    let mut bytes = protocol::encode(&Message::ToGuest(ToGuest::Tick { dt_ms: 8.0 }));
    let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) + 2;
    bytes[0..4].copy_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&[9, 9]);
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert_eq!(reader.pull(), Ok(Some(Received::Message(Message::ToGuest(ToGuest::Tick { dt_ms: 8.0 })))));
}

#[test]
fn bytes_fed_in_pieces_wait_for_the_whole_message() {
    let bytes = protocol::encode(&Message::ToGuest(ToGuest::Text { text: "abc".into() }));
    let mut reader = Reader::new();
    for byte in &bytes[..bytes.len() - 1] {
        reader.feed(std::slice::from_ref(byte));
        assert_eq!(reader.pull(), Ok(None));
    }
    reader.feed(&bytes[bytes.len() - 1..]);
    assert!(matches!(reader.pull(), Ok(Some(Received::Message(_)))));
}

#[test]
fn malformed_bytes_are_named() {
    let mut reader = Reader::new();
    let mut offer = protocol::encode(&Message::ToGuest(ToGuest::Offer {
        versions: Versions::THIS,
        capabilities: Capabilities::KNOWN,
        viewport: viewport(),
    }));
    offer[5] = b'X';
    reader.feed(&offer);
    assert_eq!(reader.pull(), Err(Malformed::BadMagic));

    let mut reader = Reader::new();
    let mut truncated = protocol::encode(&Message::ToGuest(ToGuest::Tick { dt_ms: 1.0 }));
    truncated[0..4].copy_from_slice(&5u32.to_le_bytes());
    truncated.truncate(4 + 5);
    reader.feed(&truncated);
    assert_eq!(reader.pull(), Err(Malformed::Truncated { kind: 0x48 }));

    let mut reader = Reader::new();
    reader.feed(&(protocol::MAX_MESSAGE + 1).to_le_bytes());
    reader.feed(&[0x40]);
    assert_eq!(reader.pull(), Err(Malformed::Oversized { len: protocol::MAX_MESSAGE + 1 }));

    let mut reader = Reader::new();
    let mut pointer = protocol::encode(&Message::ToGuest(ToGuest::Pointer {
        phase: PointerPhase::Move,
        x: 0.0,
        y: 0.0,
        button: PointerButton::Primary,
        clicks: 0,
        modifiers: Modifiers::NONE,
    }));
    pointer[5] = 200; // the phase
    reader.feed(&pointer);
    assert_eq!(reader.pull(), Err(Malformed::BadTag { what: "PointerPhase", tag: 200 }));
}

/// A role this build has no name for reads as a plain group, so a newer
/// guest's control is still heard by an older shell.
#[test]
fn a_role_names_itself_and_an_unknown_one_is_a_group() {
    assert_eq!(Role::Button.name(), "button");
    assert_eq!(Role::from_name("button"), Role::Button);
    assert_eq!(Role::from_name("carousel"), Role::Group);
}
