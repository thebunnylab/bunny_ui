//! The terminal: color and marks when a person reads it, plain text when
//! a script or a file does.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Whether standard output is painted — decided once, the way
/// `NO_COLOR` and `TERM=dumb` ask every tool to decide it.
pub fn painted() -> bool {
    static PAINTED: OnceLock<bool> = OnceLock::new();
    *PAINTED.get_or_init(|| allowed() && std::io::stdout().is_terminal())
}

/// The same question for standard error, where failures go.
pub fn painted_err() -> bool {
    static PAINTED: OnceLock<bool> = OnceLock::new();
    *PAINTED.get_or_init(|| allowed() && std::io::stderr().is_terminal())
}

fn allowed() -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    // the legacy Windows console prints escape codes as text until a
    // console mode is switched on, which takes `unsafe`; Windows
    // Terminal and the shells that set TERM read them as color
    !cfg!(windows) || std::env::var_os("WT_SESSION").is_some() || std::env::var_os("TERM").is_some()
}

fn paint(on: bool, code: &str, text: &str) -> String {
    if on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
}

pub fn bold(text: &str) -> String {
    paint(painted(), "1", text)
}

pub fn dim(text: &str) -> String {
    paint(painted(), "2", text)
}

pub fn green(text: &str) -> String {
    paint(painted(), "32", text)
}

pub fn yellow(text: &str) -> String {
    paint(painted(), "33", text)
}

pub fn cyan(text: &str) -> String {
    paint(painted(), "36", text)
}

/// A line that went well.
pub fn ok(text: &str) -> String {
    format!("{} {text}", green("✓"))
}

/// A line that went through, with something to know.
pub fn warn(text: &str) -> String {
    format!("{} {text}", yellow("!"))
}

/// `error: …` on standard error, with the hint under it.
pub fn print_error(message: &str, hint: Option<&str>) {
    let label = paint(painted_err(), "1;31", "error:");
    eprintln!("{label} {message}");
    if let Some(hint) = hint {
        for line in hint.lines() {
            eprintln!("  {line}");
        }
    }
}
