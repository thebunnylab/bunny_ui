//! The check and the download: ask the feed, then fetch, hash and verify
//! the artifact before anything installs it.

use std::path::{Path, PathBuf};

use bunny_ui_formats::sha::{self, Sha256};
use bunny_ui_release::{
    Format, InstallProgress, Offer, PlatformRelease, ReleaseAsset, ReleaseManifest, Transfer, UpdateError,
    UpdateStatus, Version,
};

use crate::config::{Feed, UpdaterConfig};
use crate::fetch::Fetch;

/// An updater: a configuration and a transport.
///
/// Each call blocks until it is done, so it runs on a thread of its own.
pub struct Updater<F: Fetch> {
    config: UpdaterConfig,
    fetch: F,
}

#[cfg(feature = "curl")]
impl Updater<crate::CurlFetch> {
    /// An updater that fetches through `curl`.
    #[must_use]
    pub fn new(config: UpdaterConfig) -> Updater<crate::CurlFetch> {
        Updater::with_fetch(config, crate::CurlFetch::default())
    }
}

impl<F: Fetch> Updater<F> {
    #[must_use]
    pub fn with_fetch(config: UpdaterConfig, fetch: F) -> Updater<F> {
        Updater { config, fetch }
    }

    #[must_use]
    pub fn config(&self) -> &UpdaterConfig {
        &self.config
    }

    #[cfg(test)]
    pub(crate) fn fetch_ref(&self) -> &F {
        &self.fetch
    }

    /// Asks the feed whether a newer build is there, or whether this build
    /// is retired. Nothing is downloaded.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Transport`] when the feed does not answer or answers
    /// with an error status, and [`UpdateError::Manifest`] when the answer
    /// is malformed or for another product or channel. The product is
    /// refused here, before a download: the signature of the wrong app's
    /// artifact verifies correctly.
    pub fn check(&self) -> Result<UpdateStatus, UpdateError> {
        match &self.config.feed {
            Feed::Server(base) => self.check_server(base),
            Feed::Static(template) => self.check_static(template),
        }
    }

    /// The URL of one check on a release server.
    #[must_use]
    pub fn server_url(&self, base: &str) -> String {
        let c = &self.config;
        format!(
            "{}/releases/{}/{}/{}/{}/{}",
            base.trim_end_matches('/'),
            c.product,
            c.channel,
            c.target.os,
            c.target.arch,
            c.current
        )
    }

    /// The URL of a static feed, with its names filled in.
    #[must_use]
    pub fn static_url(&self, template: &str) -> String {
        let c = &self.config;
        template
            .replace("{product}", c.product.as_str())
            .replace("{channel}", c.channel.as_str())
            .replace("{target}", &c.target.to_string())
            .replace("{os}", c.target.os.as_str())
            .replace("{arch}", c.target.arch.as_str())
            .replace("{version}", &c.current.to_string())
    }

    fn check_server(&self, base: &str) -> Result<UpdateStatus, UpdateError> {
        let reply = self.fetch.get(&self.server_url(base))?;
        match reply.status {
            204 => Ok(UpdateStatus::UpToDate),
            _ if reply.is_success() => {
                let remote: PlatformRelease =
                    serde_json::from_slice(&reply.body).map_err(|error| UpdateError::Manifest(error.to_string()))?;
                if remote.product.as_ref() != Some(&self.config.product) {
                    return Err(UpdateError::Manifest(format!(
                        "the release is for product {:?}, this app is {}",
                        remote.product.as_ref().map(|product| product.as_str()),
                        self.config.product
                    )));
                }
                Ok(evaluate(&self.config.current, remote))
            }
            status => Err(UpdateError::Transport(format!("release server returned {status}"))),
        }
    }

    fn check_static(&self, template: &str) -> Result<UpdateStatus, UpdateError> {
        let url = self.static_url(template);
        let reply = self.fetch.get(&url)?;
        if reply.status == 204 {
            return Ok(UpdateStatus::UpToDate);
        }
        if !reply.is_success() {
            return Err(UpdateError::Transport(format!("the feed returned {} for {url}", reply.status)));
        }
        let manifest: ReleaseManifest =
            serde_json::from_slice(&reply.body).map_err(|error| UpdateError::Manifest(error.to_string()))?;
        if !manifest.is_for(&self.config.product) {
            return Err(UpdateError::Manifest(format!(
                "the manifest is for product {:?}, this app is {}",
                manifest.product.as_ref().map(|product| product.as_str()),
                self.config.product
            )));
        }
        if manifest.channel != self.config.channel {
            return Err(UpdateError::Manifest(format!(
                "the manifest is for the {} channel, this app follows {}",
                manifest.channel, self.config.channel
            )));
        }
        // no artifact for this platform is no update for it
        Ok(manifest.project(self.config.target).map_or(UpdateStatus::UpToDate, |remote| {
            evaluate(&self.config.current, remote)
        }))
    }

