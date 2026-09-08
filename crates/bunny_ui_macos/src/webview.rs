//! The webview on the Mac: the shared tenant ([`bunny_ui_apple::webview`])
//! and the one thing only AppKit can add — synthetic input by REAL
//! NSEvents addressed at the view, the capability the table calls
//! `SyntheticInput`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

use bunny_ui::action::Modifiers;
use bunny_ui::host::{MouseButton, WebviewInput};
pub use bunny_ui_apple::webview::*;

use crate::ffi::{CGPoint, CGRect, Id, NS_NOT_FOUND, NSRange, Sel, class, sel};

// The wheel is the one event AppKit has no constructor for: a scroll
// NSEvent is BORN as a CGEvent and crosses over.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateScrollWheelEvent2(
        source: Id,
        units: u32,
        wheels: u32,
        first: i32,
        second: i32,
        third: i32,
    ) -> Id;
    fn CGEventSetLocation(event: Id, location: CGPoint);
    fn CGEventSetIntegerValueField(event: Id, field: u32, value: i64);
}


// The msgSend casts in the house pattern, local to the messages this
// module sends.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_id(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
    #[link_name = "objc_msgSend"]
    fn msg_bool_sel(obj: Id, sel: Sel, a: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_bool(obj: Id, sel: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_id_u64(obj: Id, sel: Sel, a: u64) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_f64(obj: Id, sel: Sel) -> f64;
    #[link_name = "objc_msgSend"]
    fn msg_rect(obj: Id, sel: Sel) -> CGRect;
    #[link_name = "objc_msgSend"]
    fn msg_point_point_id(obj: Id, sel: Sel, point: CGPoint, view: Id) -> CGPoint;
    #[link_name = "objc_msgSend"]
    fn msg_void_id_range(obj: Id, sel: Sel, a: Id, range: NSRange);
    /// `+[NSEvent mouseEventWithType:location:modifierFlags:timestamp:
    /// windowNumber:context:eventNumber:clickCount:pressure:]`
    #[link_name = "objc_msgSend"]
    fn msg_mouse_event(
        obj: Id,
        sel: Sel,
        kind: u64,
        location: CGPoint,
        flags: u64,
        timestamp: f64,
        window: i64,
        context: Id,
        number: i64,
        clicks: i64,
        pressure: f32,
    ) -> Id;
    /// `+[NSEvent keyEventWithType:location:modifierFlags:timestamp:
    /// windowNumber:context:characters:charactersIgnoringModifiers:
    /// isARepeat:keyCode:]`
    #[link_name = "objc_msgSend"]
    fn msg_key_event(
        obj: Id,
        sel: Sel,
        kind: u64,
        location: CGPoint,
        flags: u64,
        timestamp: f64,
        window: i64,
        context: Id,
        characters: Id,
        bare: Id,
        repeat: i8,
        code: u16,
    ) -> Id;

}

// The event types AppKit numbers, and the modifier bits it reads.
// Named here because a bare 25 in a call is nobody's idea of a middle
// button.
const LEFT_DOWN: u64 = 1;
const LEFT_UP: u64 = 2;
const RIGHT_DOWN: u64 = 3;
const RIGHT_UP: u64 = 4;
const MOUSE_MOVED: u64 = 5;
const LEFT_DRAGGED: u64 = 6;
const RIGHT_DRAGGED: u64 = 7;
const KEY_DOWN: u64 = 10;
const KEY_UP: u64 = 11;
const OTHER_DOWN: u64 = 25;
const OTHER_UP: u64 = 26;
const OTHER_DRAGGED: u64 = 27;
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_OPTION: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

/// One synthetic event into the page — the capability the table calls
/// `SyntheticInput`, served here by REAL NSEvents addressed at the
/// view.
///
/// This is why the mac can serve that cell at all: an event built by
/// `+[NSEvent mouseEventWithType:…]` and handed to the view is the
/// same object a hand produces, so the page reads `isTrusted` as true
/// and a button that guards on it works. The alternative is a
/// synthetic DOM event — `el.click()` — and real sites refuse those.
///
/// Fire-and-forget, the way nothing comes back from a hand. A page
/// with no window takes nothing: there are no coordinates to aim by.
pub(crate) fn input(view: Id, event: &WebviewInput) {
    unsafe {
        let window = msg_id(view, sel("window"));
        if window.is_null() {
            return;
        }
        crate::ffi::lend_hand(|| match event {
            WebviewInput::Click { x, y, clicks, button } => {
                let at = window_point(view, *x, *y);
                let (down, up) = press_kinds(*button);
                // a double click is two PAIRS, counted 1 then 2 — the
                // page reads the count off the second press, so one
                // press carrying a 2 is a lie a hand never tells
                for count in 1..=(*clicks).clamp(1, 3) {
                    send_mouse(view, window, down, at, Modifiers::NONE, count, 1.0);
                    send_mouse(view, window, up, at, Modifiers::NONE, count, 0.0);
                }
            }
            WebviewInput::Hover { x, y } => {
                let at = window_point(view, *x, *y);
                send_mouse(view, window, MOUSE_MOVED, at, Modifiers::NONE, 0, 0.0);
            }
            WebviewInput::Down { x, y, button, clicks, modifiers } => {
                let at = window_point(view, *x, *y);
                let (down, _) = press_kinds(*button);
                send_mouse(view, window, down, at, *modifiers, (*clicks).clamp(1, 3), 1.0);
            }
            WebviewInput::Up { x, y, button, clicks, modifiers } => {
                let at = window_point(view, *x, *y);
                let (_, up) = press_kinds(*button);
                send_mouse(view, window, up, at, *modifiers, (*clicks).clamp(1, 3), 0.0);
            }
            WebviewInput::Drag { x, y, button, modifiers } => {
                let at = window_point(view, *x, *y);
                let dragged = match button {
                    MouseButton::Left => LEFT_DRAGGED,
                    MouseButton::Right => RIGHT_DRAGGED,
                    MouseButton::Middle => OTHER_DRAGGED,
                };
                send_mouse(view, window, dragged, at, *modifiers, 1, 1.0);
            }
            WebviewInput::Scroll { x, y, dx, dy } => {
                send_wheel(view, window_point(view, *x, *y), *dx, *dy);
            }
            WebviewInput::Type { text } => send_text(view, window, text),
            WebviewInput::Key { key } => send_key(view, window, key),
        })
    }
}

/// The press and release types of one button.
const fn press_kinds(button: MouseButton) -> (u64, u64) {
    match button {
        MouseButton::Left => (LEFT_DOWN, LEFT_UP),
        MouseButton::Right => (RIGHT_DOWN, RIGHT_UP),
        // WebKit reads the middle button off the TYPE alone, so the
        // "other" pair is the middle one here
        MouseButton::Middle => (OTHER_DOWN, OTHER_UP),
    }
}

/// CSS pixels from the view's top-left corner, to the window
/// coordinates an NSEvent carries. The flip happens once, here: CSS
/// counts down from the top and AppKit counts up from the bottom,
/// unless the view says it is flipped (WKWebView does).
unsafe fn window_point(view: Id, x: f64, y: f64) -> CGPoint {
    unsafe {
        let bounds = msg_rect(view, sel("bounds"));
        let local = if msg_bool(view, sel("isFlipped")) != 0 {
            CGPoint { x: bounds.origin.x + x, y: bounds.origin.y + y }
        } else {
            CGPoint { x: bounds.origin.x + x, y: bounds.origin.y + bounds.size.height - y }
        };
        msg_point_point_id(view, sel("convertPoint:toView:"), local, null_mut())
    }
}

/// What the keyboard was holding, in AppKit's own bits.
const fn flags(modifiers: Modifiers) -> u64 {
    let mut bits = 0;
    if modifiers.shift {
        bits |= FLAG_SHIFT;
    }
    if modifiers.control {
        bits |= FLAG_CONTROL;
    }
    if modifiers.option {
        bits |= FLAG_OPTION;
    }
    if modifiers.command {
        bits |= FLAG_COMMAND;
    }
    bits
}

/// The clock an NSEvent is stamped with: seconds since the machine
/// woke, which is what AppKit puts there.
unsafe fn now() -> f64 {
    unsafe {
        let info = msg_id(class("NSProcessInfo"), sel("processInfo"));
        msg_f64(info, sel("systemUptime"))
    }
}

/// Builds one mouse event and hands it STRAIGHT to the view — not to
/// the window, and not to the queue the user's own hand shares. The
/// view is the address: the page takes what the app aimed at it, and
/// the pointer on the desk never moves.
unsafe fn send_mouse(
    view: Id,
    window: Id,
    kind: u64,
    at: CGPoint,
    modifiers: Modifiers,
    clicks: u32,
    pressure: f32,
) {
    unsafe {
        let event = msg_mouse_event(
            class("NSEvent"),
            sel(
                "mouseEventWithType:location:modifierFlags:timestamp:windowNumber:\
                 context:eventNumber:clickCount:pressure:",
            ),
            kind,
            at,
            flags(modifiers),
            now(),
            msg_i64(window, sel("windowNumber")),
            null_mut(),
            0,
            clicks as i64,
            pressure,
        );
        if !event.is_null() {
            let target = if kind == MOUSE_MOVED { move_door(view) } else { view };
            msg_void_id(target, sel(mouse_door(kind)), event);
        }
    }
}

/// Where a MOVE is heard. The engine watches the pointer through a
/// TRACKING AREA, and the area's owner — not the view — is what
/// answers `mouseMoved:`. Sent to the view, the message finds no
/// taker there and walks up the responder chain instead, which is the
/// app's own scene rather than the page.
unsafe fn move_door(view: Id) -> Id {
    unsafe {
        let areas = msg_id(view, sel("trackingAreas"));
        if areas.is_null() {
            return view;
        }
        for index in 0..msg_i64(areas, sel("count")).max(0) as u64 {
            let owner = msg_id(msg_id_u64(areas, sel("objectAtIndex:"), index), sel("owner"));
            if owner.is_null() || std::ptr::eq(owner, view) {
                continue;
            }
            if msg_bool_sel(owner, sel("respondsToSelector:"), sel("mouseMoved:")) != 0 {
                return owner;
            }
        }
        view
    }
}

/// The message a mouse event of this type is delivered by.
const fn mouse_door(kind: u64) -> &'static str {
    match kind {
        LEFT_DOWN => "mouseDown:",
        LEFT_UP => "mouseUp:",
        RIGHT_DOWN => "rightMouseDown:",
        RIGHT_UP => "rightMouseUp:",
        LEFT_DRAGGED => "mouseDragged:",
        RIGHT_DRAGGED => "rightMouseDragged:",
        OTHER_DOWN => "otherMouseDown:",
        OTHER_UP => "otherMouseUp:",
        OTHER_DRAGGED => "otherMouseDragged:",
        _ => "mouseMoved:",
    }
}

