//! Hand-written UIKit FFI — zero dependencies.
//!
//! This module is the shell's `unsafe` border on the phone. UIKit is
//! called through `objc_msgSend` re-declared with the concrete signature
//! of each message (the shared Apple half carries the vocabulary; every
//! module declares its own aliases), and three classes are born at
//! runtime via `objc_allocateClassPair`/`class_addMethod`:
//!
//! - `BunnyAppDelegate` (UIResponder) — the application delegate: the
//!   launch that builds the window, the background and foreground, a
//!   memory warning, a url handed over, and the app's two clocks (the
//!   caret blink, the display link) delivered by selector;
//! - `BunnyViewController` (UIViewController) — the safe area and the
//!   traits (dark, the size class), which UIKit tells the controller;
//! - `BunnyView` (UIView) — whose backing layer IS a `CAMetalLayer`
//!   (`+layerClass`), which hears every touch, and which is the
//!   responder the soft keyboard types into (`UIKeyInput`).
//!
//! UIKit counts from the top-left in points, like the layout: no flip
//! happens anywhere here. Callbacks reach the Rust world through a
//! thread-local handler (UIKit's main run loop is one thread, like the
//! rest of the engine), and an event raised from inside a handler waits
//! its turn instead of re-entering one.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;
use std::sync::Once;

