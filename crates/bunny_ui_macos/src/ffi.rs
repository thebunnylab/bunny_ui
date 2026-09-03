//! Hand-written Objective-C / CoreGraphics FFI — zero dependencies.
//!
//! This module is the project's sanctioned `unsafe` border: the
//! Objective-C runtime is called through `objc_msgSend` re-declared with
//! the concrete signature of each message (on arm64 there is ONE single
//! entry point for all messages — small structs go and come back in
//! registers, no `_stret` variant), and two classes are born at runtime
//! via `objc_allocateClassPair`/`class_addMethod`:
//!
//! - `BunnyView` (NSView) — receives the full pointer cycle
//!   (`mouseDown:`/`mouseUp:`/`mouseMoved:`/`mouseDragged:`/enter and
//!   exit via NSTrackingArea) and converts each position to layout
//!   coordinates (AppKit counts from the bottom up; the flip happens
//!   here, once);
//! - `BunnyDelegate` (NSObject) — `windowDidResize:` repaints and
//!   `windowWillClose:` quits the app (closing the window closes the
//!   process).
//!
//! Callbacks reach the Rust world through a thread-local handler (the
//! AppKit run loop is single-thread, like the rest of the engine).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CString, c_char, c_void};
use std::sync::Once;
use std::sync::atomic::{AtomicPtr, Ordering};

pub type Id = *mut c_void;
pub type Sel = *const c_void;

/// `NSRange` — (location, length) in UTF-16 units, the vocabulary of the
/// input system.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NSRange {
    pub location: u64,
    pub length: u64,
}

/// `NSNotFound` (NSIntegerMax) — AppKit's "no range".
pub const NS_NOT_FOUND: u64 = i64::MAX as u64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

// Re-declaring `objc_msgSend` with the concrete signature of each message
// is the runtime's designed usage (the symbol is a trampoline that
// preserves the call ABI) — the clashing-declarations lint does not apply.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_getProtocol(name: *const c_char) -> Id;
    fn class_addProtocol(class: Id, protocol: Id) -> i8;
    fn sel_getName(sel: Sel) -> *const c_char;

    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_void_f64(obj: Id, sel: Sel, a: f64);
    #[link_name = "objc_msgSend"]
    fn msg_void_u32(obj: Id, sel: Sel, a: u32);
    #[link_name = "objc_msgSend"]
    fn msg_f64(obj: Id, sel: Sel) -> f64;
    #[link_name = "objc_msgSend"]
    fn msg_bool_i64(obj: Id, sel: Sel, a: i64) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_point(obj: Id, sel: Sel) -> CGPoint;
    #[link_name = "objc_msgSend"]
    fn msg_rect(obj: Id, sel: Sel) -> CGRect;
    #[link_name = "objc_msgSend"]
    fn msg_init_rect(obj: Id, sel: Sel, rect: CGRect) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_rect(obj: Id, sel: Sel, rect: CGRect);
    #[link_name = "objc_msgSend"]
    fn msg_init_window(obj: Id, sel: Sel, rect: CGRect, style: u64, backing: u64, defer: i8)
    -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_init_tracking(obj: Id, sel: Sel, rect: CGRect, options: u64, owner: Id, info: Id)
    -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_bool(obj: Id, sel: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_u16(obj: Id, sel: Sel) -> u16;
    #[link_name = "objc_msgSend"]
    fn msg_u64(obj: Id, sel: Sel) -> u64;
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
    #[link_name = "objc_msgSend"]
    fn msg_id_arg(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_bool_id_id(obj: Id, sel: Sel, a: Id, b: Id) -> i8;
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
    #[link_name = "objc_msgSend"]
    fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_rect_rect(obj: Id, sel: Sel, rect: CGRect) -> CGRect;
    #[link_name = "objc_msgSend"]
    fn msg_id_id_sel(obj: Id, sel: Sel, target: Id, selector: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_id_id(obj: Id, sel: Sel, a: Id, b: Id);
    #[link_name = "objc_msgSend"]
    fn msg_point_point(obj: Id, sel: Sel, point: CGPoint) -> CGPoint;
    #[link_name = "objc_msgSend"]
    fn msg_void_id_i64(obj: Id, sel: Sel, a: Id, b: i64);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_i64_id(obj: Id, sel: Sel, a: Id, b: i64, c: Id);
    #[link_name = "objc_msgSendSuper"]
    fn msg_super_bool_id(sup: *const ObjcSuper, sel: Sel, a: Id) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_void_rect_bool(obj: Id, sel: Sel, rect: CGRect, flag: i8);
    #[link_name = "objc_msgSend"]
    fn msg_void_i64(obj: Id, sel: Sel, a: i64);
    #[link_name = "objc_msgSend"]
    fn msg_void_u64(obj: Id, sel: Sel, a: u64);
    #[link_name = "objc_msgSend"]
    fn msg_void_size(obj: Id, sel: Sel, size: CGSize);
}

// AppKit/QuartzCore come in via the ObjC runtime; the link guarantees the
// classes.
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    /// The pasteboard string type (`public.utf8-plain-text`).
    static NSPasteboardTypeString: Id;
}
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    /// The run-loop mode set that keeps a callback alive during event
    /// tracking (live resize, menus) — the display link schedules here.
    static NSRunLoopCommonModes: Id;
}
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    pub(crate) fn CGColorSpaceRelease(space: *mut c_void);
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const u8,
        size: usize,
        release: *const c_void,
    ) -> *mut c_void;
    fn CGDataProviderCreateWithCFData(data: *const c_void) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    pub(crate) fn CGContextDrawImage(context: Id, rect: CGRect, image: Id);
    pub(crate) fn CGContextSetInterpolationQuality(context: Id, quality: i32);
    fn CGContextSaveGState(context: Id);
    fn CGContextRestoreGState(context: Id);
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
        provider: *mut c_void,
        decode: *const f64,
        should_interpolate: bool,
        intent: i32,
    ) -> Id;
    pub(crate) fn CGImageRelease(image: Id);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CFRelease(cf: *const c_void);
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
    fn CFRunLoopGetMain() -> Id;
    fn CFRunLoopSourceCreate(
        allocator: Id,
        order: isize,
        context: *mut CFRunLoopSourceContext,
    ) -> Id;
    fn CFRunLoopAddSource(loop_: Id, source: Id, mode: Id);
    fn CFRunLoopSourceSignal(source: Id);
    fn CFRunLoopWakeUp(loop_: Id);
    static kCFRunLoopCommonModes: Id;
}

/// The version-0 source context. Only `perform` matters here: the
/// source carries no state of its own, so every other hook stays null.
#[repr(C)]
struct CFRunLoopSourceContext {
    version: isize,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> Id>,
    equal: Option<extern "C" fn(*const c_void, *const c_void) -> u8>,
    hash: Option<extern "C" fn(*const c_void) -> usize>,
    schedule: Option<extern "C" fn(*mut c_void, Id, Id)>,
    cancel: Option<extern "C" fn(*mut c_void, Id, Id)>,
    perform: Option<extern "C" fn(*mut c_void)>,
}

pub(crate) unsafe fn class(name: &str) -> Id {
    let name = CString::new(name).expect("class name without NUL");
    unsafe { objc_getClass(name.as_ptr()) }
}

pub(crate) unsafe fn sel(name: &str) -> Sel {
    let name = CString::new(name).expect("selector without NUL");
    unsafe { sel_registerName(name.as_ptr()) }
}

// MARK: - Events

/// What the platform delivers to the Rust world. Positions in LAYOUT
/// coordinates (origin at top-left, logical points) — the AppKit flip
/// already happened.
///
/// `Clone` because an app with several windows fans the beats out: one
/// display-link tick reaches every window that wants it.
#[derive(Clone)]
pub enum AppEvent {
    MouseDown { x: f64, y: f64, clicks: u8, modifiers: bunny_ui::action::Modifiers },
    /// The right button (or a two-finger tap): the context-menu press.
    RightMouseDown { x: f64, y: f64 },
    MouseUp { x: f64, y: f64 },
    MouseMoved { x: f64, y: f64, modifiers: bunny_ui::action::Modifiers },
    /// The pointer left the window — without this event the hover would
    /// stay stuck at the edge (the reason for using NSTrackingArea).
    MouseExited,
    /// Scrolling: deltas in points (trackpad arrives precise and with
    /// momentum; the legacy wheel is converted from lines to points on
    /// arrival).
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    /// RAW key — only arrives here when the focused field is NOT in the
    /// path (no focus, or cmd held): shortcuts and function keys. With
    /// focus, the event enters the input system (`interpretKeyEvents:`)
    /// and comes back through the IME events below.
    Key { code: u16, shift: bool, command: bool, chars: String },
    /// The IME committed final text (or plain typing via the input system).
    ImeInsert { text: String },
    /// Live composition: the marked text + the selection INSIDE it (UTF-16).
    ImeMark { text: String, location: u64, length: u64 },
    /// The composition ended by committing what was marked.
    ImeUnmark,
    /// `doCommandBySelector:` — movement/editing by selector name
    /// ("moveLeft:", "deleteBackward:", …); the policy lives in the shell.
    Command { selector: String },
    /// Half-period of the caret blink (the shell's NSTimer).
    Blink,
    /// One display-link tick: compose the next animated frame. `dt` is
    /// the interval this frame covers, in seconds, already clamped.
    Frame { dt: f64 },
    /// The window changed size (or needs the first frame).
    Redraw,
    /// The window stopped being key (the user switched apps or
    /// windows) — open popovers close, the platform's own manner.
    ResignKey,
    /// The window is key again — a frozen decoration resumes.
    BecomeKey,
    /// A DIALOG window's close button was pressed. The window itself
    /// has not closed and will not close itself: the shell answers by
    /// running the overlay's dismissal, and the flipped binding is
    /// what takes the window down — one road out, the same one the
    /// app's own Escape takes.
    DialogClose {
        /// The dialog's `NSWindow`, as an address — the shell finds
        /// the overlay path in its pool by it.
        window: usize,
    },
    /// This window is going away, and it is not the last — the app
    /// stays up, and the shell drops what it kept for it. (The last
    /// window's close terminates the app instead, so this never
    /// arrives for it.)
    WindowClosed,
    /// A task woke from somewhere else — a worker thread finished a
    /// step. The frame the shell already knows how to draw drains the
    /// queue on its way.
    Wake,
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
    /// True while the app is lending a hosted page a synthetic hand
    /// ([`lend_hand`]).
    static LENDING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `work` with the SCENE's ears closed.
///
/// A synthetic event is addressed at the platform view, and whatever
/// the engine does not consume walks up the responder chain — into
/// the app's own view, which is the one that asked for the event in
/// the first place, inside the very frame that sent it. The scene
/// must not hear a phantom press at a point it never named, and the
/// re-entered handler would be a borrow of what is already borrowed.
pub(crate) fn lend_hand<T>(work: impl FnOnce() -> T) -> T {
    let held = LENDING.with(|lending| lending.replace(true));
    let answer = work();
    LENDING.with(|lending| lending.set(held));
    answer
}

/// Registers who receives the events (the shell's loop).
pub fn set_handler(handler: Box<dyn FnMut(AppEvent)>) {
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

thread_local! {
    /// Which window the event now in the handler belongs to — `0` for
    /// the beats every window shares (the frame tick, the caret blink,
    /// a worker's wake).
    static SOURCE: Cell<usize> = const { Cell::new(0) };
    /// A handler is running: anything raised from inside it queues.
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
    /// What was raised while a handler ran, in the order it was raised.
    static PENDING: RefCell<Vec<(usize, AppEvent)>> = const { RefCell::new(Vec::new()) };
    /// Every top-level window the app has open, in the order they were
    /// created, with the view and delegate that came with it. The app
    /// quits when the LAST one closes — with one window that is the old
    /// contract, word for word.
    static WINDOWS: RefCell<Vec<(usize, Id, Id)>> = const { RefCell::new(Vec::new()) };
    /// Which window carries the app's BEAT — the caret blink and the
    /// display link are one per app, and they hang off the view of
    /// whichever window was there first. When that window closes with
    /// others still open, the beat moves house.
    static BEAT_OWNER: Cell<usize> = const { Cell::new(0) };
}

/// What the OS gives a window besides its content.
///
/// A door has one size: the sign-in window is not resizable and not
/// minimizable, and neither is expressible any other way — the style
/// mask is set once, when the window is born.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manners {
    pub resizable: bool,
    pub minimizable: bool,
}

impl Default for Manners {
    /// The workbench's: it resizes and it minimizes.
    fn default() -> Self {
        Manners { resizable: true, minimizable: true }
    }
}

/// Drops everything keyed by a window that is going away.
///
/// While the app died with its only window, a stale entry here was
/// unreachable: nothing ticked after the close. Now windows close and the
/// app lives, so a registry that outlives its key is a pointer to a freed
/// object — and the traffic-light registry is walked by the DISPLAY LINK,
/// which is to say on the very next frame. That crash is what this
/// function exists to have already prevented.
fn forget_window(window: usize) {
    let view = WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .find(|(open, _, _)| *open == window)
            .map(|(_, view, _)| *view as usize)
    });
    PLACED_LIGHTS.with(|slot| slot.borrow_mut().retain(|(open, _)| *open as usize != window));
    // NOT `NATURAL_LIGHTS`: that is the SYSTEM's own geometry, read once on
    // purpose. Clearing it would let the next window read a frame we had
    // already moved, which is the compounding this file's oldest comment
    // about the lights exists to prevent.
    if let Some(view) = view {
        BACKING.with(|store| store.borrow_mut().remove(&view));
        PANEL_ORIGINS.with(|origins| origins.borrow_mut().remove(&view));
    }
}

/// Closes a top-level window. AppKit runs the delegate, which is where
/// the app's own bookkeeping (and the last-window rule) happens.
pub fn close_top_level(window: usize) {
    unsafe { msg_void(window as Id, sel("close")) };
}

/// Starts the app's beat on this window: the caret's blink half-period
/// and the display link that paces animation, both delivered by
/// selector to the window's own delegate on the main run loop.
///
/// One per APP, not one per window: a second link would tick the same
/// vsync twice and pay for every frame twice. The window it hangs off
/// is an implementation detail the close path repairs.
unsafe fn start_beat(window: Id, view: Id, delegate: Id) {
    unsafe {
        BEAT_OWNER.with(|owner| owner.set(window as usize));
        // the caret blink half-period — the run loop retains the timer
        let _ = msg_timer(
            class("NSTimer"),
            sel("scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:"),
            0.5,
            delegate,
            sel("bunnyBlink:"),
            std::ptr::null_mut(),
            1,
        );

        // the frame driver: a display link owned by the view, delivered
        // by SELECTOR on the main run loop (macOS 14+) — no blocks, no
        // extra thread. Born PAUSED: events repaint by themselves; the
        // link runs only while something animates. An older system
        // skips it and animations snap to their target.
        if msg_bool_sel(view, sel("respondsToSelector:"), sel("displayLinkWithTarget:selector:"))
            != 0
        {
            let link = msg_id_id_sel(
                view,
                sel("displayLinkWithTarget:selector:"),
                delegate,
                sel("bunnyFrame:"),
            );
            if !link.is_null() {
                msg_void_bool(link, sel("setPaused:"), 1);
                // the link arrives unscheduled — common modes keep the
                // ticks coming during event tracking (live resize)
                msg_void_id_id(
                    link,
                    sel("addToRunLoop:forMode:"),
                    msg_id(class("NSRunLoop"), sel("mainRunLoop")),
                    NSRunLoopCommonModes,
                );
                LINK.with(|slot| slot.set(link));
            }
        } else {
            eprintln!("bunny_ui: this macOS has no view display link; animations snap");
        }
    }
}

