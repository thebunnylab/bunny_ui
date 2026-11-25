//! The acceptance of T2-APP-64: a block written for bunny-ui 0.5, carried
//! forward by `bunny migrate`, builds against the framework in this tree
//! and runs behind the shell protocol.
//!
//! The source is the migrated file itself, the one `bunny-cli`'s
//! `tests/migrate.rs` checks the rules produce byte for byte from
//! `tests/migrate/0.5/places.rs`: if a rule or the framework drifts, one
//! of the two tests says so.

#[path = "../../../crates/bunny_cli/tests/migrate/0.6/places.rs"]
mod places;

use bunny_ui_shell::guest::Guest;
use bunny_ui_shell::host::TestHost;
use bunny_ui_shell::protocol::{
    self, Capabilities, Message, Modifiers, PointerButton, PointerPhase, Role, Scheme, ToGuest, Versions, Viewport,
};

fn send(guest: &mut Guest<places::Places>, host: &mut TestHost, messages: &[ToGuest]) {
    let mut bytes = Vec::new();
    for message in messages {
        bytes.extend(protocol::encode(&Message::ToGuest(message.clone())));
    }
    host.receive(&guest.call(&bytes)).expect("the block speaks the protocol");
}

#[test]
fn the_migrated_block_builds_and_runs_in_the_shell() {
    let viewport = Viewport { width: 360.0, height: 240.0, scale: 1.0, scheme: Scheme::Light };
    let mut guest = Guest::new(places::places());
    let mut host = TestHost::new();
    send(&mut guest, &mut host, &[ToGuest::Offer {
        versions: Versions { oldest: 1, newest: 1 },
        capabilities: Capabilities::KNOWN,
        viewport,
    }]);
    assert!(host.session().is_some(), "{:?}", host.refusal());
    assert!(host.faults().is_empty(), "{:?}", host.faults());
    let labels: Vec<&str> = host.semantics().iter().map(|node| node.label.as_str()).collect();
    for label in ["Lisbon", "Places"] {
        assert!(labels.iter().any(|shown| shown.contains(label)), "{label} in {labels:?}");
    }
    assert!(labels.iter().any(|shown| shown.contains("0 visits")), "{labels:?}");

    // the bar's own item, which 0.5 gave `.toolbar` as a bare view
    let visit = host
        .semantics()
        .iter()
        .find(|node| node.role == Role::Button && node.label == "Visit")
        .expect("the bar's Visit button")
        .bounds;
    let (x, y) = (visit.0 + visit.2 / 2.0, visit.1 + visit.3 / 2.0);
    let press = |phase| ToGuest::Pointer { phase, x, y, button: PointerButton::Primary, clicks: 1, modifiers: Modifiers::NONE };
    send(&mut guest, &mut host, &[press(PointerPhase::Down), press(PointerPhase::Up)]);
    assert!(host.semantics().iter().any(|node| node.label.contains("1 visits")), "{:?}", host.semantics());
}
