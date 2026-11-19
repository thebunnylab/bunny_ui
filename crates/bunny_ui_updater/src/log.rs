//! The reason an update failed, kept where someone can read it later.
//!
//! The UI says only "update failed": a transport error on screen asks the
//! user to debug the release pipeline. The detail goes to this file. An
//! app started from the desktop has no terminal, and on macOS its stderr
//! goes to `/dev/null`, so without the file a failure leaves no trace.
//!
//! ## Production gotchas
//!
//! - **The file is bounded.** Past [`LIMIT`] bytes the oldest whole lines
//!   go, so a feed that stays down for months does not grow it forever.
//! - **A log that cannot be written goes to stderr**, the only place left.
//!   The UI still says that the update failed, so the failure is never
//!   silent.

use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bunny_ui_release::{UpdateError, Version};

/// The most the log keeps, in bytes: hundreds of failures.
pub const LIMIT: usize = 64 * 1024;

/// Which half of the update cycle failed, and why.
#[derive(Debug, Clone, Copy)]
pub enum Failure<'a> {
    /// The check: the feed did not answer, or answered wrongly.
    Check(&'a UpdateError),
    /// The download, the verification or the install of `to`.
    Install { to: &'a Version, error: &'a UpdateError },
}

impl fmt::Display for Failure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Check(error) => write!(f, "check failed: {error}"),
            Failure::Install { to, error } => write!(f, "install of {to} failed: {error}"),
        }
    }
}

/// The update log of one app.
#[derive(Debug, Clone)]
pub struct UpdateLog {
    path: PathBuf,
}

impl UpdateLog {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> UpdateLog {
        UpdateLog { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Adds one line: the time in UTC, the version that runs, and what
    /// failed. The line goes to stderr too, for a build run from a
    /// terminal, and only there when the file cannot be written.
    pub fn record(&self, running: &Version, failure: Failure<'_>) {
        let line = line(&utc(SystemTime::now()), running, failure);
        eprintln!("{line}");
        if let Err(error) = self.append(&line) {
            eprintln!("the update log {} cannot be written: {error}", self.path.display());
        }
    }

    fn append(&self, line: &str) -> io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(file, "{line}")?;
        drop(file);
        if std::fs::metadata(&self.path)?.len() > LIMIT as u64 {
            let text = std::fs::read_to_string(&self.path)?;
            let kept = newest_lines(&text, LIMIT);
            let partial = self.path.with_extension("log.part");
            std::fs::write(&partial, kept)?;
            std::fs::rename(&partial, &self.path)?;
        }
        Ok(())
    }
}

pub(crate) fn line(time: &str, running: &Version, failure: Failure<'_>) -> String {
    // a line break inside an error would split the entry
    let text = failure.to_string().replace(['\r', '\n'], " ");
    format!("{time} {running} {text}")
}

/// The end of `text` that fits in `limit` bytes, from the start of a line.
pub(crate) fn newest_lines(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut cut = text.len() - limit;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    // a cut at the start of a line keeps that line
    if text.as_bytes()[cut - 1] == b'\n' {
        return &text[cut..];
    }
    match text[cut..].find('\n') {
        Some(at) => &text[cut + at + 1..],
        None => "",
    }
}

/// `time` as RFC 3339 in UTC, to the second: `2026-11-19T20:13:24Z`.
pub(crate) fn utc(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest / 60 % 60, rest % 60)
}

/// The proleptic Gregorian date of a count of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
