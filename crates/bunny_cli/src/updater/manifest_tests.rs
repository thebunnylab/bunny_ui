use super::*;

fn asset(file: &str, format: &str) -> Asset {
    Asset {
        url: format!("https://dl.example.com/app/1.2.0/{file}"),
        signature: String::from("c2ln"),
        format: format.to_string(),
        size: Some(123_456),
        sha256: Some(String::from("00ff")),
    }
}

fn one(target: &str, asset: Asset) -> Manifest {
    Manifest {
        product: Some(String::from("app")),
        version: String::from("1.2.0"),
        channel: String::from("stable"),
        pub_date: Some(String::from("2026-11-19T10:00:00Z")),
        notes: None,
        platforms: vec![(target.to_string(), asset)],
        min_supported: None,
    }
}

/// What the release contract reads `text` as, written back by serde.
fn through_the_contract(text: &str) -> String {
    let manifest: bunny_ui_release::ReleaseManifest = serde_json::from_str(text).unwrap();
    serde_json::to_string_pretty(&manifest).unwrap() + "\n"
}

#[test]
fn latest_json_is_the_bytes_the_contract_writes() {
    let mut full = one("linux-x86_64", asset("app-1.2.0-linux-x86_64.tar.gz", "tar.gz"));
    full.platforms.push((String::from("darwin-x86_64"), asset("App-1.2.0.dmg", "dmg")));
    let bare = Asset { size: None, sha256: None, ..asset("App-1.2.0-windows-arm64.zip", "zip") };
    full.platforms.push((String::from("windows-aarch64"), bare));
    full.platforms.push((String::from("darwin-aarch64"), asset("App-1.2.0.dmg", "dmg")));
    full.platforms.push((String::from("linux-aarch64"), asset("app-1.2.0-linux-aarch64.tar.gz", "tar.gz")));
    full.notes = Some(String::from("Faster \"sync\".\n\tAnd a tab, a \u{8}, a \u{1f} and é 🐰."));
    full.min_supported = Some(String::from("1.0.0-beta.2"));
    let minimal = Manifest { product: None, pub_date: None, ..one("windows-x86_64", asset("A.zip", "zip")) };
    for manifest in [full, minimal] {
        let text = manifest.to_json();
        assert_eq!(text, through_the_contract(&text));
        assert_eq!(Manifest::parse(&text).unwrap().to_json(), text, "it reads back");
    }
}

