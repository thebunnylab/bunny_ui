//! The updater's part of `bunny build macos|windows|linux`: the package
//! signed, `<package>.sig` beside it, and a `latest.json` that lists it.
//!
//! It runs only when `Cargo.toml` has `[package.metadata.bunny.updater]`.
//! The private key comes from `--signing-key <file>` or
//! `BUNNY_SIGNING_KEY`. Without one, the build goes on and writes no
//! updater file. The signature covers the package's final bytes: on a
//! Mac, the disk image after notarization and stapling, which both
//! change it.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use bunny_ui_formats::base64;
use bunny_ui_formats::ed25519::SigningKey;
use bunny_ui_formats::sha::{self, Sha256};

use super::{Info, Options, Shipped};
use crate::devices::Platform;
use crate::error::{self, Error, Result};
use crate::project::Project;
use crate::term;
use crate::updater::manifest::{self, Asset, Manifest};
use crate::updater::{self, Config};

/// What a release build signs with, decided before it starts.
pub struct Release {
    config: Config,
    key: SigningKey,
    channel: String,
}

/// The release key and the channel, checked before the build: a wrong
/// key, channel or version fails now, not after the build. `None`: this
/// build writes no updater file, and the warning says why.
pub fn prepare(project: &Project, platform: Platform, options: &Options, key_file: Option<&str>) -> Result<Option<Release>> {
    let Some(config) = &project.updater else {
        if key_file.is_some() {
            return Err(Error::usage(format!("--signing-key: Cargo.toml has no {}", updater::TABLE)));
        }
        return Ok(None);
    };
    if !matches!(platform, Platform::Macos | Platform::Windows | Platform::Linux) {
        return Ok(None);
    }
    let channel = config.channel(std::env::var("BUNNY_CHANNEL").ok().as_deref())?;
    manifest::check_version(&project.version).map_err(|why| {
        Error::new(format!("the updater reads the version as SemVer: {why}")).hint("set it in Cargo.toml, or with --build-name 1.2.0")
    })?;
    if platform == Platform::Macos && !options.dmg {
        println!("{}", term::warn("--no-dmg: the updater downloads the disk image, so this build writes no .sig and no latest.json"));
        return Ok(None);
    }
    let from_env = std::env::var("BUNNY_SIGNING_KEY").ok();
    let Some(key) = updater::release_key(key_file.map(Path::new), from_env.as_deref(), config)? else {
        println!(
            "{}",
            term::warn("no signing key (BUNNY_SIGNING_KEY or --signing-key): this build writes no .sig and no latest.json, and no app updates to it")
        );
        return Ok(None);
    };
    if config.url.is_none() {
        println!("{}", term::warn(&format!("no `url` in {}: this build writes the .sig and no latest.json", updater::TABLE)));
    }
    Ok(Some(Release { config: config.clone(), key, channel }))
}

impl Release {
    /// Signs the package, writes `<package>.sig`, and writes
    /// `latest.json` in `out`.
    pub fn publish(&self, project: &Project, shipped: &Shipped, out: &Path, info: &mut Info) -> Result<()> {
        let path = &shipped.path;
        let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let signature = updater::sign_file(&self.key, path)?;
        // a signature that does not verify is never written
        let raw: [u8; 64] = base64::decode(&signature).and_then(|bytes| bytes.try_into().ok()).unwrap_or([0; 64]);
        let reader = fs::File::open(path).map_err(error::at(path))?;
        if !self.key.verifying_key().verify_reader(reader, &raw).map_err(error::at(path))? {
            return Err(Error::new(format!("the signature of {file} does not verify")));
        }
        let sig_name = format!("{file}.sig");
        fs::write(out.join(&sig_name), format!("{signature}\n")).map_err(error::at(out))?;
        info.file(&sig_name);
        info.field("channel", &self.channel);
        let size = fs::metadata(path).map_err(error::at(path))?.len();
        let sha256 = sha::file::<Sha256>(path).map_err(error::at(path))?;
        println!("{}", term::ok(&format!("Signed {file} for the updater")));
        let asset = Asset { url: String::new(), signature, format: shipped.format.to_string(), size: Some(size), sha256: Some(sha256) };
        if let Some(manifest) = manifest_for(&self.config, &project.version, &self.channel, shipped, asset, now()) {
            fs::write(out.join("latest.json"), manifest.to_json()).map_err(error::at(out))?;
            info.file("latest.json");
            println!("{}", term::ok(&format!("latest.json lists it for {}", shipped.targets.join(" and "))));
        }
        Ok(())
    }
}

/// The manifest of one build: each of its targets, all with the same
/// package. `None`: the table has no `url`, so there is nothing to
/// download from.
pub fn manifest_for(config: &Config, version: &str, channel: &str, shipped: &Shipped, asset: Asset, now: u64) -> Option<Manifest> {
    let template = config.url.as_deref()?;
    let file = shipped.path.file_name().unwrap_or_default().to_string_lossy();
    let asset = Asset { url: manifest::fill_url(template, version, &file, channel), ..asset };
    Some(Manifest {
        product: Some(config.product.clone()),
        version: version.to_string(),
        channel: channel.to_string(),
        pub_date: Some(manifest::rfc3339(now)),
        notes: None,
        platforms: shipped.targets.iter().map(|target| (target.to_string(), asset.clone())).collect(),
        min_supported: None,
    })
}

/// Now, in seconds — or `SOURCE_DATE_EPOCH`, for a build made again to
/// the same bytes.
fn now() -> u64 {
    if let Some(epoch) = std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|text| text.trim().parse().ok()) {
        return epoch;
    }
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
