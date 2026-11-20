//! The release contract of bunny-ui: what a release *is*, for everything
//! that publishes, serves or installs one.
//!
//! `bunny build` writes manifests in this shape. A release server can
//! flatten a manifest to one platform in this shape. The updater in the
//! app reads both. The contract is data only, with serde as its one
//! dependency, so a server can use it with no UI and no HTTP client.
//!
//! ## The wire
//!
//! - A [`ReleaseManifest`] is one version, with one [`ReleaseAsset`] for
//!   each platform. A static feed (a file on a bucket) serves it as it is.
//! - A [`ChannelPointer`] names the current version of a channel, and the
//!   lowest version the channel still supports.
//! - A [`PlatformRelease`] is a manifest projected to one platform: the
//!   `200` answer of a release server to one check. A `204` means "up to
//!   date".
//!
//! ## Production gotchas
//!
//! **[`ProductId`] is load-bearing.** One bucket can serve many apps. A
//! signature proves who published an artifact, not which app it is
//! for: the wrong app's artifact verifies correctly. Only
//! [`ReleaseManifest::is_for`] proves that the release is for this app,
//! and an updater must refuse a release before it downloads anything
//! when the product does not match.
//!
//! **The JSON is not signed, only the artifact is.** The `url`, the
//! `format` and `min_supported` come through TLS and the server, not
//! through the signature.

#![forbid(unsafe_code)]

mod version;

#[cfg(test)]
mod tests;

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeMap;

pub use version::Version;

/// The release track that a build follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Preview,
    Nightly,
}

impl Channel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Preview => "preview",
            Channel::Nightly => "nightly",
        }
    }

    /// The channel of a build setting such as `BUNNY_CHANNEL`. An unknown
    /// value and no value are both [`Channel::Stable`].
    #[must_use]
    pub fn from_build(value: Option<&str>) -> Channel {
        match value {
            Some("preview") => Channel::Preview,
            Some("nightly") => Channel::Nightly,
            _ => Channel::Stable,
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Channel {
    type Err = UpdateError;
    fn from_str(text: &str) -> Result<Channel, UpdateError> {
        match text {
            "stable" => Ok(Channel::Stable),
            "preview" => Ok(Channel::Preview),
            "nightly" => Ok(Channel::Nightly),
            other => Err(UpdateError::Parse(format!("unknown channel: {other:?}"))),
        }
    }
}

/// The operating-system half of a [`TargetTriple`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Os {
    Darwin,
    Windows,
    Linux,
}

impl Os {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Os::Darwin => "darwin",
            Os::Windows => "windows",
            Os::Linux => "linux",
        }
    }
}

