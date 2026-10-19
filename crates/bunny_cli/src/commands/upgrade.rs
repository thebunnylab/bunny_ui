//! `bunny upgrade`: the project's platform folders brought up to the
//! templates of this `bunny`.
//!
//! Each folder `bunny new` wrote keeps a stamp (`.bunny-template`) with
//! the hash every file had then. A file that still has it was never
//! edited, and the new template's takes its place. A file that was
//! edited stays as it is, and the new template's lands next to it as
//! `<file>.bunny-new`, for the person to merge. A file the template adds
//! is written; one the person removed stays removed. The app's own files
//! — `src/`, `Cargo.toml` — are never touched.

use std::fs;
use std::path::{Path, PathBuf};

use crate::args::{HELP, Matches, Opt};
use crate::error::{self, Result};
use crate::project::Project;
use crate::templates::{self, PLATFORMS, STAMP, Set, Stamp, fnv64};
use crate::term;

pub const SUMMARY: &str = "Bring the project's platform folders up to this bunny's templates";

pub const USAGE: &str = "bunny upgrade [OPTIONS]";

pub const ABOUT: &str = "\
Updates the platform folders (android/, ios/, macos/, web/) to the templates
of this version of bunny. A file you never edited is replaced; a file you
edited stays yours, and the new template's version is written next to it as
<file>.bunny-new for you to merge. src/ and Cargo.toml are the app's and are
never touched.";

pub const OPTIONS: &[Opt] = &[
    Opt::flag("dry-run", "Say what would change, and change nothing"),
    Opt::value("package", "NAME", "The app to upgrade, in a workspace of several").short('p'),
    HELP,
];

/// What upgrading does to one file of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// It is the template's already.
    Same,
    /// Never edited: the new template's replaces it.
    Update,
    /// The template has it, the folder does not: it is written.
    Add,
    /// Edited by the person: it stays, the template's goes next to it.
    Yours,
    /// The person removed it: it stays removed.
    Removed,
    /// The template no longer has it: it stays, and is named.
    Dropped,
}

pub fn run(matches: &Matches) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project = Project::discover(&cwd, matches.value("package"))?;
    let dry = matches.flag("dry-run");
    let mut merges = 0;
    let mut found = false;
    for set in PLATFORMS {
        let folder = project.dir.join(set.name);
        if !folder.is_dir() {
            continue;
        }
        found = true;
        let stamp = fs::read_to_string(folder.join(STAMP)).ok().and_then(|text| Stamp::parse(&text));
        let steps = plan(&folder, set, stamp.as_ref())?;
        let from = stamp.as_ref().map_or_else(|| String::from("?"), |stamp| stamp.revision.to_string());
        if steps.iter().all(|(_, step)| *step == Step::Same) && stamp.as_ref().is_some_and(|stamp| stamp.revision == set.revision) {
            println!("{}", term::dim(&format!("{:<8} up to date (template {})", set.name, set.revision)));
            continue;
        }
        println!("{}", term::bold(&format!("{:<8} template {from} → {}", set.name, set.revision)));
        for (path, step) in &steps {
            let shown = path.display().to_string().replace('\\', "/");
            match step {
                Step::Same => {}
                Step::Update => println!("  {} {shown}", term::green("updated")),
                Step::Add => println!("  {} {shown}", term::green("added  ")),
                Step::Yours => {
                    merges += 1;
                    println!("  {} {shown} — you edited it: the template's is in {shown}.bunny-new", term::yellow("yours  "));
                }
                Step::Removed => println!("  {} {shown} — you removed it", term::dim("left   ")),
                Step::Dropped => println!("  {} {shown} — the template no longer has it", term::dim("left   ")),
            }
        }
        if !dry {
            apply(&folder, set, &steps)?;
        }
    }
    if !found {
        println!("{}", term::dim("no platform folders here: `bunny new . --platforms …` adds them"));
    } else if dry {
        println!("{}", term::dim("--dry-run: nothing was written"));
    } else if merges > 0 {
        println!("{}", term::dim("merge each .bunny-new into the file beside it, then delete it"));
    }
    Ok(())
}

/// What upgrading `folder` to `set` does to each file — the template's,
/// then those only the stamp knows.
pub fn plan(folder: &Path, set: &Set, stamp: Option<&Stamp>) -> Result<Vec<(PathBuf, Step)>> {
    let mut steps = Vec::new();
    for template in set.files {
        let relative = template.output(set);
        let key = relative.to_string_lossy().replace('\\', "/");
        let path = folder.join(&relative);
        let step = if !path.exists() {
            // a file the stamp lists was there once: the person took it out
            if stamp.and_then(|stamp| stamp.hash(&key)).is_some() { Step::Removed } else { Step::Add }
        } else {
            let now = fnv64(&fs::read(&path).map_err(error::at(&path))?);
            let then = stamp.and_then(|stamp| stamp.hash(&key));
            if now == fnv64(template.bytes) || then == Some(fnv64(template.bytes)) {
                // the template's already — or the template did not change
                // this file, and the person's edit is all there is
                Step::Same
            } else if then == Some(now) {
                Step::Update
            } else {
                Step::Yours
            }
        };
        steps.push((relative, step));
    }
    if let Some(stamp) = stamp {
        for (_, file) in &stamp.files {
            if !set.files.iter().any(|template| template.output(set).to_string_lossy().replace('\\', "/") == *file)
                && folder.join(file).exists()
            {
                steps.push((PathBuf::from(file), Step::Dropped));
            }
        }
    }
    Ok(steps)
}