/// The wheel, at a point. AppKit's event constructor refuses this one
/// type, so the event is born in CoreGraphics and crosses over — and
/// an event that crossed over carries no window, which makes AppKit
/// read its location as SCREEN coordinates. The location is therefore
/// written as the window point flipped about the first screen, so the
/// page is asked about the point the app named.
///
/// The deltas arrive in the page's signs (`dy` counts down) and the
/// wheel's are the opposite: a wheel that turns away from the hand
/// moves the content up.
///
/// It goes as a GESTURE, in two beats: one that begins and carries
/// the whole delta, and one that ends and carries nothing. A precise
/// scroll is a gesture, and one that never ends leaves the engine
/// holding it open — with the end, the engine closes it and moves the
/// pointer over what is now under it, exactly as it does for a hand
/// on a trackpad (the page reports that move; it is the engine's own,
/// not one the app sent).
unsafe fn send_wheel(view: Id, at: CGPoint, dx: f64, dy: f64) {
    const PIXEL_UNITS: u32 = 0;
    const SCROLL_PHASE: u32 = 99;
    const PHASE_BEGAN: i64 = 1;
    const PHASE_ENDED: i64 = 4;
    unsafe {
        for (phase, x, y) in [(PHASE_BEGAN, -dx as i32, -dy as i32), (PHASE_ENDED, 0, 0)] {
            let event = CGEventCreateScrollWheelEvent2(null_mut(), PIXEL_UNITS, 2, y, x, 0);
            if event.is_null() {
                return;
            }
            CGEventSetIntegerValueField(event, SCROLL_PHASE, phase);
            if let Some(height) = main_screen_height() {
                CGEventSetLocation(event, CGPoint { x: at.x, y: height - at.y });
            }
            let carried = msg_id_id(class("NSEvent"), sel("eventWithCGEvent:"), event);
            if !carried.is_null() {
                msg_void_id(view, sel("scrollWheel:"), carried);
            }
            crate::ffi::CFRelease(event as *const c_void);
        }
    }
}

