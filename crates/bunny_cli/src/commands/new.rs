//! `bunny new`: a project that runs on every platform, from one command.
//!
//! The project is a plain cargo package — `src/lib.rs` holds the app and
//! ends in `bunny_ui::app!`, `src/main.rs` calls `run()` — plus one
//! folder per platform with that platform's own files. The folders are
//! the person's from then on: `new` never writes over a file, and run in
//! an app that already exists it only adds the platforms missing.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::{HELP, Matches, Opt};
use crate::error::{self, Error, Result};
use crate::ids;
use crate::template::{self, Escape};
use crate::templates::{self, APP, PLATFORMS, STAMP, Set, Template};
use crate::term;

pub const SUMMARY: &str = "Create an app that runs on every platform";

pub const USAGE: &str = "bunny new <PATH> [OPTIONS]";

pub const ABOUT: &str = "\
Creates a bunny-ui app in PATH: a cargo package whose src/lib.rs holds the
app, and one folder per platform (android, ios, macos, web) with that
platform's own files, yours to edit. The folder's name is the crate's name.

Run inside an app that already exists, it adds the platform folders that are
missing and touches nothing else.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("name", "NAME", "The name people read (default: from the folder's name)"),
    Opt::value("org", "ORG", "The reverse-DNS prefix of the app's id (default: com.example)"),
    Opt::value("id", "ID", "The whole app id, instead of ORG.CRATE"),
    Opt::value("platforms", "LIST", "The platform folders to create (default: android,ios,macos,web)"),
    Opt::value("bunny-ui-path", "DIR", "Depend on a local bunny-ui checkout").hidden(),
    HELP,
];

/// Where the app's `bunny-ui` comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// crates.io, at the version this tool was released with — the CLI
    /// and the framework move in lockstep.
    Registry(&'static str),
    /// A checkout's facade crate, for the project's own tests.
    Path(PathBuf),
}

/// What `new` was asked for, read and checked before a file is written.
#[derive(Debug)]
pub struct Plan {
    pub dir: PathBuf,
    /// The app already exists: only platform folders are added.
    pub existing: bool,
    pub crate_name: String,
    pub app_name: String,
    pub id: String,
    pub platforms: Vec<&'static Set>,
    pub source: Source,
}

/// What `new` did.
#[derive(Debug, Default)]
pub struct Report {
    pub created: bool,
    pub added: Vec<&'static str>,
    /// Platform folders that were already there, left alone.
    pub kept: Vec<&'static str>,
}

pub fn run(matches: &Matches) -> Result<()> {
    let plan = plan(matches)?;
    let report = create(&plan)?;
    print(&plan, &report);
    Ok(())
}

/// Reads and checks the command line.
pub fn plan(matches: &Matches) -> Result<Plan> {
    let dir = match matches.positionals.as_slice() {
        [dir] => PathBuf::from(dir),
        [] => return Err(Error::usage("`bunny new` needs the folder to create").hint("bunny new my_app")),
        [_, extra, ..] => return Err(Error::usage(format!("unexpected argument `{extra}`"))),
    };
    let existing = is_bunny_app(&dir);
    if !existing && dir.exists() {
        if !dir.is_dir() {
            return Err(Error::new(format!("`{}` exists and is not a folder", dir.display())));
        }
        if fs::read_dir(&dir).map_err(error::at(&dir))?.next().is_some() {
            return Err(Error::new(format!("`{}` already exists and is not empty", dir.display())).hint(
                "pick a new folder — or, inside a bunny app, `bunny new .` adds the platforms it lacks",
            ));
        }
    }
    let absolute = std::path::absolute(&dir).map_err(error::at(&dir))?;
    let folder = absolute.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let crate_name = folder.clone();
    if !existing {
        ids::check_crate_name(&crate_name)?;
    }
    let app_name = match matches.value("name") {
        Some(name) if name.trim().is_empty() => return Err(Error::usage("`--name` cannot be empty")),
        Some(name) => name.trim().to_string(),
        None => ids::display_name(&crate_name),
    };
    let id = match (matches.value("id"), matches.value("org")) {
        (Some(_), Some(_)) => return Err(Error::usage("give `--id` or `--org`, not both")),
        (Some(id), None) => id.to_string(),
        (None, org) => {
            let org = org.unwrap_or(ids::DEFAULT_ORG);
            ids::check_org(org)?;
            ids::default_id(org, &crate_name)
        }
    };
    if !existing {
        ids::check_app_id(&id)?;
    }
    let platforms = match matches.value("platforms") {
        None => PLATFORMS.iter().collect(),
        Some(list) => {
            let mut chosen: Vec<&'static Set> = Vec::new();
            for name in list.split(',').map(str::trim).filter(|name| !name.is_empty()) {
                let Some(set) = templates::platform(name) else {
                    let known: Vec<&str> = PLATFORMS.iter().map(|set| set.name).collect();
                    return Err(Error::usage(format!("unknown platform `{name}`"))
                        .hint(format!("the platforms are: {}", known.join(", "))));
                };
                if !chosen.iter().any(|known| known.name == set.name) {
                    chosen.push(set);
                }
            }
            // the order the folders are listed in, whatever order was typed
            PLATFORMS.iter().filter(|set| chosen.iter().any(|c| c.name == set.name)).collect()
        }
    };
    let source = match matches.value("bunny-ui-path") {
        None => Source::Registry(env!("CARGO_PKG_VERSION")),
        Some(path) => Source::Path(facade_dir(Path::new(path))?),
    };
    Ok(Plan { dir, existing, crate_name, app_name, id, platforms, source })
}

