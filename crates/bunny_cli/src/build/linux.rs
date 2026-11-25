//! `bunny build linux`: the app as a `.tar.gz` any distribution opens —
//! the binary, its `.desktop` entry, its icon and an `install.sh` that
//! puts them where the desktop looks — under `build/linux/`.
//!
//! The `.desktop` file is the app's own `linux/app.desktop` when it has
//! one, else a plain one. It is named by the app's id, and so are the icon and
//! `StartupWMClass`: it is the id the window gives Wayland and X11, and
//! the desktop matches the window to its entry by it. The libraries the
//! binary needs are read from it (`DT_NEEDED`) and named as each
//! distribution's packages, in the build's answer and the package's
//! README.

use std::fs;
use std::path::Path;

use super::{Info, Log, Options, Shipped};
use crate::cargo::{self, Target};
use crate::error::{self, Error, Result};
use crate::formats::{deflate, elf, tar};
use crate::project::Project;
use crate::term;

/// The icon sizes the hicolor theme has folders for.
const HICOLOR: &[u32] = &[16, 22, 24, 32, 48, 64, 128, 256, 512];

/// What the build needs before it starts — checked before the last
/// package is cleared away.
pub fn check(project: &Project) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err(Error::new("a Linux app is built on Linux").hint("run `bunny build linux` on Linux"));
    }
    project.require_bin()?;
    Ok(())
}

/// Builds the binary and its `.tar.gz` into `out`. The `.tar.gz` is what
/// the updater downloads.
pub fn build(project: &Project, options: &Options, out: &Path, log: &mut Log, info: &mut Info) -> Result<Shipped> {
    check(project)?;
    let bin = project.require_bin()?;
    let built = cargo::build(&cargo::Build {
        manifest: project.manifest.clone(),
        package: project.package.clone(),
        what: Target::Bin(bin.to_string()),
        release: options.release,
        profile: None,
        target: None,
        features: options.features.clone(),
        env: project.build_env(),
        rustc_args: Vec::new(),
        quiet: false,
        diagnostics: cargo::Diagnostics::Render,
    })?;
    log.note(&format!("cargo built {}", built.artifact.display()));
    let binary = fs::read(&built.artifact).map_err(error::at(&built.artifact))?;
    fs::write(out.join(bin), &binary).map_err(error::at(out))?;
    set_executable(&out.join(bin))?;

    // what the binary needs, as each distribution installs it
    let needed = elf::needed(&binary).map_err(|why| Error::new(format!("{bin}: {why}")))?;
    let packages = Packages::of(&needed);
    log.note(&format!("{bin} needs {}", needed.join(" ")));
    info.field("needs", &needed.join(" "));
    info.field("debian", &packages.debian.join(" "));
    info.field("fedora", &packages.fedora.join(" "));
    info.field("arch", &packages.arch.join(" "));

    let id = project.id.clone().unwrap_or_else(|| project.package.clone());
    let root = format!("{}-{}", project.package, project.version);
    let entry_path = project.dir.join("linux").join("app.desktop");
    let entry = if entry_path.is_file() {
        let template = fs::read_to_string(&entry_path).map_err(error::at(&entry_path))?;
        log.note("linux/app.desktop is the desktop entry");
        own_desktop(&template, &project.name, bin, &id).map_err(Error::new)?
    } else {
        desktop(&project.name, bin, &id)
    };
    let mut entries = vec![
        tar::Entry::folder(&root),
        tar::Entry::folder(&format!("{root}/bin")),
        tar::Entry::file(&format!("{root}/bin/{bin}"), 0o755, binary),
        tar::Entry::folder(&format!("{root}/share/applications")),
        tar::Entry::file(&format!("{root}/share/applications/{id}.desktop"), 0o644, entry.into_bytes()),
    ];
    let icon_path = project.dir.join("linux").join("AppIcon.png");
    let icon = if icon_path.is_file() {
        let bytes = fs::read(&icon_path).map_err(error::at(&icon_path))?;
        let folder = icon_folder(&bytes).ok_or_else(|| Error::new("linux/AppIcon.png is not a PNG"))?;
        entries.push(tar::Entry::file(&format!("{root}/share/{folder}/{id}.png"), 0o644, bytes));
        Some(folder)
    } else {
        log.note("no linux/AppIcon.png: the desktop shows its own icon for the app");
        None
    };
    entries.push(tar::Entry::file(&format!("{root}/install.sh"), 0o755, install_script(bin, &id, icon.as_deref()).into_bytes()));
    entries.push(tar::Entry::file(&format!("{root}/README.txt"), 0o644, readme(&project.name, &packages).into_bytes()));
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    let archive_name = format!("{root}-linux-{arch}.tar.gz");
    let archive = tar::write(&entries).map_err(Error::new)?;
    fs::write(out.join(&archive_name), deflate::gzip(&archive)).map_err(error::at(out))?;
    info.file(bin);
    info.file(&archive_name);
    if !packages.debian.is_empty() {
        println!("{} {}", term::dim("needs, on Debian and Ubuntu:"), packages.debian.join(" "));
    }
    Ok(Shipped { path: out.join(&archive_name), format: "tar.gz", targets: vec![if arch == "aarch64" { "linux-aarch64" } else { "linux-x86_64" }] })
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(error::at(path))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// The packages that install what the binary needs, per distribution.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Packages {
    pub debian: Vec<&'static str>,
    pub fedora: Vec<&'static str>,
    pub arch: Vec<&'static str>,
    /// Libraries no table here names.
    pub unknown: Vec<String>,
}

