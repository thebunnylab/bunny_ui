//! Keys and taps in the terms the keymap reads — pure, so the Mac runs
//! the tests the phone cannot.
//!
//! Android hands a `NativeActivity` key EVENTS, not text: a key code
//! and the modifiers held. What the key types comes from a table of
//! the keys a latin layout has (the soft keyboard sends the same codes
//! for the keys it shows). A composed character, a script the table
//! does not know, an emoji — those need an `InputConnection`, which is
//! Java, and do not arrive; the crate doc says so.

use std::time::{Duration, Instant};

use bunny_ui::action::{Key, KeyPattern};

// android/keycodes.h — the named keys the shell reads by number
pub const KEYCODE_BACK: i32 = 4;
pub const KEYCODE_A: i32 = 29;
pub const KEYCODE_C: i32 = 31;
pub const KEYCODE_V: i32 = 50;
pub const KEYCODE_X: i32 = 52;
pub const KEYCODE_TAB: i32 = 61;
pub const KEYCODE_ENTER: i32 = 66;
pub const KEYCODE_DEL: i32 = 67;
pub const KEYCODE_PAGE_UP: i32 = 92;
pub const KEYCODE_PAGE_DOWN: i32 = 93;
pub const KEYCODE_ESCAPE: i32 = 111;
pub const KEYCODE_FORWARD_DEL: i32 = 112;
pub const KEYCODE_MOVE_HOME: i32 = 122;
pub const KEYCODE_MOVE_END: i32 = 123;
pub const KEYCODE_NUMPAD_ENTER: i32 = 160;
pub const KEYCODE_DPAD_UP: i32 = 19;
pub const KEYCODE_DPAD_DOWN: i32 = 20;
pub const KEYCODE_DPAD_LEFT: i32 = 21;
pub const KEYCODE_DPAD_RIGHT: i32 = 22;
pub const KEYCODE_DPAD_CENTER: i32 = 23;

// android/input.h — the meta-state bits
const META_SHIFT_ON: i32 = 0x01;
const META_ALT_ON: i32 = 0x02;
const META_CTRL_ON: i32 = 0x1000;
const META_META_ON: i32 = 0x10000;
const META_CAPS_LOCK_ON: i32 = 0x100000;

/// One key press, in the terms the keymap reads.
#[derive(Clone, Debug)]
pub struct KeyStroke {
    /// The Android key code (`AKEYCODE_*`).
    pub keycode: i32,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
    /// What the key types with no modifier applied — empty for a key
    /// that types nothing.
    pub chars_ignoring: String,
    /// The character this key TYPED under the modifiers held; `None`
    /// under control, alt or meta, and for a key that types nothing.
    pub typed: Option<char>,
}

/// The stroke a key event is, from its code and the meta state.
pub fn stroke_of(keycode: i32, meta_state: i32) -> KeyStroke {
    let shift = meta_state & META_SHIFT_ON != 0;
    let control = meta_state & META_CTRL_ON != 0;
    let alt = meta_state & META_ALT_ON != 0;
    let meta = meta_state & META_META_ON != 0;
    let caps = meta_state & META_CAPS_LOCK_ON != 0;
    let (chars_ignoring, typed) = match lookup(keycode) {
        Some((base, shifted)) => {
            let typed = if control || alt || meta {
                None
            } else if base.is_ascii_alphabetic() {
                // caps lock and shift each turn a letter over; both
                // together turn it back
                Some(if shift ^ caps { base.to_ascii_uppercase() } else { base })
            } else if shift {
                Some(shifted)
            } else {
                Some(base)
            };
            (base.to_string(), typed)
        }
        None => (String::new(), None),
    };
    KeyStroke { keycode, shift, control, alt, meta, chars_ignoring, typed }
}

