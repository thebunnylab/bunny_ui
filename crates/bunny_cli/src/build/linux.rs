//! `bunny build linux`: the app as a `.tar.gz` any distribution opens —
//! the binary, its `.desktop` entry, its icon and an `install.sh` that
//! puts them where the desktop looks — under `build/linux/`.
//!
//! The `.desktop` file is named by the app's id, and so are the icon and
//! `StartupWMClass`: it is the id the window gives Wayland and X11, and
//! the desktop matches the window to its entry by it. The libraries the
//! binary needs are read from it (`DT_NEEDED`) and named as each
//! distribution's packages, in the build's answer and the package's
//! README.

use std::fs;
use std::path::Path;

use super::{Info, Log, Options};
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

/// Builds the binary and its `.tar.gz` into `out`.
pub fn build(project: &Project, options: &Options, out: &Path, log: &mut Log, info: &mut Info) -> Result<()> {
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
    let mut entries = vec![
        tar::Entry::folder(&root),
        tar::Entry::folder(&format!("{root}/bin")),
        tar::Entry::file(&format!("{root}/bin/{bin}"), 0o755, binary),
        tar::Entry::folder(&format!("{root}/share/applications")),
        tar::Entry::file(&format!("{root}/share/applications/{id}.desktop"), 0o644, desktop(&project.name, bin, &id).into_bytes()),
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
    Ok(())
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
sed "s|^Exec=.*|Exec=$prefix/bin/{bin}|" "$here/share/applications/{id}.desktop" > "$prefix/share/applications/{id}.desktop"
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
    fn the_desktop_entry_names_the_window() {
        let entry = desktop("Notes", "notes", "io.bunny.notes");
        assert!(entry.contains("\nIcon=io.bunny.notes\n") && entry.contains("\nStartupWMClass=io.bunny.notes\n"));
    }
}
