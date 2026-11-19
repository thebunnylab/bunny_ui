//! `bunny signer generate`: the updater's key pair. The `.pub` goes into
//! every build of the app and into git; the `.key` signs the releases
//! and stays out of git.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use bunny_ui_formats::base64;
use bunny_ui_formats::ed25519::SigningKey;

use crate::args::{HELP, Matches, Opt};
use crate::error::{self, Error, Result};
use crate::project::Project;
use crate::term;
use crate::updater;

pub const SUMMARY: &str = "Make the key pair that signs the app's updates";

pub const USAGE: &str = "bunny signer generate [OPTIONS]";

pub const ABOUT: &str = "\
Writes an Ed25519 key pair for the updater: <NAME>.pub, the public key, and
<NAME>.key, the private key, each as one line of base64. The app carries the
.pub, and checks each update against it. Commit the .pub. The .key signs the
releases: keep it secret, out of git (bunny adds it to .gitignore), and give
it to the release build as BUNNY_SIGNING_KEY or --signing-key.

Make the key pair once, before the first release. A new pair does not verify
what the old one signed, and the apps already out there carry the old .pub.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("out", "DIR", "The folder for the two files (default: keys)"),
    Opt::value("name", "NAME", "The name of the two files (default: updater)"),
    Opt::flag("force", "Write over a key pair that is there"),
    HELP,
];

pub fn run(matches: &Matches) -> Result<()> {
    match matches.positionals.as_slice() {
        [action] if action == "generate" => {}
        [] => return Err(Error::usage("name the action: bunny signer generate")),
        [other, ..] => {
            return Err(Error::usage(format!("`{other}` is not an action of `bunny signer`")).hint("bunny signer generate"));
        }
    }
    let cwd = std::env::current_dir()?;
    // the app's folder, when there is one: where the .gitignore and the
    // pubkeys paths of Cargo.toml start
    let project = Project::locate(&cwd, None).ok();
    let root = project.as_ref().map_or_else(|| cwd.clone(), |project| project.dir.clone());
    // a package name the release contract refuses as a product id needs
    // a product of its own
    let product = project
        .as_ref()
        .filter(|project| project.updater.is_none() && updater::check_product(&project.package).is_err())
        .map(|project| updater::suggest_product(&project.package));
    let out = match matches.value("out") {
        Some(dir) => cwd.join(dir),
        None => root.join("keys"),
    };
    let name = matches.value("name").unwrap_or("updater");
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(Error::usage(format!("`--name {name}`: a name, not a path")));
    }
    let key = SigningKey::generate().map_err(|error| Error::new(format!("no random bytes from the system: {error}")))?;
    let written = write_pair(&key, &out, name, matches.flag("force"))?;
    let shown = |path: &Path| path.strip_prefix(&cwd).unwrap_or(path).display().to_string();
    println!("{} {}", term::ok("Wrote"), term::bold(&shown(&written.public)));
    println!("{} {}", term::ok("Wrote"), term::bold(&shown(&written.private)));
    match ignore(&root, &written.private)? {
        Ignored::Added(line) => println!("{} {line} {}", term::ok("Added"), term::dim("to .gitignore")),
        Ignored::Already => {}
        Ignored::Outside => {
            println!("{}", term::warn(&format!("{} is outside the app's folder: keep it out of git", shown(&written.private))));
        }
    }
    let relative = written.public.strip_prefix(&root).unwrap_or(&written.public);
    println!("\n{}\n", term::bold("Add this to Cargo.toml, with your own URLs:"));
    let id = match (&product, &project) {
        (Some(product), _) => product.clone(),
        (None, Some(project)) => project.updater.as_ref().map_or_else(|| project.package.clone(), |config| config.product.clone()),
        (None, None) => String::from("my-app"),
    };
    println!("{}", snippet(&portable(relative), product.as_deref(), &id));
    if !project.as_ref().is_some_and(|project| project.updater_feature) {
        println!("\n{}\n", term::bold("And turn the updater on in the app's bunny-ui dependency:"));
        println!("bunny-ui = {{ version = \"…\", features = [\"updater\"] }}");
    }
    println!(
        "\n{}",
        term::dim(&format!(
            "Commit {}. On the machine that builds the releases, set BUNNY_SIGNING_KEY to the contents of {}.",
            shown(&written.public),
            shown(&written.private)
        ))
    );
    Ok(())
}

