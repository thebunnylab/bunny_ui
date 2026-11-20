//! The network through the `curl` command.
//!
//! The standard library has no TLS. Every Mac, every Linux desktop and
//! Windows 10 and later has `curl`, which uses the system's certificates
//! and reads the proxy from `HTTPS_PROXY`. The updater runs it as a
//! child process and reads the status code back with `-w`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use bunny_ui_release::{Transfer, UpdateError};

use crate::fetch::{Fetch, HttpReply, check_url};

/// A [`Fetch`] that runs `curl`.
#[derive(Debug, Clone)]
pub struct CurlFetch {
    program: PathBuf,
}

impl Default for CurlFetch {
    fn default() -> CurlFetch {
        CurlFetch { program: PathBuf::from(if cfg!(windows) { "curl.exe" } else { "curl" }) }
    }
}

impl CurlFetch {
    /// A fetch that runs the `curl` at `program`.
    #[must_use]
    pub fn at(program: impl Into<PathBuf>) -> CurlFetch {
        CurlFetch { program: program.into() }
    }

    fn command(&self, url: &str) -> Result<Command, UpdateError> {
        check_url(url)?;
        let mut command = Command::new(&self.program);
        command
            .args(["--silent", "--show-error", "--location"])
            // a redirect can go to http or https, never to another scheme
            .args(["--proto", "=https,http,file", "--proto-redir", "=https,http"])
            .args(["--connect-timeout", "30", "--retry", "3"])
            .args(["--user-agent", concat!("bunny-ui-updater/", env!("CARGO_PKG_VERSION"))])
            .stdin(Stdio::null());
        Ok(command)
    }

    fn not_run(&self, error: &std::io::Error) -> UpdateError {
        UpdateError::Transport(format!("could not run {}: {error}", self.program.display()))
    }
}

/// The status code `curl` wrote with `-w`. A `file://` URL has no
/// status, and curl writes `000`: a file that was read is a `200`.
fn status_of(text: &str, url: &str) -> Result<u16, UpdateError> {
    let code: u16 = text
        .trim()
        .parse()
        .map_err(|_| UpdateError::Transport(format!("curl gave no status for {url}")))?;
    Ok(if code == 0 && url.get(..7).is_some_and(|head| head.eq_ignore_ascii_case("file://")) { 200 } else { code })
}

fn failed(stderr: &[u8], url: &str) -> UpdateError {
    let why = String::from_utf8_lossy(stderr);
    UpdateError::Transport(format!("could not fetch {url}: {}", why.trim()))
}

impl Fetch for CurlFetch {
    fn get(&self, url: &str) -> Result<HttpReply, UpdateError> {
        let out = self
            .command(url)?
            .args(["--max-time", "120", "--output", "-", "--write-out", "\n%{http_code}", url])
            .output()
            .map_err(|error| self.not_run(&error))?;
        if !out.status.success() {
            return Err(failed(&out.stderr, url));
        }
        // the body, then a line break, then the status: split at the last one
        let at = out.stdout.iter().rposition(|&byte| byte == b'\n').unwrap_or(0);
        let status = status_of(&String::from_utf8_lossy(&out.stdout[at..]), url)?;
        let mut body = out.stdout;
        body.truncate(at);
        Ok(HttpReply { status, body })
    }

    fn download(&self, url: &str, to: &Path, received: &mut dyn FnMut(Transfer)) -> Result<u16, UpdateError> {
        let mut child = self
            .command(url)?
            // a download that moves less than 1 KB/s for a minute is stuck
            .args(["--speed-limit", "1024", "--speed-time", "60"])
            .arg("--output")
            .arg(to)
            .args(["--write-out", "%{http_code}", url])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| self.not_run(&error))?;
        received(Transfer::new(0, None));
        // curl has no callback: the size of the file is what has arrived
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => return Err(UpdateError::Transport(format!("waiting for curl: {error}"))),
            }
            if let Ok(meta) = std::fs::metadata(to) {
                received(Transfer::new(meta.len(), None));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let mut stdout = String::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = pipe.read_to_string(&mut stdout);
        }
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_end(&mut stderr);
        }
        if !status.success() {
            return Err(failed(&stderr, url));
        }
        if let Ok(meta) = std::fs::metadata(to) {
            received(Transfer::new(meta.len(), None));
        }
        status_of(&stdout, url)
    }
}
