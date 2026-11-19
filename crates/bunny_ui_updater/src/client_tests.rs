use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bunny_ui_formats::base64;
use bunny_ui_formats::ed25519::SigningKey;
use bunny_ui_release::{
    Arch, Channel, Format, InstallProgress, Os, ProductId, ReleaseAsset, TargetTriple, Transfer, UpdateError,
    UpdateStatus, Version,
};

use crate::client::{Ticker, evaluate, staged_name, verify_file};
use crate::{Feed, Fetch, HttpReply, Updater, UpdaterConfig};

fn product() -> ProductId {
    ProductId::parse("app").unwrap()
}

fn v(text: &str) -> Version {
    text.parse().unwrap()
}

const MAC: TargetTriple = TargetTriple::new(Os::Darwin, Arch::Aarch64);

/// A folder of its own for each test, empty at the start.
fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("bunny-updater-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_seed(&[seed; 32])
}

fn signature(key: &SigningKey, bytes: &[u8]) -> String {
    base64::encode(&key.sign(bytes))
}

fn config(feed: Feed, cache: &Path) -> UpdaterConfig {
    let mut config = UpdaterConfig::new(feed, product(), v("1.0.0")).cache(cache).key(key(1).verifying_key());
    config.target = MAC;
    config
}

/// A transport with one answer to `get` and one body to download, which
/// arrives in pieces of `chunk` bytes. It keeps every URL it was asked.
#[derive(Default)]
struct Canned {
    get: Mutex<Option<Result<HttpReply, String>>>,
    download: Option<(u16, Vec<u8>)>,
    chunk: usize,
    length: Option<u64>,
    urls: Mutex<Vec<String>>,
}

impl Canned {
    fn answering(status: u16, body: &str) -> Canned {
        Canned { get: Mutex::new(Some(Ok(HttpReply::new(status, body)))), ..Canned::default() }
    }

    fn serving(status: u16, body: &[u8]) -> Canned {
        Canned { download: Some((status, body.to_vec())), chunk: usize::MAX, ..Canned::default() }
    }

    fn asked(&self) -> Vec<String> {
        self.urls.lock().unwrap().clone()
    }
}

impl Fetch for Canned {
    fn get(&self, url: &str) -> Result<HttpReply, UpdateError> {
        self.urls.lock().unwrap().push(url.to_owned());
        match self.get.lock().unwrap().clone() {
            Some(Ok(reply)) => Ok(reply),
            Some(Err(why)) => Err(UpdateError::Transport(why)),
            None => panic!("nothing to answer {url}"),
        }
    }

    fn download(&self, url: &str, to: &Path, received: &mut dyn FnMut(Transfer)) -> Result<u16, UpdateError> {
        self.urls.lock().unwrap().push(url.to_owned());
        let Some((status, body)) = &self.download else {
            return Err(UpdateError::Transport("refused".into()));
        };
        let mut written = Vec::new();
        received(Transfer::new(0, self.length));
        for piece in body.chunks(self.chunk.max(1)) {
            written.extend_from_slice(piece);
            received(Transfer::new(written.len() as u64, self.length));
        }
        std::fs::write(to, &written).unwrap();
        Ok(*status)
    }
}

fn release_json(product: &str, version: &str, floor: Option<&str>) -> String {
    let floor = floor.map(|floor| format!(r#","min_supported":"{floor}""#)).unwrap_or_default();
    format!(
        r#"{{"product":"{product}","version":"{version}","url":"https://dl/app.dmg","signature":"c2ln","format":"dmg","notes":"fixes"{floor}}}"#
    )
}

fn release(version: &str, floor: Option<&str>) -> bunny_ui_release::PlatformRelease {
    serde_json::from_str(&release_json("app", version, floor)).unwrap()
}

// ---- the check on a release server ----

#[test]
fn the_server_url_is_the_product_channel_os_arch_version_path() {
    let cache = folder("server-url");
    let config = config(Feed::Server("https://api.example/".into()), &cache).channel(Channel::Preview);
    let fetch = Canned::answering(204, "");
    let updater = Updater::with_fetch(config, fetch);
    assert!(matches!(updater.check().unwrap(), UpdateStatus::UpToDate));
    assert_eq!(updater.fetch_ref().asked(), ["https://api.example/releases/app/preview/darwin/aarch64/1.0.0"]);
}

#[test]
fn an_error_status_is_a_transport_error() {
    let cache = folder("server-500");
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), Canned::answering(503, ""));
    assert!(matches!(updater.check(), Err(UpdateError::Transport(why)) if why.contains("503")));
}

