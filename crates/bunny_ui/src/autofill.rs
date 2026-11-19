//! The passwords the system keeps for the app, asked for by the app.
//!
//! A sign-in window can offer the account the platform's password
//! manager holds for it before anything is typed: the person picks it,
//! the system authorizes, and the window signs in. [`saved_password`] is
//! the app's door to that sheet. A shell that has one installs how it
//! is asked ([`install_requester`]), the way it installs how a
//! notification is shown, and every other shell answers
//! [`SavedPasswordError::Unsupported`] — said, never a quiet nothing.
//!
//! The boxes of the window are the other half, and a passive one: a
//! field that says what it holds (`TextField::autocomplete`) lets the
//! system's own AutoFill fill it in place, from its suggestions or from
//! the Edit menu, and offer the code to the one-time-code box. Neither
//! half needs the other.
//!
//! On Apple platforms the sheet offers the passwords saved for the
//! domains the bundle associates with — a `webcredentials:` entry in its
//! entitlements, confirmed by the domain's `apple-app-site-association`.
//! A bundle with no such domain is refused by the system, by name
//! ([`SavedPasswordError::Failed`]).
//!
//! ```ignore
//! form.task(async move || match autofill::saved_password().await {
//!     Ok(saved) => sign_in(saved.user(), saved.password()),
//!     // nothing saved, a cancel, a platform without the service: the
//!     // form is already there to be typed into
//!     Err(_) => {}
//! })
//! ```

use std::cell::RefCell;
use std::fmt;

// =============================================================================
// The credential
// =============================================================================

/// An account the system handed over: the name it signs in with and its
/// password.
///
/// The password never prints — `Debug` writes a placeholder — and its
/// bytes are overwritten when the value drops, so a credential the app
/// let go of does not stay readable in freed memory. An app that keeps
/// the password (in a binding, to submit it) takes over that duty.
pub struct SavedPassword {
    user: String,
    password: String,
}

impl SavedPassword {
    /// A credential from its two halves — what a shell builds from the
    /// system's answer.
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self { user: user.into(), password: password.into() }
    }

    /// The name the account signs in with — an address, most often.
    #[must_use]
    pub fn user(&self) -> &str {
        &self.user
    }

    /// The account's password.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for SavedPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedPassword")
            .field("user", &self.user)
            .field("password", &"<hidden>")
            .finish()
    }
}

impl Drop for SavedPassword {
    fn drop(&mut self) {
        // the String's own buffer, overwritten in place before it is
        // freed; `black_box` keeps the optimizer from deciding a write
        // to memory about to be released is dead
        let mut bytes = std::mem::take(&mut self.password).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
    }
}

// =============================================================================
// How a request ends without one
// =============================================================================

/// Every way a request ends without a credential. Each one leaves the
/// form as it was: the person can still type.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SavedPasswordError {
    /// No shell on this thread offers the service: a platform without
    /// it (Windows, Linux, Android, the browser — whose own password
    /// manager fills the page's inputs by itself), or no shell running.
    Unsupported,
    /// The system closed the request without a credential: the person
    /// dismissed the sheet, or nothing was saved for the app. Apple
    /// reports both as a cancel when the request asks only for what the
    /// device already holds, so the two cannot be told apart — and the
    /// answer to both is the same form.
    Declined,
    /// The system refused the request: a bundle associated with no
    /// domain, an answer it could not read, a shell that let the request
    /// go unanswered. `code` is the platform's (on Apple platforms an
    /// `ASAuthorizationError`, `0` when the shell itself gave up) and
    /// `reason` its own words.
    Failed {
        /// The platform's error code.
        code: i64,
        /// The platform's description of the refusal.
        reason: String,
    },
}

impl fmt::Display for SavedPasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("this platform offers no saved passwords to the app"),
            Self::Declined => f.write_str("no saved password was chosen"),
            Self::Failed { code, reason } => {
                write!(f, "the system refused the saved-password request ({code}): {reason}")
            }
        }
    }
}

impl std::error::Error for SavedPasswordError {}

// =============================================================================
// The shell's door
// =============================================================================

/// Where a shell hands the system's answer to the request it was given —
/// once. Dropping it unanswered ends the request as
/// [`SavedPasswordError::Failed`] with code `0`, so a waiting window
/// never waits forever.
pub struct Answer {
    sender: motor::task::Sender<Result<SavedPassword, SavedPasswordError>>,
}

impl Answer {
    /// Ends the request with the system's answer. A request whose task
    /// is gone — its view left the scene — hears nothing, and the
    /// credential drops (and is wiped) here.
    pub fn give(self, outcome: Result<SavedPassword, SavedPasswordError>) {
        // a reader that left is the only refusal, and it wants nothing
        let _ = self.sender.send(outcome);
    }
}

type Requester = Box<dyn Fn(Answer)>;

thread_local! {
    /// The shell's own way to ask the system — installed at boot, on the
    /// thread the app runs on.
    static REQUESTER: RefCell<Option<Requester>> = const { RefCell::new(None) };
}

/// The shell installs how the system is asked for a saved password on
/// this platform. The requester starts the system's sheet and keeps the
/// [`Answer`] until the system replies, on this thread.
pub fn install_requester(requester: impl Fn(Answer) + 'static) {
    REQUESTER.with(|slot| *slot.borrow_mut() = Some(Box::new(requester)));
}

// =============================================================================
// The app's door
// =============================================================================

/// Asks the system for a password it keeps for the app, and waits for
/// the person's choice — from a `.task`, which a window that closes
/// cancels with it.
///
/// The system is asked on the first poll, not when the future is made.
/// A shell without the service answers [`SavedPasswordError::Unsupported`]
/// at once. The Apple shells show nothing at all when the device holds
/// no password for the app, so a sign-in window can ask every time it
/// appears without ever showing an empty sheet.
pub async fn saved_password() -> Result<SavedPassword, SavedPasswordError> {
    let (sender, receiver) = motor::task::channel();
    let asked = REQUESTER.with(|slot| {
        slot.borrow().as_ref().map(|requester| requester(Answer { sender }))
    });
    if asked.is_none() {
        return Err(SavedPasswordError::Unsupported);
    }
    receiver.recv().await.unwrap_or_else(|| {
        Err(SavedPasswordError::Failed {
            code: 0,
            reason: String::from("the shell dropped the request without an answer"),
        })
    })
}

#[cfg(test)]
#[path = "autofill_tests.rs"]
mod tests;