/// The window the event being handled came from, or `0` when it came
/// from the app itself. An app with one window never asks.
pub fn event_source() -> usize {
    SOURCE.with(Cell::get)
}

/// The window an event ANSWERS to: itself, or — for a dialog or a
/// popover panel, which hang off the window they belong to — the window
/// they hang from. A dialog's key change is its parent scene's news.
fn owning_window(window: Id) -> usize {
    let mut window = window;
    for _ in 0..8 {
        if window.is_null() {
            return 0;
        }
        let parent = unsafe { msg_id(window, sel("parentWindow")) };
        if parent.is_null() {
            return window as usize;
        }
        window = parent;
    }
    window as usize
}

/// Delivers an event that belongs to ONE window — the callbacks that
/// know which (a resize, a key change, a dialog's own news).
pub fn dispatch_to(window: Id, event: AppEvent) {
    dispatch_from(owning_window(window), event);
}

/// Delivers a beat every window shares — the frame tick, the blink, a
/// worker's wake. The app fans it out.
pub fn dispatch_all(event: AppEvent) {
    dispatch_from(0, event);
}

fn dispatch_from(source: usize, event: AppEvent) {
    if LENDING.with(Cell::get) {
        // the page declined it and the chain walked it back here —
        // see `lend_hand`
        return;
    }
    // An event raised from INSIDE a handler waits its turn instead of
    // re-entering one. The handler is borrowed for as long as it runs, and
    // this is an `extern "C"` frame: a borrow panic here cannot unwind, so
    // it aborts the process rather than failing a call. It happens for real
    // — a worker's Wake paints, the paint completes a sign-in, and the
    // sign-in opens the window that replaces the one being painted.
    //
    // Queued, not dropped: the second event still arrives, after the first
    // finishes, in the order it was raised. Dropping it would trade an abort
    // for a window that never draws.
    if DISPATCHING.with(Cell::get) {
        PENDING.with(|queue| queue.borrow_mut().push((source, event)));
        return;
    }
    DISPATCHING.with(|flag| flag.set(true));
    let previous = SOURCE.replace(source);
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
    SOURCE.set(previous);
    DISPATCHING.with(|flag| flag.set(false));
    // …and whatever the handler raised while it ran, in order. Each one is a
    // full dispatch, so an event raised by one of THOSE queues behind it.
    loop {
        let next = PENDING.with(|queue| {
            let mut queue = queue.borrow_mut();
            if queue.is_empty() { None } else { Some(queue.remove(0)) }
        });
        let Some((source, event)) = next else { break };
        dispatch_from(source, event);
    }
}

/// Delivers an event to the handler — used by the callbacks and by the
/// first frame. The window is the one holding the keyboard, which is
/// the window every INPUT event comes from: a press makes its window
/// key before AppKit sends it, and the tracking area is armed
/// `ActiveInKeyWindow`, so a hover in a background window never fires.
pub fn dispatch(event: AppEvent) {
    let key = unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_id(app, sel("keyWindow"))
    };
    dispatch_from(owning_window(key), event);
}

/// The run loop source a background thread knocks on. It lives in a
/// static (not a thread-local) because the signal comes from ANY
/// thread — `CFRunLoopSourceSignal` and `CFRunLoopWakeUp` are the
/// thread-safe half of CoreFoundation.
static WAKE_SOURCE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

extern "C" fn perform_wake(_info: *mut c_void) {
    dispatch_all(AppEvent::Wake);
}

/// Opens that door. Called once, on the main thread, while the window
/// is being built.
pub fn install_wake_source() {
    if !WAKE_SOURCE.load(Ordering::SeqCst).is_null() {
        return;
    }
    unsafe {
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: std::ptr::null_mut(),
            retain: None,
            release: None,
            copy_description: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(perform_wake),
        };
        let source = CFRunLoopSourceCreate(std::ptr::null_mut(), 0, &mut context);
        // COMMON modes: a live resize or a tracking loop must not
        // silence a task that just landed
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
        WAKE_SOURCE.store(source, Ordering::SeqCst);
    }
}

/// Asks the main run loop for one more turn. Safe from any thread, and
/// never re-entrant: a signal raised DURING a frame lands on the next
/// turn instead of nesting inside this one.
pub fn wake_from_any_thread() {
    let source = WAKE_SOURCE.load(Ordering::SeqCst);
    if source.is_null() {
        return;
    }
    unsafe {
        CFRunLoopSourceSignal(source);
        CFRunLoopWakeUp(CFRunLoopGetMain());
    }
}

thread_local! {
    /// The SCENE origin of every panel view — a popover's events
    /// translate back into scene coordinates here, so the runtime's
    /// hit-test never learns which surface the pointer touched.
    static PANEL_ORIGINS: RefCell<HashMap<usize, (f64, f64)>> =
        RefCell::new(HashMap::new());
}

/// The event position in layout coordinates — AppKit counts from the
/// bottom, the layout counts from the top; the flip lives here, once.
/// A panel view adds its scene origin: one translation, one place.
unsafe fn event_layout_point(this: Id, event: Id) -> (f64, f64) {
    unsafe {
        let point = msg_point(event, sel("locationInWindow"));
        let bounds = msg_rect(this, sel("bounds"));
        let (dx, dy) = PANEL_ORIGINS
            .with(|origins| origins.borrow().get(&(this as usize)).copied())
            .unwrap_or((0.0, 0.0));
        (point.x + dx, bounds.size.height - point.y + dy)
    }
}

thread_local! {
    /// The window-drag gate: `true` = this press drags the window (the
    /// scene declared a drag region there and nothing interactive won).
    static DRAG_GATE: RefCell<Option<Box<dyn Fn(f64, f64) -> bool>>> =
        const { RefCell::new(None) };
}

/// Installs the drag gate — the shell wires it to the runtime's drag
/// regions once, at boot.
pub fn set_drag_gate(gate: Box<dyn Fn(f64, f64) -> bool>) {
    DRAG_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

extern "C" fn bunny_right_mouse_down(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    dispatch(AppEvent::RightMouseDown { x, y });
}

/// The four the keymap names, out of one AppKit bitfield — the same
/// bits the key road reads, in one place so the two cannot drift.
fn modifiers_of(flags: u64) -> bunny_ui::action::Modifiers {
    bunny_ui::action::Modifiers {
        shift: flags & (1 << 17) != 0,
        control: flags & (1 << 18) != 0,
        option: flags & (1 << 19) != 0,
        command: flags & (1 << 20) != 0,
    }
}

extern "C" fn bunny_mouse_down(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    // the scene's own title bar: a press on a drag region (with no
    // interactive target above it) moves the WINDOW — the event goes
    // to AppKit whole and never reaches the runtime
    let dragging =
        DRAG_GATE.with(|gate| gate.borrow().as_ref().is_some_and(|gate| gate(x, y)));
    if dragging {
        unsafe {
            let window = msg_id(this, sel("window"));
            msg_void_id(window, sel("performWindowDragWithEvent:"), event);
        }
        return;
    }
    // AppKit already counts: 2 is the word, 3 is the line
    let clicks = unsafe { msg_u64(event, sel("clickCount")).min(255) as u8 };
    let modifiers = unsafe { modifiers_of(msg_u64(event, sel("modifierFlags"))) };
    dispatch(AppEvent::MouseDown { x, y, clicks, modifiers });
}

extern "C" fn bunny_mouse_up(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    dispatch(AppEvent::MouseUp { x, y });
}

/// `mouseMoved:`, `mouseDragged:` and `mouseEntered:` all land here —
/// dragged is MANDATORY: with the button held AppKit sends dragged, never
/// moved (without it the pressed visual won't release when dragging out).
extern "C" fn bunny_mouse_moved(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    // the same flags the press reads, off the same event — what a box
    // needs to offer a command-click before the hand commits to it
    let modifiers = unsafe { modifiers_of(msg_u64(event, sel("modifierFlags"))) };
    dispatch(AppEvent::MouseMoved { x, y, modifiers });
}

extern "C" fn bunny_mouse_exited(_this: Id, _sel: Sel, _event: Id) {
    dispatch(AppEvent::MouseExited);
}

/// BunnyView accepts first responder — without this, keyDown never arrives.
extern "C" fn bunny_accepts_first_responder(_this: Id, _sel: Sel) -> i8 {
    1
}

/// The raw AppKit key already extracted — the keymap gate's vocabulary.
pub struct KeyStroke {
    pub code: u16,
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
    pub chars: String,
    /// The character this key TYPED, under the modifiers actually
    /// held. `charactersByApplyingModifiers:` with the event's own
    /// flags — the same selector the bare read uses, asked the other
    /// question, so one API answers both and the layout answers both
    /// the same way.
    ///
    /// NOT `charactersIgnoringModifiers`, which was here before and
    /// which nobody read: it ignores OPTION, and option is exactly the
    /// modifier that types on the layouts this framework is written
    /// for — on this machine option and the digit four make a cent
    /// sign, and that read would have answered "4".
    pub typed: Option<char>,
    /// The key's OWN character, with no modifier applied at all
    /// (`charactersByApplyingModifiers:0`) — what a `Char` pattern must
    /// match, and read through the USER'S LAYOUT, so the Brazilian
    /// keyboard this is written on needs no table of US pairs.
    pub chars_bare: String,
}

thread_local! {
    /// The shell's keyboard gate: sees keyDown BEFORE the input system.
    /// `true` = the keymap dispatched — the event dies here.
    static KEY_GATE: RefCell<Option<Box<dyn FnMut(&KeyStroke) -> bool>>> =
        const { RefCell::new(None) };
}

/// Registers the gate (the shell installs it along with the event handler).
pub fn set_key_gate(gate: Box<dyn FnMut(&KeyStroke) -> bool>) {
    KEY_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
}

fn gate_consumed(stroke: &KeyStroke) -> bool {
    KEY_GATE.with(|slot| slot.borrow_mut().as_mut().is_some_and(|gate| gate(stroke)))
}

// MARK: - IME sync (the input system's SYNCHRONOUS questions)

/// The focused-field mirror that `NSTextInputClient` answers from on the
/// spot — the shell re-syncs it every frame (mutation travels through
/// events; reading travels through here). It carries the WHOLE text:
/// reconversion asks for arbitrary substrings.
#[derive(Clone)]
struct ImeMirror {
    text: std::rc::Rc<str>,
    selected: NSRange,
    marked: NSRange,
    caret_screen: CGRect,
}

thread_local! {
    static IME: RefCell<Option<ImeMirror>> = const { RefCell::new(None) };
    /// keyDown enters the input system (composition) only when the shell
    /// says a field is focused.
    static INTERPRET: Cell<bool> = const { Cell::new(false) };
    /// "Which UTF-16 index sits at this LAYOUT point?" — installed by
    /// the shell, capturing the runtime (zero-ivar classes: state lives
    /// beside the run loop).
    static IME_INDEX: RefCell<Option<Box<dyn Fn(f64, f64) -> Option<u64>>>> =
        const { RefCell::new(None) };
    /// "Where, on screen, is the caret rect for this UTF-16 index?" —
    /// the ranged half of firstRectForCharacterRange.
    static IME_RECT: RefCell<Option<Box<dyn Fn(u64) -> Option<CGRect>>>> =
        const { RefCell::new(None) };
}

/// The shell installs the two synchronous answers the input system may
/// ask beyond the mirror: index-at-point and rect-at-index.
pub fn set_ime_resolvers(
    index_at: Box<dyn Fn(f64, f64) -> Option<u64>>,
    rect_for: Box<dyn Fn(u64) -> Option<CGRect>>,
) {
    IME_INDEX.with(|slot| *slot.borrow_mut() = Some(index_at));
    IME_RECT.with(|slot| *slot.borrow_mut() = Some(rect_for));
}

/// The shell syncs the focused-field mirror (`None` = no focus).
pub fn sync_ime(state: Option<(std::rc::Rc<str>, NSRange, Option<NSRange>, CGRect)>) {
    INTERPRET.with(|flag| flag.set(state.is_some()));
    IME.with(|ime| {
        *ime.borrow_mut() = state.map(|(text, selected, marked, caret_screen)| ImeMirror {
            text,
            selected,
            marked: marked.unwrap_or(NSRange { location: NS_NOT_FOUND, length: 0 }),
            caret_screen,
        });
    });
}

fn ime_mirror() -> Option<ImeMirror> {
    IME.with(|ime| ime.borrow().clone())
}

/// NSString OR NSAttributedString → Rust (the input system sends both).
unsafe fn text_argument_to_string(object: Id) -> String {
    unsafe {
        if object.is_null() {
            return String::new();
        }
        let plain = if msg_bool_sel(object, sel("respondsToSelector:"), sel("string")) != 0 {
            msg_id(object, sel("string"))
        } else {
            object
        };
        let utf8 = msg_id(plain, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}

extern "C" fn bunny_insert_text(_this: Id, _sel: Sel, string: Id, _replacement: NSRange) {
    let text = unsafe { text_argument_to_string(string) };
    dispatch(AppEvent::ImeInsert { text });
}

extern "C" fn bunny_set_marked_text(
    _this: Id,
    _sel: Sel,
    string: Id,
    selected: NSRange,
    _replacement: NSRange,
) {
    let text = unsafe { text_argument_to_string(string) };
    dispatch(AppEvent::ImeMark { text, location: selected.location, length: selected.length });
}

extern "C" fn bunny_unmark_text(_this: Id, _sel: Sel) {
    dispatch(AppEvent::ImeUnmark);
}

extern "C" fn bunny_has_marked_text(_this: Id, _sel: Sel) -> i8 {
    i8::from(ime_mirror().is_some_and(|ime| ime.marked.location != NS_NOT_FOUND))
}

extern "C" fn bunny_marked_range(_this: Id, _sel: Sel) -> NSRange {
    ime_mirror()
        .map(|ime| ime.marked)
        .unwrap_or(NSRange { location: NS_NOT_FOUND, length: 0 })
}

extern "C" fn bunny_selected_range(_this: Id, _sel: Sel) -> NSRange {
    ime_mirror()
        .map(|ime| ime.selected)
        .unwrap_or(NSRange { location: 0, length: 0 })
}

extern "C" fn bunny_attributed_substring(
    _this: Id,
    _sel: Sel,
    range: NSRange,
    actual: *mut NSRange,
) -> Id {
    // reconversion and candidate previews ask for arbitrary substrings
    // — answered from the mirror's full text, clamped to what exists
    let Some(ime) = ime_mirror() else {
        return std::ptr::null_mut();
    };
    let text: &str = &ime.text;
    let total = text.encode_utf16().count() as u64;
    let location = range.location.min(total);
    let length = range.length.min(total - location);
    let start = bunny_ui::text_input::utf16_to_byte(text, location as usize);
    let end = bunny_ui::text_input::utf16_to_byte(text, (location + length) as usize);
    let Ok(slice) = CString::new(&text[start..end]) else {
        return std::ptr::null_mut();
    };
    unsafe {
        if !actual.is_null() {
            *actual = NSRange { location, length };
        }
        let string =
            msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), slice.as_ptr());
        let attributed = msg_id_arg(
            msg_id(class("NSAttributedString"), sel("alloc")),
            sel("initWithString:"),
            string,
        );
        // the input system owns the answer from here
        msg_id(attributed, sel("autorelease"))
    }
}

extern "C" fn bunny_valid_attributes(_this: Id, _sel: Sel) -> Id {
    unsafe { msg_id(class("NSArray"), sel("array")) }
}

/// Where the candidate window lands: the rect at the REQUESTED index
/// (the composition's start, usually) — the caret rect as fallback.
extern "C" fn bunny_first_rect(
    _this: Id,
    _sel: Sel,
    range: NSRange,
    actual: *mut NSRange,
) -> CGRect {
    if !actual.is_null() {
        unsafe { *actual = NSRange { location: range.location, length: 0 } };
    }
    let ranged = (range.location != NS_NOT_FOUND)
        .then(|| {
            IME_RECT.with(|slot| {
                slot.borrow().as_ref().and_then(|resolve| resolve(range.location))
            })
        })
        .flatten();
    ranged
        .or_else(|| ime_mirror().map(|ime| ime.caret_screen))
        .unwrap_or(CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 0.0, height: 0.0 },
        })
}