/// What a key types on a latin layout: the plain character and the
/// shifted one. `None` = the key types nothing (a named key, a
/// modifier, a key the table does not know).
pub fn lookup(keycode: i32) -> Option<(char, char)> {
    Some(match keycode {
        29..=54 => {
            let letter = (b'a' + (keycode - 29) as u8) as char;
            (letter, letter.to_ascii_uppercase())
        }
        7..=16 => {
            let digit = (b'0' + (keycode - 7) as u8) as char;
            (digit, b")!@#$%^&*("[(keycode - 7) as usize] as char)
        }
        144..=153 => {
            let digit = (b'0' + (keycode - 144) as u8) as char;
            (digit, digit)
        }
        62 => (' ', ' '),
        55 => (',', '<'),
        56 => ('.', '>'),
        68 => ('`', '~'),
        69 => ('-', '_'),
        70 => ('=', '+'),
        71 => ('[', '{'),
        72 => (']', '}'),
        73 => ('\\', '|'),
        74 => (';', ':'),
        75 => ('\'', '"'),
        76 => ('/', '?'),
        77 => ('@', '@'),
        81 => ('+', '+'),
        17 => ('*', '*'),
        18 => ('#', '#'),
        _ => return None,
    })
}

/// Android key code → the keymap vocabulary. Named keys come from the
/// code; the rest becomes `Char` through the key's OWN character —
/// what it types with no modifier applied — lowercased, because CapsLock
/// is never a chord. `None` = lone modifier/function key.
///
/// The modifier mapping is the platform's: Ctrl is the accelerator,
/// so Ctrl carries `command`; Alt carries `option`; the `control` flag
/// stays false (Meta belongs to the system). The phone's back key is
/// the escape — it closes what is open before it leaves the app.
pub fn key_pattern(stroke: &KeyStroke) -> Option<KeyPattern> {
    let named = match stroke.keycode {
        KEYCODE_DPAD_DOWN => Some(Key::Down),
        KEYCODE_DPAD_UP => Some(Key::Up),
        KEYCODE_DPAD_LEFT => Some(Key::Left),
        KEYCODE_DPAD_RIGHT => Some(Key::Right),
        KEYCODE_ENTER | KEYCODE_NUMPAD_ENTER | KEYCODE_DPAD_CENTER => Some(Key::Enter),
        KEYCODE_ESCAPE | KEYCODE_BACK => Some(Key::Escape),
        KEYCODE_TAB => Some(Key::Tab),
        KEYCODE_PAGE_UP => Some(Key::PageUp),
        KEYCODE_PAGE_DOWN => Some(Key::PageDown),
        KEYCODE_DEL => Some(Key::Backspace),
        KEYCODE_FORWARD_DEL => Some(Key::Delete),
        KEYCODE_MOVE_HOME => Some(Key::Home),
        KEYCODE_MOVE_END => Some(Key::End),
        _ => None,
    };
    let key = named.or_else(|| {
        let base = stroke.chars_ignoring.chars().next()?;
        (!base.is_control()).then(|| Key::Char(base.to_ascii_lowercase()))
    })?;
    Some(KeyPattern {
        key,
        shift: stroke.shift,
        command: stroke.control,
        option: stroke.alt,
        control: false,
    })
}

/// The tap count a touch carries — the platform gives none, so the
/// shell counts: a down within the window and the reach of the last
/// up continues the series.
pub struct TapCounter {
    last_up: Option<(Instant, f64, f64)>,
    count: u8,
}

impl TapCounter {
    /// The series' window, from the last lift to the next press.
    const WINDOW: Duration = Duration::from_millis(300);
    /// How far the next press may land from the last lift, in points.
    const REACH: f64 = 25.0;

    pub const fn new() -> TapCounter {
        TapCounter { last_up: None, count: 0 }
    }

    /// A finger came down at `(x, y)`; answers the tap it is.
    pub fn down(&mut self, now: Instant, x: f64, y: f64) -> u8 {
        let continues = self.last_up.is_some_and(|(at, last_x, last_y)| {
            now.duration_since(at) <= Self::WINDOW
                && (x - last_x).hypot(y - last_y) <= Self::REACH
        });
        self.count = if continues { self.count.saturating_add(1) } else { 1 };
        self.count
    }

    /// The finger lifted at `(x, y)`.
    pub fn up(&mut self, now: Instant, x: f64, y: f64) {
        self.last_up = Some((now, x, y));
    }

