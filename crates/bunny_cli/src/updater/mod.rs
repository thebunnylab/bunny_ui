//! The self-updater as `bunny` sees it: what
//! `[package.metadata.bunny.updater]` says, the keys every build of the
//! app carries, and the signature and the `latest.json` a release build
//! writes beside its package.
//!
//! ```toml
//! [package.metadata.bunny.updater]
//! product = "my-app"
//! feed = "https://dl.example.com/my-app/{channel}/latest.json"
//! pubkeys = ["keys/updater.pub"]
//! channel = "stable"
//! url = "https://dl.example.com/my-app/{version}/{file}"
//! ```
//!
//! The app reads the feed, the product, the keys and the channel at
//! compile time from `BUNNY_UPDATER_*` and `BUNNY_CHANNEL`. The private
//! key never goes into a build: `bunny build` reads it from
//! `BUNNY_SIGNING_KEY` or `--signing-key`, signs the package, and forgets
//! it.

pub mod manifest;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use bunny_ui_formats::base64;
use bunny_ui_formats::ed25519::{SigningKey, VerifyingKey};

use crate::args;
use crate::error::{self, Error, Result};
use crate::json::Value;

/// The table in `Cargo.toml`, for the messages.
pub const TABLE: &str = "[package.metadata.bunny.updater]";

/// The channels the release contract knows.
pub const CHANNELS: &[&str] = &["stable", "preview", "nightly"];

/// The keys the table accepts.
const KEYS: &[&str] = &["product", "feed", "server", "pubkeys", "channel", "url"];

/// Where the app asks for its updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feed {
    /// A `latest.json` on any static host. `{channel}` in the URL is
    /// filled in by the app.
    Static(String),
    /// A release server's base URL.
    Server(String),
}

/// `[package.metadata.bunny.updater]`, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The product id the feed and the manifest name.
    pub product: String,
    pub feed: Feed,
    /// The public key files, as written in `Cargo.toml`.
    pub pubkeys: Vec<String>,
    /// The same files, from the package's folder.
    pub pubkey_paths: Vec<PathBuf>,
    /// The channel of `Cargo.toml`. `BUNNY_CHANNEL` wins over it.
    pub channel: Option<String>,
    /// Where the package goes, for `latest.json`: `{version}` and
    /// `{file}` are filled in.
    pub url: Option<String>,
    /// The base64 of each public key, after [`Config::load_keys`].
    pub keys: Vec<String>,
}

fn table_error(message: impl std::fmt::Display) -> Error {
    Error::new(format!("Cargo.toml's {TABLE} {message}"))
}

