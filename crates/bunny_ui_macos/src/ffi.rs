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
    fn msg_void_rect_bool(obj: Id, sel: Sel, rect: CGRect, flag: i8);
    #[link_name = "objc_msgSend"]
    fn msg_void_i64(obj: Id, sel: Sel, a: i64);
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
pub enum AppEvent {
    MouseDown { x: f64, y: f64, clicks: u8 },
    /// The right button (or a two-finger tap): the context-menu press.
    RightMouseDown { x: f64, y: f64 },
    MouseUp { x: f64, y: f64 },
    MouseMoved { x: f64, y: f64 },
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
    /// A task woke from somewhere else — a worker thread finished a
    /// step. The frame the shell already knows how to draw drains the
    /// queue on its way.
    Wake,
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
}

/// Registers who receives the events (the shell's loop).
pub fn set_handler(handler: Box<dyn FnMut(AppEvent)>) {
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

/// Delivers an event to the handler — used by the callbacks and by the
/// first frame.
pub fn dispatch(event: AppEvent) {
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

/// The run loop source a background thread knocks on. It lives in a
/// static (not a thread-local) because the signal comes from ANY
/// thread — `CFRunLoopSourceSignal` and `CFRunLoopWakeUp` are the
/// thread-safe half of CoreFoundation.
static WAKE_SOURCE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

extern "C" fn perform_wake(_info: *mut c_void) {
    dispatch(AppEvent::Wake);
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
    dispatch(AppEvent::MouseDown { x, y, clicks });
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
    dispatch(AppEvent::MouseMoved { x, y });
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
    /// `charactersIgnoringModifiers` — which ignores command and option
    /// and APPLIES shift, so it is the typing char, not the key's name.
    pub chars_ignoring: String,
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
        let (x, y) = (in_window.x, bounds.size.height - in_window.y);
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

extern "C" fn bunny_key_down(this: Id, _sel: Sel, event: Id) {
    unsafe {
        let code = msg_u16(event, sel("keyCode"));
        let flags = msg_u64(event, sel("modifierFlags"));
        let stroke = KeyStroke {
            code,
            shift: flags & (1 << 17) != 0,
            control: flags & (1 << 18) != 0,
            option: flags & (1 << 19) != 0,
            command: flags & (1 << 20) != 0,
            chars: text_argument_to_string(msg_id(event, sel("characters"))),
            chars_ignoring: text_argument_to_string(
                msg_id(event, sel("charactersIgnoringModifiers")),
            ),
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

extern "C" fn bunny_window_did_resign_key(_this: Id, _sel: Sel, _note: Id) {
    dispatch(AppEvent::ResignKey);
}

extern "C" fn bunny_window_did_resize(_this: Id, _sel: Sel, note: Id) {
    // AppKit re-lays the titlebar container on every resize and on the
    // way in and out of full screen, putting the buttons back where it
    // wants them — so the app's placement is re-applied here. The
    // notification names the window, so nothing global is needed.
    let window = unsafe { msg_id(note, sel("object")) };
    place_traffic_lights(window);
    dispatch(AppEvent::Redraw);
}

thread_local! {
    /// Where the app asked for the native buttons, in points from the
    /// window's TOP-LEFT corner. `None` = wherever macOS puts them.
    static TRAFFIC_LIGHTS: Cell<Option<(f64, f64, Option<f64>)>> = const { Cell::new(None) };
    /// The window that carries them, so the frame tick can put them
    /// back without being handed anything.
    static LIGHTS_WINDOW: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
}

/// The app's answer to "where do the buttons sit", set once before the
/// window is built.
pub fn set_traffic_lights(at: Option<(f64, f64, Option<f64>)>) {
    TRAFFIC_LIGHTS.with(|slot| slot.set(at));
}

/// Puts the buttons back if AppKit moved them — cheap enough to ask
/// every frame, and silent when there is nothing to do.
///
/// It has to be asked that often. `setTitle:` re-lays the titlebar
/// container, and so does a resize, a trip through full screen, and
/// every other thing that touches the window's chrome; there is no
/// notification for "the container laid out". Three frame reads and a
/// comparison is cheaper than being wrong.
pub fn keep_traffic_lights() {
    let window = LIGHTS_WINDOW.with(|slot| slot.get());
    if !window.is_null() {
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
    let Some((x, y, size)) = TRAFFIC_LIGHTS.with(|slot| slot.get()) else {
        return;
    };
    if window.is_null() {
        return;
    }
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

extern "C" fn bunny_blink(_this: Id, _sel: Sel, _timer: Id) {
    dispatch(AppEvent::Blink);
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
    dispatch(AppEvent::Frame { dt });
}

extern "C" fn bunny_window_will_close(_this: Id, _sel: Sel, _note: Id) {
    unsafe {
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
}

/// Pauses or resumes the per-frame driver. Without a link (an older
/// macOS) this is a no-op — animations then complete instantly.
pub fn set_frame_driver_paused(paused: bool) {
    LINK.with(|slot| {
        let link = slot.get();
        if !link.is_null() {
            unsafe { msg_void_bool(link, sel("setPaused:"), paused as i8) };
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
            bunny_window_did_resize as *const c_void,
            types.as_ptr(),
        );
        // a moved window re-clamps its popovers against the screen —
        // the child panels FOLLOW by AppKit's own hand; this repaint
        // only re-runs the overlay geometry
        class_addMethod(
            delegate,
            sel("windowDidMove:"),
            bunny_window_did_resize as *const c_void,
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
            sel("bunnyBlink:"),
            bunny_blink as *const c_void,
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

    /// The pointer's outfit over the scene. Direct `set` — no cursor
    /// rects for now (AppKit may restore it at resize edges; a cosmetic
    /// glitch we accept).
    pub fn set_cursor(&self, cursor: Cursor) {
        let name = match cursor {
            Cursor::Arrow => "arrowCursor",
            Cursor::Pointing => "pointingHandCursor",
            Cursor::ResizeLeftRight => "resizeLeftRightCursor",
        };
        unsafe {
            msg_void(msg_id(class("NSCursor"), sel(name)), sel("set"));
        }
    }
}

/// What the pointer wears: the hand over an interactive target, the
/// horizontal resizer over a split's grip, the arrow elsewhere.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Pointing,
    ResizeLeftRight,
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
        // titled | closable | miniaturizable | resizable
        // (+ full-size content when the scene owns the chrome)
        let style: u64 = if scene_chrome {
            1 | 2 | 4 | 8 | (1 << 15)
        } else {
            1 | 2 | 4 | 8
        };
        let window = msg_id(class("NSWindow"), sel("alloc"));
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

        // delegate: resize repaints, closing quits
        let delegate = msg_id(msg_id(class("BunnyDelegate"), sel("alloc")), sel("init"));
        msg_void_id(window, sel("setDelegate:"), delegate);

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
        if msg_bool_sel(
            view,
            sel("respondsToSelector:"),
            sel("displayLinkWithTarget:selector:"),
        ) != 0
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

        msg_void_id(window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
        // the keyboard is born pointing at the event view
        msg_void_id(window, sel("makeFirstResponder:"), view);
        msg_void_bool(app, sel("activateIgnoringOtherApps:"), 1);
        // LAST: the traffic lights are placed after everything that
        // touches the chrome. `setTitle:` alone puts them back where
        // the system wants them, and it runs above.
        LIGHTS_WINDOW.with(|slot| slot.set(window));
        place_traffic_lights(window);
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

        // hover and exit work inside the panel like anywhere else
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

        // the child contract: the panel rides every parent move
        msg_void_id_i64(parent.window, sel("addChildWindow:ordered:"), panel, 1);

        objc_autoreleasePoolPop(pool);
        WindowHandle { window: panel, view }
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
            msg_void_id(parent.window, sel("removeChildWindow:"), self.window);
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
