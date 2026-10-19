//! The Rust toolchain: the compiler's version, its host, and which
//! targets' standard libraries are installed.

use std::path::{Path, PathBuf};

use super::QUICK;
use crate::process;

/// The oldest compiler that reads edition 2024.
pub const MINIMUM: (u32, u32) = (1, 85);

#[derive(Clone, Debug)]
pub struct Rust {
    /// `1.98.1`.
    pub version: String,
    pub major: u32,
    pub minor: u32,
    /// The host triple: `aarch64-apple-darwin`.
    pub host: String,
    pub sysroot: PathBuf,
    /// Installed by rustup — the only way `rustup target add` can fix a
    /// missing target.
    pub rustup: bool,
}

impl Rust {
    pub fn recent_enough(&self) -> bool {
        (self.major, self.minor) >= MINIMUM
    }

    /// Whether the standard library for `target` is installed — what a
    /// cross build needs, with or without rustup.
    pub fn has_target(&self, target: &str) -> bool {
        has_target(&self.sysroot, target)
    }
}

pub fn has_target(sysroot: &Path, target: &str) -> bool {
    sysroot.join("lib/rustlib").join(target).join("lib").is_dir()
}

/// The compiler on the PATH, or `None` when there is none.
pub fn detect() -> Option<Rust> {
    let verbose = process::run("rustc", &["-vV"], QUICK).ok().filter(process::Output::ok)?;
    let (version, host) = parse_verbose(&verbose.stdout)?;
    let mut numbers = version.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    let (major, minor) = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    let sysroot = process::run("rustc", &["--print", "sysroot"], QUICK).ok().filter(process::Output::ok)?;
    Some(Rust {
        version,
        major,
        minor,
        host,
        sysroot: PathBuf::from(sysroot.stdout.trim()),
        rustup: process::which("rustup").is_some(),
    })
}

/// `release:` and `host:` out of `rustc -vV`.
fn parse_verbose(text: &str) -> Option<(String, String)> {
    let field = |name: &str| {
        text.lines().find_map(|line| line.strip_prefix(name).map(|value| value.trim().to_string()))
    };
    Some((field("release:")?, field("host:")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verbose_version_reads() {
        let text = "rustc 1.98.1 (797e8a9bc 2026-08-05)\nbinary: rustc\ncommit-hash: 797e8a9\n\
                    host: aarch64-apple-darwin\nrelease: 1.98.1\nLLVM version: 21.1.0\n";
        assert_eq!(parse_verbose(text), Some((String::from("1.98.1"), String::from("aarch64-apple-darwin"))));
    }

    #[test]
    fn a_target_is_its_library_folder() {
        let root = std::env::temp_dir().join(format!("bunny-sysroot-{}", std::process::id()));
        std::fs::create_dir_all(root.join("lib/rustlib/wasm32-unknown-unknown/lib")).unwrap();
        assert!(has_target(&root, "wasm32-unknown-unknown"));
        assert!(!has_target(&root, "aarch64-linux-android"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
