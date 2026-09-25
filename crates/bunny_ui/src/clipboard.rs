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
//! Text only, on the thread the app runs on, like every door a handler
//! is called from.

use std::cell::RefCell;

type Writer = Box<dyn Fn(&str)>;
type Reader = Box<dyn Fn() -> Option<String>>;

thread_local! {
    /// The platform's clipboard, lent by the shell at boot.
    static SYSTEM: RefCell<Option<(Writer, Reader)>> = const { RefCell::new(None) };
    /// The process's own, where no shell lent one.
    static MEMORY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The shell's half: how this platform writes and reads its clipboard's
/// text. Called once, when the app is made; the last one installed
/// answers.
pub fn install(write: impl Fn(&str) + 'static, read: impl Fn() -> Option<String> + 'static) {
    SYSTEM.with(|slot| *slot.borrow_mut() = Some((Box::new(write), Box::new(read))));
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