use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;
use bunny_ui_apple::ffi::{
    CGPoint, CGRect, Id, NSRunLoopCommonModes, ObjcSuper, Sel, class, class_addMethod,
    class_addProtocol, modifiers_of, objc_allocateClassPair, objc_getProtocol,
    objc_registerClassPair, object_getClass, sel, text_argument_to_string,
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
    fn msg_id_id(obj: Id, sel: Sel, a: Id) -> Id;
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
    /// A hardware key the keymap gate did not take, that types nothing
    /// — an arrow, a forward delete, an escape, a chord with command.
    Key(KeyStroke),
    /// The caret's blink half-period.
    Blink,
    /// One display-link tick; `dt` seconds since the last, clamped.
    Frame { dt: f64 },
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
/// and the window exists — it mounts the scene. UIKit's main loop never
/// returns.
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

/// The launch: the window, its controller and the view are built here,
/// the beat starts, the mount runs, and the window shows.
extern "C" fn bunny_did_finish_launching(this: Id, _sel: Sel, _app: Id, _options: Id) -> i8 {
    unsafe {
        let screen = msg_id(class("UIScreen"), sel("mainScreen"));
        let bounds = msg_rect(screen, sel("bounds"));
        let window =
            msg_init_rect(msg_id(class("UIWindow"), sel("alloc")), sel("initWithFrame:"), bounds);
        let controller = msg_id(msg_id(class("BunnyViewController"), sel("alloc")), sel("init"));
        let view =
            msg_init_rect(msg_id(class("BunnyView"), sel("alloc")), sel("initWithFrame:"), bounds);
        // off, a view is handed exactly one touch and the second finger
        // is never reported at all
        msg_void_bool(view, sel("setMultipleTouchEnabled:"), 1);
        msg_void_id(controller, sel("setView:"), view);
        msg_void_id(window, sel("setRootViewController:"), controller);
        WINDOW.with(|slot| slot.set(window));
        VIEW.with(|slot| slot.set(view));
        DELEGATE.with(|slot| slot.set(this));
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
        // the mount: the GPU road, the handler, the gates
        if let Some(boot) = BOOT.with(|slot| slot.borrow_mut().take()) {
            boot();
        }
        msg_void(window, sel("makeKeyAndVisible"));
    }
    dispatch(AppEvent::Redraw);
    1
}

extern "C" fn bunny_did_enter_background(_this: Id, _sel: Sel, _app: Id) {
    BACKGROUNDED.with(|slot| slot.set(true));
    // the link parks: a frame drawn off the screen is a frame the
    // system kills the app for
    set_frame_driver(DriverPace::Off);
    dispatch(AppEvent::Background);
    let _ = bunny_ui::app::emit(bunny_ui::app::AppEvent::WillSleep);
}

extern "C" fn bunny_will_enter_foreground(_this: Id, _sel: Sel, _app: Id) {
    BACKGROUNDED.with(|slot| slot.set(false));
    dispatch(AppEvent::Foreground);
    let _ = bunny_ui::app::emit(bunny_ui::app::AppEvent::DidWake);
}

extern "C" fn bunny_memory_warning(_this: Id, _sel: Sel, _app: Id) {
    dispatch(AppEvent::MemoryWarning);
}

/// A url handed to the app — the deep link, the same event a second
/// launch delivers everywhere else.
extern "C" fn bunny_open_url(_this: Id, _sel: Sel, _app: Id, url: Id, _options: Id) -> i8 {
    let text = unsafe {
        let string = msg_id(url, sel("absoluteString"));
        let chars = msg_cstr(string, sel("UTF8String"));
        if chars.is_null() { String::new() } else { CStr::from_ptr(chars).to_string_lossy().into_owned() }
    };
    let _ = bunny_ui::app::emit(bunny_ui::app::AppEvent::Reopened { arguments: vec![text] });
    1
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
    dispatch(AppEvent::Frame { dt: SLOW.with(|slot| slot.get().1) });
}

extern "C" fn bunny_frame(_this: Id, _sel: Sel, link: Id) {
    let dt = unsafe {
        let last = msg_f64(link, sel("timestamp"));
        let next = msg_f64(link, sel("targetTimestamp"));
        // the first tick after a resume reports the whole pause as the
        // gap — a clamped step keeps springs continuous
        (next - last).clamp(0.0, 1.0 / 30.0)
    };
    dispatch(AppEvent::Frame { dt });
}

/// The keyboard moved: its frame after the move, in screen coordinates,
/// against the view's bottom edge — the overlap is what the layout must
/// give up.
extern "C" fn bunny_keyboard(_this: Id, _sel: Sel, note: Id) {
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

/// The touches of an `NSSet`, by address.
unsafe fn touches_of(set: Id) -> Vec<Id> {
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
    for touch in unsafe { touches_of(touches) } {
        let (x, y) = unsafe { touch_point(touch, this) };
        let taps = unsafe { msg_u64(touch, sel("tapCount")) }.clamp(1, u8::MAX as u64) as u8;
        dispatch(AppEvent::TouchBegan { id: touch as u64, x, y, taps });
    }
}

/// `touchesMoved:` carries only the fingers that moved; a finger held
/// still during a pinch is in the EVENT's set and nowhere else — so the
/// event is asked, and every live finger reports where it stands.
extern "C" fn bunny_touches_moved(this: Id, _sel: Sel, _touches: Id, event: Id) {
    let all = unsafe { touches_of(msg_id(event, sel("allTouches"))) };
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
    for touch in unsafe { touches_of(touches) } {
        let (x, y) = unsafe { touch_point(touch, this) };
        dispatch(AppEvent::TouchEnded { id: touch as u64, x, y });
    }
}

extern "C" fn bunny_touches_cancelled(_this: Id, _sel: Sel, touches: Id, _event: Id) {
    for touch in unsafe { touches_of(touches) } {
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

extern "C" fn bunny_no(_this: Id, _sel: Sel) -> i8 {
    0
}

/// `UITextInputTraits` answered by hand: an empty adoption leaves every
/// getter nil, and the keyboard then guesses — sentence capitals and
/// autocorrection on a field that wanted neither. The traits say NO
/// (the enum's `…No` is 1) and `autocapitalizationType` says none (0).
extern "C" fn bunny_trait_no(_this: Id, _sel: Sel) -> i64 {
    1
}

extern "C" fn bunny_trait_zero(_this: Id, _sel: Sel) -> i64 {
    0
}

extern "C" fn bunny_insert_text(_this: Id, _sel: Sel, text: Id) {
    let text = unsafe { text_argument_to_string(text) };
    if !text.is_empty() {
        dispatch(AppEvent::Text(text));
    }
}

extern "C" fn bunny_delete_backward(_this: Id, _sel: Sel) {
    dispatch(AppEvent::DeleteBackward);
}

extern "C" fn bunny_claim_keyboard(this: Id, _sel: Sel) {
    unsafe {
        let _ = msg_bool(this, sel("becomeFirstResponder"));
    }
}

extern "C" fn bunny_drop_keyboard(this: Id, _sel: Sel) {
    unsafe {
        let _ = msg_bool(this, sel("resignFirstResponder"));
    }
}

/// The HID usages that type nothing and therefore never reach
/// `insertText:` — the ones a press is the only word of.
fn silent_key(hid: u64) -> bool {
    matches!(hid, 0x29 | 0x2B | 0x4A..=0x52)
}

/// A hardware key. The keymap gate hears it first; a silent key the
/// gate declines becomes an editing command; everything else goes on to
/// UIKit, which types it through `insertText:` — so a letter is never
/// typed twice, and backspace and return arrive by the keyboard's road.
extern "C" fn bunny_presses_began(this: Id, _sel: Sel, presses: Id, event: Id) {
    let mut taken = false;
    for press in unsafe { touches_of(presses) } {
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
        let bool_ = CString::new("c@:").expect("type encoding");
        let bool_id_id = CString::new("c@:@@").expect("type encoding");
        let bool_id_id_id = CString::new("c@:@@@").expect("type encoding");
        let class_ = CString::new("#@:").expect("type encoding");
        let int = CString::new("q@:").expect("type encoding");

        // the view: a CAMetalLayer for a skin, ears for every finger,
        // and the responder the keyboard types into
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
            ("pressesBegan:withEvent:", bunny_presses_began as *const c_void),
            ("pressesEnded:withEvent:", bunny_presses_ended as *const c_void),
        ] {
            class_addMethod(view, sel(name), imp, v_id_id.as_ptr());
        }
        class_addMethod(view, sel("layoutSubviews"), bunny_layout_subviews as *const c_void, v.as_ptr());
        class_addMethod(view, sel("canBecomeFirstResponder"), bunny_yes as *const c_void, bool_.as_ptr());
        class_addMethod(view, sel("hasText"), bunny_yes as *const c_void, bool_.as_ptr());
        class_addMethod(view, sel("isSecureTextEntry"), bunny_no as *const c_void, bool_.as_ptr());
        class_addMethod(view, sel("insertText:"), bunny_insert_text as *const c_void, v_id.as_ptr());
        class_addMethod(view, sel("deleteBackward"), bunny_delete_backward as *const c_void, v.as_ptr());
        class_addMethod(view, sel("bunnyClaimKeyboard"), bunny_claim_keyboard as *const c_void, v.as_ptr());
        class_addMethod(view, sel("bunnyDropKeyboard"), bunny_drop_keyboard as *const c_void, v.as_ptr());
        for name in [
            "autocorrectionType",
            "spellCheckingType",
            "smartQuotesType",
            "smartDashesType",
            "smartInsertDeleteType",
        ] {
            class_addMethod(view, sel(name), bunny_trait_no as *const c_void, int.as_ptr());
        }
        for name in ["autocapitalizationType", "keyboardType", "returnKeyType", "keyboardAppearance"] {
            class_addMethod(view, sel(name), bunny_trait_zero as *const c_void, int.as_ptr());
        }
        for protocol in ["UIKeyInput", "UITextInputTraits"] {
            let name = CString::new(protocol).expect("protocol name");
            let protocol = objc_getProtocol(name.as_ptr());
            if !protocol.is_null() {
                class_addProtocol(view, protocol);
            }
        }
        objc_registerClassPair(view);

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
            sel("application:openURL:options:"),
            bunny_open_url as *const c_void,
            bool_id_id_id.as_ptr(),
        );
        for (name, imp) in [
            ("applicationDidEnterBackground:", bunny_did_enter_background as *const c_void),
            ("applicationWillEnterForeground:", bunny_will_enter_foreground as *const c_void),
            ("applicationDidReceiveMemoryWarning:", bunny_memory_warning as *const c_void),
            ("bunnyBlink:", bunny_blink as *const c_void),
            ("bunnySlow:", bunny_slow as *const c_void),
            ("bunnyFrame:", bunny_frame as *const c_void),
            ("bunnyKeyboard:", bunny_keyboard as *const c_void),
        ] {
            class_addMethod(delegate, sel(name), imp, v_id.as_ptr());
        }
        objc_registerClassPair(delegate);
    });
}
