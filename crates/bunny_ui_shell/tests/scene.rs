//! A guest and the test host, talking only in protocol bytes: the
//! handshake, the scene, the events and the semantics.

#![cfg(feature = "engine")]

extern crate bunny_ui_core as bunny_ui;

use bunny_ui::prelude::*;
use bunny_ui_shell::guest::Guest;
use bunny_ui_shell::host::{TestHost, native};
use bunny_ui_shell::protocol::{
    self, AccessAction, Capabilities, Message, Modifiers, PointerButton, PointerPhase, Role, Scheme, ToGuest, ToHost,
    Versions, Viewport,
};

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(text!("count: {}", self.count), button(text("Add one"), move || self.count.add(1)))
            .alignment(HorizontalAlignment::Leading)
            .padding()
    }
}

fn counter() -> Counter {
    Counter { count: State::new(0) }
}

const VIEWPORT: Viewport = Viewport { width: 200.0, height: 90.0, scale: 2.0, scheme: Scheme::Light };

fn offer(capabilities: Capabilities) -> Vec<u8> {
    protocol::encode(&Message::ToGuest(ToGuest::Offer {
        versions: Versions::THIS,
        capabilities,
        viewport: VIEWPORT,
    }))
}

fn turn(guest: &mut Guest<Counter>, host: &mut TestHost, messages: &[ToGuest]) {
    let mut bytes = Vec::new();
    for message in messages {
        bytes.extend(protocol::encode(&Message::ToGuest(message.clone())));
    }
    host.receive(&guest.call(&bytes)).expect("the guest speaks the protocol");
}

fn click(x: f64, y: f64) -> [ToGuest; 2] {
    let pointer = |phase| ToGuest::Pointer {
        phase,
        x,
        y,
        button: PointerButton::Primary,
        clicks: 1,
        modifiers: Modifiers::NONE,
    };
    [pointer(PointerPhase::Down), pointer(PointerPhase::Up)]
}

#[test]
fn the_guest_accepts_the_newest_common_version_and_the_capabilities_it_knows() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    let wider = Capabilities::from_bits(Capabilities::KNOWN.bits() | 1 << 50);
    host.receive(&guest.call(&offer(wider))).unwrap();
    let session = host.session().expect("accepted");
    assert_eq!(session.version, protocol::PROTOCOL_VERSION);
    assert_eq!(session.capabilities, Capabilities::KNOWN);
    assert!(session.engine.starts_with("bunny-ui "), "{}", session.engine);
    assert!(host.scene().is_some(), "the first scene rides with the answer");
}

#[test]
fn a_host_with_no_common_version_is_refused_and_given_no_scene() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    let bytes = protocol::encode(&Message::ToGuest(ToGuest::Offer {
        versions: Versions { oldest: 90, newest: 91 },
        capabilities: Capabilities::KNOWN,
        viewport: VIEWPORT,
    }));
    host.receive(&guest.call(&bytes)).unwrap();
    assert!(host.session().is_none());
    let refusal = host.refusal().expect("refused");
    assert_eq!(refusal.0, Versions::THIS);
    assert!(host.scene().is_none());
    // events before a handshake are not the guest's to act on
    turn(&mut guest, &mut host, &click(20.0, 50.0));
    assert!(host.scene().is_none());
}

#[test]
fn the_scene_paints_what_the_native_shell_paints() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::KNOWN))).unwrap();
    let painted = host.paint().expect("the scene raises");
    let expected = native(counter, VIEWPORT, &[]);
    assert_eq!(painted.len(), 200 * 2 * 90 * 2 * 4);
    assert!(painted.chunks(4).any(|pixel| pixel != &painted[..4]), "the scene paints something");
    assert!(painted == expected[0], "the scene's pixels differ from the native render");
}

