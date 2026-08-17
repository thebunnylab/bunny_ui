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
use std::ffi::{CString, c_char, c_void};
use std::sync::Once;

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
}

// AppKit/QuartzCore come in via the ObjC runtime; the link guarantees the
// classes.
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    /// The pasteboard string type (`public.utf8-plain-text`).
    static NSPasteboardTypeString: Id;
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
    fn CGContextDrawImage(context: Id, rect: CGRect, image: Id);
    fn CGContextSetInterpolationQuality(context: Id, quality: i32);
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
    fn CGImageRelease(image: Id);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CFRelease(cf: *const c_void);
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
    MouseDown { x: f64, y: f64 },
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
    /// The window changed size (or needs the first frame).
    Redraw,
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

/// The event position in layout coordinates — AppKit counts from the
/// bottom, the layout counts from the top; the flip lives here, once.
unsafe fn event_layout_point(this: Id, event: Id) -> (f64, f64) {
    unsafe {
        let point = msg_point(event, sel("locationInWindow"));
        let bounds = msg_rect(this, sel("bounds"));
        (point.x, bounds.size.height - point.y)
    }
}

extern "C" fn bunny_mouse_down(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    dispatch(AppEvent::MouseDown { x, y });
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
    /// `charactersIgnoringModifiers` — the BASE char: `Char` patterns
    /// match through here (shift/option do not change the key's identity).
    pub chars_ignoring: String,
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
/// events; reading travels through here).
#[derive(Clone, Copy)]
struct ImeMirror {
    selected: NSRange,
    marked: NSRange,
    caret_screen: CGRect,
}

thread_local! {
    static IME: Cell<Option<ImeMirror>> = const { Cell::new(None) };
    /// keyDown enters the input system (composition) only when the shell
    /// says a field is focused.
    static INTERPRET: Cell<bool> = const { Cell::new(false) };
}

/// The shell syncs the focused-field mirror (`None` = no focus).
pub fn sync_ime(state: Option<(NSRange, Option<NSRange>, CGRect)>) {
    INTERPRET.with(|flag| flag.set(state.is_some()));
    IME.with(|ime| {
        ime.set(state.map(|(selected, marked, caret_screen)| ImeMirror {
            selected,
            marked: marked.unwrap_or(NSRange { location: NS_NOT_FOUND, length: 0 }),
            caret_screen,
        }));
    });
}

fn ime_mirror() -> Option<ImeMirror> {
    IME.with(|ime| ime.get())
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
    _range: NSRange,
    _actual: *mut NSRange,
) -> Id {
    // honest floor: some IMEs query this for reconversion — without it
    // normal composition keeps working
    std::ptr::null_mut()
}

extern "C" fn bunny_valid_attributes(_this: Id, _sel: Sel) -> Id {
    unsafe { msg_id(class("NSArray"), sel("array")) }
}

/// Where the candidate window lands: the caret rect, on screen.
extern "C" fn bunny_first_rect(
    _this: Id,
    _sel: Sel,
    _range: NSRange,
    _actual: *mut NSRange,
) -> CGRect {
    ime_mirror().map(|ime| ime.caret_screen).unwrap_or(CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 0.0, height: 0.0 },
    })
}

extern "C" fn bunny_character_index(_this: Id, _sel: Sel, _point: CGPoint) -> u64 {
    // honest floor (dictionary lookup by mouse comes later)
    0
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

extern "C" fn bunny_window_did_resize(_this: Id, _sel: Sel, _note: Id) {
    dispatch(AppEvent::Redraw);
}

extern "C" fn bunny_blink(_this: Id, _sel: Sel, _timer: Id) {
    dispatch(AppEvent::Blink);
}

extern "C" fn bunny_window_will_close(_this: Id, _sel: Sel, _note: Id) {
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_id(app, sel("terminate:"), std::ptr::null_mut());
    }
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
        class_addMethod(
            delegate,
            sel("bunnyBlink:"),
            bunny_blink as *const c_void,
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
        BACKING.with(|backing| {
            let mut backing = backing.borrow_mut();
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

    /// Hand over an interactive target; arrow elsewhere. Direct `set` —
    /// no cursor rects for now (AppKit may restore it at resize edges; a
    /// cosmetic glitch we accept).
    pub fn set_cursor_pointing(&self, pointing: bool) {
        unsafe {
            let cursor = if pointing {
                msg_id(class("NSCursor"), sel("pointingHandCursor"))
            } else {
                msg_id(class("NSCursor"), sel("arrowCursor"))
            };
            msg_void(cursor, sel("set"));
        }
    }
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
    static BACKING: RefCell<Backing> =
        RefCell::new(Backing { width: 0, height: 0, bytes: Vec::new() });
}

/// `drawRect:` — paints the dirty union from the backing through a
/// NO-COPY CGImage. The context arrives clipped to the dirty rect; the
/// CTM flip converts our top-down rows to AppKit's bottom-up world.
extern "C" fn bunny_draw_rect(this: Id, _sel: Sel, _dirty: CGRect) {
    BACKING.with(|backing| {
        let backing = backing.borrow();
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
pub fn create_window(title: &str, width: f64, height: f64) -> WindowHandle {
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
        let style: u64 = 1 | 2 | 4 | 8;
        let window = msg_id(class("NSWindow"), sel("alloc"));
        let window = msg_init_window(
            window,
            sel("initWithContentRect:styleMask:backing:defer:"),
            rect,
            style,
            2, // buffered
            0,
        );

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

        msg_void_id(window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
        // the keyboard is born pointing at the event view
        msg_void_id(window, sel("makeFirstResponder:"), view);
        msg_void_bool(app, sel("activateIgnoringOtherApps:"), 1);
        objc_autoreleasePoolPop(pool);

        WindowHandle { window, view }
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
