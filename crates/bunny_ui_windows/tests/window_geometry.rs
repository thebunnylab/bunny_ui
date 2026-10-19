//! Native content geometry contract; this is not physical presentation evidence.
#[cfg(target_os = "windows")]
use bunny_ui_windows as shell;
#[cfg(target_os = "windows")]
#[path = "../../bunny_ui/tests/support/window_geometry.rs"]
mod contract;
#[cfg(target_os = "windows")]
fn main() {
    contract::main();
}
#[cfg(not(target_os = "windows"))]
fn main() {}
