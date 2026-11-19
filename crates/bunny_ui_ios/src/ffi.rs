//! Hand-written UIKit FFI — zero dependencies.
//!
//! This module is the shell's `unsafe` border on the phone. UIKit is
//! called through `objc_msgSend` re-declared with the concrete signature
//! of each message (the shared Apple half carries the vocabulary; every
//! module declares its own aliases), and eight classes are born at
//! runtime via `objc_allocateClassPair`/`class_addMethod`:
//!
//! - `BunnyAppDelegate` (UIResponder) — the application delegate: the
//!   launch, a memory warning, the main menu, the scene configuration
//!   that names the scene delegate, and the app's two clocks (the caret
//!   blink, the display link) delivered by selector;
//! - `BunnySceneDelegate` (UIResponder) — the window scene's delegate:
//!   the connection that builds (or re-homes) the window and runs the
//!   mount, the background and foreground, a url handed over. An app
//!   built with the iOS 27 SDK that adopts no scene life cycle is
//!   trapped by UIKit before its launch returns, so the window lives
//!   here and nowhere else;
//! - `BunnyViewController` (UIViewController) — the safe area and the
//!   traits (dark, the size class), which UIKit tells the controller;
//! - `BunnyView` (UIView) — whose backing layer IS a `CAMetalLayer`
//!   (`+layerClass`), and which hears every touch. It is no text view
//!   itself: AutoFill looks for the boxes of a form among the text views
//!   inside the view controller's view, and stops at the first one it
//!   meets — a `BunnyView` that took text would hide every box inside it;
//! - `BunnyKeyInput` (UIView) — the keyboard's doors, two empty views
//!   inside `BunnyView`, the responders the soft keyboard types into.
//!   They adopt the whole of `UITextInput`, not only `UIKeyInput`: the
//!   keyboard's input assistant — the QuickType bar, the Passwords key,
//!   the one-time code from Messages — attaches only to a responder that
//!   can say what the focused field holds, where its caret stands and what
//!   is being composed. A door answers from a mirror of that field the
//!   shell syncs after every frame ([`sync_text`]). When the focus moves
//!   from one field to another with the keyboard up, the keyboard moves to
//!   the other door: the input system then meets each field as a responder
//!   of its own, since AutoFill keeps what it learned of a responder for
//!   as long as that responder holds the keyboard;
//! - `BunnyAutofillBox` (UIView) — the other boxes of the focused field's
//!   form, one hidden view each, while a box of the form holds the
//!   keyboard ([`sync_autofill`]). AutoFill pairs a name box with a
//!   password box only across separate text views, and every box of the
//!   scene types through the doors; so each other box gets a view of its
//!   own, wearing its `textContentType` and its frame, and one Passwords
//!   pick fills the form. A proxy answers the same `UITextInput` from its
//!   box's text, never takes the keyboard, a touch or VoiceOver, draws
//!   nothing, and hands what the system writes into it back to its box by
//!   path ([`AppEvent::Fill`]);
//! - `BunnyTextPosition` (UITextPosition) and `BunnyTextRange`
//!   (UITextRange) — an offset, and a pair of them, in UTF-16: the
//!   objects the keyboard holds and hands back. Each carries its
//!   offsets in instance variables, the one state these classes keep.
//!
//! UIKit counts from the top-left in points, like the layout: no flip
//! happens anywhere here. Callbacks reach the Rust world through a
//! thread-local handler (UIKit's main run loop is one thread, like the
//! rest of the engine), and an event raised from inside a handler waits
//! its turn instead of re-entering one.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::{Arc, Once};

use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;
use bunny_ui::text_input::{byte_to_utf16, utf16_to_byte};
use bunny_ui_apple::ffi::{
    CGPoint, CGRect, Id, NS_NOT_FOUND, NSRange, NSRunLoopCommonModes, ObjcSuper, Sel,
    class, class_addMethod, class_addProtocol, modifiers_of, objc_allocateClassPair,
    objc_getProtocol, objc_registerClassPair, object_getClass, sel, text_argument_to_string,
};
use bunny_ui_apple::metal::MetalPresenter;

/// `UIEdgeInsets` — top, left, bottom, right, in points.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UIEdgeInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIApplicationMain(argc: i32, argv: *mut *mut c_char, principal: Id, delegate: Id) -> i32;
    fn UIAccessibilityIsReduceMotionEnabled() -> bool;
    /// The keyboard is about to move — its frame after the move rides
    /// the note's user info.
    static UIKeyboardWillChangeFrameNotification: Id;
    static UIKeyboardWillHideNotification: Id;
    static UIKeyboardFrameEndUserInfoKey: Id;
}

// The same trampoline discipline as every module of the Apple half: one
// alias per concrete message signature. Structs of four doubles (a rect,
// the insets) come back through memory on arm64, and the compiler
// arranges that from the declared return type.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_id(obj: Id, sel: Sel, a: Id, b: Id);
    #[link_name = "objc_msgSend"]
    fn msg_bool(obj: Id, sel: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_f64(obj: Id, sel: Sel) -> f64;
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
    #[link_name = "objc_msgSend"]
    fn msg_u64(obj: Id, sel: Sel) -> u64;
    #[link_name = "objc_msgSend"]
    fn msg_rect(obj: Id, sel: Sel) -> CGRect;
    #[link_name = "objc_msgSend"]
    fn msg_insets(obj: Id, sel: Sel) -> UIEdgeInsets;
    #[link_name = "objc_msgSend"]
    fn msg_point_id(obj: Id, sel: Sel, a: Id) -> CGPoint;
    #[link_name = "objc_msgSend"]
    fn msg_rect_rect_id(obj: Id, sel: Sel, rect: CGRect, view: Id) -> CGRect;
    #[link_name = "objc_msgSend"]
    fn msg_init_rect(obj: Id, sel: Sel, rect: CGRect) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_rect(obj: Id, sel: Sel, rect: CGRect);
    #[link_name = "objc_msgSend"]
    fn msg_void_f64(obj: Id, sel: Sel, a: f64);
    #[link_name = "objc_msgSend"]
    fn msg_id_id(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_id(obj: Id, sel: Sel, a: Id, b: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_sel(obj: Id, sel: Sel, target: Id, selector: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_cstr(obj: Id, sel: Sel) -> *const c_char;
    #[link_name = "objc_msgSend"]
    fn msg_void_sel_id_f64(obj: Id, sel: Sel, selector: Sel, arg: Id, delay: f64);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_sel_id_id(obj: Id, sel: Sel, observer: Id, selector: Sel, name: Id, object: Id);
    #[link_name = "objc_msgSend"]
    fn msg_timer(
        obj: Id,
        sel: Sel,
        interval: f64,
        target: Id,
        selector: Sel,
        info: Id,
        repeats: i8,
    ) -> Id;
    #[link_name = "objc_msgSendSuper"]
    fn msg_super_void(sup: *const ObjcSuper, sel: Sel);
    #[link_name = "objc_msgSendSuper"]
    fn msg_super_void_id(sup: *const ObjcSuper, sel: Sel, a: Id);
    #[link_name = "objc_msgSendSuper"]
    fn msg_super_void_id_id(sup: *const ObjcSuper, sel: Sel, a: Id, b: Id);
}

// MARK: - Events

/// What the platform delivers to the Rust world. Positions in LAYOUT
/// coordinates (origin at top-left, logical points) — UIKit's own.
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// The view laid out (a rotation, the first show) — present a frame.
    Redraw,
    /// A task woke from somewhere else — a worker thread finished a
    /// step. The frame the shell already knows how to draw drains the
    /// queue on its way.
    Wake,
    /// The app left the screen: nothing presents until it is back.
    Background,
    /// The app is back on the screen.
    Foreground,
    /// The system wants memory back — drop what can be re-made.
    MemoryWarning,
    /// The traits moved: dark or light (`None` = unspecified), and
    /// whether the width is compact.
    Traits { dark: Option<bool>, compact: bool },
    /// The safe area moved — a rotation, a bar that came or went.
    SafeArea { top: f64, left: f64, bottom: f64, right: f64 },
    /// The soft keyboard covers this many points of the view's bottom
    /// (0 when hidden).
    Keyboard { overlap: f64 },
    TouchBegan { id: u64, x: f64, y: f64, taps: u8 },
    TouchMoved { id: u64, x: f64, y: f64 },
    TouchEnded { id: u64, x: f64, y: f64 },
    TouchCancelled { id: u64 },
    /// Text the keyboard typed — the soft keyboard's and the hardware
    /// one's, one road. A return arrives as `"\n"`.
    Text(String),
    /// The keyboard's backspace.
    DeleteBackward,
    /// The keyboard's composition: the marked text and the selection
    /// INSIDE it, in UTF-16 (`setMarkedText:selectedRange:`). An empty
    /// text is a composition withdrawn.
    Marked { text: String, location: u64, length: u64 },
    /// The composition ends, committed as it stands (`unmarkText`).
    Unmark,
    /// The keyboard placed the selection — UTF-16 over the whole text
    /// (`setSelectedTextRange:`; the space bar's trackpad).
    Select { location: u64, length: u64 },
    /// The keyboard replaces a range of the text (`replaceRange:withText:`).
    Replace { location: u64, length: u64, text: String },
    /// The system's AutoFill wrote a box of the form that does not hold
    /// the keyboard, through the box's proxy ([`sync_autofill`]): the
    /// box's path, and its whole text now.
    Fill { field: String, text: Filled },
    /// A hardware key the keymap gate did not take, that types nothing
    /// — an arrow, a forward delete, an escape, a chord with command.
    Key(KeyStroke),
    /// A hardware keyboard's modifier keys moved — what the hand holds
    /// now. The release makes no stroke; this is the only report of it.
    Modifiers(bunny_ui::action::Modifiers),
    /// The caret's blink half-period.
    Blink,
    /// One display-link tick; `dt` seconds since the last, clamped.
    /// `dt` is the animations' step, clamped; `elapsed` the seconds since
    /// the last beat by the wall, which the hand's clock reads.
    Frame { dt: f64, elapsed: f64 },
}

/// A box's text as AutoFill wrote it — as often as not a saved password,
/// whole. Its `Debug`, which the trace prints, says how long it is and
/// nothing more, and its bytes are overwritten when it is dropped.
#[derive(Clone)]
pub struct Filled(String);

impl Filled {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Filled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} chars>", self.0.chars().count())
    }
}

impl Drop for Filled {
    fn drop(&mut self) {
        // `black_box` keeps the optimizer from deciding a write to memory
        // about to be released is dead (`bunny_ui::autofill::SavedPassword`)
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
    }
}

/// One hardware key press, in the terms the keymap reads.
#[derive(Clone, Debug)]
pub struct KeyStroke {
    /// The USB HID usage code of the key (`UIKey.keyCode`).
    pub hid: u64,
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
    /// What the key types with no modifier applied, from the layout.
    pub chars_ignoring: String,
    /// The character this key TYPED under the modifiers held.
    pub typed: Option<char>,
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
    /// The keymap gate — asked first about every hardware key.
    static KEY_GATE: RefCell<Option<Box<dyn FnMut(&KeyStroke) -> bool>>> =
        const { RefCell::new(None) };
    /// A handler is running: anything raised from inside it queues.
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
    /// What was raised while a handler ran, in the order it was raised.
    static PENDING: RefCell<Vec<AppEvent>> = const { RefCell::new(Vec::new()) };
    /// What the shell runs once the app has launched: the mount.
    static BOOT: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static WINDOW: Cell<Id> = const { Cell::new(null_mut()) };
    static VIEW: Cell<Id> = const { Cell::new(null_mut()) };
    static DELEGATE: Cell<Id> = const { Cell::new(null_mut()) };
    /// The display link — one per app, born paused.
    static LINK: Cell<Id> = const { Cell::new(null_mut()) };
    /// The slow beat: a repeating timer and its interval.
    static SLOW: Cell<(Id, f64)> = const { Cell::new((null_mut(), 0.0)) };
    /// The window's presenter, once the GPU road came up.
    static PRESENTER: RefCell<Option<MetalPresenter>> = const { RefCell::new(None) };
    /// The app is off the screen: presenting would get it killed.
    static BACKGROUNDED: Cell<bool> = const { Cell::new(false) };
    /// Whether the soft keyboard is wanted — the view claims or drops
    /// the first responder when this flips.
    static KEYBOARD_WANTED: Cell<bool> = const { Cell::new(false) };
    /// The keys the soft keyboard lays out, as the `UIKeyboardType` the
    /// view answers to `keyboardType` — the focused field's
    /// (`TextField::keyboard_type`).
    static KEYBOARD_KIND: Cell<i64> = const { Cell::new(0) };
    /// What the focused field holds (`TextField::autocomplete`) — kept to
    /// resolve the kit's constant only when it changes.
    static HOLDS: Cell<Option<bunny_ui::text_input::Autocomplete>> = const { Cell::new(None) };
    /// The `UITextContentType` constant the view answers to
    /// `textContentType` — null when the focused field says nothing.
    static CONTENT_TYPE: Cell<Id> = const { Cell::new(null_mut()) };
    /// Whether the focused field hides what it holds — the view's
    /// `isSecureTextEntry` (`TextField::secret`).
    static SECURE: Cell<bool> = const { Cell::new(false) };
}

