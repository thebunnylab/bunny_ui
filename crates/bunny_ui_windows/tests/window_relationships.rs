//! Win32 owner-property and public ownership contract witness.
#[cfg(target_os = "windows")]
use bunny_ui_windows as shell;
#[cfg(target_os = "windows")]
#[path = "../../bunny_ui/tests/support/window_relationships.rs"]
mod contract;
#[cfg(target_os = "windows")]
fn parent(child: &str, expected: Option<&str>) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn GetWindow(window: isize, command: u32) -> isize;
        fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
    }
    let owned = |title: &str| unsafe {
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        assert_ne!(window, 0);
        let mut process = 0;
        GetWindowThreadProcessId(window, &raw mut process);
        assert_eq!(process, std::process::id());
        window
    };
    assert_eq!(
        unsafe { GetWindow(owned(child), 4) },
        expected.map_or(0, owned)
    );
}
#[cfg(target_os = "windows")]
fn main() {
    contract::run(parent);
}
#[cfg(not(target_os = "windows"))]
fn main() {}
