//! The network door: the one thing the updater asks of a transport.

use std::path::Path;

use bunny_ui_release::{Transfer, UpdateError};

/// The answer to one `GET`: the status code and the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpReply {
    #[must_use]
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> HttpReply {
        HttpReply { status, body: body.into() }
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// An HTTP `GET`, the only network operation of the updater.
///
/// The calls block: the updater runs on a thread of its own, never on
/// the UI thread. [`crate::CurlFetch`] is the transport of the `curl`
/// feature. An app with a client of its own implements this trait.
pub trait Fetch: Send + Sync {
    /// A small document: a manifest or a check.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Transport`] when no answer came. A status that is
    /// not a success is an answer, not an error.
    fn get(&self, url: &str) -> Result<HttpReply, UpdateError>;

    /// An artifact, written to the file `to`, with the status code back.
    /// `received` hears how many bytes have arrived while they arrive.
    ///
    /// The default gets the body whole and writes it.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Transport`] when no answer came, and
    /// [`UpdateError::Apply`] when the file cannot be written.
    fn download(&self, url: &str, to: &Path, received: &mut dyn FnMut(Transfer)) -> Result<u16, UpdateError> {
        let reply = self.get(url)?;
        std::fs::write(to, &reply.body).map_err(|error| UpdateError::Apply(format!("writing the download: {error}")))?;
        let size = u64::try_from(reply.body.len()).unwrap_or(u64::MAX);
        received(Transfer::new(size, Some(size)));
        Ok(reply.status)
    }
}

/// Refuses a URL that is not `https`, `http` or `file`.
///
/// A URL comes from a feed. A URL with no scheme, or one that starts
/// with `-`, must never reach a tool as a command-line option. A
/// [`Fetch`] that runs a command calls this first.
///
/// # Errors
///
/// [`UpdateError::Transport`] that names the URL.
pub fn check_url(url: &str) -> Result<(), UpdateError> {
    let allowed = ["https://", "http://", "file://"];
    let follows = |scheme: &&str| {
        url.len() > scheme.len() && url.get(..scheme.len()).is_some_and(|head| head.eq_ignore_ascii_case(scheme))
    };
    if allowed.iter().any(follows) {
        Ok(())
    } else {
        Err(UpdateError::Transport(format!("the URL {url:?} is not https, http or file")))
    }
}
