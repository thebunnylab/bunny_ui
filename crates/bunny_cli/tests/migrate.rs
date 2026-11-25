//! `bunny migrate`: a block written for one bunny-ui moves to the next by
//! rules, the diff is shown before anything is written, and what no rule
//! can carry is named, line by line, for a person or an agent.
//!
//! The block in `tests/migrate/0.5/places.rs` is bunny-ui 0.5; the rules
//! turn it into `tests/migrate/0.6/places.rs` byte for byte, and
//! `apps/hello_block/tests/migrated.rs` builds and runs that file against
//! the framework in this tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use bunny_cli::migrate::{self, Version, diff};

const BEFORE: &str = include_str!("migrate/0.5/places.rs");
const AFTER: &str = include_str!("migrate/0.6/places.rs");

const V04: Version = Version::new(0, 4);
const V05: Version = Version::new(0, 5);
const V06: Version = Version::new(0, 6);

fn bunny(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("the bunny binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A fresh, empty folder for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bunny-cli-migrate-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A block's crate written for bunny-ui 0.5.
fn block_05(name: &str) -> PathBuf {
    let dir = scratch(name);
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"places\"\nversion = \"0.5.0\"\nedition = \"2024\"\n\n[dependencies]\nbunny-ui = { version = \"0.5\", features = [\"canvas\"] }\nserde = \"0.5\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), BEFORE).unwrap();
    // what cargo built is not the block's source
    fs::create_dir_all(dir.join("target/debug")).unwrap();
    fs::write(dir.join("target/debug/old.rs"), "fn f() { nav_link_value(1, 2); }\n").unwrap();
    dir
}

#[test]
fn a_block_written_for_0_5_reads_as_0_6() {
    let migrated = migrate::source(BEFORE, V05, V06).unwrap();
    assert!(migrated.gaps.is_empty(), "{:?}", migrated.gaps);
    assert!(migrated.text == AFTER, "{}", diff::unified(AFTER, &migrated.text, "expected", "migrated"));
}

#[test]
fn a_block_already_at_the_version_is_left_alone() {
    let migrated = migrate::source(AFTER, V06, V06).unwrap();
    assert_eq!(migrated.text, AFTER);
    assert!(migrated.gaps.is_empty());
}

#[test]
fn comments_strings_and_definitions_keep_their_words() {
    let source = "// nav_link_value(a, b)\n/* .refreshable(|| x) /* nested */ */\nlet s = \"nav_link_value(a, b)\";\nlet r = r#\"nav_link_value(a, \"b\")\"#;\nfn nav_link_value(a: u8, b: u8) {}\nlet c = '(';\n";
    assert_eq!(migrate::source(source, V05, V06).unwrap().text, source);
}

#[test]
fn links_swap_inside_out_and_arguments_split_at_their_own_commas() {
    let cases = [
        ("nav_link_value(1, nav_link_value(2, text(\"a\")))", "nav_link_value(nav_link_value(text(\"a\"), 2), 1)"),
        ("nav_link_value(route(1, [2, 3]), text(\"x\"))", "nav_link_value(text(\"x\"), route(1, [2, 3]))"),
        ("nav_link_value(Vec::<(u8, u8)>::new(), label)", "nav_link_value(label, Vec::<(u8, u8)>::new())"),
        ("nav_link_value(|a, b| a + b, label,)", "nav_link_value(label, |a, b| a + b,)"),
        ("bunny_ui::nav_link_value('a', \"b\")", "bunny_ui::nav_link_value(\"b\", 'a')"),
        ("nav_link_value(\n    Place::Porto, // where\n    cell(),\n)", "nav_link_value(\n    cell(), // where\n    Place::Porto,\n)"),
    ];
    for (before, after) in cases {
        assert_eq!(migrate::source(before, V05, V06).unwrap().text, after, "{before}");
    }
}

#[test]
fn the_bar_takes_items_and_the_refresh_is_async() {
    let cases = [
        (".toolbar(add_button())", ".toolbar(toolbar_item(add_button()))"),
        (".toolbar((edit(), add()))", ".toolbar((toolbar_item(edit()), toolbar_item(add())))"),
        (".toolbar(toolbar_item(add()).leading())", ".toolbar(toolbar_item(add()).leading())"),
        (".refreshable(|| model.reload())", ".refreshable(async || model.reload())"),
        (".refreshable(move || { a(); b() })", ".refreshable(async move || { a(); b() })"),
        (".refreshable(async move || model.reload().await)", ".refreshable(async move || model.reload().await)"),
        ("navigation_stack(path, (root(),))", "navigation_stack(path, root())"),
        ("let link: NavigationLink<Text> = l;", "let link: NavLink<Text> = l;"),
        ("navigation_link(Details::new(id), text(\"Open\"))", "nav_link(text(\"Open\"), move || Details::new(id))"),
        ("navigation_link(\n    About,\n    text(\"About\"),\n)", "nav_link(\n    text(\"About\"),\n    move || About,\n)"),
    ];
    for (before, after) in cases {
        assert_eq!(migrate::source(before, V05, V06).unwrap().text, after, "{before}");
    }
}

#[test]
fn what_no_rule_carries_is_named_where_it_stands() {
    let source = "struct A {\n    path: State<NavigationPath>,\n}\nfn b() {\n    page.navigation_destination();\n    page.refreshable(reload);\n    navigation_stack_content((a(), b()));\n}\n";
    let migrated = migrate::source(source, V05, V06).unwrap();
    assert_eq!(migrated.text, source, "a gap is named, never guessed at");
    let at: Vec<(usize, usize, &str)> = migrated.gaps.iter().map(|gap| (gap.line, gap.column, gap.found.as_str())).collect();
    assert_eq!(
        at,
        [
            (2, 17, "NavigationPath"),
            (5, 10, "navigation_destination"),
            (6, 10, "refreshable"),
            (7, 5, "navigation_stack_content"),
        ]
    );
    assert!(migrated.gaps.iter().all(|gap| !gap.note.is_empty()));
}

#[test]
fn from_0_4_a_modifier_report_reads_its_present_flag() {
    let source = "if runtime.modifiers_changed(held) {\n    present();\n}\nlet t = runtime.modifiers_changed(held).tap_taken;\nmatch m { KeyMatch::Action(a) => run(a), KeyMatch::Pending => {}, KeyMatch::None => {} }\n";
    let migrated = migrate::source(source, V04, V05).unwrap();
    assert_eq!(
        migrated.text,
        "if runtime.modifiers_changed(held).present {\n    present();\n}\nlet t = runtime.modifiers_changed(held).tap_taken;\nmatch m { KeyMatch::Action(a) => run(a), KeyMatch::Pending => {}, KeyMatch::None => {} }\n"
    );
    assert_eq!(migrated.gaps.len(), 1, "{:?}", migrated.gaps);
    assert_eq!((migrated.gaps[0].line, migrated.gaps[0].found.as_str()), (5, "KeyMatch"));
}

#[test]
fn the_steps_chain_from_0_4_to_0_6() {
    let source = "fn f() { if r.modifiers_changed(h) { nav_link_value(1, text(\"a\")); } }\n";
    assert_eq!(
        migrate::source(source, V04, V06).unwrap().text,
        "fn f() { if r.modifiers_changed(h).present { nav_link_value(text(\"a\"), 1); } }\n"
    );
    assert!(migrate::source(source, V06, V05).is_err(), "no rules lead back");
    assert!(migrate::source(source, V05, Version::new(0, 9)).is_err(), "no rules reach a version this bunny does not know");
}

#[test]
fn the_manifest_asks_for_the_next_version_where_it_names_one() {
    let manifest = "[package]\nname = \"x\"\nversion = \"0.5.0\"\n\n[dependencies]\nbunny-ui = \"0.5\"\nbunny-ui-shell = { version = \"0.5.0\", features = [\"engine\"] }\nui = { package = \"bunny-ui-core\", version = \"^0.5\", default-features = false }\nserde = \"0.5\"\n\n[target.'cfg(unix)'.dependencies.bunny-ui-preview]\nversion = \"=0.5.0\"\n\n[dev-dependencies]\n\"bunny-ui-formats\" = { version = \"~0.5.1\" }\n";
    assert_eq!(migrate::manifest_version(manifest), migrate::Requirement::Version(V05));
    assert_eq!(
        migrate::manifest(manifest, V06).as_deref(),
        Some(
            "[package]\nname = \"x\"\nversion = \"0.5.0\"\n\n[dependencies]\nbunny-ui = \"0.6\"\nbunny-ui-shell = { version = \"0.6.0\", features = [\"engine\"] }\nui = { package = \"bunny-ui-core\", version = \"^0.6\", default-features = false }\nserde = \"0.5\"\n\n[target.'cfg(unix)'.dependencies.bunny-ui-preview]\nversion = \"=0.6.0\"\n\n[dev-dependencies]\n\"bunny-ui-formats\" = { version = \"~0.6.0\" }\n"
        )
    );
    let workspace = "[package]\nname = \"x\"\n\n[dependencies]\nbunny-ui = { workspace = true }\n";
    assert_eq!(migrate::manifest_version(workspace), migrate::Requirement::Workspace);
    assert_eq!(migrate::manifest(workspace, V06), None);
    let path = "[package]\nname = \"x\"\n\n[dependencies]\nbunny-ui = { path = \"../bunny_ui\" }\n";
    assert_eq!(migrate::manifest_version(path), migrate::Requirement::None);
    let root = "[workspace]\nmembers = [\"a\"]\n\n[workspace.dependencies]\nbunny-ui = { version = \"0.5.0\", path = \"crates/bunny_ui\" }\n";
    assert_eq!(migrate::manifest_version(root), migrate::Requirement::Version(V05));
}

#[test]
fn the_diff_is_unified() {
    let before = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
    let after = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
    assert_eq!(
        diff::unified(before, after, "a/x.rs", "b/x.rs"),
        "--- a/x.rs\n+++ b/x.rs\n@@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n d\n e\n@@ -8,3 +8,4 @@\n h\n i\n j\n+k\n"
    );
    assert_eq!(diff::unified(before, before, "a", "b"), "");
    assert_eq!(diff::unified("x", "y", "a", "b"), "--- a\n+++ b\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n");
}

#[test]
fn bunny_migrate_shows_the_diff_and_writes_only_when_told() {
    let dir = block_05("cli");
    let out = bunny(&["migrate", "--dry-run"], &dir);
    let shown = text(&out.stdout);
    assert!(out.status.success(), "{}{}", shown, text(&out.stderr));
    assert!(shown.contains("bunny-ui 0.5 → 0.6"), "{shown}");
    assert!(shown.contains("--- a/src/lib.rs\n+++ b/src/lib.rs\n"), "{shown}");
    assert!(shown.contains("\n-            nav_link_value(Place::Lisbon, text(\"Lisbon\")),\n"), "{shown}");
    assert!(shown.contains("\n+            nav_link_value(text(\"Lisbon\"), Place::Lisbon),\n"), "{shown}");
    assert!(shown.contains("\n-bunny-ui = { version = \"0.5\", features = [\"canvas\"] }\n+bunny-ui = { version = \"0.6\", features = [\"canvas\"] }\n"), "{shown}");
    assert!(!shown.contains("target/"), "what cargo built is not read: {shown}");
    assert_eq!(fs::read_to_string(dir.join("src/lib.rs")).unwrap(), BEFORE, "a dry run writes nothing");

    // nobody at the terminal to say yes: the diff, and nothing written
    let out = bunny(&["migrate"], &dir);
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("+++ b/src/lib.rs"));
    assert!(text(&out.stdout).contains("--yes"), "{}", text(&out.stdout));
    assert_eq!(fs::read_to_string(dir.join("src/lib.rs")).unwrap(), BEFORE);

    let out = bunny(&["migrate", "--yes"], &dir);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("+++ b/src/lib.rs"), "the diff is shown before it is written");
    assert_eq!(fs::read_to_string(dir.join("src/lib.rs")).unwrap(), AFTER);
    let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("bunny-ui = { version = \"0.6\", features = [\"canvas\"] }"), "{manifest}");
    assert!(manifest.contains("serde = \"0.5\"") && manifest.contains("version = \"0.5.0\"\nedition"), "{manifest}");
    assert!(fs::read_to_string(dir.join("target/debug/old.rs")).unwrap().contains("nav_link_value(1, 2)"));

    // the manifest now says 0.6: a second run has nothing to do
    let out = bunny(&["migrate", "--yes"], &dir);
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("already"), "{}", text(&out.stdout));
    assert_eq!(fs::read_to_string(dir.join("src/lib.rs")).unwrap(), AFTER);
}