    /// Downloads `asset`, checks its SHA-256 and its signature, and gives
    /// back the file ready to install. `progress` hears each stage, and the
    /// download each time its whole percent moves.
    ///
    /// The asset comes from [`Updater::check`], which refused it already
    /// when it was for another product.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Apply`] when the configuration has no key, before
    /// anything is downloaded. [`UpdateError::Transport`] when the download
    /// fails. [`UpdateError::Integrity`] or [`UpdateError::BadSignature`]
    /// when the bytes are not the ones signed: nothing is kept.
    pub fn download(
        &self,
        asset: &ReleaseAsset,
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<Verified, UpdateError> {
        if self.config.keys.is_empty() {
            return Err(UpdateError::Apply("the updater has no public key, so no artifact can verify".into()));
        }
        let staging = self.config.staging();
        std::fs::create_dir_all(&staging)
            .map_err(|error| UpdateError::Apply(format!("update folder {}: {error}", staging.display())))?;
        let partial = staging.join(format!(".download-{}.part", unique_write()));
        let outcome = self.download_to(asset, &partial, progress);
        if outcome.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        outcome
    }

    fn download_to(
        &self,
        asset: &ReleaseAsset,
        partial: &Path,
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<Verified, UpdateError> {
        let mut ticker = Ticker::default();
        let status = self.fetch.download(&asset.url, partial, &mut |arrived| {
            // the server's length when it gives one, else the manifest's size
            let arrived = Transfer::new(arrived.received(), arrived.total().or(asset.size));
            if ticker.moved(arrived) {
                progress(InstallProgress::Downloading(arrived));
            }
        })?;
        if !(200..300).contains(&status) {
            return Err(UpdateError::Transport(format!("release server returned {status} for {}", asset.url)));
        }
        progress(InstallProgress::Verifying);
        let digest = sha::file::<Sha256>(partial)
            .map_err(|error| UpdateError::Apply(format!("reading the download: {error}")))?;
        if let Some(expected) = asset.sha256.as_deref()
            && !digest.eq_ignore_ascii_case(expected)
        {
            return Err(UpdateError::Integrity { expected: expected.to_owned(), actual: digest });
        }
        verify_file(&self.config.keys, partial, &asset.signature)?;
        let path = self.config.staging().join(staged_name(&self.config.product, asset.format, &digest));
        std::fs::rename(partial, &path).map_err(|error| UpdateError::Apply(format!("keeping the download: {error}")))?;
        Ok(Verified { path, format: asset.format })
    }
}

/// An artifact whose signature verified. Only [`Updater::download`] makes
/// one, so nothing can install bytes that were not verified.
#[derive(Debug)]
#[must_use]
pub struct Verified {
    path: PathBuf,
    format: Format,
}

impl Verified {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The format the feed gave, never one guessed from the file name.
    #[must_use]
    pub fn format(&self) -> Format {
        self.format
    }
}

/// Whether one of `keys` signed all the bytes of the file at `path`.
///
/// # Errors
///
/// [`UpdateError::BadSignature`] when the signature is malformed or no key
/// verifies it, and [`UpdateError::Apply`] when the file cannot be read.
pub fn verify_file(
    keys: &[bunny_ui_formats::ed25519::VerifyingKey],
    path: &Path,
    signature: &str,
) -> Result<(), UpdateError> {
    let bytes = bunny_ui_formats::base64::decode(signature.trim()).ok_or(UpdateError::BadSignature)?;
    let signature: [u8; 64] = bytes.try_into().map_err(|_| UpdateError::BadSignature)?;
    for key in keys {
        let file = std::fs::File::open(path).map_err(|error| UpdateError::Apply(format!("reading the download: {error}")))?;
        let good = key
            .verify_reader(std::io::BufReader::new(file), &signature)
            .map_err(|error| UpdateError::Apply(format!("reading the download: {error}")))?;
        if good {
            return Ok(());
        }
    }
    Err(UpdateError::BadSignature)
}

/// The verdict for `current` from the release of its platform.
///
/// The floor comes before the newer-than test, and the order matters: a
/// build at the same version as the channel (a rollback) must still hear
/// that it is retired. With the test first, it would hear "up to date".
pub(crate) fn evaluate(current: &Version, remote: PlatformRelease) -> UpdateStatus {
    let retired = remote.min_supported.as_ref().is_some_and(|floor| current < floor);
    let newer = remote.version.is_newer_than(current);
    if !retired && !newer {
        return UpdateStatus::UpToDate;
    }
    let asset = remote.asset();
    let PlatformRelease { version, notes, min_supported, .. } = remote;
    let offer = Offer { version, asset, notes };
    if retired {
        return UpdateStatus::Retired {
            minimum: min_supported.unwrap_or_else(|| offer.version.clone()),
            upgrade: Some(offer),
        };
    }
    UpdateStatus::Available(offer)
}

/// Passes a transfer on only when what a person sees moves: the whole
/// percent, or the mebibytes when no size is known. A download comes in
/// thousands of small pieces, and each report repaints the bar.
#[derive(Default)]
pub(crate) struct Ticker {
    shown: Option<Shown>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Percent(u8),
    Mebibytes(u64),
}

impl Ticker {
    /// Whether `arrived` is worth a report. The first one always is: it
    /// turns "nothing yet" into a bar at zero.
    pub(crate) fn moved(&mut self, arrived: Transfer) -> bool {
        let shown = arrived.percent().map_or_else(|| Shown::Mebibytes(arrived.received() >> 20), Shown::Percent);
        let moved = self.shown != Some(shown);
        self.shown = Some(shown);
        moved
    }
}

/// The name of a kept artifact: the product, the start of its SHA-256
/// and the extension of its format. It says what the bytes are, never
/// where they came from: the last segment of a signed URL can be longer
/// than the 255 bytes a file name can have.
pub(crate) fn staged_name(product: &bunny_ui_release::ProductId, format: Format, digest: &str) -> String {
    format!("{product}-{}.{}", &digest[..16.min(digest.len())], extension(format))
}

/// The extension an installer of `format` knows a file by.
pub(crate) const fn extension(format: Format) -> &'static str {
    match format {
        Format::App => "app",
        Format::Dmg => "dmg",
        Format::Nsis => "exe",
        Format::Wix => "msi",
        Format::AppImage => "AppImage",
        Format::TarGz => "tar.gz",
        Format::Zip => "zip",
    }
}

/// A name no other write uses now: the process id and a count.
fn unique_write() -> String {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{n}", std::process::id())
}
