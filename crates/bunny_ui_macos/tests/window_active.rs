//! The active window against AppKit's own word, through the public `App` API.
//!
//! The fixture reads `NSApplication.isActive` and `keyWindow` itself and asks
//! that `bunny_ui::app::window_active` and the `WindowActive` stream say the
//! same, once per change. Whether this process is brought to the front, and
//! whether it keeps the front for the length of the run, is the window
//! system's decision: a stage it did not grant is reported as not qualified,
//! never as passed. No keyboard or mouse input is sent.
#[cfg(target_os = "macos")]
const COMPLETE: &str = "The active window agreed with AppKit at every stage that was reached.";

#[cfg(target_os = "macos")]
mod native {
    use std::cell::{Cell, RefCell};
    use std::ffi::{CStr, c_char};
    use std::rc::Rc;
    use std::time::Duration;

    use bunny_ui::app::{self, AppEvent};
    use bunny_ui::layout::DialogSpec;
    use bunny_ui::prelude::*;
    use bunny_ui::window::{ConfirmationError, ConfirmationSpec, WindowId};
    use bunny_ui_apple::ffi::{CGSize, Id, Sel, class, ns_string, sel};
    use bunny_ui_macos::{App, WindowSpec};

    #[cfg(target_arch = "aarch64")]
    type NativeBool = bool;
    #[cfg(not(target_arch = "aarch64"))]
    type NativeBool = i8;