#[test]
fn a_transport_error_comes_through() {
    let cache = folder("server-down");
    let fetch = Canned { get: Mutex::new(Some(Err("no route".into()))), ..Canned::default() };
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), fetch);
    assert!(matches!(updater.check(), Err(UpdateError::Transport(why)) if why == "no route"));
}

#[test]
fn a_release_for_another_product_is_refused_before_a_download() {
    let cache = folder("server-other");
    let fetch = Canned::answering(200, &release_json("other", "2.0.0", None));
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), fetch);
    assert!(matches!(updater.check(), Err(UpdateError::Manifest(why)) if why.contains("other")));
    assert_eq!(updater.fetch_ref().asked().len(), 1, "nothing downloaded");
}

#[test]
fn a_newer_release_is_available_with_its_notes() {
    let cache = folder("server-newer");
    let fetch = Canned::answering(200, &release_json("app", "1.1.0", None));
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), fetch);
    let UpdateStatus::Available(offer) = updater.check().unwrap() else { panic!("an offer") };
    assert_eq!(offer.version, v("1.1.0"));
    assert_eq!(offer.notes.as_deref(), Some("fixes"));
    assert_eq!(offer.asset.format, Format::Dmg);
}

// ---- the verdict ----

#[test]
fn a_build_below_the_floor_is_retired_even_at_the_channels_own_version() {
    let status = evaluate(&v("1.0.0"), release("1.0.0", Some("1.0.1")));
    let UpdateStatus::Retired { minimum, upgrade } = status else { panic!("retired") };
    assert_eq!(minimum, v("1.0.1"));
    assert_eq!(upgrade.unwrap().version, v("1.0.0"), "the way out rides along");
}

#[test]
fn a_retirement_carries_the_upgrade() {
    let UpdateStatus::Retired { upgrade, .. } = evaluate(&v("0.9.0"), release("2.0.0", Some("1.0.0"))) else {
        panic!("retired")
    };
    assert_eq!(upgrade.unwrap().version, v("2.0.0"));
}

#[test]
fn a_floor_this_build_clears_is_not_a_retirement() {
    assert!(matches!(evaluate(&v("1.5.0"), release("2.0.0", Some("1.0.0"))), UpdateStatus::Available(_)));
    assert!(matches!(evaluate(&v("1.0.0"), release("2.0.0", Some("1.0.0"))), UpdateStatus::Available(_)));
}

#[test]
fn the_same_or_an_older_release_is_up_to_date() {
    assert!(matches!(evaluate(&v("1.0.0"), release("1.0.0", None)), UpdateStatus::UpToDate));
    assert!(matches!(evaluate(&v("1.2.0"), release("1.0.0", None)), UpdateStatus::UpToDate));
    assert!(matches!(evaluate(&v("1.0.0"), release("1.0.0-rc.1", None)), UpdateStatus::UpToDate));
}

// ---- a static feed ----

fn manifest(product: &str, channel: &str, floor: &str) -> String {
    format!(
        r#"{{"product":"{product}","version":"1.2.0","channel":"{channel}","platforms":{{"darwin-aarch64":{{"url":"https://dl/1.2.0/App.dmg","signature":"c2ln","format":"dmg"}}}}{floor}}}"#
    )
}

#[test]
fn a_static_url_fills_in_its_names() {
    let cache = folder("static-url");
    let config = config(Feed::Static("https://dl/{product}/{channel}/{target}/{os}-{arch}/{version}.json".into()), &cache);
    let updater = Updater::with_fetch(config, Canned::answering(200, &manifest("app", "stable", "")));
    assert!(matches!(updater.check().unwrap(), UpdateStatus::Available(_)));
    assert_eq!(updater.fetch_ref().asked(), ["https://dl/app/stable/darwin-aarch64/darwin-aarch64/1.0.0.json"]);
}

#[test]
fn a_static_manifest_for_another_product_or_channel_is_refused() {
    let cache = folder("static-other");
    let feed = Feed::Static("https://dl/latest.json".into());
    let other = Updater::with_fetch(config(feed.clone(), &cache), Canned::answering(200, &manifest("other", "stable", "")));
    assert!(matches!(other.check(), Err(UpdateError::Manifest(why)) if why.contains("product")));
    let preview = Updater::with_fetch(config(feed, &cache), Canned::answering(200, &manifest("app", "preview", "")));
    assert!(matches!(preview.check(), Err(UpdateError::Manifest(why)) if why.contains("channel")));
}

