//! Main-thread AppKit protocol witness. This does not replace a VoiceOver run.
#[cfg(target_os = "macos")]
mod probe {
    use bunny_ui::prelude::*;
    use bunny_ui_apple::ffi::{CGPoint, CGRect, Id, Sel, class, sel};
    use bunny_ui_macos::{App, WindowSpec};
    use std::{
        ffi::{CStr, c_char},
        rc::Rc,
    };

    #[allow(
        clashing_extern_declarations,
        reason = "Objective-C dispatch uses the concrete ABI of each selector"
    )]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(object: Id, selector: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn count(object: Id, selector: Sel) -> usize;
        #[link_name = "objc_msgSend"]
        fn at(object: Id, selector: Sel, index: usize) -> Id;
        #[link_name = "objc_msgSend"]
        fn chars(object: Id, selector: Sel) -> *const c_char;
        #[link_name = "objc_msgSend"]
        fn boolean(object: Id, selector: Sel) -> i8;
        #[cfg_attr(target_arch = "aarch64", link_name = "objc_msgSend")]
        #[cfg_attr(target_arch = "x86_64", link_name = "objc_msgSend_stret")]
        fn rectangle(object: Id, selector: Sel) -> CGRect;
        #[link_name = "objc_msgSend"]
        fn point_object(object: Id, selector: Sel, point: CGPoint) -> Id;
        #[link_name = "objc_msgSend"]
        fn set_point(object: Id, selector: Sel, point: CGPoint);
        #[link_name = "objc_msgSend"]
        fn allowed(object: Id, selector: Sel, requested: Sel) -> i8;
        #[link_name = "objc_msgSend"]
        fn void(object: Id, selector: Sel);
        #[link_name = "objc_msgSend"]
        fn set_object(object: Id, selector: Sel, value: Id);
        #[link_name = "objc_msgSend"]
        fn set_bool(object: Id, selector: Sel, value: i8);
    }

    unsafe fn string(value: Id) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let bytes = unsafe { chars(value, sel("UTF8String")) };
        if bytes.is_null() {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr(bytes) }
                .to_string_lossy()
                .into_owned(),
        )
    }
    unsafe fn property(value: Id, name: &str) -> Option<String> {
        unsafe { string(object(value, sel(name))) }
    }
    unsafe fn children(view: Id) -> Vec<Id> {
        let list = unsafe { object(view, sel("accessibilityChildren")) };
        (0..unsafe { count(list, sel("count")) })
            .map(|i| unsafe { at(list, sel("objectAtIndex:"), i) })
            .collect()
    }
    unsafe fn named(view: Id, name: &str) -> Id {
        unsafe { children(view) }
            .into_iter()
            .find(|node| unsafe { property(*node, "accessibilityLabel") }.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("native accessible element missing: {name}"))
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
                text("Native protocol witness"),
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
                ),
            )
            .padding()
            .sheet(self.modal.binding(), move |_| {
                erased(button(text("Dismiss modal"), move || self.modal.set(false)).padding())
            })
        }
    }

    pub fn run() {
        let app = App::new();
        let runtime = Rc::new(app.runtime());
        let form = Form {
            name: State::new("Description".into()),
            value: State::new("Lunch".into()),
            password: State::new("never-export-this".into()),
            presses: State::new(0),
            rows: State::new(vec![1, 2]),
            modal: State::new(false),
        };
        let id = app.open(
            WindowSpec::titled("Bunny accessibility protocol witness").size(480.0, 640.0),
            Rc::clone(&runtime),
            form,
        );
        unsafe {
            let ns_app = object(class("NSApplication"), sel("sharedApplication"));
            let windows = object(ns_app, sel("windows"));
            let window = (0..count(windows, sel("count")))
                .map(|i| at(windows, sel("objectAtIndex:"), i))
                .find(|window| {
                    property(*window, "title").as_deref()
                        == Some("Bunny accessibility protocol witness")
                })
                .unwrap();
            let view = object(window, sel("contentView"));
            let field = named(view, "Description");
            assert_eq!(
                property(field, "accessibilityRole").as_deref(),
                Some("AXTextField")
            );
            assert_eq!(
                property(field, "accessibilityValue").as_deref(),
                Some("Lunch")
            );
            let password = named(view, "Password");
            assert_eq!(
                property(password, "accessibilitySubrole").as_deref(),
                Some("AXSecureTextField")
            );
            assert!(property(password, "accessibilityValue").is_none());
            let initial_frame = rectangle(field, sel("accessibilityFrame"));
            assert!(initial_frame.size.width > 0.0 && initial_frame.size.height > 0.0);
            let hit = point_object(
                view,
                sel("accessibilityHitTest:"),
                CGPoint {
                    x: initial_frame.origin.x + initial_frame.size.width / 2.0,
                    y: initial_frame.origin.y + initial_frame.size.height / 2.0,
                },
            );
            assert_eq!(hit, field, "native hit testing and screen bounds agree");
            let window_frame = rectangle(window, sel("frame"));
            set_point(
                window,
                sel("setFrameOrigin:"),
                CGPoint {
                    x: window_frame.origin.x + 10.0,
                    y: window_frame.origin.y + 10.0,
                },
            );
            let moved = rectangle(field, sel("accessibilityFrame"));
            assert!((moved.origin.x - initial_frame.origin.x - 10.0).abs() < 0.01);
            assert!((moved.origin.y - initial_frame.origin.y - 10.0).abs() < 0.01);
            assert_eq!(object(field, sel("accessibilityWindow")), window);
            assert_eq!(
                allowed(
                    field,
                    sel("isAccessibilitySelectorAllowed:"),
                    sel("accessibilityPerformPress")
                ),
                0
            );
            assert_eq!(
                allowed(
                    named(view, "Save"),
                    sel("isAccessibilitySelectorAllowed:"),
                    sel("setAccessibilityValue:")
                ),
                0
            );
            set_object(
                field,
                sel("setAccessibilityValue:"),
                bunny_ui_apple::ffi::ns_string("Dinner"),
            );
            assert_eq!(form.value.get(), "Dinner");
            assert_eq!(
                property(field, "accessibilityValue").as_deref(),
                Some("Dinner")
            );
            set_bool(field, sel("setAccessibilityFocused:"), 1);
            assert_eq!(boolean(field, sel("isAccessibilityFocused")), 1);
            assert!(runtime.focused().is_some());
            assert_eq!(
                boolean(named(view, "Save"), sel("accessibilityPerformPress")),
                1
            );
            assert_eq!(form.presses.get(), 1);
            assert_eq!(
                named(view, "Updated name"),
                field,
                "a changing name keeps the native object"
            );
            let removed = object(named(view, "Row 2"), sel("retain"));
            assert_eq!(
                boolean(named(view, "Remove row"), sel("accessibilityPerformPress")),
                1
            );
            assert_eq!(boolean(removed, sel("accessibilityPerformPress")), 0);
            assert_eq!(form.presses.get(), 1);
            void(removed, sel("release"));
            let old_save = object(named(view, "Save"), sel("retain"));
            assert_eq!(
                boolean(named(view, "Open modal"), sel("accessibilityPerformPress")),
                1
            );
            assert!(
                children(view).is_empty(),
                "the covered main surface exposes no controls"
            );
            assert_eq!(boolean(old_save, sel("accessibilityPerformPress")), 0);
            let child_windows = object(window, sel("childWindows"));
            let panel = (0..count(child_windows, sel("count")))
                .map(|i| at(child_windows, sel("objectAtIndex:"), i))
                .find(|panel| {
                    children(object(*panel, sel("contentView")))
                        .iter()
                        .any(|node| {
                            property(*node, "accessibilityLabel").as_deref()
                                == Some("Dismiss modal")
                        })
                })
                .unwrap();
            let panel_view = object(panel, sel("contentView"));
            let dismiss = object(named(panel_view, "Dismiss modal"), sel("retain"));
            assert_eq!(object(dismiss, sel("accessibilityWindow")), panel);
            assert_eq!(boolean(dismiss, sel("accessibilityPerformPress")), 1);
            assert!(!form.modal.get());
            assert_eq!(boolean(dismiss, sel("accessibilityPerformPress")), 0);
            assert!(!named(view, "Updated name").is_null());
            void(dismiss, sel("release"));
            void(old_save, sel("release"));
            let needed = runtime.needs_frame();
            for _ in 0..10 {
                let _ = children(view);
            }
            assert_eq!(
                runtime.needs_frame(),
                needed,
                "native queries schedule no idle frames"
            );
            for node in children(view) {
                println!(
                    "NSAccessibility role={:?} label={:?} value={:?} focused={}",
                    property(node, "accessibilityRole"),
                    property(node, "accessibilityLabel"),
                    property(node, "accessibilityValue"),
                    boolean(node, sel("isAccessibilityFocused"))
                );
            }
        }
        // Keep a second window open so closing the first does not terminate
        // the process before the retired-object assertions can run.
        let keeper = app.open(
            WindowSpec::titled("Accessibility lifetime witness").size(240.0, 120.0),
            Rc::new(app.runtime()),
            Form {
                modal: State::new(true),
                ..form
            },
        );
        unsafe {
            let ns_app = object(class("NSApplication"), sel("sharedApplication"));
            let windows = object(ns_app, sel("windows"));
            let window = (0..count(windows, sel("count")))
                .map(|i| at(windows, sel("objectAtIndex:"), i))
                .find(|window| {
                    property(*window, "title").as_deref()
                        == Some("Bunny accessibility protocol witness")
                })
                .unwrap();
            let view = object(object(window, sel("contentView")), sel("retain"));
            let save = object(named(view, "Save"), sel("retain"));
            app.close(id);
            assert_eq!(boolean(save, sel("accessibilityPerformPress")), 0);
            assert_eq!(boolean(save, sel("isAccessibilityElement")), 0);
            assert_eq!(boolean(view, sel("isAccessibilityElement")), 0);
            void(save, sel("release"));
            void(view, sel("release"));
        }
        unsafe {
            let ns_app = object(class("NSApplication"), sel("sharedApplication"));
            let windows = object(ns_app, sel("windows"));
            let window = (0..count(windows, sel("count")))
                .map(|i| at(windows, sel("objectAtIndex:"), i))
                .find(|window| {
                    property(*window, "title").as_deref() == Some("Accessibility lifetime witness")
                })
                .unwrap();
            let child_windows = object(window, sel("childWindows"));
            let panel = at(child_windows, sel("objectAtIndex:"), 0);
            let panel_view = object(panel, sel("contentView"));
            let dismiss = named(panel_view, "Dismiss modal");
            assert_eq!(object(dismiss, sel("accessibilityWindow")), panel);
            assert_eq!(
                boolean(dismiss, sel("accessibilityPerformPress")),
                1,
                "the first accessibility query may target a modal without querying its parent"
            );
        }
        println!("Native AppKit protocol witness passed; no human assistive-technology claim.");
        app.close(keeper);
    }
}

#[cfg(target_os = "macos")]
fn main() {
    probe::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {}