/// A folder holding a bunny app: a `Cargo.toml` with the
/// `[package.metadata.bunny]` table `new` writes.
fn is_bunny_app(dir: &Path) -> bool {
    fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|manifest| manifest.contains("[package.metadata.bunny]"))
}

/// The facade crate of a checkout: the folder given, or its
/// `crates/bunny_ui_facade`.
fn facade_dir(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path).map_err(error::at(path))?;
    for candidate in [path.clone(), path.join("crates/bunny_ui_facade")] {
        let manifest = fs::read_to_string(candidate.join("Cargo.toml")).unwrap_or_default();
        if manifest.lines().any(|line| line.trim() == "name = \"bunny-ui\"") {
            return Ok(candidate);
        }
    }
    Err(Error::usage(format!("`{}` holds no bunny-ui crate", path.display())))
}

/// Writes the project — or, in an app that exists, the platform folders
/// it lacks.
pub fn create(plan: &Plan) -> Result<Report> {
    let mut report = Report::default();
    if !plan.existing {
        fs::create_dir_all(&plan.dir).map_err(error::at(&plan.dir))?;
        let (key, value) = match &plan.source {
            Source::Registry(version) => ("version", version.to_string()),
            Source::Path(path) => ("path", path.to_string_lossy().into_owned()),
        };
        let values = [
            ("CRATE_NAME", plan.crate_name.as_str()),
            ("APP_NAME", plan.app_name.as_str()),
            ("APP_ID", plan.id.as_str()),
            ("UI_KEY", key),
            ("UI_VALUE", value.as_str()),
        ];
        for file in APP.files {
            let name = format!("templates/{}", file.path);
            let text = template::render(&name, file.text(), &values, Escape::for_path(file.path))?;
            write_new(&plan.dir.join(file.output(&APP)), text.as_bytes(), false)?;
        }
        report.created = true;
    }
    for set in &plan.platforms {
        let folder = plan.dir.join(set.name);
        if folder.exists() {
            report.kept.push(set.name);
            continue;
        }
        for file in set.files {
            write_template(&folder, set, file)?;
        }
        write_new(&folder.join(STAMP), templates::stamp(set).as_bytes(), false)?;
        report.added.push(set.name);
    }
    Ok(report)
}

/// A platform file, as it is: its markers are filled on every build,
/// from whatever the app's `Cargo.toml` says then.
fn write_template(folder: &Path, set: &Set, file: &Template) -> Result<()> {
    write_new(&folder.join(file.output(set)), file.bytes, file.exec)
}

/// Writes a file that must not exist yet — `new` never writes over
/// anything.
fn write_new(path: &Path, bytes: &[u8], exec: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(error::at(parent))?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(error::at(path))?;
    file.write_all(bytes).map_err(error::at(path))?;
    #[cfg(unix)]
    if exec {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(error::at(path))?;
    }
    #[cfg(not(unix))]
    let _ = exec;
    Ok(())
}

/// The folder of the cargo workspace `dir` would fall inside, if any — a
/// package created there must be listed in it, or cargo refuses to build.
fn enclosing_workspace(dir: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(dir).ok()?;
    absolute.ancestors().skip(1).find(|ancestor| {
        fs::read_to_string(ancestor.join("Cargo.toml"))
            .is_ok_and(|manifest| manifest.lines().any(|line| line.trim() == "[workspace]"))
    }).map(Path::to_path_buf)
}

fn print(plan: &Plan, report: &Report) {
    let shown = plan.dir.display();
    if report.created {
        println!("{}", term::ok(&format!("Created {} ({})", term::bold(&plan.crate_name), plan.app_name)));
        println!("    id         {}", plan.id);
        let apple = ids::apple_id(&plan.id);
        let android = ids::android_id(&plan.id);
        if apple != plan.id || android != plan.id {
            println!("               {}", term::dim(&format!("iOS and macOS: {apple}  ·  Android: {android}")));
        }
    }
    if !report.added.is_empty() {
        println!("    platforms  {}", report.added.join(", "));
    }
    if !report.kept.is_empty() {
        println!("    kept       {} {}", report.kept.join(", "), term::dim("(already there)"));
    }
    if !report.created && report.added.is_empty() {
        println!("{}", term::ok(&format!("{shown} already has every platform asked for")));
        return;
    }
    if report.created && plan.id.starts_with(&format!("{}.", ids::DEFAULT_ORG)) {
        println!();
        println!(
            "{}",
            term::warn(&format!(
                "{} is a placeholder: set `id` in Cargo.toml (or pass `--org`) before the app goes to a store",
                ids::DEFAULT_ORG
            ))
        );
    }
    if report.created {
        if let Some(workspace) = enclosing_workspace(&plan.dir) {
            println!(
                "{}",
                term::warn(&format!(
                    "{shown} sits inside the cargo workspace at {}: list it in that workspace's `members`, \
                     or give its Cargo.toml an empty `[workspace]` table",
                    workspace.display()
                ))
            );
        }
        println!();
        println!("Next:");
        println!("    cd {shown}");
        println!("    bunny run          {}", term::dim("the app, on this computer"));
        println!("    bunny run -d ios   {}", term::dim("in the iOS Simulator"));
        println!("    bunny run -d web   {}", term::dim("in the browser"));
        println!("    bunny doctor       {}", term::dim("what each platform still needs on this machine"));
    }
}
