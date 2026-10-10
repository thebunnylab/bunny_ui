//! The system libraries the Linux shell links with and loads, and the
//! command that installs them on this distribution.

use std::fs;
use std::path::PathBuf;

/// What the shell links with (its `#[link]`s): each needs the
/// development symlink `lib<name>.so`, which only the `-dev` package
/// ships. Paired with that package on Debian and Ubuntu (the list the
/// project's own container installs), Fedora and Arch.
pub const LINKED: &[(&str, &str, &str, &str)] = &[
    ("wayland-client", "libwayland-dev", "wayland-devel", "wayland"),
    ("wayland-cursor", "libwayland-dev", "wayland-devel", "wayland"),
    ("xkbcommon", "libxkbcommon-dev", "libxkbcommon-devel", "libxkbcommon"),
    ("xkbcommon-x11", "libxkbcommon-x11-dev", "libxkbcommon-x11-devel", "libxkbcommon-x11"),
    ("xcb", "libxcb1-dev", "libxcb-devel", "libxcb"),
    ("xcb-shm", "libxcb-shm0-dev", "libxcb-devel", "libxcb"),
    ("xcb-xfixes", "libxcb-xfixes0-dev", "libxcb-devel", "libxcb"),
    ("xcb-xkb", "libxcb-xkb-dev", "libxcb-devel", "libxcb"),
    ("dbus-1", "libdbus-1-dev", "dbus-devel", "dbus"),
    ("secret-1", "libsecret-1-dev", "libsecret-devel", "libsecret"),
    ("fontconfig", "libfontconfig-dev", "fontconfig-devel", "fontconfig"),
    ("freetype", "libfreetype-dev", "freetype-devel", "freetype2"),
    ("harfbuzz", "libharfbuzz-dev", "harfbuzz-devel", "harfbuzz"),
];

/// What the shell opens at run time: the GPU paths. Without them the
/// window still draws, on the CPU.
pub const LOADED: &[(&str, &str)] = &[
    ("libEGL.so.1", "libegl1"),
    ("libwayland-egl.so.1", "libwayland-egl1"),
    ("libvulkan.so.1", "libvulkan1"),
];

/// The folders the linker and the loader search on common distributions.
fn library_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/usr/local/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/lib/x86_64-linux-gnu",
        "/lib/aarch64-linux-gnu",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    for var in ["LIBRARY_PATH", "LD_LIBRARY_PATH"] {
        if let Some(value) = std::env::var_os(var) {
            dirs.extend(std::env::split_paths(&value));
        }
    }
    dirs
}

/// Whether a library file named `file` is in any searched folder.
pub fn has_library(file: &str) -> bool {
    library_dirs().iter().any(|dir| dir.join(file).exists())
}

/// The distribution family, from `/etc/os-release`: `debian`, `fedora`,
/// `arch`, or `None` for one this does not know.
pub fn family() -> Option<&'static str> {
    let text = fs::read_to_string("/etc/os-release").ok()?;
    family_of(&text)
}

pub fn family_of(os_release: &str) -> Option<&'static str> {
    let ids: Vec<String> = os_release
        .lines()
        .filter_map(|line| line.strip_prefix("ID=").or_else(|| line.strip_prefix("ID_LIKE=")))
        .flat_map(|value| value.trim_matches('"').split(' ').map(String::from).collect::<Vec<_>>())
        .collect();
    let has = |names: &[&str]| ids.iter().any(|id| names.contains(&id.as_str()));
    if has(&["debian", "ubuntu"]) {
        Some("debian")
    } else if has(&["fedora", "rhel", "centos"]) {
        Some("fedora")
    } else if has(&["arch"]) {
        Some("arch")
    } else {
        None
    }
}

/// The command that installs `packages` on `family`.
pub fn install_command(family: Option<&str>, packages: &[&str]) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for package in packages {
        if !unique.contains(package) {
            unique.push(package);
        }
    }
    let list = unique.join(" ");
    match family {
        Some("debian") => format!("sudo apt install pkg-config build-essential {list}"),
        Some("fedora") => format!("sudo dnf install pkgconf-pkg-config gcc {list}"),
        Some("arch") => format!("sudo pacman -S --needed pkgconf base-devel {list}"),
        _ => format!("install the development packages of: {list}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_family_comes_from_id_and_id_like() {
        assert_eq!(family_of("ID=ubuntu\nID_LIKE=debian\n"), Some("debian"));
        assert_eq!(family_of("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"), Some("fedora"));
        assert_eq!(family_of("ID=endeavouros\nID_LIKE=arch\n"), Some("arch"));
        assert_eq!(family_of("ID=nixos\n"), None);
    }

    #[test]
    fn the_install_line_lists_each_package_once() {
        let line = install_command(Some("debian"), &["libxcb1-dev", "libwayland-dev", "libwayland-dev"]);
        assert_eq!(line, "sudo apt install pkg-config build-essential libxcb1-dev libwayland-dev");
    }
}
