//! The app `bunny` is pointed at: its package, its targets, and what
//! `[package.metadata.bunny]` says it is.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::cargo;
use crate::error::{Error, Result};
use crate::ids;
use crate::json::Value;

#[derive(Clone, Debug)]
pub struct Project {
    pub package: String,
    /// `1.4.0` — what people see as the version.
    pub version: String,
    pub manifest: PathBuf,
    /// The package's folder: where `android/`, `ios/`, `web/` live.
    pub dir: PathBuf,
    /// The workspace's root: where cargo reads the profiles from.
    pub workspace_root: PathBuf,
    pub target_dir: PathBuf,
    /// The binary `run` starts on the desktop and iOS.
    pub bin: Option<String>,
    pub has_lib: bool,
    /// The name people read.
    pub name: String,
    /// The reverse-DNS id, when the app has one.
    pub id: Option<String>,
    /// The build number (`CFBundleVersion`, `versionCode`).
    pub build: u64,
}

impl Project {
    /// The project in `dir` (or named by `package`, in a workspace).
    pub fn discover(dir: &Path, package: Option<&str>) -> Result<Project> {
        let metadata = cargo::metadata(dir, None)?;
        let target_dir = PathBuf::from(metadata.str_at(&["target_directory"]).unwrap_or("target"));
        let workspace_root = PathBuf::from(metadata.str_at(&["workspace_root"]).unwrap_or("."));
        let packages = metadata.get("packages").map(Value::as_array).unwrap_or_default();
        let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        let chosen = match package {
            Some(name) => packages.iter().find(|p| p.str_at(&["name"]) == Some(name)).ok_or_else(|| {
                Error::usage(format!("no package `{name}` in this workspace"))
            })?,
            None => {
                // the package whose folder holds the current one, the
                // deepest first; else the one bunny app of the workspace
                let mut holding: Vec<&Value> = packages
                    .iter()
                    .filter(|p| p.str_at(&["manifest_path"]).and_then(|m| Path::new(m).parent()).is_some_and(|d| absolute.starts_with(d)))
                    .collect();
                holding.sort_by_key(|p| std::cmp::Reverse(p.str_at(&["manifest_path"]).map_or(0, str::len)));
                let apps: Vec<&Value> = packages.iter().filter(|p| p.path(&["metadata", "bunny"]).is_some()).collect();
                match (holding.first().copied(), apps.as_slice()) {
                    (Some(found), _) => found,
                    (None, [one]) => *one,
                    _ => {
                        return Err(Error::usage("more than one package here; name the app with `-p`"));
                    }
                }
            }
        };
        Project::from_package(chosen, workspace_root, target_dir)
    }

    fn from_package(package: &Value, workspace_root: PathBuf, target_dir: PathBuf) -> Result<Project> {
        let name = package.str_at(&["name"]).unwrap_or_default().to_string();
        let manifest = PathBuf::from(package.str_at(&["manifest_path"]).unwrap_or_default());
        let dir = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
        let targets = package.get("targets").map(Value::as_array).unwrap_or_default();
        let kinds = |target: &Value| -> Vec<String> {
            target.get("kind").map(Value::as_array).unwrap_or_default().iter().filter_map(Value::as_str).map(String::from).collect()
        };
        let bins: Vec<String> = targets
            .iter()
            .filter(|target| kinds(target).iter().any(|kind| kind == "bin"))
            .filter_map(|target| target.str_at(&["name"]).map(String::from))
            .collect();
        let bin = package
            .str_at(&["default_run"])
            .map(String::from)
            .or_else(|| bins.iter().find(|bin| **bin == name).cloned())
            .or_else(|| bins.first().cloned());
        let has_lib = targets.iter().any(|target| kinds(target).iter().any(|kind| kind == "lib" || kind == "cdylib" || kind == "rlib"));
        let bunny = package.path(&["metadata", "bunny"]);
        let field = |key: &str| bunny.and_then(|table| table.str_at(&[key])).map(String::from);
        let app_name = field("name").unwrap_or_else(|| ids::display_name(&name));
        let id = field("id");
        if let Some(id) = &id {
            ids::check_app_id(id).map_err(|error| Error::new(format!("Cargo.toml's [package.metadata.bunny] id: {}", error.message)))?;
        }
        let build = bunny.and_then(|table| table.get("build")).and_then(Value::as_u64).unwrap_or(1);
        Ok(Project {
            version: package.str_at(&["version"]).unwrap_or("0.1.0").to_string(),
            package: name,
            manifest,
            dir,
            workspace_root,
            target_dir,
            bin,
            has_lib,
            name: app_name,
            id,
            build,
        })
    }

