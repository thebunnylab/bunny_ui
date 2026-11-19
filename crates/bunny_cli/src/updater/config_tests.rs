use std::path::{Path, PathBuf};

use super::*;
use crate::json;

const PUBLIC: &str = "keys/updater.pub";

fn parse(text: &str, package: &str) -> Result<Config> {
    Config::parse(&json::parse(text).unwrap(), package, Path::new("/app"))
}

fn message(text: &str, package: &str) -> String {
    parse(text, package).unwrap_err().message
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bunny-updater-config-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("keys")).unwrap();
    dir
}

fn key(byte: u8) -> SigningKey {
    SigningKey::from_seed(&[byte; 32])
}

fn public(key: &SigningKey) -> String {
    base64::encode(key.verifying_key().as_bytes())
}

#[test]
fn a_full_table_reads() {
    let config = parse(
        r#"{"product":"photo-lab","feed":"https://dl.example.com/photo-lab/{channel}/latest.json",
            "pubkeys":["keys/updater.pub","keys/backup.pub"],"channel":"preview",
            "url":"https://dl.example.com/photo-lab/{version}/{file}"}"#,
        "photo_lab",
    )
    .unwrap();
    assert_eq!(config.product, "photo-lab");
    assert_eq!(config.feed, Feed::Static(String::from("https://dl.example.com/photo-lab/{channel}/latest.json")));
    assert_eq!(config.pubkeys, ["keys/updater.pub", "keys/backup.pub"]);
    assert_eq!(config.pubkey_paths, [PathBuf::from("/app/keys/updater.pub"), PathBuf::from("/app/keys/backup.pub")]);
    assert_eq!(config.channel.as_deref(), Some("preview"));
    assert!(config.keys.is_empty(), "parse reads no file");
}

