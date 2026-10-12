//! The scan against its fixtures — the contract Trinity's tree-sitter
//! scan meets too. Set `BUNNY_SCAN_WRITE=1` to rewrite the expected JSON
//! after a deliberate change to the scanner.

use std::path::Path;

use bunny_ui_preview::scan::{ScanReport, scan_dir, scan_source};

fn check(report: &ScanReport, expected: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scan/expected").join(expected);
    let json = report.json();
    if std::env::var_os("BUNNY_SCAN_WRITE").is_some() {
        std::fs::write(&path, &json).unwrap();
    }
    let golden = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(json == golden, "{expected} moved:\n--- expected\n{golden}\n--- got\n{json}");
}

#[test]
fn the_basic_fixture_lists_four_previews_on_three_cards() {
    let report = scan_source(include_str!("scan/fixtures/basic.rs"), "src/basic.rs");
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let idents: Vec<&str> = report.previews.iter().map(|p| p.ident.as_str()).collect();
    assert_eq!(idents, ["card_short", "card_long", "plain_button", "nested_field"]);
    let sites: Vec<String> = report.previews.iter().map(|p| format!("{}:{}:{}", p.file, p.line, p.column)).collect();
    assert_eq!(sites, ["src/basic.rs:5:1", "src/basic.rs:13:1", "src/basic.rs:18:1", "src/basic.rs:24:5"]);
    check(&report, "basic.json");
}

#[test]
fn the_tricky_fixture_keeps_only_the_fine_one_and_names_the_rest() {
    let report = scan_source(include_str!("scan/fixtures/tricky.rs"), "src/tricky.rs");
    let idents: Vec<&str> = report.previews.iter().map(|p| p.ident.as_str()).collect();
    assert_eq!(idents, ["fine"]);
    assert_eq!(report.previews[0].name, "Fine");
    let refused: Vec<String> = report.refused.iter().map(ToString::to_string).collect();
    assert_eq!(
        refused,
        [
            "src/tricky.rs:10:1: `with_params` takes parameters; a preview takes none",
            "src/tricky.rs:14:1: `#[preview]` does not take `label`",
            "src/tricky.rs:19:1: `#[preview]` must sit on a free function",
        ]
    );
    check(&report, "tricky.json");
}

#[test]
fn a_crate_scan_folds_its_files_and_names_the_ident_two_of_them_claim() {
    let report = scan_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scan/fixtures"));
    let idents: Vec<&str> = report.previews.iter().map(|p| p.ident.as_str()).collect();
    assert_eq!(idents, ["card_short", "card_long", "plain_button", "nested_field", "twin", "fine"]);
    assert_eq!(report.duplicates.len(), 1);
    assert_eq!(report.duplicates[0].ident, "twin");
    assert_eq!(report.duplicates[0].kept.to_string(), "dup_a.rs:2:1");
    assert_eq!(report.duplicates[0].dropped.to_string(), "dup_b.rs:2:1");
    assert!(report.unreadable.is_empty());
    check(&report, "dir.json");
}