extern "C" fn bunny_character_index(this: Id, _sel: Sel, point: CGPoint) -> u64 {
    // dictionary lookup by mouse: screen → window → layout, then the
    // shell's resolver answers from the live field
    unsafe {
        let window = msg_id(this, sel("window"));
        if window.is_null() {
            return NS_NOT_FOUND;
        }
        let in_window = msg_point_point(window, sel("convertPointFromScreen:"), point);
        let bounds = msg_rect(this, sel("bounds"));
        // a panel (or dialog) view answers in SCENE coordinates, like
        // every other event it delivers — the same translation
        // `event_layout_point` applies
        let (dx, dy) = PANEL_ORIGINS
            .with(|origins| origins.borrow().get(&(this as usize)).copied())
            .unwrap_or((0.0, 0.0));
        let (x, y) = (in_window.x + dx, bounds.size.height - in_window.y + dy);
        IME_INDEX
            .with(|slot| slot.borrow().as_ref().and_then(|resolve| resolve(x, y)))
            .unwrap_or(NS_NOT_FOUND)
    }
}

extern "C" fn bunny_do_command(_this: Id, _sel: Sel, command: Sel) {
    let selector = unsafe {
        let name = sel_getName(command);
        if name.is_null() {
            return;
        }
        std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned()
    };
    dispatch(AppEvent::Command { selector });
}

/// The key's characters with NO modifier applied — AppKit reads the
/// user's own layout, which is why this beats any table: on a Brazilian
/// ABNT2 keyboard the physical key that types `|` under shift answers
/// its own base character here, whatever that is.
///
/// The selector arrived in macOS 10.15 (a test in this file pins that it
/// is here). Should it ever be missing, the base falls back to
/// `charactersIgnoringModifiers` — the old, shift-applied answer, which
/// is exactly today's behaviour and never a crash.
unsafe fn bare_characters(event: Id) -> String {
    unsafe {
        let selector = sel("charactersByApplyingModifiers:");
        let fallback = || text_argument_to_string(msg_id(event, sel("charactersIgnoringModifiers")));
        if msg_bool_sel(class("NSEvent"), sel("instancesRespondToSelector:"), selector) == 0 {
            return fallback();
        }
        let bare = text_argument_to_string(msg_id_u64(event, selector, 0));
        // a dead key, a keypad key or a non-printing one can answer with
        // nothing at all — then the shift-applied read is still better
        // than no character, and the named-key table has already had
        // its turn anyway
        if bare.is_empty() { fallback() } else { bare }
    }
}

/// What the key put on the screen, under the modifiers held — the
/// twin of [`bare_characters`], asked with the event's own flags
/// instead of none.
///
/// A control character is not text: control and the letter A answer
/// with U+0001 here, and a box asking what was typed must not be told
/// that. The chord modifiers are dropped a level up, where the stroke
/// is made, so this only has to be honest about what it read.
unsafe fn typed_character(event: Id, flags: u64) -> Option<char> {
    unsafe {
        let selector = sel("charactersByApplyingModifiers:");
        if msg_bool_sel(class("NSEvent"), sel("instancesRespondToSelector:"), selector) == 0 {
            return None;
        }
        let typed = text_argument_to_string(msg_id_u64(event, selector, flags));
        // a control character is not text, and neither is the private
        // block F700-F8FF where AppKit files its function keys — the
        // pattern already names those, by name
        typed
            .chars()
            .next()
            .filter(|char| !char.is_control() && !('\u{F700}'..='\u{F8FF}').contains(char))
    }
}

/// The receiver-and-class pair `objc_msgSendSuper` walks up from —
/// how an added method still reaches the implementation it shadowed.
#[repr(C)]
pub(crate) struct ObjcSuper {
    receiver: Id,
    class: Id,
}

/// NSView's own `performKeyEquivalent:` — the walk into the subviews
/// this override would otherwise swallow (the page's ⌘C in a form
/// lives down there).
unsafe fn key_equivalent_super(this: Id, event: Id) -> i8 {
    let sup = ObjcSuper { receiver: this, class: class("NSView") };
    unsafe { msg_super_bool_id(&sup, sel("performKeyEquivalent:"), event) }
}

/// `performKeyEquivalent:` — the app's chords survive the island
/// holding the keyboard. A click on a hosted page hands the first
/// responder to the platform view and `keyDown:` stops arriving; but
/// AppKit walks the view tree for COMMAND chords before any of that,
/// and this view is visited before its children. Three rules:
/// - only while the view is NOT the responder: when it is, the chord
///   arrives by `keyDown:` as ever, and running the gate twice would
///   break a pending two-step chord.
/// - command chords only — everything else is the page's to type (a
///   form that is being typed in must receive the typing).
/// - not consumed → super, so the page keeps its own chords.
extern "C" fn bunny_perform_key_equivalent(this: Id, _sel: Sel, event: Id) -> i8 {
    unsafe {
        let window = msg_id(this, sel("window"));
        if window.is_null() {
            return key_equivalent_super(this, event);
        }
        let responder = msg_id(window, sel("firstResponder"));
        if std::ptr::eq(responder, this) {
            return key_equivalent_super(this, event);
        }
        let flags = msg_u64(event, sel("modifierFlags"));
        let held = modifiers_of(flags);
        if !held.command {
            return key_equivalent_super(this, event);
        }
        let code = msg_u16(event, sel("keyCode"));
        let stroke = KeyStroke {
            code,
            shift: held.shift,
            control: held.control,
            option: held.option,
            command: held.command,
            chars: text_argument_to_string(msg_id(event, sel("characters"))),
            typed: typed_character(event, flags),
            chars_bare: bare_characters(event),
        };
        if gate_consumed(&stroke) {
            return 1; // the keymap dispatched — the event dies here
        }
        key_equivalent_super(this, event)
    }
}

extern "C" fn bunny_key_down(this: Id, _sel: Sel, event: Id) {
    unsafe {
        let code = msg_u16(event, sel("keyCode"));
        let flags = msg_u64(event, sel("modifierFlags"));
        let held = modifiers_of(flags);
        let stroke = KeyStroke {
            code,
            shift: held.shift,
            control: held.control,
            option: held.option,
            command: held.command,
            chars: text_argument_to_string(msg_id(event, sel("characters"))),
            typed: typed_character(event, flags),
            chars_bare: bare_characters(event),
        };

        // live IME composition: the keys belong to the IME (Esc closes
        // candidates, arrows walk the composition) — the keymap doesn't steal
        let composing = ime_mirror().is_some_and(|ime| ime.marked.location != NS_NOT_FOUND);
        if !composing && gate_consumed(&stroke) {
            return; // the keymap dispatched — the event dies here
        }

        // focused field without cmd: the event enters the input system —
        // IME composition comes back via insertText/setMarkedText/doCommand
        if INTERPRET.with(|flag| flag.get()) && !stroke.command {
            let array = msg_id_arg(class("NSArray"), sel("arrayWithObject:"), event);
            msg_void_id(this, sel("interpretKeyEvents:"), array);
            return;
        }
        dispatch(AppEvent::Key {
            code,
            shift: stroke.shift,
            command: stroke.command,
            chars: stroke.chars,
        });
    }
}

extern "C" fn bunny_scroll_wheel(this: Id, _sel: Sel, event: Id) {
    unsafe {
        let (x, y) = event_layout_point(this, event);
        let mut dx = msg_f64(event, sel("scrollingDeltaX"));
        let mut dy = msg_f64(event, sel("scrollingDeltaY"));
        // trackpad delivers precise points; the legacy wheel delivers line
        // TICKS — converted to points here, once
        if msg_bool(event, sel("hasPreciseScrollingDeltas")) == 0 {
            dx *= 16.0;
            dy *= 16.0;
        }
        dispatch(AppEvent::Wheel { x, y, dx, dy });
    }
}

/// AppKit's word that a drag is about to move the window's frame. It
/// comes before the first resized frame, and that is the point: the
/// presenter arms its transaction here, so the whole drag runs under
/// one contract instead of catching up on the second step.
extern "C" fn bunny_window_will_start_live_resize(_this: Id, _sel: Sel, _note: Id) {
    crate::metal::arm_transaction(true);
}

extern "C" fn bunny_window_did_end_live_resize(_this: Id, _sel: Sel, note: Id) {
    crate::metal::arm_transaction(false);
    // the hand let go: one more frame NOW, so everything that held
    // back during the drag — a hosted engine's throttled size, the
    // live layers coming home — lands exact without waiting for the
    // next pointer wiggle
    dispatch_to(unsafe { msg_id(note, sel("object")) }, AppEvent::Redraw);
}

extern "C" fn bunny_window_did_resign_key(_this: Id, _sel: Sel, note: Id) {
    dispatch_to(unsafe { msg_id(note, sel("object")) }, AppEvent::ResignKey);
}

extern "C" fn bunny_window_did_become_key(_this: Id, _sel: Sel, note: Id) {
    dispatch_to(unsafe { msg_id(note, sel("object")) }, AppEvent::BecomeKey);
}

extern "C" fn bunny_slow(_this: Id, _sel: Sel, _timer: Id) {
    // the slow beat covers exactly one interval — the clocks advance by
    // the step they were promised, with no wall clock in the path
    let dt = SLOW.with(|slot| slot.get().1);
    dispatch_all(AppEvent::Frame { dt });
}

/// A data provider that OWNS a copy of the bytes. A layer's contents
/// are read by the render server AFTER the transaction closes: a
/// provider that only borrows the shell's buffer paints a small image
/// — the commit copies it inline — and silently paints NOTHING once
/// the image is big enough to be mapped instead of copied. An
/// island-sized segment was invisible while a toast-sized one showed,
/// with identical calls. CFData owns the copy, the provider retains
/// the CFData, the image retains the provider: the pixels stay
/// truthful for as long as the layer shows them.
unsafe fn owned_provider(bytes: *const u8, length: usize) -> *mut c_void {
    unsafe {
        let data = CFDataCreate(std::ptr::null(), bytes, length as isize);
        let provider = CGDataProviderCreateWithCFData(data);
        CFRelease(data);
        provider
    }
}

/// Removes CoreAnimation's implicit animations from a layer the shell
/// created. AppKit turns them off for the backing layers IT makes; a
/// layer handed to `setLayer:` — or added as a raw sublayer — keeps
/// the default quarter-second actions, and the first abrupt step of a
/// live resize then CROSSFADES the old content over the new: the
/// whole window reads double-exposed until the animation lands, which
/// no native window does. Per-mutation `setDisableActions:` cannot
/// cover this — the resize mutates the layer from APPKIT's own
/// transaction. The dictionary answers at the layer, for every
/// transaction; NSNull is CoreAnimation's own word for "no action".
pub(crate) unsafe fn kill_layer_actions(layer: Id) {
    unsafe {
        let null = msg_id(class("NSNull"), sel("null"));
        let actions = msg_id(class("NSMutableDictionary"), sel("dictionary"));
        for key in [
            "bounds",
            "position",
            "frame",
            "contents",
            "contentsScale",
            "hidden",
            "sublayers",
            "onOrderIn",
            "onOrderOut",
            "transform",
        ] {
            let key = CString::new(key).expect("action key");
            let key =
                msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), key.as_ptr());
            msg_void_id_id(actions, sel("setObject:forKey:"), null, key);
        }
        msg_void_id(layer, sel("setActions:"), actions);
    }
}

extern "C" fn bunny_window_did_resize(_this: Id, _sel: Sel, note: Id) {
    window_frame_changed(note, "resize");
}

extern "C" fn bunny_window_did_move(_this: Id, _sel: Sel, note: Id) {
    window_frame_changed(note, "move");
}

extern "C" fn bunny_window_did_change_backing(_this: Id, _sel: Sel, note: Id) {
    window_frame_changed(note, "backing");
}

// AppKit re-lays the titlebar container on every resize and on the
// way in and out of full screen, putting the buttons back where it
// wants them — so the app's placement is re-applied here. The
// notification names the window, so nothing global is needed.
//
// All three notifications answer the same way, but the tape tells
// them apart: a drag on the left or top edge moves the origin AND
// the size, so `move` and `resize` both arrive on every step.
fn window_frame_changed(note: Id, kind: &str) {
    let window = unsafe { msg_id(note, sel("object")) };
    if crate::trace::active() {
        unsafe {
            let view = msg_id(window, sel("contentView"));
            if !view.is_null() {
                let bounds = msg_rect(view, sel("bounds"));
                let live = msg_bool(view, sel("inLiveResize")) != 0;
                crate::trace::mark(
                    "R",
                    format_args!(
                        "{:.0}x{:.0} kind={kind} live={}",
                        bounds.size.width,
                        bounds.size.height,
                        u8::from(live)
                    ),
                );
            }
        }
    }
    place_traffic_lights(window);
    dispatch_to(window, AppEvent::Redraw);
}

thread_local! {
    /// Where the app asked for the native buttons, in points from the
    /// window's TOP-LEFT corner. `None` = wherever macOS puts them.
    /// Consumed by the next window BUILT — the main window's road,
    /// armed before it exists.
    static TRAFFIC_LIGHTS: Cell<Option<(f64, f64, Option<f64>)>> = const { Cell::new(None) };
    /// Every window whose buttons the app placed — the main window's
    /// scene bar, each scene-chrome DIALOG's header — with where it
    /// asked them. The frame tick walks them all, and a frame-changed
    /// notification looks its own window up here.
    static PLACED_LIGHTS: RefCell<Vec<(Id, (f64, f64, Option<f64>))>> =
        const { RefCell::new(Vec::new()) };
}

/// The app's answer to "where do the buttons sit", set once before the
/// window is built.
pub fn set_traffic_lights(at: Option<(f64, f64, Option<f64>)>) {
    TRAFFIC_LIGHTS.with(|slot| slot.set(at));
}

