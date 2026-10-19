//! Xcode, its command-line tools, and the simulators it manages — the
//! macOS and iOS half of the machine.

use super::QUICK;
use crate::json::{self, Value};
use crate::process;

/// Where `xcode-select` points: a full Xcode, or the Command Line Tools
/// alone (enough for macOS, not for iOS).
pub fn developer_dir() -> Option<String> {
    let out = process::run("xcode-select", &["-p"], QUICK).ok().filter(process::Output::ok)?;
    Some(out.stdout.trim().to_string())
}

/// A C toolchain is there: what every Rust build on macOS links with.
pub fn has_clang() -> bool {
    process::run("xcrun", &["--find", "clang"], QUICK).is_ok_and(|out| out.ok())
}

/// `Xcode 27.0`, when a full Xcode answers.
pub fn xcode_version() -> Option<String> {
    let out = process::run("xcodebuild", &["-version"], QUICK).ok().filter(process::Output::ok)?;
    out.stdout.lines().next().map(|line| line.trim().to_string())
}

/// The iOS Simulator SDK's version — the proof the selected developer
/// directory is a full Xcode.
pub fn simulator_sdk() -> Option<String> {
    let out =
        process::run("xcrun", &["--sdk", "iphonesimulator", "--show-sdk-version"], QUICK).ok().filter(process::Output::ok)?;
    Some(out.stdout.trim().to_string())
}

/// Whether the Xcode license has been agreed to — until it is, every
/// build tool refuses to run.
pub fn license_accepted() -> bool {
    process::run("xcodebuild", &["-license", "check"], QUICK).is_ok_and(|out| out.ok())
}

/// An iOS runtime the simulators can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub identifier: String,
    pub version: String,
}

/// The available iOS runtimes, from `simctl list runtimes --json`.
pub fn ios_runtimes() -> Vec<Runtime> {
    match process::run("xcrun", &["simctl", "list", "runtimes", "--json"], QUICK) {
        Ok(out) if out.ok() => parse_runtimes(&out.stdout),
        _ => Vec::new(),
    }
}

pub fn parse_runtimes(text: &str) -> Vec<Runtime> {
    let Ok(value) = json::parse(text) else { return Vec::new() };
    value
        .get("runtimes")
        .map(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|runtime| runtime.str_at(&["platform"]) == Some("iOS"))
        .filter(|runtime| runtime.get("isAvailable").and_then(Value::as_bool) != Some(false))
        .filter_map(|runtime| {
            Some(Runtime {
                identifier: runtime.str_at(&["identifier"])?.to_string(),
                version: runtime.str_at(&["version"])?.to_string(),
            })
        })
        .collect()
}

/// How many signing identities the keychain holds — what a physical
/// device and a release need, and the simulator does not.
pub fn signing_identities() -> usize {
    match process::run("security", &["find-identity", "-v", "-p", "codesigning"], QUICK) {
        Ok(out) if out.ok() => out.stdout.lines().filter(|line| line.contains(") ") && line.contains('"')).count(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_available_ios_runtimes_count() {
        let text = r#"{"runtimes":[
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-0","version":"27.0","isAvailable":true,"platform":"iOS"},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.watchOS-13-0","version":"13.0","isAvailable":true,"platform":"watchOS"},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-0","version":"26.0","isAvailable":false,"platform":"iOS"}]}"#;
        assert_eq!(
            parse_runtimes(text),
            vec![Runtime { identifier: String::from("com.apple.CoreSimulator.SimRuntime.iOS-27-0"), version: String::from("27.0") }]
        );
    }
}
