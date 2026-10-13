//! Preview entries — how a build exports the views its `#[preview]`
//! functions build, and how the app that runs it finds them.
//!
//! A build with the facade's `preview` feature exports, per marked
//! function, a symbol named [`SYMBOL_PREFIX`] + the function's name that
//! answers a [`PreviewEntry`]: the preview's [`Meta`] (where it was
//! written and what it is called) and a function that builds the view
//! with its traits ([`Output`]). The entry is read the way a generation's
//! [`Entry`](super::Entry) is: the [`PREVIEW_ABI`] first, then the anchor
//! that proves the build runs this app's one copy of the framework.
//!
//! [`preview_entry`] looks a preview up by name in the generation that
//! runs — or, before one is loaded, in the program itself. The symbols a
//! program exports are the platform's business: a Mach-O executable keeps
//! its globals, a Linux one needs `-rdynamic`; the hot lane never depends
//! on it, since the preview host starts on a generation.
//!
//! The scan that lists previews reads the source, not the symbols: a
//! symbol table is the host's door to a preview it already knows by name.

use std::ffi::CString;
use std::fmt;

use bunny_ui::preview::{IntoPreview, Traits};

use super::{Root, anchor};

/// The shape of [`PreviewEntry`]. A host refuses an entry of another
/// shape and reads none of it but this number.
pub const PREVIEW_ABI: u32 = 1;

/// The reserved whole-app entry exported by the facade's `app!`.
pub const APP_IDENT: &str = "app";

/// What every preview's symbol starts with; the function's name follows.
pub const SYMBOL_PREFIX: &str = "bunny_preview__";

/// The symbol that answers the preview `ident` names.
pub fn symbol(ident: &str) -> String {
    format!("{SYMBOL_PREFIX}{ident}")
}

/// What a build gives the app for one preview.
#[repr(C)]
pub struct PreviewEntry {
    /// [`PREVIEW_ABI`], as the build knew it. First, like a generation's.
    pub abi: u32,
    /// [`anchor`], as the build calls it.
    pub anchor: fn() -> usize,
    /// What the preview is and where it was written.
    pub meta: Meta,
    /// Builds the view with its traits — inside a pass, so the state it
    /// makes anchors under the boundary that called it.
    pub make: fn() -> Output,
}

impl PreviewEntry {
    /// The entry of a preview `make` builds, described by `meta`.
    pub const fn new(meta: Meta, make: fn() -> Output) -> PreviewEntry {
        PreviewEntry { abi: PREVIEW_ABI, anchor, meta, make }
    }
}

impl fmt::Debug for PreviewEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewEntry").field("abi", &self.abi).field("meta", &self.meta).finish_non_exhaustive()
    }
}

/// What a preview is called and where it was written — the attribute's
/// arguments and the compiler's own answers.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    /// The function's name.
    pub ident: &'static str,
    /// The component the preview belongs to (`name = "…"`; the ident otherwise).
    pub name: &'static str,
    /// The gallery section (`scope = "…"`; empty otherwise).
    pub scope: &'static str,
    /// The preview's label on the component's card (`example = "…"`; the ident otherwise).
    pub example: &'static str,
    /// The module path the function was written in.
    pub module: &'static str,
    /// The file, as the compiler names it.
    pub file: &'static str,
    /// The line of the attribute.
    pub line: u32,
}

/// What a preview builds: the view, erased, and the traits it asked for.
pub struct Output {
    /// The view.
    pub root: Root,
    /// Its traits.
    pub traits: Traits,
}

impl Output {
    /// The output of whatever a `#[preview]` function answered — a bare
    /// view or a `Preview` with traits.
    pub fn of(value: impl IntoPreview) -> Output {
        let preview = value.into_preview();
        Output { root: Root::new(preview.view), traits: preview.traits }
    }
}

/// Why a preview could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewError {
    /// No symbol of that name in the code that runs.
    NoSuchPreview {
        /// The preview asked for.
        ident: String,
        /// The symbol looked up.
        symbol: String,
    },
    /// The entry was built for another shape of [`PreviewEntry`].
    Abi {
        /// The preview.
        ident: String,
        /// The shape the build knew.
        found: u32,
    },
    /// The build carries a framework of its own: its state would live
    /// apart from the app's.
    Anchor {
        /// The preview.
        ident: String,
    },
    /// The platform does not resolve symbols, or the lookup failed.
    Unavailable(String),
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreviewError::NoSuchPreview { ident, symbol } => {
                write!(f, "no preview `{ident}` in the code that runs (no `{symbol}`): is the build a `preview` one?")
            }
            PreviewError::Abi { ident, found } => {
                write!(f, "`{ident}` was built for another version of bunny-ui (its entry is {found}, the app's {PREVIEW_ABI})")
            }
            PreviewError::Anchor { ident } => {
                write!(f, "`{ident}` carries a copy of the framework of its own: its state would live apart from the app's")
            }
            PreviewError::Unavailable(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for PreviewError {}

/// The entry of the preview `ident` names, in the generation that runs
/// — or in the program itself before one is loaded — checked against
/// this app the way a generation is.
pub fn preview_entry(ident: &str) -> Result<&'static PreviewEntry, PreviewError> {
    let symbol_name = symbol(ident);
    let name = CString::new(symbol_name.as_str())
        .map_err(|_| PreviewError::Unavailable(format!("`{ident}` is not a symbol name")))?;
    let handle = super::current_handle().unwrap_or_else(super::native::Handle::program);
    let Some(pointer) = handle.symbol(&name) else {
        return Err(PreviewError::NoSuchPreview { ident: ident.to_string(), symbol: symbol_name });
    };
    // SAFETY: the symbol is the function `bunny_ui::__bunny_preview_entry!`
    // writes, `fn() -> &'static PreviewEntry` in the Rust ABI of the compiler
    // that built the code that runs — the generation links this app's
    // framework, and the program is this program. The library is never
    // closed, so the entry lives as long as the app.
    let entry = unsafe { std::mem::transmute::<*mut std::ffi::c_void, fn() -> &'static PreviewEntry>(pointer) };
    let entry = entry();
    if entry.abi != PREVIEW_ABI {
        return Err(PreviewError::Abi { ident: ident.to_string(), found: entry.abi });
    }
    if (entry.anchor)() != anchor() {
        return Err(PreviewError::Anchor { ident: ident.to_string() });
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_symbol_is_the_prefix_and_the_ident() {
        assert_eq!(symbol("home_preview"), "bunny_preview__home_preview");
    }

    #[test]
    fn a_missing_preview_is_named_in_the_refusal() {
        let why = preview_entry("nothing_of_the_kind").unwrap_err();
        match &why {
            PreviewError::NoSuchPreview { ident, symbol } => {
                assert_eq!(ident, "nothing_of_the_kind");
                assert_eq!(symbol, "bunny_preview__nothing_of_the_kind");
            }
            PreviewError::Unavailable(_) => {}
            other => panic!("{other}"),
        }
        assert!(why.to_string().contains("nothing_of_the_kind"));
    }
}
