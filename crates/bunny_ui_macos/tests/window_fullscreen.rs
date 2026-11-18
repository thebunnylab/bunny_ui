//! Native full screen through the public `App` API, against AppKit's own
//! style mask. A session that cannot enter full screen (no window server,
//! a refused Space) reports the stage as not qualified, never as passed.
#[cfg(target_os = "macos")]
mod native {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::Duration;

    use bunny_ui::prelude::*;
    use bunny_ui_apple::ffi::{CGPoint, Id, Sel, class, ns_string, sel};
    use bunny_ui_macos::{App, FullscreenEvent, FullscreenMode, WindowError, WindowSpec};

    #[allow(clashing_extern_declarations, reason = "Each selector uses its concrete Objective-C ABI")]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(value: Id, selector: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn number(value: Id, selector: Sel) -> u64;
        #[link_name = "objc_msgSend"]
        fn at(value: Id, selector: Sel, index: u64) -> Id;
        #[link_name = "objc_msgSend"]
        fn send(value: Id, selector: Sel, argument: Id);
        #[link_name = "objc_msgSend"]
        fn key_event(
            class: Id,
            selector: Sel,
            kind: u64,
            location: CGPoint,
            modifiers: u64,
            timestamp: f64,
            window: i64,
            context: Id,
            characters: Id,
            bare: Id,
            repeat: bool,
            code: u16,
        ) -> Id;
    }

    /// Escape, pressed in the window titled `title`, as AppKit hands it to
    /// the view that holds the keyboard.
    fn press_escape(title: &str) {
        unsafe {
            let windows = object(object(class("NSApplication"), sel("sharedApplication")), sel("windows"));
            let window = (0..number(windows, sel("count")))
                .map(|index| at(windows, sel("objectAtIndex:"), index))
                .find(|window| {
                    let name = object(*window, sel("title"));
                    std::ffi::CStr::from_ptr(object(name, sel("UTF8String")).cast()).to_str() == Ok(title)
                })
                .expect("the window is open");
            let escape = ns_string("\u{1b}");
            let event = key_event(
                class("NSEvent"),
                sel("keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:"),
                10,
                CGPoint { x: 0.0, y: 0.0 },
                0,
                0.0,
                number(window, sel("windowNumber")) as i64,
                std::ptr::null_mut(),
                escape,
                escape,
                false,
                53,
            );
            send(object(window, sel("contentView")), sel("keyDown:"), event);
        }
    }

    const AUTO_HIDE_TOOLBAR: u64 = 1 << 11;

    fn presentation_options() -> u64 {
        unsafe { number(object(class("NSApplication"), sel("sharedApplication")), sel("currentSystemPresentationOptions")) }
    }

    async fn until(mut ready: impl FnMut() -> bool) -> bool {
        for _ in 0..1_000 {
            if ready() {
                return true;
            }
            task::sleep(Duration::from_millis(10)).await;
        }
        ready()
    }

    type Heard = Rc<RefCell<Vec<FullscreenEvent>>>;

    fn landed(heard: &Heard, event: FullscreenEvent) -> bool {
        heard.borrow().contains(&event)
    }

    pub fn run() {
        let app = App::new();
        let finished = Rc::new(Cell::new(false));
        let runtime = Rc::new(app.runtime());
        let id = Rc::new(Cell::new(None));
        let window = app.open(
            WindowSpec::titled("Full screen").size(360.0, 240.0),
            Rc::clone(&runtime),
            text("full screen"),
        );
        // the driver outlives the window it sends to full screen and back,
        // so closing that one does not end the app
        app.open(
            WindowSpec::titled("Full screen driver").size(160.0, 100.0),
            Rc::new(app.runtime()),
            text("driver").task({
                let app = app.clone();
                let id = Rc::clone(&id);
                let finished = Rc::clone(&finished);
                move || {
                    let app = app.clone();
                    let id = Rc::clone(&id);
                    let finished = Rc::clone(&finished);
                    async move {
                        until(|| id.get().is_some()).await;
                        let window = id.get().unwrap();
                        let heard: Heard = Rc::new(RefCell::new(Vec::new()));
                        app.on_fullscreen_changed(window, {
                            let heard = Rc::clone(&heard);
                            move |from, event| {
                                assert_eq!(from, window);
                                println!("event: {event:?}");
                                heard.borrow_mut().push(event);
                            }
                        })
                        .unwrap();
                        assert_eq!(app.fullscreen(window), Ok(FullscreenMode::Windowed));
                        let foreign = App::new();
                        assert_eq!(foreign.fullscreen(window), Err(WindowError::StaleWindow));
                        assert_eq!(
                            foreign.set_fullscreen(window, FullscreenMode::Fullscreen),
                            Err(WindowError::StaleWindow)
                        );

                        app.set_fullscreen(window, FullscreenMode::Fullscreen).unwrap();
                        let arrived = until(|| {
                            landed(&heard, FullscreenEvent::Entered) || landed(&heard, FullscreenEvent::EnterFailed)
                        })
                        .await;
                        if !arrived || landed(&heard, FullscreenEvent::EnterFailed) {
                            println!("Full screen not qualified: this session never entered it");
                            finished.set(true);
                            app.quit();
                            return;
                        }
                        assert_eq!(*heard.borrow(), [FullscreenEvent::WillEnter, FullscreenEvent::Entered]);
                        assert_eq!(app.fullscreen(window), Ok(FullscreenMode::Fullscreen));
                        assert_eq!(presentation_options() & AUTO_HIDE_TOOLBAR, 0);

                        heard.borrow_mut().clear();
                        app.set_fullscreen(window, FullscreenMode::Presentation).unwrap();
                        assert_eq!(app.fullscreen(window), Ok(FullscreenMode::Presentation));
                        task::sleep(Duration::from_millis(200)).await;
                        assert!(heard.borrow().is_empty(), "switching modes on the same screen is no trip");
                        println!("Presentation switched in place");

                        // an ask while the window is moving waits for the trip to land
                        app.set_fullscreen(window, FullscreenMode::Windowed).unwrap();
                        app.set_fullscreen(window, FullscreenMode::Presentation).unwrap();
                        assert!(
                            until(|| heard.borrow().iter().filter(|e| **e == FullscreenEvent::Entered).count() == 1).await,
                            "the queued ask entered again: {:?}",
                            heard.borrow()
                        );
                        assert_eq!(
                            *heard.borrow(),
                            [
                                FullscreenEvent::WillExit,
                                FullscreenEvent::Exited,
                                FullscreenEvent::WillEnter,
                                FullscreenEvent::Entered
                            ]
                        );
                        assert_eq!(app.fullscreen(window), Ok(FullscreenMode::Presentation));
                        assert_ne!(presentation_options() & AUTO_HIDE_TOOLBAR, 0, "a presentation hides the toolbar too");
                        println!("A queued ask landed after the trip");

                        heard.borrow_mut().clear();
                        press_escape("Full screen");
                        assert!(until(|| landed(&heard, FullscreenEvent::Exited)).await, "Escape leaves a presentation");
                        assert_eq!(*heard.borrow(), [FullscreenEvent::WillExit, FullscreenEvent::Exited]);
                        assert_eq!(app.fullscreen(window), Ok(FullscreenMode::Windowed));
                        println!("Escape took a presentation back");

                        heard.borrow_mut().clear();
                        app.set_fullscreen(window, FullscreenMode::Fullscreen).unwrap();
                        assert!(until(|| landed(&heard, FullscreenEvent::Entered)).await);
                        assert_eq!(presentation_options() & AUTO_HIDE_TOOLBAR, 0, "plain full screen keeps the system's manners");
                        press_escape("Full screen");
                        task::sleep(Duration::from_millis(300)).await;
                        assert_eq!(app.fullscreen(window), Ok(FullscreenMode::Fullscreen), "Escape is the app's in plain full screen");
                        heard.borrow_mut().clear();
                        app.set_fullscreen(window, FullscreenMode::Windowed).unwrap();
                        assert!(until(|| landed(&heard, FullscreenEvent::Exited)).await);
                        assert_eq!(*heard.borrow(), [FullscreenEvent::WillExit, FullscreenEvent::Exited]);
                        assert_eq!(app.fullscreen(window), Ok(FullscreenMode::Windowed));
                        heard.borrow_mut().clear();
                        app.set_fullscreen(window, FullscreenMode::Windowed).unwrap();
                        task::sleep(Duration::from_millis(200)).await;
                        assert!(heard.borrow().is_empty(), "a window already windowed takes no trip");

                        app.clear_fullscreen_handler(window).unwrap();
                        app.close(window);
                        assert_eq!(app.fullscreen(window), Err(WindowError::StaleWindow));
                        assert_eq!(app.on_fullscreen_changed(window, |_, _| {}), Err(WindowError::StaleWindow));
                        println!("Native full screen contract passed");
                        finished.set(true);
                        app.quit();
                    }
                }
            }),
        );
        id.set(Some(window));
        app.run();
        assert!(finished.get(), "fixture exited before completing assertions");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use std::time::{Duration, Instant};
    if std::env::var_os("BUNNY_FULLSCREEN_CHILD").is_some() {
        native::run();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_FULLSCREEN_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(result) = child.try_wait().unwrap() {
            assert!(result.success(), "native full screen fixture: {result}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native full screen fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
