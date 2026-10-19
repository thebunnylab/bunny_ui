//! The network, through `curl` — on every Mac, every Linux desktop and
//! Windows 10 and later — since the standard library speaks no TLS.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::process;

fn curl() -> Result<String> {
    let name = if cfg!(windows) { "curl.exe" } else { "curl" };
    process::which(name)
        .map(|path| path.to_string_lossy().into_owned())
        .ok_or_else(|| Error::new("no curl: `bunny` downloads with it").hint("install curl from your system's packages"))
}

/// A small document (a package list, a release index), as text.
pub fn fetch(url: &str) -> Result<String> {
    let out = process::run(curl()?, &["-fsSL", "--retry", "3", "--max-time", "60", url], Duration::from_secs(120))
        .map_err(|error| Error::new(format!("curl: {error}")))?;
    if !out.ok() {
        return Err(Error::new(format!("could not fetch {url}: {}", out.stderr.trim()))
            .hint("check the connection, or a proxy (curl reads HTTPS_PROXY)"));
    }
    Ok(out.stdout)
}

/// A file, to `dest`, with curl's progress bar on this terminal.
pub fn download(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(crate::error::at(parent))?;
    }
    let partial = dest.with_extension("part");
    let status = Command::new(curl()?)
        .args(["-fL", "--retry", "3", "--progress-bar", "-o"])
        .arg(&partial)
        .arg(url)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| Error::new(format!("curl: {error}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::new(format!("the download of {url} failed")));
    }
    std::fs::rename(&partial, dest).map_err(crate::error::at(dest))
}
