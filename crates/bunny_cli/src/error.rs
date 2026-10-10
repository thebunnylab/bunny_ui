//! What a command says when it cannot go on: the problem, the way out,
//! and the exit code a script reads.

use std::fmt;
use std::io;
use std::path::Path;

/// A failure the person can act on. `hint` is the next thing to try —
/// a command to run, a line to add — printed under the message.
#[derive(Debug)]
pub struct Error {
    pub message: String,
    pub hint: Option<String>,
    /// 1 for a failure, 2 for a command line `bunny` could not read.
    pub code: u8,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error { message: message.into(), hint: None, code: 1 }
    }

    /// A command line that could not be read: exit code 2, like every
    /// tool that tells a typo from a failure.
    pub fn usage(message: impl Into<String>) -> Error {
        Error { message: message.into(), hint: None, code: 2 }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Error {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::new(error.to_string())
    }
}

/// An I/O failure named after the file it happened to — "permission
/// denied" alone tells nobody where.
pub fn at(path: &Path) -> impl Fn(io::Error) -> Error + '_ {
    move |error| Error::new(format!("{}: {error}", path.display()))
}

pub type Result<T> = std::result::Result<T, Error>;
