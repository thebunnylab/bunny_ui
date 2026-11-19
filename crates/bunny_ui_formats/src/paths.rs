//! The path of an archive entry, held inside the folder it unpacks to.

use std::path::{Component, Path, PathBuf};

/// The relative path of an entry, or `None` when it could land outside
/// the folder it unpacks to: an absolute path, a drive, or a `..`.
///
/// `/` and `\` both separate parts, because some Windows tools write
/// zips with `\`. `.` parts and empty parts drop out. A path with no
/// part left (the archive's own root) is `None` too.
#[must_use]
pub fn contained(entry: &str) -> Option<PathBuf> {
    if entry.starts_with('/') || entry.starts_with('\\') || entry.contains(':') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in entry.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            part => {
                // one part must stay one part on this system
                let mut components = Path::new(part).components();
                match (components.next(), components.next()) {
                    (Some(Component::Normal(_)), None) => out.push(part),
                    _ => return None,
                }
            }
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_stays_inside() {
        assert_eq!(contained("app/bin/app"), Some(PathBuf::from("app").join("bin").join("app")));
        assert_eq!(contained("./app//bin/"), Some(PathBuf::from("app").join("bin")));
        assert_eq!(contained(r"App\App.exe"), Some(PathBuf::from("App").join("App.exe")));
        for outside in ["/etc/passwd", "\\x", "../x", "a/../../x", "a/..", "C:/x", "c:x", "", ".", "./"] {
            assert_eq!(contained(outside), None, "{outside:?}");
        }
    }
}