/// Registers one window's placement and applies it once — the window
/// is known here, unlike [`set_traffic_lights`]'s pre-build moment.
fn adopt_traffic_lights(window: Id, at: (f64, f64, Option<f64>)) {
    PLACED_LIGHTS.with(|slot| {
        let mut placed = slot.borrow_mut();
        match placed.iter_mut().find(|(held, _)| *held == window) {
            Some(entry) => entry.1 = at,
            None => placed.push((window, at)),
        }
    });
    place_traffic_lights(window);
}

/// Puts the buttons back if AppKit moved them — cheap enough to ask
/// every frame, and silent when there is nothing to do.
///
/// It has to be asked that often. `setTitle:` re-lays the titlebar
/// container, and so does a resize, a trip through full screen, and
/// every other thing that touches the window's chrome; there is no
/// notification for "the container laid out". Three frame reads and a
/// comparison per window is cheaper than being wrong.
pub fn keep_traffic_lights() {
    let windows: Vec<Id> =
        PLACED_LIGHTS.with(|slot| slot.borrow().iter().map(|(window, _)| *window).collect());
    for window in windows {
        place_traffic_lights(window);
    }
}

/// The three standard buttons, in the order they sit.
const WINDOW_BUTTONS: [u64; 3] = [0, 1, 2]; // close, miniaturize, zoom

thread_local! {
    /// The buttons as macOS made them — `(x, width, height)` each,
    /// read the FIRST time we see them and never again. Every
    /// placement measures from this, because reading a frame we
    /// already moved would compound the scale on every tick.
    static NATURAL_LIGHTS: RefCell<Vec<(f64, f64, f64)>> = const { RefCell::new(Vec::new()) };
}

/// The system's own frame for one button, remembered.
fn natural_light(index: usize, seen: CGRect) -> CGRect {
    NATURAL_LIGHTS.with(|slot| {
        let mut natural = slot.borrow_mut();
        if natural.len() <= index {
            natural.resize(index + 1, (0.0, 0.0, 0.0));
            natural[index] = (seen.origin.x, seen.size.width, seen.size.height);
        }
        let (x, width, height) = natural[index];
        CGRect {
            origin: CGPoint { x, y: seen.origin.y },
            size: CGSize { width, height },
        }
    })
}

/// One button's frame, moved. The container counts from the BOTTOM,
/// like every AppKit view, and the app counts from the top, like every
/// designer: the flip lives here, alone, where a test can reach it
/// without a window.
///
/// `shift` is how far this button sits from the first one — the
/// system's own spacing, which the placement never touches.
fn light_frame(
    container_height: f64,
    frame: CGRect,
    at: (f64, f64),
    shift: f64,
    size: Option<f64>,
) -> CGRect {
    let (x, y) = at;
    // the circle IS the button's box — a smaller frame draws a smaller
    // light — so one number scales the button AND the distance to the
    // one before it: the group shrinks whole, gaps and all
    let scale = match (size, frame.size.height) {
        (Some(size), height) if height > 0.0 => size / height,
        _ => 1.0,
    };
    let side = CGSize {
        width: frame.size.width * scale,
        height: frame.size.height * scale,
    };
    CGRect {
        origin: CGPoint {
            x: x + shift * scale,
            y: container_height - y - side.height,
        },
        size: side,
    }
}

/// Moves the traffic lights to where the app asked.
///
/// There is no AppKit call for this: with a transparent titlebar and
/// full-size content, the system leaves the buttons centred in the bar
/// it WOULD have drawn — a standard 28 points — and a scene that draws
/// a taller bar gets them sitting high. So the buttons are moved by
/// hand inside the titlebar container that holds them.
///
/// The container counts from the BOTTOM, like every AppKit view, and
/// the app counts from the top, like every designer: the flip happens
/// here, once. The horizontal spacing between the three is the
/// system's own — only the group moves.
pub fn place_traffic_lights(window: Id) {
    if window.is_null() {
        return;
    }
    // the window's own ask — a window nobody registered keeps the
    // system's placement (a Native-chrome dialog, a plain window)
    let Some((x, y, size)) = PLACED_LIGHTS.with(|slot| {
        slot.borrow().iter().find(|(held, _)| *held == window).map(|(_, at)| *at)
    }) else {
        return;
    };
    unsafe {
        for (index, kind) in WINDOW_BUTTONS.into_iter().enumerate() {
            let button = msg_id_u64(window, sel("standardWindowButton:"), kind);
            if button.is_null() {
                continue;
            }
            let frame = msg_rect(button, sel("frame"));
            let container = msg_id(button, sel("superview"));
            if container.is_null() {
                continue;
            }
            let bounds = msg_rect(container, sel("bounds"));
            // measured from the SYSTEM's own geometry, remembered the
            // first time we saw it: reading the current frame would
            // compound the scale a little more on every tick
            let natural = natural_light(index, frame);
            let shift = natural.origin.x - natural_light(0, frame).origin.x;
            let placed = light_frame(bounds.size.height, natural, (x, y), shift, size);
            // a no-op must cost nothing: setting the same frame every
            // frame would dirty the titlebar for no reason
            if placed.origin.x != frame.origin.x
                || placed.origin.y != frame.origin.y
                || placed.size.width != frame.size.width
            {
                msg_void_rect(button, sel("setFrame:"), placed);
            }
        }
    }
}

thread_local! {
    /// True while a dialog holds the app window-modal: the MAIN window
    /// answers `canBecomeKeyWindow` with NO, so a click on it cannot
    /// pull the keyboard out of the dialog — the JetBrains manner.
    static MODAL_BLOCKED: Cell<bool> = const { Cell::new(false) };
}

/// `BunnyWindow`'s answer to "may I become key" — YES, a titled
/// window's own answer, until a dialog holds the app modal. The
/// subclass exists for this one question.
extern "C" fn bunny_window_can_become_key(_this: Id, _sel: Sel) -> i8 {
    i8::from(!MODAL_BLOCKED.with(Cell::get))
}

/// Holds the app window-modal under a dialog: the parent's three
/// traffic lights go dark (disabled, not hidden — the JetBrains bar)
/// and the parent refuses key. No nested run loop is involved; the
/// scene's own modal floor already swallows the parent's input.
pub fn begin_window_modal(parent: &WindowHandle) {
    MODAL_BLOCKED.with(|blocked| blocked.set(true));
    set_standard_buttons_enabled(parent, false);
}

/// The reverse, on the dialog's way out. It runs BEFORE the parent is
/// made key again — the make-key asks `canBecomeKeyWindow`, and the
/// answer has to already be yes.
pub fn end_window_modal(parent: &WindowHandle) {
    MODAL_BLOCKED.with(|blocked| blocked.set(false));
    set_standard_buttons_enabled(parent, true);
}

/// Enables or disables the window's three standard buttons. The
/// placement loop above only ever moves frames, so a disabled button
/// stays disabled through every re-place.
fn set_standard_buttons_enabled(window: &WindowHandle, enabled: bool) {
    unsafe {
        for kind in WINDOW_BUTTONS {
            let button = msg_id_u64(window.window, sel("standardWindowButton:"), kind);
            if !button.is_null() {
                msg_void_bool(button, sel("setEnabled:"), i8::from(enabled));
            }
        }
    }
}

// MARK: - The dialog delegate

/// `windowShouldClose:` on a dialog — the red button. The answer is
/// always NO: the window never closes itself; the shell hears the
/// event, runs the overlay's dismissal, and the flipped binding is
/// what takes the window down (the same road the app's Escape takes).
extern "C" fn bunny_dialog_should_close(_this: Id, _sel: Sel, sender: Id) -> i8 {
    dispatch_to(sender, AppEvent::DialogClose { window: sender as usize });
    0
}

/// A dialog's frame moved — a user drag, a resize step, the zoom
/// button. The dialog's OWN lights go back (AppKit re-lays the
/// titlebar container on every resize, and a scene-chrome dialog
/// placed them in its header; the registry answers per window, so a
/// Native-chrome dialog is a no-op here), then one Redraw: the blit
/// pulls the window's rect into the runtime and the pass re-lays the
/// dialog's content inside it.
extern "C" fn bunny_dialog_frame_changed(_this: Id, _sel: Sel, note: Id) {
    let window = unsafe { msg_id(note, sel("object")) };
    place_traffic_lights(window);
    dispatch_to(window, AppEvent::Redraw);
}

/// The dialog's own word that a drag is starting — its presenter arms
/// its transaction here, for the reason the main window's does
/// ([`bunny_window_will_start_live_resize`]): the first resized frame
/// has to already run under the contract.
extern "C" fn bunny_dialog_will_start_live_resize(_this: Id, _sel: Sel, note: Id) {
    let window = unsafe { msg_id(note, sel("object")) };
    let view = unsafe { msg_id(window, sel("contentView")) };
    crate::metal::arm_transaction_view(view, true);
}

/// The hand let go of a dialog: the contract comes off and one exact
/// frame lands, like the main window's own end-of-drag.
extern "C" fn bunny_dialog_did_end_live_resize(this: Id, sel_: Sel, note: Id) {
    let window = unsafe { msg_id(note, sel("object")) };
    let view = unsafe { msg_id(window, sel("contentView")) };
    crate::metal::arm_transaction_view(view, false);
    bunny_dialog_frame_changed(this, sel_, note);
}

/// Key travels on a dialog like on the main window — cmd-tab away
/// pauses the decorations and closes the dialog's own popovers (the
/// dialog itself stands: it is a window).
extern "C" fn bunny_dialog_did_resign_key(_this: Id, _sel: Sel, note: Id) {
    dispatch_to(unsafe { msg_id(note, sel("object")) }, AppEvent::ResignKey);
}

extern "C" fn bunny_dialog_did_become_key(_this: Id, _sel: Sel, note: Id) {
    dispatch_to(unsafe { msg_id(note, sel("object")) }, AppEvent::BecomeKey);
}

extern "C" fn bunny_blink(_this: Id, _sel: Sel, _timer: Id) {
    dispatch_all(AppEvent::Blink);
}

extern "C" fn bunny_frame(_this: Id, _sel: Sel, link: Id) {
    // whatever re-laid the chrome since the last tick, the buttons go
    // back — the check is three reads and it usually does nothing
    keep_traffic_lights();
    let dt = unsafe {
        let last = msg_f64(link, sel("timestamp"));
        let next = msg_f64(link, sel("targetTimestamp"));
        // the first tick after a resume reports the whole pause as the
        // gap — a clamped step keeps springs continuous instead of
        // teleporting them
        (next - last).clamp(0.0, 1.0 / 30.0)
    };
    dispatch_all(AppEvent::Frame { dt });
}

extern "C" fn bunny_window_will_close(_this: Id, _sel: Sel, note: Id) {
    unsafe {
        let closing = msg_id(note, sel("object")) as usize;
        // Everything this window is the key to goes with it, and it goes
        // FIRST. A closed window's pointer is a freed object, and the
        // registries here are walked by the frame beat — which keeps
        // ticking now that closing a window no longer ends the app.
        forget_window(closing);
        let survivor = WINDOWS.with(|windows| {
            let mut windows = windows.borrow_mut();
            windows.retain(|(window, _, _)| *window != closing);
            windows.first().copied()
        });
        // the beat hangs off a window's view: when that window is the
        // one leaving, it moves to a survivor — otherwise the app that
        // opened the door and closed it would animate no more
        if let Some((window, view, delegate)) = survivor
            && BEAT_OWNER.with(Cell::get) == closing
        {
            LINK.with(|slot| {
                let link = slot.replace(std::ptr::null_mut());
                if !link.is_null() {
                    msg_void(link, sel("invalidate"));
                }
            });
            start_beat(window as Id, view, delegate);
        }
        let last = survivor.is_none();
        // the app goes down with its LAST window, not with any window:
        // a second Trinity closing is a window closing, and the one
        // still open keeps the process.
        if !last {
            dispatch_to(msg_id(note, sel("object")), AppEvent::WindowClosed);
            return;
        }
        // the link retains its target (the delegate) — break the tie
        // before the app goes down
        LINK.with(|slot| {
            let link = slot.replace(std::ptr::null_mut());
            if !link.is_null() {
                msg_void(link, sel("invalidate"));
            }
        });
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_id(app, sel("terminate:"), std::ptr::null_mut());
    }
}

thread_local! {
    /// The window's display link — born paused; the shell resumes it
    /// only while animations run. Zero-ivar classes: per-window state
    /// lives beside the run loop (the backing-store pattern).
    static LINK: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
    /// The slow beat: `(timer, interval)`. Alive only while loop clocks
    /// are the sole animation — one wake per step instead of a display
    /// rate of empty ticks.
    static SLOW: Cell<(Id, f64)> = const { Cell::new((std::ptr::null_mut(), 0.0)) };
    /// The window delegate — the target the slow timer fires at.
    static DELEGATE: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
}

/// How fast the shell drives frames — the ffi twin of the runtime's
/// pace, chosen after every present.
#[derive(Clone, Copy, PartialEq)]
pub enum DriverPace {
    /// The display link runs — springs and flights are moving.
    Full,
    /// Only loop clocks live: a repeating timer beats once per step.
    Slow(f64),
    /// Nothing moves.
    Off,
}

/// Points the frame driver at the pace the moment deserves. Without a
/// display link (an older macOS) `Full` is a no-op — animations then
/// complete instantly; the slow beat works everywhere (it is a plain
/// timer).
pub fn set_frame_driver(pace: DriverPace) {
    let full = pace == DriverPace::Full;
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
                let delegate = DELEGATE.with(|slot| slot.get());
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
                        std::ptr::null_mut(),
                        1,
                    )
                };
                slot.set((fresh, wanted));
            }
            DriverPace::Full | DriverPace::Off => {
                if !timer.is_null() {
                    unsafe { msg_void(timer, sel("invalidate")) };
                    slot.set((std::ptr::null_mut(), 0.0));
                }
            }
        }
    });
}


static REGISTER_CLASSES: Once = Once::new();