#[test]
fn bunny_migrate_answers_studio_in_json() {
    let dir = block_05("json");
    fs::write(dir.join("src/nav.rs"), "fn f(p: NavigationPath) {}\n").unwrap();
    let out = bunny(&["migrate", "--json"], &dir);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON document");
    assert_eq!(answer["from"], "0.5");
    assert_eq!(answer["to"], "0.6");
    assert_eq!(answer["written"], false, "--json alone writes nothing");
    let files: Vec<&str> = answer["files"].as_array().unwrap().iter().map(|file| file["path"].as_str().unwrap()).collect();
    assert_eq!(files, ["Cargo.toml", "src/lib.rs"]);
    assert!(answer["files"][1]["diff"].as_str().unwrap().contains("+        .toolbar(toolbar_item("));
    let gap = &answer["gaps"][0];
    assert_eq!((gap["path"].as_str(), gap["line"].as_u64(), gap["column"].as_u64()), (Some("src/nav.rs"), Some(1), Some(9)));
    assert!(gap["note"].as_str().unwrap().contains("Vec"), "{gap}");

    let out = bunny(&["migrate", "--json", "--yes"], &dir);
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["written"], true);
    assert_eq!(fs::read_to_string(dir.join("src/lib.rs")).unwrap(), AFTER);
}

#[test]
fn bunny_migrate_needs_a_version_to_start_from() {
    let dir = scratch("versions");
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\nbunny-ui = { path = \"../bunny_ui\" }\n").unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "fn f() { nav_link_value(1, 2); }\n").unwrap();
    let out = bunny(&["migrate"], &dir);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--from"), "{}", text(&out.stderr));

    let out = bunny(&["migrate", "--from", "0.5", "--yes"], &dir);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(fs::read_to_string(dir.join("src/lib.rs")).unwrap(), "fn f() { nav_link_value(2, 1); }\n");
    assert!(text(&out.stdout).contains("names no version"), "{}", text(&out.stdout));

    let out = bunny(&["migrate", "--from", "0.5", "--to", "0.9"], &dir);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("0.6"), "names the newest it knows: {}", text(&out.stderr));
    let out = bunny(&["migrate", "--from", "five"], &dir);
    assert_eq!(out.status.code(), Some(2));
}
