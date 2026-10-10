//! A project `bunny new` writes, compiled for real: for the host and for
//! every target installed here, against this checkout's bunny-ui, and
//! its web build opened up to see the export the page boots.
//!
//! Slow — it builds the framework — so it runs on request:
//! `cargo test -p bunny-cli --test new_project -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGETS: &[&str] = &[
    "wasm32-unknown-unknown",
    "aarch64-linux-android",
    "aarch64-apple-ios",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf()
}

fn installed(target: &str) -> bool {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().unwrap();
    let sysroot = String::from_utf8_lossy(&sysroot.stdout).trim().to_string();
    Path::new(&sysroot).join("lib/rustlib").join(target).join("lib").is_dir()
}

/// Runs cargo in the project; fails on an error or on a warning in the
/// app's own files (the framework's are its own business).
fn cargo(app: &Path, args: &[&str]) {
    // what follows `--` is the compiler's
    let split = args.iter().position(|arg| *arg == "--").unwrap_or(args.len());
    let out = Command::new("cargo")
        .args(&args[..split])
        .arg("--message-format=short")
        .args(&args[split..])
        .current_dir(app)
        .env("CARGO_TARGET_DIR", repo().join("target/bunny-e2e"))
        .env("BUNNY_APP_NAME", "Ada \"Bunny\" App")
        .env("BUNNY_APP_ID", "io.bunny.e2e_app")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "cargo {args:?} failed:\n{stderr}");
    let ours: Vec<&str> =
        stderr.lines().filter(|line| line.starts_with("src/") && line.contains("warning")).collect();
    assert!(ours.is_empty(), "cargo {args:?} warned in the app:\n{}", ours.join("\n"));
}

#[test]
#[ignore = "builds the framework for every installed target; run with --ignored"]
fn a_new_project_builds_everywhere() {
    let here = std::env::temp_dir().join(format!("bunny-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&here);
    fs::create_dir_all(&here).unwrap();
    let facade = repo().join("crates/bunny_ui_facade");
    let out = Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(["new", "e2e_app", "--name", "Ada \"Bunny\" App", "--org", "io.bunny"])
        .arg("--bunny-ui-path")
        .arg(&facade)
        .current_dir(&here)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let app = here.join("e2e_app");

    cargo(&app, &["check", "--all-targets"]);
    for target in TARGETS.iter().filter(|target| installed(target)) {
        cargo(&app, &["check", "--all-targets", "--target", target]);
    }

    if installed("wasm32-unknown-unknown") {
        cargo(&app, &["rustc", "--lib", "--crate-type", "cdylib", "--target", "wasm32-unknown-unknown"]);
        let wasm = repo().join("target/bunny-e2e/wasm32-unknown-unknown/debug/e2e_app.wasm");
        let exports = wasm_exports(&fs::read(&wasm).unwrap());
        for name in ["start", "bunny_abi_version", "memory"] {
            assert!(exports.iter().any(|export| export == name), "{name} missing from {exports:?}");
        }
    }
    let _ = fs::remove_dir_all(&here);
}

/// The names in a wasm module's export section.
fn wasm_exports(bytes: &[u8]) -> Vec<String> {
    fn leb(bytes: &[u8], at: &mut usize) -> usize {
        let (mut value, mut shift) = (0usize, 0);
        loop {
            let byte = bytes[*at];
            *at += 1;
            value |= usize::from(byte & 0x7f) << shift;
            shift += 7;
            if byte < 0x80 {
                return value;
            }
        }
    }
    let mut at = 8;
    while at < bytes.len() {
        let id = bytes[at];
        at += 1;
        let size = leb(bytes, &mut at);
        let end = at + size;
        if id == 7 {
            let count = leb(bytes, &mut at);
            let mut names = Vec::new();
            for _ in 0..count {
                let length = leb(bytes, &mut at);
                names.push(String::from_utf8_lossy(&bytes[at..at + length]).into_owned());
                at += length + 1;
                leb(bytes, &mut at);
            }
            return names;
        }
        at = end;
    }
    Vec::new()
}