unsafe fn register_classes() {
    REGISTER_CLASSES.call_once(|| unsafe {
        let types = CString::new("v@:@").expect("type encoding");

        let view = objc_allocateClassPair(
            class("NSView"),
            CString::new("BunnyView").expect("name").as_ptr(),
            0,
        );
        class_addMethod(
            view,
            sel("mouseDown:"),
            bunny_mouse_down as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(view, sel("mouseUp:"), bunny_mouse_up as *const c_void, types.as_ptr());
        class_addMethod(
            view,
            sel("rightMouseDown:"),
            bunny_right_mouse_down as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("mouseMoved:"),
            bunny_mouse_moved as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("mouseDragged:"),
            bunny_mouse_moved as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("mouseEntered:"),
            bunny_mouse_moved as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("mouseExited:"),
            bunny_mouse_exited as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("scrollWheel:"),
            bunny_scroll_wheel as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("keyDown:"),
            bunny_key_down as *const c_void,
            types.as_ptr(),
        );
        // the chord road that survives a hosted page holding the
        // first responder — see bunny_perform_key_equivalent
        let key_equivalent_types = CString::new("c@:@").expect("type encoding");
        class_addMethod(
            view,
            sel("performKeyEquivalent:"),
            bunny_perform_key_equivalent as *const c_void,
            key_equivalent_types.as_ptr(),
        );
        let draw_types = CString::new("v@:{CGRect={CGPoint=dd}{CGSize=dd}}").expect("type encoding");
        class_addMethod(
            view,
            sel("drawRect:"),
            bunny_draw_rect as *const c_void,
            draw_types.as_ptr(),
        );
        let bool_getter = CString::new("c@:").expect("type encoding");
        class_addMethod(
            view,
            sel("acceptsFirstResponder"),
            bunny_accepts_first_responder as *const c_void,
            bool_getter.as_ptr(),
        );

        // NSTextInputClient — the input system's door (real IME)
        let encode = |types: &str| CString::new(types).expect("type encoding");
        let insert_types = encode("v@:@{_NSRange=QQ}");
        class_addMethod(
            view,
            sel("insertText:replacementRange:"),
            bunny_insert_text as *const c_void,
            insert_types.as_ptr(),
        );
        let mark_types = encode("v@:@{_NSRange=QQ}{_NSRange=QQ}");
        class_addMethod(
            view,
            sel("setMarkedText:selectedRange:replacementRange:"),
            bunny_set_marked_text as *const c_void,
            mark_types.as_ptr(),
        );
        let void_types = encode("v@:");
        class_addMethod(
            view,
            sel("unmarkText"),
            bunny_unmark_text as *const c_void,
            void_types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("hasMarkedText"),
            bunny_has_marked_text as *const c_void,
            bool_getter.as_ptr(),
        );
        let range_types = encode("{_NSRange=QQ}@:");
        class_addMethod(
            view,
            sel("markedRange"),
            bunny_marked_range as *const c_void,
            range_types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("selectedRange"),
            bunny_selected_range as *const c_void,
            range_types.as_ptr(),
        );
        let substring_types = encode("@@:{_NSRange=QQ}^{_NSRange=QQ}");
        class_addMethod(
            view,
            sel("attributedSubstringForProposedRange:actualRange:"),
            bunny_attributed_substring as *const c_void,
            substring_types.as_ptr(),
        );
        let attrs_types = encode("@@:");
        class_addMethod(
            view,
            sel("validAttributesForMarkedText"),
            bunny_valid_attributes as *const c_void,
            attrs_types.as_ptr(),
        );
        let rect_types = encode("{CGRect={CGPoint=dd}{CGSize=dd}}@:{_NSRange=QQ}^{_NSRange=QQ}");
        class_addMethod(
            view,
            sel("firstRectForCharacterRange:actualRange:"),
            bunny_first_rect as *const c_void,
            rect_types.as_ptr(),
        );
        let index_types = encode("Q@:{CGPoint=dd}");
        class_addMethod(
            view,
            sel("characterIndexForPoint:"),
            bunny_character_index as *const c_void,
            index_types.as_ptr(),
        );
        let command_types = encode("v@::");
        class_addMethod(
            view,
            sel("doCommandBySelector:"),
            bunny_do_command as *const c_void,
            command_types.as_ptr(),
        );
        let protocol = objc_getProtocol(
            CString::new("NSTextInputClient").expect("name").as_ptr(),
        );
        if !protocol.is_null() {
            class_addProtocol(view, protocol);
        }

        objc_registerClassPair(view);

        let delegate = objc_allocateClassPair(
            class("NSObject"),
            CString::new("BunnyDelegate").expect("name").as_ptr(),
            0,
        );
        class_addMethod(
            delegate,
            sel("windowDidResize:"),
            bunny_window_did_resize as *const c_void,
            types.as_ptr(),
        );
        // a monitor drag or scale flip must repaint NOW — without this,
        // the next pointer wiggle would be the first frame at the new
        // scale (blurry until then on a CAMetalLayer)
        class_addMethod(
            delegate,
            sel("windowDidChangeBackingProperties:"),
            bunny_window_did_change_backing as *const c_void,
            types.as_ptr(),
        );
        // a moved window re-clamps its popovers against the screen —
        // the child panels FOLLOW by AppKit's own hand; this repaint
        // only re-runs the overlay geometry
        class_addMethod(
            delegate,
            sel("windowDidMove:"),
            bunny_window_did_move as *const c_void,
            types.as_ptr(),
        );
        // the presenter's anti-tear contract is armed on AppKit's own
        // word, one beat BEFORE the frame that would show the seam
        class_addMethod(
            delegate,
            sel("windowWillStartLiveResize:"),
            bunny_window_will_start_live_resize as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("windowDidEndLiveResize:"),
            bunny_window_did_end_live_resize as *const c_void,
            types.as_ptr(),
        );
        // switching away closes the open popovers, the platform's way
        class_addMethod(
            delegate,
            sel("windowDidResignKey:"),
            bunny_window_did_resign_key as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("windowDidBecomeKey:"),
            bunny_window_did_become_key as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("bunnyBlink:"),
            bunny_blink as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("bunnySlow:"),
            bunny_slow as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("bunnyFrame:"),
            bunny_frame as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("windowWillClose:"),
            bunny_window_will_close as *const c_void,
            types.as_ptr(),
        );
        objc_registerClassPair(delegate);

        // the MAIN window's subclass — one question answered, nothing
        // else touched: under a dialog it refuses to become key
        let bare_bool = CString::new("c@:").expect("type encoding");
        let window = objc_allocateClassPair(
            class("NSWindow"),
            CString::new("BunnyWindow").expect("name").as_ptr(),
            0,
        );
        class_addMethod(
            window,
            sel("canBecomeKeyWindow"),
            bunny_window_can_become_key as *const c_void,
            bare_bool.as_ptr(),
        );
        class_addMethod(
            window,
            sel("canBecomeMainWindow"),
            bunny_window_can_become_key as *const c_void,
            bare_bool.as_ptr(),
        );
        objc_registerClassPair(window);

        // the DIALOG's own delegate — the shared one would terminate
        // the app on close and re-place the parent's traffic lights on
        // every dialog resize
        let sender_bool = CString::new("c@:@").expect("type encoding");
        let dialog = objc_allocateClassPair(
            class("NSObject"),
            CString::new("BunnyDialogDelegate").expect("name").as_ptr(),
            0,
        );
        class_addMethod(
            dialog,
            sel("windowShouldClose:"),
            bunny_dialog_should_close as *const c_void,
            sender_bool.as_ptr(),
        );
        class_addMethod(
            dialog,
            sel("windowDidResize:"),
            bunny_dialog_frame_changed as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            dialog,
            sel("windowDidMove:"),
            bunny_dialog_frame_changed as *const c_void,
            types.as_ptr(),
        );
        // the drag's two ends: the presenter's contract goes on before
        // the first resized frame and comes off with the exact final one
        class_addMethod(
            dialog,
            sel("windowWillStartLiveResize:"),
            bunny_dialog_will_start_live_resize as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            dialog,
            sel("windowDidEndLiveResize:"),
            bunny_dialog_did_end_live_resize as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            dialog,
            sel("windowDidResignKey:"),
            bunny_dialog_did_resign_key as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            dialog,
            sel("windowDidBecomeKey:"),
            bunny_dialog_did_become_key as *const c_void,
            types.as_ptr(),
        );
        objc_registerClassPair(dialog);
    });
}

// MARK: - Window

/// Raw window handles — `Copy`, same thread, wrapped by the safe
/// operations below.
#[derive(Clone, Copy)]
pub struct WindowHandle {
    window: Id,
    view: Id,
}

impl WindowHandle {
    /// The content view — the key a grafted presenter is filed under.
    pub(crate) fn view(&self) -> Id {
        self.view
    }

    /// Logical size of the content area (the layout viewport).
    pub fn content_size(&self) -> (f64, f64) {
        unsafe {
            let bounds = msg_rect(self.view, sel("bounds"));
            (bounds.size.width, bounds.size.height)
        }
    }

    /// The screen's scale factor (retina = 2).
    pub fn scale(&self) -> usize {
        unsafe { msg_f64(self.window, sel("backingScaleFactor")).round().max(1.0) as usize }
    }

    /// The layer keeps its distance to the BOTTOM flexible, which in
    /// AppKit's world means it hangs from the TOP edge.
    const LAYER_MIN_Y_MARGIN: u32 = 1 << 3;
    /// And its distance to the RIGHT flexible, so it hangs from the
    /// LEFT edge — together, the top-left corner the layout counts
    /// from.
    const LAYER_MAX_X_MARGIN: u32 = 1 << 2;

    /// Presents one live box on its own sublayer: the window behind it
    /// never redraws. `x`/`y` are the box's LAYOUT origin (top-left,
    /// points) and `view_height` is the height of the layout that
    /// placed it — the bottom-left flip uses the SAME world the rect
    /// was computed in, never whatever the view measures mid-resize.
    /// The pixels are straight RGBA and premultiply here (a layer's
    /// contents composite premultiplied). The layer is keyed by the
    /// box's identity and reused across steps; two backings alternate
    /// so the picture on screen is never the one being written.
    pub fn live_layer_blit(
        &self,
        key: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        view_height: f64,
        scale: usize,
        px_width: usize,
        px_height: usize,
        rgba: &[u8],
    ) {
        unsafe {
            let root = msg_id(self.view, sel("layer"));
            if root.is_null() {
                // a view without a backing layer presents by damage —
                // the live path is the GPU presenter's
                return;
            }
            LIVE_LAYERS.with(|layers| {
                let mut layers = layers.borrow_mut();
                let entry = layers.entry(key.to_string()).or_insert_with(|| {
                    let layer = msg_id(class("CALayer"), sel("layer"));
                    // a raw sublayer keeps CA's default quarter-second
                    // actions — the trail and the order-in fade both
                    // die here, for every transaction that touches it
                    unsafe { kill_layer_actions(layer) };
                    // the sublayer composites over the drawable — where
                    // the scene left the box's hole
                    msg_void_id(root, sel("addSublayer:"), layer);
                    // The two worlds disagree: layout counts from the
                    // TOP-left, a layer from the bottom-left. So a box
                    // that never moved in the layout still needs a new
                    // layer origin every time the window's HEIGHT
                    // changes — and until we place it, the old origin is
                    // wrong by exactly how far the drag has gone.
                    //
                    // The mask makes CoreAnimation keep the distance to
                    // the TOP and to the LEFT instead, in the
                    // superlayer's own resize. A box that did not move
                    // is then already right before we say anything, and
                    // one that did is corrected by the next placement,
                    // as before.
                    msg_void_u32(
                        layer,
                        sel("setAutoresizingMask:"),
                        Self::LAYER_MIN_Y_MARGIN | Self::LAYER_MAX_X_MARGIN,
                    );
                    LiveLayer { layer, buffers: [Vec::new(), Vec::new()], flip: false }
                });
                // premultiply into the spare backing
                entry.flip = !entry.flip;
                let backing = &mut entry.buffers[entry.flip as usize];
                backing.clear();
                backing.extend_from_slice(rgba);
                for pixel in backing.chunks_exact_mut(4) {
                    let alpha = pixel[3] as u32;
                    if alpha < 255 {
                        for channel in 0..3 {
                            pixel[channel] = (pixel[channel] as u32 * alpha / 255) as u8;
                        }
                    }
                }
                let provider = owned_provider(backing.as_ptr(), backing.len());
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
                // AppKit's ground is bottom-left; layout's is top-left
                without_actions(|| {
                    msg_void_rect(
                        entry.layer,
                        sel("setFrame:"),
                        CGRect {
                            origin: CGPoint { x, y: view_height - y - h },
                            size: CGSize { width: w, height: h },
                        },
                    );
                    msg_void_f64(entry.layer, sel("setContentsScale:"), scale as f64);
                    msg_void_id(entry.layer, sel("setContents:"), image);
                });
                CGImageRelease(image);
                CGColorSpaceRelease(space);
                CGDataProviderRelease(provider);
            });
        }
    }

    /// Re-places one live box's layer without touching its pixels —
    /// an ordinary frame carries a moved bar's mark along for the cost
    /// of a frame set. `view_height` is the placing layout's height,
    /// like [`WindowHandle::live_layer_blit`] takes. A box with no
    /// layer yet is a no-op (its first blit will place it).
    pub fn live_layer_place(&self, key: &str, x: f64, y: f64, w: f64, h: f64, view_height: f64) {
        LIVE_LAYERS.with(|layers| {
            let layers = layers.borrow();
            let Some(entry) = layers.get(key) else {
                return;
            };
            unsafe {
                without_actions(|| {
                    msg_void_rect(
                        entry.layer,
                        sel("setFrame:"),
                        CGRect {
                            origin: CGPoint { x, y: view_height - y - h },
                            size: CGSize { width: w, height: h },
                        },
                    );
                });
            }
        });
    }

    /// Is the window being dragged by an edge right now? While it is,
    /// the live boxes come HOME into the drawable: a layer of their own
    /// lands in a different beat than the window frame, and a drag is
    /// exactly what makes that beat visible.
    pub fn in_live_resize(&self) -> bool {
        unsafe { msg_bool(self.view, sel("inLiveResize")) != 0 }
    }

    /// Removes the layers of live boxes that left the scene.
    pub fn live_layer_sweep(&self, alive: &[String]) {
        LIVE_LAYERS.with(|layers| {
            let mut layers = layers.borrow_mut();
            layers.retain(|key, entry| {
                if alive.iter().any(|path| path == key) {
                    return true;
                }
                unsafe { msg_void(entry.layer, sel("removeFromSuperlayer")) };
                false
            });
        });
    }

    /// Mounts and places one native host box — the platform view that
    /// composites ABOVE the scene, in the hole the layout keeps
    /// (`docs/webview.md`). Two views per host: a clipping container
    /// (ours) and the tenant's view inside it, so a host in a scroll
    /// region shows the window's worth and nothing escapes.
    ///
    /// `frame` is the box's LAYOUT rect (top-left, points) and
    /// `window` the rect the clip lets through, box-LOCAL — both
    /// straight from the placement. `view_height` is the placing
    /// layout's height: the bottom-left flip uses the world the rects
    /// were computed in, like [`WindowHandle::live_layer_blit`]. An
    /// empty window hides the view instead of unmounting it — a page
    /// keeps its state while it is scrolled off.
    ///
    /// `make` runs once, when the key first appears, and returns the
    /// tenant's view with ONE retain that [`WindowHandle::host_sweep`]
    /// releases. `update` runs when `stamp` changed — how a navigation
    /// lands without a re-mount.
    pub fn host_place(
        &self,
        key: &str,
        stamp: &str,
        frame: (f64, f64, f64, f64),
        window: (f64, f64, f64, f64),
        view_height: f64,
        make: impl FnOnce() -> Id,
        update: impl FnOnce(Id, &str),
    ) {
        HOST_VIEWS.with(|hosts| {
            let mut hosts = hosts.borrow_mut();
            let slot = hosts.entry(key.to_string()).or_insert_with(|| unsafe {
                let container = msg_id(class("NSView"), sel("alloc"));
                let container = msg_init_rect(
                    container,
                    sel("initWithFrame:"),
                    CGRect {
                        origin: CGPoint { x: 0.0, y: 0.0 },
                        size: CGSize { width: 0.0, height: 0.0 },
                    },
                );
                // the container is the CLIP: whatever the tenant
                // draws stays inside the window the layout granted
                msg_void_bool(container, sel("setWantsLayer:"), 1);
                let layer = msg_id(container, sel("layer"));
                if !layer.is_null() {
                    msg_void_bool(layer, sel("setMasksToBounds:"), 1);
                }
                // width and height SIZABLE, margins fixed: during a
                // live resize APPKIT ITSELF drives both views at the
                // start of the resize cycle — the earliest beat there
                // is, the one a vanilla autoresizing app gets — and
                // the hosted engine starts its relayout before our own
                // frame has even begun. Our placement lands right
                // after as the exact truth (for the edge-anchored
                // fill pane the spring IS exact; elsewhere it is a
                // sixty-a-second approximation we correct).
                const SIZABLE: i64 = 2 | 16;
                msg_void_i64(container, sel("setAutoresizingMask:"), SIZABLE);
                let child = make();
                msg_void_i64(child, sel("setAutoresizingMask:"), SIZABLE);
                msg_void_id(container, sel("addSubview:"), child);
                msg_void_id(self.view, sel("addSubview:"), container);
                HostSlot { container, child, stamp: stamp.to_string() }
            });
            let (x, y, w, h) = frame;
            let (vx, vy, vw, vh) = window;
            unsafe {
                if vw <= 0.0 || vh <= 0.0 {
                    msg_void_bool(slot.container, sel("setHidden:"), 1);
                } else {
                    msg_void_bool(slot.container, sel("setHidden:"), 0);
                    // the container lands on the visible cut, flipped
                    // into AppKit's bottom-left world
                    msg_void_rect(
                        slot.container,
                        sel("setFrame:"),
                        CGRect {
                            origin: CGPoint { x: x + vx, y: view_height - (y + vy) - vh },
                            size: CGSize { width: vw, height: vh },
                        },
                    );
                    // the tenant keeps the WHOLE box, container-local:
                    // the cut shows through, the content never rewraps
                    msg_void_rect(
                        slot.child,
                        sel("setFrame:"),
                        CGRect {
                            origin: CGPoint { x: -vx, y: vh + vy - h },
                            size: CGSize { width: w, height: h },
                        },
                    );
                }
                if slot.stamp != stamp {
                    slot.stamp = stamp.to_string();
                    update(slot.child, stamp);
                }
            }
        });
    }

    /// Removes the hosts that left the scene — the subtree went, the
    /// platform view goes with it. Releases the two holds
    /// [`WindowHandle::host_place`] took: the container's alloc and
    /// the tenant's `make`.
    pub fn host_sweep(&self, alive: &[String]) {
        HOST_VIEWS.with(|hosts| {
            let mut hosts = hosts.borrow_mut();
            hosts.retain(|key, slot| {
                if alive.iter().any(|path| path == key) {
                    return true;
                }
                unsafe {
                    msg_void(slot.container, sel("removeFromSuperview"));
                    msg_void(slot.child, sel("release"));
                    msg_void(slot.container, sel("release"));
                }
                false
            });
        });
    }

    /// Presents damaged rects only: syncs the ffi-owned backing store
    /// (partial copy — a hover copies one row of bytes) and marks each
    /// rect dirty; AppKit calls `drawRect:` with the union and the view
    /// paints from the backing through a NO-COPY CGImage. `damage` is in
    /// PHYSICAL pixels, top-left origin.
    pub fn blit_partial(
        &self,
        width: usize,
        height: usize,
        rgba: &[u8],
        damage: &[(i64, i64, i64, i64)],
    ) {
        if damage.is_empty() {
            return;
        }
        BACKING.with(|stores| {
            let mut stores = stores.borrow_mut();
            let backing = stores
                .entry(self.view as usize)
                .or_insert_with(|| Backing { width: 0, height: 0, bytes: Vec::new() });
            if backing.width != width || backing.height != height {
                // fresh surface (first frame or resize): take everything
                backing.width = width;
                backing.height = height;
                backing.bytes.clear();
                backing.bytes.extend_from_slice(rgba);
                return;
            }
            for &(x0, y0, x1, y1) in damage {
                let x0 = x0.clamp(0, width as i64) as usize;
                let x1 = x1.clamp(0, width as i64) as usize;
                let y0 = y0.clamp(0, height as i64) as usize;
                let y1 = y1.clamp(0, height as i64) as usize;
                for y in y0..y1 {
                    let from = (y * width + x0) * 4;
                    let to = (y * width + x1) * 4;
                    backing.bytes[from..to].copy_from_slice(&rgba[from..to]);
                }
            }
        });
        unsafe {
            let scale = self.scale() as f64;
            let bounds = msg_rect(self.view, sel("bounds"));
            for &(x0, y0, x1, y1) in damage {
                let x = x0 as f64 / scale;
                let w = (x1 - x0) as f64 / scale;
                let top = y0 as f64 / scale;
                let h = (y1 - y0) as f64 / scale;
                // AppKit flip: the view origin is bottom-left
                let rect = CGRect {
                    origin: CGPoint { x, y: bounds.size.height - top - h },
                    size: CGSize { width: w, height: h },
                };
                msg_void_rect(self.view, sel("setNeedsDisplayInRect:"), rect);
            }
            // present NOW: without this the dirty rects wait for a
            // future run-loop turn and every event shows the PREVIOUS
            // frame (a theme toggle looked dead on the first click and
            // flashed one frame late on the second). displayIfNeeded
            // draws synchronously and is a no-op when nothing is dirty.
            msg_void(self.view, sel("displayIfNeeded"));
        }
    }

    /// A rect in LAYOUT coordinates (top-left) converted to AppKit's
    /// SCREEN — where the IME candidate window lands.
    pub fn layout_rect_to_screen(&self, x: f64, y: f64, width: f64, height: f64) -> CGRect {
        unsafe {
            let bounds = msg_rect(self.view, sel("bounds"));
            // AppKit flip + view == contentView (window origin)
            let window_rect = CGRect {
                origin: CGPoint { x, y: bounds.size.height - y - height },
                size: CGSize { width, height },
            };
            msg_rect_rect(self.window, sel("convertRectToScreen:"), window_rect)
        }
    }

    /// A SCREEN rect (AppKit, y-up) in this window's LAYOUT
    /// coordinates — the exact inverse of [`Self::layout_rect_to_screen`].
    pub fn screen_rect_to_layout(&self, rect: CGRect) -> (f64, f64, f64, f64) {
        unsafe {
            let in_window = msg_rect_rect(self.window, sel("convertRectFromScreen:"), rect);
            let bounds = msg_rect(self.view, sel("bounds"));
            (
                in_window.origin.x,
                bounds.size.height - in_window.origin.y - in_window.size.height,
                in_window.size.width,
                in_window.size.height,
            )
        }
    }

    /// Places a DIALOG so its CONTENT box lands on the given screen
    /// rect — the frame grows the title bar around it. ε-guarded:
    /// layout hands the same rect back every frame, and re-setting it
    /// would fight the very drag it was pulled from.
    pub fn set_content_frame_screen(&self, content: CGRect) {
        unsafe {
            let frame =
                msg_rect_rect(self.window, sel("frameRectForContentRect:"), content);
            let current = msg_rect(self.window, sel("frame"));
            let same = (frame.origin.x - current.origin.x).abs() < 0.5
                && (frame.origin.y - current.origin.y).abs() < 0.5
                && (frame.size.width - current.size.width).abs() < 0.5
                && (frame.size.height - current.size.height).abs() < 0.5;
            if !same {
                msg_void_rect_bool(self.window, sel("setFrame:display:"), frame, 1);
            }
        }
    }

    /// The dialog's CONTENT box in `main`'s layout coordinates — what
    /// the shell reports to the runtime every frame, so layout follows
    /// the window wherever the user takes it.
    pub fn content_rect_in_layout(&self, main: &WindowHandle) -> (f64, f64, f64, f64) {
        unsafe {
            let frame = msg_rect(self.window, sel("frame"));
            let content =
                msg_rect_rect(self.window, sel("contentRectForFrameRect:"), frame);
            main.screen_rect_to_layout(content)
        }
    }

    /// Is the window on screen (ordered in)?
    pub fn is_visible(&self) -> bool {
        unsafe { msg_bool(self.window, sel("isVisible")) != 0 }
    }

    /// Re-adopts a pooled dialog on reopen — the child tie was cut
    /// when it closed.
    pub fn attach_to(&self, parent: &WindowHandle) {
        unsafe {
            msg_void_id_i64(parent.window, sel("addChildWindow:ordered:"), self.window, 1);
        }
    }

    /// Fronts the window and hands the keyboard to its event view.
    pub fn make_key_with_view(&self) {
        unsafe {
            msg_void_id(self.window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
            msg_void_id(self.window, sel("makeFirstResponder:"), self.view);
        }
    }

    /// The `NSWindow` as an address — what a delegate callback names
    /// so the shell can find this handle in a pool.
    pub fn raw_window(&self) -> usize {
        self.window as usize
    }

    /// The pointer's outfit over the scene. Direct `set` — no cursor
    /// rects for now (AppKit may restore it at resize edges; a cosmetic
    /// glitch we accept).
    pub fn set_cursor(&self, cursor: Cursor) {
        // the shell speaks only when its answer CHANGES. Re-asserting
        // the same cursor on every pointer event fights whatever a
        // platform view set for its own content — a webview's hand
        // over a link would flicker against our arrow forever.
        if LAST_CURSOR.with(|last| last.replace(Some(cursor))) == Some(cursor) {
            return;
        }
        let name = match cursor {
            Cursor::Arrow => "arrowCursor",
            Cursor::Text => "IBeamCursor",
            Cursor::Pointing => "pointingHandCursor",
            Cursor::ResizeLeftRight => "resizeLeftRightCursor",
            Cursor::ResizeUpDown => "resizeUpDownCursor",
        };
        unsafe {
            msg_void(msg_id(class("NSCursor"), sel(name)), sel("set"));
        }
    }
}

/// What the pointer wears: the hand over an interactive target, a
/// resizer over a split's grip — the one that matches the way THAT
/// seam travels — and the arrow elsewhere.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    /// The I-beam, for text a press puts a caret in.
    Text,
    Pointing,
    ResizeLeftRight,
    ResizeUpDown,
}

/// `kCGImageAlphaPremultipliedLast` — bytes R,G,B,A, alpha last.
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;
/// `kCGInterpolationNone` — the backing is already pixel-exact.
const INTERPOLATION_NONE: i32 = 1;

/// The ffi-owned presentation backing: `drawRect:` always reads from
/// here, so the pointer handed to CoreGraphics never dangles — the
/// shell's surface can move freely. Synced by [`WindowHandle::blit_partial`]
/// with damage-only copies.
struct Backing {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}

thread_local! {
    /// One backing per VIEW — the main window and every popover panel
    /// present through the same `drawRect:`, each from its own store.
    static BACKING: RefCell<HashMap<usize, Backing>> = RefCell::new(HashMap::new());
    /// One sublayer per LIVE box, keyed by identity — the box's own
    /// presentation surface while its loop runs.
    static LIVE_LAYERS: RefCell<HashMap<String, LiveLayer>> = RefCell::new(HashMap::new());
    /// One platform view per HOST box, keyed by identity — the native
    /// content that composites above the scene.
    static HOST_VIEWS: RefCell<HashMap<String, HostSlot>> = RefCell::new(HashMap::new());
}

/// One mounted native host: the clipping container (ours), the
/// tenant's view inside it, and the stamp the tenant last applied — a
/// changed spec re-instructs the view, it never re-mounts it.
struct HostSlot {
    container: Id,
    child: Id,
    stamp: String,
}

/// The tenant's view mounted under `key`, if any — where a drained
/// command is spent.
pub(crate) fn host_child(key: &str) -> Option<Id> {
    HOST_VIEWS.with(|hosts| hosts.borrow().get(key).map(|slot| slot.child))
}

/// The key a tenant's view is mounted under — how a report that
/// arrives holding the VIEW (a navigation, a posted message) finds
/// its box's identity.
pub(crate) fn host_key_of_child(child: Id) -> Option<String> {
    HOST_VIEWS.with(|hosts| {
        hosts
            .borrow()
            .iter()
            .find(|(_, slot)| std::ptr::eq(slot.child, child))
            .map(|(key, _)| key.clone())
    })
}

/// One segment surface: the scene's commands that painted ABOVE a
/// host, composited over the platform view — the sandwich that keeps
/// paint order the truth, island or no island. The straight RGBA
/// stays here because the HIT TEST reads it: a painted pixel claims
/// the pointer, a clear one lets it fall through to the page below.
struct SegmentSlot {
    view: Id,
    rgba: Vec<u8>,
    width: usize,
    height: usize,
    scale: usize,
    /// Two premultiplied backings, alternating — the picture on
    /// screen is never the buffer being written (the live layers'
    /// discipline).
    buffers: [Vec<u8>; 2],
    flip: bool,
}

thread_local! {
    static SEGMENTS: RefCell<HashMap<String, SegmentSlot>> = RefCell::new(HashMap::new());
    /// The cursor the shell last chose — the gate that keeps it from
    /// re-asserting an unchanged answer every pointer event.
    static LAST_CURSOR: Cell<Option<Cursor>> = const { Cell::new(None) };
}

static REGISTER_SEGMENT: Once = Once::new();

/// The segment view's whole hit policy, in one answer: alpha claims.
/// `point` arrives in the SUPERVIEW's coordinates (bottom-left); the
/// buffer's rows count from the top.
extern "C" fn bunny_segment_hit(this: Id, _sel: Sel, point: CGPoint) -> Id {
    SEGMENTS.with(|segments| {
        // AppKit asks hitTest RE-ENTRANTLY — a setFrame: invalidates
        // tracking and asks right then. While the table is being
        // written the answer is "not mine": a pointer question in the
        // middle of a blit falls through to the page for one event
        let Ok(segments) = segments.try_borrow() else {
            return std::ptr::null_mut();
        };
        let Some(slot) = segments.values().find(|slot| std::ptr::eq(slot.view, this)) else {
            return std::ptr::null_mut();
        };
        let frame = unsafe { msg_rect(this, sel("frame")) };
        let x = point.x - frame.origin.x;
        let up = point.y - frame.origin.y;
        if x < 0.0 || up < 0.0 || x >= frame.size.width || up >= frame.size.height {
            return std::ptr::null_mut();
        }
        let column = ((x * slot.scale as f64) as usize).min(slot.width.saturating_sub(1));
        let row = (((frame.size.height - up) * slot.scale as f64) as usize)
            .min(slot.height.saturating_sub(1));
        let index = (row * slot.width + column) * 4 + 3;
        match slot.rgba.get(index) {
            Some(alpha) if *alpha > 8 => this,
            _ => std::ptr::null_mut(),
        }
    })
}

/// Every pointer verb the segment view claims goes to the content
/// view unchanged — the event still carries its window coordinates,
/// so the shell's own handlers resolve it like any other.
extern "C" fn bunny_segment_forward(this: Id, cmd: Sel, event: Id) {
    unsafe {
        let superview = msg_id(this, sel("superview"));
        if !superview.is_null() {
            msg_void_id(superview, cmd, event);
        }
    }
}

unsafe fn register_segment_class() {
    REGISTER_SEGMENT.call_once(|| unsafe {
        crate::trace::mark("X", format_args!("what=segment-class"));
        let name = CString::new("BunnySegmentView").expect("class name");
        let segment = objc_allocateClassPair(class("NSView"), name.as_ptr(), 0);
        let hit_types = CString::new("@@:{CGPoint=dd}").expect("type encoding");
        class_addMethod(
            segment,
            sel("hitTest:"),
            bunny_segment_hit as *const c_void,
            hit_types.as_ptr(),
        );
        let forward_types = CString::new("v@:@").expect("type encoding");
        for verb in
            ["mouseDown:", "mouseUp:", "rightMouseDown:", "mouseDragged:", "scrollWheel:"]
        {
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

impl WindowHandle {
    /// Presents one segment — the commands that painted above `host`,
    /// rasterized by the caller into straight RGBA sized to the
    /// CONTENT's box, never the window (a toast's segment carries a
    /// toast). The surface mounts DIRECTLY ABOVE the host's
    /// container, so content between two hosts lands between their
    /// pages. `frame` is the content box in LAYOUT coordinates
    /// (top-left, points) and `view_height` the placing layout's
    /// height — the live layers' flip, and their hang-from-top-left
    /// masks: a stale bitmap never stretches, the next place moves it
    /// whole.
    #[expect(clippy::too_many_arguments, reason = "a presenter takes what it takes")]
    pub fn segment_blit(
        &self,
        key: &str,
        host_key: &str,
        rgba: &[u8],
        frame: (f64, f64, f64, f64),
        view_height: f64,
        scale: usize,
        px_width: usize,
        px_height: usize,
    ) {
        unsafe { register_segment_class() };
        // NOTHING Objective-C runs while the table is borrowed:
        // AppKit asks hitTest RE-ENTRANTLY from addSubview: and
        // setFrame:, and the hit answer reads this same table — a
        // borrow held across the send is the abort the first QA found
        let known = SEGMENTS.with(|segments| segments.borrow().get(key).map(|slot| slot.view));
        let view = match known {
            Some(view) => view,
            None => unsafe {
                let view = msg_id(class("BunnySegmentView"), sel("alloc"));
                let view = msg_init_rect(
                    view,
                    sel("initWithFrame:"),
                    CGRect {
                        origin: CGPoint { x: 0.0, y: 0.0 },
                        size: CGSize { width: 0.0, height: 0.0 },
                    },
                );
                msg_void_bool(view, sel("setWantsLayer:"), 1);
                msg_void_i64(
                    view,
                    sel("setAutoresizingMask:"),
                    (Self::LAYER_MIN_Y_MARGIN | Self::LAYER_MAX_X_MARGIN) as i64,
                );
                match host_child_container(host_key) {
                    // NSWindowAbove = 1: directly over the page it covers
                    Some(container) => msg_void_id_i64_id(
                        self.view,
                        sel("addSubview:positioned:relativeTo:"),
                        view,
                        1,
                        container,
                    ),
                    None => msg_void_id(self.view, sel("addSubview:"), view),
                }
                SEGMENTS.with(|segments| {
                    segments.borrow_mut().insert(
                        key.to_string(),
                        SegmentSlot {
                            view,
                            rgba: Vec::new(),
                            width: 0,
                            height: 0,
                            scale,
                            buffers: [Vec::new(), Vec::new()],
                            flip: false,
                        },
                    );
                });
                view
            },
        };
        // the pixels land in the table first; the layer reads them
        // AFTER the borrow is gone. The backing stays put between the
        // two: one thread, and its only writer is this function.
        let (pointer, length) = SEGMENTS.with(|segments| {
            let mut segments = segments.borrow_mut();
            let slot = segments.get_mut(key).expect("the slot was just made");
            slot.rgba.clear();
            slot.rgba.extend_from_slice(rgba);
            slot.width = px_width;
            slot.height = px_height;
            slot.scale = scale;
            slot.flip = !slot.flip;
            // the bytes arrive ALREADY premultiplied: a raster onto a
            // transparent ground leaves rgb = colour x coverage (the
            // rasterizer's own words). Multiplying here again squared
            // the alpha — an opaque toast never showed it, and a
            // nine-percent border faded to nothing
            let backing = &mut slot.buffers[slot.flip as usize];
            backing.clear();
            backing.extend_from_slice(rgba);
            (backing.as_ptr(), backing.len())
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
            let layer = msg_id(view, sel("layer"));
            without_actions(|| {
                msg_void_rect(
                    view,
                    sel("setFrame:"),
                    CGRect {
                        origin: CGPoint { x, y: view_height - y - h },
                        size: CGSize { width: w, height: h },
                    },
                );
                if !layer.is_null() {
                    msg_void_f64(layer, sel("setContentsScale:"), scale as f64);
                    msg_void_id(layer, sel("setContents:"), image);
                }
            });
            CGImageRelease(image);
            CGColorSpaceRelease(space);
            CGDataProviderRelease(provider);
        }
    }

    /// Re-places one segment without touching its pixels — the same
    /// commands at a new flip height (the window grew, the content
    /// did not). A segment with no surface yet is a no-op.
    pub fn segment_place(&self, key: &str, frame: (f64, f64, f64, f64), view_height: f64) {
        // the view leaves the borrow before the send — hitTest reads
        // the table the moment a frame moves
        let Some(view) =
            SEGMENTS.with(|segments| segments.borrow().get(key).map(|slot| slot.view))
        else {
            return;
        };
        let (x, y, w, h) = frame;
        unsafe {
            without_actions(|| {
                msg_void_rect(
                    view,
                    sel("setFrame:"),
                    CGRect {
                        origin: CGPoint { x, y: view_height - y - h },
                        size: CGSize { width: w, height: h },
                    },
                );
            });
        }
    }

    /// Removes the segments that left the scene — nothing painted
    /// above the host this frame, so nothing covers it. The dead
    /// leave the table FIRST and their views after: removal asks
    /// hitTest too.
    pub fn segment_sweep(&self, alive: &[String]) {
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
}

/// The host's clipping container, for stacking a segment right above
/// its page.
fn host_child_container(key: &str) -> Option<Id> {
    HOST_VIEWS.with(|hosts| hosts.borrow().get(key).map(|slot| slot.container))
}

/// The shell steps back from the cursor: whatever owns it now (a
/// webview's own hover, mostly) keeps it, and the NEXT thing the
/// shell wants — even the arrow — asserts again.
pub(crate) fn yield_cursor() {
    LAST_CURSOR.with(|last| last.set(None));
}

/// One live box's sublayer and its two alternating backings — the
/// picture on screen is never the buffer being written.
struct LiveLayer {
    layer: Id,
    buffers: [Vec<u8>; 2],
    flip: bool,
}

/// Mutates layers with the implicit animations OFF. Core Animation
/// otherwise wraps every `setFrame:`/`setContents:` in a quarter-second
/// implicit animation — a live-resized bar re-places its mark every
/// frame and the layer would chase the position with that lag while the
/// drawable lands instantly (the mark visibly dancing behind the
/// window). The transaction NESTS: the changes still land with the run
/// loop's outer transaction, which is the same one a live resize
/// presents the drawable in — layer and window touch down together.
unsafe fn without_actions(body: impl FnOnce()) {
    unsafe {
        let transaction = class("CATransaction");
        msg_void(transaction, sel("begin"));
        msg_void_bool(transaction, sel("setDisableActions:"), 1);
        body();
        msg_void(transaction, sel("commit"));
    }
}

/// `drawRect:` — paints the dirty union from the backing through a
/// NO-COPY CGImage. The context arrives clipped to the dirty rect; the
/// CTM flip converts our top-down rows to AppKit's bottom-up world.
extern "C" fn bunny_draw_rect(this: Id, _sel: Sel, _dirty: CGRect) {
    BACKING.with(|stores| {
        let stores = stores.borrow();
        let Some(backing) = stores.get(&(this as usize)) else {
            return;
        };
        if backing.bytes.is_empty() {
            return;
        }
        unsafe {
            let graphics = msg_id(class("NSGraphicsContext"), sel("currentContext"));
            if graphics.is_null() {
                return;
            }
            let context = msg_id(graphics, sel("CGContext"));
            if context.is_null() {
                return;
            }
            let provider = CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                backing.bytes.as_ptr(),
                backing.bytes.len(),
                std::ptr::null(),
            );
            let space = CGColorSpaceCreateDeviceRGB();
            let image = CGImageCreate(
                backing.width,
                backing.height,
                8,
                32,
                backing.width * 4,
                space,
                ALPHA_PREMULTIPLIED_LAST,
                provider,
                std::ptr::null(),
                false,
                0,
            );
            let bounds = msg_rect(this, sel("bounds"));
            CGContextSaveGState(context);
            CGContextSetInterpolationQuality(context, INTERPOLATION_NONE);
            // no CTM flip: Quartz draws a CGImage upright in the y-up
            // context of a non-flipped view — flipping here would turn
            // the world on its head
            CGContextDrawImage(
                context,
                CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: bounds.size },
                image,
            );
            CGContextRestoreGState(context);
            CGImageRelease(image);
            CGColorSpaceRelease(space);
            CGDataProviderRelease(provider);
        }
    });
}

/// Creates the app + the window with the event view, ready for blit.
/// `scene_chrome` hides the system title bar: full-size content, a
/// transparent titlebar and no title text — the native traffic lights
/// stay at the corner and the SCENE draws the bar.
pub fn create_window(
    title: &str,
    width: f64,
    height: f64,
    scene_chrome: bool,
    manners: Manners,
) -> WindowHandle {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        register_classes();

        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        // Regular: a terminal app gets a window, the Dock and focus
        let _ = msg_bool_i64(app, sel("setActivationPolicy:"), 0);

        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width, height },
        };
        // titled | closable (+ miniaturizable, + resizable, + full-size
        // content when the scene owns the chrome). A mask without
        // Miniaturizable draws the yellow light dead, which is exactly
        // what a door that may not be put away should look like.
        let mut style: u64 = 1 | 2;
        if manners.minimizable {
            style |= 4;
        }
        if manners.resizable {
            style |= 8;
        }
        if scene_chrome {
            style |= 1 << 15;
        }
        // the subclass answers ONE question (key, under a dialog) and
        // inherits everything else
        let window = msg_id(class("BunnyWindow"), sel("alloc"));
        let window = msg_init_window(
            window,
            sel("initWithContentRect:styleMask:backing:defer:"),
            rect,
            style,
            2, // buffered
            0,
        );
        if scene_chrome {
            msg_void_bool(window, sel("setTitlebarAppearsTransparent:"), 1);
            // NSWindowTitleHidden = 1 — the title still names the
            // window in Mission Control and the Dock
            msg_void_i64(window, sel("setTitleVisibility:"), 1);
        }

        let title = CString::new(title).expect("title without NUL");
        let ns_title = msg_id_cstr(
            class("NSString"),
            sel("stringWithUTF8String:"),
            title.as_ptr(),
        );
        msg_void_id(window, sel("setTitle:"), ns_title);
        msg_void(window, sel("center"));

        // the event view becomes the content view, with its own layer
        let view = msg_id(class("BunnyView"), sel("alloc"));
        let view = msg_init_rect(view, sel("initWithFrame:"), rect);
        // the GPU graft goes BEFORE setWantsLayer: — a layer set first
        // makes the view layer-HOSTING (drawRect: never runs) and the
        // window presents by Metal; otherwise today's layer-backed CPU
        // path, byte for byte
        let _ = crate::metal::try_install(
            view,
            msg_f64(window, sel("backingScaleFactor")),
            width,
            height,
        );
        msg_void_bool(view, sel("setWantsLayer:"), 1);
        msg_void_id(window, sel("setContentView:"), view);

        // moved/entered/exited arrive via the tracking area — no first
        // responder dance, and InVisibleRect tracks the resize by itself
        // (the rect passed in is ignored). 0x223 = MouseEnteredAndExited |
        // MouseMoved | ActiveInKeyWindow | InVisibleRect.
        let area = msg_id(class("NSTrackingArea"), sel("alloc"));
        let area = msg_init_tracking(
            area,
            sel("initWithRect:options:owner:userInfo:"),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            },
            0x223,
            view,
            std::ptr::null_mut(),
        );
        msg_void_id(view, sel("addTrackingArea:"), area);

        // delegate: resize repaints, and the last one out quits
        let delegate = msg_id(msg_id(class("BunnyDelegate"), sel("alloc")), sel("init"));
        msg_void_id(window, sel("setDelegate:"), delegate);
        // the slow frame beat fires at the delegate too
        DELEGATE.with(|slot| slot.set(delegate));

        let first = WINDOWS.with(|windows| {
            let mut windows = windows.borrow_mut();
            windows.push((window as usize, view, delegate));
            windows.len() == 1
        });
        if first {
            start_beat(window, view, delegate);
        }

        msg_void_id(window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
        // the keyboard is born pointing at the event view
        msg_void_id(window, sel("makeFirstResponder:"), view);
        msg_void_bool(app, sel("activateIgnoringOtherApps:"), 1);
        // LAST: the traffic lights are placed after everything that
        // touches the chrome. `setTitle:` alone puts them back where
        // the system wants them, and it runs above.
        if let Some(at) = TRAFFIC_LIGHTS.with(Cell::get) {
            adopt_traffic_lights(window, at);
        }
        objc_autoreleasePoolPop(pool);

        WindowHandle { window, view }
    }
}

/// `NSWindowStyleMaskNonactivatingPanel` — borderless, and NEVER key:
/// the keyboard and the IME stay with the parent window.
const PANEL_STYLE: u64 = 1 << 7;

/// Creates a borderless, transparent child panel over `parent` — the
/// popover's own surface. It follows the parent on move by AppKit's
/// child-window contract; it carries NO delegate (closing it must
/// never terminate), no timer and no display link — the parent drives
/// every frame. The panel's chrome (background, border, shadow) is the
/// scene's own paint on a clear backdrop.
pub fn create_panel(parent: &WindowHandle, width: f64, height: f64) -> WindowHandle {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        register_classes();

        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width, height },
        };
        let panel = msg_id(class("NSPanel"), sel("alloc"));
        let panel = msg_init_window(
            panel,
            sel("initWithContentRect:styleMask:backing:defer:"),
            rect,
            PANEL_STYLE,
            2, // buffered
            0,
        );
        // the panel is a transparent sheet of glass: our scene paints
        // the popover's card, shadow included — the system shadow
        // would double it (and borderless panels default it ON)
        msg_void_bool(panel, sel("setOpaque:"), 0);
        msg_void_id(panel, sel("setBackgroundColor:"), msg_id(class("NSColor"), sel("clearColor")));
        msg_void_bool(panel, sel("setHasShadow:"), 0);
        // the store manages the lifetime — never AppKit's release
        msg_void_bool(panel, sel("setReleasedWhenClosed:"), 0);

        let view = msg_id(class("BunnyView"), sel("alloc"));
        let view = msg_init_rect(view, sel("initWithFrame:"), rect);
        // CPU present only: no metal graft on panels (v1)
        msg_void_bool(view, sel("setWantsLayer:"), 1);
        msg_void_id(panel, sel("setContentView:"), view);

        // hover and exit work inside the panel like anywhere else — but
        // ActiveInKeyWindow does NOT, because a popover panel never takes
        // key (that is the whole point of a panel: the window behind it
        // keeps the keyboard). With the window's own 0x223 the tracking
        // area is simply inactive and the panel receives no moves at all:
        // the rows of a menu never light, a submenu never opens, and a
        // tooltip inside a popover never appears.
        //
        // 0x243 = MouseEnteredAndExited | MouseMoved | **ActiveInActiveApp**
        // | InVisibleRect. ActiveInActiveApp rather than ActiveAlways: a
        // popover belonging to an app that is not frontmost should not be
        // tracking the pointer, and the shell already dismisses on an app
        // switch.
        let area = msg_id(class("NSTrackingArea"), sel("alloc"));
        let area = msg_init_tracking(
            area,
            sel("initWithRect:options:owner:userInfo:"),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            },
            0x243,
            view,
            std::ptr::null_mut(),
        );
        msg_void_id(view, sel("addTrackingArea:"), area);

        // the child contract: the panel rides every parent move
        msg_void_id_i64(parent.window, sel("addChildWindow:ordered:"), panel, 1);

        objc_autoreleasePoolPop(pool);
        WindowHandle { window: panel, view }
    }
}

