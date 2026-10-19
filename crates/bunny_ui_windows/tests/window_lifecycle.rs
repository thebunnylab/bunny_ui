//! Win32 close-request and retained-window protocol witness.
#[cfg(target_os = "windows")]
use bunny_ui_windows as shell;
#[cfg(target_os = "windows")]
#[path = "../../bunny_ui/tests/support/window_lifecycle.rs"]
mod contract;
#[cfg(target_os = "windows")]
fn main() {
    if std::env::var("BUNNY_LIFECYCLE_CHILD").is_ok_and(|mode| mode == "children") {
        contract::child_surfaces(visible_children);
    } else {
        contract::run(native_close);
        if std::env::var_os("BUNNY_LIFECYCLE_CHILD").is_none() {
            contract::run_child("children");
        }
    }
}
#[cfg(not(target_os = "windows"))]
fn main() {}

#[cfg(target_os = "windows")]
fn native_close(_app: &shell::App, _id: shell::WindowId, title: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn SendMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> isize;
        fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
    }
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    unsafe {
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        assert_ne!(window, 0, "owned native window");
        let mut process = 0;
        GetWindowThreadProcessId(window, &raw mut process);
        assert_eq!(
            process,
            std::process::id(),
            "only address this probe's window"
        );
        SendMessageW(window, 0x0010, 0, 0); // WM_CLOSE
    }
}

#[cfg(target_os = "windows")]
fn visible_children(title: &str) -> usize {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn IsWindowVisible(window: isize) -> i32;
        fn GetWindow(window: isize, command: u32) -> isize;
        fn EnumWindows(
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            data: isize,
        ) -> i32;
    }
    struct Count {
        parent: isize,
        visible: usize,
    }
    unsafe extern "system" fn visit(window: isize, data: isize) -> i32 {
        unsafe {
            let state = &mut *(data as *mut Count);
            let mut owner = GetWindow(window, 4); // GW_OWNER
            while owner != 0 {
                if owner == state.parent {
                    state.visible += usize::from(IsWindowVisible(window) != 0);
                    break;
                }
                owner = GetWindow(owner, 4);
            }
        }
        1
    }
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    unsafe {
        let parent = FindWindowW(std::ptr::null(), title.as_ptr());
        assert_ne!(parent, 0, "owned native parent");
        let mut state = Count { parent, visible: 0 };
        assert_ne!(EnumWindows(visit, (&raw mut state) as isize), 0);
        state.visible
    }
}