    /// The system took the touch back: the series ends.
    pub fn cancel(&mut self) {
        self.last_up = None;
        self.count = 0;
    }
}

impl Default for TapCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_map_to_the_vocabulary() {
        assert_eq!(key_pattern(&stroke_of(KEYCODE_DPAD_DOWN, 0)).unwrap().key, Key::Down);
        assert_eq!(key_pattern(&stroke_of(KEYCODE_ENTER, 0)).unwrap().key, Key::Enter);
        assert_eq!(key_pattern(&stroke_of(KEYCODE_NUMPAD_ENTER, 0)).unwrap().key, Key::Enter);
        assert_eq!(key_pattern(&stroke_of(KEYCODE_ESCAPE, 0)).unwrap().key, Key::Escape);
        assert_eq!(key_pattern(&stroke_of(KEYCODE_BACK, 0)).unwrap().key, Key::Escape);
    }

    #[test]
    fn control_carries_command() {
        let pattern = key_pattern(&stroke_of(29 + 5, META_CTRL_ON)).unwrap(); // ctrl-f
        assert_eq!(pattern.key, Key::Char('f'));
        assert!(pattern.command);
        assert!(!pattern.is_text_input(), "a chord is never typing");
    }

    #[test]
    fn a_bare_letter_is_text_input() {
        let stroke = stroke_of(KEYCODE_A, 0);
        assert_eq!(stroke.typed, Some('a'));
        let pattern = key_pattern(&stroke).unwrap();
        assert_eq!(pattern.key, Key::Char('a'));
        assert!(pattern.is_text_input());
    }

    #[test]
    fn a_lone_modifier_is_no_pattern() {
        assert!(key_pattern(&stroke_of(59, META_SHIFT_ON)).is_none(), "left shift alone");
        assert!(key_pattern(&stroke_of(131, 0)).is_none(), "F1 is silent");
    }

    #[test]
    fn shift_and_caps_lock_turn_a_letter_over() {
        assert_eq!(stroke_of(KEYCODE_A, META_SHIFT_ON).typed, Some('A'));
        assert_eq!(stroke_of(KEYCODE_A, META_CAPS_LOCK_ON).typed, Some('A'));
        assert_eq!(stroke_of(KEYCODE_A, META_SHIFT_ON | META_CAPS_LOCK_ON).typed, Some('a'));
        // the key's identity never turns
        let pattern = key_pattern(&stroke_of(KEYCODE_A, META_SHIFT_ON | META_ALT_ON)).unwrap();
        assert_eq!(pattern.key, Key::Char('a'));
        assert!(pattern.option);
    }

    #[test]
    fn shift_reaches_the_second_row_of_a_digit() {
        assert_eq!(stroke_of(8, 0).typed, Some('1'));
        assert_eq!(stroke_of(8, META_SHIFT_ON).typed, Some('!'));
        assert_eq!(stroke_of(69, META_SHIFT_ON).typed, Some('_'));
        assert_eq!(stroke_of(KEYCODE_A, META_CTRL_ON).typed, None, "a chord types nothing");
    }

    #[test]
    fn taps_count_within_the_window_and_the_reach() {
        let mut taps = TapCounter::new();
        let start = Instant::now();
        assert_eq!(taps.down(start, 10.0, 10.0), 1);
        taps.up(start + Duration::from_millis(50), 10.0, 10.0);
        assert_eq!(taps.down(start + Duration::from_millis(200), 14.0, 12.0), 2);
        taps.up(start + Duration::from_millis(250), 14.0, 12.0);
        // too far
        assert_eq!(taps.down(start + Duration::from_millis(300), 90.0, 12.0), 1);
        taps.up(start + Duration::from_millis(350), 90.0, 12.0);
        // too late
        assert_eq!(taps.down(start + Duration::from_millis(800), 90.0, 12.0), 1);
        taps.cancel();
        taps.up(start + Duration::from_millis(850), 90.0, 12.0);
        assert_eq!(taps.down(start + Duration::from_millis(900), 90.0, 12.0), 1);
    }
}