impl FromStr for Os {
    type Err = UpdateError;
    fn from_str(text: &str) -> Result<Os, UpdateError> {
        match text {
            "darwin" | "macos" => Ok(Os::Darwin),
            "windows" | "win" => Ok(Os::Windows),
            "linux" => Ok(Os::Linux),
            other => Err(UpdateError::Parse(format!("unknown os: {other:?}"))),
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The CPU half of a [`TargetTriple`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Arch {
    Aarch64,
    X86_64,
}

impl Arch {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
        }
    }
}

impl FromStr for Arch {
    type Err = UpdateError;
    fn from_str(text: &str) -> Result<Arch, UpdateError> {
        match text {
            "aarch64" | "arm64" => Ok(Arch::Aarch64),
            "x86_64" | "x64" | "amd64" => Ok(Arch::X86_64),
            other => Err(UpdateError::Parse(format!("unknown arch: {other:?}"))),
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A platform (`"os-arch"`): the key into the `platforms` of a manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetTriple {
    pub os: Os,
    pub arch: Arch,
}

impl TargetTriple {
    #[must_use]
    pub const fn new(os: Os, arch: Arch) -> TargetTriple {
        TargetTriple { os, arch }
    }
}

impl fmt::Display for TargetTriple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

impl FromStr for TargetTriple {
    type Err = UpdateError;
    fn from_str(text: &str) -> Result<TargetTriple, UpdateError> {
        let (os, arch) = text
            .split_once('-')
            .ok_or_else(|| UpdateError::Parse(format!("invalid target {text:?}")))?;
        Ok(TargetTriple { os: os.parse()?, arch: arch.parse()? })
    }
}

impl serde::Serialize for TargetTriple {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for TargetTriple {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<TargetTriple, D::Error> {
        <std::borrow::Cow<'de, str>>::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom)
    }
}

/// The installer format of an artifact. It selects how the updater
/// puts the build in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    App,
    Dmg,
    Nsis,
    Wix,
    AppImage,
    #[serde(rename = "tar.gz")]
    TarGz,
    /// A zip of the app's folder: what `bunny build windows` writes.
    Zip,
}

/// Which app a release belongs to: 1 to 32 characters of `[a-z0-9-]`,
/// with no `-` at the start or the end.
///
/// The slug goes into a URL and into an object key, so it has a
/// grammar: a slug with `/` or `..` in it would address something else.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct ProductId(String);

impl ProductId {
    /// Parses a product slug.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Parse`] that names the rule that failed.
    pub fn parse(raw: impl AsRef<str>) -> Result<ProductId, UpdateError> {
        let raw = raw.as_ref().trim();
        let shaped = !raw.is_empty()
            && raw.len() <= 32
            && raw.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !raw.starts_with('-')
            && !raw.ends_with('-');
        if shaped {
            Ok(ProductId(raw.to_owned()))
        } else {
            Err(UpdateError::Parse(format!(
                "product {raw:?} must be 1-32 chars of [a-z0-9-], not starting or ending with '-'"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProductId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ProductId {
    type Err = UpdateError;
    fn from_str(raw: &str) -> Result<ProductId, UpdateError> {
        ProductId::parse(raw)
    }
}

/// Goes through [`ProductId::parse`], so a manifest cannot carry a slug
/// that addresses something other than itself.
impl<'de> serde::Deserialize<'de> for ProductId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<ProductId, D::Error> {
        let raw = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        ProductId::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// One downloadable artifact for a platform, with its Ed25519 signature.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseAsset {
    pub url: String,
    /// The base64 Ed25519 signature of the artifact's bytes.
    pub signature: String,
    pub format: Format,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The lowercase hex SHA-256 of the artifact's bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// The release of one version: one artifact for each platform.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReleaseManifest {
    /// Which app the release is for. A manifest with no product never
    /// matches: see [`ReleaseManifest::is_for`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<ProductId>,
    pub version: Version,
    pub channel: Channel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub platforms: BTreeMap<TargetTriple, ReleaseAsset>,
    /// The lowest version the channel still supports. A static feed has
    /// no channel pointer, so the manifest carries the floor itself. A
    /// build below it is retired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_supported: Option<Version>,
}

impl ReleaseManifest {
    #[must_use]
    pub fn asset_for(&self, target: TargetTriple) -> Option<&ReleaseAsset> {
        self.platforms.get(&target)
    }

    /// Whether the manifest is for `product`. A manifest with no product
    /// is for no product: that is the safe answer.
    #[must_use]
    pub fn is_for(&self, product: &ProductId) -> bool {
        self.product.as_ref() == Some(product)
    }

    /// The manifest projected to one platform: what a release server
    /// answers for one check. `None` when the platform has no artifact.
    #[must_use]
    pub fn project(&self, target: TargetTriple) -> Option<PlatformRelease> {
        let asset = self.asset_for(target)?;
        Some(PlatformRelease {
            product: self.product.clone(),
            version: self.version.clone(),
            url: asset.url.clone(),
            signature: asset.signature.clone(),
            format: asset.format,
            notes: self.notes.clone(),
            pub_date: self.pub_date.clone(),
            size: asset.size,
            sha256: asset.sha256.clone(),
            min_supported: self.min_supported.clone(),
        })
    }
}

/// The `channels/{channel}.json` pointer to the current version of a
/// channel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelPointer {
    pub version: Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_supported: Option<Version>,
}

/// A [`ReleaseManifest`] projected to one platform: the `200` answer to
/// one check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlatformRelease {
    /// Which app the release is for. The flat answer carries it too, so
    /// the client can refuse a mismatch itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<ProductId>,
    pub version: Version,
    pub url: String,
    pub signature: String,
    pub format: Format,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// The lowest version the channel still supports.
    ///
    /// It comes on a `200` beside the release, not as a status code: an
    /// installed build reads an unknown status as a transport error and
    /// shows nothing, so a `410` would never reach the builds it retires.
    /// A client that does not know the field ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_supported: Option<Version>,
}

impl PlatformRelease {
    /// The artifact of the release.
    #[must_use]
    pub fn asset(&self) -> ReleaseAsset {
        ReleaseAsset {
            url: self.url.clone(),
            signature: self.signature.clone(),
            format: self.format,
            size: self.size,
            sha256: self.sha256.clone(),
        }
    }
}

/// A build that the client can move to.
#[derive(Debug, Clone)]
pub struct Offer {
    pub version: Version,
    /// The artifact for this platform.
    pub asset: ReleaseAsset,
    pub notes: Option<String>,
}

/// The outcome of a check.
#[derive(Debug, Clone)]
pub enum UpdateStatus {
    /// Nothing newer, and this build is still supported.
    UpToDate,
    /// A newer build, to take when the user wants.
    Available(Offer),
    /// This build is below the floor of its channel: not an offer, a stop.
    Retired {
        /// The lowest version still supported.
        minimum: Version,
        /// The way out, when this platform has one. `None` means that
        /// there is nothing to install.
        upgrade: Option<Offer>,
    },
}

/// What an install produced, and what the caller must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// The bytes under this process changed: start the same executable
    /// path again to run the new build.
    ReadyOnRestart,
    /// A helper waits for this process to exit. It finishes the install
    /// and starts the app. Start nothing: two processes in one install
    /// folder is the failure this variant prevents.
    PendingHelper,
    /// The install is in place, but `current_exe()` does not name it any
    /// more. On Linux, a replaced running image keeps its old inode, and
    /// `/proc/self/exe` then ends in `" (deleted)"`. Start this path.
    RelaunchAt(std::path::PathBuf),
}

/// How much of an artifact has arrived.
///
/// `total` is `None` when no size is known. A size of zero is also
/// unknown, so [`Transfer::percent`] never divides by zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transfer {
    received: u64,
    total: Option<core::num::NonZeroU64>,
}

impl Transfer {
    #[must_use]
    pub fn new(received: u64, total: Option<u64>) -> Transfer {
        Transfer { received, total: total.and_then(core::num::NonZeroU64::new) }
    }

    #[must_use]
    pub const fn received(self) -> u64 {
        self.received
    }

    #[must_use]
    pub fn total(self) -> Option<u64> {
        self.total.map(core::num::NonZeroU64::get)
    }

    /// The whole percent received, at most 100. `None` while the size
    /// is unknown.
    #[must_use]
    pub fn percent(self) -> Option<u8> {
        self.total.map(|total| {
            let percent = u128::from(self.received) * 100 / u128::from(total.get());
            u8::try_from(percent.min(100)).unwrap_or(100)
        })
    }
}

/// Where an install is, between the click and [`Installed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallProgress {
    /// The artifact is downloading.
    Downloading(Transfer),
    /// All the bytes arrived. The hash and the signature are checked.
    Verifying,
    /// Verified. The build is put in place.
    Applying,
}

/// Why an update did not happen.
#[derive(Debug)]
pub enum UpdateError {
    Transport(String),
    Manifest(String),
    NoAssetForPlatform(TargetTriple),
    BadSignature,
    Integrity { expected: String, actual: String },
    Apply(String),
    NotBundled,
    Parse(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::Transport(why) => write!(f, "update transport error: {why}"),
            UpdateError::Manifest(why) => write!(f, "malformed release manifest: {why}"),
            UpdateError::NoAssetForPlatform(target) => write!(f, "no release asset for platform {target}"),
            UpdateError::BadSignature => f.write_str("signature verification failed"),
            UpdateError::Integrity { expected, actual } => {
                write!(f, "download integrity check failed (expected {expected}, got {actual})")
            }
            UpdateError::Apply(why) => write!(f, "applying the update failed: {why}"),
            UpdateError::NotBundled => f.write_str("not running from an installed bundle"),
            UpdateError::Parse(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for UpdateError {}

/// The platform this build was compiled for.
#[must_use]
pub const fn current_target() -> TargetTriple {
    #[cfg(target_os = "macos")]
    let os = Os::Darwin;
    #[cfg(target_os = "windows")]
    let os = Os::Windows;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let os = Os::Linux;

    #[cfg(target_arch = "aarch64")]
    let arch = Arch::Aarch64;
    #[cfg(not(target_arch = "aarch64"))]
    let arch = Arch::X86_64;

    TargetTriple::new(os, arch)
}

/// The version of the crate that **calls** the macro.
///
/// A function cannot do this: `env!` reads where it is written, so a
/// function here would always answer this crate's version. An app with
/// its own version would then ask for updates as this crate, and get
/// an offer of the build it already runs, with no error anywhere. A
/// macro expands in the caller.
///
/// # Panics
///
/// When the caller's version is not SemVer, which cargo does not allow.
#[macro_export]
macro_rules! current_version {
    () => {{
        let raw = env!("CARGO_PKG_VERSION");
        raw.parse::<$crate::Version>().expect("the calling crate's version is valid semver")
    }};
}

/// The channel the **calling** crate was built for: `BUNNY_CHANNEL` at
/// compile time, `stable` when it is not set or not known.
///
/// A macro for the same reason as [`current_version!`]: the build
/// setting must be read where the app is compiled.
#[macro_export]
macro_rules! current_channel {
    () => {
        $crate::Channel::from_build(option_env!("BUNNY_CHANNEL"))
    };
}