/// The height AppKit flips a screen-coordinate event about — the
/// first screen, which is the one the menu bar lives on.
unsafe fn main_screen_height() -> Option<f64> {
    unsafe {
        let screens = msg_id(class("NSScreen"), sel("screens"));
        if screens.is_null() {
            return None;
        }
        let screen = msg_id(screens, sel("firstObject"));
        if screen.is_null() {
            return None;
        }
        Some(msg_rect(screen, sel("frame")).size.height)
    }
}

/// Types into the page's own focus. This is a COMMIT, not a run of
/// keystrokes: `insertText:replacementRange:` is the door an input
/// method and a paste both land through, so the text arrives in a
/// field, in a `contenteditable`, and under a keyboard layout nobody
/// guessed. Where the engine does not answer that message, each
/// character goes as its own keystroke instead.
unsafe fn send_text(view: Id, window: Id, text: &str) {
    unsafe {
        let door = sel("insertText:replacementRange:");
        if msg_bool_sel(view, sel("respondsToSelector:"), door) != 0 {
            msg_void_id_range(view, door, ns(text), NSRange { location: NS_NOT_FOUND, length: 0 });
            return;
        }
        for character in text.chars() {
            stroke(view, window, 0, &character.to_string());
        }
    }
}

/// One named key, by the name the page uses. The mac numbers its keys
/// by POSITION on the board, and spells the ones with no character of
/// their own inside a private area of Unicode — the engine reads both
/// to name the key back to the page.
unsafe fn send_key(view: Id, window: Id, key: &str) {
    let (code, characters) = match key {
        "Enter" | "Return" => (36, "\r"),
        "Tab" => (48, "\t"),
        "Escape" | "Esc" => (53, "\u{1b}"),
        "Backspace" => (51, "\u{7f}"),
        "Delete" => (117, "\u{f728}"),
        "ArrowUp" => (126, "\u{f700}"),
        "ArrowDown" => (125, "\u{f701}"),
        "ArrowLeft" => (123, "\u{f702}"),
        "ArrowRight" => (124, "\u{f703}"),
        "Home" => (115, "\u{f729}"),
        "End" => (119, "\u{f72b}"),
        "PageUp" => (116, "\u{f72c}"),
        "PageDown" => (121, "\u{f72d}"),
        "Space" => (49, " "),
        // a single character IS its own key name, the way the page
        // spells it; a name nobody knows presses nothing
        other if other.chars().count() == 1 => (0, other),
        _ => return,
    };
    unsafe { stroke(view, window, code, characters) }
}

/// One press and release of a key, delivered to the view.
unsafe fn stroke(view: Id, window: Id, code: u16, characters: &str) {
    unsafe {
        let door = sel(
            "keyEventWithType:location:modifierFlags:timestamp:windowNumber:\
             context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
        );
        let number = msg_i64(window, sel("windowNumber"));
        let origin = CGPoint { x: 0.0, y: 0.0 };
        for (kind, deliver) in [(KEY_DOWN, "keyDown:"), (KEY_UP, "keyUp:")] {
            let text = ns(characters);
            let event = msg_key_event(
                class("NSEvent"),
                door,
                kind,
                origin,
                0,
                now(),
                number,
                null_mut(),
                text,
                text,
                0,
                code,
            );
            if !event.is_null() {
                msg_void_id(view, sel(deliver), event);
            }
        }
    }
}


/// A borrowed NSString for the message being sent (autoreleased).
unsafe fn ns(text: &str) -> Id {
    let text = CString::new(text).unwrap_or_default();
    unsafe { msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr()) }
}

#[allow(dead_code)]
unsafe fn to_string(ns: Id) -> String {
    if ns.is_null() {
        return String::new();
    }
    unsafe {
        let utf8 = msg_id(ns, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return String::new();
        }
        CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}
