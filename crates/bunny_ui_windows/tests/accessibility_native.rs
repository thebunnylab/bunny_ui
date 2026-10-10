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
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Guid {
        d1: u32,
        d2: u16,
        d3: u16,
        d4: [u8; 8],
    }
    const VALUE_IID: Guid = Guid {
        d1: 0xa94cd8b1,
        d2: 0x0844,
        d3: 0x4cd6,
        d4: [0x9d, 0x2d, 0x64, 0x05, 0x37, 0xab, 0x39, 0xe9],
    };
    const INVOKE_IID: Guid = Guid {
        d1: 0xfb377fbe,
        d2: 0x8ea6,
        d3: 0x46d5,
        d4: [0x9c, 0x73, 0x64, 0x99, 0x64, 0x2d, 0x30, 0x59],
    };
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
        from_point: unsafe extern "system" fn(Object, NativePoint, *mut Object) -> Hresult,
        focused: unsafe extern "system" fn(Object, *mut Object) -> Hresult,
        unused: [usize; 14],
        property_condition: unsafe extern "system" fn(Object, i32, Variant, *mut Object) -> Hresult,
        // CreatePropertyConditionEx through RemoveAutomationEventHandler, slots 24..33.
        unused_conditions_events: [usize; 10],
        add_properties: unsafe extern "system" fn(
            Object,
            Object,
            i32,
            Object,
            Object,
            *const i32,
            i32,
        ) -> Hresult,
        add_properties_array: usize,
        remove_properties: unsafe extern "system" fn(Object, Object, Object) -> Hresult,
    }
    #[repr(C)]
    struct Element {
        unknown: Unknown,
        focus: unsafe extern "system" fn(Object) -> Hresult,
        runtime_id: unsafe extern "system" fn(Object, *mut Object) -> Hresult,
        find_first: unsafe extern "system" fn(Object, i32, Object, *mut Object) -> Hresult,
        // FindAll through BuildUpdatedCache, slots 6..9.
        unused: [usize; 4],
        property: unsafe extern "system" fn(Object, i32, *mut Variant) -> Hresult,
        unused_properties: [usize; 3],
        pattern: unsafe extern "system" fn(Object, i32, *const Guid, *mut Object) -> Hresult,
    }
    #[repr(C)]
    struct ValuePattern {
        unknown: Unknown,
        set: unsafe extern "system" fn(Object, *const u16) -> Hresult,
        get: unsafe extern "system" fn(Object, *mut *mut u16) -> Hresult,
    }
    #[repr(C)]
    struct InvokePattern {
        unknown: Unknown,
        invoke: unsafe extern "system" fn(Object) -> Hresult,
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
        fn SysFreeString(text: *mut u16);
        fn SafeArrayGetLBound(array: Object, dimension: u32, bound: *mut i32) -> Hresult;
        fn SafeArrayGetUBound(array: Object, dimension: u32, bound: *mut i32) -> Hresult;
        fn SafeArrayGetElement(array: Object, index: *const i32, out: Object) -> Hresult;
        fn SafeArrayDestroy(array: Object) -> Hresult;
        fn SysStringLen(text: *const u16) -> u32;
        fn VariantClear(value: *mut Variant) -> Hresult;
    }
    #[link(name = "user32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn CreateWindowExW(
            extended: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: isize,
            menu: isize,
            instance: isize,
            parameters: Object,
        ) -> isize;
        fn IsWindow(window: isize) -> i32;
        fn GetWindowRect(window: isize, rect: *mut NativeRect) -> i32;
        fn SetWindowPos(
            window: isize,
            after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
        fn GetWindowThreadProcessId(window: isize, process: *mut u32) -> u32;
        fn EnumThreadWindows(
            thread: u32,
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            data: isize,
        ) -> i32;
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
    type Received = std::sync::Arc<std::sync::Mutex<Vec<(i32, String)>>>;
    #[repr(C)]
    struct EventTable {
        unknown: Unknown,
        property: unsafe extern "system" fn(Object, Object, i32, Variant) -> Hresult,
    }
    #[repr(C)]
    struct EventSink {
        table: *const EventTable,
        refs: std::sync::atomic::AtomicU32,
        received: Received,
    }
    unsafe extern "system" fn event_query(
        this: Object,
        iid: *const Guid,
        out: *mut Object,
    ) -> Hresult {
        if out.is_null() {
            return 0x80004003u32 as i32;
        }
        unsafe {
            *out = ptr::null_mut();
        }
        if iid.is_null() {
            return 0x80004003u32 as i32;
        }
        let unknown = Guid {
            d1: 0,
            d2: 0,
            d3: 0,
            d4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
        };
        let handler = Guid {
            d1: 0x40cd37d4,
            d2: 0xc756,
            d3: 0x4b0c,
            d4: [0x8c, 0x6f, 0xbd, 0xdf, 0xee, 0xb1, 0x3b, 0x50],
        };
        if unsafe { *iid } != unknown && unsafe { *iid } != handler {
            return 0x80004002u32 as i32;
        }
        unsafe {
            event_retain(this);
            *out = this;
        }
        0
    }
    unsafe extern "system" fn event_retain(this: Object) -> u32 {
        unsafe { &*(this as *const EventSink) }
            .refs
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }
    unsafe extern "system" fn event_release(this: Object) -> u32 {
        let old = unsafe { &*(this as *const EventSink) }
            .refs
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
        if old == 1 {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            unsafe {
                drop(Box::from_raw(this as *mut EventSink));
            }
        }
        old - 1
    }
    unsafe extern "system" fn event_property(
        this: Object,
        _sender: Object,
        property: i32,
        value: Variant,
    ) -> Hresult {
        let text = if value.kind == 8 && value.data[0] != 0 {
            unsafe {
                String::from_utf16_lossy(std::slice::from_raw_parts(
                    value.data[0] as *const u16,
                    SysStringLen(value.data[0] as *const u16) as usize,
                ))
            }
        } else {
            String::new()
        };
        let sink = unsafe { &*(this as *const EventSink) };
        match sink.received.lock() {
            Ok(mut events) => {
                events.push((property, text));
                0
            }
            Err(_) => 0x80004005u32 as i32,
        }
    }
    static EVENT_TABLE: EventTable = EventTable {
        unknown: Unknown {
            query: event_query,
            retain: event_retain,
            release: event_release,
        },
        property: event_property,
    };
    struct Events<'a> {
        client: &'a Owned,
        element: &'a Owned,
        sink: Owned,
        received: Received,
    }
    impl<'a> Events<'a> {
        unsafe fn subscribe(client: &'a Owned, element: &'a Owned) -> Self {
            let received = Received::default();
            let sink = Owned(
                Box::into_raw(Box::new(EventSink {
                    table: &EVENT_TABLE,
                    refs: std::sync::atomic::AtomicU32::new(1),
                    received: std::sync::Arc::clone(&received),
                }))
                .cast(),
            );
            let properties = [30005, 30045, 30008];
            unsafe {
                succeeded((client.table::<Client>().add_properties)(
                    client.0,
                    element.0,
                    1,
                    ptr::null_mut(),
                    sink.0,
                    properties.as_ptr(),
                    properties.len() as i32,
                ));
            }
            Self {
                client,
                element,
                sink,
                received,
            }
        }
        fn saw(&self, property: i32, value: &str) -> bool {
            self.received
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.0 == property && event.1 == value)
        }
    }
    impl Drop for Events<'_> {
        fn drop(&mut self) {
            unsafe {
                (self.client.table::<Client>().remove_properties)(
                    self.client.0,
                    self.element.0,
                    self.sink.0,
                );
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
    #[repr(C)]
    #[derive(Default)]
    struct NativeRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    struct NativePoint {
        x: i32,
        y: i32,
    }
    unsafe fn read_integer(element: &Owned, property: i32, kind: u16) -> i32 {
        let mut value = Variant::empty();
        unsafe {
            succeeded((element.table::<Element>().property)(
                element.0, property, &mut value,
            ));
            assert_eq!(value.kind, kind);
            let integer = if kind == 11 {
                value.data[0] as i16 as i32
            } else {
                value.data[0] as i32
            };
            succeeded(VariantClear(&mut value));
            integer
        }
    }
    unsafe fn array_values<T: Default>(array: Object) -> Vec<T> {
        assert!(!array.is_null());
        let (mut lower, mut upper) = (0, 0);
        unsafe {
            succeeded(SafeArrayGetLBound(array, 1, &mut lower));
            succeeded(SafeArrayGetUBound(array, 1, &mut upper));
            (lower..=upper)
                .map(|index| {
                    let mut value = T::default();
                    succeeded(SafeArrayGetElement(
                        array,
                        &index,
                        ptr::from_mut(&mut value).cast(),
                    ));
                    value
                })
                .collect()
        }
    }
    unsafe fn identity(element: &Owned) -> Vec<i32> {
        let mut array = ptr::null_mut();
        unsafe {
            succeeded((element.table::<Element>().runtime_id)(
                element.0, &mut array,
            ));
            let values = array_values(array);
            succeeded(SafeArrayDestroy(array));
            values
        }
    }
    unsafe fn bounds(element: &Owned) -> Vec<f64> {
        let mut value = Variant::empty();
        unsafe {
            succeeded((element.table::<Element>().property)(
                element.0, 30001, &mut value,
            ));
            assert_eq!(value.kind, 0x2000 | 5);
            let bounds = array_values(value.data[0] as Object);
            succeeded(VariantClear(&mut value));
            bounds
        }
    }
    unsafe fn find(client: &Owned, root: &Owned, name: &str) -> Option<Owned> {
        unsafe {
            let words = wide(name);
            let mut value = Variant {
                kind: 8,
                reserved: [0; 3],
                data: [
                    SysAllocStringLen(words.as_ptr(), (words.len() - 1) as u32) as usize,
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
            ));
            (!raw.is_null()).then(|| Owned(raw))
        }
    }
    unsafe fn named(client: &Owned, root: &Owned, name: &str) -> Owned {
        unsafe { find(client, root, name) }
            .unwrap_or_else(|| panic!("UIA client cannot find the retained {name} field"))
    }
    unsafe fn root(client: &Owned, window: isize) -> Owned {
        let mut raw = ptr::null_mut();
        unsafe {
            succeeded((client.table::<Client>().from_handle)(
                client.0, window, &mut raw,
            ));
        }
        assert!(!raw.is_null());
        Owned(raw)
    }
    unsafe fn pattern(element: &Owned, id: i32, iid: &Guid) -> Owned {
        let mut raw = ptr::null_mut();
        unsafe {
            succeeded((element.table::<Element>().pattern)(
                element.0, id, iid, &mut raw,
            ));
        }
        assert!(!raw.is_null(), "missing UIA pattern {id}");
        Owned(raw)
    }
    unsafe fn invoke(element: &Owned) {
        unsafe {
            let pattern = pattern(element, 10000, &INVOKE_IID);
            succeeded((pattern.table::<InvokePattern>().invoke)(pattern.0));
        }
    }
    fn until(label: &str, mut check: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if check() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "UIA request did not settle: {label}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    unsafe extern "system" fn collect(window: isize, data: isize) -> i32 {
        unsafe {
            (&mut *(data as *mut Vec<isize>)).push(window);
        }
        1
    }
    unsafe fn modal(client: &Owned, window: isize) -> Option<Owned> {
        unsafe {
            let mut windows = Vec::<isize>::new();
            EnumThreadWindows(
                GetWindowThreadProcessId(window, ptr::null_mut()),
                collect,
                ptr::from_mut(&mut windows) as isize,
            );
            for candidate in windows {
                if candidate != window
                    && let Some(button) = find(client, &root(client, candidate), "Dismiss modal")
                {
                    return Some(button);
                }
            }
            None
        }
    }
    unsafe fn client(window: isize, first_modal: bool) {
        unsafe {
            succeeded(CoInitializeEx(ptr::null_mut(), 0));
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
            if first_modal {
                let dismiss =
                    modal(&client, window).expect("first UIA query reaches modal controls");
                invoke(&dismiss);
                until("first modal dismissed", || {
                    find(&client, &root(&client, window), "Description").is_some()
                });
                println!("UIA native client: modal available on first query");
                return;
            }
            let root = root(&client, window);
            let field = named(&client, &root, "Description");
            assert_eq!(read_string(&field, 30045), "Lunch");
            assert_eq!(read_integer(&field, 30003, 3), 50004);
            let events = Events::subscribe(&client, &field);
            let stable = identity(&field);
            assert!(!stable.is_empty());
            let text = named(&client, &root, "Native UIA witness");
            assert_eq!(read_integer(&text, 30003, 3), 50020);
            let initial = bounds(&field);
            println!(
                "UIA before: name={:?}, value={:?}, runtime={stable:?}, bounds={initial:?}",
                read_string(&field, 30005),
                read_string(&field, 30045)
            );
            assert_eq!(initial.len(), 4);
            assert!(initial[2] > 0.0 && initial[3] > 0.0);
            let mut old = NativeRect::default();
            assert_ne!(GetWindowRect(window, &mut old), 0);
            assert_ne!(
                SetWindowPos(
                    window,
                    0,
                    old.left + 10,
                    old.top + 10,
                    0,
                    0,
                    0x0001 | 0x0004 | 0x0010
                ),
                0
            );
            let mut moved = NativeRect::default();
            assert_ne!(GetWindowRect(window, &mut moved), 0);
            let moved_field = bounds(&field);
            assert!((moved_field[0] - initial[0] - f64::from(moved.left - old.left)).abs() < 0.01);
            assert!((moved_field[1] - initial[1] - f64::from(moved.top - old.top)).abs() < 0.01);
            succeeded((field.table::<Element>().focus)(field.0));
            until("field focus", || read_integer(&field, 30008, 11) != 0);
            let value_pattern = pattern(&field, 10002, &VALUE_IID);
            let words = wide("Dinner 👩‍🚀");
            let string = SysAllocStringLen(words.as_ptr(), (words.len() - 1) as u32);
            succeeded((value_pattern.table::<ValuePattern>().set)(
                value_pattern.0,
                string,
            ));
            SysFreeString(string);
            until("field value", || read_string(&field, 30045) == "Dinner 👩‍🚀");
            let native = named(&client, &root, "Hosted control");
            assert_eq!(read_integer(&native, 30003, 3), 50000);
            let native_bounds = bounds(&native);
            let mut hit = ptr::null_mut();
            succeeded((client.table::<Client>().from_point)(
                client.0,
                NativePoint {
                    x: (native_bounds[0] + native_bounds[2] / 2.0) as i32,
                    y: (native_bounds[1] + native_bounds[3] / 2.0) as i32,
                },
                &mut hit,
            ));
            assert!(!hit.is_null());
            assert_eq!(
                read_string(&Owned(hit), 30005),
                "Hosted control",
                "native HWND hit testing remains reachable"
            );
            succeeded((native.table::<Element>().focus)(native.0));
            until("native child focus", || {
                let mut focused = ptr::null_mut();
                succeeded((client.table::<Client>().focused)(client.0, &mut focused));
                !focused.is_null() && read_string(&Owned(focused), 30005) == "Hosted control"
            });
            assert_eq!(
                read_integer(&field, 30008, 11),
                0,
                "Bunny field must yield focus to hosted HWND"
            );
            let password = named(&client, &root, "Password");
            assert_ne!(read_integer(&password, 30019, 11), 0);
            let secret = pattern(&password, 10002, &VALUE_IID);
            let mut raw = ptr::null_mut();
            let hr = (secret.table::<ValuePattern>().get)(secret.0, &mut raw);
            // UIA may normalize a refused Value property into an empty BSTR.
            // Test the security boundary at the client and the exact HRESULT
            // separately at the provider ABI; never allow nonempty output.
            let length = if raw.is_null() { 0 } else { SysStringLen(raw) };
            println!("UIA password read: HRESULT={hr:#x}, returned UTF-16 units={length}");
            assert_eq!(length, 0, "password must not be exported by UIA");
            SysFreeString(raw);
            let mut secret_property = Variant::empty();
            let hr =
                (password.table::<Element>().property)(password.0, 30045, &mut secret_property);
            if hr >= 0 && secret_property.kind == 8 {
                assert_eq!(
                    SysStringLen(secret_property.data[0] as *const u16),
                    0,
                    "secret must not be exported"
                );
            }
            succeeded(VariantClear(&mut secret_property));
            let save = named(&client, &root, "Save");
            assert_eq!(read_integer(&save, 30003, 3), 50000);
            invoke(&save);
            until("dynamic field name", || {
                read_string(&field, 30005) == "Updated name"
            });
            assert_eq!(
                identity(&named(&client, &root, "Updated name")),
                stable,
                "identity survives name and value changes"
            );
            println!(
                "UIA after edit/invoke: name={:?}, value={:?}, runtime={:?}, bounds={:?}",
                read_string(&field, 30005),
                read_string(&field, 30045),
                identity(&field),
                bounds(&field)
            );
            until("native value event", || events.saw(30045, "Dinner 👩‍🚀"));
            until("native name event", || events.saw(30005, "Updated name"));
            println!("UIA native property events: name and value changes delivered");
            println!("UIA phase: unsubscribe start");
            drop(events);
            println!("UIA phase: unsubscribed");
            println!("UIA phase: locate retiring row");
            let row = named(&client, &root, "Row 2");
            println!("UIA phase: retiring row located");
            let removed = pattern(&row, 10000, &INVOKE_IID);
            println!("UIA phase: remove-row invoke start");
            invoke(&named(&client, &root, "Remove row"));
            println!("UIA phase: remove-row invoke accepted");
            println!("UIA phase: wait row removal");
            until("row removed", || find(&client, &root, "Row 2").is_none());
            println!("UIA phase: row removed");
            assert!(
                (removed.table::<InvokePattern>().invoke)(removed.0) < 0,
                "retired provider must refuse invocation"
            );
            println!("UIA phase: open-modal invoke start");
            invoke(&named(&client, &root, "Open modal"));
            println!("UIA phase: open-modal invoke accepted");
            let mut dismiss = None;
            until("modal exposed", || {
                dismiss = modal(&client, window);
                dismiss.is_some()
            });
            assert!(
                find(&client, &root, "Updated name").is_none(),
                "modal excludes background fields"
            );
            println!("UIA phase: dismiss-modal invoke start");
            invoke(&dismiss.unwrap());
            println!("UIA phase: dismiss-modal invoke accepted");
            until("modal dismissed", || {
                find(&client, &root, "Updated name").is_some()
            });
            let current = named(&client, &root, "Updated name");
            let current_value = pattern(&current, 10002, &VALUE_IID);
            println!("UIA phase: window-close post");
            assert_ne!(PostMessageW(window, 0x0010, 0, 0), 0);
            println!("UIA phase: window-close posted");
            println!("UIA phase: wait window close");
            until("window closed", || IsWindow(window) == 0);
            println!("UIA phase: window closed");
            let mut raw = ptr::null_mut();
            assert!(
                (current_value.table::<ValuePattern>().get)(current_value.0, &mut raw) < 0,
                "closed provider must refuse reads"
            );
            assert!(raw.is_null());
            println!(
                "UIA native client: roles, value/focus/actions, dynamic names, stable IDs, password, moved bounds, modal exclusion, retired and closed providers verified"
            );
        }
    }
    #[derive(Clone, Copy)]
    struct Form {
        name: State<String>,
        value: State<String>,
        password: State<String>,
        presses: State<u32>,
        rows: State<Vec<u32>>,
        modal: State<bool>,
    }
    impl Component for Form {
        fn body(self) -> impl View {
            vstack!(
                text("Native UIA witness"),
                text_field("Description", self.value.binding()).accessibility_label(self.name),
                text_field("Password", self.password.binding()).secret(true),
                button(text("Save"), move || {
                    self.presses.add(1);
                    self.name.set("Updated name".into());
                }),
                button(text("Open modal"), move || self.modal.set(true)),
                button(text("Remove row"), move || self.rows.set(vec![1])),
                for_each(
                    self.rows,
                    |id| id.to_string(),
                    move |id| {
                        let id = *id;
                        button(text(format!("Row {id}")), move || self.presses.add(id))
                    }
                )
            )
            .padding()
            .sheet(self.modal.binding(), move |_| {
                erased(button(text("Dismiss modal"), move || self.modal.set(false)).padding())
            })
        }
    }
    pub fn run() {
        let mta_host = std::env::args().any(|arg| arg == "--mta-host");
        let _host_apartment = if mta_host {
            unsafe {
                succeeded(CoInitializeEx(ptr::null_mut(), 0));
            }
            Some(Apartment)
        } else {
            None
        };
        let app = App::new();
        let first_modal = std::env::args().any(|arg| arg == "--modal-first");
        let form = Form {
            name: State::new("Description".into()),
            value: State::new("Lunch".into()),
            password: State::new("never-export-this".into()),
            presses: State::new(0),
            rows: State::new(vec![1, 2]),
            modal: State::new(first_modal),
        };
        let keeper_title = format!("Bunny UIA keeper {}", std::process::id());
        app.open(
            WindowSpec::titled(keeper_title.clone()).size(160.0, 100.0),
            Rc::new(app.runtime()),
            text("UIA pump keeper"),
        );
        let keeper = unsafe { FindWindowW(ptr::null(), wide(&keeper_title).as_ptr()) };
        assert_ne!(keeper, 0);
        let title = format!("Bunny UIA witness {}", std::process::id());
        app.open(
            WindowSpec::titled(title.clone()).size(480.0, 640.0),
            Rc::new(app.runtime()),
            form,
        );
        let window = unsafe { FindWindowW(ptr::null(), wide(&title).as_ptr()) };
        assert_ne!(window, 0);
        if !first_modal {
            // A real HWND child is independently supplied by Windows. Our
            // fragment must compose with it, including hit testing and focus.
            let native = unsafe {
                CreateWindowExW(
                    0,
                    wide("BUTTON").as_ptr(),
                    wide("Hosted control").as_ptr(),
                    0x40000000 | 0x10000000 | 0x00010000,
                    20,
                    400,
                    180,
                    30,
                    window,
                    0,
                    0,
                    ptr::null_mut(),
                )
            };
            assert_ne!(native, 0);
        }
        // A deadlocked provider must fail the probe, not occupy the CI runner.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(60));
            eprintln!("UIA native probe timed out");
            std::process::exit(1);
        });
        let worker = std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| unsafe { client(window, first_modal) });
            unsafe {
                PostMessageW(window, 0x0010, 0, 0);
                // Keep the STA pump alive while UIA queries and releases the
                // first window's retired objects. Join only after COM returns.
                PostMessageW(keeper, 0x0010, 0, 0);
            } // WM_CLOSE, our own window
            result
        });
        app.run();
        println!("UIA phase: window pump returned");
        if let Err(reason) = worker.join().unwrap() {
            std::panic::resume_unwind(reason);
        }
        if !first_modal {
            assert_eq!(form.value.get(), "Dinner 👩‍🚀");
            assert_eq!(form.presses.get(), 1, "stale row did not call its callback");
            assert_eq!(form.password.get(), "never-export-this");
            assert!(!form.modal.get());
            if !mta_host {
                for mode in ["--modal-first", "--mta-host"] {
                    assert!(
                        std::process::Command::new(std::env::current_exe().unwrap())
                            .arg(mode)
                            .status()
                            .unwrap()
                            .success(),
                        "UIA subprocess {mode}"
                    );
                }
            }
            println!(
                "UIA model: Description={:?}, presses={}, secret unchanged; host={}",
                form.value.get(),
                form.presses.get(),
                if mta_host { "MTA" } else { "STA" }
            );
        }
    }
}
#[cfg(target_os = "windows")]
fn main() {
    probe::run();
}
#[cfg(not(target_os = "windows"))]
fn main() {}
