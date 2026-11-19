//! `bunny updater merge`: the `latest.json` of each platform's build,
//! joined into the one a static feed serves.

use std::path::Path;

use crate::args::{HELP, Matches, Opt};
use crate::error::{self, Error, Result};
use crate::term;
use crate::updater::manifest::{self, Manifest};

pub const SUMMARY: &str = "Join the latest.json of each platform into one";

pub const USAGE: &str = "bunny updater merge <FILE>... [-o FILE]";

pub const ABOUT: &str = "\
Each `bunny build macos|windows|linux` writes a latest.json for its own
platform. `merge` joins them into one manifest with every platform, ready
to upload next to the packages.

The product, the version and the channel must be the same in each file. A
platform in two files must have the same package in both. The notes and
min_supported are kept, and must agree where two files have them. The date
is the latest one.

Without -o, the manifest goes to standard output.";

pub const OPTIONS: &[Opt] = &[Opt::value("output", "FILE", "Write the manifest to this file").short('o'), HELP];

pub fn run(matches: &Matches) -> Result<()> {
    let files = match matches.positionals.split_first() {
        Some((action, files)) if action == "merge" => files,
        Some((other, _)) => {
            return Err(Error::usage(format!("`{other}` is not an action of `bunny updater`"))
                .hint("bunny updater merge a.json b.json -o latest.json"));
        }
        None => return Err(Error::usage("name the action: bunny updater merge a.json b.json -o latest.json")),
    };
    if files.is_empty() {
        return Err(Error::usage("name the manifests to merge: bunny updater merge a.json b.json -o latest.json"));
    }
    let merged = merge_files(files.iter().map(Path::new))?;
    let text = merged.to_json();
    match matches.value("output") {
        Some(out) => {
            let path = Path::new(out);
            std::fs::write(path, &text).map_err(error::at(path))?;
            let targets: Vec<&str> = merged.platforms.iter().map(|(target, _)| target.as_str()).collect();
            eprintln!("{} {} {}", term::ok("Wrote"), term::bold(out), term::dim(&format!("({})", targets.join(", "))));
        }
        None => print!("{text}"),
    }
    Ok(())
}

/// The manifests in `files`, read and merged.
pub fn merge_files<'a>(files: impl Iterator<Item = &'a Path>) -> Result<Manifest> {
    let mut manifests = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(path).map_err(error::at(path))?;
        let manifest = Manifest::parse(&text).map_err(|why| Error::new(format!("{}: {why}", path.display())))?;
        manifests.push((path.display().to_string(), manifest));
    }
    manifest::merge(&manifests).map_err(Error::new)
}