/// Registers who receives the events (the shell's loop).
pub fn set_handler(handler: Box<dyn FnMut(AppEvent)>) {
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

/// Registers the keymap gate: `true` = the stroke was spent.
pub fn set_key_gate(gate: Box<dyn FnMut(&KeyStroke) -> bool>) {
    KEY_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

/// Delivers an event to the handler. An event raised from INSIDE a
/// handler waits its turn instead of re-entering one: the handler is
/// borrowed for as long as it runs, and this is an `extern "C"` frame
/// — a borrow panic here cannot unwind, so it would abort the process.
/// Queued, not dropped: the second event still arrives, after the
/// first finishes, in the order it was raised.
pub fn dispatch(event: AppEvent) {
    if DISPATCHING.with(Cell::get) {
        PENDING.with(|queue| queue.borrow_mut().push(event));
        return;
    }
    DISPATCHING.with(|flag| flag.set(true));
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
    DISPATCHING.with(|flag| flag.set(false));
    loop {
        let next = PENDING.with(|queue| {
            let mut queue = queue.borrow_mut();
            if queue.is_empty() { None } else { Some(queue.remove(0)) }
        });
        let Some(event) = next else { break };
        dispatch(event);
    }
}

/// A knock from another thread lands here, on the main thread, as one
/// more beat.
extern "C" fn perform_wake(_info: *mut c_void) {
    dispatch(AppEvent::Wake);
}

// MARK: - The application

/// Hands the process to UIKit. `boot` runs once the app has launched
/// and the first scene connected its window — it mounts the scene.
/// UIKit's main loop never returns.
pub fn run(boot: Box<dyn FnOnce()>) -> ! {
    unsafe {
        register_classes();
        BOOT.with(|slot| *slot.borrow_mut() = Some(boot));
        // the delegate's name, owned for the life of the process — no
        // pool stands before UIKit's own, so nothing here autoreleases
        let name = CString::new("BunnyAppDelegate").expect("class name");
        let delegate = msg_id_cstr(
            msg_id(class("NSString"), sel("alloc")),
            sel("initWithUTF8String:"),
            name.as_ptr(),
        );
        let mut argv: [*mut c_char; 1] = [null_mut()];
        UIApplicationMain(0, argv.as_mut_ptr(), null_mut(), delegate);
    }
    unreachable!("UIApplicationMain never returns")
}

/// The launch: the beat starts and the process-wide doors open. The
/// window is NOT built here — it belongs to the scene that connects
/// next ([`bunny_scene_connect`]), and so does the mount.
extern "C" fn bunny_did_finish_launching(this: Id, _sel: Sel, _app: Id, _options: Id) -> i8 {
    unsafe {
        // on an SDK before 27, a bundle with no scene manifest launches
        // the old way and no scene ever connects: a window that never
        // comes is a blank screen that looks like a hang, so say it here
        let bundle = msg_id(class("NSBundle"), sel("mainBundle"));
        let key = ns_string("UIApplicationSceneManifest");
        if msg_id_id(bundle, sel("objectForInfoDictionaryKey:"), key).is_null() {
            eprintln!(
                "bunny_ui ios: the bundle's Info.plist declares no UIApplicationSceneManifest — \
                 the window is built when a scene connects, and without the manifest none ever does"
            );
            std::process::abort();
        }
        DELEGATE.with(|slot| slot.set(this));
        // the center learns its delegate BEFORE the launch returns: a
        // tap on a notification can be the very thing that launched
        // the process, and the response arrives the moment this returns
        bunny_ui_apple::notifications::install_delegate(this);
        bunny_ui_apple::ffi::install_wake_source(perform_wake);
        start_beat(this);
        // the keyboard's own word on where it is
        let center = msg_id(class("NSNotificationCenter"), sel("defaultCenter"));
        msg_void_id_sel_id_id(
            center,
            sel("addObserver:selector:name:object:"),
            this,
            sel("bunnyKeyboard:"),
            UIKeyboardWillChangeFrameNotification,
            null_mut(),
        );
        msg_void_id_sel_id_id(
            center,
            sel("addObserver:selector:name:object:"),
            this,
            sel("bunnyKeyboard:"),
            UIKeyboardWillHideNotification,
            null_mut(),
        );
    }
    1
}

/// Which delegate a connecting scene gets — the framework's own, named
/// here rather than in the app's `Info.plist`, so the manifest the app
/// declares needs no class name from inside the shell.
extern "C" fn bunny_scene_configuration(_this: Id, _sel: Sel, _app: Id, session: Id, _options: Id) -> Id {
    unsafe {
        let configuration = msg_id_id_id(
            class("UISceneConfiguration"),
            sel("configurationWithName:sessionRole:"),
            ns_string("Default"),
            msg_id(session, sel("role")),
        );
        msg_void_id(configuration, sel("setDelegateClass:"), class("BunnySceneDelegate"));
        configuration
    }
}

/// A scene connected: the window is built inside it, the mount runs,
/// and the window shows. A scene the system DISCONNECTED (to reclaim a
/// background scene's memory) comes back as a new scene for the same
/// process — the window, the view and the GPU road it carries survive,
/// and are re-homed into it instead of built twice.
extern "C" fn bunny_scene_connect(_this: Id, _sel: Sel, scene: Id, _session: Id, options: Id) {
    unsafe {
        let window = WINDOW.with(Cell::get);
        if window.is_null() {
            let window = msg_id_id(msg_id(class("UIWindow"), sel("alloc")), sel("initWithWindowScene:"), scene);
            let bounds = msg_rect(window, sel("bounds"));
            let controller = msg_id(msg_id(class("BunnyViewController"), sel("alloc")), sel("init"));
            let view =
                msg_init_rect(msg_id(class("BunnyView"), sel("alloc")), sel("initWithFrame:"), bounds);
            // off, a view is handed exactly one touch and the second finger
            // is never reported at all
            msg_void_bool(view, sel("setMultipleTouchEnabled:"), 1);
            msg_void_id(controller, sel("setView:"), view);
            // the keyboard's two doors: empty views inside this one, that
            // no finger reaches ([`bunny_renew_keyboard`]); the first holds
            // the keyboard first. The view keeps them, and so do we, for
            // the life of the app
            let doors = [DOOR_OPEN, NO_RECT].map(|frame| {
                let door = msg_init_rect(msg_id(class("BunnyKeyInput"), sel("alloc")), sel("initWithFrame:"), frame);
                msg_void_bool(door, sel("setUserInteractionEnabled:"), 0);
                msg_void_id(view, sel("addSubview:"), door);
                door
            });
            DOORS.with(|slot| slot.set(doors));
            msg_void_id(window, sel("setRootViewController:"), controller);
            WINDOW.with(|slot| slot.set(window));
            VIEW.with(|slot| slot.set(view));
            // the mount: the GPU road, the handler, the gates
            if let Some(boot) = BOOT.with(|slot| slot.borrow_mut().take()) {
                boot();
            }
            msg_void(window, sel("makeKeyAndVisible"));
        } else {
            msg_void_id(window, sel("setWindowScene:"), scene);
            msg_void(window, sel("makeKeyAndVisible"));
        }
        // a url that launched the app rides the connection, and its own
        // door is never knocked for it
        reopen_with(msg_id(options, sel("URLContexts")));
    }
    dispatch(AppEvent::Redraw);
}

/// Urls handed to a scene that is already connected.
extern "C" fn bunny_scene_open_urls(_this: Id, _sel: Sel, _scene: Id, contexts: Id) {
    unsafe { reopen_with(contexts) };
}

/// Each url of a set of `UIOpenURLContext`s, as the deep link — the same
/// event a second launch delivers everywhere else.
unsafe fn reopen_with(contexts: Id) {
    for context in unsafe { members_of(contexts) } {
        let text = unsafe {
            let string = msg_id(msg_id(context, sel("URL")), sel("absoluteString"));
            let chars = msg_cstr(string, sel("UTF8String"));
            if chars.is_null() { String::new() } else { CStr::from_ptr(chars).to_string_lossy().into_owned() }
        };
        let _ = bunny_ui::app::emit(bunny_ui::app::AppEvent::Reopened { arguments: vec![text] });
    }
}

extern "C" fn bunny_did_enter_background(_this: Id, _sel: Sel, _scene: Id) {
    BACKGROUNDED.with(|slot| slot.set(true));
    // the link parks: a frame drawn off the screen is a frame the
    // system kills the app for
    set_frame_driver(DriverPace::Off);
    dispatch(AppEvent::Background);
    let _ = bunny_ui::app::emit(bunny_ui::app::AppEvent::WillSleep);
}

/// The scene is coming back. A scene ALSO enters the foreground on its
/// first connection, which is a launch and not a return: only an app
/// that went to the background hears that it came back.
extern "C" fn bunny_will_enter_foreground(_this: Id, _sel: Sel, _scene: Id) {
    if !BACKGROUNDED.with(|slot| slot.replace(false)) {
        return;
    }
    dispatch(AppEvent::Foreground);
    let _ = bunny_ui::app::emit(bunny_ui::app::AppEvent::DidWake);
}

extern "C" fn bunny_memory_warning(_this: Id, _sel: Sel, _app: Id) {
    dispatch(AppEvent::MemoryWarning);
}

// MARK: - The beat

/// Starts the app's beat: the caret's blink half-period and the display
/// link that paces animation, both delivered by selector to the
/// delegate on the main run loop. The link is born PAUSED — events
/// repaint by themselves and the link runs only while something moves.
unsafe fn start_beat(delegate: Id) {
    unsafe {
        let _ = msg_timer(
            class("NSTimer"),
            sel("scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:"),
            0.5,
            delegate,
            sel("bunnyBlink:"),
            null_mut(),
            1,
        );
        let link = msg_id_id_sel(
            class("CADisplayLink"),
            sel("displayLinkWithTarget:selector:"),
            delegate,
            sel("bunnyFrame:"),
        );
        if link.is_null() {
            eprintln!("bunny_ui ios: no display link; animations snap");
            return;
        }
        msg_void_bool(link, sel("setPaused:"), 1);
        msg_void_id_id(
            link,
            sel("addToRunLoop:forMode:"),
            msg_id(class("NSRunLoop"), sel("mainRunLoop")),
            NSRunLoopCommonModes,
        );
        LINK.with(|slot| slot.set(link));
    }
}

extern "C" fn bunny_blink(_this: Id, _sel: Sel, _timer: Id) {
    dispatch(AppEvent::Blink);
}

extern "C" fn bunny_slow(_this: Id, _sel: Sel, _timer: Id) {
    let step = SLOW.with(|slot| slot.get().1);
    dispatch(AppEvent::Frame { dt: step, elapsed: step });
}

extern "C" fn bunny_frame(_this: Id, _sel: Sel, link: Id) {
    let (dt, stamp) = unsafe {
        let last = msg_f64(link, sel("timestamp"));
        let next = msg_f64(link, sel("targetTimestamp"));
        // the first tick after a resume reports the whole pause as the
        // gap — a clamped step keeps springs continuous
        ((next - last).clamp(0.0, 1.0 / 30.0), last)
    };
    // the wall's gap between two frames that were DRAWN: a scene that
    // takes a hundred milliseconds a frame skips the display's beats, and
    // the link's own step would count every skipped one as nothing
    let elapsed = LAST_STAMP
        .with(|slot| slot.replace(Some(stamp)))
        .map_or(dt, |previous| (stamp - previous).max(0.0));
    dispatch(AppEvent::Frame { dt, elapsed });
}

thread_local! {
    /// The display link's timestamp at the last beat — forgotten when the
    /// link pauses, so a resume never reports the nap as a gap.
    static LAST_STAMP: Cell<Option<f64>> = const { Cell::new(None) };
}

/// The keyboard moved: its frame after the move, in screen coordinates,
/// against the view's bottom edge — the overlap is what the layout must
/// give up.
extern "C" fn bunny_keyboard(_this: Id, _sel: Sel, note: Id) {
    // the keyboard handed back and taken again in one turn stays on the
    // screen, but UIKit still posts the hide on the way: heard, the scene
    // would drop to the bottom for a frame and climb back
    if RENEWING.with(Cell::get) {
        return;
    }
    let overlap = unsafe {
        let view = VIEW.with(Cell::get);
        if view.is_null() {
            return;
        }
        let info = msg_id(note, sel("userInfo"));
        let value = msg_id_id(info, sel("objectForKey:"), UIKeyboardFrameEndUserInfoKey);
        if value.is_null() {
            0.0
        } else {
            let frame = msg_rect(value, sel("CGRectValue"));
            let local = msg_rect_rect_id(view, sel("convertRect:fromView:"), frame, null_mut());
            let bounds = msg_rect(view, sel("bounds"));
            (bounds.size.height - local.origin.y).clamp(0.0, bounds.size.height)
        }
    };
    dispatch(AppEvent::Keyboard { overlap });
}

/// How fast the shell drives frames — the ffi twin of the runtime's
/// pace, chosen after every present.
#[derive(Clone, Copy, PartialEq)]
pub enum DriverPace {
    /// The display link runs — springs, flights and fingers are moving.
    Full,
    /// Only loop clocks live: a repeating timer beats once per step.
    Slow(f64),
    /// Nothing moves.
    Off,
}

/// Points the frame driver at the pace the moment deserves.
pub fn set_frame_driver(pace: DriverPace) {
    let full = pace == DriverPace::Full && !BACKGROUNDED.with(Cell::get);
    if !full {
        LAST_STAMP.with(|slot| slot.set(None));
    }
    LINK.with(|slot| {
        let link = slot.get();
        if !link.is_null() {
            unsafe { msg_void_bool(link, sel("setPaused:"), (!full) as i8) };
        }
    });
    SLOW.with(|slot| {
        let (timer, interval) = slot.get();
        match pace {
            DriverPace::Slow(wanted) => {
                if !timer.is_null() && (interval - wanted).abs() < f64::EPSILON {
                    return;
                }
                if !timer.is_null() {
                    unsafe { msg_void(timer, sel("invalidate")) };
                }
                let delegate = DELEGATE.with(Cell::get);
                if delegate.is_null() {
                    return;
                }
                let fresh = unsafe {
                    msg_timer(
                        class("NSTimer"),
                        sel("scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:"),
                        wanted,
                        delegate,
                        sel("bunnySlow:"),
                        null_mut(),
                        1,
                    )
                };
                slot.set((fresh, wanted));
            }
            DriverPace::Full | DriverPace::Off => {
                if !timer.is_null() {
                    unsafe { msg_void(timer, sel("invalidate")) };
                    slot.set((null_mut(), 0.0));
                }
            }
        }
    });
}

// MARK: - The view: touches, layout, the keyboard

/// The members of an `NSSet`, by address — the touches or presses of
/// an event, the urls a scene was handed.
unsafe fn members_of(set: Id) -> Vec<Id> {
    unsafe {
        if set.is_null() {
            return Vec::new();
        }
        let all = msg_id(set, sel("allObjects"));
        let count = msg_u64(all, sel("count"));
        (0..count).map(|index| msg_id_u64(all, sel("objectAtIndex:"), index)).collect()
    }
}

unsafe fn touch_point(touch: Id, view: Id) -> (f64, f64) {
    let point = unsafe { msg_point_id(touch, sel("locationInView:"), view) };
    (point.x, point.y)
}

extern "C" fn bunny_touches_began(this: Id, _sel: Sel, touches: Id, _event: Id) {
    for touch in unsafe { members_of(touches) } {
        let (x, y) = unsafe { touch_point(touch, this) };
        let taps = unsafe { msg_u64(touch, sel("tapCount")) }.clamp(1, u8::MAX as u64) as u8;
        dispatch(AppEvent::TouchBegan { id: touch as u64, x, y, taps });
    }
}

/// `touchesMoved:` carries only the fingers that moved; a finger held
/// still during a pinch is in the EVENT's set and nowhere else — so the
/// event is asked, and every live finger reports where it stands.
extern "C" fn bunny_touches_moved(this: Id, _sel: Sel, _touches: Id, event: Id) {
    let all = unsafe { members_of(msg_id(event, sel("allTouches"))) };
    for touch in all {
        // UITouchPhase: began 0, moved 1, stationary 2, ended 3, cancelled 4
        let phase = unsafe { msg_i64(touch, sel("phase")) };
        if phase == 1 || phase == 2 {
            let (x, y) = unsafe { touch_point(touch, this) };
            dispatch(AppEvent::TouchMoved { id: touch as u64, x, y });
        }
    }
}

extern "C" fn bunny_touches_ended(this: Id, _sel: Sel, touches: Id, _event: Id) {
    for touch in unsafe { members_of(touches) } {
        let (x, y) = unsafe { touch_point(touch, this) };
        dispatch(AppEvent::TouchEnded { id: touch as u64, x, y });
    }
}

extern "C" fn bunny_touches_cancelled(_this: Id, _sel: Sel, touches: Id, _event: Id) {
    for touch in unsafe { members_of(touches) } {
        dispatch(AppEvent::TouchCancelled { id: touch as u64 });
    }
}

/// `+layerClass` — the view's backing layer is a `CAMetalLayer`, so
/// there is no graft: UIKit makes the layer, the presenter attaches.
extern "C" fn bunny_layer_class(_this: Id, _sel: Sel) -> Id {
    unsafe { class("CAMetalLayer") }
}

/// The view laid out — a rotation, a split, the first show. One funnel:
/// the presenter re-reads the bounds and the scale on the next present.
extern "C" fn bunny_layout_subviews(this: Id, _sel: Sel) {
    unsafe {
        let sup = ObjcSuper { receiver: this, class: class("UIView") };
        msg_super_void(&sup, sel("layoutSubviews"));
    }
    dispatch(AppEvent::Redraw);
}

extern "C" fn bunny_yes(_this: Id, _sel: Sel) -> i8 {
    1
}

/// `UITextInputTraits` answered by hand: an empty adoption leaves every
/// getter nil, and the keyboard then guesses — sentence capitals and
/// autocorrection on a field that wanted neither. The traits say NO
/// (the enum's `…No` is 1) and `autocapitalizationType` says none (0);
/// `keyboardType` is the focused field's ([`want_keys`]). The traits a
/// view meets only once it adopts `UITextInput` — inline predictions,
/// math completion, Writing Tools — say no as well: adopting it is for
/// the input assistant's AutoFill, not for a keyboard that rewrites.
extern "C" fn bunny_trait_no(_this: Id, _sel: Sel) -> i64 {
    1
}

extern "C" fn bunny_trait_zero(_this: Id, _sel: Sel) -> i64 {
    0
}

/// `writingToolsBehavior` — `UIWritingToolsBehaviorNone` is -1.
extern "C" fn bunny_trait_none(_this: Id, _sel: Sel) -> i64 {
    -1
}

extern "C" fn bunny_keyboard_type(_this: Id, _sel: Sel) -> i64 {
    KEYBOARD_KIND.with(Cell::get)
}

/// `isSecureTextEntry` — the focused field's `secret` ([`want_traits`]):
/// a password box reaches the system as one, so the keyboard learns
/// nothing from it and Passwords offers to fill it. Only the door that
/// holds the keyboard is the field; the other is no box of any kind. A
/// proxy answers for its own box ([`sync_autofill`]).
extern "C" fn bunny_secure_entry(this: Id, _sel: Sel) -> i8 {
    let secret = with_proxy(this, |proxy| proxy.secret)
        .unwrap_or_else(|| this == door() && SECURE.with(Cell::get));
    i8::from(secret)
}

/// `textContentType` — what the focused field holds, in UIKit's words
/// ([`want_traits`]); nil when it says nothing, and nil from the door
/// that does not hold the keyboard — AutoFill looks over every text view
/// in the window for its group, and must not find the field twice. A
/// proxy says what its own box holds: it is how the group finds the
/// form's other boxes.
extern "C" fn bunny_content_type(this: Id, _sel: Sel) -> Id {
    with_proxy(this, |proxy| proxy.content_type)
        .unwrap_or_else(|| if this == door() { CONTENT_TYPE.with(Cell::get) } else { null_mut() })
}

/// `hasText` — a door always has a field behind it; a proxy has text
/// when its box holds some.
extern "C" fn bunny_has_text(this: Id, _sel: Sel) -> i8 {
    i8::from(with_proxy(this, |proxy| proxy.mirror.units > 0).unwrap_or(true))
}

/// The keys change under a keyboard that is up: UIKit read `keyboardType`
/// when the view took the first responder and reads it again only when
/// asked to. A keyboard on its way down is left to go.
extern "C" fn bunny_reload_keys(_this: Id, _sel: Sel) {
    if !KEYBOARD_WANTED.with(Cell::get) {
        return;
    }
    let door = door();
    unsafe {
        if msg_bool(door, sel("isFirstResponder")) != 0 {
            msg_void(door, sel("reloadInputViews"));
        }
    }
}

extern "C" fn bunny_insert_text(this: Id, _sel: Sel, text: Id) {
    let text = unsafe { text_argument_to_string(text) };
    if text.is_empty() {
        return;
    }
    if is_proxy(this) {
        proxy_write(this, Write::Insert(text));
    } else {
        keyboard_edit(this, AppEvent::Text(text));
    }
}

extern "C" fn bunny_delete_backward(this: Id, _sel: Sel) {
    if is_proxy(this) {
        proxy_write(this, Write::DeleteBackward);
    } else {
        keyboard_edit(this, AppEvent::DeleteBackward);
    }
}

/// The door the keyboard types through now — the first until a renewal
/// moves it ([`bunny_renew_keyboard`]).
fn door() -> Id {
    let door = DOOR.with(Cell::get);
    if door.is_null() { DOORS.with(Cell::get)[0] } else { door }
}

extern "C" fn bunny_claim_keyboard(_this: Id, _sel: Sel) {
    unsafe {
        let _ = msg_bool(door(), sel("becomeFirstResponder"));
    }
}

extern "C" fn bunny_drop_keyboard(_this: Id, _sel: Sel) {
    unsafe {
        let _ = msg_bool(door(), sel("resignFirstResponder"));
    }
}

/// The focus moved to another field with the keyboard up: the keyboard
/// moves to the other door, which UIKit keeps on the screen as a switch
/// of responders, and the input system meets the new field as a new
/// responder. It has to be another object. AutoFill keeps the group of
/// boxes it found for a responder for as long as that responder holds the
/// keyboard, by identity, and handing the keyboard back and taking it
/// again does not clear it. With one door, a pick made in the address
/// box after the password box held the keyboard typed the password into
/// the address box, in the clear. `reloadInputViews` lays the keys out
/// again and nothing more.
extern "C" fn bunny_renew_keyboard(_this: Id, _sel: Sel) {
    if !KEYBOARD_WANTED.with(Cell::get) {
        return;
    }
    let [first, second] = DOORS.with(Cell::get);
    let held = door();
    if second.is_null() || unsafe { msg_bool(held, sel("isFirstResponder")) } == 0 {
        return;
    }
    let next = if held == first { second } else { first };
    // the door answers as the field before UIKit asks it anything, and
    // only the door that holds the keyboard has a size ([`DOOR_OPEN`])
    DOOR.with(|slot| slot.set(next));
    unsafe {
        msg_void_rect(next, sel("setFrame:"), DOOR_OPEN);
        msg_void_rect(held, sel("setFrame:"), NO_RECT);
    }
    RENEWING.with(|flag| flag.set(true));
    unsafe {
        let _ = msg_bool(next, sel("becomeFirstResponder"));
    }
    RENEWING.with(|flag| flag.set(false));
}

// MARK: - The document the keyboard reads (UITextInput)

/// The focused field as the input system reads it — what `UITextInput`
/// answers from on the spot. The shell re-syncs it after every frame it
/// presents ([`sync_text`]); the keyboard's edits travel the other way,
/// as events. Offsets are UTF-16 over the whole text, the kit's
/// vocabulary; the caret is in the view's points, which are the layout's.
#[derive(Clone)]
pub struct Document {
    /// The field's path: a focus that moved is a new document even when
    /// the two texts read the same.
    pub field: String,
    /// The field's own text, shared — never copied for the mirror.
    pub text: Arc<str>,
    /// (location, length) of the selection; a caret has length 0.
    pub selected: (u64, u64),
    /// (location, length) of a live composition.
    pub marked: Option<(u64, u64)>,
    /// The caret, for a question the field's geometry cannot answer.
    pub caret: CGRect,
}

/// The document and its text's length in UTF-16 — counted once per text,
/// not once per frame.
struct Mirror {
    document: Document,
    units: u64,
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// Room for an instance variable on a class not yet registered —
    /// how a position carries its offset.
    fn class_addIvar(class: Id, name: *const c_char, size: usize, alignment: u8, types: *const c_char) -> i8;
    fn class_getInstanceVariable(class: Id, name: *const c_char) -> Id;
    fn ivar_getOffset(ivar: Id) -> isize;
    /// ARC's weak reference, by hand: the cell is zeroed when the object
    /// goes, which is what UIKit's `weak` on `inputDelegate` promises.
    fn objc_storeWeak(location: *mut Id, object: Id) -> Id;
    fn objc_loadWeak(location: *mut Id) -> Id;
}

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_init_bytes(obj: Id, sel: Sel, bytes: *const u8, length: u64, encoding: u64) -> Id;
}

thread_local! {
    static MIRROR: RefCell<Option<Rc<Mirror>>> = const { RefCell::new(None) };
    /// The keyboard's own edit is being applied: what it changes is news
    /// the keyboard already has, and is not announced back to it.
    static KEYBOARD_EDITING: Cell<bool> = const { Cell::new(false) };
    /// The view is handing the keyboard back and taking it again for a
    /// new field ([`bunny_renew_keyboard`]): the hide UIKit posts on the
    /// way is no move of the keyboard.
    static RENEWING: Cell<bool> = const { Cell::new(false) };
    /// The input system's delegate of each door, in [`DOORS`]'s order,
    /// held weakly (`objc_storeWeak` writes these very
    /// cells, which live as long as the main thread). One cell for both
    /// would be cleared by the door UIKit lets go of after it set the
    /// other's.
    static INPUT_DELEGATES: [Cell<Id>; 2] = const { [Cell::new(null_mut()), Cell::new(null_mut())] };
    /// The keyboard's two doors: empty views inside `BunnyView` that
    /// answer the keyboard, so a field that takes the keyboard from
    /// another meets the input system as another responder
    /// ([`bunny_renew_keyboard`]).
    static DOORS: Cell<[Id; 2]> = const { Cell::new([null_mut(); 2]) };
    /// The door the keyboard types through now — null until the second
    /// door first takes it, which reads as the first ([`door`]).
    static DOOR: Cell<Id> = const { Cell::new(null_mut()) };
    /// The string tokenizer the kit asks for words and lines — one, made
    /// on the first question, for the doors.
    static TOKENIZER: Cell<Id> = const { Cell::new(null_mut()) };
    static TEXT_INDEX: RefCell<Option<IndexAt>> = const { RefCell::new(None) };
    static TEXT_RECT: RefCell<Option<RectAt>> = const { RefCell::new(None) };
    /// Where the offset of a `BunnyTextPosition`, and the two ends of a
    /// `BunnyTextRange`, sit inside an instance — read at registration.
    static POSITION_IVAR: Cell<isize> = const { Cell::new(0) };
    static RANGE_IVARS: Cell<(isize, isize)> = const { Cell::new((0, 0)) };
}

/// "Which offset sits under this point of the view?" — installed by the
/// shell, capturing the runtime; `None` off the focused field.
pub type IndexAt = Box<dyn Fn(f64, f64) -> Option<u64>>;

/// "Where does the caret stand at this offset?" — in the view's points.
pub type RectAt = Box<dyn Fn(u64) -> Option<CGRect>>;

/// The shell installs the two questions the mirror cannot answer alone:
/// the offset under a point and the caret at an offset.
pub fn set_text_resolvers(index_at: IndexAt, rect_at: RectAt) {
    TEXT_INDEX.with(|slot| *slot.borrow_mut() = Some(index_at));
    TEXT_RECT.with(|slot| *slot.borrow_mut() = Some(rect_at));
}

/// The shell syncs the document after a frame (`None` = no field holds
/// the keyboard). A change the keyboard did not make — a tap that moved
/// the caret, the focus gone to another field, the app rewriting the
/// text — is announced to the input system AROUND the swap: told only
/// after, the keyboard would go on composing over a text that is no
/// longer there, and offer what it read from the field before.
pub fn sync_text(document: Option<Document>) {
    let old = mirror();
    let (field_moved, text_moved, selection_moved) = match (old.as_deref(), document.as_ref()) {
        (None, None) => (false, false, false),
        (Some(old), Some(new)) => {
            let field_moved = old.document.field != new.field;
            (
                field_moved,
                field_moved
                    || !(Arc::ptr_eq(&old.document.text, &new.text) || old.document.text == new.text),
                old.document.selected != new.selected || old.document.marked != new.marked,
            )
        }
        _ => (false, true, true),
    };
    // another field took the keyboard: a fresh session for it, on the
    // next turn of the main queue (the handler that moved the focus is
    // still running) — see [`bunny_renew_keyboard`]
    if field_moved {
        perform_soon("bunnyRenewKeyboard");
    }
    let fresh = document.map(|document| {
        let units = match old.as_deref() {
            Some(old) if Arc::ptr_eq(&old.document.text, &document.text) => old.units,
            _ => byte_to_utf16(&document.text, document.text.len()) as u64,
        };
        Rc::new(Mirror { document, units })
    });
    let announce = (text_moved || selection_moved) && !KEYBOARD_EDITING.with(Cell::get);
    let delegate = if announce { input_delegate() } else { null_mut() };
    let view = door();
    let (will, did) = if text_moved {
        ("textWillChange:", "textDidChange:")
    } else {
        ("selectionWillChange:", "selectionDidChange:")
    };
    // no borrow is held across the calls: the delegate reads the
    // document back, and may commit its composition, which queues
    if !delegate.is_null() {
        unsafe { msg_void_id(delegate, sel(will), view) };
    }
    MIRROR.with(|slot| *slot.borrow_mut() = fresh);
    if !delegate.is_null() {
        unsafe { msg_void_id(delegate, sel(did), view) };
    }
}

fn mirror() -> Option<Rc<Mirror>> {
    MIRROR.with(|slot| slot.borrow().clone())
}

/// The document `this` answers from: a door's is the focused field's
/// mirror ([`sync_text`]), a proxy's is its own box ([`sync_autofill`]),
/// and any other object — a proxy already let go of — has none.
fn mirror_of(this: Id) -> Option<Rc<Mirror>> {
    match with_proxy(this, |proxy| Rc::clone(&proxy.mirror)) {
        Some(mirror) => Some(mirror),
        None if is_door(this) => mirror(),
        None => None,
    }
}

/// The document's length in UTF-16 — 0 without a field.
fn units_of(this: Id) -> u64 {
    mirror_of(this).map_or(0, |mirror| mirror.units)
}

/// One of the two responders the keyboard types through
/// ([`bunny_renew_keyboard`]).
fn is_door(this: Id) -> bool {
    DOORS.with(Cell::get).contains(&this)
}

/// The weak cell holding `this`'s input delegate — a door's, a proxy's,
/// and none for anything else: a proxy already let go of must not write
/// over a door's.
fn delegate_cell(this: Id) -> Option<*mut Id> {
    if let Some(cell) = with_proxy(this, |proxy| proxy.delegate.as_ptr()) {
        return Some(cell);
    }
    let door = DOORS.with(Cell::get).iter().position(|door| *door == this)?;
    Some(INPUT_DELEGATES.with(|cells| cells[door].as_ptr()))
}

/// The input delegate of the door that holds the keyboard.
fn input_delegate() -> Id {
    delegate_cell(door()).map_or(null_mut(), |cell| unsafe { objc_loadWeak(cell) })
}

/// An edit the keyboard asked for: the frame it draws syncs the
/// keyboard's own news, which is not announced back to it. One that
/// comes through the door that no longer holds the keyboard was meant
/// for the field before — its offsets count that field's text — and is
/// dropped rather than made in the field that has the keyboard now.
fn keyboard_edit(door: Id, event: AppEvent) {
    if door != self::door() {
        return;
    }
    let was = KEYBOARD_EDITING.with(|flag| flag.replace(true));
    dispatch(event);
    KEYBOARD_EDITING.with(|flag| flag.set(was));
}

/// The offset under a point of the focused field — a door's question; a
/// proxy's box has no geometry the system reads.
fn index_at(this: Id, x: f64, y: f64) -> Option<u64> {
    if !is_door(this) {
        return None;
    }
    TEXT_INDEX.with(|slot| slot.borrow().as_ref().and_then(|resolve| resolve(x, y)))
}

/// The caret at an offset of the focused field — a door's question, as
/// [`index_at`].
fn rect_at(this: Id, offset: u64) -> Option<CGRect> {
    if !is_door(this) {
        return None;
    }
    TEXT_RECT.with(|slot| slot.borrow().as_ref().and_then(|resolve| resolve(offset)))
}

const NO_RECT: CGRect = CGRect {
    origin: CGPoint { x: 0.0, y: 0.0 },
    size: CGSize { width: 0.0, height: 0.0 },
};

/// The frame of the door that holds the keyboard — a point. The other
/// door has none ([`NO_RECT`]): AutoFill takes every text view with a
/// size for a box of the form, and an empty door standing between the
/// address and the password would break their pair.
const DOOR_OPEN: CGRect = CGRect {
    origin: CGPoint { x: 0.0, y: 0.0 },
    size: CGSize { width: 1.0, height: 1.0 },
};

/// The `u64` an instance variable holds, by its offset in the instance.
unsafe fn ivar(object: Id, offset: isize) -> *mut u64 {
    unsafe { object.cast::<u8>().offset(offset).cast::<u64>() }
}

/// A `BunnyTextPosition` at `offset` — autoreleased: the kit keeps what
/// it wants of it.
unsafe fn position(offset: u64) -> Id {
    unsafe {
        let object = msg_id(msg_id(class("BunnyTextPosition"), sel("alloc")), sel("init"));
        ivar(object, POSITION_IVAR.with(Cell::get)).write(offset);
        msg_id(object, sel("autorelease"))
    }
}

/// A `BunnyTextRange` over two offsets, in either order.
unsafe fn range(from: u64, to: u64) -> Id {
    unsafe {
        let object = msg_id(msg_id(class("BunnyTextRange"), sel("alloc")), sel("init"));
        let (start, end) = RANGE_IVARS.with(Cell::get);
        ivar(object, start).write(from.min(to));
        ivar(object, end).write(from.max(to));
        msg_id(object, sel("autorelease"))
    }
}

/// The offset a position the kit hands back holds — `None` for nil, and
/// for an object this shell did not make: its memory is not ours to read.
unsafe fn offset_of(position: Id) -> Option<u64> {
    unsafe {
        let ours = !position.is_null()
            && msg_bool_id(position, sel("isKindOfClass:"), class("BunnyTextPosition")) != 0;
        ours.then(|| ivar(position, POSITION_IVAR.with(Cell::get)).read())
    }
}

/// The (start, end) of a range the kit hands back — `None` as above.
unsafe fn span_of(range: Id) -> Option<(u64, u64)> {
    unsafe {
        let ours = !range.is_null()
            && msg_bool_id(range, sel("isKindOfClass:"), class("BunnyTextRange")) != 0;
        let (start, end) = RANGE_IVARS.with(Cell::get);
        ours.then(|| (ivar(range, start).read(), ivar(range, end).read()))
    }
}

/// An autoreleased `NSString` of this text — by its bytes, so a NUL in a
/// field is text like any other, where a C string would stop at it.
unsafe fn ns_text(text: &str) -> Id {
    unsafe {
        // NSUTF8StringEncoding
        let string = msg_init_bytes(
            msg_id(class("NSString"), sel("alloc")),
            sel("initWithBytes:length:encoding:"),
            text.as_ptr(),
            text.len() as u64,
            4,
        );
        msg_id(string, sel("autorelease"))
    }
}

/// Where a direction points along the text. The kit passes a
/// `UITextLayoutDirection` (right 2, left 3, up 4, down 5) and, in a few
/// questions, a `UITextStorageDirection` (forward 0, backward 1).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Toward {
    Start,
    End,
    Up,
    Down,
}

