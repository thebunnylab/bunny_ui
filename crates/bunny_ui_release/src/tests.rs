use super::*;

fn v(text: &str) -> Version {
    text.parse().unwrap()
}

fn asset(format: Format) -> ReleaseAsset {
    ReleaseAsset { url: "u".into(), signature: "s".into(), format, size: None, sha256: None }
}

#[test]
fn version_orders_by_precedence_not_text() {
    assert!(v("0.10.0").is_newer_than(&v("0.9.0")));
    assert!(v("0.1.0").is_newer_than(&v("0.1.0-rc.1")));
    assert!(!v("0.1.0").is_newer_than(&v("0.1.0")));
    assert_eq!(v("v0.2.0"), v("0.2.0"));
}

#[test]
fn version_precedence_is_the_semver_specs() {
    // SemVer 2.0.0, section 11: each one is below the next
    let chain = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "2.0.0",
        "2.1.0",
        "2.1.1",
    ];
    for pair in chain.windows(2) {
        assert!(v(pair[1]).is_newer_than(&v(pair[0])), "{} > {}", pair[1], pair[0]);
        assert!(!v(pair[0]).is_newer_than(&v(pair[1])));
    }
}

#[test]
fn version_order_and_parse_match_the_semver_crate() {
    let texts = [
        "0.0.0", "0.0.1", "0.1.0", "1.0.0", "1.2.3", "10.20.30", "1.0.0-0", "1.0.0-1", "1.0.0-01a",
        "1.0.0-a", "1.0.0-a.1", "1.0.0-a.b", "1.0.0-a-b", "1.0.0-alpha.10", "1.0.0-alpha.9",
        "1.0.0+build", "1.0.0+build.1", "1.0.0+001", "1.0.0+1", "1.0.0-rc.1+build.7", "1.0.0+b",
        "1.0.0+a.b", "1.0.0+a.10", "1.0.0+a.9", "18446744073709551615.0.0",
    ];
    for a in texts {
        let ours = Version::parse_strict(a).unwrap();
        let theirs = semver::Version::parse(a).unwrap();
        assert_eq!(ours.to_string(), theirs.to_string(), "{a}");
        for b in texts {
            let order = ours.cmp(&Version::parse_strict(b).unwrap());
            assert_eq!(order, theirs.cmp(&semver::Version::parse(b).unwrap()), "{a} vs {b}");
        }
    }
    let refused = [
        "", "1", "1.0", "1.0.0.0", "01.0.0", "1.01.0", "1.0.01", "1.0.0-", "1.0.0+", "1.0.0-01",
        "1.0.0-a..b", "1.0.0-a_b", "1.0.0+a..b", "-1.0.0", "1.0.0 ", " 1.0.0", "v1.0.0",
        "18446744073709551616.0.0", "a.b.c",
    ];
    for text in refused {
        assert!(Version::parse_strict(text).is_err(), "{text:?} is refused");
        assert!(semver::Version::parse(text).is_err(), "semver refuses {text:?} too");
    }
}

#[test]
fn the_wire_version_has_no_v() {
    assert!(serde_json::from_str::<Version>("\"v1.0.0\"").is_err());
    assert_eq!(serde_json::from_str::<Version>("\"1.0.0-rc.1\"").unwrap(), v("1.0.0-rc.1"));
}

#[test]
fn channel_round_trips_and_rejects_unknown() {
    for c in [Channel::Stable, Channel::Preview, Channel::Nightly] {
        assert_eq!(c.as_str().parse::<Channel>().unwrap(), c);
    }
    assert!("beta".parse::<Channel>().is_err());
}

#[test]
fn target_serializes_as_one_string_key() {
    let t = TargetTriple::new(Os::Darwin, Arch::Aarch64);
    assert_eq!(t.to_string(), "darwin-aarch64");
    assert_eq!("macos-arm64".parse::<TargetTriple>().unwrap(), t);
    assert_eq!(serde_json::to_string(&t).unwrap(), "\"darwin-aarch64\"");
}

#[test]
fn manifest_round_trips_through_json() {
    let mut platforms = BTreeMap::new();
    platforms.insert(
        current_target(),
        ReleaseAsset {
            url: "https://dl/x.dmg".into(),
            signature: "sig".into(),
            format: Format::Dmg,
            size: Some(42),
            sha256: Some("ab".into()),
        },
    );
    let manifest = ReleaseManifest {
        product: Some(ProductId::parse("relay").unwrap()),
        version: v("0.1.0"),
        channel: Channel::Stable,
        pub_date: None,
        notes: None,
        platforms,
        min_supported: None,
    };
    let back: ReleaseManifest = serde_json::from_str(&serde_json::to_string(&manifest).unwrap()).unwrap();
    assert_eq!(back.version, v("0.1.0"));
    assert!(back.notes.is_none());
    assert_eq!(back.asset_for(current_target()).unwrap().format, Format::Dmg);
}

