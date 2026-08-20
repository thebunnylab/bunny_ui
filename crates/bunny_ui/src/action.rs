//! Named actions + key patterns — the smallest keymap with the right shape.
//!
//! A key becomes INTENT (`KeyPattern → ActionId`, in the `Runtime`'s
//! keymap); intent finds the HANDLER in force (`.on_action`, retained in
//! the reconciler like the click actions — the innermost wins). The
//! shell only translates and composes: match → dispatch → repaint. A
//! binding with no mounted handler does not consume the key — the screen
//! without the palette types normally.

/// An action's nominal identity — declared as a const by the app:
/// `const SELECT_NEXT: ActionId = ActionId("finder.select_next");`
/// Namespace by convention (`"app.action"`); the string prints and
/// serializes (map debugging today, a configurable keymap tomorrow).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ActionId(pub &'static str);

/// What the keymap makes of one stroke. A chord is why this is not an
/// `Option`: a stroke that STARTS a sequence fires nothing and belongs
/// to nobody else — the keyboard is held until the next one resolves
/// it or lets it go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyMatch {
    /// A binding answered. The caller dispatches it.
    Action(ActionId),
    /// The stroke opened (or continued) a sequence. Nothing fired, and
    /// the stroke is SPENT — the field must not type it, and the app's
    /// which-key can read [`Runtime::pending_chord`] to show what is
    /// still on offer.
    ///
    /// [`Runtime::pending_chord`]: crate::runtime::Runtime::pending_chord
    Pending,
    /// Nothing is bound. The stroke walks on, as it always did.
    None,
}

/// Closes the innermost open popover. Every mounted popover registers
/// a handler for it, and the runtime pre-binds Escape inside the
/// reserved [`OVERLAY_CONTEXT`] — apps never wire this themselves.
pub const OVERLAY_DISMISS: ActionId = ActionId("bunny.popover.dismiss");

/// The key context every open popover declares. The `bunny.` prefix is
/// reserved for the framework's own contexts.
pub const OVERLAY_CONTEXT: &str = "bunny.popover";

/// The prefix the framework keeps for itself. A key context under it
/// belongs to the house: an app never declares one, and emptying the
/// key table leaves them standing.
pub const RESERVED_PREFIX: &str = "bunny.";

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Named keys + the printable case (lowercase, no modifier).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Down,
    Up,
    Left,
    Right,
    Enter,
    Escape,
    Tab,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Home,
    End,
    /// A printable key, named by THE CHARACTER IT TYPES WITH NO
    /// MODIFIER APPLIED, lowercased.
    ///
    /// This is the contract every shell owes, and it is what makes a
    /// chord spellable: `command_shift(Char('\\'))` is "the backslash
    /// key, with shift", not "the pipe character". A shell that reports
    /// the SHIFTED character instead makes every chord on shifted
    /// punctuation unmatchable — the letters survive by accident,
    /// because lowercasing repairs them, and the punctuation does not.
    ///
    /// Each platform has the right question to ask: `ToUnicode` with a
    /// clean keyboard state on Windows, `charactersByApplyingModifiers:0`
    /// on macOS, the keyboard layout map in a browser that has one. All
    /// three read the USER'S OWN LAYOUT, which is why none of them is a
    /// table of US pairs — a table would be wrong on the Brazilian
    /// keyboard this framework is written on.
    Char(char),
}

/// The pattern the keymap matches — EXACT modifiers (Cmd+Enter does not
/// match the Enter binding). `Eq + Hash`: a direct map key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyPattern {
    pub key: Key,
    pub shift: bool,
    pub command: bool,
    pub option: bool,
    pub control: bool,
}

/// One keystroke, as the platform read it: the pattern that NAMES the
/// key, and the character that key TYPES under the modifiers held.
///
/// Two fields because they are two questions, and one answer cannot
/// serve both. The pattern carries the key's OWN character, read
/// through the user's layout, so a chord spelled with a backslash
/// matches the key the user pressed and not the `|` that key would
/// have typed. The typed character is the other half — what a box that
/// REFUSES text still needs, because what a `{` means is the box's
/// business, and which key makes a `{` is the layout's: on ABNT2, US,
/// AZERTY and Dvorak it is a different key, and no table in an app can
/// know which.
///
/// `typed` is `None` when command or control is held. Those are the
/// chord modifiers: a chord types nothing, and every platform asked
/// under them answers with the key's base character, which would be a
/// lie about what reached the screen.
///
/// It is what the key ALONE types. A dead key answers with its mark
/// and not with the letter it will compose — the composed letter is
/// the input system's, and it arrives by the text door.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stroke {
    pub pattern: KeyPattern,
    pub typed: Option<char>,
}

