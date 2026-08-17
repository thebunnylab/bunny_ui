//! The theme — semantic tokens resolved at RUNTIME.
//!
//! The vocabulary is a flat struct of named tokens (discoverable, typed)
//! with one global accessor per token: `theme::accent()` at the call
//! site, no context at all — that is how hundreds of reads per frame
//! stay ergonomic. The store is a thread-local `Cell` (the world is
//! single-thread by design): swapping the theme is an `install(...)` —
//! the global VERSION bumps and the next pass rebuilds the retention
//! (tokens read in body get recorded into the retained scene; the
//! rebuild is a price paid ONCE per swap, not per frame).
//!
//! Reading rule: BUILT-IN chrome (Button, Field, scrollbar) reads the
//! token at PLACEMENT (place) — a retheme repaints without re-running
//! body; the app reads wherever it wants
//! (`.foreground_color(theme::accent())` in body is the common case,
//! and the version covers the invalidation).

use std::cell::Cell;

use crate::layout::Color;

macro_rules! theme_tokens {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {
        /// A theme's token set — flat, `Copy`, open to grow along with
        /// the chrome.
        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct Theme {
            $($(#[$doc])* pub $name: Color,)+
        }

        $(
            $(#[$doc])*
            pub fn $name() -> Color {
                THEME.with(|theme| theme.get().$name)
            }
        )+
    };
}

theme_tokens! {
    /// The window's floor.
    canvas,
    /// Raised surface (panels, cards).
    panel,
    /// Primary text.
    fg,
    /// Secondary text (metadata, paths).
    fg_secondary,
    /// Faint text (badges, empty states).
    fg_faint,
    /// Field placeholder text.
    placeholder,
    /// The brand color — focus, links, match highlights.
    accent,
    /// Surface border.
    border,
    /// Internal divider line.
    divider,
    /// Row background under the pointer.
    row_hover,
    /// Pressed/active row background.
    row_pressed,
    /// Text selection veil.
    selection,
    /// Focused field border.
    focus,
    /// The caret.
    caret,
    /// Control background (button).
    control,
    /// Control background under hover.
    control_hovered,
    /// Pressed control background.
    control_pressed,
    /// The text field well.
    field,
    /// Field border at rest.
    field_border,
    /// The scrollbar thumb.
    scrollbar,
    /// The veil behind overlays.
    backdrop,
}

impl Theme {
    /// The light one-pencil theme — the values the framework always used
    /// (the test defaults depend on this equality).
    pub const fn light() -> Theme {
        Theme {
            canvas: Color::hex(0xF2F3F7),
            panel: Color::WHITE,
            // the house's exact BLACK: the default text ink IS this token
            // (the headless goldens count on the equality)
            fg: Color::BLACK,
            fg_secondary: Color::hex(0x8A94A6),
            fg_faint: Color::hex(0xB3BAC7),
            placeholder: Color::hex(0x9AA2B1),
            accent: Color::hex(0x3B82F6),
            border: Color::hex_a(0x64748B55),
            divider: Color::hex_a(0x64748B2E),
            row_hover: Color::hex_a(0x3B82F617),
            row_pressed: Color::hex_a(0x3B82F62E),
            selection: Color::hex_a(0x3B82F640),
            focus: Color::hex(0x3B82F6),
            caret: Color::BLACK,
            control: Color::hex(0xDDE1E9),
            control_hovered: Color::hex(0xE7EAF1),
            control_pressed: Color::hex(0xC7CCD8),
            field: Color::WHITE,
            field_border: Color::OUTLINE,
            scrollbar: Color::rgba(0, 0, 0, 90),
            backdrop: Color::hex_a(0x0F172A55),
        }
    }

    /// The dark side of the same pencil.
    pub const fn dark() -> Theme {
        Theme {
            canvas: Color::hex(0x101014),
            panel: Color::hex(0x18181D),
            fg: Color::hex(0xEDEDF2),
            fg_secondary: Color::hex(0x9B9BA8),
            fg_faint: Color::hex(0x55555F),
            placeholder: Color::hex(0x6A6A76),
            accent: Color::hex(0x4C8DFF),
            border: Color::hex_a(0xFFFFFF14),
            divider: Color::hex_a(0xFFFFFF0D),
            row_hover: Color::hex_a(0x4C8DFF17),
            row_pressed: Color::hex_a(0x4C8DFF2E),
            selection: Color::hex_a(0x4C8DFF40),
            focus: Color::hex(0x4C8DFF),
            caret: Color::hex(0xEDEDF2),
            control: Color::hex(0x26262E),
            control_hovered: Color::hex(0x2F2F38),
            control_pressed: Color::hex(0x1E1E24),
            field: Color::hex(0x1E1E24),
            field_border: Color::hex_a(0xFFFFFF1A),
            scrollbar: Color::hex_a(0xFFFFFF2E),
            backdrop: Color::hex_a(0x00000066),
        }
    }
}

thread_local! {
    static THEME: Cell<Theme> = const { Cell::new(Theme::light()) };
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// Swaps the WHOLE theme between frames. The global version bumps — the
/// next pass of any `Runtime` rebuilds the retention once.
pub fn install(theme: Theme) {
    THEME.with(|current| current.set(theme));
    VERSION.with(|version| version.set(version.get() + 1));
}

/// The current snapshot — for hot loops that read many tokens.
pub fn current() -> Theme {
    THEME.with(|theme| theme.get())
}

/// The installed theme's version — whoever retains derived output compares it.
pub fn version() -> u64 {
    VERSION.with(|version| version.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_swaps_tokens_and_bumps_the_version() {
        let before = version();
        install(Theme::dark());
        assert_eq!(accent(), Theme::dark().accent);
        assert_eq!(version(), before + 1);
        install(Theme::light());
        assert_eq!(accent(), Theme::light().accent);
        assert_eq!(version(), before + 2);
    }
}
