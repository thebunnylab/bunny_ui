//! The shell protocol as bytes: what a Trinity shell and a block built
//! with any engine version must keep agreeing on.

use bunny_ui_preview::wire;
use bunny_ui_shell::protocol::{
    self, AccessAction, Capabilities, Context, Fetch, Fetched, Filter, Malformed, Message, Modifiers, Outcome,
    PointerButton, PointerPhase, Reader, Received, Requested, Response, Role, Row, Rows, Scheme, SemanticNode, ToGuest,
    ToHost, Versions, Viewport,
};

fn round_trip(message: Message) {
    let bytes = protocol::encode(&message);
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert_eq!(reader.pull(), Ok(Some(Received::Message(message))));
    assert_eq!(reader.pull(), Ok(None));
}

fn scene() -> wire::Frame {
    wire::Frame {
        preview: String::new(),
        variant: 0,
        seq: 3,
        generation: 0,
        size: wire::Size { width: 10.0, height: 20.0 },
        scale: 2.0,
        background: wire::Color { r: 255, g: 255, b: 255, a: 255 },
        strings: vec!["hi".into()],
        commands: vec![wire::Draw::PopClip],
        sites: Vec::new(),
        hosts: Vec::new(),
        metrics: wire::Metrics { build_ms: 0.0, layout_ms: 0.0, body_passes: 0, commands: 1 },
    }
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
        Message::ToHost(ToHost::Scene(scene())),
        Message::ToHost(ToHost::Resource(wire::Resource {
            key: 5,
            kind: wire::ResourceKind::Rgba { size: (1, 1), rgba: vec![1, 2, 3, 4] },
        })),
        Message::ToHost(ToHost::Pace { animating: true }),
        Message::ToHost(ToHost::Fault { reason: "kind 64 ended early".into() }),
        Message::ToGuest(ToGuest::Context(Context::default())),
        Message::ToGuest(ToGuest::Context(Context {
            filters: vec![
                Filter { field: "stage".into(), value: "doing".into() },
                Filter { field: "owner".into(), value: "Ana Lúcia".into() },
            ],
            record: Some("task_01".into()),
        })),
        Message::ToGuest(ToGuest::Rows(Rows {
            view: "devflow_active_work".into(),
            columns: vec!["title".into(), "owner".into(), "estimate".into()],
            rows: vec![
                Row { record: "task_01".into(), cells: vec![Some("Ship it".into()), Some("Ana".into()), Some("3".into())] },
                Row { record: "task_02".into(), cells: vec![Some("Review".into()), None, None] },
            ],
        })),
        Message::ToGuest(ToGuest::Rows(Rows { view: "empty".into(), columns: Vec::new(), rows: Vec::new() })),
        Message::ToGuest(ToGuest::Answer { request: 1, outcome: Outcome::Landed }),
        Message::ToGuest(ToGuest::Answer { request: 2, outcome: Outcome::Held }),
        Message::ToGuest(ToGuest::Answer { request: 3, outcome: Outcome::Declined }),
        Message::ToGuest(ToGuest::Answer { request: 4, outcome: Outcome::Refused("not declared".into()) }),
        Message::ToHost(ToHost::Request(Requested {
            request: 9,
            action: "send_task_to_review".into(),
            record: Some("task_01".into()),
        })),
        Message::ToHost(ToHost::Request(Requested { request: 10, action: "new_task".into(), record: None })),
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

/// The signature's kinds sit where the protocol document puts them, and a
/// request's bytes never move: a block built today asks a shell of any
/// later year in these bytes.
#[test]
fn a_request_is_pinned_byte_for_byte() {
    let bytes = protocol::encode(&Message::ToHost(ToHost::Request(Requested {
        request: 2,
        action: "go".into(),
        record: None,
    })));
    let mut expected = Vec::new();
    expected.extend_from_slice(&16u32.to_le_bytes());
    expected.push(0x08);
    expected.extend_from_slice(&2u64.to_le_bytes());
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(b"go");
    expected.push(0);
    assert_eq!(bytes, expected);
    let kind_of = |message: ToGuest| protocol::encode(&Message::ToGuest(message))[4];
    assert_eq!(kind_of(ToGuest::Context(Context::default())), 0x49);
    assert_eq!(kind_of(ToGuest::Rows(Rows { view: String::new(), columns: Vec::new(), rows: Vec::new() })), 0x4a);
    assert_eq!(kind_of(ToGuest::Answer { request: 0, outcome: Outcome::Landed }), 0x4b);
    assert_eq!(Capabilities::CONTEXT.bits(), 1 << 6);
    assert_eq!(Capabilities::READS.bits(), 1 << 7);
    assert_eq!(Capabilities::REQUESTS.bits(), 1 << 8);
}

/// A block built before the signature knew none of its bits: the host's
/// offer of them drops out of its answer, and the host sends it none of
/// the signature's kinds.
#[test]
fn an_older_block_answers_without_the_signature_bits() {
    let older = Capabilities::from_bits(0b11_1111);
    let offered = Capabilities::KNOWN;
    let answered = offered & older;
    assert!(!answered.contains(Capabilities::CONTEXT));
    assert!(!answered.contains(Capabilities::READS));
    assert!(!answered.contains(Capabilities::REQUESTS));
    assert_eq!(
        format!("{:?}", Capabilities::KNOWN),
        "{SCENE, POINTER, KEYS, TEXT, ACCESSIBILITY, TIME, CONTEXT, READS, REQUESTS, FETCHES}"
    );
}

/// A filter, a row and a cell each sit behind their own length, so a
/// newer shell may append to them and an older block reads what it knows.
#[test]
fn a_row_grown_at_its_end_reads_as_the_row_it_was() {
    let rows = Rows {
        view: "v".into(),
        columns: vec!["title".into()],
        rows: vec![Row { record: "r".into(), cells: vec![Some("t".into())] }],
    };
    let bytes = protocol::encode(&Message::ToGuest(ToGuest::Rows(rows.clone())));
    // the row's element: its length sits after the view, the column and the row count
    let at = 4 + 1 + (4 + 1) + 4 + (4 + 5) + 4;
    let mut grown = bytes[..at].to_vec();
    let element = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    grown.extend_from_slice(&(element + 3).to_le_bytes());
    grown.extend_from_slice(&bytes[at + 4..]);
    grown.extend_from_slice(&[7, 7, 7]);
    let total = u32::from_le_bytes(grown[..4].try_into().unwrap()) + 3;
    grown[..4].copy_from_slice(&total.to_le_bytes());
    let mut reader = Reader::new();
    reader.feed(&grown);
    assert_eq!(reader.pull(), Ok(Some(Received::Message(Message::ToGuest(ToGuest::Rows(rows))))));
}

#[test]
fn a_cell_is_found_by_its_column() {
    let rows = Rows {
        view: "v".into(),
        columns: vec!["title".into(), "owner".into()],
        rows: vec![Row { record: "r".into(), cells: vec![Some("t".into()), None] }],
    };
    let row = &rows.rows[0];
    assert_eq!(rows.cell(row, "title"), Some("t"));
    assert_eq!(rows.cell(row, "owner"), None);
    assert_eq!(rows.cell(row, "stage"), None);
}

#[test]
fn an_unknown_outcome_is_refused_by_name() {
    let mut bytes = protocol::encode(&Message::ToGuest(ToGuest::Answer { request: 1, outcome: Outcome::Landed }));
    *bytes.last_mut().unwrap() = 9;
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert_eq!(reader.pull(), Err(Malformed::BadTag { what: "Outcome", tag: 9 }));
}

// =============================================================================
// The Effect level: fetches
// =============================================================================

/// Every shape of a fetch and its answer survives the bytes.
#[test]
fn a_fetch_and_every_answer_round_trip() {
    let messages = [
        Message::ToHost(ToHost::Fetch(Fetch { fetch: 3, url: "https://api.example.com/rates?day=1".into() })),
        Message::ToGuest(ToGuest::Fetched {
            fetch: 3,
            fetched: Fetched::Answered(Response {
                status: 200,
                content_type: Some("application/json".into()),
                body: br#"{"usd":5.1}"#.to_vec(),
            }),
        }),
        Message::ToGuest(ToGuest::Fetched {
            fetch: 4,
            fetched: Fetched::Answered(Response { status: 204, content_type: None, body: Vec::new() }),
        }),
        Message::ToGuest(ToGuest::Fetched { fetch: 5, fetched: Fetched::Refused("not granted".into()) }),
        Message::ToGuest(ToGuest::Fetched { fetch: 6, fetched: Fetched::Failed("timed out".into()) }),
    ];
    for message in messages {
        round_trip(message);
    }
}

/// The Effect level's kinds and bit sit where the protocol document puts
/// them, and a fetch's bytes never move.
#[test]
fn a_fetch_is_pinned_byte_for_byte() {
    let bytes = protocol::encode(&Message::ToHost(ToHost::Fetch(Fetch { fetch: 7, url: "https://a.io/".into() })));
    let mut expected = Vec::new();
    expected.extend_from_slice(&26u32.to_le_bytes());
    expected.push(0x09);
    expected.extend_from_slice(&7u64.to_le_bytes());
    expected.extend_from_slice(&13u32.to_le_bytes());
    expected.extend_from_slice(b"https://a.io/");
    assert_eq!(bytes, expected);

    let answered = protocol::encode(&Message::ToGuest(ToGuest::Fetched {
        fetch: 7,
        fetched: Fetched::Answered(Response { status: 404, content_type: None, body: b"no".to_vec() }),
    }));
    let mut expected = Vec::new();
    expected.extend_from_slice(&19u32.to_le_bytes());
    expected.push(0x4c);
    expected.extend_from_slice(&7u64.to_le_bytes());
    expected.push(0); // answered
    expected.extend_from_slice(&404u16.to_le_bytes());
    expected.push(0); // no content type
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(b"no");
    assert_eq!(answered, expected);
    assert_eq!(Capabilities::FETCHES.bits(), 1 << 9);
}

/// A block built before the Effect level knows no `FETCHES`: the bit drops
/// out of its answer, and a `Fetched` sent to it anyway is skipped by its
/// length like any kind it has never heard of.
#[test]
fn an_older_block_answers_without_the_fetch_bit() {
    let signature_era = Capabilities::from_bits(0b1_1111_1111);
    assert!(!(Capabilities::KNOWN & signature_era).contains(Capabilities::FETCHES));
    let bytes = protocol::encode(&Message::ToGuest(ToGuest::Fetched { fetch: 1, fetched: Fetched::Refused("no".into()) }));
    assert_eq!(bytes[4], 0x4c);
    assert!(bytes[4] >= 0x40, "a fetch's answer travels host to guest");
}

#[test]
fn an_unknown_fetch_answer_is_refused_by_name() {
    let mut bytes = protocol::encode(&Message::ToGuest(ToGuest::Fetched { fetch: 1, fetched: Fetched::Refused(String::new()) }));
    // the tag sits after the length, the kind and the fetch's number
    bytes[4 + 1 + 8] = 9;
    let mut reader = Reader::new();
    reader.feed(&bytes);
    assert_eq!(reader.pull(), Err(Malformed::BadTag { what: "Fetched", tag: 9 }));
}
