//! The facade's own tests and examples resolve the preview entries
//! `#[preview]` exports by name, in their own binary — and an executable
//! exports no symbol unless its link line says so. This asks for that on
//! the facade's TEST and EXAMPLE binaries alone: an application's build is
//! untouched, and `bunny preview` never needs it, since the host looks its
//! previews up in the generation it loads.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target.as_str() {
        "macos" | "ios" => {
            println!("cargo::rustc-link-arg-tests=-Wl,-export_dynamic");
            println!("cargo::rustc-link-arg-examples=-Wl,-export_dynamic");
        }
        "linux" | "android" => {
            println!("cargo::rustc-link-arg-tests=-Wl,--export-dynamic");
            println!("cargo::rustc-link-arg-examples=-Wl,--export-dynamic");
        }
        _ => {}
    }
}
