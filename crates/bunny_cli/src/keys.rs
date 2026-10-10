//! Single keys while the app runs: `r`, `R`, `q` — read from the
//! terminal without waiting for Enter, the way `flutter run` reads them.
//!
//! On Unix the terminal is switched with `stty` (no `unsafe`): no line
//! buffering, no echo, and Ctrl-C arriving as a key, so `bunny` stops
//! the app itself and puts the terminal back. It is switched only while
//! the app runs — during a build Ctrl-C is the shell's, as always — and
//! restored on drop, panics included. On Windows a key is a line: `r`
//! then Enter.

use std::io::Read;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

/// Ctrl-C, as a key.
pub const INTERRUPT: char = '\u{3}';

pub struct Keys {
    receiver: Receiver<char>,
    /// The terminal's settings before raw mode, to put back.
    #[cfg(unix)]
    saved: Option<String>,
}

impl Keys {
    /// Starts reading keys, or `None` when nobody is at a terminal.
    pub fn start() -> Option<Keys> {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return None;
        }
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            #[cfg(unix)]
            let source = std::fs::File::open("/dev/tty");
            #[cfg(not(unix))]
            let source: std::io::Result<std::io::Stdin> = Ok(std::io::stdin());
            let Ok(mut source) = source else { return };
            let mut byte = [0u8; 1];
            while let Ok(1) = source.read(&mut byte) {
                if byte[0] == b'\n' || byte[0] == b'\r' {
                    continue;
                }
                if sender.send(char::from(byte[0])).is_err() {
                    return;
                }
            }
        });
        Some(Keys {
            receiver,
            #[cfg(unix)]
            saved: None,
        })
    }

    /// Single keys from now on (while the app runs).
    pub fn raw(&mut self) {
        #[cfg(unix)]
        if self.saved.is_none() {
            self.saved = stty(&["-g"]).map(|settings| settings.trim().to_string());
            let _ = stty(&["-icanon", "-echo", "-isig", "min", "1"]);
        }
        // a key typed during the build is not an order for the new run
        while self.receiver.try_recv().is_ok() {}
    }

    /// The terminal as it was (for a build, and on the way out).
    pub fn cooked(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = self.saved.take() {
            let _ = stty(&[&saved]);
        }
    }

    /// The next key, waiting at most `timeout`.
    pub fn next(&self, timeout: Duration) -> Option<char> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.cooked();
    }
}

/// `stty` on the controlling terminal, its answer.
#[cfg(unix)]
fn stty(args: &[&str]) -> Option<String> {
    let tty = std::fs::File::open("/dev/tty").ok()?;
    let out = std::process::Command::new("stty").args(args).stdin(tty).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