#[test]
fn a_static_manifest_with_no_artifact_here_is_no_update_here() {
    let cache = folder("static-elsewhere");
    let mut config = config(Feed::Static("https://dl/latest.json".into()), &cache);
    config.target = TargetTriple::new(Os::Linux, Arch::X86_64);
    let updater = Updater::with_fetch(config, Canned::answering(200, &manifest("app", "stable", "")));
    assert!(matches!(updater.check().unwrap(), UpdateStatus::UpToDate));
}

#[test]
fn a_static_manifest_retires_a_build_below_its_floor() {
    let cache = folder("static-floor");
    let body = manifest("app", "stable", r#","min_supported":"1.1.0""#);
    let updater = Updater::with_fetch(config(Feed::Static("https://dl/latest.json".into()), &cache), Canned::answering(200, &body));
    assert!(matches!(updater.check().unwrap(), UpdateStatus::Retired { minimum, .. } if minimum == v("1.1.0")));
}

#[test]
fn a_missing_static_manifest_is_an_error_not_silence() {
    let cache = folder("static-404");
    let updater = Updater::with_fetch(config(Feed::Static("https://dl/latest.json".into()), &cache), Canned::answering(404, "no"));
    assert!(matches!(updater.check(), Err(UpdateError::Transport(why)) if why.contains("404")));
}

// ---- the download ----

fn asset(signature: String, sha256: Option<String>, size: Option<u64>, url: &str) -> ReleaseAsset {
    ReleaseAsset { url: url.into(), signature, format: Format::Dmg, size, sha256 }
}

fn staging_files(cache: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(cache.join("updates"))
        .map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn a_download_is_hashed_verified_and_kept_by_what_it_is() {
    let cache = folder("download-good");
    let bytes = b"the new build".repeat(100);
    let digest = bunny_ui_formats::sha::sha256(&bytes);
    let good = asset(signature(&key(1), &bytes), Some(digest.to_uppercase()), None, "https://dl/x.dmg");
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), Canned::serving(200, &bytes));
    let mut stages = Vec::new();
    let verified = updater.download(&good, &mut |stage| stages.push(stage)).unwrap();
    assert_eq!(verified.format(), Format::Dmg);
    assert_eq!(std::fs::read(verified.path()).unwrap(), bytes);
    assert_eq!(staging_files(&cache), [format!("app-{}.dmg", &digest[..16])]);
    assert_eq!(stages.last(), Some(&InstallProgress::Verifying));
    assert!(matches!(stages[0], InstallProgress::Downloading(_)));
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn a_bad_signature_keeps_nothing() {
    let cache = folder("download-bad-sig");
    let bytes = b"the new build".to_vec();
    let forged = asset(signature(&key(9), &bytes), None, None, "https://dl/x.dmg");
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), Canned::serving(200, &bytes));
    assert!(matches!(updater.download(&forged, &mut |_| {}), Err(UpdateError::BadSignature)));
    let garbage = asset("not base64!".into(), None, None, "https://dl/x.dmg");
    assert!(matches!(updater.download(&garbage, &mut |_| {}), Err(UpdateError::BadSignature)));
    assert!(staging_files(&cache).is_empty(), "{:?}", staging_files(&cache));
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn a_wrong_hash_keeps_nothing() {
    let cache = folder("download-bad-hash");
    let bytes = b"the new build".to_vec();
    let wrong = asset(signature(&key(1), &bytes), Some("00".repeat(32)), None, "https://dl/x.dmg");
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), Canned::serving(200, &bytes));
    assert!(matches!(updater.download(&wrong, &mut |_| {}), Err(UpdateError::Integrity { .. })));
    assert!(staging_files(&cache).is_empty());
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn an_updater_with_no_key_downloads_nothing() {
    let cache = folder("download-no-key");
    let mut config = config(Feed::Server("https://api".into()), &cache);
    config.keys.clear();
    let updater = Updater::with_fetch(config, Canned::serving(200, b"x"));
    let result = updater.download(&asset("c2ln".into(), None, None, "https://dl/x.dmg"), &mut |_| {});
    assert!(matches!(result, Err(UpdateError::Apply(why)) if why.contains("no public key")));
    assert!(updater.fetch_ref().asked().is_empty());
}

