use super::{Event, build_failed};
use crate::cargo::Diagnostic;
use crate::error::Error;

#[test]
fn failed_reload_preserves_every_compiler_diagnostic() {
    let diagnostic = Diagnostic {
        level: String::from("error"),
        message: String::from("cannot find value `missing` in this scope"),
        code: Some(String::from("E0425")),
        file: Some(String::from("src/lib.rs")),
        line: Some(12),
        column: Some(18),
        rendered: String::from("error[E0425]: cannot find value `missing` in this scope\n"),
    };
    let mut warning = diagnostic.clone();
    warning.level = String::from("warning");
    let expected = vec![warning, diagnostic];
    let mut events = Vec::new();
    build_failed(Error::build_failed(expected.clone()), &mut events);
    assert!(
        matches!(events.as_slice(), [Event::BuildFailed { diagnostics }] if diagnostics == &expected)
    );
}

#[test]
fn diagnostic_free_failure_keeps_its_message_and_hint_and_ends_building() {
    let mut events = Vec::new();
    build_failed(
        Error::new("generation library could not be copied")
            .hint("check the target directory permissions"),
        &mut events,
    );
    assert!(
        matches!(events.as_slice(), [Event::Note(message), Event::BuildFailed { diagnostics }]
        if message == "generation library could not be copied\ncheck the target directory permissions"
            && diagnostics.is_empty())
    );
}

#[test]
fn diagnostic_free_failure_without_hint_still_ends_building() {
    let mut events = Vec::new();
    build_failed(Error::new("cargo did not start"), &mut events);
    assert!(
        matches!(events.as_slice(), [Event::Note(message), Event::BuildFailed { diagnostics }]
        if message == "cargo did not start" && diagnostics.is_empty())
    );
}
