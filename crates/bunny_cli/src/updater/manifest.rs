//! `latest.json`: the release manifest a static feed serves, written
//! and read by hand.
//!
//! The bytes are the ones `serde_json::to_string_pretty` writes for a
//! `bunny_ui_release::ReleaseManifest`, with a newline at the end: the
//! fields in the contract's order, the platforms in its order (darwin,
//! windows, linux, and aarch64 before x86_64), and each empty optional
//! field left out. A test holds the two to the same bytes.

use std::fmt::Write;

use crate::json::{self, Value};

/// The package formats of the release contract.
pub const FORMATS: &[&str] = &["app", "dmg", "nsis", "wix", "appimage", "tar.gz", "zip"];

/// The operating systems and the CPUs of the contract, in its order.
const OSES: &[&str] = &["darwin", "windows", "linux"];
const ARCHES: &[&str] = &["aarch64", "x86_64"];

/// One platform's package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub url: String,
    /// The base64 Ed25519 signature of the package's bytes.
    pub signature: String,
    pub format: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// One release, for one or more platforms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub product: Option<String>,
    pub version: String,
    pub channel: String,
    /// RFC 3339, in UTC.
    pub pub_date: Option<String>,
    pub notes: Option<String>,
    /// By target (`darwin-aarch64`), in the contract's order.
    pub platforms: Vec<(String, Asset)>,
    pub min_supported: Option<String>,
}

/// The place of a target in the contract's order. `None`: the contract
/// does not know it.
pub fn target_rank(target: &str) -> Option<(usize, usize)> {
    let (os, arch) = target.split_once('-')?;
    Some((OSES.iter().position(|known| *known == os)?, ARCHES.iter().position(|known| *known == arch)?))
}

impl Manifest {
    /// The platforms put in the contract's order.
    pub fn sort(&mut self) {
        self.platforms.sort_by_key(|(target, _)| target_rank(target).unwrap_or((usize::MAX, usize::MAX)));
    }

    /// The manifest as `latest.json`.
    pub fn to_json(&self) -> String {
        let mut manifest = self.clone();
        manifest.sort();
        let mut out = String::from("{\n");
        let mut fields: Vec<String> = Vec::new();
        let field = |key: &str, value: &str| format!("  {}: {}", json::quote(key), json::quote(value));
        if let Some(product) = &manifest.product {
            fields.push(field("product", product));
        }
        fields.push(field("version", &manifest.version));
        fields.push(field("channel", &manifest.channel));
        if let Some(date) = &manifest.pub_date {
            fields.push(field("pub_date", date));
        }
        if let Some(notes) = &manifest.notes {
            fields.push(field("notes", notes));
        }
        if manifest.platforms.is_empty() {
            fields.push(String::from("  \"platforms\": {}"));
        } else {
            let platforms: Vec<String> =
                manifest.platforms.iter().map(|(target, asset)| format!("    {}: {}", json::quote(target), asset.to_json())).collect();
            fields.push(format!("  \"platforms\": {{\n{}\n  }}", platforms.join(",\n")));
        }
        if let Some(floor) = &manifest.min_supported {
            fields.push(field("min_supported", floor));
        }
        out.push_str(&fields.join(",\n"));
        out.push_str("\n}\n");
        out
    }

    /// Reads a manifest. The error names the field that is wrong.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let value = json::parse(text)?;
        if !matches!(value, Value::Object(_)) {
            return Err(String::from("not a JSON object"));
        }
        let optional = |key: &str| -> Result<Option<String>, String> {
            match value.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(_) => Err(format!("`{key}` is not a string")),
            }
        };
        let required = |key: &str| -> Result<String, String> { optional(key)?.ok_or_else(|| format!("no `{key}`")) };
        let product = optional("product")?;
        if let Some(product) = &product {
            super::check_product(product).map_err(|why| format!("product \"{product}\": {why}"))?;
        }
        let version = required("version")?;
        check_version(&version).map_err(|why| format!("version: {why}"))?;
        let channel = required("channel")?;
        super::check_channel(&channel)?;
        let min_supported = optional("min_supported")?;
        if let Some(floor) = &min_supported {
            check_version(floor).map_err(|why| format!("min_supported: {why}"))?;
        }
        let mut platforms = Vec::new();
        match value.get("platforms") {
            Some(Value::Object(members)) => {
                for (target, asset) in members {
                    if target_rank(target).is_none() {
                        return Err(format!("`{target}` is not a target: darwin, windows or linux, then aarch64 or x86_64"));
                    }
                    if platforms.iter().any(|(known, _)| known == target) {
                        return Err(format!("{target} is there twice"));
                    }
                    let asset = Asset::parse(asset).map_err(|why| format!("platforms.{target}: {why}"))?;
                    platforms.push((target.clone(), asset));
                }
            }
            _ => return Err(String::from("no `platforms` object")),
        }
        let mut manifest = Manifest {
            product,
            version,
            channel,
            pub_date: optional("pub_date")?,
            notes: optional("notes")?,
            platforms,
            min_supported,
        };
        manifest.sort();
        Ok(manifest)
    }
}

