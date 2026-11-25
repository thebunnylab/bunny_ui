//! The answers the hello block gave from INSIDE its sandbox — the
//! `wasm32` build, run with no imports by `scripts/measure-hello-block.mjs`
//! — painted by the test host and held to the native render.
//!
//! ```sh
//! cargo build -p hello-block --target wasm32-unknown-unknown --profile web
//! node scripts/measure-hello-block.mjs target/wasm32-unknown-unknown/web/hello_block.wasm /tmp/answers
//! HELLO_BLOCK_ANSWERS=/tmp/answers cargo test -p hello-block --test sandboxed -- --ignored
//! ```
//!
//! Ignored by default: it needs the wasm build and node, which the
//! workspace's test run does not.

use bunny_ui_shell::host::{TestHost, native};
use bunny_ui_shell::protocol::{Modifiers, PointerButton, PointerPhase, Role, Scheme, ToGuest, Viewport};
use hello_block::hello;

#[test]
#[ignore = "needs the wasm build and its answers (HELLO_BLOCK_ANSWERS)"]
fn the_sandboxed_block_paints_as_the_native_one() {
    let dir = std::path::PathBuf::from(std::env::var("HELLO_BLOCK_ANSWERS").expect("HELLO_BLOCK_ANSWERS"));
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"));
    // the viewport the measurement offers
    let viewport = Viewport { width: 320.0, height: 140.0, scale: 2.0, scheme: Scheme::Light };

    let mut host = TestHost::new();
    host.receive(&read("handshake.bin")).unwrap();
    let session = host.session().expect("accepted");
    assert_eq!(session.version, 1);
    assert!(session.engine.starts_with("bunny-ui "));
    let painted = host.paint().unwrap();
    assert!(painted == native(hello, viewport, &[])[0], "the sandboxed scene differs from the native render");

    let button = host.semantics().iter().find(|node| node.role == Role::Button).unwrap().bounds;
    let (x, y) = (button.0 + button.2 / 2.0, button.1 + button.3 / 2.0);
    let press = |phase| ToGuest::Pointer { phase, x, y, button: PointerButton::Primary, clicks: 1, modifiers: Modifiers::NONE };
    host.receive(&read("press.bin")).unwrap();
    assert!(host.semantics().iter().any(|node| node.label == "Pressed 1 times"));
    let expected = native(hello, viewport, &[press(PointerPhase::Down), press(PointerPhase::Up)]);
    assert!(host.paint().unwrap() == expected[1], "after the press the sandboxed scene differs");
}
