//! Cargo, asked the way it answers machines: `cargo metadata` for what
//! the project is, `--message-format=json-render-diagnostics` for what a
//! build produced — so no path is guessed, and `CARGO_TARGET_DIR`, a
//! workspace or a target triple never send `bunny` to the wrong file.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::json::{self, Value};

/// `cargo metadata --no-deps`, as a value.
pub fn metadata(dir: &Path, manifest_path: Option<&Path>) -> Result<Value> {
    let mut command = Command::new("cargo");
    command.args(["metadata", "--no-deps", "--format-version", "1"]).current_dir(dir);
    if let Some(manifest) = manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    let out = command.output().map_err(|error| Error::new(format!("cargo: {error}")).hint("is Rust installed? https://rustup.rs"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("could not find `Cargo.toml`") {
            return Err(Error::new(format!("no app in {} or any folder above it", dir.display()))
                .hint("run it inside an app's folder, or create one: bunny new my_app"));
        }
        return Err(Error::new(format!("cargo could not read the project:\n{}", stderr.trim())));
    }
    json::parse(&String::from_utf8_lossy(&out.stdout)).map_err(|error| Error::new(format!("cargo metadata: {error}")))
}

/// One build `bunny` asks cargo for.
#[derive(Clone, Debug, Default)]
pub struct Build {
    /// The package's manifest — the artifacts come from it.
    pub manifest: PathBuf,
    pub package: String,
    pub what: Target,
    pub release: bool,
    /// A custom profile (`web`), in place of dev or release.
    pub profile: Option<String>,
    /// A cross target triple; `None` builds for this machine.
    pub target: Option<String>,
    pub features: Vec<String>,
    pub env: Vec<(String, OsString)>,
    /// Passed to rustc for the final crate alone (after `--`): a flag
    /// that does not rebuild the dependencies.
    pub rustc_args: Vec<String>,
    /// Without cargo's progress lines: only what the compiler says.
    pub quiet: bool,
    /// Where the compiler's diagnostics go: the terminal, or the result.
    pub diagnostics: Diagnostics,
}

/// What becomes of the compiler's diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Diagnostics {
    /// Rendered by cargo on the terminal, as `bunny run` shows them.
    #[default]
    Render,
    /// Collected into [`Built::diagnostics`], and nothing on the terminal
    /// — the lane an IDE drives reads them as data.
    Collect,
}

/// One thing the compiler said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// `error`, `warning`, `note`, `help`.
    pub level: String,
    pub message: String,
    /// The lint or error code, when it has one.
    pub code: Option<String>,
    /// The primary span's file, relative to where cargo ran.
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// The text as the compiler would have shown it.
    pub rendered: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// The library, as a C dynamic library — what Android loads and the
    /// browser instantiates.
    #[default]
    CdylibLib,
    Bin(String),
}

/// What a build produced.
#[derive(Debug)]
pub struct Built {
    /// The executable, for a binary; the shared library or the `.wasm`,
    /// for a library.
    pub artifact: PathBuf,
    /// The folder of every resolved package that matters to `bunny` —
    /// where `bunny-ui-web`'s glue lives.
    pub packages: Vec<(String, PathBuf)>,
    /// The other packages' targets this build compiled again, by name —
    /// a dependency that changed. The rest were fresh from an earlier
    /// build.
    pub rebuilt: Vec<String>,
    /// What the compiler said, under [`Diagnostics::Collect`].
    pub diagnostics: Vec<Diagnostic>,
}

