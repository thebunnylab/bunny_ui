//! A real UIA client queries our own window from a separate MTA thread.
//! This native protocol witness does not replace a human NVDA workflow.
#[cfg(target_os = "windows")]
mod probe {
    use bunny_ui::prelude::*;
    use bunny_ui_windows::{App, WindowSpec};
    use std::{ffi::c_void, ptr, rc::Rc};

    type Object = *mut c_void;
    type Hresult = i32;
    #[repr(C)]
    struct Guid {
        d1: u32,
        d2: u16,
        d3: u16,
        d4: [u8; 8],
    }
    const CLIENT_CLASS: Guid = Guid {
        d1: 0xff48dba4,
        d2: 0x60ef,
        d3: 0x4201,
        d4: [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e],
    };
    const CLIENT_IID: Guid = Guid {
        d1: 0x30cbe57d,
        d2: 0xd9d0,
        d3: 0x452a,
        d4: [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee],
    };
    #[repr(C)]
    struct Unknown {
        query: unsafe extern "system" fn(Object, *const Guid, *mut Object) -> Hresult,
        retain: unsafe extern "system" fn(Object) -> u32,
        release: unsafe extern "system" fn(Object) -> u32,
    }
    // UIAutomationClient.h: the prefix through CreatePropertyCondition.
    #[repr(C)]
    struct Client {
        unknown: Unknown,
        compare_elements: usize,
        compare_runtime_ids: usize,
        get_root: usize,
        from_handle: unsafe extern "system" fn(Object, isize, *mut Object) -> Hresult,
        // ElementFromPoint through CreateFalseCondition, slots 7..22.
        unused: [usize; 16],
        property_condition: unsafe extern "system" fn(Object, i32, Variant, *mut Object) -> Hresult,
    }
    #[repr(C)]
    struct Element {
        unknown: Unknown,
        focus: unsafe extern "system" fn(Object) -> Hresult,
        runtime_id: usize,
        find_first: unsafe extern "system" fn(Object, i32, Object, *mut Object) -> Hresult,
        // FindAll through BuildUpdatedCache, slots 6..9.
        unused: [usize; 4],
        property: unsafe extern "system" fn(Object, i32, *mut Variant) -> Hresult,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Variant {
        kind: u16,
        reserved: [u16; 3],
        data: [usize; 2],
    }
    impl Variant {
        const fn empty() -> Self {
            Self {
                kind: 0,
                reserved: [0; 3],
                data: [0; 2],
            }
        }
    }
    #[link(name = "ole32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: Object, mode: u32) -> Hresult;
        fn CoUninitialize();
        fn CoCreateInstance(
            class: *const Guid,
            outer: Object,
            context: u32,
            iid: *const Guid,
            out: *mut Object,
        ) -> Hresult;
    }
    #[link(name = "oleaut32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn SysAllocStringLen(text: *const u16, length: u32) -> *mut u16;
        fn SysStringLen(text: *const u16) -> u32;
        fn VariantClear(value: *mut Variant) -> Hresult;
    }
    #[link(name = "user32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn PostMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> i32;
    }
    struct Owned(Object);
    impl Owned {
        unsafe fn table<T>(&self) -> &T {
            unsafe { &**(self.0 as *const *const T) }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                (self.table::<Unknown>().release)(self.0);
            }
        }
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    fn succeeded(result: Hresult) {
        assert!(result >= 0, "UIA HRESULT {result:#x}");
    }
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }
    unsafe fn read_string(element: &Owned, property: i32) -> String {
        let mut value = Variant::empty();
        unsafe {
            succeeded((element.table::<Element>().property)(
                element.0, property, &mut value,
            ));
            assert_eq!(value.kind, 8, "expected a UIA BSTR property");
            let pointer = value.data[0] as *const u16;
            let result = if pointer.is_null() {
                String::new()
            } else {
                String::from_utf16_lossy(std::slice::from_raw_parts(
                    pointer,
                    SysStringLen(pointer) as usize,
                ))
            };
            succeeded(VariantClear(&mut value));
            result
        }
    }
    unsafe fn client(window: isize) {
        unsafe {
            succeeded(CoInitializeEx(ptr::null_mut(), 0)); // COINIT_MULTITHREADED
            let _apartment = Apartment;
            let mut raw = ptr::null_mut();
            succeeded(CoCreateInstance(
                &CLIENT_CLASS,
                ptr::null_mut(),
                1,
                &CLIENT_IID,
                &mut raw,
            ));
            let client = Owned(raw);
            let mut raw = ptr::null_mut();
            succeeded((client.table::<Client>().from_handle)(
                client.0, window, &mut raw,
            ));
            assert!(!raw.is_null());
            let root = Owned(raw);
            let name = wide("Description");
            let mut value = Variant {
                kind: 8,
                reserved: [0; 3],
                data: [
                    SysAllocStringLen(name.as_ptr(), (name.len() - 1) as u32) as usize,
                    0,
                ],
            };
            assert_ne!(value.data[0], 0);
            let mut raw = ptr::null_mut();
            succeeded((client.table::<Client>().property_condition)(
                client.0, 30005, value, &mut raw,
            ));
            succeeded(VariantClear(&mut value));
            let condition = Owned(raw);
            let mut raw = ptr::null_mut();
            succeeded((root.table::<Element>().find_first)(
                root.0,
                4,
                condition.0,
                &mut raw,
            )); // TreeScope_Descendants
            assert!(
                !raw.is_null(),
                "UIA client cannot find the retained Description field"
            );
            let field = Owned(raw);
            assert_eq!(read_string(&field, 30005), "Description");
            assert_eq!(read_string(&field, 30045), "Lunch");
            println!("UIA native client: Description=Lunch");
        }
    }
    pub fn run() {
        let app = App::new();
        let value = State::new("Lunch".to_string());
        let title = format!("Bunny UIA witness {}", std::process::id());
        app.open(
            WindowSpec::titled(title.clone()).size(480.0, 480.0),
            Rc::new(app.runtime()),
            vstack!(
                text("Native UIA witness"),
                text_field("Description", value.binding())
            )
            .padding(),
        );
        let window = unsafe { FindWindowW(ptr::null(), wide(&title).as_ptr()) };
        assert_ne!(window, 0);
        // A deadlocked provider must fail the probe, not occupy the CI runner.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(60));
            eprintln!("UIA native probe timed out");
            std::process::exit(1);
        });
        let worker = std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| unsafe { client(window) });
            unsafe {
                PostMessageW(window, 0x0010, 0, 0);
            } // WM_CLOSE, our own window
            result
        });
        app.run();
        if let Err(reason) = worker.join().unwrap() {
            std::panic::resume_unwind(reason);
        }
    }
}
#[cfg(target_os = "windows")]
fn main() {
    probe::run();
}
#[cfg(not(target_os = "windows"))]
fn main() {}