/// Creates a DIALOG window over `parent` — the real titled window an
/// overlay with `OverlaySurface::Window` asked for. Sized by its
/// CONTENT rect; the caller places it with
/// [`WindowHandle::set_content_frame_screen`].
///
/// The dialog manners, each pinned to its AppKit lever:
/// - titled | closable | resizable and NO miniaturizable — the yellow
///   button renders disabled, the JetBrains dialog's own bar;
/// - `NSWindowCollectionBehaviorFullScreenAuxiliary` — strips the
///   implicit fullscreen-primary every resizable window gets, so the
///   green button ZOOMS in place instead of going fullscreen, and lets
///   the window join a fullscreen parent's space instead of switching
///   away from it;
/// - a child of `parent` (`addChildWindow:ordered:` above) — floats
///   over it and rides its moves;
/// - its own delegate (`BunnyDialogDelegate`) — closing flips the
///   app's binding, never terminates, and a dialog resize never
///   re-places the PARENT's traffic lights;
/// - no timer, no display link, no Metal graft — the parent drives
///   every frame and the content arrives by CPU blit, the child
///   panel's own discipline.
///
/// `lights` = scene chrome: no system bar (full-size content, a
/// transparent titlebar, the title hidden but still naming the window
/// to the OS), and the native traffic lights placed at that point
/// from the window's top-left — the app's own header carries them,
/// the main window's `Chrome::SceneAt` road for a dialog.
pub fn create_dialog(
    parent: &WindowHandle,
    title: &str,
    width: f64,
    height: f64,
    min_width: f64,
    min_height: f64,
    lights: Option<(f64, f64)>,
) -> WindowHandle {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        register_classes();

        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width, height },
        };
        // titled | closable | resizable — miniaturizable stays OFF, so
        // the yellow light is born disabled (+ full-size content when
        // the dialog's own header owns the top edge)
        let style: u64 = if lights.is_some() { 1 | 2 | 8 | (1 << 15) } else { 1 | 2 | 8 };
        let window = msg_id(class("NSWindow"), sel("alloc"));
        let window = msg_init_window(
            window,
            sel("initWithContentRect:styleMask:backing:defer:"),
            rect,
            style,
            2, // buffered
            0,
        );
        if lights.is_some() {
            msg_void_bool(window, sel("setTitlebarAppearsTransparent:"), 1);
            // NSWindowTitleHidden = 1 — the title still names the
            // window in Mission Control
            msg_void_i64(window, sel("setTitleVisibility:"), 1);
        }
        // NSWindowCollectionBehaviorFullScreenAuxiliary (1 << 8)
        msg_void_u64(window, sel("setCollectionBehavior:"), 1 << 8);
        msg_void_size(
            window,
            sel("setContentMinSize:"),
            CGSize { width: min_width, height: min_height },
        );
        let title = CString::new(title).expect("title without NUL");
        let ns_title = msg_id_cstr(
            class("NSString"),
            sel("stringWithUTF8String:"),
            title.as_ptr(),
        );
        msg_void_id(window, sel("setTitle:"), ns_title);
        // the store manages the lifetime — never AppKit's release
        msg_void_bool(window, sel("setReleasedWhenClosed:"), 0);

        let view = msg_id(class("BunnyView"), sel("alloc"));
        let view = msg_init_rect(view, sel("initWithFrame:"), rect);
        // the GPU graft, BEFORE setWantsLayer: — the main window's own
        // discipline. A dialog is a real window that the reader
        // resizes, and a CPU raster of its whole content on every step
        // of the drag is what made one lag its own corner. Refused or
        // failed, the view stays on the CPU road (blit_partial).
        let _ = crate::metal::try_install_view(
            view,
            msg_f64(parent.window, sel("backingScaleFactor")),
            width,
            height,
        );
        msg_void_bool(view, sel("setWantsLayer:"), 1);
        msg_void_id(window, sel("setContentView:"), view);

        // hover must work while EITHER of our windows is key — the
        // panel's reasoning, spelled out above `PANEL_STYLE`. 0x243 =
        // MouseEnteredAndExited | MouseMoved | ActiveInActiveApp |
        // InVisibleRect.
        let area = msg_id(class("NSTrackingArea"), sel("alloc"));
        let area = msg_init_tracking(
            area,
            sel("initWithRect:options:owner:userInfo:"),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            },
            0x243,
            view,
            std::ptr::null_mut(),
        );
        msg_void_id(view, sel("addTrackingArea:"), area);

        let delegate = msg_id(msg_id(class("BunnyDialogDelegate"), sel("alloc")), sel("init"));
        msg_void_id(window, sel("setDelegate:"), delegate);

        // the child contract: floats above the parent, rides its
        // moves, and joins the space the parent is on — a fullscreen
        // space included
        msg_void_id_i64(parent.window, sel("addChildWindow:ordered:"), window, 1);

        // LAST, the main window's own discipline: the lights are
        // placed after everything that touches the chrome
        if let Some((x, y)) = lights {
            adopt_traffic_lights(window, (x, y, None));
        }

        objc_autoreleasePoolPop(pool);
        WindowHandle { window, view }
    }
}