impl Toward {
    fn of(direction: i64) -> Self {
        match direction {
            1 | 3 => Self::Start,
            4 => Self::Up,
            5 => Self::Down,
            _ => Self::End,
        }
    }

    fn back(self) -> Self {
        match self {
            Self::Start => Self::End,
            Self::End => Self::Start,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }

    /// Toward the text's end — right, down, forward.
    fn ahead(self) -> bool {
        matches!(self, Self::End | Self::Down)
    }
}

/// `steps` characters along the text from the UTF-16 offset `from` — a
/// surrogate pair is one step, never two halves. `None` past either end.
fn step_chars(text: &str, from: u64, steps: u64, ahead: bool) -> Option<u64> {
    let byte = utf16_to_byte(text, from as usize);
    let at = byte_to_utf16(text, byte) as u64;
    let walk = |chars: &mut dyn Iterator<Item = char>| {
        let mut units = 0;
        for _ in 0..steps {
            units += chars.next()?.len_utf16() as u64;
        }
        Some(units)
    };
    if ahead {
        walk(&mut text[byte..].chars()).map(|units| at + units)
    } else {
        walk(&mut text[..byte].chars().rev()).map(|units| at - units)
    }
}

/// `lines` rows up or down from an offset, through the field's own
/// geometry: the caret's rect there, a row's height over or under it,
/// the offset under that point. Off the field's first row it is the
/// start, off its last the end — where an arrow key takes a caret.
fn step_lines(this: Id, from: u64, lines: u64, up: bool, units: u64) -> u64 {
    let edge = if up { 0 } else { units };
    let mut at = from;
    for _ in 0..lines {
        let Some(caret) = rect_at(this, at) else { return edge };
        let height = caret.size.height.max(1.0);
        let y = if up { caret.origin.y - height / 2.0 } else { caret.origin.y + height * 1.5 };
        match index_at(this, caret.origin.x, y) {
            Some(next) if next != at => at = next,
            _ => return edge,
        }
    }
    at
}

// The questions (the kit's reads)

/// `textInRange:` — clamped to the text that exists; nil without a field.
extern "C" fn bunny_text_in_range(this: Id, _sel: Sel, range: Id) -> Id {
    let (Some(mirror), Some((start, end))) = (mirror_of(this), unsafe { span_of(range) }) else {
        return null_mut();
    };
    let text = &mirror.document.text;
    let from = utf16_to_byte(text, start as usize);
    let to = utf16_to_byte(text, end as usize).max(from);
    unsafe { ns_text(&text[from..to]) }
}

extern "C" fn bunny_selected_text_range(this: Id, _sel: Sel) -> Id {
    mirror_of(this).map_or(null_mut(), |mirror| {
        let (location, length) = mirror.document.selected;
        unsafe { range(location, location + length) }
    })
}

/// nil when nothing is being composed — what tells the keyboard there is
/// no composition of its own to carry on.
extern "C" fn bunny_marked_text_range(this: Id, _sel: Sel) -> Id {
    mirror_of(this)
        .and_then(|mirror| mirror.document.marked)
        .map_or(null_mut(), |(location, length)| unsafe { range(location, location + length) })
}

/// `markedTextStyle` — none: the field draws its composition its own way.
extern "C" fn bunny_marked_text_style(_this: Id, _sel: Sel) -> Id {
    null_mut()
}

extern "C" fn bunny_beginning_of_document(_this: Id, _sel: Sel) -> Id {
    unsafe { position(0) }
}

extern "C" fn bunny_end_of_document(this: Id, _sel: Sel) -> Id {
    unsafe { position(units_of(this)) }
}

extern "C" fn bunny_range_between(_this: Id, _sel: Sel, from: Id, to: Id) -> Id {
    match unsafe { (offset_of(from), offset_of(to)) } {
        (Some(from), Some(to)) => unsafe { range(from, to) },
        _ => null_mut(),
    }
}

/// `positionFromPosition:offset:` — nil before the start or past the end,
/// as the kit asks.
extern "C" fn bunny_position_offset(this: Id, _sel: Sel, from: Id, offset: i64) -> Id {
    unsafe { offset_of(from) }
        .and_then(|from| from.checked_add_signed(offset))
        .filter(|to| *to <= units_of(this))
        .map_or(null_mut(), |to| unsafe { position(to) })
}

/// `positionFromPosition:inDirection:offset:` — along the text by whole
/// characters, across it by rows.
extern "C" fn bunny_position_toward(this: Id, _sel: Sel, from: Id, direction: i64, offset: i64) -> Id {
    let (Some(mirror), Some(from)) = (mirror_of(this), unsafe { offset_of(from) }) else {
        return null_mut();
    };
    let from = from.min(mirror.units);
    let toward = if offset < 0 { Toward::of(direction).back() } else { Toward::of(direction) };
    let steps = offset.unsigned_abs();
    let to = match toward {
        Toward::Start | Toward::End => step_chars(&mirror.document.text, from, steps, toward.ahead()),
        Toward::Up | Toward::Down => Some(step_lines(this, from, steps, toward == Toward::Up, mirror.units)),
    };
    to.map_or(null_mut(), |to| unsafe { position(to) })
}

/// `comparePosition:toPosition:` — `NSComparisonResult`: -1 when the
/// first stands before the second.
extern "C" fn bunny_compare_positions(_this: Id, _sel: Sel, first: Id, second: Id) -> i64 {
    match unsafe { (offset_of(first), offset_of(second)) } {
        (Some(first), Some(second)) => first.cmp(&second) as i64,
        _ => 0,
    }
}

extern "C" fn bunny_offset_between(_this: Id, _sel: Sel, from: Id, to: Id) -> i64 {
    match unsafe { (offset_of(from), offset_of(to)) } {
        (Some(from), Some(to)) => to as i64 - from as i64,
        _ => 0,
    }
}

extern "C" fn bunny_input_delegate(this: Id, _sel: Sel) -> Id {
    delegate_cell(this).map_or(null_mut(), |cell| unsafe { objc_loadWeak(cell) })
}

extern "C" fn bunny_set_input_delegate(this: Id, _sel: Sel, delegate: Id) {
    if let Some(cell) = delegate_cell(this) {
        unsafe { objc_storeWeak(cell, delegate) };
    }
}

/// `tokenizer` — UIKit's own string tokenizer over this view, made on the
/// first question and kept for the life of the view, which is the app's.
/// A proxy gets one of its own, let go with it ([`unmount`]).
extern "C" fn bunny_tokenizer(this: Id, _sel: Sel) -> Id {
    let made = |input: Id| unsafe {
        msg_id_id(msg_id(class("UITextInputStringTokenizer"), sel("alloc")), sel("initWithTextInput:"), input)
    };
    if let Some(kept) = with_proxy(this, |proxy| proxy.tokenizer.get()) {
        if !kept.is_null() {
            return kept;
        }
        let tokenizer = made(this);
        if with_proxy(this, |proxy| proxy.tokenizer.set(tokenizer)).is_none() {
            // the pool moved under the question: autoreleased, the kit's alone
            unsafe { msg_id(tokenizer, sel("autorelease")) };
        }
        return tokenizer;
    }
    TOKENIZER.with(|slot| {
        if slot.get().is_null() {
            slot.set(made(this));
        }
        slot.get()
    })
}

extern "C" fn bunny_farthest_in_range(_this: Id, _sel: Sel, range: Id, direction: i64) -> Id {
    unsafe { span_of(range) }.map_or(null_mut(), |(start, end)| unsafe {
        position(if Toward::of(direction).ahead() { end } else { start })
    })
}

/// `characterRangeByExtendingPosition:inDirection:` — the one character
/// on that side of the position.
extern "C" fn bunny_extend_position(this: Id, _sel: Sel, at: Id, direction: i64) -> Id {
    let (Some(mirror), Some(at)) = (mirror_of(this), unsafe { offset_of(at) }) else {
        return null_mut();
    };
    let at = at.min(mirror.units);
    let other = step_chars(&mirror.document.text, at, 1, Toward::of(direction).ahead()).unwrap_or(at);
    unsafe { range(at, other) }
}

/// `NSWritingDirectionNatural`: the field lays bidirectional text out
/// itself, run by run.
extern "C" fn bunny_writing_direction(_this: Id, _sel: Sel, _at: Id, _direction: i64) -> i64 {
    -1
}

/// `setBaseWritingDirection:forRange:` — the field's direction is its
/// text's, and nothing the keyboard can set.
extern "C" fn bunny_set_writing_direction(_this: Id, _sel: Sel, _direction: i64, _range: Id) {}

/// `firstRectForRange:` — where the keyboard's own panels look: a range on
/// one row runs from its start's caret to its end's; one that wraps
/// answers its first row's start.
extern "C" fn bunny_first_rect(this: Id, _sel: Sel, range: Id) -> CGRect {
    let Some((start, end)) = (unsafe { span_of(range) }) else {
        return NO_RECT;
    };
    let Some(first) = rect_at(this, start).or_else(|| mirror_of(this).map(|mirror| mirror.document.caret)) else {
        return NO_RECT;
    };
    match rect_at(this, end) {
        Some(last)
            if end > start
                && (last.origin.y - first.origin.y).abs() < 0.5
                && last.origin.x > first.origin.x =>
        {
            CGRect {
                origin: first.origin,
                size: CGSize { width: last.origin.x - first.origin.x, height: first.size.height },
            }
        }
        _ => first,
    }
}

extern "C" fn bunny_caret_rect(this: Id, _sel: Sel, at: Id) -> CGRect {
    unsafe { offset_of(at) }
        .and_then(|at| rect_at(this, at))
        .or_else(|| mirror_of(this).map(|mirror| mirror.document.caret))
        .unwrap_or(NO_RECT)
}

/// `selectionRectsForRange:` — none: the field paints its own selection,
/// and the kit is handed nothing to draw over it.
extern "C" fn bunny_selection_rects(_this: Id, _sel: Sel, _range: Id) -> Id {
    unsafe { msg_id(class("NSArray"), sel("array")) }
}

/// `closestPositionToPoint:` — nil off the field.
extern "C" fn bunny_closest_position(this: Id, _sel: Sel, point: CGPoint) -> Id {
    index_at(this, point.x, point.y).map_or(null_mut(), |at| unsafe { position(at) })
}

extern "C" fn bunny_closest_position_within(this: Id, _sel: Sel, point: CGPoint, range: Id) -> Id {
    match (index_at(this, point.x, point.y), unsafe { span_of(range) }) {
        (Some(at), Some((start, end))) => unsafe { position(at.clamp(start, end)) },
        _ => null_mut(),
    }
}

extern "C" fn bunny_character_range_at(this: Id, _sel: Sel, point: CGPoint) -> Id {
    let (Some(mirror), Some(at)) = (mirror_of(this), index_at(this, point.x, point.y)) else {
        return null_mut();
    };
    let next = step_chars(&mirror.document.text, at, 1, true).unwrap_or(at);
    unsafe { range(at, next) }
}

/// `textInputView` — the view, from either door: every rect and point the
/// kit trades is in the view's points, the layout's. A proxy is its own:
/// its caret stands at its own edge.
extern "C" fn bunny_text_input_view(this: Id, _sel: Sel) -> Id {
    if is_proxy(this) { this } else { VIEW.with(Cell::get) }
}

// The keyboard's edits (they travel as events, like typing)

/// `setSelectedTextRange:` — the kit hands back the selection it just
/// read as often as it moves one: an unmoved selection is no edit, and
/// must not commit a composition.
extern "C" fn bunny_set_selected_text_range(this: Id, _sel: Sel, range: Id) {
    let Some((start, end)) = (unsafe { span_of(range) }) else {
        return;
    };
    if is_proxy(this) {
        proxy_write(this, Write::Select { start, end });
        return;
    }
    let unmoved = mirror().is_some_and(|mirror| {
        let (location, length) = mirror.document.selected;
        (location, location + length) == (start, end)
    });
    if !unmoved {
        keyboard_edit(this, AppEvent::Select { location: start, length: end - start });
    }
}

/// `setMarkedText:selectedRange:` — the composition, with the selection
/// INSIDE it; a range the kit leaves unsaid (`NSNotFound`) or overlong is
/// the composition's end.
extern "C" fn bunny_set_marked_text(this: Id, _sel: Sel, text: Id, selected: NSRange) {
    let text = unsafe { text_argument_to_string(text) };
    let units = text.encode_utf16().count() as u64;
    let location = if selected.location == NS_NOT_FOUND { units } else { selected.location.min(units) };
    let length = selected.length.min(units - location);
    // a proxy never holds the keyboard, so nothing composes in it — and
    // a composition half-applied as a fill would write the box twice
    if !is_proxy(this) {
        keyboard_edit(this, AppEvent::Marked { text, location, length });
    }
}

extern "C" fn bunny_unmark_text(this: Id, _sel: Sel) {
    if !is_proxy(this) {
        keyboard_edit(this, AppEvent::Unmark);
    }
}

extern "C" fn bunny_set_marked_text_style(_this: Id, _sel: Sel, _style: Id) {}

extern "C" fn bunny_replace_range(this: Id, _sel: Sel, range: Id, text: Id) {
    let Some((start, end)) = (unsafe { span_of(range) }) else {
        return;
    };
    let text = unsafe { text_argument_to_string(text) };
    if is_proxy(this) {
        proxy_write(this, Write::Replace { start, end, text });
    } else {
        keyboard_edit(this, AppEvent::Replace { location: start, length: end - start, text });
    }
}

// The position and the range themselves

extern "C" fn bunny_range_start(this: Id, _sel: Sel) -> Id {
    unsafe { span_of(this) }.map_or(null_mut(), |(start, _)| unsafe { position(start) })
}

extern "C" fn bunny_range_end(this: Id, _sel: Sel) -> Id {
    unsafe { span_of(this) }.map_or(null_mut(), |(_, end)| unsafe { position(end) })
}

extern "C" fn bunny_range_empty(this: Id, _sel: Sel) -> i8 {
    i8::from(unsafe { span_of(this) }.is_none_or(|(start, end)| start == end))
}

/// Two positions are equal when they stand at the same offset — the kit
/// compares what it read twice.
extern "C" fn bunny_position_equal(this: Id, _sel: Sel, other: Id) -> i8 {
    i8::from(unsafe { offset_of(other).is_some() && offset_of(this) == offset_of(other) })
}

extern "C" fn bunny_position_hash(this: Id, _sel: Sel) -> u64 {
    unsafe { offset_of(this) }.unwrap_or(0)
}

extern "C" fn bunny_range_equal(this: Id, _sel: Sel, other: Id) -> i8 {
    i8::from(unsafe { span_of(other).is_some() && span_of(this) == span_of(other) })
}

extern "C" fn bunny_range_hash(this: Id, _sel: Sel) -> u64 {
    unsafe { span_of(this) }.map_or(0, |(start, end)| start.rotate_left(32) ^ end)
}

// MARK: - The form's other boxes (AutoFill's proxies)

/// One box of the form that does not hold the keyboard, standing as a
/// hidden `BunnyAutofillBox` ([`sync_autofill`]).
struct Proxy {
    view: Id,
    /// The box as the system reads it: its path, its text, a caret at the
    /// proxy's own edge.
    mirror: Rc<Mirror>,
    holds: Option<bunny_ui::text_input::Autocomplete>,
    /// The `UITextContentType` constant for what the box holds — the
    /// kit's own string, nil when the box names nothing.
    content_type: Id,
    secret: bool,
    frame: CGRect,
    /// The input system's delegate, held weakly — boxed, so the cell
    /// `objc_storeWeak` writes never moves while the proxy stands.
    delegate: Box<Cell<Id>>,
    /// The tokenizer the kit asked this proxy for, made on the first
    /// question and owned until the proxy goes.
    tokenizer: Cell<Id>,
}

thread_local! {
    /// The proxies standing now, in the form's order. Never borrowed
    /// across a call into UIKit: an answer reads the pool, and a pool
    /// borrowed for writing answers nothing rather than abort the frame
    /// UIKit called in from.
    static PROXIES: RefCell<Vec<Proxy>> = const { RefCell::new(Vec::new()) };
    /// The view the proxies stand in: inside `BunnyView`, its bounds, with
    /// no touch and no VoiceOver for anything in it — made with the first
    /// proxy, kept for the life of the app.
    static SHELF: Cell<Id> = const { Cell::new(null_mut()) };
    /// `BUNNY_IOS_TRACE`: the shelf and the proxies say what they are and
    /// what the system wrote into them (a path and a length, never text).
    static TRACING: bool = std::env::var_os("BUNNY_IOS_TRACE").is_some();
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    /// The weak cell's last word, before the memory that holds it goes.
    fn objc_destroyWeak(location: *mut Id);
}

/// What the proxy `this` answers, read through `read` — `None` when
/// `this` is no proxy standing now.
fn with_proxy<T>(this: Id, read: impl FnOnce(&Proxy) -> T) -> Option<T> {
    PROXIES.with(|pool| {
        let pool = pool.try_borrow().ok()?;
        pool.iter().find(|proxy| proxy.view == this).map(read)
    })
}

fn is_proxy(this: Id) -> bool {
    with_proxy(this, |_| ()).is_some()
}

fn tracing() -> bool {
    TRACING.with(|on| *on)
}

/// A box's document as its proxy keeps it: the caret at the end, where a
/// fill leaves it, and the caret's rect at the proxy's own leading edge.
fn box_mirror(field: String, text: Arc<str>, selected: Option<(u64, u64)>, frame: CGRect) -> Rc<Mirror> {
    let units = byte_to_utf16(&text, text.len()) as u64;
    let selected = selected.map_or((units, 0), |(location, length)| {
        let location = location.min(units);
        (location, length.min(units - location))
    });
    let caret = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 0.0, height: frame.size.height } };
    Rc::new(Mirror { document: Document { field, text, selected, marked: None, caret }, units })
}

