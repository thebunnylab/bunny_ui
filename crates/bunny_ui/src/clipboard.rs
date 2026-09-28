//! The clipboard, as the app's own handlers reach it.
//!
//! A focused field copies and pastes by itself: the shell asks it, and
//! the text goes out and comes in through the platform. What had no door
//! at all was the APP writing the clipboard — a row's "Copy Path", a
//! terminal's copy menu, the copy button over a response. They are all a
//! click handler holding a string, and a handler has no shell to ask.
//!
//! ```ignore
//! menu_item("Copy Path", move || bunny_ui::clipboard::write(&path))
//! ```
//!
//! Every shell lends its platform's clipboard here when the app is made
//! ([`install`]), so the same line writes the general pasteboard on the
//! mac, the Windows clipboard, the Wayland or X11 selection, the phones'
//! pasteboards — and, in a browser, `navigator.clipboard.writeText`
//! inside the gesture that clicked, the one moment a page may write it.
//! Where nobody lent one — a headless probe, a test — the process keeps
//! its own, so a copy followed by a paste still agrees with itself.
//!
//! Text, and a picture where the shell can read one — the mac and Windows —
//! on the thread the app runs on, like every door a handler is called
//! from. A paste reaches the input that holds the keyboard through
//! [`crate::runtime::Runtime::paste`]: the picture first, for a field or
//! a box that takes one, the text after.

use std::cell::RefCell;

type Writer = Box<dyn Fn(&str)>;
type Reader = Box<dyn Fn() -> Option<String>>;
type ImageReader = Box<dyn Fn() -> Option<ClipboardImage>>;

/// A picture on the clipboard: the bytes of one of its encodings, and
/// what they are (`image/png`, `image/tiff`). A screenshot copied to
/// the clipboard, an image copied from a page — what a composer turns
/// into an attachment instead of pasting as nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardImage {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

thread_local! {
    /// The platform's clipboard, lent by the shell at boot.
    static SYSTEM: RefCell<Option<(Writer, Reader)>> = const { RefCell::new(None) };
    /// The platform's pictures, lent by a shell that can read them.
    static IMAGES: RefCell<Option<ImageReader>> = const { RefCell::new(None) };
    /// The process's own, where no shell lent one.
    static MEMORY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Which picture a paste reads off a clipboard that holds one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// PNG, as the clipboard holds it.
    Png,
    /// The platform's own bitmap — a DIB on Windows, a TIFF on the Mac —
    /// which the shell re-encodes as PNG, the one of the two a receiver
    /// takes.
    Native,
}

/// The rule every desktop shell reads its clipboard's picture by.
///
/// **A copy that carries text is a text copy.** Office suites put a
/// rendering of every selection beside its text — Word, Excel and Outlook
/// on Windows, and Office and iWork on the Mac — and so does a browser
/// selection that spans an image. Reading the picture there would turn a
/// paragraph pasted into a composer into an attachment of its rendering,
/// with the paragraph itself gone. Otherwise PNG before the platform's
/// own bitmap: a screenshot and an image copied from a page carry both.
pub fn picture_to_read(has_text: bool, has_png: bool, has_native: bool) -> Option<Picture> {
    match (has_text, has_png, has_native) {
        (true, ..) | (false, false, false) => None,
        (false, true, _) => Some(Picture::Png),
        (false, false, true) => Some(Picture::Native),
    }
}

/// The shell's half: how this platform writes and reads its clipboard's
/// text. Called once, when the app is made; the last one installed
/// answers.
pub fn install(write: impl Fn(&str) + 'static, read: impl Fn() -> Option<String> + 'static) {
    SYSTEM.with(|slot| *slot.borrow_mut() = Some((Box::new(write), Box::new(read))));
}

/// The shell's half for pictures: how this platform reads an image off
/// its clipboard. A shell that cannot lends nothing, and a read answers
/// `None` there.
pub fn install_image_reader(read: impl Fn() -> Option<ClipboardImage> + 'static) {
    IMAGES.with(|slot| *slot.borrow_mut() = Some(Box::new(read)));
}

/// The picture the clipboard holds, when it holds one, the shell can read
/// it, and [`picture_to_read`] says a paste should — the mac (PNG, then a
/// TIFF re-encoded as PNG) and Windows (PNG, then a DIB re-encoded as
/// PNG), both never beside text. The others answer `None` until their
/// shells learn to.
pub fn read_image() -> Option<ClipboardImage> {
    IMAGES.with(|slot| slot.borrow().as_ref().and_then(|read| read()))
}

/// Puts `text` on the clipboard — the system's, where a shell lent one.
pub fn write(text: &str) {
    let lent = SYSTEM.with(|slot| {
        slot.borrow().as_ref().map(|(write, _)| write(text)).is_some()
    });
    if !lent {
        MEMORY.with(|slot| *slot.borrow_mut() = Some(text.to_string()));
    }
}

/// What the clipboard holds as text, or `None` when it is empty or holds
/// something else.
///
/// A browser grants a page no reading on demand — the clipboard reaches
/// a page only inside a paste, and there it arrives as text through the
/// field's own road — so in a browser this answers `None`.
pub fn read() -> Option<String> {
    let lent = SYSTEM.with(|slot| slot.borrow().as_ref().map(|(_, read)| read()));
    match lent {
        Some(text) => text,
        None => MEMORY.with(|slot| slot.borrow().clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn a_copy_that_carries_text_pastes_as_text() {
        // text beside a picture — an office suite's selection — is a text copy
        assert_eq!(picture_to_read(true, true, true), None);
        assert_eq!(picture_to_read(true, false, true), None);
        assert_eq!(picture_to_read(true, true, false), None);
        // a screenshot or an image from a page: PNG first, the platform's after
        assert_eq!(picture_to_read(false, true, true), Some(Picture::Png));
        assert_eq!(picture_to_read(false, true, false), Some(Picture::Png));
        assert_eq!(picture_to_read(false, false, true), Some(Picture::Native));
        assert_eq!(picture_to_read(false, false, false), None);
    }

    #[test]
    fn a_process_with_no_shell_keeps_its_own() {
        write("src/main.rs");
        assert_eq!(read().as_deref(), Some("src/main.rs"), "a copy and a paste agree");
    }

    #[test]
    fn a_lent_clipboard_takes_every_write_and_answers_every_read() {
        let system: Rc<RefCell<Option<String>>> = Rc::default();
        {
            let (to, from) = (Rc::clone(&system), Rc::clone(&system));
            install(
                move |text| *to.borrow_mut() = Some(format!("system: {text}")),
                move || from.borrow().clone(),
            );
        }
        write("apps/trinity");
        assert_eq!(system.borrow().as_deref(), Some("system: apps/trinity"));
        assert_eq!(read().as_deref(), Some("system: apps/trinity"));
        MEMORY.with(|slot| assert!(slot.borrow().is_none(), "the memory is not the clipboard"));
    }
}