impl WindowHandle {
    /// Places the panel at a SCREEN rect (AppKit coordinates, y-up) —
    /// the parent's `layout_rect_to_screen` produces it.
    pub fn set_frame_screen(&self, rect: CGRect) {
        unsafe { msg_void_rect_bool(self.window, sel("setFrame:display:"), rect, 1) };
    }

    /// Registers where this panel's view sits in SCENE coordinates —
    /// its pointer events translate back through it.
    pub fn set_scene_origin(&self, x: f64, y: f64) {
        PANEL_ORIGINS.with(|origins| {
            origins.borrow_mut().insert(self.view as usize, (x, y));
        });
    }

    /// Detaches and hides a child panel, and forgets its stores. The
    /// panel object stays reusable-dead (released when the process
    /// goes) — panels are few and pooled by path.
    pub fn close_panel(&self, parent: &WindowHandle) {
        unsafe {
            // the ACTUAL holder, not the caller's guess: a dropdown
            // born inside a dialog is the DIALOG's child, and the
            // dialog itself is the main window's
            let holder = msg_id(self.window, sel("parentWindow"));
            let holder = if holder.is_null() { parent.window } else { holder };
            msg_void_id(holder, sel("removeChildWindow:"), self.window);
            msg_void_id(self.window, sel("orderOut:"), std::ptr::null_mut());
        }
        PANEL_ORIGINS.with(|origins| {
            origins.borrow_mut().remove(&(self.view as usize));
        });
        BACKING.with(|stores| {
            stores.borrow_mut().remove(&(self.view as usize));
        });
    }

