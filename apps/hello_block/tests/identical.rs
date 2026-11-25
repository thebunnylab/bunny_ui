//! The acceptance of T2-APP-60: the hello block, compiled for the
//! Trinity shell, renders in a test host identically to its native
//! render — before and after a press — and its protocol version is
//! negotiated.

use bunny_ui_shell::guest::Guest;
use bunny_ui_shell::host::{TestHost, native};
use bunny_ui_shell::protocol::{
    self, Capabilities, Message, Modifiers, PointerButton, PointerPhase, Role, Scheme, ToGuest, Versions, Viewport,
};
use hello_block::hello;

fn send(guest: &mut Guest<hello_block::Hello>, host: &mut TestHost, messages: &[ToGuest]) {
    let mut bytes = Vec::new();
    for message in messages {
        bytes.extend(protocol::encode(&Message::ToGuest(message.clone())));
    }
    host.receive(&guest.call(&bytes)).expect("the block speaks the protocol");
}

#[test]
fn the_hello_block_renders_as_it_does_natively() {
    for viewport in [
        Viewport { width: 320.0, height: 140.0, scale: 2.0, scheme: Scheme::Light },
        Viewport { width: 280.0, height: 160.0, scale: 1.0, scheme: Scheme::Dark },
    ] {
        let mut guest = Guest::new(hello());
        let mut host = TestHost::new();
        send(&mut guest, &mut host, &[ToGuest::Offer {
            versions: Versions { oldest: 1, newest: 7 },
            capabilities: Capabilities::KNOWN,
            viewport,
        }]);
        assert_eq!(host.session().map(|session| session.version), Some(1), "negotiated down to the version both speak");

        let painted = host.paint().unwrap();
        let expected = native(hello, viewport, &[]);
        assert!(painted.chunks(4).any(|pixel| pixel != &painted[..4]));
        assert!(painted == expected[0], "{viewport:?}: the block's scene differs from the native render");

        let button = host.semantics().iter().find(|node| node.role == Role::Button).expect("the button").bounds;
        let (x, y) = (button.0 + button.2 / 2.0, button.1 + button.3 / 2.0);
        let press = |phase| ToGuest::Pointer {
            phase,
            x,
            y,
            button: PointerButton::Primary,
            clicks: 1,
            modifiers: Modifiers::NONE,
        };
        let click = [press(PointerPhase::Down), press(PointerPhase::Up)];
        send(&mut guest, &mut host, &click);
        assert!(host.semantics().iter().any(|node| node.label == "Pressed 1 times"));
        let expected = native(hello, viewport, &click);
        assert!(expected[0] != expected[1]);
        assert!(host.paint().unwrap() == expected[1], "{viewport:?}: after the press the scenes differ");
    }
}
