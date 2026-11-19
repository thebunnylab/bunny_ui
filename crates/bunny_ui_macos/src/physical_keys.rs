//! AppKit's virtual key codes as the W3C UI Events `code` vocabulary
//! names the same keys: the place of a key on the board, whatever the
//! layout prints on it.

/// The `code` of an AppKit virtual key code; empty for one the
/// vocabulary has no name for.
pub(crate) const fn code(key_code: u16) -> &'static str {
    match key_code {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        10 => "IntlBackslash",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        22 => "Digit6",
        23 => "Digit5",
        24 => "Equal",
        25 => "Digit9",
        26 => "Digit7",
        27 => "Minus",
        28 => "Digit8",
        29 => "Digit0",
        30 => "BracketRight",
        31 => "KeyO",
        32 => "KeyU",
        33 => "BracketLeft",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Enter",
        37 => "KeyL",
        38 => "KeyJ",
        39 => "Quote",
        40 => "KeyK",
        41 => "Semicolon",
        42 => "Backslash",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "Backquote",
        51 => "Backspace",
        53 => "Escape",
        54 => "MetaRight",
        55 => "MetaLeft",
        56 => "ShiftLeft",
        57 => "CapsLock",
        58 => "AltLeft",
        59 => "ControlLeft",
        60 => "ShiftRight",
        61 => "AltRight",
        62 => "ControlRight",
        63 => "Fn",
        64 => "F17",
        65 => "NumpadDecimal",
        67 => "NumpadMultiply",
        69 => "NumpadAdd",
        71 => "NumLock",
        72 => "AudioVolumeUp",
        73 => "AudioVolumeDown",
        74 => "AudioVolumeMute",
        75 => "NumpadDivide",
        76 => "NumpadEnter",
        78 => "NumpadSubtract",
        79 => "F18",
        80 => "F19",
        81 => "NumpadEqual",
        82 => "Numpad0",
        83 => "Numpad1",
        84 => "Numpad2",
        85 => "Numpad3",
        86 => "Numpad4",
        87 => "Numpad5",
        88 => "Numpad6",
        89 => "Numpad7",
        90 => "F20",
        91 => "Numpad8",
        92 => "Numpad9",
        93 => "IntlYen",
        94 => "IntlRo",
        95 => "NumpadComma",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        102 => "Lang2",
        103 => "F11",
        104 => "Lang1",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        110 => "ContextMenu",
        111 => "F12",
        113 => "F15",
        114 => "Insert",
        115 => "Home",
        116 => "PageUp",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "PageDown",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => "",
    }
}

/// The modifier flags that change what a key TYPES: caps lock, shift
/// and option. Command and control make chords and are set aside.
pub(crate) const TYPING_FLAGS: u64 = (1 << 16) | (1 << 17) | (1 << 19);

/// Whether the modifier key `key_code` is down now, after the
/// `flagsChanged:` that carried `flags`; `None` for a key that is no
/// modifier.
///
/// The side comes from the device-dependent bits, so a right shift let
/// go while the left one stays held still reads as a release. A source
/// that sets only the generic bit is read by the generic bit.
pub(crate) const fn modifier_down(key_code: u16, flags: u64) -> Option<bool> {
    let (own, both, generic) = match key_code {
        56 => (0x02, 0x06, 1 << 17),
        60 => (0x04, 0x06, 1 << 17),
        59 => (0x01, 0x2001, 1 << 18),
        62 => (0x2000, 0x2001, 1 << 18),
        58 => (0x20, 0x60, 1 << 19),
        61 => (0x40, 0x60, 1 << 19),
        55 => (0x08, 0x18, 1 << 20),
        54 => (0x10, 0x18, 1 << 20),
        // the lock, not the key: down while it is on, as Chrome tells it
        57 => return Some(flags & (1 << 16) != 0),
        _ => return None,
    };
    Some(if flags & both == 0 {
        flags & generic != 0
    } else {
        flags & own != 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_modifiers_are_named_by_their_place() {
        assert_eq!(code(0), "KeyA");
        assert_eq!(code(18), "Digit1");
        assert_eq!(code(56), "ShiftLeft");
        assert_eq!(code(54), "MetaRight");
        assert_eq!(code(76), "NumpadEnter");
        assert_eq!(code(52), "");
    }

    #[test]
    fn a_side_let_go_reads_as_released_while_the_other_stays_down() {
        let both_shifts = (1 << 17) | 0x02 | 0x04;
        assert_eq!(modifier_down(60, both_shifts), Some(true));
        let left_only = (1 << 17) | 0x02;
        assert_eq!(modifier_down(60, left_only), Some(false));
        assert_eq!(modifier_down(56, left_only), Some(true));
        assert_eq!(modifier_down(56, 1 << 17), Some(true));
        assert_eq!(modifier_down(56, 0), Some(false));
        assert_eq!(modifier_down(0, 0), None);
    }
}