impl Asset {
    fn to_json(&self) -> String {
        let mut fields = vec![
            format!("\"url\": {}", json::quote(&self.url)),
            format!("\"signature\": {}", json::quote(&self.signature)),
            format!("\"format\": {}", json::quote(&self.format)),
        ];
        if let Some(size) = self.size {
            fields.push(format!("\"size\": {size}"));
        }
        if let Some(sha256) = &self.sha256 {
            fields.push(format!("\"sha256\": {}", json::quote(sha256)));
        }
        let mut out = String::from("{\n");
        for (at, field) in fields.iter().enumerate() {
            let _ = writeln!(out, "      {field}{}", if at + 1 < fields.len() { "," } else { "" });
        }
        out.push_str("    }");
        out
    }

    fn parse(value: &Value) -> Result<Asset, String> {
        let string = |key: &str| -> Result<Option<String>, String> {
            match value.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(_) => Err(format!("`{key}` is not a string")),
            }
        };
        let required = |key: &str| -> Result<String, String> { string(key)?.ok_or_else(|| format!("no `{key}`")) };
        let format = required("format")?;
        if !FORMATS.contains(&format.as_str()) {
            return Err(format!("\"{format}\" is not a format: {}", FORMATS.join(", ")));
        }
        let size = match value.get("size") {
            None | Some(Value::Null) => None,
            Some(number) => Some(number.as_u64().ok_or("`size` is not a whole number")?),
        };
        Ok(Asset { url: required("url")?, signature: required("signature")?, format, size, sha256: string("sha256")? })
    }
}

/// One manifest of several, each for its own platforms. The product,
/// the version and the channel must agree, and so must the notes and
/// the floor where two have one. A target in two manifests must have
/// the same package in both. The date is the latest of them.
pub fn merge(manifests: &[(String, Manifest)]) -> Result<Manifest, String> {
    let Some(((first_name, first), rest)) = manifests.split_first() else {
        return Err(String::from("no manifest to merge"));
    };
    let mut merged = first.clone();
    for (name, manifest) in rest {
        let differ = |what: &str, a: &str, b: &str| format!("{first_name} and {name} differ in {what}: {a} and {b}");
        let shown = |value: &Option<String>| value.clone().unwrap_or_else(|| String::from("none"));
        if manifest.product != merged.product {
            return Err(differ("product", &shown(&merged.product), &shown(&manifest.product)));
        }
        if manifest.version != merged.version {
            return Err(differ("version", &merged.version, &manifest.version));
        }
        if manifest.channel != merged.channel {
            return Err(differ("channel", &merged.channel, &manifest.channel));
        }
        for (key, mine, theirs) in [
            ("min_supported", &mut merged.min_supported, &manifest.min_supported),
            ("notes", &mut merged.notes, &manifest.notes),
        ] {
            match (mine.as_ref(), theirs) {
                (Some(a), Some(b)) if a != b => return Err(differ(key, a, b)),
                (None, Some(b)) => *mine = Some(b.clone()),
                _ => {}
            }
        }
        if let Some(date) = &manifest.pub_date
            && merged.pub_date.as_ref().is_none_or(|known| date > known)
        {
            merged.pub_date = Some(date.clone());
        }
        for (target, asset) in &manifest.platforms {
            match merged.platforms.iter().find(|(known, _)| known == target) {
                Some((_, known)) if known == asset => {}
                Some(_) => return Err(format!("{target} is in more than one manifest, with different packages (the last is {name})")),
                None => merged.platforms.push((target.clone(), asset.clone())),
            }
        }
    }
    merged.sort();
    Ok(merged)
}

/// The strict SemVer form the release contract reads: three numbers
/// with no leading zero, then `-pre` and `+build` identifiers.
pub fn check_version(text: &str) -> Result<(), String> {
    let fail = |why: &str| Err(format!("\"{text}\" is not a SemVer version: {why}"));
    let (rest, build) = match text.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (text, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let identifiers = |part: &str, pre: bool| {
        part.split('.').all(|id| {
            let shaped = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            let digits = id.bytes().all(|b| b.is_ascii_digit());
            shaped && !(pre && digits && id.len() > 1 && id.starts_with('0'))
        })
    };
    if build.is_some_and(|build| !identifiers(build, false)) {
        return fail("the build metadata");
    }
    if pre.is_some_and(|pre| !identifiers(pre, true)) {
        return fail("the pre-release");
    }
    let numbers: Vec<&str> = core.split('.').collect();
    let numeric = |part: &&str| {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit())
            && !(part.len() > 1 && part.starts_with('0'))
            && part.parse::<u64>().is_ok()
    };
    if numbers.len() != 3 || !numbers.iter().all(numeric) {
        return fail("it needs three numbers, MAJOR.MINOR.PATCH");
    }
    Ok(())
}

/// `seconds` after the Unix epoch, as RFC 3339 in UTC:
/// `2026-11-19T18:25:48Z`.
pub fn rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Howard Hinnant's days-to-civil
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3_600, rest % 3_600 / 60, rest % 60)
}

/// The URL template with `{version}`, `{file}` and `{channel}` filled
/// in. The file name is percent-encoded: a space is `%20`.
pub fn fill_url(template: &str, version: &str, file: &str, channel: &str) -> String {
    let mut encoded = String::new();
    for byte in file.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    template.replace("{version}", version).replace("{channel}", channel).replace("{file}", &encoded)
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
