//! `bunny build`: the app as a package to ship — a site for the web, a
//! signed app and its disk image for macOS — under the project's
//! `build/<platform>/`, next to a `build-info.json` that says what each
//! file is and a `build.log` with every command that made them.

pub mod android;
pub mod linux;
pub mod macos;
pub mod web;
pub mod windows;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{self, Error, Result};
use crate::json;
use crate::process::{self, Output};
use crate::project::Project;

/// How `bunny build` builds.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Optimized — the default; `--debug` turns it off.
    pub release: bool,
    pub features: Vec<String>,
    /// macOS: who signs the app.
    pub signing: Signing,
    /// macOS: the notarytool keychain profile, when there is one.
    pub notary_profile: Option<String>,
    /// macOS: notarize, or fail.
    pub notarize: bool,
    /// macOS: one binary for Apple silicon and Intel.
    pub universal: bool,
    /// macOS: a disk image next to the app.
    pub dmg: bool,
    /// Android: the ABIs to build for — none named, every one bunny-ui
    /// supports.
    pub abis: Vec<String>,
}

/// Who signs a macOS app.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Signing {
    /// The one Developer ID Application identity in the keychain — or an
    /// ad hoc signature, with a warning, when there is none.
    #[default]
    Auto,
    /// This identity, by name or by hash.
    Identity(String),
    /// Nobody: the app stays as the linker left it.
    Unsigned,
}

/// Every command a build ran and what each answered: `build.log`, the
/// file to read when a signature or a notarization goes wrong.
#[derive(Default)]
pub struct Log {
    text: String,
}

impl Log {
    /// A line of the build's own.
    pub fn note(&mut self, text: &str) {
        self.text.push_str("# ");
        self.text.push_str(text);
        self.text.push('\n');
    }

    /// Runs `program`, its command line and its answer kept.
    pub fn run<S: AsRef<OsStr>>(&mut self, program: &str, args: &[S], timeout: Duration) -> Result<Output> {
        self.run_with(program, args, &[], timeout)
    }

    /// [`Log::run`] with more environment — which the log leaves out:
    /// it may hold a password.
    pub fn run_with<S: AsRef<OsStr>>(
        &mut self,
        program: &str,
        args: &[S],
        env: &[(&str, &OsStr)],
        timeout: Duration,
    ) -> Result<Output> {
        let line: Vec<String> = std::iter::once(program.to_string())
            .chain(args.iter().map(|arg| quote(&arg.as_ref().to_string_lossy())))
            .collect();
        self.text.push_str(&format!("$ {}\n", line.join(" ")));
        let out = process::run_in(program, args, None, env, timeout)
            .map_err(|error| Error::new(format!("{program}: {error}")))?;
        for text in [&out.stdout, &out.stderr] {
            if !text.trim().is_empty() {
                self.text.push_str(text.trim_end());
                self.text.push('\n');
            }
        }
        match out.code {
            Some(code) => self.text.push_str(&format!("(exit {code})\n\n")),
            None => self.text.push_str("(killed at its deadline)\n\n"),
        }
        Ok(out)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, &self.text).map_err(error::at(path))
    }
}

/// An argument as a shell would need it written.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c)) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// `build-info.json`: what the package is and how it was made, for a
/// person or a release script to read.
pub struct Info {
    fields: Vec<(String, String)>,
    files: Vec<String>,
}

impl Info {
    pub fn new(project: &Project, platform: &str, options: &Options) -> Info {
        let mut info = Info { fields: Vec::new(), files: Vec::new() };
        info.field("platform", platform);
        info.field("name", &project.name);
        info.field("id", project.id.as_deref().unwrap_or(""));
        info.field("version", &project.version);
        info.field("build", &project.build.to_string());
        info.field("profile", if options.release { "release" } else { "debug" });
        info.field("bunny", env!("CARGO_PKG_VERSION"));
        info
    }

    pub fn field(&mut self, key: &str, value: &str) {
        self.fields.retain(|(name, _)| name != key);
        self.fields.push((key.to_string(), value.to_string()));
    }

    /// A file of the package, by its path under `build/<platform>/`.
    pub fn file(&mut self, name: &str) {
        self.files.push(name.to_string());
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let mut text = String::from("{\n");
        for (key, value) in &self.fields {
            text.push_str(&format!("  {}: {},\n", json::quote(key), json::quote(value)));
        }
        let files: Vec<String> = self.files.iter().map(|file| json::quote(file)).collect();
        text.push_str(&format!("  \"files\": [{}]\n}}\n", files.join(", ")));
        std::fs::write(path, text).map_err(error::at(path))
    }
}

/// `build/<platform>/` in the app's folder, emptied: a package holds what
/// this build made and nothing older.
pub fn out_dir(project: &Project, platform: &str) -> Result<PathBuf> {
    let dir = project.dir.join("build").join(platform);
    crate::platform::fresh_dir(&dir)?;
    Ok(dir)
}

/// A file's size the way people read it.
pub fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_logged_argument_reads_back_as_the_shell_would() {
        assert_eq!(quote("--options=runtime"), "--options=runtime");
        assert_eq!(quote("Developer ID Application: Ada (AB12CD34EF)"), "'Developer ID Application: Ada (AB12CD34EF)'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn sizes_read_as_people_read_them() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(2_048), "2.0 KB");
        assert_eq!(size(3_400_000), "3.4 MB");
    }
}
