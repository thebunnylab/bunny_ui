use std::path::PathBuf;

use super::*;
use crate::json;

fn config(url: Option<&str>) -> Config {
    let mut text = String::from(r#"{"product":"photo-lab","feed":"https://dl.example.com/{channel}/latest.json","pubkeys":["k.pub"]"#);
    if let Some(url) = url {
        text.push_str(&format!(r#","url":"{url}""#));
    }
    text.push('}');
    Config::parse(&json::parse(&text).unwrap(), "photo_lab", Path::new("/app")).unwrap()
}

fn asset() -> Asset {
    Asset { url: String::new(), signature: String::from("c2ln"), format: String::from("dmg"), size: Some(42), sha256: Some(String::from("ab")) }
}

#[test]
fn a_universal_mac_lists_both_cpus_with_one_disk_image() {
    let shipped = Shipped { path: PathBuf::from("/out/Photo Lab-1.2.0.dmg"), format: "dmg", targets: vec!["darwin-aarch64", "darwin-x86_64"] };
    let config = config(Some("https://dl.example.com/photo-lab/{version}/{file}"));
    let manifest = manifest_for(&config, "1.2.0", "preview", &shipped, asset(), 1_795_120_348).unwrap();
    assert_eq!(manifest.product.as_deref(), Some("photo-lab"));
    assert_eq!(manifest.channel, "preview");
    assert_eq!(manifest.pub_date.as_deref(), Some("2026-11-19T20:32:28Z"));
    let targets: Vec<&str> = manifest.platforms.iter().map(|(target, _)| target.as_str()).collect();
    assert_eq!(targets, ["darwin-aarch64", "darwin-x86_64"]);
    for (_, each) in &manifest.platforms {
        assert_eq!(each.url, "https://dl.example.com/photo-lab/1.2.0/Photo%20Lab-1.2.0.dmg");
        assert_eq!((each.size, each.sha256.as_deref(), each.format.as_str()), (Some(42), Some("ab"), "dmg"));
    }
    // and the contract reads it
    let parsed: bunny_ui_release::ReleaseManifest = serde_json::from_str(&manifest.to_json()).unwrap();
    assert_eq!(parsed.platforms.len(), 2);
}

#[test]
fn without_a_url_there_is_no_manifest() {
    let shipped = Shipped { path: PathBuf::from("/out/a.zip"), format: "zip", targets: vec!["windows-x86_64"] };
    assert!(manifest_for(&config(None), "1.0.0", "stable", &shipped, asset(), 0).is_none());
}

#[test]
fn the_release_signs_the_package_and_lists_it() {
    let dir = std::env::temp_dir().join(format!("bunny-release-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let package = dir.join("photo_lab-1.2.0-linux-x86_64.tar.gz");
    let bytes: Vec<u8> = (0..5_000u32).map(|n| (n % 253) as u8).collect();
    fs::write(&package, &bytes).unwrap();
    let key = SigningKey::from_seed(&[3; 32]);
    let mut config = config(Some("https://dl.example.com/photo-lab/{version}/{file}"));
    config.keys = vec![base64::encode(key.verifying_key().as_bytes())];
    let project = Project {
        package: String::from("photo_lab"),
        version: String::from("1.2.0"),
        manifest: dir.join("Cargo.toml"),
        dir: dir.clone(),
        workspace_root: dir.clone(),
        target_dir: dir.join("target"),
        bin: None,
        has_lib: false,
        name: String::from("Photo Lab"),
        id: None,
        build: 1,
        updater: Some(config.clone()),
    };
    let release = Release { config, key: key.clone(), channel: String::from("stable") };
    let shipped = Shipped { path: package.clone(), format: "tar.gz", targets: vec!["linux-x86_64"] };
    let mut info = Info::new(&project, "linux", &Options::default());
    release.publish(&project, &shipped, &dir, &mut info).unwrap();

    let sig = fs::read_to_string(dir.join("photo_lab-1.2.0-linux-x86_64.tar.gz.sig")).unwrap();
    assert!(sig.ends_with('\n') && !sig.trim().contains('\n'), "one line: {sig:?}");
    let raw: [u8; 64] = base64::decode(sig.trim()).unwrap().try_into().unwrap();
    assert!(key.verifying_key().verify(&bytes, &raw), "the .sig verifies the package");

    let text = fs::read_to_string(dir.join("latest.json")).unwrap();
    let manifest: bunny_ui_release::ReleaseManifest = serde_json::from_str(&text).unwrap();
    let target = bunny_ui_release::TargetTriple::new(bunny_ui_release::Os::Linux, bunny_ui_release::Arch::X86_64);
    let listed = manifest.asset_for(target).unwrap();
    assert_eq!(listed.signature, sig.trim());
    assert_eq!(listed.size, Some(5_000));
    assert_eq!(listed.sha256.as_deref(), Some(bunny_ui_formats::sha::sha256(&bytes).as_str()));
    assert_eq!(listed.format, bunny_ui_release::Format::TarGz);
    assert_eq!(listed.url, "https://dl.example.com/photo-lab/1.2.0/photo_lab-1.2.0-linux-x86_64.tar.gz");
    assert_eq!(manifest.channel, bunny_ui_release::Channel::Stable);

    info.write(&dir.join("build-info.json")).unwrap();
    let written = fs::read_to_string(dir.join("build-info.json")).unwrap();
    assert!(written.contains("\"photo_lab-1.2.0-linux-x86_64.tar.gz.sig\", \"latest.json\""), "{written}");
    let _ = fs::remove_dir_all(&dir);
}
