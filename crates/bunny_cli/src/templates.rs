//! The files `bunny new` writes, embedded in the binary: the app's own
//! (`Cargo.toml`, `src/`) and one folder per platform.
//!
//! A template is registered here by its path under `templates/`; the
//! test at the bottom fails on a file that sits there unregistered, so
//! nothing is shipped in the crate and silently left out of projects.
//! Two names stand in for what `cargo package` would drop: `*.tmpl`
//! (a `Cargo.toml` or a `.rs` with placeholders is not one cargo should
//! read) and `gitignore` (dotfiles), renamed on the way out.

use std::path::PathBuf;

/// One embedded file.
#[derive(Clone, Copy, Debug)]
pub struct Template {
    /// Its path under `templates/`, with `/`.
    pub path: &'static str,
    pub bytes: &'static [u8],
    /// Written executable (`gradlew`).
    pub exec: bool,
}

/// A folder of templates `bunny new` writes as a whole.
#[derive(Clone, Copy, Debug)]
pub struct Set {
    /// Its folder under `templates/`, and the project folder it becomes
    /// (`app` is the project's root).
    pub name: &'static str,
    /// Bumped whenever a file in the set changes: a project records the
    /// revision it was created at, and `bunny` knows what it is looking
    /// at.
    pub revision: u32,
    pub files: &'static [Template],
}

macro_rules! template {
    ($path:literal) => {
        Template {
            path: $path,
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/", $path)),
            exec: false,
        }
    };
    ($path:literal, exec) => {
        Template {
            path: $path,
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/", $path)),
            exec: true,
        }
    };
}

/// The project itself: rendered with the app's names at `new`.
pub const APP: Set = Set {
    name: "app",
    revision: 1,
    files: &[
        template!("app/Cargo.toml.tmpl"),
        template!("app/README.md.tmpl"),
        template!("app/gitignore"),
        template!("app/src/lib.rs.tmpl"),
        template!("app/src/main.rs.tmpl"),
    ],
};

pub const ANDROID: Set = Set {
    name: "android",
    revision: 1,
    files: &[
        template!("android/build.gradle.kts"),
        template!("android/gitignore"),
        template!("android/gradle.properties"),
        template!("android/gradle/wrapper/gradle-wrapper.jar"),
        template!("android/gradle/wrapper/gradle-wrapper.properties"),
        template!("android/gradlew", exec),
        template!("android/gradlew.bat"),
        template!("android/settings.gradle.kts"),
        template!("android/app/build.gradle.kts"),
        template!("android/app/src/main/AndroidManifest.xml"),
    ],
};

pub const IOS: Set = Set { name: "ios", revision: 1, files: &[template!("ios/Info.plist")] };

pub const MACOS: Set = Set { name: "macos", revision: 1, files: &[template!("macos/Info.plist")] };

pub const WEB: Set = Set { name: "web", revision: 1, files: &[template!("web/index.html")] };

/// The platform folders, in the order `bunny new` lists them.
pub const PLATFORMS: &[Set] = &[ANDROID, IOS, MACOS, WEB];

/// The platform folder named `name`.
pub fn platform(name: &str) -> Option<&'static Set> {
    PLATFORMS.iter().find(|set| set.name == name)
}

impl Template {
    /// Where the file lands, relative to the folder its set becomes:
    /// the set's prefix off, `.tmpl` off, `gitignore` back to
    /// `.gitignore`.
    pub fn output(&self, set: &Set) -> PathBuf {
        let inner = self.path.strip_prefix(set.name).and_then(|rest| rest.strip_prefix('/')).unwrap_or(self.path);
        let inner = inner.strip_suffix(".tmpl").unwrap_or(inner);
        let mut path = PathBuf::new();
        let mut parts = inner.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() && part == "gitignore" {
                path.push(".gitignore");
            } else {
                path.push(part);
            }
        }
        path
    }

    /// The text of a template that has placeholders; every one of those
    /// is UTF-8 (the jar is the only binary, and it has none).
    pub fn text(&self) -> &'static str {
        std::str::from_utf8(self.bytes).unwrap_or("")
    }
}

/// The name of the file each platform folder keeps its origin in.
pub const STAMP: &str = ".bunny-template";

/// The stamp of a platform folder as `new` wrote it: the template's
/// revision, the tool's version, and a hash per file — so a later
/// `bunny` tells a file the person changed from one they did not.
pub fn stamp(set: &Set) -> String {
    let mut out = format!(
        "# Written by `bunny new`. Keep it: `bunny` reads which template this folder\n\
         # came from, and which of its files you have changed since.\n\
         template {} {}\ncli {}\n",
        set.name,
        set.revision,
        env!("CARGO_PKG_VERSION")
    );
    for template in set.files {
        let path = template.output(set);
        out.push_str(&format!("file {:016x} {}\n", fnv64(template.bytes), path.display()).replace('\\', "/"));
    }
    out
}

/// FNV-1a over the bytes with `\r\n` read as `\n`: a checkout that
/// converted line endings is not a file someone edited.
pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte == b'\r' && bytes.get(index) == Some(&b'\n') {
            continue;
        }
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Every file under `templates/` is registered in a set — a file
    /// added there and forgotten here would ship in the crate and never
    /// reach a project.
    #[test]
    fn every_template_on_disk_is_registered() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let mut on_disk = Vec::new();
        walk(&root, &root, &mut on_disk);
        let registered: Vec<&str> =
            [APP].iter().chain(PLATFORMS).flat_map(|set| set.files.iter().map(|t| t.path)).collect();
        for path in &on_disk {
            assert!(registered.contains(&path.as_str()), "templates/{path} is not registered in templates.rs");
        }
        assert_eq!(on_disk.len(), registered.len(), "a registered template is missing from disk");
    }

    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.file_name().is_some_and(|name| name != "LICENSE") {
                out.push(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }

    #[test]
    fn outputs_drop_the_disguises() {
        assert_eq!(APP.files[0].output(&APP), Path::new("Cargo.toml"));
        assert_eq!(template!("app/gitignore").output(&APP), Path::new(".gitignore"));
        assert_eq!(template!("app/src/lib.rs.tmpl").output(&APP), Path::new("src/lib.rs"));
        assert_eq!(
            template!("android/app/src/main/AndroidManifest.xml").output(&ANDROID),
            Path::new("app/src/main/AndroidManifest.xml")
        );
    }

    #[test]
    fn line_endings_do_not_change_the_hash() {
        assert_eq!(fnv64(b"a\r\nb\r\n"), fnv64(b"a\nb\n"));
        assert_ne!(fnv64(b"a\nb\n"), fnv64(b"a\nc\n"));
        // a lone `\r` is content
        assert_ne!(fnv64(b"a\rb"), fnv64(b"ab"));
    }

    #[test]
    fn the_stamp_lists_every_file() {
        let stamp = stamp(&ANDROID);
        assert!(stamp.contains("template android 1\n"));
        assert!(stamp.contains(" gradlew\n") && stamp.contains(" app/src/main/AndroidManifest.xml\n"));
        assert_eq!(stamp.lines().filter(|line| line.starts_with("file ")).count(), ANDROID.files.len());
    }
}
