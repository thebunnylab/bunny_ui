//! Native dialog geometry witness, run on the native event thread.
#[cfg(target_os = "linux")]
use bunny_ui_linux as shell;
#[cfg(target_os = "linux")]
#[path = "../../bunny_ui/tests/support/dialog_geometry.rs"]
mod contract;
#[cfg(target_os = "linux")]
fn main() {
    if std::env::var_os("BUNNY_DIALOG_GEOMETRY_PROBE").is_none() {
        println!(
            "Native dialog geometry requires BUNNY_DIALOG_GEOMETRY_PROBE=1 and a display; not run."
        );
        return;
    }
    contract::main();
}
#[cfg(not(target_os = "linux"))]
fn main() {}
