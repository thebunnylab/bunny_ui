//! Native dialog geometry witness, run on AppKit's main thread.
#[cfg(target_os = "macos")]
use bunny_ui_macos as shell;
#[cfg(target_os = "macos")]
#[path = "../../bunny_ui/tests/support/dialog_geometry.rs"]
mod contract;
#[cfg(target_os = "macos")]
fn main() {
    contract::main();
}
#[cfg(not(target_os = "macos"))]
fn main() {}
