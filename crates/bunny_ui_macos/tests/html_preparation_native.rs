//! Own-window proof of hidden fresh association and preparation retain cleanup.
#[cfg(target_os = "macos")]
mod native {
    use bunny_ui::host::{NetworkPolicy, WebviewHandle, webview_html};
    use bunny_ui::prelude::*;
    use bunny_ui_apple::ffi::{CGRect, Id, Sel, class, sel};
    use bunny_ui_macos::{App, HtmlPreparation, WindowSpec};
    use std::cell::RefCell;
    use std::ffi::{CStr, c_char};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const TITLE: &str = "Hidden HTML preparation protocol";

    #[allow(
        clashing_extern_declarations,
        reason = "Each Objective-C selector uses its concrete ABI"
    )]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(value: Id, selector: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn count(value: Id, selector: Sel) -> usize;
        #[link_name = "objc_msgSend"]
        fn at(value: Id, selector: Sel, index: usize) -> Id;
        #[link_name = "objc_msgSend"]
        fn chars(value: Id, selector: Sel) -> *const c_char;
        #[link_name = "objc_msgSend"]
        fn boolean(value: Id, selector: Sel) -> i8;
        #[link_name = "objc_msgSend"]
        fn is_class(value: Id, selector: Sel, expected: Id) -> i8;
        #[link_name = "objc_msgSend"]
        fn frame(value: Id, selector: Sel) -> CGRect;
    }

    fn webviews() -> Vec<Id> {
        // NSApplication.windows belongs only to this synthetic test process.
        unsafe {
            let app = object(class("NSApplication"), sel("sharedApplication"));
            let windows = object(app, sel("windows"));
            let window = (0..count(windows, sel("count")))
                .map(|index| at(windows, sel("objectAtIndex:"), index))
                .find(|window| {
                    let title = object(*window, sel("title"));
                    CStr::from_ptr(chars(title, sel("UTF8String"))).to_bytes() == TITLE.as_bytes()
                })
                .expect("the owned protocol window");
            let mut pending = vec![object(window, sel("contentView"))];
            let mut result = Vec::new();
            while let Some(view) = pending.pop() {
                if is_class(view, sel("isKindOfClass:"), class("WKWebView")) != 0 {
                    result.push(view);
                } else {
                    let children = object(view, sel("subviews"));
                    pending.extend(
                        (0..count(children, sel("count")))
                            .map(|index| at(children, sel("objectAtIndex:"), index)),
                    );
                }
            }
            result
        }
    }

    fn hidden(view: Id) -> bool {
        unsafe { boolean(view, sel("isHidden")) != 0 }
    }

    fn assert_parked(view: Id) {
        unsafe {
            assert!(hidden(view));
            assert_eq!(boolean(view, sel("canBecomeKeyView")), 0);
            assert!(!object(view, sel("window")).is_null());
            let bounds = frame(view, sel("frame"));
            assert_eq!((bounds.size.width, bounds.size.height), (1.0, 1.0));
        }
    }

    #[derive(Clone)]
    struct Probe {
        app: App,
        reserve: Rc<RefCell<Option<HtmlPreparation>>>,
        phase: State<u8>,
        ready: State<bool>,
        handle: WebviewHandle,
    }

    impl Component for Probe {
        fn body(self) -> impl View {
            let phase = self.phase.get();
            let ready = self.ready;
            let reader = (phase >= 2).then(|| {
                webview_html("<html><body>owned fresh body</body></html>", "", NetworkPolicy::Deny)
                    .user_script("document.addEventListener('DOMContentLoaded', () => window.bunny.post('ready'));")
                    .on_message(move |message| {
                        assert_eq!(message, "ready");
                        ready.set(true);
                    })
                    .handle(&self.handle)
                    .id("reader")
                    .frame(320.0, 180.0)
            });
            let probe = self.clone();
            vstack!(text("Owned protocol window"), reader).task(move || {
                let probe = probe.clone();
                async move {
                    let deadline = Instant::now() + Duration::from_secs(15);
                    loop {
                        assert!(Instant::now() < deadline, "preparation protocol deadline");
                        let available = probe.reserve.borrow().as_ref().map(HtmlPreparation::available);
                        match probe.phase.get() {
                            0 if available == Some(2) => {
                                let views = webviews();
                                assert_eq!(views.len(), 2, "fresh engines must be associated while hidden");
                                views.into_iter().for_each(assert_parked);
                                probe.reserve.borrow_mut().take();
                                assert!(webviews().is_empty(), "dropping preparation must detach unclaimed views");
                                *probe.reserve.borrow_mut() = Some(HtmlPreparation::request());
                                probe.phase.set(1);
                            }
                            1 if available == Some(2) => probe.phase.set(2),
                            2 if probe.ready.get() => {
                                let views = webviews();
                                assert_eq!(views.len(), 2);
                                assert_eq!(views.iter().filter(|view| hidden(**view)).count(), 1);
                                let visible = *views.iter().find(|view| !hidden(**view)).unwrap();
                                unsafe {
                                    let bounds = frame(visible, sel("frame"));
                                    assert_eq!((bounds.size.width, bounds.size.height), (320.0, 180.0));
                                }
                                probe.reserve.borrow_mut().take();
                                assert_eq!(webviews(), vec![visible], "the borrowed reader must survive reserve drop");
                                probe.phase.set(4);
                                let phase = probe.phase;
                                probe.handle.eval("document.body.innerText", move |result| {
                                    assert_eq!(result.unwrap(), "\"owned fresh body\"");
                                    phase.set(3);
                                });
                            }
                            3 => {
                                println!("native preparation: hidden fresh views, key exclusion, unclaimed cleanup and live reader ownership passed");
                                probe.app.quit();
                                break;
                            }
                            _ => {}
                        }
                        task::sleep(Duration::from_millis(10)).await;
                    }
                }
            })
        }
    }

    pub fn run() {
        let app = App::new();
        app.open(
            WindowSpec::titled(TITLE).size(500.0, 260.0),
            Rc::new(app.runtime()),
            Probe {
                app: app.clone(),
                reserve: Rc::new(RefCell::new(Some(HtmlPreparation::request()))),
                phase: State::new(0),
                ready: State::new(false),
                handle: WebviewHandle::new(),
            },
        );
        app.run();
    }
}

#[cfg(target_os = "macos")]
fn main() {
    native::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {}