/// The shell syncs the form after a frame
/// ([`bunny_ui::runtime::Runtime::autofill_group`]): every box but the
/// one holding the keyboard gets a proxy wearing what it holds, whether
/// it hides it, its frame and its text; a proxy whose box left the form
/// goes. Said before the keyboard is claimed — the claim and a door's
/// renewal wait for the next turn of the main queue — so the group
/// AutoFill builds for the door that takes the keyboard finds the form
/// whole. An empty group lets every proxy go.
///
/// A box the app rewrote is announced to its proxy's input delegate
/// around the swap, as [`sync_text`] does for the door; a box that reads
/// back what the proxy itself wrote is no news.
pub fn sync_autofill(group: &[bunny_ui::runtime::AutofillBox], focused: Option<&str>) {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return;
    }
    let others: Vec<_> = group.iter().filter(|member| Some(member.path.as_str()) != focused).collect();
    // out of the pool first, out of the view after: UIKit may ask the
    // pool something on the way
    let gone: Vec<Proxy> = PROXIES.with(|pool| {
        let mut pool = pool.borrow_mut();
        let (kept, gone) = std::mem::take(&mut *pool)
            .into_iter()
            .partition(|proxy| others.iter().any(|member| member.path == proxy.mirror.document.field));
        *pool = kept;
        gone
    });
    for proxy in gone {
        unsafe { unmount(proxy) };
    }
    for member in others {
        let frame = CGRect {
            origin: CGPoint { x: member.frame.origin.x, y: member.frame.origin.y },
            size: CGSize { width: member.frame.size.width, height: member.frame.size.height },
        };
        let standing = PROXIES.with(|pool| {
            pool.borrow().iter().find(|proxy| proxy.mirror.document.field == member.path).map(|proxy| {
                (proxy.view, Rc::clone(&proxy.mirror), proxy.holds, proxy.frame, proxy.delegate.as_ptr())
            })
        });
        match standing {
            Some((proxy, mirror, holds, was, delegate)) => unsafe {
                if holds != member.holds {
                    let content = bunny_ui_apple::autofill::content_type(member.holds, bunny_ui_apple::autofill::Kit::UIKit);
                    update_proxy(proxy, |standing| {
                        standing.holds = member.holds;
                        standing.content_type = content;
                    });
                }
                update_proxy(proxy, |standing| standing.secret = member.secret);
                let moved = was.origin.x != frame.origin.x
                    || was.origin.y != frame.origin.y
                    || was.size.width != frame.size.width
                    || was.size.height != frame.size.height;
                if moved {
                    update_proxy(proxy, |standing| standing.frame = frame);
                    msg_void_rect(proxy, sel("setFrame:"), frame);
                }
                let rewritten = *mirror.document.text != *member.text;
                if rewritten || moved {
                    let selected = (!rewritten).then_some(mirror.document.selected);
                    let fresh = box_mirror(member.path.clone(), Arc::clone(&member.text), selected, frame);
                    let delegate = if rewritten { objc_loadWeak(delegate) } else { null_mut() };
                    if !delegate.is_null() {
                        msg_void_id(delegate, sel("textWillChange:"), proxy);
                    }
                    update_proxy(proxy, |standing| standing.mirror = fresh);
                    if !delegate.is_null() {
                        msg_void_id(delegate, sel("textDidChange:"), proxy);
                    }
                }
            },
            None => unsafe { mount(view, member, frame) },
        }
    }
}

