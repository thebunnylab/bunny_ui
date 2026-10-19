//! The host's tests resolve the preview entries they export by name, in
//! the test binary itself — and an executable exports no symbol unless its
//! link line says so. This asks for that on the TEST binaries alone.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target.as_str() {
        "macos" | "ios" => println!("cargo::rustc-link-arg-tests=-Wl,-export_dynamic"),
        "linux" | "android" => println!("cargo::rustc-link-arg-tests=-Wl,--export-dynamic"),
        _ => {}
    }
}
