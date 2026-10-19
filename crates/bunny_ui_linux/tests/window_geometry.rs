//! Native content geometry contract; this is not physical presentation evidence.
#[cfg(target_os = "linux")]
use bunny_ui_linux as shell;
#[cfg(target_os = "linux")]
#[path = "../../bunny_ui/tests/support/window_geometry.rs"]
mod contract;
#[cfg(target_os = "linux")]
fn main() {
    if std::env::var_os("BUNNY_WINDOW_GEOMETRY_PROBE").is_none() {
        println!("Native geometry requires BUNNY_WINDOW_GEOMETRY_PROBE=1 and a display; not run.");
        return;
    }
    contract::main();
}
#[cfg(not(target_os = "linux"))]
fn main() {}
