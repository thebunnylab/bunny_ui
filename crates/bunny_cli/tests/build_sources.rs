//! Build provenance uses stable inputs and an acknowledged runtime generation.
use bunny_cli::hot::sources::BuildInputs;
use std::fs;
use std::path::PathBuf;

struct Project(PathBuf);
impl Project {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("bunny-sources-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/ui")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"fixture\"\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() { fixture::run(); }").unwrap();
        fs::write(root.join("src/lib.rs"), "bunny_ui::app!(home);").unwrap();
        fs::write(
            root.join("src/ui/card.rs"),
            "// café\nfn card() { opaque!(); }",
        )
        .unwrap();
        Self(root)
    }
    fn capture(&self) -> BuildInputs {
        BuildInputs::capture(std::slice::from_ref(&self.0)).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn stable_sources_include_executable_and_nested_component_text() {
    let project = Project::new("stable");
    let sources = project.capture().finish().unwrap().unwrap();
    assert_eq!(
        sources.files()[&project.0.join("src/main.rs")],
        "fn main() { fixture::run(); }"
    );
    assert_eq!(
        sources.files()[&project.0.join("src/ui/card.rs")],
        "// café\nfn card() { opaque!(); }"
    );
    assert!(!sources.files().contains_key(&project.0.join("Cargo.toml")));
}

#[test]
fn changed_sources_additions_removals_and_build_controls_refuse_provenance() {
    let project = Project::new("changed");
    for (file, text) in [
        ("src/lib.rs", "bunny_ui::app!(new_home);"),
        ("src/new.rs", "fn new() {}"),
        ("Cargo.toml", "[workspace]"),
    ] {
        let before = project.capture();
        fs::write(project.0.join(file), text).unwrap();
        assert!(before.finish().unwrap().is_none(), "{file}");
    }
    let before = project.capture();
    fs::remove_file(project.0.join("src/main.rs")).unwrap();
    assert!(before.finish().unwrap().is_none());
}

#[test]
fn reload_proof_rejects_inputs_that_only_a_restart_can_load() {
    let project = Project::new("reload");
    let started = project.capture();
    fs::write(project.0.join("src/lib.rs"), "bunny_ui::app!(new_home);").unwrap();
    let restart =
        |path: &std::path::Path| path.ends_with("main.rs") || path.ends_with("Cargo.toml");
    assert!(project.capture().unchanged_where(&started, restart));
    fs::write(project.0.join("src/main.rs"), "fn main() { changed(); }").unwrap();
    assert!(!project.capture().unchanged_where(&started, restart));
}

#[test]
fn missing_roots_and_unreadable_source_are_errors_not_empty_receipts() {
    let project = Project::new("invalid");
    assert!(BuildInputs::capture(&[project.0.join("missing")]).is_err());
    fs::write(project.0.join("src/ui/card.rs"), [0xff, 0xfe]).unwrap();
    assert!(BuildInputs::capture(std::slice::from_ref(&project.0)).is_err());
}