/// Writes what `plan` decided, and the folder's new stamp.
fn apply(folder: &Path, set: &Set, steps: &[(PathBuf, Step)]) -> Result<()> {
    for template in set.files {
        let relative = template.output(set);
        let Some((_, step)) = steps.iter().find(|(path, _)| *path == relative) else { continue };
        let target = match step {
            Step::Update | Step::Add => folder.join(&relative),
            Step::Yours => {
                let mut name = relative.as_os_str().to_owned();
                name.push(".bunny-new");
                folder.join(name)
            }
            _ => continue,
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(error::at(parent))?;
        }
        fs::write(&target, template.bytes).map_err(error::at(&target))?;
        #[cfg(unix)]
        if template.exec && *step != Step::Yours {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).map_err(error::at(&target))?;
        }
    }
    // the stamp now says this revision: a file kept as the person's is
    // measured against the new template from here on
    let stamp = folder.join(STAMP);
    fs::write(&stamp, templates::stamp(set)).map_err(error::at(&stamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::ANDROID;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bunny-upgrade-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A folder as an older template wrote it: every file the template's,
    /// but one that the old template had otherwise, and its stamp saying so.
    fn written_by_an_older_template(dir: &Path, old: &str, old_bytes: &[u8]) -> Stamp {
        for template in ANDROID.files {
            let path = dir.join(template.output(&ANDROID));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, template.bytes).unwrap();
        }
        fs::write(dir.join(old), old_bytes).unwrap();
        let mut stamp = Stamp::parse(&templates::stamp(&ANDROID)).unwrap();
        stamp.revision = 1;
        for (hash, file) in &mut stamp.files {
            if file == old {
                *hash = fnv64(old_bytes);
            }
        }
        stamp
    }

    #[test]
    fn an_untouched_file_is_updated_and_an_edited_one_stays() {
        let dir = folder("plan");
        let stamp = written_by_an_older_template(&dir, ".gitignore", b"/build\n");
        let steps = plan(&dir, &ANDROID, Some(&stamp)).unwrap();
        let step = |path: &str| steps.iter().find(|(file, _)| file == Path::new(path)).map(|(_, step)| step.clone());
        assert_eq!(step(".gitignore"), Some(Step::Update), "the old template's, never edited");
        assert_eq!(step("gradlew"), Some(Step::Same));

        // the old template had another gradle file, and the person edited
        // theirs; they removed the manifest
        fs::write(dir.join("app/build.gradle.kts"), "// mine\n").unwrap();
        fs::remove_file(dir.join("app/src/main/AndroidManifest.xml")).unwrap();
        fs::write(dir.join("old.txt"), "from an old template").unwrap();
        let mut stamp = stamp;
        for (hash, file) in &mut stamp.files {
            if file == "app/build.gradle.kts" {
                *hash = fnv64(b"// the old template's\n");
            }
        }
        stamp.files.push((fnv64(b"from an old template"), String::from("old.txt")));
        let steps = plan(&dir, &ANDROID, Some(&stamp)).unwrap();
        let step = |path: &str| steps.iter().find(|(file, _)| file == Path::new(path)).map(|(_, step)| step.clone());
        assert_eq!(step("app/build.gradle.kts"), Some(Step::Yours));
        assert_eq!(step("app/src/main/AndroidManifest.xml"), Some(Step::Removed));
        assert_eq!(step("old.txt"), Some(Step::Dropped));

        apply(&dir, &ANDROID, &steps).unwrap();
        assert_eq!(fs::read_to_string(dir.join("app/build.gradle.kts")).unwrap(), "// mine\n", "the person's file stays");
        let theirs = ANDROID.files.iter().find(|t| t.path.ends_with("app/build.gradle.kts")).unwrap();
        assert_eq!(fs::read(dir.join("app/build.gradle.kts.bunny-new")).unwrap(), theirs.bytes);
        assert!(fs::read_to_string(dir.join(".gitignore")).unwrap().contains("key.properties"));
        assert!(!dir.join("app/src/main/AndroidManifest.xml").exists(), "a removed file stays removed");
        let written = Stamp::parse(&fs::read_to_string(dir.join(STAMP)).unwrap()).unwrap();
        assert_eq!(written.revision, ANDROID.revision);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_to_a_file_the_template_kept_is_left_alone() {
        let dir = folder("kept");
        let stamp = written_by_an_older_template(&dir, ".gitignore", b"/build\n");
        fs::write(dir.join("gradle.properties"), "org.gradle.caching=true\n").unwrap();
        let steps = plan(&dir, &ANDROID, Some(&stamp)).unwrap();
        assert!(steps.iter().any(|(file, step)| file == Path::new("gradle.properties") && *step == Step::Same));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_stamp_every_difference_is_the_persons() {
        let dir = folder("unstamped");
        written_by_an_older_template(&dir, ".gitignore", b"/build\n");
        let steps = plan(&dir, &ANDROID, None).unwrap();
        assert!(steps.iter().any(|(file, step)| file == Path::new(".gitignore") && *step == Step::Yours));
        let _ = fs::remove_dir_all(&dir);
    }
}
