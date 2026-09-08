//! The Objective-C and CoreFoundation glue every Apple shell speaks.
//!
//! One runtime serves macOS and iOS: `objc_msgSend` re-declared with the
//! concrete signature of each message (arm64 has ONE entry point — small
//! structs travel in registers, there is no `_stret` twin), classes born
//! at runtime through `objc_allocateClassPair`/`class_addMethod`, and
//! the CoreFoundation run loop the frame lives on. What differs per
//! platform — AppKit's windows and events, UIKit's touches and
//! responders — stays in the shell that owns it. This module holds only
//! what both need, so a struct layout or a selector is written once.
//!
//! Every module of the Apple half declares its OWN `objc_msgSend`
//! aliases for the messages it sends (the house pattern). The alias
//! table below serves this module alone.

use std::ffi::{CString, c_char, c_void};
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

/// `NSNotFound` (NSIntegerMax) — Foundation's "no range".
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

/// The receiver-and-class pair `objc_msgSendSuper` walks up from —
/// how an added method still reaches the implementation it shadowed.
#[repr(C)]
pub struct ObjcSuper {
    pub receiver: Id,
    pub class: Id,
}

// Re-declaring `objc_msgSend` with the concrete signature of each message
// is the runtime's designed usage (the symbol is a trampoline that
// preserves the call ABI) — the clashing-declarations lint does not apply.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    pub fn objc_getClass(name: *const c_char) -> Id;
    /// The class OF an object — for a class object, its metaclass. A
    /// class method (`+layerClass`) is added there, not on the class.
    pub fn object_getClass(obj: Id) -> Id;
    pub fn sel_registerName(name: *const c_char) -> Sel;
    pub fn sel_getName(sel: Sel) -> *const c_char;
    pub fn objc_autoreleasePoolPush() -> *mut c_void;
    pub fn objc_autoreleasePoolPop(pool: *mut c_void);
    pub fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    pub fn objc_registerClassPair(class: Id);
    pub fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    pub fn objc_getProtocol(name: *const c_char) -> Id;
    pub fn class_addProtocol(class: Id, protocol: Id) -> i8;

    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_cstr(obj: Id, sel: Sel) -> *const c_char;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_id_id(obj: Id, sel: Sel, a: Id, b: Id);
    #[link_name = "objc_msgSend"]
    fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    /// The run-loop mode set that keeps a callback alive during event
    /// tracking (live resize, menus, a scroll) — the display link
    /// schedules here.
    pub static NSRunLoopCommonModes: Id;
}

// QuartzCore comes in via the ObjC runtime; the link guarantees the
// layer classes.
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    pub fn CGColorSpaceRelease(space: *mut c_void);
    pub fn CGContextDrawImage(context: Id, rect: CGRect, image: Id);
    pub fn CGContextSetInterpolationQuality(context: Id, quality: i32);
    pub fn CGImageRelease(image: Id);
    pub fn CGDataProviderCreateWithCFData(data: *const c_void) -> *mut c_void;
    pub fn CGDataProviderRelease(provider: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    pub fn CGImageCreate(
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
}

/// `kCGImageAlphaPremultipliedLast` — the RGBA layout a layer's
/// contents and a drawing context agree on.
pub const ALPHA_PREMULTIPLIED_LAST: u32 = 1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(cf: *const c_void);
    pub fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
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

pub unsafe fn class(name: &str) -> Id {
    let name = CString::new(name).expect("class name without NUL");
    unsafe { objc_getClass(name.as_ptr()) }
}

pub unsafe fn sel(name: &str) -> Sel {
    let name = CString::new(name).expect("selector without NUL");
    unsafe { sel_registerName(name.as_ptr()) }
}

/// A fresh autoreleased `NSString` with this text.
pub unsafe fn ns_string(text: &str) -> Id {
    let text = CString::new(text).expect("string without NUL");
    unsafe { msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr()) }
}

/// An `NSError`'s own words, or a placeholder when it has none.
pub unsafe fn error_message(error: Id) -> String {
    unsafe {
        if error.is_null() {
            return "unknown error".to_string();
        }
        let description = msg_id(error, sel("localizedDescription"));
        if description.is_null() {
            return "unknown error".to_string();
        }
        let chars = msg_cstr(description, sel("UTF8String"));
        if chars.is_null() {
            return "unknown error".to_string();
        }
        std::ffi::CStr::from_ptr(chars).to_string_lossy().into_owned()
    }
}

/// NSString OR NSAttributedString → Rust (the input system sends both).
pub unsafe fn text_argument_to_string(object: Id) -> String {
    unsafe {
        if object.is_null() {
            return String::new();
        }
        let plain = if msg_bool_sel(object, sel("respondsToSelector:"), sel("string")) != 0 {
            msg_id(object, sel("string"))
        } else {
            object
        };
        let utf8 = msg_cstr(plain, sel("UTF8String"));
        if utf8.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}

/// The four the keymap names, out of one modifier bitfield. AppKit's
/// `NSEventModifierFlags` and UIKit's `UIKeyModifierFlags` place them
/// on the SAME bits, so the key road and the pointer road on both
/// platforms read them here and cannot drift.
pub fn modifiers_of(flags: u64) -> bunny_ui::action::Modifiers {
    bunny_ui::action::Modifiers {
        shift: flags & (1 << 17) != 0,
        control: flags & (1 << 18) != 0,
        option: flags & (1 << 19) != 0,
        command: flags & (1 << 20) != 0,
    }
}

/// A data provider that OWNS a copy of the bytes. A layer's contents
/// are read by the render server after the transaction — a provider
/// that only borrows the shell's buffer paints a small image (the
/// commit copies it inline) and silently paints NOTHING once the
/// image is big enough to be mapped instead of copied. CFData owns the
/// copy, the provider retains the CFData, the image retains the
/// provider: the pixels stay truthful for as long as the layer shows
/// them.
pub unsafe fn owned_provider(bytes: *const u8, length: usize) -> *mut c_void {
    unsafe {
        let data = CFDataCreate(std::ptr::null(), bytes, length as isize);
        let provider = CGDataProviderCreateWithCFData(data);
        CFRelease(data);
        provider
    }
}

/// Removes CoreAnimation's implicit animations from a layer the shell
/// created. The platform turns them off for the backing layers IT
/// makes; a layer handed to `setLayer:` — or added as a raw sublayer —
/// keeps the default quarter-second actions, and the first abrupt step
/// of a live resize then CROSSFADES the old content over the new: the
/// whole window reads double-exposed until the animation lands, which
/// no native window does. Per-mutation `setDisableActions:` cannot
/// cover this — the resize mutates the layer from the PLATFORM's own
/// transaction. The dictionary answers at the layer, for every
/// transaction; NSNull is CoreAnimation's own word for "no action".
pub unsafe fn kill_layer_actions(layer: Id) {
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
            let key = ns_string(key);
            msg_void_id_id(actions, sel("setObject:forKey:"), null, key);
        }
        msg_void_id(layer, sel("setActions:"), actions);
    }
}

/// The run loop source a background thread knocks on. It lives in a
/// static (not a thread-local) because the signal comes from ANY
/// thread — `CFRunLoopSourceSignal` and `CFRunLoopWakeUp` are the
/// thread-safe half of CoreFoundation.
static WAKE_SOURCE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Opens that door. Called once, on the main thread, while the first
/// window is being built. `perform` is the shell's own answer to a
/// knock — it runs on the main thread, on the next turn of the loop.
pub fn install_wake_source(perform: extern "C" fn(*mut c_void)) {
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
            perform: Some(perform),
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
