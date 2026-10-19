//! Public ownership requests under a native X11 server or Wayland compositor.
#[cfg(target_os = "linux")]
use bunny_ui_linux as shell;
#[cfg(target_os = "linux")]
#[path = "../../bunny_ui/tests/support/window_relationships.rs"]
mod contract;
#[cfg(target_os = "linux")]
fn main() {
    if std::env::var_os("BUNNY_WINDOW_RELATIONSHIPS_PROBE").is_none() {
        println!(
            "Native ownership requires BUNNY_WINDOW_RELATIONSHIPS_PROBE=1 and a display; not run."
        );
        return;
    }
    contract::run(|child, parent| {
        println!(
            "Native parent request: {child} -> {parent:?}; compositor behavior is not qualified"
        )
    });
}
#[cfg(not(target_os = "linux"))]
fn main() {}