/// Writes a standing proxy's record — briefly, and never across a call
/// into UIKit.
fn update_proxy(view: Id, write: impl FnOnce(&mut Proxy)) {
    PROXIES.with(|pool| {
        if let Ok(mut pool) = pool.try_borrow_mut()
            && let Some(proxy) = pool.iter_mut().find(|proxy| proxy.view == view)
        {
            write(proxy);
        }
    });
}

/// The shelf the proxies stand in, made on the first call: inside the
/// view, its bounds and resized with it, deaf to touches (a view that
/// takes no touch hands none to anything inside it), and hidden from
/// VoiceOver — the scene's own boxes are what an assistive technology
/// reads, never their proxies.
unsafe fn shelf(view: Id) -> Id {
    let kept = SHELF.with(Cell::get);
    if !kept.is_null() {
        return kept;
    }
    unsafe {
        let shelf = msg_init_rect(msg_id(class("UIView"), sel("alloc")), sel("initWithFrame:"), msg_rect(view, sel("bounds")));
        // UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight
        msg_void_u64(shelf, sel("setAutoresizingMask:"), 2 | 16);
        msg_void_bool(shelf, sel("setUserInteractionEnabled:"), 0);
        msg_void_bool(shelf, sel("setAccessibilityElementsHidden:"), 1);
        msg_void_bool(shelf, sel("setIsAccessibilityElement:"), 0);
        msg_void_id(view, sel("addSubview:"), shelf);
        SHELF.with(|slot| slot.set(shelf));
        if tracing() {
            eprintln!(
                "bunny_ui ios: AutoFill shelf up — userInteractionEnabled {}, accessibilityElementsHidden {}, isAccessibilityElement {}",
                msg_bool(shelf, sel("isUserInteractionEnabled")),
                msg_bool(shelf, sel("accessibilityElementsHidden")),
                msg_bool(shelf, sel("isAccessibilityElement")),
            );
        }
        shelf
    }
}

/// A proxy for `member`, into the pool first and onto the shelf after.
unsafe fn mount(view: Id, member: &bunny_ui::runtime::AutofillBox, frame: CGRect) {
    unsafe {
        let shelf = shelf(view);
        let proxy = msg_init_rect(msg_id(class("BunnyAutofillBox"), sel("alloc")), sel("initWithFrame:"), frame);
        msg_void_bool(proxy, sel("setIsAccessibilityElement:"), 0);
        let content_type = bunny_ui_apple::autofill::content_type(member.holds, bunny_ui_apple::autofill::Kit::UIKit);
        PROXIES.with(|pool| {
            pool.borrow_mut().push(Proxy {
                view: proxy,
                mirror: box_mirror(member.path.clone(), Arc::clone(&member.text), None, frame),
                holds: member.holds,
                content_type,
                secret: member.secret,
                frame,
                delegate: Box::new(Cell::new(null_mut())),
                tokenizer: Cell::new(null_mut()),
            });
        });
        msg_void_id(shelf, sel("addSubview:"), proxy);
        if tracing() {
            eprintln!(
                "bunny_ui ios: AutoFill proxy up — {} holds {:?}, secret {}, isAccessibilityElement {}",
                member.path,
                member.holds,
                member.secret,
                msg_bool(proxy, sel("isAccessibilityElement")),
            );
        }
    }
}

/// A proxy let go of: off the shelf, its weak cell closed, its tokenizer
/// and itself released. UIKit may hold it a while longer; then it answers
/// as no box at all, since the pool no longer knows it.
unsafe fn unmount(proxy: Proxy) {
    unsafe {
        msg_void(proxy.view, sel("removeFromSuperview"));
        objc_destroyWeak(proxy.delegate.as_ptr());
        let tokenizer = proxy.tokenizer.get();
        if !tokenizer.is_null() {
            msg_void(tokenizer, sel("release"));
        }
        msg_void(proxy.view, sel("release"));
        if tracing() {
            eprintln!("bunny_ui ios: AutoFill proxy down — {}", proxy.mirror.document.field);
        }
    }
}

/// What the system writes into a proxy, in its box's text (UTF-16).
enum Write {
    /// `insertText:` — over the selection.
    Insert(String),
    /// `replaceRange:withText:` — over a span.
    Replace { start: u64, end: u64, text: String },
    /// `deleteBackward` — the selection, or the character before the caret.
    DeleteBackward,
    /// `setSelectedTextRange:` — the selection alone: nothing to fill.
    Select { start: u64, end: u64 },
}

/// Applies what the system wrote into the proxy `this` to the proxy's
/// copy of its box — at once, so what the system reads back next is what
/// it wrote — and hands the box's whole new text to the shell
/// ([`AppEvent::Fill`]). The frame that follows reads the box back from
/// the runtime ([`sync_autofill`]): a box that kept less than it was
/// given shows the system what it kept.
fn proxy_write(this: Id, write: Write) {
    let Some(mirror) = with_proxy(this, |proxy| Rc::clone(&proxy.mirror)) else {
        return;
    };
    let document = &mirror.document;
    let (location, length) = document.selected;
    let (start, end, insert, said) = match write {
        Write::Insert(text) => (location, location + length, text, "insertText"),
        Write::Replace { start, end, text } => (start, end, text, "replaceRange"),
        Write::DeleteBackward if length > 0 => (location, location + length, String::new(), "deleteBackward"),
        Write::DeleteBackward => {
            let from = step_chars(&document.text, location, 1, false).unwrap_or(location);
            (from, location, String::new(), "deleteBackward")
        }
        Write::Select { start, end } => {
            let fresh = box_mirror(
                document.field.clone(),
                Arc::clone(&document.text),
                Some((start.min(end), end.max(start) - start.min(end))),
                CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: document.caret.size },
            );
            update_proxy(this, |proxy| proxy.mirror = fresh);
            return;
        }
    };
    let text = &document.text;
    let from = utf16_to_byte(text, start.min(mirror.units) as usize);
    let to = utf16_to_byte(text, end.min(mirror.units) as usize).max(from);
    let mut filled = String::with_capacity(text.len() - (to - from) + insert.len());
    filled.push_str(&text[..from]);
    filled.push_str(&insert);
    filled.push_str(&text[to..]);
    let caret = byte_to_utf16(&filled, from + insert.len()) as u64;
    if tracing() {
        eprintln!("bunny_ui ios: AutoFill wrote {} through {said}", document.field);
    }
    let fresh = box_mirror(
        document.field.clone(),
        Arc::from(filled.as_str()),
        Some((caret, 0)),
        CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: document.caret.size },
    );
    update_proxy(this, |proxy| proxy.mirror = fresh);
    dispatch(AppEvent::Fill { field: document.field.clone(), text: Filled(filled) });
}

/// The HID usages that type nothing and therefore never reach
/// `insertText:` — the ones a press is the only word of.
fn silent_key(hid: u64) -> bool {
    matches!(hid, 0x29 | 0x2B | 0x4A..=0x52)
}

/// The HID usages of the modifier keys themselves — left and right
/// control, shift, option and command (0xE0 to 0xE7).
fn modifier_key(hid: u64) -> bool {
    (0xE0..=0xE7).contains(&hid)
}

/// A press of a modifier key, down or up: the EVENT's flags are the
/// state it leaves, which is the whole news. `true` when one was found.
fn report_modifiers(presses: Id, event: Id) -> bool {
    let moved = unsafe { members_of(presses) }.into_iter().any(|press| unsafe {
        let key = msg_id(press, sel("key"));
        !key.is_null() && modifier_key(msg_i64(key, sel("keyCode")).max(0) as u64)
    });
    if moved {
        let flags = unsafe { msg_i64(event, sel("modifierFlags")) }.max(0) as u64;
        dispatch(AppEvent::Modifiers(modifiers_of(flags)));
    }
    moved
}

/// A hardware key. The keymap gate hears it first; a silent key the
/// gate declines becomes an editing command; everything else goes on to
/// UIKit, which types it through `insertText:` — so a letter is never
/// typed twice, and backspace and return arrive by the keyboard's road.
extern "C" fn bunny_presses_began(this: Id, _sel: Sel, presses: Id, event: Id) {
    report_modifiers(presses, event);
    let mut taken = false;
    for press in unsafe { members_of(presses) } {
        let key = unsafe { msg_id(press, sel("key")) };
        if key.is_null() {
            continue;
        }
        let stroke = unsafe {
            let hid = msg_i64(key, sel("keyCode")).max(0) as u64;
            let flags = msg_i64(key, sel("modifierFlags")).max(0) as u64;
            let modifiers = modifiers_of(flags);
            let read = |selector: &str| {
                let string = msg_id(key, sel(selector));
                if string.is_null() {
                    return String::new();
                }
                let chars = msg_cstr(string, sel("UTF8String"));
                if chars.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(chars).to_string_lossy().into_owned()
                }
            };
            let typed = read("characters").chars().next().filter(|c| !c.is_control());
            KeyStroke {
                hid,
                shift: modifiers.shift,
                control: modifiers.control,
                option: modifiers.option,
                command: modifiers.command,
                chars_ignoring: read("charactersIgnoringModifiers"),
                typed,
            }
        };
        let gated = KEY_GATE.with(|slot| {
            slot.borrow_mut().as_mut().is_some_and(|gate| gate(&stroke))
        });
        if gated {
            taken = true;
            continue;
        }
        if silent_key(stroke.hid) || stroke.command {
            dispatch(AppEvent::Key(stroke));
            taken = true;
        }
    }
    if !taken {
        unsafe {
            let sup = ObjcSuper { receiver: this, class: class("UIView") };
            msg_super_void_id_id(&sup, sel("pressesBegan:withEvent:"), presses, event);
        }
    }
}

extern "C" fn bunny_presses_ended(this: Id, _sel: Sel, presses: Id, event: Id) {
    report_modifiers(presses, event);
    unsafe {
        let sup = ObjcSuper { receiver: this, class: class("UIView") };
        msg_super_void_id_id(&sup, sel("pressesEnded:withEvent:"), presses, event);
    }
}

// MARK: - The controller: the safe area and the traits

extern "C" fn bunny_safe_area_changed(this: Id, _sel: Sel) {
    unsafe {
        let sup = ObjcSuper { receiver: this, class: class("UIViewController") };
        msg_super_void(&sup, sel("viewSafeAreaInsetsDidChange"));
    }
    let insets = safe_area();
    dispatch(AppEvent::SafeArea {
        top: insets.top,
        left: insets.left,
        bottom: insets.bottom,
        right: insets.right,
    });
}

extern "C" fn bunny_traits_changed(this: Id, _sel: Sel, previous: Id) {
    unsafe {
        let sup = ObjcSuper { receiver: this, class: class("UIViewController") };
        msg_super_void_id(&sup, sel("traitCollectionDidChange:"), previous);
    }
    let (dark, compact) = traits();
    dispatch(AppEvent::Traits { dark, compact });
}

// MARK: - What the shell reads

/// The view's size in points.
pub fn view_size() -> (f64, f64) {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return (0.0, 0.0);
    }
    let bounds = unsafe { msg_rect(view, sel("bounds")) };
    (bounds.size.width, bounds.size.height)
}

/// Device pixels per point — 2 or 3 on every phone.
pub fn view_scale() -> usize {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return 1;
    }
    unsafe { msg_f64(view, sel("contentScaleFactor")).round().max(1.0) as usize }
}

/// The view's safe area, in points.
pub fn safe_area() -> UIEdgeInsets {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return UIEdgeInsets::default();
    }
    unsafe { msg_insets(view, sel("safeAreaInsets")) }
}

/// The traits: dark or light (`None` when the system says neither), and
/// whether the horizontal size class is compact.
pub fn traits() -> (Option<bool>, bool) {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return (None, false);
    }
    unsafe {
        let traits = msg_id(view, sel("traitCollection"));
        // UIUserInterfaceStyle: unspecified 0, light 1, dark 2
        let dark = match msg_i64(traits, sel("userInterfaceStyle")) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        };
        // UIUserInterfaceSizeClass: unspecified 0, compact 1, regular 2
        let compact = msg_i64(traits, sel("horizontalSizeClass")) == 1;
        (dark, compact)
    }
}

/// The system's reduce-motion setting.
pub fn reduce_motion() -> bool {
    unsafe { UIAccessibilityIsReduceMotionEnabled() }
}

