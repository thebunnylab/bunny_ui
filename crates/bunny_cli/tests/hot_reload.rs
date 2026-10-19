//! Hot reload on this computer, the whole road without a window: a
//! project `bunny new` writes, built the way `bunny run` builds it hot —
//! the binary on the framework's shared library, and two generations of
//! the app's library, the second after an edit. The binary loads both
//! and draws its first view as text after each (`BUNNY_HOT_CHECK`), so a
//! machine with no screen proves the load, the entry and the swap.
//!
//! Slow — it builds the framework — so it runs on request:
//! `cargo test -p bunny-cli --test hot_reload -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf()
}

fn target_dir() -> PathBuf {
    repo().join("target/bunny-hot-e2e")
}

/// The host's triple and the folder of its standard library.
fn host() -> (String, PathBuf) {
    let rustc = |args: &[&str]| {
        let out = Command::new("rustc").args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let host = rustc(&["-vV"]).lines().find_map(|line| line.strip_prefix("host:")).unwrap().trim().to_string();
    let std_libs = Path::new(rustc(&["--print", "sysroot"]).trim()).join("lib/rustlib").join(&host).join("lib");
    (host, std_libs)
}

fn cargo(app: &Path, args: &[&str]) {
    let out = Command::new("cargo")
        .args(args)
        .current_dir(app)
        .env("CARGO_TARGET_DIR", target_dir())
        .output()
        .unwrap();
    assert!(out.status.success(), "cargo {args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
#[ignore = "builds the framework as a shared library; run with --ignored"]
fn two_generations_load_into_the_running_app() {
    let here = std::env::temp_dir().join(format!("bunny-hot-{}", std::process::id()));
    let _ = fs::remove_dir_all(&here);
    fs::create_dir_all(&here).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(["new", "hot_app", "--org", "io.bunny"])
        .arg("--bunny-ui-path")
        .arg(repo().join("crates/bunny_ui_facade"))
        .current_dir(&here)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let app = here.join("hot_app");
    let (host, std_libs) = host();
    let hot = ["--features", "bunny-ui/hot", "--target", host.as_str()];
    let generation = |salt: u32, into: &Path| {
        let salt = format!("metadata=bunny-salt-{salt}");
        cargo(&app, &[&["rustc", "--lib", "--crate-type", "cdylib"][..], &hot, &["--", "-C", salt.as_str()]].concat());
        let debug = target_dir().join(&host).join("debug");
        let built = [
            debug.join(format!("libhot_app.{}", std::env::consts::DLL_EXTENSION)),
            debug.join("hot_app.dll"),
        ];
        let built = built.iter().find(|path| path.is_file()).expect("the generation was built");
        fs::copy(built, into).unwrap();
    };

    cargo(&app, &[&["build", "--bin", "hot_app"][..], &hot].concat());
    let first = here.join(format!("generation-1.{}", std::env::consts::DLL_EXTENSION));
    generation(1, &first);
    let lib = app.join("src/lib.rs");
    let source = fs::read_to_string(&lib).unwrap();
    fs::write(&lib, source.replace("\"Hello from bunny-ui\"", "\"Hello again\"")).unwrap();
    let second = here.join(format!("generation-2.{}", std::env::consts::DLL_EXTENSION));
    generation(1, &second);

    let debug = target_dir().join(&host).join("debug");
    let variable = if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else if cfg!(windows) {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut paths = vec![std_libs, debug.join("deps"), debug.clone()];
    paths.extend(std::env::var_os(variable).iter().flat_map(std::env::split_paths));
    let binary = debug.join(format!("hot_app{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new(&binary)
        .env("BUNNY_HOT_GENERATION", &first)
        .env("BUNNY_HOT_CHECK", &second)
        .env(variable, std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report = format!("stdout:\n{stdout}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{report}");
    let first_draw = stdout.find("@@bunny-hot loaded 1").expect(&report);
    let second_draw = stdout.find("@@bunny-hot loaded 2").expect(&report);
    assert!(stdout[first_draw..second_draw].contains("Hello from bunny-ui"), "{report}");
    assert!(stdout[second_draw..].contains("Hello again"), "{report}");
    let _ = fs::remove_dir_all(&here);
}
