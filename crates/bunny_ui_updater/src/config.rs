//! What an updater asks, where, and which keys it trusts.

use std::path::PathBuf;

use bunny_ui_formats::ed25519::VerifyingKey;
use bunny_ui_release::{Channel, ProductId, TargetTriple, UpdateError, Version};

/// Where the updater asks for releases.
///
/// The address is fixed when the app is built, never read from a setting:
/// an address that a user can change is an address that an attacker can
/// change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feed {
    /// A release server that answers one check with one platform:
    /// `GET {base}/releases/{product}/{channel}/{os}/{arch}/{version}`,
    /// `200` with a `PlatformRelease` or `204` when up to date.
    Server(String),
    /// A manifest file, as on a bucket or in the assets of a release.
    /// The URL can contain `{product}`, `{channel}`, `{target}`, `{os}`,
    /// `{arch}` and `{version}`, which the updater fills in.
    Static(String),
}

/// The settings of one updater.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    pub feed: Feed,
    /// Which app this is. A release for another product is refused before
    /// the download.
    pub product: ProductId,
    pub channel: Channel,
    /// The version that runs now.
    pub current: Version,
    pub target: TargetTriple,
    /// The public keys an artifact can be signed with. The artifact is
    /// accepted when any key verifies it, so the signing key can change
    /// without a build that no longer updates.
    pub keys: Vec<VerifyingKey>,
    /// Where downloads wait before they are installed, and where the log
    /// goes.
    pub cache: PathBuf,
}

impl UpdaterConfig {
    /// A configuration for the stable channel on this platform, with no
    /// key yet and the system's cache folder of the product.
    #[must_use]
    pub fn new(feed: Feed, product: ProductId, current: Version) -> UpdaterConfig {
        let cache = default_cache(&product);
        UpdaterConfig {
            feed,
            product,
            channel: Channel::Stable,
            current,
            target: bunny_ui_release::current_target(),
            keys: Vec::new(),
            cache,
        }
    }

    #[must_use]
    pub fn channel(mut self, channel: Channel) -> UpdaterConfig {
        self.channel = channel;
        self
    }

    #[must_use]
    pub fn cache(mut self, cache: impl Into<PathBuf>) -> UpdaterConfig {
        self.cache = cache.into();
        self
    }

    #[must_use]
    pub fn key(mut self, key: VerifyingKey) -> UpdaterConfig {
        self.keys.push(key);
        self
    }

    /// Adds a public key in its text form: the base64 of 32 bytes, as a
    /// `.pub` file of `bunny signer generate` holds it.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Parse`] when the text is not a public key. A key
    /// that the build holds but cannot read is a release nobody can
    /// install, so it is an error and never left out in silence.
    pub fn key_base64(self, text: &str) -> Result<UpdaterConfig, UpdateError> {
        Ok(self.key(parse_key(text)?))
    }

    /// The folder for the files that wait to be installed.
    #[must_use]
    pub fn staging(&self) -> PathBuf {
        self.cache.join("updates")
    }

    /// The log of the failures, one line each.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.cache.join("update.log")
    }
}

/// A public key from the base64 text of its 32 bytes.
///
/// # Errors
///
/// [`UpdateError::Parse`] that says what is wrong with the text.
pub fn parse_key(text: &str) -> Result<VerifyingKey, UpdateError> {
    let bytes = bunny_ui_formats::base64::decode(text.trim())
        .ok_or_else(|| UpdateError::Parse("the public key is not base64".into()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| UpdateError::Parse(format!("the public key is {} bytes, not 32", bytes.len())))?;
    VerifyingKey::from_bytes(&bytes).ok_or_else(|| UpdateError::Parse("the public key is not a point of the curve".into()))
}

/// The cache folder of `product` on this system:
///
/// - macOS: `~/Library/Caches/<product>`
/// - Windows: `%LOCALAPPDATA%\<product>`
/// - Linux: `$XDG_CACHE_HOME/<product>`, else `~/.cache/<product>`
#[must_use]
pub fn default_cache(product: &ProductId) -> PathBuf {
    system_cache().unwrap_or_else(std::env::temp_dir).join(product.as_str())
}

fn system_cache() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library").join("Caches"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA")
    } else {
        var("XDG_CACHE_HOME").filter(|path| path.is_absolute()).or_else(|| var("HOME").map(|home| home.join(".cache")))
    }
}