/// The keys the soft keyboard lays out for the field that holds it — an
/// email's `@`, a code's number pad. Said before [`want_keyboard`]: a
/// keyboard about to come up reads the kind as it comes, and one already
/// up is told to lay its keys out again — on the next turn of the main
/// queue, like the claim.
pub fn want_keys(keyboard: bunny_ui::text_input::KeyboardType) {
    use bunny_ui::text_input::KeyboardType;
    // UIKeyboardType: default 0, URL 3, ASCII-capable number pad 11 (a
    // code's digits are ASCII whatever the locale), phone pad 5, email
    // address 7, decimal pad 8, web search 10
    let kind = match keyboard {
        KeyboardType::Text => 0,
        KeyboardType::Email => 7,
        KeyboardType::Digits => 11,
        KeyboardType::Decimal => 8,
        KeyboardType::Phone => 5,
        KeyboardType::Url => 3,
        KeyboardType::Search => 10,
    };
    if KEYBOARD_KIND.with(|slot| slot.replace(kind)) == kind || !KEYBOARD_WANTED.with(Cell::get) {
        return;
    }
    reload_keys_soon();
}

/// What the field holding the keyboard holds and whether it hides it —
/// the view's `textContentType` and `isSecureTextEntry`, read by the
/// system's AutoFill: the address box is offered the saved account, the
/// password box the password, the code box the one-time code. Said with
/// [`want_keys`], and like it a keyboard already up is told to read the
/// traits again on the next turn of the main queue.
pub fn want_traits(holds: Option<bunny_ui::text_input::Autocomplete>, secret: bool) {
    let mut moved = SECURE.with(|slot| slot.replace(secret)) != secret;
    if HOLDS.with(|slot| slot.replace(holds)) != holds {
        let content = bunny_ui_apple::autofill::content_type(holds, bunny_ui_apple::autofill::Kit::UIKit);
        CONTENT_TYPE.with(|slot| slot.set(content));
        moved = true;
    }
    if moved && KEYBOARD_WANTED.with(Cell::get) {
        reload_keys_soon();
    }
}

/// The keyboard that is up reads the view's traits again, on the next
/// turn of the main queue ([`bunny_reload_keys`]).
fn reload_keys_soon() {
    perform_soon("bunnyReloadKeys");
}

/// One of the view's own selectors, on the next turn of the main queue:
/// UIKit re-enters the app synchronously from a first-responder change,
/// and the handler that asked is still running.
fn perform_soon(selector: &str) {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return;
    }
    unsafe {
        msg_void_sel_id_f64(
            view,
            sel("performSelector:withObject:afterDelay:"),
            sel(selector),
            null_mut(),
            0.0,
        );
    }
}

/// The window the system's sheets hang from: the app's view's own —
/// null before the scene put the view in one.
pub fn anchor_window() -> Id {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return null_mut();
    }
    unsafe { msg_id(view, sel("window")) }
}

/// Claims (or drops) the soft keyboard — deferred to the next turn of
/// the main queue, because UIKit re-enters the app synchronously from
/// a first-responder change and the handler that asked is still running.
pub fn want_keyboard(wanted: bool) {
    if KEYBOARD_WANTED.with(|slot| slot.replace(wanted)) == wanted {
        return;
    }
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return;
    }
    let selector = if wanted { "bunnyClaimKeyboard" } else { "bunnyDropKeyboard" };
    unsafe {
        msg_void_sel_id_f64(
            view,
            sel("performSelector:withObject:afterDelay:"),
            sel(selector),
            null_mut(),
            0.0,
        );
    }
}

pub fn clipboard_write(text: &str) {
    unsafe {
        let pasteboard = msg_id(class("UIPasteboard"), sel("generalPasteboard"));
        let Ok(text) = CString::new(text) else { return };
        let string = msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr());
        msg_void_id(pasteboard, sel("setString:"), string);
    }
}

pub fn clipboard_read() -> Option<String> {
    unsafe {
        let pasteboard = msg_id(class("UIPasteboard"), sel("generalPasteboard"));
        let string = msg_id(pasteboard, sel("string"));
        if string.is_null() {
            return None;
        }
        let chars = msg_cstr(string, sel("UTF8String"));
        if chars.is_null() {
            return None;
        }
        Some(CStr::from_ptr(chars).to_string_lossy().into_owned())
    }
}

// MARK: - The GPU road

/// Attaches the presenter to the view's own layer and primes it. False
/// when the GPU road is refused or cannot come up — the phone has no
/// CPU road, so the view then stays blank and says so once.
pub fn install_gpu() -> bool {
    let view = VIEW.with(Cell::get);
    if view.is_null() {
        return false;
    }
    let (layer, scale, bounds) = unsafe {
        (msg_id(view, sel("layer")), msg_f64(view, sel("contentScaleFactor")), msg_rect(view, sel("bounds")))
    };
    let Some(mut presenter) = MetalPresenter::attach(layer, scale) else {
        eprintln!("bunny_ui ios: the GPU road did not come up — nothing paints on this device");
        return false;
    };
    presenter.prime(bounds.size.width, bounds.size.height, scale.round().max(1.0) as usize);
    PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
    true
}

/// Presents one frame — nothing while the app is off the screen.
pub fn present(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    if BACKGROUNDED.with(Cell::get) {
        return;
    }
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.present(display, size, scale, canvas, text, images, false);
        }
    });
}

// MARK: - The classes

static REGISTER_CLASSES: Once = Once::new();

unsafe fn register_classes() {
    REGISTER_CLASSES.call_once(|| unsafe {
        let v_id = CString::new("v@:@").expect("type encoding");
        let v_id_id = CString::new("v@:@@").expect("type encoding");
        let v = CString::new("v@:").expect("type encoding");
        let class_ = CString::new("#@:").expect("type encoding");

        // the keyboard's positions and ranges: offsets in UTF-16, kept in
        // instance variables (added before the class is registered, read
        // back by where they landed once it is)
        let position = objc_allocateClassPair(class("UITextPosition"), c"BunnyTextPosition".as_ptr(), 0);
        class_addIvar(position, c"bunnyOffset".as_ptr(), 8, 3, c"Q".as_ptr());
        class_addMethod(position, sel("isEqual:"), bunny_position_equal as *const c_void, c"c@:@".as_ptr());
        class_addMethod(position, sel("hash"), bunny_position_hash as *const c_void, c"Q@:".as_ptr());
        objc_registerClassPair(position);
        let at = |class: Id, name: &CStr| ivar_getOffset(class_getInstanceVariable(class, name.as_ptr()));
        POSITION_IVAR.with(|slot| slot.set(at(position, c"bunnyOffset")));
        let range = objc_allocateClassPair(class("UITextRange"), c"BunnyTextRange".as_ptr(), 0);
        class_addIvar(range, c"bunnyStart".as_ptr(), 8, 3, c"Q".as_ptr());
        class_addIvar(range, c"bunnyEnd".as_ptr(), 8, 3, c"Q".as_ptr());
        for (name, imp, types) in [
            ("start", bunny_range_start as *const c_void, c"@@:"),
            ("end", bunny_range_end as *const c_void, c"@@:"),
            ("isEmpty", bunny_range_empty as *const c_void, c"c@:"),
            ("isEqual:", bunny_range_equal as *const c_void, c"c@:@"),
            ("hash", bunny_range_hash as *const c_void, c"Q@:"),
        ] {
            class_addMethod(range, sel(name), imp, types.as_ptr());
        }
        objc_registerClassPair(range);
        RANGE_IVARS.with(|slot| slot.set((at(range, c"bunnyStart"), at(range, c"bunnyEnd"))));

        // the view: a CAMetalLayer for a skin and ears for every finger.
        // It takes no text — the doors inside it do ([`add_key_input_methods`])
        let view = objc_allocateClassPair(
            class("UIView"),
            CString::new("BunnyView").expect("name").as_ptr(),
            0,
        );
        // a CLASS method lives on the metaclass; added on the class it
        // would be an instance method nobody calls, and the view would
        // wear a plain CALayer that no drawable comes from
        class_addMethod(
            object_getClass(view),
            sel("layerClass"),
            bunny_layer_class as *const c_void,
            class_.as_ptr(),
        );
        for (name, imp) in [
            ("touchesBegan:withEvent:", bunny_touches_began as *const c_void),
            ("touchesMoved:withEvent:", bunny_touches_moved as *const c_void),
            ("touchesEnded:withEvent:", bunny_touches_ended as *const c_void),
            ("touchesCancelled:withEvent:", bunny_touches_cancelled as *const c_void),
        ] {
            class_addMethod(view, sel(name), imp, v_id_id.as_ptr());
        }
        class_addMethod(view, sel("layoutSubviews"), bunny_layout_subviews as *const c_void, v.as_ptr());
        class_addMethod(view, sel("bunnyClaimKeyboard"), bunny_claim_keyboard as *const c_void, v.as_ptr());
        class_addMethod(view, sel("bunnyDropKeyboard"), bunny_drop_keyboard as *const c_void, v.as_ptr());
        class_addMethod(view, sel("bunnyReloadKeys"), bunny_reload_keys as *const c_void, v.as_ptr());
        class_addMethod(view, sel("bunnyRenewKeyboard"), bunny_renew_keyboard as *const c_void, v.as_ptr());
        objc_registerClassPair(view);

        // the keyboard's doors: the keyboard's methods and nothing else —
        // no layer of their own to draw, no finger to hear
        let door = objc_allocateClassPair(class("UIView"), c"BunnyKeyInput".as_ptr(), 0);
        add_key_input_methods(door);
        objc_registerClassPair(door);

        // AutoFill's proxies: the text and its traits, and none of what
        // makes a responder — UIView's own `canBecomeFirstResponder` says
        // no, so the system can never move the keyboard into one
        let proxy = objc_allocateClassPair(class("UIView"), c"BunnyAutofillBox".as_ptr(), 0);
        add_text_input_methods(proxy);
        objc_registerClassPair(proxy);

        // the controller: what UIKit tells a controller and not a view
        let controller = objc_allocateClassPair(
            class("UIViewController"),
            CString::new("BunnyViewController").expect("name").as_ptr(),
            0,
        );
        class_addMethod(
            controller,
            sel("viewSafeAreaInsetsDidChange"),
            bunny_safe_area_changed as *const c_void,
            v.as_ptr(),
        );
        class_addMethod(
            controller,
            sel("traitCollectionDidChange:"),
            bunny_traits_changed as *const c_void,
            v_id.as_ptr(),
        );
        objc_registerClassPair(controller);
        register_life_classes();
    });
}

/// The responder the keyboard types into, on a class still open: the
/// hardware keys, the first responder, the main menu's rows, and the text
/// ([`add_text_input_methods`]). Both doors wear it
/// ([`bunny_renew_keyboard`]).
unsafe fn add_key_input_methods(class: Id) {
    unsafe {
        for (name, imp) in [
            ("pressesBegan:withEvent:", bunny_presses_began as *const c_void),
            ("pressesEnded:withEvent:", bunny_presses_ended as *const c_void),
        ] {
            class_addMethod(class, sel(name), imp, c"v@:@@".as_ptr());
        }
        class_addMethod(class, sel("canBecomeFirstResponder"), bunny_yes as *const c_void, c"c@:".as_ptr());
        add_text_input_methods(class);
        // the main menu's rows and the standard edits, while the view
        // holds the keyboard
        add_menu_methods(
            class,
            bunny_view_validate as *const c_void,
            bunny_view_can_perform as *const c_void,
        );
    }
}

/// The text the system reads and writes, on a class still open: the
/// traits, `UIKeyInput` and the whole of `UITextInput`. The doors and
/// AutoFill's proxies wear it, each answer asking whose document it reads
/// ([`mirror_of`]).
unsafe fn add_text_input_methods(class: Id) {
    unsafe {
        class_addMethod(class, sel("hasText"), bunny_has_text as *const c_void, c"c@:".as_ptr());
        class_addMethod(class, sel("isSecureTextEntry"), bunny_secure_entry as *const c_void, c"c@:".as_ptr());
        class_addMethod(class, sel("textContentType"), bunny_content_type as *const c_void, c"@@:".as_ptr());
        class_addMethod(class, sel("insertText:"), bunny_insert_text as *const c_void, c"v@:@".as_ptr());
        class_addMethod(class, sel("deleteBackward"), bunny_delete_backward as *const c_void, c"v@:".as_ptr());
        let view = class;
        let int = c"q@:";
        for name in [
            "autocorrectionType",
            "spellCheckingType",
            "smartQuotesType",
            "smartDashesType",
            "smartInsertDeleteType",
            "inlinePredictionType",
            "mathExpressionCompletionType",
        ] {
            class_addMethod(view, sel(name), bunny_trait_no as *const c_void, int.as_ptr());
        }
        for name in ["autocapitalizationType", "returnKeyType", "keyboardAppearance"] {
            class_addMethod(view, sel(name), bunny_trait_zero as *const c_void, int.as_ptr());
        }
        class_addMethod(view, sel("writingToolsBehavior"), bunny_trait_none as *const c_void, int.as_ptr());
        class_addMethod(view, sel("keyboardType"), bunny_keyboard_type as *const c_void, int.as_ptr());
        // the whole of UITextInput: every question answered from the
        // mirror ([`sync_text`]), every edit sent as an event
        for (name, imp, types) in [
            ("textInRange:", bunny_text_in_range as *const c_void, c"@@:@"),
            ("replaceRange:withText:", bunny_replace_range as *const c_void, c"v@:@@"),
            ("selectedTextRange", bunny_selected_text_range as *const c_void, c"@@:"),
            ("setSelectedTextRange:", bunny_set_selected_text_range as *const c_void, c"v@:@"),
            ("markedTextRange", bunny_marked_text_range as *const c_void, c"@@:"),
            ("markedTextStyle", bunny_marked_text_style as *const c_void, c"@@:"),
            ("setMarkedTextStyle:", bunny_set_marked_text_style as *const c_void, c"v@:@"),
            ("setMarkedText:selectedRange:", bunny_set_marked_text as *const c_void, c"v@:@{_NSRange=QQ}"),
            ("unmarkText", bunny_unmark_text as *const c_void, c"v@:"),
            ("beginningOfDocument", bunny_beginning_of_document as *const c_void, c"@@:"),
            ("endOfDocument", bunny_end_of_document as *const c_void, c"@@:"),
            ("textRangeFromPosition:toPosition:", bunny_range_between as *const c_void, c"@@:@@"),
            ("positionFromPosition:offset:", bunny_position_offset as *const c_void, c"@@:@q"),
            (
                "positionFromPosition:inDirection:offset:",
                bunny_position_toward as *const c_void,
                c"@@:@qq",
            ),
            ("comparePosition:toPosition:", bunny_compare_positions as *const c_void, c"q@:@@"),
            ("offsetFromPosition:toPosition:", bunny_offset_between as *const c_void, c"q@:@@"),
            ("inputDelegate", bunny_input_delegate as *const c_void, c"@@:"),
            ("setInputDelegate:", bunny_set_input_delegate as *const c_void, c"v@:@"),
            ("tokenizer", bunny_tokenizer as *const c_void, c"@@:"),
            (
                "positionWithinRange:farthestInDirection:",
                bunny_farthest_in_range as *const c_void,
                c"@@:@q",
            ),
            (
                "characterRangeByExtendingPosition:inDirection:",
                bunny_extend_position as *const c_void,
                c"@@:@q",
            ),
            (
                "baseWritingDirectionForPosition:inDirection:",
                bunny_writing_direction as *const c_void,
                c"q@:@q",
            ),
            ("setBaseWritingDirection:forRange:", bunny_set_writing_direction as *const c_void, c"v@:q@"),
            ("firstRectForRange:", bunny_first_rect as *const c_void, c"{CGRect={CGPoint=dd}{CGSize=dd}}@:@"),
            (
                "caretRectForPosition:",
                bunny_caret_rect as *const c_void,
                c"{CGRect={CGPoint=dd}{CGSize=dd}}@:@",
            ),
            ("selectionRectsForRange:", bunny_selection_rects as *const c_void, c"@@:@"),
            ("closestPositionToPoint:", bunny_closest_position as *const c_void, c"@@:{CGPoint=dd}"),
            (
                "closestPositionToPoint:withinRange:",
                bunny_closest_position_within as *const c_void,
                c"@@:{CGPoint=dd}@",
            ),
            ("characterRangeAtPoint:", bunny_character_range_at as *const c_void, c"@@:{CGPoint=dd}"),
            ("textInputView", bunny_text_input_view as *const c_void, c"@@:"),
        ] {
            class_addMethod(view, sel(name), imp, types.as_ptr());
        }
        for protocol in ["UIKeyInput", "UITextInputTraits", "UITextInput"] {
            let name = CString::new(protocol).expect("protocol name");
            let protocol = objc_getProtocol(name.as_ptr());
            if !protocol.is_null() {
                class_addProtocol(view, protocol);
            }
        }
    }
}

