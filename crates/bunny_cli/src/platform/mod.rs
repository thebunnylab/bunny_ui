//! Each platform's way to build the app, put it where it runs, and start
//! it — and the running app `bunny run` watches.

pub mod desktop;
pub mod ios;
pub mod web;

use std::fs;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

use crate::error::{self, Error, Result};
use crate::template::{self, Escape};

/// How `bunny run` builds and starts the app.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub release: bool,
    pub features: Vec<String>,
    /// For the app: what followed `--`.
    pub args: Vec<String>,
    /// Start it and let go: no console, nothing to watch.
    pub detach: bool,
}

/// An app `bunny run` started.
pub trait Session {
    /// Waits up to `timeout` for the app to end: `Some(code)` once it
    /// has (`code` is `None` when a signal ended it).
    fn wait(&mut self, timeout: Duration) -> Option<Option<i32>>;
    /// Ends the app.
    fn stop(&mut self);
}

/// A session that is one child process, with what else ends the app.
pub struct ChildSession {
    pub child: Child,
    pub on_stop: Option<Box<dyn FnMut()>>,
}

impl Session for ChildSession {
    fn wait(&mut self, timeout: Duration) -> Option<Option<i32>> {
        let until = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status.code()),
                Ok(None) => {}
                Err(_) => return Some(None),
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop(&mut self) {
        if let Some(stop) = self.on_stop.as_mut() {
            stop();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A plist from the project, its `@BUNNY_…@` markers filled.
pub fn render_plist(path: &Path, values: &[(&str, &str)]) -> Result<String> {
    let text = fs::read_to_string(path).map_err(error::at(path))?;
    template::render(&path.display().to_string(), &text, values, Escape::Xml)
}

/// The string a plist gives `key` — the `<string>` after its `<key>`.
pub fn plist_string(text: &str, key: &str) -> Option<String> {
    let after = &text[text.find(&format!("<key>{key}</key>"))?..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")?;
    Some(after[start..start + end].trim().to_string())
}

/// A fresh folder: what an earlier build left there is gone.
pub fn fresh_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(error::at(dir))?;
    }
    fs::create_dir_all(dir).map_err(error::at(dir))
}

/// Copies the built binary into a bundle, keeping its permissions.
pub fn copy_binary(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).map_err(|error| Error::new(format!("{} → {}: {error}", from.display(), to.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plist_string_is_read_by_its_key() {
        let text = "<dict>\n  <key>CFBundleName</key>\n  <string>Notes</string>\n  <key>MinimumOSVersion</key>\n    <string> 17.0 </string>\n</dict>";
        assert_eq!(plist_string(text, "MinimumOSVersion").as_deref(), Some("17.0"));
        assert_eq!(plist_string(text, "CFBundleName").as_deref(), Some("Notes"));
        assert_eq!(plist_string(text, "Missing"), None);
    }
}