impl Config {
    /// Reads the table from the package's metadata. `package` is the
    /// cargo package name, and `dir` is the package's folder. The key
    /// files are not read here: [`Config::load_keys`] reads them.
    pub fn parse(table: &Value, package: &str, dir: &Path) -> Result<Config> {
        let Value::Object(members) = table else {
            return Err(table_error("is not a table"));
        };
        for (key, _) in members {
            if !KEYS.contains(&key.as_str()) {
                let error = table_error(format!("has an unknown key `{key}`"));
                return Err(match args::nearest(key, KEYS.iter().copied()) {
                    Some(near) => error.hint(format!("did you mean `{near}`?")),
                    None => error.hint(format!("the keys are {}", KEYS.join(", "))),
                });
            }
        }
        let string = |key: &str| -> Result<Option<String>> {
            match table.get(key) {
                None => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.trim().to_string())),
                Some(_) => Err(table_error(format!("`{key}` is not a string"))),
            }
        };
        let product = match string("product")? {
            Some(product) => {
                check_product(&product).map_err(|why| table_error(format!("product \"{product}\": {why}")))?;
                product
            }
            None => {
                check_product(package).map_err(|why| {
                    table_error(format!("has no product, and the package name `{package}` is not one: {why}"))
                        .hint(format!("set it: product = \"{}\"", suggest_product(package)))
                })?;
                package.to_string()
            }
        };
        let feed = match (string("feed")?, string("server")?) {
            (Some(feed), None) => {
                check_url("feed", &feed)?;
                Feed::Static(feed)
            }
            (None, Some(server)) => {
                check_url("server", &server)?;
                Feed::Server(server)
            }
            (Some(_), Some(_)) => {
                return Err(table_error("has both `feed` and `server`")
                    .hint("keep one: `feed` is a static latest.json, `server` is a release server"));
            }
            (None, None) => {
                return Err(table_error("has no `feed` and no `server`")
                    .hint("add one: feed = \"https://dl.example.com/my-app/{channel}/latest.json\""));
            }
        };
        let pubkeys: Vec<String> = match table.get("pubkeys") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(|text| text.trim().to_string()))
                .collect::<Option<Vec<String>>>()
                .ok_or_else(|| table_error("`pubkeys` is not a list of paths"))?,
            Some(_) => return Err(table_error("`pubkeys` is not a list of paths")),
        };
        if pubkeys.is_empty() || pubkeys.iter().any(String::is_empty) {
            return Err(table_error("has no `pubkeys`: an app with no key installs no update")
                .hint("bunny signer generate, then pubkeys = [\"keys/updater.pub\"]"));
        }
        let channel = string("channel")?;
        if let Some(channel) = &channel {
            check_channel(channel).map_err(|why| table_error(format!("channel: {why}")))?;
        }
        let url = string("url")?;
        if let Some(url) = &url {
            check_url("url", url)?;
            if !url.contains("{file}") {
                return Err(table_error("`url` has no {file}")
                    .hint("url = \"https://dl.example.com/my-app/{version}/{file}\""));
            }
        }
        let pubkey_paths = pubkeys.iter().map(|path| dir.join(path)).collect();
        Ok(Config { product, feed, pubkeys, pubkey_paths, channel, url, keys: Vec::new() })
    }

    /// Reads each public key file. A file that is not the base64 of an
    /// Ed25519 public key is an error.
    pub fn load_keys(&mut self) -> Result<()> {
        let mut keys = Vec::new();
        for (written, path) in self.pubkeys.iter().zip(&self.pubkey_paths) {
            let text = std::fs::read_to_string(path).map_err(|error| {
                table_error(format!("pubkeys: {written}: {error}")).hint("bunny signer generate writes a key pair")
            })?;
            let text = text.trim();
            public_key(text).map_err(|why| table_error(format!("pubkeys: {written}: {why}")))?;
            keys.push(text.to_string());
        }
        self.keys = keys;
        Ok(())
    }

    /// The channel of this build: `BUNNY_CHANNEL` when it is set, else
    /// `channel` in `Cargo.toml`, else `stable`.
    pub fn channel(&self, from_env: Option<&str>) -> Result<String> {
        match from_env.map(str::trim).filter(|channel| !channel.is_empty()) {
            Some(channel) => {
                check_channel(channel).map_err(|why| Error::new(format!("BUNNY_CHANNEL: {why}")))?;
                Ok(channel.to_string())
            }
            None => Ok(self.channel.clone().unwrap_or_else(|| String::from("stable"))),
        }
    }

    /// What the app reads at compile time.
    pub fn env(&self, channel: &str) -> Vec<(String, OsString)> {
        let mut env = vec![(String::from("BUNNY_UPDATER_PRODUCT"), OsString::from(&self.product))];
        match &self.feed {
            Feed::Static(url) => env.push((String::from("BUNNY_UPDATER_FEED"), OsString::from(url))),
            Feed::Server(url) => env.push((String::from("BUNNY_UPDATER_SERVER"), OsString::from(url))),
        }
        env.push((String::from("BUNNY_UPDATER_PUBKEYS"), OsString::from(self.keys.join(","))));
        env.push((String::from("BUNNY_CHANNEL"), OsString::from(channel)));
        env
    }
}

