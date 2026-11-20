//! A semantic version (SemVer 2.0.0), ordered by precedence.

use core::cmp::Ordering;
use core::fmt;
use core::str::FromStr;

use crate::UpdateError;

/// A semantic version: `major.minor.patch`, an optional pre-release
/// after `-` and optional build metadata after `+`.
///
/// The order is SemVer precedence, so `0.10.0` is newer than `0.9.0`
/// and `1.0.0-rc.1` is older than `1.0.0`. Two versions that differ
/// only in build metadata are not equal: the metadata breaks the tie.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: String,
    build: String,
}

impl Version {
    /// A release version with no pre-release and no build metadata.
    #[must_use]
    pub const fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch, pre: String::new(), build: String::new() }
    }

    #[must_use]
    pub const fn major(&self) -> u64 {
        self.major
    }

    #[must_use]
    pub const fn minor(&self) -> u64 {
        self.minor
    }

    #[must_use]
    pub const fn patch(&self) -> u64 {
        self.patch
    }

    /// The pre-release, empty for a release.
    #[must_use]
    pub fn pre(&self) -> &str {
        &self.pre
    }

    /// The build metadata, empty when there is none.
    #[must_use]
    pub fn build(&self) -> &str {
        &self.build
    }

    #[must_use]
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    #[must_use]
    pub fn is_newer_than(&self, other: &Version) -> bool {
        self > other
    }

    /// Parses the strict SemVer form, with no `v` before it: the form
    /// on the wire.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Parse`] that names the part that is wrong.
    pub fn parse_strict(text: &str) -> Result<Version, UpdateError> {
        let fail = |why: &str| UpdateError::Parse(format!("invalid version {text:?}: {why}"));
        let (rest, build) = match text.split_once('+') {
            Some((rest, build)) => (rest, build),
            None => (text, ""),
        };
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, pre),
            None => (rest, ""),
        };
        if text.contains('+') {
            identifiers(build, false).map_err(|why| fail(&format!("build metadata: {why}")))?;
        }
        if rest.contains('-') {
            identifiers(pre, true).map_err(|why| fail(&format!("pre-release: {why}")))?;
        }
        let mut parts = core.split('.');
        let mut number = |name: &str| -> Result<u64, UpdateError> {
            let part = parts.next().ok_or_else(|| fail(&format!("no {name} number")))?;
            numeric(part).ok_or_else(|| fail(&format!("the {name} number {part:?} is not a number")))
        };
        let (major, minor, patch) = (number("major")?, number("minor")?, number("patch")?);
        if parts.next().is_some() {
            return Err(fail("more than three numbers"));
        }
        Ok(Version { major, minor, patch, pre: pre.to_owned(), build: build.to_owned() })
    }
}

/// A number with no sign, no leading zero and no more than 64 bits.
fn numeric(part: &str) -> Option<u64> {
    let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let leading_zero = part.len() > 1 && part.starts_with('0');
    if digits && !leading_zero { part.parse().ok() } else { None }
}

/// Checks dot-separated identifiers. A numeric identifier of a
/// pre-release cannot have a leading zero.
fn identifiers(text: &str, pre: bool) -> Result<(), String> {
    for identifier in text.split('.') {
        if identifier.is_empty() {
            return Err("an empty identifier".into());
        }
        if !identifier.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
            return Err(format!("{identifier:?} has a character outside [0-9A-Za-z-]"));
        }
        let all_digits = identifier.bytes().all(|byte| byte.is_ascii_digit());
        if pre && all_digits && identifier.len() > 1 && identifier.starts_with('0') {
            return Err(format!("{identifier:?} has a leading zero"));
        }
    }
    Ok(())
}

/// SemVer precedence of two identifier lists. An empty list is the
/// greater when `empty_is_greater` (a release is above its pre-releases)
/// and the lesser when not (no build metadata is below some).
fn compare_identifiers(a: &str, b: &str, empty_is_greater: bool) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return if empty_is_greater { Ordering::Greater } else { Ordering::Less },
        (false, true) => return if empty_is_greater { Ordering::Less } else { Ordering::Greater },
        (false, false) => {}
    }
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        let order = match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => compare_identifier(x, y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
}

fn compare_identifier(x: &str, y: &str) -> Ordering {
    let x_numeric = x.bytes().all(|byte| byte.is_ascii_digit());
    let y_numeric = y.bytes().all(|byte| byte.is_ascii_digit());
    match (x_numeric, y_numeric) {
        // digit strings of any length: fewer digits (without leading
        // zeros) is the smaller number
        (true, true) => {
            let (xt, yt) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
            xt.len().cmp(&yt.len()).then_with(|| xt.cmp(yt)).then_with(|| x.len().cmp(&y.len()))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => x.cmp(y),
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| compare_identifiers(&self.pre, &other.pre, true))
            .then_with(|| compare_identifiers(&self.build, &other.build, false))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Accepts a `v` before the version (`v1.2.0`), as a tag has.
impl FromStr for Version {
    type Err = UpdateError;
    fn from_str(text: &str) -> Result<Version, UpdateError> {
        Version::parse_strict(text.strip_prefix('v').unwrap_or(text))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Version({self})")
    }
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The wire takes the strict form only, with no `v`.
impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Version, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Version::parse_strict(&text).map_err(serde::de::Error::custom)
    }
}