/// The bytes a publisher and a release server already exchange: a
/// manifest and a flat release, as they are written today. The contract
/// reads them and writes them back byte for byte.
#[test]
fn the_wire_reads_and_writes_the_same_bytes() {
    let manifest = r#"{"product":"bunnylab","version":"0.4.2","channel":"preview","pub_date":"2026-11-02T10:00:00Z","platforms":{"darwin-aarch64":{"url":"https://dl/bunnylab/releases/0.4.2/bunnylab-0.4.2-aarch64.dmg","signature":"c2ln","format":"dmg","size":123456,"sha256":"00ff"},"windows-x86_64":{"url":"https://dl/bunnylab/releases/0.4.2/bunnylab-setup-0.4.2-x86_64.exe","signature":"c2ln","format":"nsis","size":777},"linux-x86_64":{"url":"https://dl/bunnylab/releases/0.4.2/bunnylab-0.4.2-x86_64.tar.gz","signature":"c2ln","format":"tar.gz","size":654321,"sha256":"ff00"}}}"#;
    // the platforms go in the order of `Os` (darwin, windows, linux), the
    // order a manifest written by the same contract already has
    let parsed: ReleaseManifest = serde_json::from_str(manifest).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), manifest);
    assert!(parsed.is_for(&ProductId::parse("bunnylab").unwrap()));

    let flat = r#"{"product":"bunnylab","version":"0.4.2","url":"https://dl/bunnylab/releases/0.4.2/bunnylab-0.4.2-aarch64.dmg","signature":"c2ln","format":"dmg","pub_date":"2026-11-02T10:00:00Z","size":123456,"sha256":"00ff","min_supported":"0.3.0"}"#;
    let release: PlatformRelease = serde_json::from_str(flat).unwrap();
    assert_eq!(serde_json::to_string(&release).unwrap(), flat);

    // the projection of the manifest is the flat answer, floor apart
    let mut projected = parsed.project(TargetTriple::new(Os::Darwin, Arch::Aarch64)).unwrap();
    projected.min_supported = Some(v("0.3.0"));
    assert_eq!(serde_json::to_string(&projected).unwrap(), flat);
    assert!(parsed.project(TargetTriple::new(Os::Windows, Arch::Aarch64)).is_none());

    let pointer = r#"{"version":"0.4.2","updated":"2026-11-02T10:00:00Z","min_supported":"0.3.0"}"#;
    let read: ChannelPointer = serde_json::from_str(pointer).unwrap();
    assert_eq!(serde_json::to_string(&read).unwrap(), pointer);
}