#[test]
fn the_product_is_the_package_name_when_it_fits() {
    let config = parse(r#"{"server":"https://api.example.com","pubkeys":["k.pub"]}"#, "notes").unwrap();
    assert_eq!(config.product, "notes");
    assert_eq!(config.feed, Feed::Server(String::from("https://api.example.com")));
    let error = parse(r#"{"server":"https://api.example.com","pubkeys":["k.pub"]}"#, "Photo_Lab").unwrap_err();
    assert!(error.message.contains("package name `Photo_Lab`"), "{}", error.message);
    assert_eq!(error.hint.as_deref(), Some("set it: product = \"photo-lab\""));
}

#[test]
fn a_wrong_table_says_what_is_wrong() {
    let cases = [
        (r#"{"pubkeys":["k.pub"]}"#, "no `feed` and no `server`"),
        (r#"{"feed":"https://a","server":"https://b","pubkeys":["k.pub"]}"#, "both `feed` and `server`"),
        (r#"{"feed":"https://a"}"#, "no `pubkeys`"),
        (r#"{"feed":"https://a","pubkeys":[]}"#, "no `pubkeys`"),
        (r#"{"feed":"https://a","pubkeys":"k.pub"}"#, "`pubkeys` is not a list"),
        (r#"{"feed":"https://a","pubkeys":[1]}"#, "`pubkeys` is not a list"),
        (r#"{"feed":"ftp://a","pubkeys":["k.pub"]}"#, "not an https, http or file URL"),
        (r#"{"feed":"https://a","pubkeys":["k.pub"],"channel":"beta"}"#, "\"beta\" is not a channel"),
        (r#"{"feed":"https://a","pubkeys":["k.pub"],"url":"https://dl/{version}/app.dmg"}"#, "`url` has no {file}"),
        (r#"{"feed":"https://a","pubkeys":["k.pub"],"product":"-app"}"#, "product \"-app\""),
        (r#"{"feed":1,"pubkeys":["k.pub"]}"#, "`feed` is not a string"),
        (r#"["feed"]"#, "is not a table"),
    ];
    for (text, expected) in cases {
        let message = message(text, "app");
        assert!(message.starts_with("Cargo.toml's [package.metadata.bunny.updater]"), "{message}");
        assert!(message.contains(expected), "{text}: {message}");
    }
}

#[test]
fn a_typo_in_a_key_names_the_key_it_meant() {
    let error = parse(r#"{"feed":"https://a","pubkey":["k.pub"]}"#, "app").unwrap_err();
    assert!(error.message.contains("unknown key `pubkey`"), "{}", error.message);
    assert_eq!(error.hint.as_deref(), Some("did you mean `pubkeys`?"));
}

#[test]
fn the_keys_load_as_their_base64() {
    let dir = folder("load");
    let (first, second) = (key(1), key(2));
    std::fs::write(dir.join("keys/updater.pub"), format!("{}\n", public(&first))).unwrap();
    std::fs::write(dir.join("keys/backup.pub"), format!("  {}  \n", public(&second))).unwrap();
    let table = json::parse(r#"{"feed":"https://a/{channel}/latest.json","pubkeys":["keys/updater.pub","keys/backup.pub"]}"#).unwrap();
    let mut config = Config::parse(&table, "app", &dir).unwrap();
    config.load_keys().unwrap();
    assert_eq!(config.keys, [public(&first), public(&second)]);
    let env = config.env("stable");
    let get = |name: &str| env.iter().find(|(key, _)| key == name).map(|(_, value)| value.to_string_lossy().into_owned());
    assert_eq!(get("BUNNY_UPDATER_PRODUCT").as_deref(), Some("app"));
    assert_eq!(get("BUNNY_UPDATER_FEED").as_deref(), Some("https://a/{channel}/latest.json"));
    assert_eq!(get("BUNNY_UPDATER_SERVER"), None);
    assert_eq!(get("BUNNY_UPDATER_PUBKEYS"), Some(format!("{},{}", public(&first), public(&second))));
    assert_eq!(get("BUNNY_CHANNEL").as_deref(), Some("stable"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_key_file_that_is_no_key_is_refused() {
    let dir = folder("bad");
    let table = json::parse(&format!(r#"{{"feed":"https://a","pubkeys":["{PUBLIC}"]}}"#)).unwrap();
    let mut config = Config::parse(&table, "app", &dir).unwrap();
    let error = config.load_keys().unwrap_err();
    assert!(error.message.contains("pubkeys: keys/updater.pub"), "a missing file: {}", error.message);
    // about half of all y have no x on the curve: the first such one
    let off_curve = (2u8..)
        .map(|y| {
            let mut bytes = [0u8; 32];
            bytes[0] = y;
            bytes
        })
        .find(|bytes| VerifyingKey::from_bytes(bytes).is_none())
        .unwrap();
    for (text, expected) in [
        (String::from("not base64!"), "not base64"),
        (base64::encode(&[7; 16]), "16 bytes"),
        (base64::encode(&off_curve), "not on the curve"),
    ] {
        std::fs::write(dir.join(PUBLIC), &text).unwrap();
        let error = config.load_keys().unwrap_err();
        assert!(error.message.contains(expected), "{text}: {}", error.message);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_channel_of_the_environment_wins() {
    let config = parse(r#"{"feed":"https://a","pubkeys":["k.pub"],"channel":"preview"}"#, "app").unwrap();
    assert_eq!(config.channel(None).unwrap(), "preview");
    assert_eq!(config.channel(Some("  ")).unwrap(), "preview");
    assert_eq!(config.channel(Some("nightly")).unwrap(), "nightly");
    assert!(config.channel(Some("beta")).unwrap_err().message.starts_with("BUNNY_CHANNEL"));
    let plain = parse(r#"{"feed":"https://a","pubkeys":["k.pub"]}"#, "app").unwrap();
    assert_eq!(plain.channel(None).unwrap(), "stable");
}

#[test]
fn the_release_key_must_match_a_pubkey() {
    let dir = folder("release");
    let mut config = parse(r#"{"feed":"https://a","pubkeys":["k.pub"]}"#, "app").unwrap();
    let signer = key(9);
    config.keys = vec![public(&key(1)), public(&signer)];
    let seed = base64::encode(signer.seed());
    let from_env = release_key(None, Some(&format!("{seed}\n")), &config).unwrap().unwrap();
    assert_eq!(from_env.seed(), signer.seed());
    // the file wins over the environment
    let file = dir.join("updater.key");
    std::fs::write(&file, format!("{seed}\n")).unwrap();
    let other = base64::encode(key(1).seed());
    assert_eq!(release_key(Some(&file), Some(&other), &config).unwrap().unwrap().seed(), signer.seed());
    assert!(release_key(None, None, &config).unwrap().is_none());
    assert!(release_key(None, Some(""), &config).unwrap().is_none());
    let error = release_key(None, Some(&base64::encode(key(3).seed())), &config).unwrap_err();
    assert!(error.message.contains("matches none of the app's pubkeys"), "{}", error.message);
    let error = release_key(None, Some("AAAA"), &config).unwrap_err();
    assert!(error.message.starts_with("BUNNY_SIGNING_KEY: 3 bytes"), "{}", error.message);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_signed_file_verifies_with_the_public_key() {
    let dir = folder("sign");
    let file = dir.join("App-1.0.0.dmg");
    let bytes: Vec<u8> = (0..70_000u32).map(|n| (n * 7 % 251) as u8).collect();
    std::fs::write(&file, &bytes).unwrap();
    let signer = key(4);
    let signature = sign_file(&signer, &file).unwrap();
    let raw: [u8; 64] = base64::decode(&signature).unwrap().try_into().unwrap();
    let verifier = public_key(&public(&signer)).unwrap();
    assert!(verifier.verify(&bytes, &raw));
    let mut changed = bytes.clone();
    changed[1_000] ^= 1;
    assert!(!verifier.verify(&changed, &raw), "one changed bit fails");
    assert!(!public_key(&public(&key(5))).unwrap().verify(&bytes, &raw), "another key fails");
    let _ = std::fs::remove_dir_all(&dir);
}