    /// Whether the workspace defines the web's shipping profile — cargo
    /// reads profiles from the root manifest only.
    pub fn has_web_profile(&self) -> bool {
        std::fs::read_to_string(self.workspace_root.join("Cargo.toml"))
            .is_ok_and(|manifest| manifest.lines().any(|line| line.trim() == "[profile.web]"))
    }

    /// The id, which a phone needs before anything is installed on it.
    pub fn require_id(&self) -> Result<&str> {
        self.id.as_deref().ok_or_else(|| {
            Error::new("the app has no id").hint(
                "add it to Cargo.toml:\n[package.metadata.bunny]\nid = \"com.yourcompany.my_app\"",
            )
        })
    }

    pub fn require_bin(&self) -> Result<&str> {
        self.bin.as_deref().ok_or_else(|| {
            Error::new(format!("{} has no binary to start", self.package))
                .hint("add src/main.rs with `fn main() { my_app::run() }`")
        })
    }

    /// The version as Apple reads it: up to three numbers, the rest
    /// (`-beta.2`, `+build`) left off.
    pub fn apple_version(&self) -> String {
        let core = self.version.split(['-', '+']).next().unwrap_or("0");
        let parts: Vec<&str> = core.split('.').take(3).collect();
        parts.join(".")
    }

    /// Where `bunny` keeps what it assembles for `platform`.
    pub fn out_dir(&self, platform: &str, variant: &str, release: bool) -> PathBuf {
        self.target_dir.join("bunny").join(platform).join(variant).join(if release { "release" } else { "debug" })
    }

    /// What every build of the app carries: who it is, read by
    /// `bunny_ui::app!` at compile time.
    pub fn build_env(&self) -> Vec<(String, OsString)> {
        let mut env = vec![(String::from("BUNNY_APP_NAME"), OsString::from(&self.name))];
        if let Some(id) = &self.id {
            env.push((String::from("BUNNY_APP_ID"), OsString::from(id)));
        }
        env
    }

    /// A platform folder of the project, or the way to add it.
    pub fn platform_dir(&self, platform: &str) -> Result<PathBuf> {
        let dir = self.dir.join(platform);
        if dir.is_dir() {
            return Ok(dir);
        }
        Err(Error::new(format!("{} has no {platform}/ folder", self.package))
            .hint(format!("add it: bunny new . --platforms {platform}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    fn package(text: &str) -> Project {
        Project::from_package(&json::parse(text).unwrap(), PathBuf::from("/w"), PathBuf::from("/t")).unwrap()
    }

    #[test]
    fn the_app_reads_from_its_package() {
        let project = package(
            r#"{"name":"photo_lab","version":"1.4.0-beta.2","manifest_path":"/a/photo_lab/Cargo.toml","default_run":null,
                "targets":[{"name":"photo_lab","kind":["lib"]},{"name":"photo_lab","kind":["bin"]},{"name":"tool","kind":["bin"]}],
                "metadata":{"bunny":{"name":"Photo Lab","id":"io.bunny.photo_lab","build":42}}}"#,
        );
        assert_eq!(project.bin.as_deref(), Some("photo_lab"));
        assert!(project.has_lib);
        assert_eq!(project.name, "Photo Lab");
        assert_eq!(project.id.as_deref(), Some("io.bunny.photo_lab"));
        assert_eq!(project.build, 42);
        assert_eq!(project.apple_version(), "1.4.0");
        assert_eq!(project.dir, PathBuf::from("/a/photo_lab"));
        assert_eq!(project.out_dir("ios", "iphonesimulator", false), PathBuf::from("/t/bunny/ios/iphonesimulator/debug"));
    }

    #[test]
    fn a_plain_package_gets_defaults() {
        let project = package(
            r#"{"name":"notes","version":"0.3.1","manifest_path":"/n/Cargo.toml",
                "targets":[{"name":"notes","kind":["bin"]}],"metadata":null}"#,
        );
        assert_eq!(project.name, "Notes");
        assert_eq!(project.id, None);
        assert_eq!(project.build, 1);
        assert!(project.require_id().is_err());
        assert!(!project.has_lib);
    }
}