/// The application's delegate and the scene's: the launch, the life on
/// the screen, the clocks, a url handed over.
unsafe fn register_life_classes() {
    unsafe {
        let v_id = c"v@:@";
        let v_id_id = c"v@:@@";
        let v_id_id_id = c"v@:@@@";
        let bool_id_id = c"c@:@@";
        let id_id_id_id = c"@@:@@@";

        // the delegate: the launch, the life, the clocks
        let delegate = objc_allocateClassPair(
            class("UIResponder"),
            CString::new("BunnyAppDelegate").expect("name").as_ptr(),
            0,
        );
        class_addMethod(
            delegate,
            sel("application:didFinishLaunchingWithOptions:"),
            bunny_did_finish_launching as *const c_void,
            bool_id_id.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("application:configurationForConnectingSceneSession:options:"),
            bunny_scene_configuration as *const c_void,
            id_id_id_id.as_ptr(),
        );
        for (name, imp) in [
            ("applicationDidReceiveMemoryWarning:", bunny_memory_warning as *const c_void),
            ("bunnyBlink:", bunny_blink as *const c_void),
            ("bunnySlow:", bunny_slow as *const c_void),
            ("bunnyFrame:", bunny_frame as *const c_void),
            ("bunnyKeyboard:", bunny_keyboard as *const c_void),
        ] {
            class_addMethod(delegate, sel(name), imp, v_id.as_ptr());
        }
        // the notification center's doors and its protocol come from
        // the shared half — the class is still open, so they land on
        // the very object UIKit hands the launch to
        bunny_ui_apple::notifications::add_delegate_methods(delegate);
        // the main menu: built here, and answered here when no view holds
        // the keyboard — the delegate ends every responder chain
        class_addMethod(delegate, sel("buildMenuWithBuilder:"), bunny_build_menu as *const c_void, v_id.as_ptr());
        add_menu_methods(
            delegate,
            bunny_delegate_validate as *const c_void,
            bunny_delegate_can_perform as *const c_void,
        );
        objc_registerClassPair(delegate);

        // the scene's delegate: the window, the life on the screen, a url
        let scene = objc_allocateClassPair(
            class("UIResponder"),
            CString::new("BunnySceneDelegate").expect("name").as_ptr(),
            0,
        );
        class_addMethod(
            scene,
            sel("scene:willConnectToSession:options:"),
            bunny_scene_connect as *const c_void,
            v_id_id_id.as_ptr(),
        );
        class_addMethod(scene, sel("scene:openURLContexts:"), bunny_scene_open_urls as *const c_void, v_id_id.as_ptr());
        for (name, imp) in [
            ("sceneDidEnterBackground:", bunny_did_enter_background as *const c_void),
            ("sceneWillEnterForeground:", bunny_will_enter_foreground as *const c_void),
        ] {
            class_addMethod(scene, sel(name), imp, v_id.as_ptr());
        }
        let protocol = objc_getProtocol(c"UIWindowSceneDelegate".as_ptr());
        if !protocol.is_null() {
            class_addProtocol(scene, protocol);
        }
        objc_registerClassPair(scene);
    }
}

// MARK: - The main menu (the iPad's menu bar, the ⌘-hold sheet)

use bunny_ui::action::KeyPattern;
use bunny_ui::menu::{Edit, Pick};
use bunny_ui_apple::ffi::ns_string;
use bunny_ui_apple::uikit_menu::{Anchor, NamedKey, Place, Standard, UiArrangement, UiCommand, UiInput, UiItem};

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIMenuApplication: Id;
    static UIMenuFile: Id;
    static UIMenuEdit: Id;
    static UIMenuView: Id;
    static UIMenuWindow: Id;
    static UIMenuHelp: Id;
    static UIMenuPreferences: Id;
    static UIMenuAbout: Id;
    static UIMenuFormat: Id;
    static UIMenuFind: Id;
    static UIKeyInputUpArrow: Id;
    static UIKeyInputDownArrow: Id;
    static UIKeyInputLeftArrow: Id;
    static UIKeyInputRightArrow: Id;
    static UIKeyInputEscape: Id;
    static UIKeyInputPageUp: Id;
    static UIKeyInputPageDown: Id;
    static UIKeyInputHome: Id;
    static UIKeyInputEnd: Id;
    static UIKeyInputDelete: Id;
    static UIKeyInputF1: Id;
    static UIKeyInputF2: Id;
    static UIKeyInputF3: Id;
    static UIKeyInputF4: Id;
    static UIKeyInputF5: Id;
    static UIKeyInputF6: Id;
    static UIKeyInputF7: Id;
    static UIKeyInputF8: Id;
    static UIKeyInputF9: Id;
    static UIKeyInputF10: Id;
    static UIKeyInputF11: Id;
    static UIKeyInputF12: Id;
}

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_menu(class: Id, sel: Sel, title: Id, image: Id, identifier: Id, options: u64, children: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_key_command(class: Id, sel: Sel, title: Id, image: Id, action: Sel, input: Id, flags: u64, plist: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_command(class: Id, sel: Sel, title: Id, image: Id, action: Sel, plist: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_array(class: Id, sel: Sel, objects: *const Id, count: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_u64(obj: Id, sel: Sel, a: u64);
    #[link_name = "objc_msgSend"]
    fn msg_bool_id(obj: Id, sel: Sel, a: Id) -> i8;
    #[link_name = "objc_msgSendSuper"]
    fn msg_super_bool_sel_id(sup: *const ObjcSuper, sel: Sel, a: Sel, b: Id) -> i8;
}

/// `UIMenuOptionsDisplayInline`: a group drawn in place, rules around it.
const INLINE: u64 = 1;
/// `UIMenuElementAttributesDisabled`.
const DISABLED: u64 = 1;

/// Runs a chosen row, with the single stroke it stands for: `true` = it
/// ran (or the keymap took the stroke first).
pub type MenuGate = Box<dyn FnMut(Pick, Option<KeyPattern>) -> bool>;

/// Says whether the window answers a row now.
pub type MenuAnswers = Box<dyn Fn(Pick) -> bool>;

thread_local! {
    /// The app's main menu, as `buildMenuWithBuilder:` will file it.
    static MENU: RefCell<Option<UiArrangement>> = const { RefCell::new(None) };
    /// What each built row runs, by the index its property list carries,
    /// and the single stroke it stands for. A rebuild replaces it whole.
    static MENU_ROWS: RefCell<Vec<(Pick, Option<KeyPattern>)>> = const { RefCell::new(Vec::new()) };
    /// Runs a chosen row: its stroke offered to the keymap first, the row
    /// after — installed by the shell, which holds the runtime.
    static MENU_GATE: RefCell<Option<MenuGate>> = const { RefCell::new(None) };
    /// Does the window answer a row now?
    static MENU_ANSWERS: RefCell<Option<MenuAnswers>> = const { RefCell::new(None) };
}

/// Files `menu` as the app's main menu; UIKit rebuilds it now if the app
/// has launched, or builds it the first time it is asked.
pub fn install_menu_bar(menu: UiArrangement) {
    MENU.with(|slot| *slot.borrow_mut() = Some(menu));
    if DELEGATE.with(Cell::get).is_null() {
        return;
    }
    unsafe {
        let system = msg_id(class("UIMenuSystem"), sel("mainSystem"));
        if !system.is_null() {
            msg_void(system, sel("setNeedsRebuild"));
        }
    }
}

/// Registers who runs a chosen row.
pub fn set_menu_gate(gate: MenuGate) {
    MENU_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

/// Registers who says whether the window answers a row.
pub fn set_menu_answers(answers: MenuAnswers) {
    MENU_ANSWERS.with(|slot| *slot.borrow_mut() = Some(answers));
}

fn menu_answers(pick: Pick) -> bool {
    MENU_ANSWERS.with(|slot| slot.borrow().as_ref().is_some_and(|answers| answers(pick)))
}

fn menu_run(pick: Pick, stroke: Option<KeyPattern>) {
    let _ = MENU_GATE.with(|slot| slot.borrow_mut().as_mut().is_some_and(|gate| gate(pick, stroke)));
}

/// UIKit's identifier for one of its own menus.
fn standard(menu: Standard) -> Id {
    unsafe {
        match menu {
            Standard::File => UIMenuFile,
            Standard::Edit => UIMenuEdit,
            Standard::View => UIMenuView,
            Standard::Window => UIMenuWindow,
            Standard::Help => UIMenuHelp,
        }
    }
}

/// An `NSArray` of `items`.
unsafe fn array(items: &[Id]) -> Id {
    unsafe { msg_array(class("NSArray"), sel("arrayWithObjects:count:"), items.as_ptr(), items.len() as u64) }
}

/// A `UIMenu` — inline (a group) or a titled menu of its own.
unsafe fn menu(title: &str, identifier: Id, options: u64, children: &[Id]) -> Id {
    unsafe {
        msg_menu(
            class("UIMenu"),
            sel("menuWithTitle:image:identifier:options:children:"),
            ns_string(title),
            null_mut(),
            identifier,
            options,
            array(children),
        )
    }
}

/// UIKit's input string for a key command.
unsafe fn input(key: UiInput) -> Id {
    unsafe {
        match key {
            UiInput::Text(character) => ns_string(&character.to_string()),
            UiInput::Named(named) => match named {
                NamedKey::Up => UIKeyInputUpArrow,
                NamedKey::Down => UIKeyInputDownArrow,
                NamedKey::Left => UIKeyInputLeftArrow,
                NamedKey::Right => UIKeyInputRightArrow,
                NamedKey::Escape => UIKeyInputEscape,
                NamedKey::PageUp => UIKeyInputPageUp,
                NamedKey::PageDown => UIKeyInputPageDown,
                NamedKey::Home => UIKeyInputHome,
                NamedKey::End => UIKeyInputEnd,
                NamedKey::Delete => UIKeyInputDelete,
                NamedKey::F(number) => [
                    UIKeyInputF1,
                    UIKeyInputF2,
                    UIKeyInputF3,
                    UIKeyInputF4,
                    UIKeyInputF5,
                    UIKeyInputF6,
                    UIKeyInputF7,
                    UIKeyInputF8,
                    UIKeyInputF9,
                    UIKeyInputF10,
                    UIKeyInputF11,
                    UIKeyInputF12,
                ][usize::from(number.clamp(1, 12)) - 1],
            },
        }
    }
}

/// One row: a key command where UIKit can name the stroke, a command
/// otherwise; its property list is the index of what it runs.
unsafe fn command(row: &UiCommand) -> Id {
    let index = MENU_ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        rows.push((row.pick(), row.stroke()));
        rows.len() - 1
    });
    unsafe {
        let plist = msg_id_u64(class("NSNumber"), sel("numberWithUnsignedLongLong:"), index as u64);
        let title = ns_string(&row.title);
        match &row.key {
            Some(key) => msg_key_command(
                class("UIKeyCommand"),
                sel("commandWithTitle:image:action:input:modifierFlags:propertyList:"),
                title,
                null_mut(),
                sel("bunnyMenuPick:"),
                input(key.input),
                key.modifiers,
                plist,
            ),
            None => msg_command(
                class("UICommand"),
                sel("commandWithTitle:image:action:propertyList:"),
                title,
                null_mut(),
                sel("bunnyMenuPick:"),
                plist,
            ),
        }
    }
}

unsafe fn element(item: &UiItem) -> Id {
    unsafe {
        match item {
            UiItem::Command(row) => command(row),
            UiItem::Submenu { title, groups } => menu(title, null_mut(), 0, &self::groups(groups)),
        }
    }
}

/// Each group an inline menu — UIKit rules them apart.
unsafe fn groups(groups: &[Vec<UiItem>]) -> Vec<Id> {
    groups
        .iter()
        .map(|group| {
            let items: Vec<Id> = group.iter().map(|item| unsafe { element(item) }).collect();
            unsafe { menu("", null_mut(), INLINE, &items) }
        })
        .collect()
}

unsafe fn exists(builder: Id, identifier: Id) -> bool {
    unsafe { !msg_id_id(builder, sel("menuForIdentifier:"), identifier).is_null() }
}

/// `buildMenuWithBuilder:` — the app's main menu, filed into UIKit's
/// (`bunny_ui_apple::uikit_menu` decided where; this only builds it).
/// Context menus ask the same method with another system, and get nothing.
extern "C" fn bunny_build_menu(_this: Id, _sel: Sel, builder: Id) {
    let Some(arranged) = MENU.with(|slot| slot.borrow().clone()) else { return };
    unsafe {
        let main = msg_id(class("UIMenuSystem"), sel("mainSystem"));
        if !std::ptr::eq(msg_id(builder, sel("system")), main) {
            return;
        }
        MENU_ROWS.with(|rows| rows.borrow_mut().clear());
        for gone in [UIMenuFormat, UIMenuFind] {
            msg_void_id(builder, sel("removeMenuForIdentifier:"), gone);
        }
        if !arranged.about {
            msg_void_id(builder, sel("removeMenuForIdentifier:"), UIMenuAbout);
        }
        if let Some(settings) = &arranged.settings {
            let group = menu("", UIMenuPreferences, INLINE, &[command(settings)]);
            if exists(builder, UIMenuPreferences) {
                msg_void_id_id(builder, sel("replaceMenuForIdentifier:withMenu:"), UIMenuPreferences, group);
            } else {
                msg_void_id_id(builder, sel("insertChildMenu:atStartOfMenuForIdentifier:"), group, UIMenuApplication);
            }
        }
        // the menu each new one stands after, as it was actually built: an
        // anchor UIKit does not hold is a menu inserted nowhere, silently
        let mut built: Vec<(Anchor, Id)> = Vec::new();
        for filed in &arranged.menus {
            let children = groups(&filed.groups);
            match &filed.place {
                Place::Into(own) if exists(builder, standard(*own)) => {
                    for group in children {
                        msg_void_id_id(builder, sel("insertChildMenu:atEndOfMenuForIdentifier:"), group, standard(*own));
                    }
                    built.push((Anchor::Standard(*own), standard(*own)));
                }
                place => {
                    let identifier = match place {
                        Place::Into(own) => standard(*own),
                        Place::After(_) => ns_string(&filed.identifier),
                    };
                    let after = match place {
                        Place::After(anchor) => built
                            .iter()
                            .rev()
                            .find(|(placed, _)| placed == anchor)
                            .map(|(_, id)| *id)
                            .filter(|id| exists(builder, *id)),
                        Place::Into(_) => built.last().map(|(_, id)| *id),
                    }
                    .unwrap_or(if exists(builder, UIMenuView) { UIMenuView } else { UIMenuApplication });
                    let own = menu(&filed.title, identifier, 0, &children);
                    msg_void_id_id(builder, sel("insertSiblingMenu:afterMenuForIdentifier:"), own, after);
                    let anchor = match place {
                        Place::Into(own) => Anchor::Standard(*own),
                        Place::After(_) => Anchor::Own(filed.identifier.clone()),
                    };
                    built.push((anchor, identifier));
                }
            }
        }
        // `BUNNY_IOS_TRACE=1`: the main menu as UIKit HOLDS it after the
        // build — read back from the builder, so a menu it dropped shows as
        // missing rather than as asked for
        if std::env::var_os("BUNNY_IOS_TRACE").is_some() {
            for (filed, (_, identifier)) in arranged.menus.iter().zip(&built) {
                let held = msg_id_id(builder, sel("menuForIdentifier:"), *identifier);
                let rows = if held.is_null() {
                    "MISSING".to_owned()
                } else {
                    format!("{} groups", msg_u64(msg_id(held, sel("children")), sel("count")))
                };
                eprintln!("bunny_ui ios: main menu {:?}: {rows}", filed.title);
            }
            let settings = exists(builder, UIMenuPreferences);
            eprintln!("bunny_ui ios: main menu Settings group held: {settings}");
        }
    }
}

/// What a row of ours runs, read off the command UIKit hands back.
fn row_of(command: Id) -> Option<(Pick, Option<KeyPattern>)> {
    if command.is_null() {
        return None;
    }
    let index = unsafe {
        let plist = msg_id(command, sel("propertyList"));
        let number = class("NSNumber");
        if plist.is_null() || msg_bool_id(plist, sel("isKindOfClass:"), number) == 0 {
            return None;
        }
        usize::try_from(msg_u64(plist, sel("unsignedLongLongValue"))).ok()?
    };
    MENU_ROWS.with(|rows| rows.borrow().get(index).copied())
}

/// `bunnyMenuPick:` — a row was chosen, by a click or by its keys. Its
/// stroke goes to the keymap first: UIKit matched the key command before
/// the view heard the key (`uikit_menu`'s gotcha).
extern "C" fn bunny_menu_pick(_this: Id, _sel: Sel, command: Id) {
    if let Some((pick, stroke)) = row_of(command) {
        menu_run(pick, stroke);
    }
}

/// `validateCommand:` — a row the window does not answer is dark. A
/// command that is not ours keeps the class's own answer.
unsafe fn validate(this: Id, command: Id, superclass: &str) {
    match row_of(command) {
        Some((pick, _)) => {
            if !menu_answers(pick) {
                unsafe { msg_void_u64(command, sel("setAttributes:"), DISABLED) };
            }
        }
        None => unsafe {
            let sup = ObjcSuper { receiver: this, class: class(superclass) };
            msg_super_void_id(&sup, sel("validateCommand:"), command);
        },
    }
}

extern "C" fn bunny_view_validate(this: Id, _sel: Sel, command: Id) {
    unsafe { validate(this, command, "UIView") }
}

extern "C" fn bunny_delegate_validate(this: Id, _sel: Sel, command: Id) {
    unsafe { validate(this, command, "UIResponder") }
}

/// The standard edits this shell answers, by UIKit's selector.
fn edit_of(action: Sel) -> Option<Edit> {
    let name = unsafe { CStr::from_ptr(bunny_ui_apple::ffi::sel_getName(action)) }.to_str().ok()?;
    match name {
        "cut:" => Some(Edit::Cut),
        "copy:" => Some(Edit::Copy),
        "paste:" => Some(Edit::Paste),
        "selectAll:" => Some(Edit::SelectAll),
        _ => None,
    }
}

/// `canPerformAction:withSender:` — our rows always find their target here,
/// an edit only while the window can take it; anything else is the class's.
unsafe fn can_perform(this: Id, action: Sel, sender: Id, superclass: &str) -> i8 {
    if std::ptr::eq(action, unsafe { sel("bunnyMenuPick:") }) {
        return 1;
    }
    match edit_of(action) {
        Some(edit) => i8::from(menu_answers(Pick::Edit(edit))),
        None => unsafe {
            let sup = ObjcSuper { receiver: this, class: class(superclass) };
            msg_super_bool_sel_id(&sup, sel("canPerformAction:withSender:"), action, sender)
        },
    }
}

extern "C" fn bunny_view_can_perform(this: Id, _sel: Sel, action: Sel, sender: Id) -> i8 {
    unsafe { can_perform(this, action, sender, "UIView") }
}

extern "C" fn bunny_delegate_can_perform(this: Id, _sel: Sel, action: Sel, sender: Id) -> i8 {
    unsafe { can_perform(this, action, sender, "UIResponder") }
}

/// `cut:`, `copy:`, `paste:` and `selectAll:` — UIKit's own Edit rows and
/// their keys, offered to the keymap as their stroke first, as every row is.
extern "C" fn bunny_edit(_this: Id, action: Sel, _sender: Id) {
    if let Some(edit) = edit_of(action) {
        menu_run(Pick::Edit(edit), Some(edit.stroke()));
    }
}

/// The menu's methods, on a responder class: the view when it holds the
/// keyboard, the delegate at the chain's end when nothing does.
unsafe fn add_menu_methods(class: Id, validate: *const c_void, can_perform: *const c_void) {
    let v_id = CString::new("v@:@").expect("type encoding");
    let can = CString::new("c@::@").expect("type encoding");
    unsafe {
        class_addMethod(class, sel("bunnyMenuPick:"), bunny_menu_pick as *const c_void, v_id.as_ptr());
        class_addMethod(class, sel("validateCommand:"), validate, v_id.as_ptr());
        class_addMethod(class, sel("canPerformAction:withSender:"), can_perform, can.as_ptr());
        for edit in ["cut:", "copy:", "paste:", "selectAll:"] {
            class_addMethod(class, sel(edit), bunny_edit as *const c_void, v_id.as_ptr());
        }
    }
}

// MARK: - Native hosts (a platform view in the hole the layout keeps)

use std::collections::HashMap;

use bunny_ui_apple::ffi::{
    ALPHA_PREMULTIPLIED_LAST, CGColorSpaceCreateDeviceRGB, CGColorSpaceRelease,
    CGDataProviderRelease, CGImageCreate, CGImageRelease, CGSize, kill_layer_actions,
    owned_provider,
};

/// One mounted native host: the clipping container (ours), the
/// tenant's view inside it, and the stamp the tenant last applied — a
/// changed spec re-instructs the view, it never re-mounts it.
struct HostSlot {
    container: Id,
    child: Id,
    stamp: String,
}

/// One segment surface: the scene's commands that painted ABOVE a
/// host, composited over the platform view — the sandwich that keeps
/// paint order the truth. The straight RGBA stays here because the hit
/// test reads it: a painted pixel claims the finger, a clear one lets
/// it fall through to the page below.
struct SegmentSlot {
    view: Id,
    rgba: Vec<u8>,
    width: usize,
    height: usize,
    scale: usize,
}

thread_local! {
    /// One platform view per HOST box, keyed by identity.
    static HOST_VIEWS: RefCell<HashMap<String, HostSlot>> = RefCell::new(HashMap::new());
    static SEGMENTS: RefCell<HashMap<String, SegmentSlot>> = RefCell::new(HashMap::new());
}

/// Mounts (once) and places a native host under `key`: a clipping
/// container of ours at the VISIBLE cut of the box, the tenant's view
/// inside it at the WHOLE box, so the cut shows through and the
/// content never rewraps. `frame` and `window` are the box and its
/// visible part in layout points — UIKit's own coordinates, no flip.
/// An empty window hides the view instead of unmounting it: a page
/// keeps its state while it is scrolled off.
///
/// `make` runs once, calls its placement continuation, and returns the
/// tenant's view with ONE retain that [`host_sweep`] releases. `update`
/// runs when `stamp` changed — how a navigation lands without a
/// re-mount.
pub fn host_place(
    key: &str,
    stamp: &str,
    frame: (f64, f64, f64, f64),
    window: (f64, f64, f64, f64),
    make: impl FnOnce(&dyn Fn(Id)) -> Id,
    update: impl FnOnce(Id, &str),
) {
    let scene = VIEW.with(Cell::get);
    if scene.is_null() {
        return;
    }
    // the make runs OUTSIDE the table's borrow: a tenant may report on
    // creation, and a report reads nothing here but a borrow held
    // across it is a habit that aborts elsewhere
    let known = HOST_VIEWS.with(|hosts| hosts.borrow().contains_key(key));
    if !known {
        let (container, child) = unsafe {
            let container = msg_init_rect(
                msg_id(class("UIView"), sel("alloc")),
                sel("initWithFrame:"),
                CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 0.0,
                        height: 0.0,
                    },
                },
            );
            // the container is the CLIP: whatever the tenant draws
            // stays inside the window the layout granted
            msg_void_bool(container, sel("setClipsToBounds:"), 1);
            let child = make(&|child| {
                msg_void_id(container, sel("addSubview:"), child);
                msg_void_id(scene, sel("addSubview:"), container);
                position_host(container, child, frame, window);
            });
            (container, child)
        };
        HOST_VIEWS.with(|hosts| {
            hosts.borrow_mut().insert(
                key.to_string(),
                HostSlot {
                    container,
                    child,
                    stamp: stamp.to_string(),
                },
            );
        });
    }
    let (container, child, stale) = HOST_VIEWS.with(|hosts| {
        let mut hosts = hosts.borrow_mut();
        let slot = hosts.get_mut(key).expect("the slot was just made");
        let stale = slot.stamp != stamp;
        if stale {
            slot.stamp = stamp.to_string();
        }
        (slot.container, slot.child, stale)
    });
    unsafe {
        position_host(container, child, frame, window);
    }
    if stale {
        update(child, stamp);
    }
}