/// Runs the build. Cargo's progress and diagnostics go straight to the
/// terminal, rendered as cargo renders them; the artifact paths come
/// from its JSON on stdout.
pub fn build(spec: &Build) -> Result<Built> {
    let mut command = Command::new("cargo");
    let subcommand = if spec.what == Target::CdylibLib || !spec.rustc_args.is_empty() { "rustc" } else { "build" };
    command.arg(subcommand).arg("--manifest-path").arg(&spec.manifest).arg("-p").arg(&spec.package);
    match &spec.what {
        Target::CdylibLib => {
            command.args(["--lib", "--crate-type", "cdylib"]);
        }
        Target::Bin(name) => {
            command.arg("--bin").arg(name);
        }
    }
    match &spec.profile {
        Some(profile) => {
            command.arg("--profile").arg(profile);
        }
        None if spec.release => {
            command.arg("--release");
        }
        None => {}
    }
    if let Some(target) = &spec.target {
        command.arg("--target").arg(target);
    }
    if !spec.features.is_empty() {
        command.arg("--features").arg(spec.features.join(","));
    }
    match spec.diagnostics {
        Diagnostics::Render => {
            command.arg("--message-format=json-render-diagnostics");
        }
        Diagnostics::Collect => {
            command.arg("--message-format=json");
        }
    }
    if spec.quiet {
        command.arg("--quiet");
    }
    if std::io::stderr().is_terminal() && spec.diagnostics == Diagnostics::Render {
        command.arg("--color=always");
    }
    if !spec.rustc_args.is_empty() {
        command.arg("--").args(&spec.rustc_args);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    // collected diagnostics leave the terminal alone: cargo's own
    // progress goes nowhere, and what the compiler says is data
    let stderr = if spec.diagnostics == Diagnostics::Collect { Stdio::null() } else { Stdio::inherit() };
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(stderr);
    let mut child = command.spawn().map_err(|error| Error::new(format!("cargo: {error}")))?;
    let mut artifact = None;
    let mut packages = Vec::new();
    let mut rebuilt = Vec::new();
    let mut diagnostics = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
            let Ok(message) = json::parse(&line) else { continue };
            if message.str_at(&["reason"]) == Some("compiler-message") {
                if let Some(diagnostic) = diagnostic(&message) {
                    diagnostics.push(diagnostic);
                }
                continue;
            }
            if message.str_at(&["reason"]) != Some("compiler-artifact") {
                continue;
            }
            let (Some(name), Some(manifest)) = (message.str_at(&["target", "name"]), message.str_at(&["manifest_path"]))
            else {
                continue;
            };
            if let Some(dir) = Path::new(manifest).parent() {
                packages.push((name.to_string(), dir.to_path_buf()));
            }
            if message.get("fresh").and_then(Value::as_bool) == Some(false) && Path::new(manifest) != spec.manifest {
                rebuilt.push(name.to_string());
            }
            if Path::new(manifest) != spec.manifest {
                continue;
            }
            let kinds: Vec<&str> =
                message.path(&["target", "kind"]).map(Value::as_array).unwrap_or_default().iter().filter_map(Value::as_str).collect();
            match &spec.what {
                Target::Bin(bin) if name == bin && kinds.contains(&"bin") => {
                    artifact = message.str_at(&["executable"]).map(PathBuf::from);
                }
                Target::CdylibLib if kinds.iter().any(|kind| matches!(*kind, "lib" | "cdylib" | "rlib")) => {
                    let files = message.get("filenames").map(Value::as_array).unwrap_or_default();
                    artifact = files
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|file| [".so", ".wasm", ".dylib", ".dll"].iter().any(|ext| file.ends_with(ext)))
                        .map(PathBuf::from)
                        .or(artifact);
                }
                _ => {}
            }
        }
    }
    let status = child.wait().map_err(|error| Error::new(format!("cargo: {error}")))?;
    if !status.success() {
        // cargo has already said why, on the terminal — or in the
        // diagnostics, which the error carries for whoever collects them
        return Err(Error::build_failed(diagnostics));
    }
    let artifact = artifact.ok_or_else(|| Error::new("cargo built nothing `bunny` can run"))?;
    Ok(Built { artifact, packages, rebuilt, diagnostics })
}

/// A `compiler-message` as data: the level, the message, the primary
/// span and the rendered text.
fn diagnostic(message: &Value) -> Option<Diagnostic> {
    let inner = message.get("message")?;
    let level = inner.str_at(&["level"])?.to_string();
    let primary = inner.get("spans").map(Value::as_array).unwrap_or_default().iter().find(|span| span.get("is_primary").and_then(Value::as_bool) == Some(true));
    Some(Diagnostic {
        level,
        message: inner.str_at(&["message"]).unwrap_or_default().to_string(),
        code: inner.str_at(&["code", "code"]).map(String::from),
        file: primary.and_then(|span| span.str_at(&["file_name"])).map(String::from),
        line: primary.and_then(|span| span.get("line_start")).and_then(Value::as_u64).map(|line| line as u32),
        column: primary.and_then(|span| span.get("column_start")).and_then(Value::as_u64).map(|column| column as u32),
        rendered: inner.str_at(&["rendered"]).unwrap_or_default().to_string(),
    })
}