/// The two files of a key pair.
#[derive(Debug)]
pub struct Pair {
    pub public: PathBuf,
    pub private: PathBuf,
}

/// Writes `<name>.pub` and `<name>.key` in `dir`, each one line of
/// base64. The `.key` is readable by its owner alone. Without `force`, a
/// file that is there is an error, and nothing is written.
pub fn write_pair(key: &SigningKey, dir: &Path, name: &str, force: bool) -> Result<Pair> {
    let pair = Pair { public: dir.join(format!("{name}.pub")), private: dir.join(format!("{name}.key")) };
    if !force && let Some(there) = [&pair.public, &pair.private].into_iter().find(|path| path.exists()) {
        return Err(Error::new(format!("{} is there already", there.display()))
            .hint("a new key pair does not verify what the old one signed; --force writes over it"));
    }
    fs::create_dir_all(dir).map_err(error::at(dir))?;
    fs::write(&pair.public, format!("{}\n", base64::encode(key.verifying_key().as_bytes()))).map_err(error::at(&pair.public))?;
    write_secret(&pair.private, &format!("{}\n", base64::encode(key.seed())))?;
    Ok(pair)
}

#[cfg(unix)]
fn write_secret(path: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file =
        fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).map_err(error::at(path))?;
    // a file that was there keeps its mode through `open`
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(error::at(path))?;
    file.write_all(text.as_bytes()).map_err(error::at(path))
}

#[cfg(not(unix))]
fn write_secret(path: &Path, text: &str) -> Result<()> {
    let mut file = fs::File::create(path).map_err(error::at(path))?;
    file.write_all(text.as_bytes()).map_err(error::at(path))
}

/// What [`ignore`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Ignored {
    /// This line went into `.gitignore`.
    Added(String),
    /// A line of `.gitignore` covers the key already.
    Already,
    /// The key is outside `root`, where its `.gitignore` reaches.
    Outside,
}

/// Puts the private key's path into `root/.gitignore`, unless a line
/// there covers it already.
pub fn ignore(root: &Path, private: &Path) -> Result<Ignored> {
    let Ok(relative) = private.strip_prefix(root) else {
        return Ok(Ignored::Outside);
    };
    let line = format!("/{}", portable(relative));
    let path = root.join(".gitignore");
    let text = fs::read_to_string(&path).unwrap_or_default();
    let covered = text.lines().map(str::trim).any(|known| [line.as_str(), &line[1..], "*.key"].contains(&known));
    if covered {
        return Ok(Ignored::Already);
    }
    let mut out = text.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("# the updater's private key\n");
    out.push_str(&line);
    out.push('\n');
    fs::write(&path, out).map_err(error::at(&path))?;
    Ok(Ignored::Added(line))
}

/// A relative path with `/` between its parts, as `Cargo.toml` and
/// `.gitignore` write it on every system.
fn portable(path: &Path) -> String {
    path.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The table to paste into `Cargo.toml`, with a `product` line when the
/// package name is not one. `id` goes into the example URLs.
pub fn snippet(pubkey: &str, product: Option<&str>, id: &str) -> String {
    let mut text = format!("{}\n", updater::TABLE);
    if let Some(product) = product {
        text.push_str(&format!("product = \"{product}\"\n"));
    }
    text.push_str(&format!("feed = \"https://dl.example.com/{id}/{{channel}}/latest.json\"\n"));
    text.push_str(&format!("pubkeys = [\"{pubkey}\"]\n"));
    text.push_str(&format!("url = \"https://dl.example.com/{id}/{{version}}/{{file}}\""));
    text
}

#[cfg(test)]
#[path = "signer_tests.rs"]
mod tests;