#[test]
fn a_manifest_with_a_floor_carries_it_and_one_without_does_not() {
    let mut manifest: ReleaseManifest =
        serde_json::from_str(r#"{"product":"app","version":"2.0.0","channel":"stable","platforms":{}}"#).unwrap();
    assert!(manifest.min_supported.is_none());
    assert!(!serde_json::to_string(&manifest).unwrap().contains("min_supported"));
    manifest.min_supported = Some(v("1.5.0"));
    assert!(serde_json::to_string(&manifest).unwrap().ends_with(r#""min_supported":"1.5.0"}"#));
}

#[test]
fn a_manifest_without_a_product_is_for_no_product() {
    let manifest: ReleaseManifest =
        serde_json::from_str(r#"{"version":"1.0.0","channel":"stable","platforms":{}}"#).unwrap();
    assert!(!manifest.is_for(&ProductId::parse("app").unwrap()));
    assert!(serde_json::from_str::<ReleaseManifest>(r#"{"product":"../x","version":"1.0.0","channel":"stable","platforms":{}}"#).is_err());
}

#[test]
fn a_transfer_reads_its_whole_percent() {
    assert_eq!(Transfer::new(0, Some(200)).percent(), Some(0));
    assert_eq!(Transfer::new(1, Some(200)).percent(), Some(0));
    assert_eq!(Transfer::new(99, Some(200)).percent(), Some(49));
    assert_eq!(Transfer::new(200, Some(200)).percent(), Some(100));
    assert_eq!(Transfer::new(u64::MAX, Some(u64::MAX)).percent(), Some(100));
}

#[test]
fn a_transfer_past_its_size_reads_done_not_over() {
    assert_eq!(Transfer::new(300, Some(200)).percent(), Some(100));
}

#[test]
fn a_transfer_without_a_size_has_no_percent() {
    assert_eq!(Transfer::new(4096, None).percent(), None);
    let zero = Transfer::new(4096, Some(0));
    assert_eq!(zero.percent(), None);
    assert_eq!(zero.total(), None);
    assert_eq!(zero.received(), 4096);
}

#[test]
fn channel_as_str_display_and_serde_for_every_variant() {
    for (c, s) in [(Channel::Stable, "stable"), (Channel::Preview, "preview"), (Channel::Nightly, "nightly")] {
        assert_eq!(c.as_str(), s);
        assert_eq!(c.to_string(), s);
        assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{s}\""));
        assert_eq!(serde_json::from_str::<Channel>(&format!("\"{s}\"")).unwrap(), c);
    }
    assert!(matches!("beta".parse::<Channel>(), Err(UpdateError::Parse(_))));
    let set: std::collections::HashSet<Channel> = [Channel::Stable, Channel::Stable].into();
    assert_eq!(set.len(), 1);
}

#[test]
fn channel_from_build_maps_each_value() {
    assert_eq!(Channel::from_build(Some("preview")), Channel::Preview);
    assert_eq!(Channel::from_build(Some("nightly")), Channel::Nightly);
    assert_eq!(Channel::from_build(Some("stable")), Channel::Stable);
    assert_eq!(Channel::from_build(Some("garbage")), Channel::Stable);
    assert_eq!(Channel::from_build(None), Channel::Stable);
    assert_eq!(current_channel!(), Channel::from_build(option_env!("BUNNY_CHANNEL")));
}

#[test]
fn os_round_trips_aliases_and_rejects_unknown() {
    for (os, canonical, aliases) in [
        (Os::Darwin, "darwin", &["darwin", "macos"][..]),
        (Os::Windows, "windows", &["windows", "win"][..]),
        (Os::Linux, "linux", &["linux"][..]),
    ] {
        assert_eq!(os.as_str(), canonical);
        assert_eq!(os.to_string(), canonical);
        for alias in aliases {
            assert_eq!(alias.parse::<Os>().unwrap(), os);
        }
    }
    assert!(matches!("plan9".parse::<Os>(), Err(UpdateError::Parse(_))));
    assert!(Os::Darwin < Os::Linux);
}

#[test]
fn arch_round_trips_aliases_and_rejects_unknown() {
    for (arch, canonical, aliases) in [
        (Arch::Aarch64, "aarch64", &["aarch64", "arm64"][..]),
        (Arch::X86_64, "x86_64", &["x86_64", "x64", "amd64"][..]),
    ] {
        assert_eq!(arch.as_str(), canonical);
        assert_eq!(arch.to_string(), canonical);
        for alias in aliases {
            assert_eq!(alias.parse::<Arch>().unwrap(), arch);
        }
    }
    assert!(matches!("riscv".parse::<Arch>(), Err(UpdateError::Parse(_))));
    assert!(Arch::Aarch64 < Arch::X86_64);
}

#[test]
fn target_triple_display_parse_and_errors() {
    let t = TargetTriple::new(Os::Linux, Arch::X86_64);
    assert_eq!(t.to_string(), "linux-x86_64");
    assert_eq!("linux-x86_64".parse::<TargetTriple>().unwrap(), t);
    assert_eq!(serde_json::from_str::<TargetTriple>("\"linux-x86_64\"").unwrap(), t);
    assert!(matches!("linux".parse::<TargetTriple>(), Err(UpdateError::Parse(_))));
    assert!(matches!("linux-pdp11".parse::<TargetTriple>(), Err(UpdateError::Parse(_))));
    assert!(TargetTriple::new(Os::Darwin, Arch::Aarch64) < t);
}

#[test]
fn version_display_and_parse_error() {
    assert_eq!(v("1.2.3").to_string(), "1.2.3");
    assert_eq!(v("v1.2.3").to_string(), "1.2.3");
    assert!(matches!("not.a.version".parse::<Version>(), Err(UpdateError::Parse(_))));
    let set: std::collections::HashSet<Version> = [v("1.0.0"), v("1.0.0")].into();
    assert_eq!(set.len(), 1);
    assert_eq!(serde_json::to_string(&v("1.0.0")).unwrap(), "\"1.0.0\"");
    assert_eq!(Version::new(1, 2, 3), v("1.2.3"));
    assert_eq!((v("1.2.3-rc.1+b").pre(), v("1.2.3-rc.1+b").build()), ("rc.1", "b"));
    assert!(v("1.0.0-rc.1").is_prerelease() && !v("1.0.0").is_prerelease());
}

#[test]
fn format_serde_for_every_variant() {
    for (f, s) in [
        (Format::App, "app"),
        (Format::Dmg, "dmg"),
        (Format::Nsis, "nsis"),
        (Format::Wix, "wix"),
        (Format::AppImage, "appimage"),
        (Format::TarGz, "tar.gz"),
        (Format::Zip, "zip"),
    ] {
        assert_eq!(serde_json::to_string(&f).unwrap(), format!("\"{s}\""));
        assert_eq!(serde_json::from_str::<Format>(&format!("\"{s}\"")).unwrap(), f);
    }
}

#[test]
fn release_asset_skips_none_size_and_sha() {
    let bare = asset(Format::TarGz);
    let json = serde_json::to_string(&bare).unwrap();
    assert!(!json.contains("size"), "{json}");
    assert!(!json.contains("sha256"), "{json}");
    let full = ReleaseAsset { size: Some(9), sha256: Some("ff".into()), ..bare.clone() };
    let json = serde_json::to_string(&full).unwrap();
    assert!(json.contains("\"size\":9"));
    assert!(json.contains("\"sha256\":\"ff\""));
    assert_ne!(bare, full);
}

#[test]
fn manifest_asset_for_missing_platform_is_none() {
    let manifest = ReleaseManifest {
        product: Some(ProductId::parse("relay").unwrap()),
        version: v("0.1.0"),
        channel: Channel::Nightly,
        pub_date: Some("2026-06-26".into()),
        notes: Some("first".into()),
        platforms: BTreeMap::new(),
        min_supported: None,
    };
    assert!(manifest.asset_for(current_target()).is_none());
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(json.contains("\"pub_date\":\"2026-06-26\""));
    assert!(json.contains("\"notes\":\"first\""));
}

#[test]
fn channel_pointer_round_trips() {
    let pointer = ChannelPointer {
        version: v("2.0.0"),
        updated: Some("2026-06-26T00:00:00Z".into()),
        min_supported: Some(v("1.0.0")),
    };
    let wire = serde_json::to_string(&pointer).unwrap();
    let back: ChannelPointer = serde_json::from_str(&wire).unwrap();
    assert_eq!(back.version, v("2.0.0"));
    assert_eq!(back.min_supported.unwrap(), v("1.0.0"));
    let bare = ChannelPointer { version: v("3.0.0"), updated: None, min_supported: None };
    let json = serde_json::to_string(&bare).unwrap();
    assert!(!json.contains("updated"));
    assert!(!json.contains("min_supported"));
}

#[test]
fn platform_release_round_trips() {
    let release = PlatformRelease {
        product: Some(ProductId::parse("relay").unwrap()),
        version: v("1.5.0"),
        url: "https://dl/relay.dmg".into(),
        signature: "sig".into(),
        format: Format::Dmg,
        notes: Some("notes".into()),
        pub_date: Some("2026-06-26".into()),
        size: Some(1024),
        sha256: Some("deadbeef".into()),
        min_supported: None,
    };
    let wire = serde_json::to_string(&release).unwrap();
    let back: PlatformRelease = serde_json::from_str(&wire).unwrap();
    assert_eq!(back.version, v("1.5.0"));
    assert_eq!(back.size, Some(1024));
    assert_eq!(back.asset(), ReleaseAsset {
        url: "https://dl/relay.dmg".into(),
        signature: "sig".into(),
        format: Format::Dmg,
        size: Some(1024),
        sha256: Some("deadbeef".into()),
    });
    assert!(
        !wire.contains("min_supported"),
        "an absent floor stays off the wire: a client that does not know the field sees today's shape"
    );
}

#[test]
fn installed_compares() {
    assert_eq!(Installed::PendingHelper, Installed::PendingHelper);
    assert_ne!(Installed::PendingHelper, Installed::ReadyOnRestart);
    let status = UpdateStatus::Available(Offer { version: v("9.9.9"), asset: asset(Format::App), notes: None });
    assert!(format!("{status:?}").contains("Available"));
}

#[test]
fn current_version_and_target_are_well_formed() {
    assert_eq!(current_version!().to_string(), env!("CARGO_PKG_VERSION"));
    let t = current_target();
    assert!(matches!(t.os, Os::Darwin | Os::Windows | Os::Linux));
    assert!(matches!(t.arch, Arch::Aarch64 | Arch::X86_64));
}

#[test]
fn update_error_displays_each_variant() {
    assert!(UpdateError::Transport("net".into()).to_string().contains("net"));
    assert!(UpdateError::Manifest("bad".into()).to_string().contains("bad"));
    assert!(UpdateError::NoAssetForPlatform(current_target()).to_string().contains("no release asset"));
    assert_eq!(UpdateError::BadSignature.to_string(), "signature verification failed");
    assert!(
        UpdateError::Integrity { expected: "a".into(), actual: "b".into() }
            .to_string()
            .contains("expected a, got b")
    );
    assert!(UpdateError::Apply("oops".into()).to_string().contains("oops"));
    assert_eq!(UpdateError::NotBundled.to_string(), "not running from an installed bundle");
    assert_eq!(UpdateError::Parse("x".into()).to_string(), "x");
}
