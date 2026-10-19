//! Linux retained-window API witness under a real X11 server or Wayland compositor.
//! The API request here is not an observed compositor-generated close event.
#[cfg(target_os = "linux")]
use bunny_ui_linux as shell;
#[cfg(target_os = "linux")]
#[path = "../../bunny_ui/tests/support/window_lifecycle.rs"]
mod contract;
#[cfg(target_os = "linux")]
fn main() {
    if std::env::var_os("BUNNY_WINDOW_LIFECYCLE_PROBE").is_none() {
        println!(
            "Native window lifecycle requires BUNNY_WINDOW_LIFECYCLE_PROBE=1 and a native display."
        );
        return;
    }
    contract::run(|app, id, _title| {
        app.request_close(id).unwrap();
    });
}
#[cfg(not(target_os = "linux"))]
fn main() {}