impl From<KeyPattern> for Stroke {
    /// A stroke that types nothing — what a test spells, and what a
    /// shell answers for a key its platform gives no character for.
    fn from(pattern: KeyPattern) -> Stroke {
        Stroke { pattern, typed: None }
    }
}

impl From<&KeyPattern> for Stroke {
    fn from(pattern: &KeyPattern) -> Stroke {
        Stroke::from(*pattern)
    }
}

impl From<&Stroke> for Stroke {
    fn from(stroke: &Stroke) -> Stroke {
        *stroke
    }
}

impl Stroke {
    /// The stroke, and what it typed. A shell may hand over whatever
    /// its platform said: the two things that are never a typed
    /// character are dropped HERE, once, so no shell has to remember
    /// them and no shell can forget them.
    ///
    /// A chord types nothing — and every platform asked under command
    /// or control answers with the key's own character anyway, which
    /// would be a lie about what reached the screen. A control
    /// character is not text: control and the letter A make U+0001,
    /// and a box asking what was typed must not be told that.
    pub fn new(pattern: KeyPattern, typed: Option<char>) -> Stroke {
        let chord = pattern.command || pattern.control;
        Stroke { pattern, typed: typed.filter(|typed| !chord && !typed.is_control()) }
    }

    /// How the same key would be spelled by what it TYPES — the
    /// modifiers that made the character are SPENT, so a binding
    /// written as the character alone matches it.
    ///
    /// `None` when the key typed nothing. Shift and option are spent
    /// here because they are what produced the character; command and
    /// control never get this far, because a chord types nothing.
    pub fn typed_pattern(&self) -> Option<KeyPattern> {
        self.typed.map(|typed| KeyPattern {
            key: Key::Char(typed),
            shift: false,
            command: false,
            option: false,
            control: false,
        })
    }
}

impl KeyPattern {
    /// The bare key.
    pub const fn key(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: false, option: false, control: false }
    }

    /// Cmd + key.
    pub const fn command(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: true, option: false, control: false }
    }

    /// Shift + key.
    pub const fn shift(key: Key) -> KeyPattern {
        KeyPattern { key, shift: true, command: false, option: false, control: false }
    }

    /// Cmd + Shift + key — a product's second shelf of shortcuts.
    pub const fn command_shift(key: Key) -> KeyPattern {
        KeyPattern { key, shift: true, command: true, option: false, control: false }
    }

    /// Control + key.
    pub const fn control(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: false, option: false, control: true }
    }

    /// Control + Shift + key.
    pub const fn control_shift(key: Key) -> KeyPattern {
        KeyPattern { key, shift: true, command: false, option: false, control: true }
    }

    /// Option (Alt) + key.
    pub const fn option(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: false, option: true, control: false }
    }

    /// A bare Char (no cmd/ctrl) is TYPING: with a field focused, the
    /// gate lets it through to the text without consulting the map —
    /// bound or not. (Option counts as typing: option+a composes "å" on
    /// macOS.)
    pub fn is_text_input(&self) -> bool {
        matches!(self.key, Key::Char(_)) && !self.command && !self.control
    }

    /// No accelerator held. Shift does not count — it types, it does
    /// not command. This is the shape a focused field may claim: a bare
    /// Enter is a break, `⌘↵` belongs to the app.
    pub fn is_plain(&self) -> bool {
        !self.command && !self.control && !self.option
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_input_patterns_are_recognized() {
        assert!(KeyPattern::key(Key::Char('a')).is_text_input());
        assert!(!KeyPattern::command(Key::Char('a')).is_text_input());
        assert!(!KeyPattern::key(Key::Down).is_text_input());
        let option_a = KeyPattern { option: true, ..KeyPattern::key(Key::Char('a')) };
        assert!(option_a.is_text_input(), "option composes text on macOS");
    }

    #[test]
    fn a_plain_stroke_holds_no_accelerator() {
        assert!(KeyPattern::key(Key::Enter).is_plain());
        let shifted = KeyPattern { shift: true, ..KeyPattern::key(Key::Enter) };
        assert!(shifted.is_plain(), "shift types, it does not command");
        assert!(!KeyPattern::command(Key::Enter).is_plain(), "the app owns the chord");
    }
}