/// The product id rule of the release contract: 1 to 32 characters of
/// `[a-z0-9-]`, with no `-` at the start or at the end.
pub fn check_product(product: &str) -> std::result::Result<(), String> {
    let shaped = !product.is_empty()
        && product.len() <= 32
        && product.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !product.starts_with('-')
        && !product.ends_with('-');
    if shaped { Ok(()) } else { Err(String::from("a product id is 1 to 32 characters of [a-z0-9-], with no '-' at an end")) }
}

/// A product id made from a package name: `Photo_Lab` gives `photo-lab`.
pub fn suggest_product(package: &str) -> String {
    let lower: String = package.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let trimmed: String = lower.trim_matches('-').chars().take(32).collect();
    let trimmed = trimmed.trim_end_matches('-');
    if trimmed.is_empty() { String::from("my-app") } else { trimmed.to_string() }
}

pub fn check_channel(channel: &str) -> std::result::Result<(), String> {
    if CHANNELS.contains(&channel) {
        Ok(())
    } else {
        Err(format!("\"{channel}\" is not a channel: {}", CHANNELS.join(", ")))
    }
}

/// The updater reaches https, http and file URLs, and nothing else.
fn check_url(key: &str, url: &str) -> Result<()> {
    let scheme = url.split_once("://").map(|(scheme, _)| scheme);
    match scheme {
        Some("https" | "http" | "file") => Ok(()),
        _ => Err(table_error(format!("`{key}` \"{url}\" is not an https, http or file URL"))),
    }
}

/// An Ed25519 public key from its base64.
pub fn public_key(text: &str) -> std::result::Result<VerifyingKey, String> {
    let bytes = base64::decode(text.trim()).ok_or_else(|| String::from("not base64"))?;
    let bytes: [u8; 32] =
        bytes.try_into().map_err(|bytes: Vec<u8>| format!("{} bytes, and a public key is 32", bytes.len()))?;
    VerifyingKey::from_bytes(&bytes).ok_or_else(|| String::from("not an Ed25519 public key (the point is not on the curve)"))
}

/// An Ed25519 private key from the base64 of its 32-byte seed.
pub fn signing_key(text: &str) -> std::result::Result<SigningKey, String> {
    let bytes = base64::decode(text.trim()).ok_or_else(|| String::from("not base64"))?;
    let seed: [u8; 32] =
        bytes.try_into().map_err(|bytes: Vec<u8>| format!("{} bytes, and a key seed is 32", bytes.len()))?;
    Ok(SigningKey::from_seed(&seed))
}

/// The private key of a release build: `--signing-key <file>`, else
/// `BUNNY_SIGNING_KEY`. `None`: there is none. The key must match one of
/// the public keys the app carries, or nothing could verify what it
/// signs.
pub fn release_key(file: Option<&Path>, from_env: Option<&str>, config: &Config) -> Result<Option<SigningKey>> {
    let key = match (file, from_env.map(str::trim).filter(|text| !text.is_empty())) {
        (Some(file), _) => {
            let text = std::fs::read_to_string(file).map_err(error::at(file))?;
            signing_key(&text).map_err(|why| Error::new(format!("--signing-key {}: {why}", file.display())))?
        }
        (None, Some(text)) => signing_key(text).map_err(|why| Error::new(format!("BUNNY_SIGNING_KEY: {why}")))?,
        (None, None) => return Ok(None),
    };
    let public = base64::encode(key.verifying_key().as_bytes());
    if !config.keys.contains(&public) {
        return Err(Error::new("the signing key matches none of the app's pubkeys: nothing could verify what it signs")
            .hint(format!("use the .key of one of {}, or add its .pub to pubkeys", config.pubkeys.join(", "))));
    }
    Ok(Some(key))
}

/// The base64 Ed25519 signature of the file's bytes.
pub fn sign_file(key: &SigningKey, path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(error::at(path))?;
    Ok(base64::encode(&key.sign(&bytes)))
}

#[cfg(test)]
mod config_tests;