    #[allow(
        clashing_extern_declarations,
        reason = "Each selector uses its concrete Objective-C ABI"
    )]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn object(value: Id, selector: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn flag(value: Id, selector: Sel) -> NativeBool;
        #[link_name = "objc_msgSend"]
        fn chars(value: Id, selector: Sel) -> *const c_char;
        #[link_name = "objc_msgSend"]
        fn process(value: Id, selector: Sel) -> i32;
        #[link_name = "objc_msgSend"]
        fn activate(value: Id, selector: Sel, options: usize) -> NativeBool;
        #[link_name = "objc_msgSend"]
        fn image_size(value: Id, selector: Sel, size: CGSize) -> Id;
        #[link_name = "objc_msgSend"]
        fn image_name(value: Id, selector: Sel, name: Id) -> NativeBool;
    }

    /// The scenes this fixture opened, by the title AppKit shows for each.
    type Scenes = Rc<RefCell<Vec<(&'static str, WindowId)>>>;

    fn application() -> Id {
        unsafe { object(class("NSApplication"), sel("sharedApplication")) }
    }

    fn in_front() -> bool {
        unsafe { flag(application(), sel("isActive")) as u8 != 0 }
    }

    /// The window the key window answers for: a sheet for the window it is
    /// attached to, a dialog for the window it hangs from, any other for itself.
    fn key_host() -> Id {
        unsafe {
            let mut window = object(application(), sel("keyWindow"));
            for _ in 0..8 {
                if window.is_null() {
                    break;
                }
                let attached_to = object(window, sel("sheetParent"));
                let above = if attached_to.is_null() {
                    object(window, sel("parentWindow"))
                } else {
                    attached_to
                };
                if above.is_null() {
                    break;
                }
                window = above;
            }
            window
        }
    }

    fn title_of(window: Id) -> String {
        unsafe {
            CStr::from_ptr(chars(object(window, sel("title")), sel("UTF8String")))
                .to_string_lossy()
                .into_owned()
        }
    }

    /// Is the key window itself, not the one it answers for, titled `title`?
    fn key_is(title: &str) -> bool {
        let key = unsafe { object(application(), sel("keyWindow")) };
        !key.is_null() && title_of(key) == title
    }

    fn sheet_holds_key() -> bool {
        unsafe {
            let key = object(application(), sel("keyWindow"));
            !key.is_null() && !object(key, sel("sheetParent")).is_null()
        }
    }

    /// AppKit's own answer: the scene whose window (or sheet) is key in the
    /// active application, read independently of the shell.
    fn truth(scenes: &Scenes) -> Option<WindowId> {
        if !in_front() {
            return None;
        }
        let host = key_host();
        if host.is_null() {
            return None;
        }
        let title = title_of(host);
        scenes
            .borrow()
            .iter()
            .find(|(name, _)| *name == title)
            .map(|(_, id)| *id)
    }

    /// The framework's answer. More than one active window is already a defect.
    fn answer(scenes: &Scenes) -> Option<WindowId> {
        let active: Vec<WindowId> = scenes
            .borrow()
            .iter()
            .map(|(_, id)| *id)
            .filter(|id| app::window_active(*id))
            .collect();
        assert!(active.len() <= 1, "two windows active at once: {active:?}");
        active.first().copied()
    }

    /// What a subscriber must hear for one settled move of the active window.
    fn moved(from: Option<WindowId>, to: Option<WindowId>) -> Vec<(WindowId, bool)> {
        if from == to {
            return Vec::new();
        }
        from.map(|window| (window, false))
            .into_iter()
            .chain(to.map(|window| (window, true)))
            .collect()
    }

    /// The subscriber's side: what it heard, and what that adds up to.
    struct Ear {
        life: task::Receiver<AppEvent>,
        believes: Cell<Option<WindowId>>,
    }

    impl Ear {
        /// Everything heard since the last call. Every event must be a change
        /// of what the subscriber already believed: no repeat, no second
        /// active window, no word about a window that was not the active one.
        fn heard(&self) -> Vec<(WindowId, bool)> {
            let mut events = Vec::new();
            while let Some(event) = self.life.try_recv() {
                let AppEvent::WindowActive { window, active } = event else {
                    continue;
                };
                let believed = self.believes.get();
                if active {
                    assert_eq!(believed, None, "{window:?} became active over another");
                    self.believes.set(Some(window));
                } else {
                    assert_eq!(believed, Some(window), "{window:?} was not the active one");
                    self.believes.set(None);
                }
                events.push((window, active));
            }
            events
        }
    }

    async fn until(mut ready: impl FnMut() -> bool) -> bool {
        for _ in 0..500 {
            if ready() {
                return true;
            }
            task::sleep(Duration::from_millis(10)).await;
        }
        ready()
    }

    /// Waits for the shell to say what AppKit says, and for both to hold
    /// still. The shell reads the key one wake after AppKit moved it.
    async fn settled(stage: &str, scenes: &Scenes) -> Option<WindowId> {
        let mut still = 0;
        for _ in 0..500 {
            still = if answer(scenes) == truth(scenes) {
                still + 1
            } else {
                0
            };
            if still == 8 {
                return truth(scenes);
            }
            task::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "{stage}: the shell says {:?} and AppKit says {:?}",
            answer(scenes),
            truth(scenes)
        );
    }

    /// One stage: `act`, wait for AppKit to reach `reached`, then require the
    /// shell's answer, the query and the stream to be that one settled move.
    /// `false` when another application took the front meanwhile — the stage
    /// then proves nothing about key travel, and says so.
    async fn stage(
        name: &str,
        scenes: &Scenes,
        ear: &Ear,
        act: impl FnOnce(),
        mut reached: impl FnMut(Option<WindowId>) -> bool,
    ) -> bool {
        let before = settled(name, scenes).await;
        assert_eq!(ear.heard(), [], "{name}: nothing moved before the stage");
        act();
        let arrived = until(|| reached(truth(scenes))).await;
        let after = settled(name, scenes).await;
        let heard = ear.heard();
        assert_eq!(
            ear.believes.get(),
            after,
            "{name}: the stream and the query agree"
        );
        if !in_front() {
            println!("Not qualified: another application took the front during {name}");
            return false;
        }
        assert!(
            arrived,
            "{name}: AppKit never reached the expected key window"
        );
        assert_eq!(heard, moved(before, after), "{name}: one event per change");
        println!("{name}: {before:?} -> {after:?}, heard {heard:?}");
        true
    }

    // NSAlert's nib otherwise asks IconServices synchronously for the icon of
    // an unbundled process (see `confirmation_native`).
    fn supply_application_icon() {
        unsafe {
            let icon = image_size(
                object(class("NSImage"), sel("alloc")),
                sel("initWithSize:"),
                CGSize {
                    width: 32.0,
                    height: 32.0,
                },
            );
            assert!(!icon.is_null());
            assert_ne!(
                image_name(icon, sel("setName:"), ns_string("NSApplicationIcon")) as u8,
                0
            );
        }
    }

    /// One scene of the fixture: its window, and a dialog of its own to raise.
    struct Desk {
        id: WindowId,
        title: &'static str,
        runtime: Rc<Runtime>,
        dialog: State<bool>,
        dialog_title: &'static str,
    }

    fn raise(app: &App, title: &'static str, dialog_title: &'static str) -> Desk {
        let dialog = State::new(false);
        let runtime = Rc::new(app.runtime());
        let id = app.open(
            WindowSpec::titled(title).size(320.0, 200.0),
            Rc::clone(&runtime),
            text(title).dialog(dialog.binding(), DialogSpec::titled(dialog_title), |_| {
                erased(text("its own dialog"))
            }),
        );
        Desk {
            id,
            title,
            runtime,
            dialog,
            dialog_title,
        }
    }

    pub fn run() {
        // Whoever holds the front now gets it back at the end: that hand-over
        // is the real "away from the window", and it leaves the desk as found.
        let previous = unsafe {
            let front = object(
                object(class("NSWorkspace"), sel("sharedWorkspace")),
                sel("frontmostApplication"),
            );
            if front.is_null()
                || process(front, sel("processIdentifier")) == std::process::id() as i32
            {
                None
            } else {
                Some(object(front, sel("retain")))
            }
        };
        let (sender, life) = task::channel::<AppEvent>();
        app::subscribe(sender);
        let ear = Ear {
            life,
            believes: Cell::new(None),
        };
        let app = App::new();
        supply_application_icon();
        let desks = [
            raise(&app, "Active first", "Dialog of the first"),
            raise(&app, "Active second", "Dialog of the second"),
        ];
        let scenes: Scenes = Rc::new(RefCell::new(
            desks.iter().map(|desk| (desk.title, desk.id)).collect(),
        ));
        assert!(
            desks.iter().all(|desk| !app::window_active(desk.id)),
            "a window is born inactive"
        );
        let completed = Rc::new(Cell::new(false));
        let done = Rc::clone(&completed);
        let driver = app.clone();
        task::spawn(async move {
            let app = driver;
            let qualified = travel(&app, &scenes, &ear, &desks).await;
            if qualified {
                step_back(&scenes, &ear, previous).await;
            }
            done.set(true);
            println!("{}", super::COMPLETE);
            app.quit();
        })
        .detach();
        app.run();
        assert!(completed.get(), "fixture exited before all assertions");
    }

    /// The key travelling between this app's own windows. `false` as soon as
    /// the window system withholds or takes away the front.
    async fn travel(app: &App, scenes: &Scenes, ear: &Ear, desks: &[Desk; 2]) -> bool {
        if !until(in_front).await {
            let nobody = settled("launch", scenes).await;
            assert_eq!(nobody, None);
            assert_eq!(
                ear.heard(),
                [],
                "a process in the background has no active window"
            );
            println!("Not qualified: the window system did not bring this process to the front");
            return false;
        }
        // Launch: the key lands on one of the two, and the subscriber heard
        // that and nothing it has to take back.
        let born = settled("launch", scenes).await;
        let heard = ear.heard();
        assert_eq!(
            ear.believes.get(),
            born,
            "launch: the stream and the query agree"
        );
        assert!(
            born.is_some(),
            "launch: an active app with two windows has a key window"
        );
        println!("launch: active {born:?}, heard {heard:?}");
        let (here, desk) = if born == Some(desks[0].id) {
            (desks[0].id, &desks[1])
        } else {
            (desks[1].id, &desks[0])
        };
        let other = desk.id;

        // `focus`, `hide` and `show` run with the scene's ears closed
        // (`lend_hand`): the shell's own key events are dropped there, and the
        // active window must still be heard.
        if !stage(
            "focus another window",
            scenes,
            ear,
            || app.focus(other).unwrap(),
            |now| now == Some(other),
        )
        .await
        {
            return false;
        }
        if !stage(
            "hide the active window",
            scenes,
            ear,
            || app.hide(other).unwrap(),
            |now| now != Some(other),
        )
        .await
        {
            return false;
        }
        if !stage(
            "show it again",
            scenes,
            ear,
            || app.show(other).unwrap(),
            |now| now == Some(other),
        )
        .await
        {
            return false;
        }

        // A sheet takes the key from its window and is still that window.
        let question = ConfirmationSpec::new("Leave?", "The sheet belongs to its window.").unwrap();
        let mut confirmation = None;
        if !stage(
            "its own sheet takes the key",
            scenes,
            ear,
            || confirmation = Some(app.confirm(other, question).unwrap()),
            |now| now == Some(other) && sheet_holds_key(),
        )
        .await
        {
            return false;
        }
        let mut confirmation = confirmation.expect("the sheet was asked for");
        assert!(sheet_holds_key(), "an actual sheet holds the key");
        assert!(
            app::window_active(other),
            "the window under its sheet is the active one"
        );
        assert!(confirmation.cancel());
        assert_eq!(confirmation.await, Err(ConfirmationError::Cancelled));
        if !stage(
            "the sheet hands it back",
            scenes,
            ear,
            || {},
            |now| now == Some(other) && !sheet_holds_key(),
        )
        .await
        {
            return false;
        }

        // …and so does a dialog of its own: the key crosses to a child window
        // and comes back, and the window was never left.
        if !stage(
            "its own dialog takes the key",
            scenes,
            ear,
            || {
                desk.dialog.set(true);
                desk.runtime.request_frame();
            },
            |now| now == Some(other) && key_is(desk.dialog_title),
        )
        .await
        {
            return false;
        }
        if !stage(
            "the dialog hands it back",
            scenes,
            ear,
            || {
                desk.dialog.set(false);
                desk.runtime.request_frame();
            },
            |now| now == Some(other) && key_is(desk.title),
        )
        .await
        {
            return false;
        }

        // A window opened in front takes the place; closed, its last word is
        // that it is not active, and a closed id never answers yes again.
        let mut third = None;
        let opened = stage(
            "open a third window",
            scenes,
            ear,
            || {
                let id = app.open(
                    WindowSpec::titled("Active third").size(240.0, 160.0),
                    Rc::new(app.runtime()),
                    text("third"),
                );
                scenes.borrow_mut().push(("Active third", id));
                third = Some(id);
            },
            |now| now.is_some() && now != Some(other) && now != Some(here),
        )
        .await;
        let third = third.expect("the third window was opened");
        if !opened {
            return false;
        }
        if !stage(
            "close it while active",
            scenes,
            ear,
            || app.close(third),
            |now| now != Some(third),
        )
        .await
        {
            return false;
        }
        assert!(!app::window_active(third), "a closed window is not active");
        true
    }

    /// The person goes to another application: no window of this one is
    /// active, and the one that was says so once.
    async fn step_back(scenes: &Scenes, ear: &Ear, previous: Option<Id>) {
        let Some(previous) = previous else {
            println!("Not qualified: no other application held the front to hand it back to");
            return;
        };
        let before = settled("step back", scenes).await;
        assert_eq!(ear.heard(), []);
        unsafe {
            let _ = activate(previous, sel("activateWithOptions:"), 0);
        }
        if !until(|| !in_front()).await {
            println!("Not qualified: the previous application did not take the front back");
            return;
        }
        let after = settled("step back", scenes).await;
        let heard = ear.heard();
        assert_eq!(after, None, "no window of a background app is active");
        assert_eq!(
            heard,
            moved(before, None),
            "step back: one event, for the window that was left"
        );
        println!("step back: {before:?} -> None, heard {heard:?}");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    if std::env::var_os("BUNNY_ACTIVE_CHILD").is_some() {
        native::run();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_ACTIVE_CHILD", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
            print!("{output}");
            assert!(
                status.success(),
                "native active-window regression: {status}"
            );
            assert!(
                output.contains(COMPLETE),
                "active-window fixture exited before completing its assertions"
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native active-window regression timed out");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
