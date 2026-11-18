//! Every folder of `fidelity/cases` with a `case.rs` becomes a module of
//! this crate and two entries of its wasm: `start_dom_<case>` boots the
//! view on elements, `start_gpu_<case>` on WebGL. A case is added by
//! adding its folder; nothing here names one.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"));
    let cases = manifest.join("../cases");
    println!("cargo::rerun-if-changed={}", cases.display());
    let mut names: Vec<String> = std::fs::read_dir(&cases)
        .expect("fidelity/cases")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("case.rs").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut out = String::new();
    for name in &names {
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "a case's folder is a Rust name: {name}"
        );
        let path = cases.join(name).join("case.rs");
        println!("cargo::rerun-if-changed={}", path.display());
        let _ = writeln!(out, "#[path = {:?}]\npub mod {name};", path.display().to_string());
        let _ = writeln!(
            out,
            "#[cfg(target_arch = \"wasm32\")]\n#[unsafe(no_mangle)]\npub extern \"C\" fn start_dom_{name}(width: f64, height: f64, scale: f64) {{\n    bunny_ui_web::start_dom(width, height, scale, {name}::view());\n}}"
        );
        let _ = writeln!(
            out,
            "#[cfg(target_arch = \"wasm32\")]\n#[unsafe(no_mangle)]\npub extern \"C\" fn start_gpu_{name}(width: f64, height: f64, scale: f64) {{\n    bunny_ui_web::start(width, height, scale, {name}::view());\n}}"
        );
    }
    let _ = writeln!(out, "/// Every case, by its folder's name.\npub const CASES: &[&str] = &{names:?};");
    let _ = writeln!(
        out,
        "/// Lays every case out in the pixel pipeline, by name.\npub fn laid_out(runtime: &bunny_ui::prelude::Runtime, size: bunny_ui::prelude::Size) -> Vec<(&'static str, Vec<String>)> {{\n    vec![{}]\n}}",
        names
            .iter()
            .map(|name| format!("({name:?}, named(runtime, &{name}::view(), size))"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let target = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets the out dir")).join("cases.rs");
    std::fs::write(target, out).expect("the generated cases");
}
