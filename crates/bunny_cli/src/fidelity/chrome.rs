//! The browser: found, started headless with every flag that makes its
//! pixels repeatable, and spoken to over its debugging socket.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::cdp::{self, Session};
use crate::error::{Error, Result};
use crate::json::Value;

/// The flags every capture runs under.
///
/// The rasterizer is Skia on the CPU, the colors are sRGB untouched, the
/// text is grayscale and unhinted, and no scrollbar takes room — the
/// same picture on every run. `SwiftShader` stands in for the GPU the
/// `WebGL` page needs: since Chrome 137 a context on it is refused
/// without the last flag.
pub const FLAGS: &[&str] = &[
    "--headless=new",
    "--disable-gpu",
    "--disable-gpu-rasterization",
    "--disable-gpu-compositing",
    "--force-color-profile=srgb",
    "--disable-lcd-text",
    "--font-render-hinting=none",
    "--hide-scrollbars",
    "--enable-unsafe-swiftshader",
];

/// What keeps a headless browser quiet: no first run, no network of its
/// own, no throttling of a page it thinks is in the background.
const QUIET: &[&str] = &[
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-component-update",
    "--disable-background-networking",
    "--disable-default-apps",
    "--disable-sync",
    "--mute-audio",
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
    "--remote-debugging-port=0",
];

/// The browser binary.
///
/// The one named, `BUNNY_CHROME`, a Chrome for Testing kept under a
/// checkout's `gitignore/chrome` (where `@puppeteer/browsers` installs
/// it) — this one's or, from a worktree, the main checkout's — or one the
/// system has.
///
/// # Errors
///
/// A named path that is no file, or no browser anywhere.
pub fn find(named: Option<&str>, checkouts: &[PathBuf]) -> Result<PathBuf> {
    if let Some(path) = named.map(PathBuf::from).or_else(|| std::env::var_os("BUNNY_CHROME").map(PathBuf::from)) {
        return if path.is_file() { Ok(path) } else { Err(Error::new(format!("no browser at {}", path.display()))) };
    }
    let mut found = Vec::new();
    for checkout in checkouts {
        collect_testing_builds(&checkout.join("gitignore/chrome"), 0, &mut found);
    }
    found.sort();
    if let Some(newest) = found.pop() {
        return Ok(newest);
    }
    let system = [
        "/Applications/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    if let Some(path) = system.iter().map(PathBuf::from).find(|path| path.is_file()) {
        return Ok(path);
    }
    ["chrome", "google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]
        .iter()
        .find_map(|name| crate::process::which(name))
        .ok_or_else(|| {
            Error::new("no Chrome to capture with").hint(
                "install the version the baseline names: npx @puppeteer/browsers install chrome@150.0.7871.24 --path gitignore/chrome, or pass --chrome PATH",
            )
        })
}

/// Chrome for Testing binaries under `dir`, a few levels deep.
fn collect_testing_builds(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "Google Chrome for Testing" || (name == "chrome" && path.is_file()) {
            if crate::process::is_executable(&path) {
                found.push(path);
            }
        } else if path.is_dir() && depth < 6 {
            collect_testing_builds(&path, depth + 1, found);
        }
    }
}

/// A running browser. Dropped, it is killed and its profile removed.
pub struct Chrome {
    child: Child,
    profile: PathBuf,
    browser: Session,
    /// `host:port` of the debugging server.
    host: String,
    /// `Chrome/150.0.7871.24`.
    pub version: String,
}

impl Chrome {
    /// Starts `binary` headless with a fresh profile at `profile`.
    ///
    /// # Errors
    ///
    /// The binary would not start, or never opened its debugging socket.
    pub fn launch(binary: &Path, profile: PathBuf) -> Result<Self> {
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile).map_err(crate::error::at(&profile))?;
        let mut child = Command::new(binary)
            .args(FLAGS)
            .args(QUIET)
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Error::new(format!("{}: {error}", binary.display())))?;
        let stderr = child.stderr.take().ok_or_else(|| Error::new("the browser's stderr"))?;
        let (sender, receiver) = mpsc::channel();
        // the browser writes as long as it lives: the pipe is drained to
        // the end, or a full pipe would stall it
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(std::result::Result::ok) {
                if let Some(url) = line.split("DevTools listening on ").nth(1) {
                    let _ = sender.send(url.trim().to_owned());
                }
            }
        });
        let Ok(url) = receiver.recv_timeout(Duration::from_secs(30)) else {
            let _ = child.kill();
            return Err(Error::new(format!("{} did not open its debugging socket", binary.display())));
        };
        let host = url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| Error::new(format!("an odd debugging address: {url}")))?
            .to_owned();
        let mut browser = Session::open(&url)?;
        let version = browser.call("Browser.getVersion", cdp::object(&[]))?.str_at(&["product"]).unwrap_or("?").to_owned();
        Ok(Self { child, profile, browser, host, version })
    }

    /// A fresh tab, and the session that drives it.
    ///
    /// # Errors
    ///
    /// The browser opened no tab, or its socket refused.
    pub fn open_page(&mut self) -> Result<Page> {
        let created = self.browser.call("Target.createTarget", cdp::object(&[("url", cdp::text("about:blank"))]))?;
        let target = created.str_at(&["targetId"]).ok_or_else(|| Error::new("the browser opened no tab"))?.to_owned();
        let session = Session::open(&format!("ws://{}/devtools/page/{target}", self.host))?;
        Ok(Page { session, target })
    }

    /// Closes the tab.
    ///
    /// # Errors
    ///
    /// The browser's own.
    pub fn close_page(&mut self, page: Page) -> Result<()> {
        drop(page.session);
        self.browser.call("Target.closeTarget", cdp::object(&[("targetId", Value::String(page.target))]))?;
        Ok(())
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// One tab.
pub struct Page {
    pub session: Session,
    target: String,
}
