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

/// Closes the innermost open popover. Every mounted popover registers
/// a handler for it, and the runtime pre-binds Escape inside the
/// reserved [`OVERLAY_CONTEXT`] — apps never wire this themselves.
pub const OVERLAY_DISMISS: ActionId = ActionId("bunny.popover.dismiss");

/// The key context every open popover declares. The `bunny.` prefix is
/// reserved for the framework's own contexts.
pub const OVERLAY_CONTEXT: &str = "bunny.popover";

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

    /// A bare Char (no cmd/ctrl) is TYPING: with a field focused, the
    /// gate lets it through to the text without consulting the map —
    /// bound or not. (Option counts as typing: option+a composes "å" on
    /// macOS.)
    pub fn is_text_input(&self) -> bool {
        matches!(self.key, Key::Char(_)) && !self.command && !self.control
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
}