    /// The screen's visible frame in this window's LAYOUT coordinates
    /// (top-left origin; left/above the window comes out negative).
    /// `None` when the window is off every screen.
    pub fn screen_bounds_in_layout(&self) -> Option<(f64, f64, f64, f64)> {
        unsafe {
            let screen = msg_id(self.window, sel("screen"));
            if screen.is_null() {
                return None;
            }
            let visible = msg_rect(screen, sel("visibleFrame"));
            let in_window = msg_rect_rect(self.window, sel("convertRectFromScreen:"), visible);
            let bounds = msg_rect(self.view, sel("bounds"));
            // AppKit flip: y-up window coords → top-left layout coords
            let top = bounds.size.height - in_window.origin.y - in_window.size.height;
            Some((in_window.origin.x, top, in_window.size.width, in_window.size.height))
        }
    }
}

/// Enters the AppKit run loop — returns when the app terminates.
pub fn run() {
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void(app, sel("run"));
    }
}

// MARK: - Clipboard

/// Writes text to the system's general pasteboard.
pub fn clipboard_write(text: &str) {
    unsafe {
        let pasteboard = msg_id(class("NSPasteboard"), sel("generalPasteboard"));
        let _ = msg_i64(pasteboard, sel("clearContents"));
        let Ok(text) = CString::new(text) else { return };
        let string = msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr());
        let _ = msg_bool_id_id(
            pasteboard,
            sel("setString:forType:"),
            string,
            NSPasteboardTypeString,
        );
    }
}

/// Reads the general pasteboard's text (`None` = empty or non-text).
pub fn clipboard_read() -> Option<String> {
    unsafe {
        let pasteboard = msg_id(class("NSPasteboard"), sel("generalPasteboard"));
        let string = msg_id_arg(pasteboard, sel("stringForType:"), NSPasteboardTypeString);
        if string.is_null() {
            return None;
        }
        let utf8 = msg_id(string, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event raised from INSIDE a handler waits its turn.
    ///
    /// This aborted a running app, and the shape it aborted in is the app's
    /// own: a worker's `Wake` paints a window, the paint completes a sign-in,
    /// and the sign-in opens the window that replaces the one being painted.
    /// The handler is borrowed for as long as it runs and this is an
    /// `extern "C"` frame, so the borrow panic could not unwind — it took the
    /// process with it.
    ///
    /// Queued, and never dropped: dropping would trade the abort for a window
    /// that silently never draws.
    #[test]
    fn an_event_raised_inside_a_handler_waits_its_turn() {
        use std::cell::RefCell;
        use std::rc::Rc;

        fn name(event: &AppEvent) -> &'static str {
            match event {
                AppEvent::Redraw => "redraw",
                AppEvent::Wake => "wake",
                AppEvent::Blink => "blink",
                _ => "other",
            }
        }

        let seen: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let armed = Rc::new(std::cell::Cell::new(true));
        set_handler(Box::new({
            let (seen, armed) = (Rc::clone(&seen), Rc::clone(&armed));
            move |event| {
                seen.borrow_mut().push(name(&event));
                // the re-entrant raise, once — the sign-in's own move
                if matches!(event, AppEvent::Wake) && armed.replace(false) {
                    dispatch_all(AppEvent::Redraw);
                    // …and it has NOT run yet: the queue holds it until this
                    // handler returns, so the order stays the order
                    seen.borrow_mut().push("still inside");
                }
            }
        }));

        dispatch_all(AppEvent::Wake);
        assert_eq!(
            *seen.borrow(),
            vec!["wake", "still inside", "redraw"],
            "the raised event arrives AFTER the handler that raised it, and it arrives",
        );
    }

    /// The selector pain 34's fix stands on. `charactersIgnoringModifiers`
    /// only ignores command and option — it APPLIES shift, so a chord on
    /// shifted punctuation arrives as the shifted character and never
    /// matches its spec. `charactersByApplyingModifiers:` asks for the
    /// characters under a chosen set of modifiers, so ZERO gives the
    /// key's own base character IN THE USER'S LAYOUT — no table of US
    /// pairs, which would be wrong on the Brazilian keyboard this is
    /// written on.
    ///
    /// It arrived in macOS 10.15. This test is the proof that it is
    /// here, and the alarm if a build target ever predates it.
    #[test]
    fn the_lights_flip_from_the_top_into_the_container() {
        // the numbers of the pain: a bar of forty points wants its
        // lights at (16, 14) from the window's top-left corner
        let button = CGRect {
            origin: CGPoint { x: 7.0, y: 6.0 },
            size: CGSize { width: 14.0, height: 14.0 },
        };
        let placed = light_frame(40.0, button, (16.0, 14.0), 0.0, None);
        assert_eq!(placed.origin.x, 16.0, "sixteen from the leading edge");
        // AppKit counts up from the bottom: 40 - 14 - 14
        assert_eq!(placed.origin.y, 12.0, "and fourteen from the top, flipped");
        assert_eq!(
            (placed.size.width, placed.size.height),
            (button.size.width, button.size.height),
            "the button never changes size"
        );
    }

    #[test]
    fn the_group_keeps_the_systems_own_spacing() {
        let button = CGRect {
            origin: CGPoint { x: 7.0, y: 6.0 },
            size: CGSize { width: 14.0, height: 14.0 },
        };
        // the second and third buttons carry their distance from the
        // first: only the GROUP moves
        let first = light_frame(40.0, button, (16.0, 14.0), 0.0, None);
        let second = light_frame(40.0, button, (16.0, 14.0), 20.0, None);
        assert_eq!(second.origin.x - first.origin.x, 20.0);
        assert_eq!(second.origin.y, first.origin.y, "and they stay on one line");
    }

    #[test]
    fn a_smaller_light_takes_its_gap_with_it() {
        // the circle IS the button's box, so one number is the whole
        // look: the button shrinks and so does the distance to the one
        // before it — the group stays a group
        let button = CGRect {
            origin: CGPoint { x: 7.0, y: 6.0 },
            size: CGSize { width: 14.0, height: 14.0 },
        };
        let first = light_frame(40.0, button, (16.0, 14.0), 0.0, Some(7.0));
        let second = light_frame(40.0, button, (16.0, 14.0), 20.0, Some(7.0));
        assert_eq!(first.size.width, 7.0, "half the diameter");
        assert_eq!(first.size.height, 7.0);
        assert_eq!(second.origin.x - first.origin.x, 10.0, "and half the gap");
        // the top edge is still where the app asked: what changes is
        // the size, never the anchor
        assert_eq!(first.origin.y, 40.0 - 14.0 - 7.0);
    }

    #[test]
    fn appkit_still_hands_out_the_window_buttons() {
        // the one thing that could rot under us: the selector the
        // placement rides on
        unsafe {
            let responds = msg_bool_sel(
                class("NSWindow"),
                sel("instancesRespondToSelector:"),
                sel("standardWindowButton:"),
            );
            assert_ne!(responds, 0, "NSWindow has no standardWindowButton:");
        }
    }

    #[test]
    fn appkit_still_speaks_the_dialogs_selectors() {
        // every selector the dialog window rides on, pinned — the same
        // alarm the window buttons keep above
        unsafe {
            for name in [
                "setCollectionBehavior:",
                "setContentMinSize:",
                "frameRectForContentRect:",
                "contentRectForFrameRect:",
                "convertRectFromScreen:",
                "parentWindow",
                "isVisible",
            ] {
                let responds = msg_bool_sel(
                    class("NSWindow"),
                    sel("instancesRespondToSelector:"),
                    sel(name),
                );
                assert_ne!(responds, 0, "NSWindow has no {name}");
            }
            // the parent's lights go dark through the button itself
            let responds = msg_bool_sel(
                class("NSButton"),
                sel("instancesRespondToSelector:"),
                sel("setEnabled:"),
            );
            assert_ne!(responds, 0, "NSButton has no setEnabled:");
        }
    }

    #[test]
    fn appkit_can_report_a_key_without_its_modifiers() {
        unsafe {
            let responds = msg_bool_sel(
                class("NSEvent"),
                sel("instancesRespondToSelector:"),
                sel("charactersByApplyingModifiers:"),
            );
            assert_ne!(responds, 0, "NSEvent has no charactersByApplyingModifiers:");
        }
    }
}