impl Packages {
    pub fn of(needed: &[String]) -> Packages {
        let mut packages = Packages::default();
        for library in needed {
            match elf::package(library) {
                None => {}
                Some(("?", _, _)) => packages.unknown.push(library.clone()),
                Some((debian, fedora, arch)) => {
                    for (list, name) in [(&mut packages.debian, debian), (&mut packages.fedora, fedora), (&mut packages.arch, arch)] {
                        if !list.contains(&name) {
                            list.push(name);
                        }
                    }
                }
            }
        }
        packages
    }
}

/// The desktop entry (the freedesktop.org specification).
pub fn desktop(name: &str, bin: &str, id: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={bin}\nIcon={id}\nStartupWMClass={id}\nTerminal=false\nCategories=Utility;\n"
    )
}

/// The app's own desktop entry, `linux/app.desktop`, with the placeholders
/// `macos/Info.plist` takes: `@BUNNY_APP_NAME@`, `@BUNNY_EXECUTABLE@` and
/// `@BUNNY_BUNDLE_ID@`. It keeps the keys the launcher and the window need
/// to find each other.
pub fn own_desktop(template: &str, name: &str, bin: &str, id: &str) -> std::result::Result<String, String> {
    let entry = template
        .replace("@BUNNY_APP_NAME@", name)
        .replace("@BUNNY_EXECUTABLE@", bin)
        .replace("@BUNNY_BUNDLE_ID@", id);
    if !entry.trim_start().starts_with("[Desktop Entry]") {
        return Err(String::from("linux/app.desktop must start with [Desktop Entry]"));
    }
    for key in ["Type=", "Name=", "Exec="] {
        if !entry.lines().any(|line| line.starts_with(key)) {
            return Err(format!("linux/app.desktop needs a {key} line"));
        }
    }
    let mut entry = entry;
    if !entry.ends_with('\n') {
        entry.push('\n');
    }
    for (key, value) in [("Icon=", id), ("StartupWMClass=", id)] {
        if !entry.lines().any(|line| line.starts_with(key)) {
            entry.push_str(&format!("{key}{value}\n"));
        }
    }
    Ok(entry)
}

/// Where the icon goes under `share/`: the hicolor folder of its size,
/// or `pixmaps` for a size the theme has no folder for. `None`: not a PNG.
pub fn icon_folder(png: &[u8]) -> Option<String> {
    if png.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let width = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);
    Some(if width == height && HICOLOR.contains(&width) {
        format!("icons/hicolor/{width}x{width}/apps")
    } else {
        String::from("pixmaps")
    })
}