#[test]
fn a_download_error_status_is_a_transport_error_and_keeps_nothing() {
    let cache = folder("download-403");
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), Canned::serving(403, b"denied"));
    let result = updater.download(&asset("c2ln".into(), None, None, "https://dl/x.dmg"), &mut |_| {});
    assert!(matches!(result, Err(UpdateError::Transport(why)) if why.contains("403")));
    assert!(staging_files(&cache).is_empty());
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn a_signed_url_of_any_length_installs_like_any_other() {
    let cache = folder("download-presigned");
    let bytes = b"build".to_vec();
    let url = format!("https://dl/app.dmg?X-Amz-Signature={}", "f".repeat(400));
    let signed = asset(signature(&key(1), &bytes), None, None, &url);
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), Canned::serving(200, &bytes));
    let verified = updater.download(&signed, &mut |_| {}).unwrap();
    assert!(verified.path().file_name().unwrap().len() < 64);
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn a_streamed_download_reports_each_whole_percent_once() {
    let cache = folder("download-percent");
    let bytes = vec![7u8; 1000];
    let fetch = Canned { chunk: 3, length: Some(1000), ..Canned::serving(200, &bytes) };
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), fetch);
    let mut seen = Vec::new();
    let _verified = updater
        .download(&asset(signature(&key(1), &bytes), None, None, "https://dl/x.dmg"), &mut |stage| {
            if let InstallProgress::Downloading(transfer) = stage {
                seen.push(transfer.percent().unwrap());
            }
        })
        .unwrap();
    assert_eq!(seen, (0..=100).collect::<Vec<u8>>());
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn a_reply_with_no_length_counts_against_the_manifest_size() {
    let cache = folder("download-size");
    let bytes = vec![7u8; 200];
    let fetch = Canned { chunk: 100, ..Canned::serving(200, &bytes) };
    let updater = Updater::with_fetch(config(Feed::Server("https://api".into()), &cache), fetch);
    let mut seen = Vec::new();
    let _verified = updater
        .download(&asset(signature(&key(1), &bytes), None, Some(200), "https://dl/x.dmg"), &mut |stage| {
            if let InstallProgress::Downloading(transfer) = stage {
                seen.push(transfer.percent());
            }
        })
        .unwrap();
    assert_eq!(seen, [Some(0), Some(50), Some(100)]);
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn a_size_nobody_named_ticks_by_the_mebibyte() {
    let mut ticker = Ticker::default();
    let reports: Vec<bool> =
        [0, 1, 1 << 19, 1 << 20, (1 << 20) + 5, 3 << 20].iter().map(|&n| ticker.moved(Transfer::new(n, None))).collect();
    assert_eq!(reports, [true, false, false, true, false, true]);
}

// ---- keys ----

#[test]
fn any_key_of_the_build_can_verify() {
    let cache = folder("verify-any");
    std::fs::create_dir_all(&cache).unwrap();
    let path = cache.join("artifact");
    std::fs::write(&path, b"bytes").unwrap();
    let keys = [key(1).verifying_key(), key(2).verifying_key()];
    assert!(verify_file(&keys, &path, &signature(&key(2), b"bytes")).is_ok());
    assert!(matches!(verify_file(&keys, &path, &signature(&key(3), b"bytes")), Err(UpdateError::BadSignature)));
    assert!(matches!(verify_file(&keys, &path, &signature(&key(1), b"other")), Err(UpdateError::BadSignature)));
    assert!(matches!(verify_file(&keys, &path, "c2ln"), Err(UpdateError::BadSignature)), "too short");
    std::fs::remove_dir_all(&cache).unwrap();
}

#[test]
fn a_key_the_build_cannot_read_is_an_error() {
    let base = UpdaterConfig::new(Feed::Server("https://api".into()), product(), v("1.0.0"));
    let good = base64::encode(key(1).verifying_key().as_bytes());
    assert_eq!(base.clone().key_base64(&format!("{good}\n")).unwrap().keys, [key(1).verifying_key()]);
    assert!(base.clone().key_base64("not a key").is_err());
    assert!(base.clone().key_base64(&base64::encode(&[1; 31])).is_err());
    // y = 2 has no x on the curve
    let mut off = [0u8; 32];
    off[0] = 2;
    assert!(base.key_base64(&base64::encode(&off)).is_err());
}

#[test]
fn the_cache_is_the_products_folder_in_the_systems_cache() {
    let cache = crate::default_cache(&product());
    assert!(cache.ends_with("app"), "{}", cache.display());
    let config = UpdaterConfig::new(Feed::Server("https://api".into()), product(), v("1.0.0"));
    assert_eq!(config.staging(), cache.join("updates"));
    assert_eq!(config.log_path(), cache.join("update.log"));
}

#[test]
fn the_staged_name_is_the_product_the_content_and_the_format() {
    assert_eq!(staged_name(&product(), Format::Zip, "0123456789abcdef0123"), "app-0123456789abcdef.zip");
    assert_eq!(staged_name(&product(), Format::TarGz, "ab"), "app-ab.tar.gz");
}
