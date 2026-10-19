//! Native host hit testing and original NSEvent forwarding over a real WebKit
//! document. Events are injected into this test's application; this does not
//! certify physical trackpad timing or replace application-level acceptance.
#[cfg(target_os = "macos")]
mod probe {
    use bunny_ui::action::Modifiers;
    use bunny_ui::host::{MouseButton, NetworkPolicy, WebviewHandle, WebviewInput};
    use bunny_ui::layout::Point;
    use bunny_ui::prelude::*;
    use bunny_ui_apple::ffi::{
        CFRelease, CGPoint, CGRect, Id, ObjcSuper, Sel, class, class_addMethod,
        objc_allocateClassPair, objc_registerClassPair, sel,
    };
    use bunny_ui_macos::{App, WindowSpec};
    use std::cell::Cell;
    use std::ffi::{CStr, c_char, c_void};
    use std::rc::Rc;
    use std::time::Duration;

    thread_local! { static EVENT: Cell<Id> = const { Cell::new(std::ptr::null_mut()) }; }
    #[allow(
        clashing_extern_declarations,
        reason = "Objective-C selectors use their concrete ABI"
    )]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(value: Id, command: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn count(value: Id, command: Sel) -> usize;
        #[link_name = "objc_msgSend"]
        fn at(value: Id, command: Sel, index: usize) -> Id;
        #[link_name = "objc_msgSend"]
        fn chars(value: Id, command: Sel) -> *const c_char;
        #[cfg_attr(target_arch = "aarch64", link_name = "objc_msgSend")]
        #[cfg_attr(target_arch = "x86_64", link_name = "objc_msgSend_stret")]
        fn rectangle(value: Id, command: Sel) -> CGRect;
        #[link_name = "objc_msgSend"]
        fn point(value: Id, command: Sel, point: CGPoint) -> Id;
        #[link_name = "objc_msgSend"]
        fn send(value: Id, command: Sel, argument: Id);
        #[link_name = "objc_msgSend"]
        fn is_class(value: Id, command: Sel, class: Id) -> i8;
        #[link_name = "objc_msgSend"]
        fn screen_point(value: Id, command: Sel, point: CGPoint) -> CGPoint;
        #[link_name = "objc_msgSendSuper"]
        fn super_object(value: *const ObjcSuper, command: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn with_event(value: Id, command: Sel, event: Id) -> Id;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateScrollWheelEvent2(
            source: Id,
            units: u32,
            wheels: u32,
            y: i32,
            x: i32,
            z: i32,
        ) -> Id;
        fn CGEventSetLocation(event: Id, location: CGPoint);
    }

    extern "C" fn current_event(this: Id, command: Sel) -> Id {
        let event = EVENT.with(Cell::get);
        if !event.is_null() {
            return event;
        }
        unsafe {
            super_object(
                &ObjcSuper {
                    receiver: this,
                    class: class("NSApplication"),
                },
                command,
            )
        }
    }

    unsafe fn application() {
        unsafe {
            let kind = objc_allocateClassPair(
                class("NSApplication"),
                c"BunnyScrollProbeApplication".as_ptr(),
                0,
            );
            class_addMethod(
                kind,
                sel("currentEvent"),
                current_event as *const c_void,
                c"@@:".as_ptr(),
            );
            objc_registerClassPair(kind);
            object(kind, sel("sharedApplication"));
        }
    }

    unsafe fn owned_window() -> Id {
        unsafe {
            let windows = object(
                object(class("NSApplication"), sel("sharedApplication")),
                sel("windows"),
            );
            (0..count(windows, sel("count")))
                .map(|i| at(windows, sel("objectAtIndex:"), i))
                .find(|window| {
                    CStr::from_ptr(chars(object(*window, sel("title")), sel("UTF8String")))
                        .to_bytes()
                        == b"Embedded scroll protocol witness"
                })
                .expect("the fixture's own native window")
        }
    }

    /// Inject a real CG-backed NSEvent into native hit testing. Its location
    /// is client-local for the enclosing scene, or screen-local for WebKit's
    /// windowless synthetic event contract. No event leaves this process.
    unsafe fn wheel(dx: i32, dy: i32, parent: bool) {
        unsafe {
            let window = owned_window();
            let root = object(window, sel("contentView"));
            let bounds = rectangle(root, sel("bounds"));
            let location = CGPoint {
                x: 180.0,
                y: bounds.size.height - 120.0,
            };
            let event = CGEventCreateScrollWheelEvent2(std::ptr::null_mut(), 0, 2, dy, dx, 0);
            assert!(!event.is_null());
            let screens = object(class("NSScreen"), sel("screens"));
            let screen = object(screens, sel("firstObject"));
            let height = rectangle(screen, sel("frame")).size.height;
            let at = if dy != 0 {
                location
            } else {
                screen_point(window, sel("convertPointToScreen:"), location)
            };
            CGEventSetLocation(
                event,
                CGPoint {
                    x: at.x,
                    y: height - at.y,
                },
            );
            let carried = with_event(class("NSEvent"), sel("eventWithCGEvent:"), event);
            assert!(!carried.is_null());
            EVENT.with(|slot| slot.set(carried));
            let hit = point(root, sel("hitTest:"), location);
            EVENT.with(|slot| slot.set(std::ptr::null_mut()));
            assert!(!hit.is_null());
            let proxy = is_class(hit, sel("isKindOfClass:"), class("BunnyHostView")) != 0;
            assert_eq!(
                proxy, parent,
                "only an opted-in document proxies wheel input"
            );
            send(hit, sel("scrollWheel:"), carried);
            CFRelease(event as *const c_void);
        }
    }

    unsafe fn pointer_stays_with_document() {
        unsafe {
            let root = object(owned_window(), sel("contentView"));
            let height = rectangle(root, sel("bounds")).size.height;
            let hit = point(
                root,
                sel("hitTest:"),
                CGPoint {
                    x: 180.0,
                    y: height - 120.0,
                },
            );
            assert!(!hit.is_null());
            assert_eq!(
                is_class(hit, sel("isKindOfClass:"), class("BunnyHostView")),
                0
            );
        }
    }

    async fn value(handle: &WebviewHandle, js: &str) -> f64 {
        let (sender, receiver) = task::channel();
        handle.eval(js, move |result| {
            let _ = sender.send(result);
        });
        receiver
            .recv()
            .await
            .expect("native eval callback")
            .expect("valid expression")
            .parse()
            .expect("numeric expression")
    }

    #[derive(Clone)]
    struct Fixture {
        app: App,
        handle: WebviewHandle,
        second: WebviewHandle,
        offset: State<Point>,
        parent: State<bool>,
        links: State<usize>,
        done: State<bool>,
    }

    impl Component for Fixture {
        fn body(self) -> impl View {
            let links = self.links;
            let page = webview_html(
                "<!doctype html><html><body style='margin:0'><a href='https://example.test/letter' style='display:block;height:30px'>Open test link</a><div style='width:1800px;height:7000px;background:#49336b'>Synthetic correspondence</div><p id='end'>End of synthetic body</p></body></html>",
                "", NetworkPolicy::Deny,
            ).handle(&self.handle).on_link(move |_| links.set(links.get() + 1));
            let page = if self.parent.get() {
                page.scroll_with_parent()
            } else {
                page
            };
            let second = webview_html(
                "<div style='height:600px'>Second message</div>",
                "",
                NetworkPolicy::Deny,
            )
            .handle(&self.second);
            let second = if self.parent.get() {
                second.scroll_with_parent()
            } else {
                second
            };
            let first_height = if self.parent.get() { 7100.0 } else { 300.0 };
            let script = self.clone();
            scroll(vstack!(text("Conversation header").frame_height(60.0), page.frame_height(first_height), text("Next message").frame_height(40.0), second.frame_height(700.0), text("Conversation footer").frame_height(1200.0)))
                .offset(self.offset.binding())
                .task(move || {
                    let script = script.clone();
                    async move {
                        let mut ready = false;
                        for _ in 0..200 {
                            if value(&script.handle, "Number(document.readyState === 'complete' && document.getElementById('end') !== null && document.scrollingElement.scrollWidth > document.scrollingElement.clientWidth)").await > 0.0 {
                                ready = true;
                                break;
                            }
                            task::sleep(Duration::from_millis(25)).await;
                        }
                        assert!(ready, "the actual local document loaded and laid out its wide content");
                        unsafe { pointer_stays_with_document(); wheel(0, -80, true); }
                        task::sleep(Duration::from_millis(250)).await;
                        assert!(script.offset.get().y > 0.0, "vertical body input scrolls the enclosing conversation");
                        assert_eq!(value(&script.handle, "document.scrollingElement.scrollTop").await, 0.0);
                        println!("native vertical wheel: parent offset={}, document offset=0", script.offset.get().y);
                        script.offset.set(Point { x: 0.0, y: 0.0 });
                        task::sleep(Duration::from_millis(250)).await;
                        unsafe { wheel(-100, 0, true); }
                        task::sleep(Duration::from_millis(250)).await;
                        assert_eq!(script.offset.get().y, 0.0);
                        assert!(value(&script.handle, "document.scrollingElement.scrollLeft").await > 0.0);
                        println!("native horizontal wheel: document moves, parent stays");
                        script.handle.eval("window.scrollTo(0, 0)", |_| {});
                        task::sleep(Duration::from_millis(100)).await;
                        script.handle.click(20.0, 10.0);
                        task::sleep(Duration::from_millis(500)).await;
                        assert_eq!(script.links.get(), 1, "document links retain their native input");
                        for event in [
                            WebviewInput::Down { x: 4.0, y: 40.0, button: MouseButton::Left, clicks: 1, modifiers: Modifiers::NONE },
                            WebviewInput::Drag { x: 150.0, y: 40.0, button: MouseButton::Left, modifiers: Modifiers::NONE },
                            WebviewInput::Up { x: 150.0, y: 40.0, button: MouseButton::Left, clicks: 1, modifiers: Modifiers::NONE },
                        ] { script.handle.input(event); }
                        task::sleep(Duration::from_millis(500)).await;
                        assert!(value(&script.handle, "window.getSelection().toString().length").await > 0.0, "native text selection remains available");
                        script.offset.set(Point { x: 0.0, y: 7200.0 });
                        task::sleep(Duration::from_millis(250)).await;
                        unsafe { wheel(0, -80, true); }
                        task::sleep(Duration::from_millis(250)).await;
                        assert!(script.offset.get().y > 7200.0, "another message routes to the same outer reader");
                        assert_eq!(value(&script.second, "document.scrollingElement.scrollTop").await, 0.0);
                        println!("large fitted body, native text selection and multiple hosted messages passed");
                        script.offset.set(Point { x: 0.0, y: 0.0 });
                        script.parent.set(false);
                        task::sleep(Duration::from_millis(250)).await;
                        unsafe { wheel(0, -80, false); }
                        task::sleep(Duration::from_millis(250)).await;
                        assert_eq!(script.offset.get().y, 0.0);
                        println!("default document hit testing and pointer ownership retained");
                        script.done.set(true);
                        script.app.quit();
                    }
                })
        }
    }

    pub fn run() {
        unsafe {
            application();
        }
        let app = App::new();
        let fixture = Fixture {
            app: app.clone(),
            handle: WebviewHandle::new(),
            second: WebviewHandle::new(),
            offset: State::new(Point::default()),
            parent: State::new(true),
            links: State::new(0),
            done: State::new(false),
        };
        let done = fixture.done;
        let (sender, receiver) = task::channel();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(20));
            let _ = sender.send(());
        });
        let timeout = app.clone();
        task::spawn(async move {
            let _ = receiver.recv().await;
            if !done.get() {
                timeout.quit();
            }
        })
        .detach();
        app.open(
            WindowSpec::titled("Embedded scroll protocol witness").size(600.0, 400.0),
            Rc::new(app.runtime()),
            fixture,
        );
        app.run();
        assert!(done.get(), "native scroll protocol witness did not finish");
    }
}
#[cfg(target_os = "macos")]
fn main() {
    probe::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {}