#[test]
fn a_click_over_the_protocol_lands_where_a_native_click_lands() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::KNOWN))).unwrap();
    let before = host.scene().unwrap().seq;
    let button = host
        .semantics()
        .iter()
        .find(|node| node.role == Role::Button)
        .map(|node| node.bounds)
        .expect("the button is exposed");
    let (x, y) = (button.0 + button.2 / 2.0, button.1 + button.3 / 2.0);
    turn(&mut guest, &mut host, &click(x, y));
    assert!(host.scene().unwrap().seq > before, "a click that changed the view sends a new scene");
    let labels: Vec<&str> = host.semantics().iter().map(|node| node.label.as_str()).collect();
    assert!(labels.contains(&"count: 1"), "{labels:?}");
    let expected = native(counter, VIEWPORT, &click(x, y));
    assert!(expected[0] != expected[1], "the click changes what the native shell paints");
    assert!(host.paint().unwrap() == expected[1], "after the click the pixels differ from the native render");
}

#[test]
fn an_accessibility_action_presses_the_button() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::KNOWN))).unwrap();
    let id = host.semantics().iter().find(|node| node.role == Role::Button).unwrap().id;
    turn(&mut guest, &mut host, &[ToGuest::Accessibility { node: id, action: AccessAction::Activate }]);
    assert!(host.semantics().iter().any(|node| &*node.label == "count: 1"));
}

#[test]
fn semantics_are_sent_only_to_a_host_that_granted_them() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::SCENE | Capabilities::POINTER))).unwrap();
    assert!(host.scene().is_some());
    assert!(host.semantics().is_empty());
}

#[test]
fn an_ungranted_kind_of_event_is_ignored() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::SCENE | Capabilities::ACCESSIBILITY))).unwrap();
    let before = host.scene().unwrap().seq;
    let id = host.semantics().iter().find(|node| node.role == Role::Button).unwrap().id;
    let bounds = host.semantics().iter().find(|node| node.id == id).unwrap().bounds;
    turn(&mut guest, &mut host, &click(bounds.0 + 4.0, bounds.1 + 4.0));
    assert_eq!(host.scene().unwrap().seq, before, "pointer was not granted");
}

#[test]
fn a_resize_lays_the_scene_out_again() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::KNOWN))).unwrap();
    let dark = Viewport { width: 260.0, height: 100.0, scale: 1.0, scheme: Scheme::Dark };
    turn(&mut guest, &mut host, &[ToGuest::Resize(dark)]);
    let scene = host.scene().unwrap();
    assert_eq!((scene.size.width, scene.size.height, scene.scale), (260.0, 100.0, 1.0));
    assert!(host.paint().unwrap() == native(counter, dark, &[])[0]);
}

#[test]
fn malformed_input_is_answered_with_a_fault_not_a_crash() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::KNOWN))).unwrap();
    let mut bad = protocol::encode(&Message::ToGuest(ToGuest::Tick { dt_ms: 1.0 }));
    bad[0..4].copy_from_slice(&3u32.to_le_bytes());
    bad.truncate(7);
    host.receive(&guest.call(&bad)).unwrap();
    assert!(host.faults().iter().any(|fault| fault.contains("ended")), "{:?}", host.faults());
}

#[test]
fn a_guest_answer_with_nothing_new_is_empty() {
    let mut guest = Guest::new(counter());
    let mut host = TestHost::new();
    host.receive(&guest.call(&offer(Capabilities::KNOWN))).unwrap();
    assert!(guest.call(&[]).is_empty(), "nothing changed, nothing is said");
    // a kind from a newer host is skipped, and says nothing either
    let mut newer = Vec::new();
    newer.extend_from_slice(&2u32.to_le_bytes());
    newer.extend_from_slice(&[0x7f, 0]);
    assert!(guest.call(&newer).is_empty());
}

#[test]
fn the_guest_never_sends_a_message_meant_for_it() {
    let mut guest = Guest::new(counter());
    let out = guest.call(&offer(Capabilities::KNOWN));
    let mut reader = protocol::Reader::new();
    reader.feed(&out);
    while let Some(received) = reader.pull().unwrap() {
        assert!(matches!(received, protocol::Received::Message(Message::ToHost(ToHost::Accept { .. } | ToHost::Scene(_) | ToHost::Semantics { .. } | ToHost::Pace { .. } | ToHost::Resource(_)))), "{received:?}");
    }
}