/// Position a clipped UIKit tenant before initial content or retained resize.
unsafe fn position_host(
    container: Id,
    child: Id,
    frame: (f64, f64, f64, f64),
    window: (f64, f64, f64, f64),
) {
    let (x, y, w, h) = frame;
    let (vx, vy, vw, vh) = window;
    unsafe {
        if vw <= 0.0 || vh <= 0.0 {
            msg_void_bool(container, sel("setHidden:"), 1);
        } else {
            msg_void_bool(container, sel("setHidden:"), 0);
            msg_void_rect(
                container,
                sel("setFrame:"),
                CGRect {
                    origin: CGPoint {
                        x: x + vx,
                        y: y + vy,
                    },
                    size: CGSize {
                        width: vw,
                        height: vh,
                    },
                },
            );
            msg_void_rect(
                child,
                sel("setFrame:"),
                CGRect {
                    origin: CGPoint { x: -vx, y: -vy },
                    size: CGSize {
                        width: w,
                        height: h,
                    },
                },
            );
        }
    }
}

/// Removes the hosts that left the scene — the subtree went, the
/// platform view goes with it. Releases the two holds [`host_place`]
/// took: the container's alloc and the tenant's `make`.
pub fn host_sweep(alive: &[String]) {
    let dead: Vec<(String, Id, Id)> = HOST_VIEWS.with(|hosts| {
        let mut hosts = hosts.borrow_mut();
        let mut dead = Vec::new();
        hosts.retain(|key, slot| {
            if alive.iter().any(|path| path == key) {
                return true;
            }
            dead.push((key.clone(), slot.container, slot.child));
            false
        });
        dead
    });
    for (key, container, child) in dead {
        unsafe {
            msg_void(container, sel("removeFromSuperview"));
            crate::webview::retire(&key, child);
            msg_void(child, sel("release"));
            msg_void(container, sel("release"));
        }
    }
}

/// The tenant's view mounted under `key`, if any — where a drained
/// command is spent.
pub fn host_child(key: &str) -> Option<Id> {
    HOST_VIEWS.with(|hosts| hosts.borrow().get(key).map(|slot| slot.child))
}

fn host_container(key: &str) -> Option<Id> {
    HOST_VIEWS.with(|hosts| hosts.borrow().get(key).map(|slot| slot.container))
}

/// The segment view's whole hit policy, in one answer: a painted pixel
/// claims the finger, a clear one lets it fall through to the page.
/// `point` arrives in the view's OWN coordinates, top-left.
extern "C" fn bunny_segment_inside(this: Id, _sel: Sel, point: CGPoint, _event: Id) -> i8 {
    SEGMENTS.with(|segments| {
        // UIKit asks re-entrantly from a frame change; while the table
        // is being written the answer is "not mine" for one event
        let Ok(segments) = segments.try_borrow() else {
            return 0;
        };
        let Some(slot) = segments.values().find(|slot| std::ptr::eq(slot.view, this)) else {
            return 0;
        };
        let bounds = unsafe { msg_rect(this, sel("bounds")) };
        if point.x < 0.0 || point.y < 0.0 || point.x >= bounds.size.width || point.y >= bounds.size.height {
            return 0;
        }
        let column = ((point.x * slot.scale as f64) as usize).min(slot.width.saturating_sub(1));
        let row = ((point.y * slot.scale as f64) as usize).min(slot.height.saturating_sub(1));
        let index = (row * slot.width + column) * 4 + 3;
        match slot.rgba.get(index) {
            Some(alpha) if *alpha > 8 => 1,
            _ => 0,
        }
    })
}

/// Every touch the segment view claims goes to the scene's view
/// unchanged — the touches still answer `locationInView:` for any
/// view, so the shell's own handlers resolve them like any other.
extern "C" fn bunny_segment_forward(this: Id, cmd: Sel, touches: Id, event: Id) {
    unsafe {
        let superview = msg_id(this, sel("superview"));
        if !superview.is_null() {
            msg_void_id_id(superview, cmd, touches, event);
        }
    }
}

static REGISTER_SEGMENT: Once = Once::new();

unsafe fn register_segment_class() {
    REGISTER_SEGMENT.call_once(|| unsafe {
        let name = CString::new("BunnySegmentView").expect("class name");
        let segment = objc_allocateClassPair(class("UIView"), name.as_ptr(), 0);
        let inside_types = CString::new("c@:{CGPoint=dd}@").expect("type encoding");
        class_addMethod(
            segment,
            sel("pointInside:withEvent:"),
            bunny_segment_inside as *const c_void,
            inside_types.as_ptr(),
        );
        let forward_types = CString::new("v@:@@").expect("type encoding");
        for verb in [
            "touchesBegan:withEvent:",
            "touchesMoved:withEvent:",
            "touchesEnded:withEvent:",
            "touchesCancelled:withEvent:",
        ] {
            class_addMethod(
                segment,
                sel(verb),
                bunny_segment_forward as *const c_void,
                forward_types.as_ptr(),
            );
        }
        objc_registerClassPair(segment);
    });
}

/// Presents one segment — the commands that painted above `host_key`,
/// rasterized by the caller into straight RGBA sized to the CONTENT's
/// box, never the window. The surface mounts DIRECTLY ABOVE the host's
/// container, so content between two hosts lands between their pages.
/// `frame` is the content box in layout points.
pub fn segment_blit(
    key: &str,
    host_key: &str,
    rgba: &[u8],
    frame: (f64, f64, f64, f64),
    scale: usize,
    px_width: usize,
    px_height: usize,
) {
    let scene = VIEW.with(Cell::get);
    if scene.is_null() {
        return;
    }
    unsafe { register_segment_class() };
    // NOTHING Objective-C runs while the table is borrowed: UIKit asks
    // the hit question re-entrantly, and the answer reads this table
    let known = SEGMENTS.with(|segments| segments.borrow().get(key).map(|slot| slot.view));
    let view = match known {
        Some(view) => view,
        None => unsafe {
            let view = msg_init_rect(
                msg_id(class("BunnySegmentView"), sel("alloc")),
                sel("initWithFrame:"),
                CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 0.0, height: 0.0 } },
            );
            // the layer is OURS: its implicit actions die at birth
            kill_layer_actions(msg_id(view, sel("layer")));
            match host_container(host_key) {
                Some(container) => {
                    msg_void_id_id(scene, sel("insertSubview:aboveSubview:"), view, container)
                }
                None => msg_void_id(scene, sel("addSubview:"), view),
            }
            SEGMENTS.with(|segments| {
                segments.borrow_mut().insert(
                    key.to_string(),
                    SegmentSlot { view, rgba: Vec::new(), width: 0, height: 0, scale },
                );
            });
            view
        },
    };
    let (pointer, length) = SEGMENTS.with(|segments| {
        let mut segments = segments.borrow_mut();
        let slot = segments.get_mut(key).expect("the slot was just made");
        // the bytes arrive ALREADY premultiplied: a raster onto a
        // transparent ground leaves rgb = colour × coverage
        slot.rgba.clear();
        slot.rgba.extend_from_slice(rgba);
        slot.width = px_width;
        slot.height = px_height;
        slot.scale = scale;
        (slot.rgba.as_ptr(), slot.rgba.len())
    });
    let (x, y, w, h) = frame;
    unsafe {
        let provider = owned_provider(pointer, length);
        let space = CGColorSpaceCreateDeviceRGB();
        let image = CGImageCreate(
            px_width,
            px_height,
            8,
            32,
            px_width * 4,
            space,
            ALPHA_PREMULTIPLIED_LAST,
            provider,
            std::ptr::null(),
            false,
            0,
        );
        msg_void_rect(
            view,
            sel("setFrame:"),
            CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } },
        );
        let layer = msg_id(view, sel("layer"));
        if !layer.is_null() {
            msg_void_f64(layer, sel("setContentsScale:"), scale as f64);
            msg_void_id(layer, sel("setContents:"), image);
        }
        CGImageRelease(image);
        CGColorSpaceRelease(space);
        CGDataProviderRelease(provider);
    }
}

/// Re-places one segment without touching its pixels. A segment with
/// no surface yet is a no-op.
pub fn segment_place(key: &str, frame: (f64, f64, f64, f64)) {
    let Some(view) = SEGMENTS.with(|segments| segments.borrow().get(key).map(|slot| slot.view))
    else {
        return;
    };
    let (x, y, w, h) = frame;
    unsafe {
        msg_void_rect(
            view,
            sel("setFrame:"),
            CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } },
        );
    }
}

/// Removes the segments that left the scene — nothing painted above
/// the host this frame, so nothing covers it. The dead leave the table
/// FIRST and their views after: removal asks the hit question too.
pub fn segment_sweep(alive: &[String]) {
    let dead: Vec<Id> = SEGMENTS.with(|segments| {
        let mut segments = segments.borrow_mut();
        let mut dead = Vec::new();
        segments.retain(|key, slot| {
            if alive.iter().any(|path| path == key) {
                return true;
            }
            dead.push(slot.view);
            false
        });
        dead
    });
    for view in dead {
        unsafe {
            msg_void(view, sel("removeFromSuperview"));
            msg_void(view, sel("release"));
        }
    }
}
