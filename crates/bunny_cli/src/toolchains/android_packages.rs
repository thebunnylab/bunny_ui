//! Where the Android toolchain is downloaded from, without Android
//! Studio: Google's own package repository (the list `sdkmanager` reads)
//! for the command-line tools and the NDK's version, and Adoptium for a
//! JDK — each archive with the checksum its publisher gives.

use crate::json::{self, Value};

/// Google's repository manifest.
pub const REPOSITORY: &str = "https://dl.google.com/android/repository/repository2-3.xml";

/// The JDK major version `bunny` installs.
pub const JDK_MAJOR: u32 = 21;

/// An archive to download, and how to check it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
    pub url: String,
    pub name: String,
    pub size: u64,
    /// `sha1:…` or `sha256:…`.
    pub checksum: String,
}

/// Every `<remotePackage path="…">` body whose path matches, on the
/// stable channel.
fn stable_packages(xml: &str, matches: impl Fn(&str) -> bool) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<remotePackage path=\"") {
        let after = &rest[at + "<remotePackage path=\"".len()..];
        let Some(quote) = after.find('"') else { break };
        let path = &after[..quote];
        let Some(end) = after.find("</remotePackage>") else { break };
        let body = &after[quote..end];
        if matches(path) && body.contains("<channelRef ref=\"channel-0\"") {
            found.push((path, body));
        }
        rest = &after[end..];
    }
    found
}

/// Google's Android CLI for this host — one native binary, the
/// successor of `sdkmanager` and `avdmanager` — or `None` where Google
/// ships none (Linux on Arm, Windows on Arm).
pub fn cli_url() -> Option<String> {
    let platform = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin_arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin_x86_64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux_x86_64"
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        "windows_x86_64"
    } else {
        return None;
    };
    let name = if cfg!(windows) { "android.exe" } else { "android" };
    Some(format!("https://dl.google.com/android/cli/latest/{platform}/{name}"))
}

/// The package paths in `android sdk list --all`: words shaped like
/// `ndk/30.0.16248370` or `system-images/android-36/google_apis/arm64-v8a`
/// (the older `;` spelling is read too).
pub fn listed_packages(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == '|')
        .map(|word| word.trim_matches(|c: char| c == '"' || c == ',' || c == '`'))
        .filter(|word| {
            word.contains(['/', ';'])
                && word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && word.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | ';' | '.' | '-' | '_'))
        })
        .map(String::from)
        .collect()
}

/// The newest NDK among the listed packages.
pub fn newest_listed_ndk(packages: &[String]) -> Option<String> {
    packages
        .iter()
        .filter(|path| path.starts_with("ndk/") || path.starts_with("ndk;"))
        .max_by_key(|path| path[4..].split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>())
        .cloned()
}

/// The system image for API `api` and `abi`, with Google's APIs and
/// without the Play Store (which forbids root and `run-as` tricks).
pub fn listed_image(packages: &[String], api: u32, abi: &str) -> Option<String> {
    let wanted = [String::from("system-images"), format!("android-{api}"), String::from("google_apis"), abi.to_string()];
    packages
        .iter()
        .find(|path| {
            let parts: Vec<&str> = path.split(['/', ';']).collect();
            parts == wanted.iter().map(String::as_str).collect::<Vec<_>>()
        })
        .cloned()
}

/// The newest stable NDK's package: `ndk;30.0.16248370`.
pub fn newest_ndk(xml: &str) -> Option<String> {
    stable_packages(xml, |path| path.starts_with("ndk;"))
        .into_iter()
        .map(|(path, _)| path)
        .max_by_key(|path| path[4..].split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>())
        .map(String::from)
}

/// Adoptium's query for the newest Temurin JDK for this host.
pub fn temurin_query() -> String {
    let os = if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x64" };
    format!(
        "https://api.adoptium.net/v3/assets/latest/{JDK_MAJOR}/hotspot?os={os}&architecture={arch}&image_type=jdk&vendor=eclipse"
    )
}

/// The JDK archive in Adoptium's answer.
pub fn temurin(text: &str) -> Option<Archive> {
    let value = json::parse(text).ok()?;
    let package = value.as_array().first()?.path(&["binary", "package"])?;
    Some(Archive {
        url: package.str_at(&["link"])?.to_string(),
        name: package.str_at(&["name"])?.to_string(),
        size: package.get("size").and_then(Value::as_u64).unwrap_or(0),
        checksum: format!("sha256:{}", package.str_at(&["checksum"])?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<sdk:sdk-repository>
<channel id="channel-0">stable</channel>
<remotePackage path="ndk;9.10.1"><channelRef ref="channel-0"/></remotePackage>
<remotePackage path="ndk;30.0.16248370"><channelRef ref="channel-0"/></remotePackage>
<remotePackage path="ndk;31.0.1"><channelRef ref="channel-1"/></remotePackage>
<remotePackage path="ndk;27.3.13750724"><channelRef ref="channel-0"/></remotePackage>
</sdk:sdk-repository>"#;

    #[test]
    fn the_listed_packages_by_their_paths() {
        let text = "Installed packages:\n  Path                                   | Version | Description\n  \
                    platform-tools                         | 37.0.1  | Android SDK Platform-Tools\n\
                    Available packages:\n  ndk/27.3.13750724 | 27.3.13750724 | NDK (Side by side)\n  \
                    ndk/30.0.16248370 | 30.0.16248370 | NDK\n  ndk-bundle | 22.1 | old\n  \
                    system-images/android-36/google_apis/arm64-v8a | 7 | Google APIs ARM 64\n  \
                    system-images/android-36/google_apis_playstore/arm64-v8a | 7 | Play\n  platforms/android-36 | 2 |\n";
        let packages = listed_packages(text);
        assert_eq!(newest_listed_ndk(&packages).as_deref(), Some("ndk/30.0.16248370"));
        assert_eq!(
            listed_image(&packages, 36, "arm64-v8a").as_deref(),
            Some("system-images/android-36/google_apis/arm64-v8a")
        );
        assert!(packages.contains(&String::from("platforms/android-36")));
        // the older spelling still reads
        let old = listed_packages("ndk;26.1.1 system-images;android-36;google_apis;x86_64");
        assert_eq!(newest_listed_ndk(&old).as_deref(), Some("ndk;26.1.1"));
        assert_eq!(listed_image(&old, 36, "x86_64").as_deref(), Some("system-images;android-36;google_apis;x86_64"));
    }

    #[test]
    fn the_cli_is_named_for_this_host() {
        if let Some(url) = cli_url() {
            assert!(url.starts_with("https://dl.google.com/android/cli/latest/"));
        }
    }

    #[test]
    fn the_newest_stable_ndk_by_number() {
        assert_eq!(newest_ndk(XML).as_deref(), Some("ndk;30.0.16248370"));
    }

    #[test]
    fn adoptium_names_the_archive_and_its_checksum() {
        let text = r#"[{"binary":{"package":{"checksum":"3623232f","link":"https://github.com/adoptium/x.tar.gz",
            "name":"OpenJDK21U-jdk_aarch64_mac_hotspot_21.0.12.1_1.tar.gz","size":200073404}},"version":{"major":21}}]"#;
        let jdk = temurin(text).unwrap();
        assert_eq!(jdk.checksum, "sha256:3623232f");
        assert_eq!(jdk.size, 200_073_404);
        assert!(jdk.name.ends_with(".tar.gz"));
    }
}