/// `install.sh`: the app into `~/.local` (or the prefix given), where the
/// desktop's launcher finds it.
fn install_script(bin: &str, id: &str, icon: Option<&str>) -> String {
    let icon_lines = icon.map_or_else(String::new, |folder| {
        format!("mkdir -p \"$prefix/share/{folder}\"\ncp \"$here/share/{folder}/{id}.png\" \"$prefix/share/{folder}/\"\n")
    });
    format!(
        r#"#!/bin/sh
# Installs the app for this user (~/.local), or under the prefix given:
#   ./install.sh            ./install.sh /usr/local   (with sudo)
set -e
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${{1:-$HOME/.local}}"
mkdir -p "$prefix/bin" "$prefix/share/applications"
cp "$here/bin/{bin}" "$prefix/bin/"
chmod 755 "$prefix/bin/{bin}"
sed "s|^Exec=[^ ]*|Exec=$prefix/bin/{bin}|" "$here/share/applications/{id}.desktop" > "$prefix/share/applications/{id}.desktop"
{icon_lines}command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$prefix/share/applications" || true
echo "installed {bin} in $prefix"
"#
    )
}

fn readme(name: &str, packages: &Packages) -> String {
    let mut text = format!("{name}\n\nRun bin/ in place, or install it for the launcher: ./install.sh\n");
    if !packages.debian.is_empty() {
        text.push_str(&format!(
            "\nIt needs these libraries, which most desktops already have:\n  Debian, Ubuntu:  sudo apt install {}\n  Fedora:          sudo dnf install {}\n  Arch:            sudo pacman -S {}\n",
            packages.debian.join(" "),
            packages.fedora.join(" "),
            packages.arch.join(" ")
        ));
    }
    if !packages.unknown.is_empty() {
        text.push_str(&format!("  and: {}\n", packages.unknown.join(" ")));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_libraries_become_each_distributions_packages() {
        let needed: Vec<String> = ["libwayland-client.so.0", "libc.so.6", "libglib-2.0.so.0", "libgio-2.0.so.0", "libodd.so.3"]
            .iter()
            .map(|name| name.to_string())
            .collect();
        let packages = Packages::of(&needed);
        assert_eq!(packages.debian, ["libwayland-client0", "libglib2.0-0"], "the C library left out, a package named once");
        assert_eq!(packages.fedora, ["libwayland-client", "glib2"]);
        assert_eq!(packages.unknown, ["libodd.so.3"]);
    }

    #[test]
    fn the_icon_goes_where_its_size_belongs() {
        let png = |side: u32| {
            let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            bytes.extend_from_slice(&side.to_be_bytes());
            bytes.extend_from_slice(&side.to_be_bytes());
            bytes
        };
        assert_eq!(icon_folder(&png(512)).as_deref(), Some("icons/hicolor/512x512/apps"));
        assert_eq!(icon_folder(&png(1024)).as_deref(), Some("pixmaps"));
        assert_eq!(icon_folder(b"GIF89a"), None);
    }

    #[test]
    fn an_own_desktop_entry_takes_the_placeholders_and_keeps_the_window_keys() {
        let template = "[Desktop Entry]\nType=Application\nName=@BUNNY_APP_NAME@\nExec=@BUNNY_EXECUTABLE@ %u\nMimeType=text/html;\n";
        let entry = own_desktop(template, "hop", "hop", "dev.hop.browser").expect("entry");
        assert!(entry.contains("\nName=hop\n") && entry.contains("\nExec=hop %u\n"));
        assert!(entry.contains("\nIcon=dev.hop.browser\n") && entry.contains("\nStartupWMClass=dev.hop.browser\n"));
        assert!(own_desktop("Name=x\n", "a", "a", "a").is_err());
        assert!(own_desktop("[Desktop Entry]\nType=Application\nName=a\n", "a", "a", "a").is_err());
    }

    #[test]
    fn the_install_script_keeps_the_arguments_of_exec() {
        assert!(install_script("hop", "dev.hop.browser", None).contains("s|^Exec=[^ ]*|Exec=$prefix/bin/hop|"));
    }

    #[test]
    fn the_desktop_entry_names_the_window() {
        let entry = desktop("Notes", "notes", "io.bunny.notes");
        assert!(entry.contains("\nIcon=io.bunny.notes\n") && entry.contains("\nStartupWMClass=io.bunny.notes\n"));
    }
}
