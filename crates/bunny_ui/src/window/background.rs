//! What a window shows where its scene paints nothing.

/// The background a desktop window asks its system for.
///
/// ```ignore
/// WindowSpec::titled("Trinity").background(WindowBackground::Blurred)
/// ```
///
/// A window is opaque until its spec says otherwise. In a window that is not
/// opaque the alpha of the theme's canvas is real: it is how strongly the
/// canvas lies over what is behind the window. Every translucent paint above
/// it adds to it, unless the view replaces what is under it with
/// `.background_blend(Blend::Replace)`.
///
/// Not every system can serve every ask. A shell reports the background it
/// will serve before a window exists and the one it did serve after the
/// window opens. `docs/window-background.md` holds the platform table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowBackground {
    /// The canvas at full strength. Nothing behind the window shows, and the
    /// alpha of the canvas is ignored.
    #[default]
    Opaque,
    /// The system composites the window by the alpha its scene left. What is
    /// behind the window shows through, sharp.
    Clear,
    /// The same as [`WindowBackground::Clear`], and the system blurs what is
    /// behind the window.
    Blurred,
}

impl WindowBackground {
    /// Resolves this ask against two facts about the system: `alpha`, it
    /// composites a window by the window's own alpha, and `blur`, it can blur
    /// what is behind a window.
    ///
    /// A blur the system cannot give resolves to [`WindowBackground::Opaque`],
    /// never to [`WindowBackground::Clear`]. A scene drawn to stand on a blur
    /// cannot be read over a sharp desktop.
    #[must_use]
    pub const fn resolve(self, alpha: bool, blur: bool) -> WindowBackground {
        match self {
            WindowBackground::Clear if alpha => WindowBackground::Clear,
            WindowBackground::Blurred if alpha && blur => WindowBackground::Blurred,
            _ => WindowBackground::Opaque,
        }
    }

    /// Whether the system composites this window by its own alpha. A
    /// presenter is told this once, when it is installed.
    #[must_use]
    pub const fn is_translucent(self) -> bool {
        !matches!(self, WindowBackground::Opaque)
    }
}

#[cfg(test)]
mod tests {
    use super::WindowBackground;

    #[test]
    fn a_window_is_opaque_until_it_asks_otherwise() {
        assert_eq!(WindowBackground::default(), WindowBackground::Opaque);
        assert!(!WindowBackground::Opaque.is_translucent());
        for (alpha, blur) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(WindowBackground::Opaque.resolve(alpha, blur), WindowBackground::Opaque);
        }
    }

    #[test]
    fn a_clear_window_needs_only_the_alpha() {
        assert_eq!(WindowBackground::Clear.resolve(true, false), WindowBackground::Clear);
        assert_eq!(WindowBackground::Clear.resolve(true, true), WindowBackground::Clear);
        assert_eq!(WindowBackground::Clear.resolve(false, true), WindowBackground::Opaque);
        assert!(WindowBackground::Clear.is_translucent());
    }

    #[test]
    fn a_blur_the_system_cannot_give_is_opaque_and_never_clear() {
        assert_eq!(WindowBackground::Blurred.resolve(true, true), WindowBackground::Blurred);
        assert_eq!(WindowBackground::Blurred.resolve(true, false), WindowBackground::Opaque);
        assert_eq!(WindowBackground::Blurred.resolve(false, true), WindowBackground::Opaque);
        assert_eq!(WindowBackground::Blurred.resolve(false, false), WindowBackground::Opaque);
        assert!(WindowBackground::Blurred.is_translucent());
    }
}