#[test]
fn the_order_is_the_contracts() {
    let mut manifest = one("linux-x86_64", asset("a", "tar.gz"));
    for target in ["windows-x86_64", "darwin-x86_64", "linux-aarch64", "darwin-aarch64", "windows-aarch64"] {
        manifest.platforms.push((target.to_string(), asset("a", "zip")));
    }
    let text = manifest.to_json();
    let order: Vec<usize> = ["darwin-aarch64", "darwin-x86_64", "windows-aarch64", "windows-x86_64", "linux-aarch64", "linux-x86_64"]
        .iter()
        .map(|target| text.find(target).unwrap())
        .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    let fields: Vec<usize> =
        ["\"product\"", "\"version\"", "\"channel\"", "\"pub_date\"", "\"platforms\""].iter().map(|key| text.find(key).unwrap()).collect();
    assert!(fields.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert!(text.starts_with("{\n  \"product\": \"app\",\n  \"version\": \"1.2.0\",\n"), "{text}");
    assert!(!text.contains("notes") && !text.contains("min_supported"), "the empty ones are left out");
}

#[test]
fn a_wrong_manifest_is_refused_with_its_field() {
    let good = one("darwin-aarch64", asset("A.dmg", "dmg")).to_json();
    let cases = [
        (good.replace("\"1.2.0\"", "\"1.2\""), "version"),
        (good.replace("\"stable\"", "\"beta\""), "\"beta\" is not a channel"),
        (good.replace("darwin-aarch64", "macos-arm64"), "`macos-arm64` is not a target"),
        (good.replace("\"dmg\"", "\"pkg\""), "platforms.darwin-aarch64: \"pkg\" is not a format"),
        (good.replace("\"signature\": \"c2ln\",", ""), "no `signature`"),
        (good.replace("123456", "-1"), "`size` is not a whole number"),
        (good.replace("\"app\"", "\"App\""), "product \"App\""),
        (String::from("[]"), "not a JSON object"),
        (String::from("{\"version\":\"1.0.0\",\"channel\":\"stable\"}"), "no `platforms`"),
    ];
    for (text, expected) in cases {
        let error = Manifest::parse(&text).unwrap_err();
        assert!(error.contains(expected), "{expected}: {error}");
    }
}

#[test]
fn merging_joins_the_platforms() {
    let mac = one("darwin-aarch64", asset("App-1.2.0.dmg", "dmg"));
    let mut windows = one("windows-x86_64", asset("App-1.2.0-windows-x64.zip", "zip"));
    windows.pub_date = Some(String::from("2026-11-19T11:30:00Z"));
    windows.min_supported = Some(String::from("1.0.0"));
    let linux = Manifest { pub_date: None, ..one("linux-x86_64", asset("app-1.2.0-linux-x86_64.tar.gz", "tar.gz")) };
    let merged = merge(&[(String::from("linux.json"), linux), (String::from("mac.json"), mac.clone()), (String::from("win.json"), windows)]).unwrap();
    let targets: Vec<&str> = merged.platforms.iter().map(|(target, _)| target.as_str()).collect();
    assert_eq!(targets, ["darwin-aarch64", "windows-x86_64", "linux-x86_64"]);
    assert_eq!(merged.pub_date.as_deref(), Some("2026-11-19T11:30:00Z"), "the latest date");
    assert_eq!(merged.min_supported.as_deref(), Some("1.0.0"), "a floor is kept");
    // the same package twice is the same platform once
    let again = merge(&[(String::from("a"), merged.clone()), (String::from("b"), mac)]).unwrap();
    assert_eq!(again, merged);
}

#[test]
fn merging_refuses_what_does_not_agree() {
    let mac = one("darwin-aarch64", asset("App-1.2.0.dmg", "dmg"));
    let named = |manifest: &Manifest| (String::from("b.json"), manifest.clone());
    let cases = [
        (Manifest { version: String::from("1.2.1"), ..mac.clone() }, "differ in version: 1.2.0 and 1.2.1"),
        (Manifest { channel: String::from("preview"), ..mac.clone() }, "differ in channel"),
        (Manifest { product: None, ..mac.clone() }, "differ in product: app and none"),
        (one("darwin-aarch64", Asset { signature: String::from("b3RoZXI="), ..asset("App-1.2.0.dmg", "dmg") }), "darwin-aarch64 is in more than one manifest"),
    ];
    for (other, expected) in cases {
        let error = merge(&[(String::from("a.json"), mac.clone()), named(&other)]).unwrap_err();
        assert!(error.contains(expected), "{expected}: {error}");
    }
    let floor = |version: &str| Manifest { min_supported: Some(version.to_string()), ..mac.clone() };
    let error = merge(&[(String::from("a.json"), floor("1.0.0")), (String::from("b.json"), floor("1.1.0"))]).unwrap_err();
    assert!(error.contains("a.json and b.json differ in min_supported: 1.0.0 and 1.1.0"), "{error}");
    assert!(merge(&[]).is_err());
}

#[test]
fn versions_are_strict_semver() {
    for good in ["1.2.0", "0.0.1", "1.4.0-beta.2", "2.0.0-rc.1+build.5", "1.0.0+20261119", "10.20.30-alpha-1"] {
        assert!(check_version(good).is_ok(), "{good}");
    }
    for bad in ["1.2", "1.2.3.4", "v1.2.0", "01.2.0", "1.2.0-", "1.2.0-01", "1.2.0+", "1.2.0-a..b", "1.x.0", ""] {
        assert!(check_version(bad).is_err(), "{bad}");
    }
}

#[test]
fn dates_are_rfc_3339_in_utc() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339(1_795_120_348), "2026-11-19T20:32:28Z");
    assert_eq!(rfc3339(4_107_542_399), "2100-02-28T23:59:59Z");
}

#[test]
fn the_url_template_is_filled_in() {
    let url = fill_url("https://dl.example.com/app/{channel}/{version}/{file}", "1.2.0", "Photo Lab-1.2.0.dmg", "stable");
    assert_eq!(url, "https://dl.example.com/app/stable/1.2.0/Photo%20Lab-1.2.0.dmg");
    assert_eq!(fill_url("https://d/{file}", "1.0.0", "a+b~c.tar.gz", "stable"), "https://d/a%2Bb~c.tar.gz");
}
