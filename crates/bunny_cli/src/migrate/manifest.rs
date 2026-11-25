//! The bunny-ui version a `Cargo.toml` asks for, read and rewritten in
//! place. The family moves in lockstep (`bunny-ui`, `bunny-ui-core`,
//! `bunny-ui-shell`, …), so every one of its requirements moves
//! together; every other line keeps its bytes.

use super::Version;

/// What a manifest says about the bunny-ui it builds with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    /// A version, the same for every member of the family it names.
    Version(Version),
    /// `workspace = true`: the workspace's manifest says.
    Workspace,
    /// No version: a path or git dependency, or no bunny-ui at all.
    None,
}

/// One requirement string of the family: where its text is.
struct Found {
    /// The bytes inside the quotes.
    start: usize,
    end: usize,
    version: Option<Version>,
}

/// The bunny-ui version `manifest` asks for.
pub fn manifest_version(manifest: &str) -> Requirement {
    let (found, workspace) = scan(manifest);
    match found.iter().find_map(|found| found.version) {
        Some(version) => Requirement::Version(version),
        None if workspace => Requirement::Workspace,
        None => Requirement::None,
    }
}

/// `manifest` asking for `to` wherever it names a version of the
/// family; `None` when it names none.
pub fn manifest(manifest: &str, to: Version) -> Option<String> {
    let (found, _) = scan(manifest);
    let found: Vec<&Found> = found.iter().filter(|found| found.version.is_some()).collect();
    if found.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(manifest.len());
    let mut cursor = 0;
    for found in found {
        out.push_str(&manifest[cursor..found.start]);
        out.push_str(&rewrite(&manifest[found.start..found.end], to));
        cursor = found.end;
    }
    out.push_str(&manifest[cursor..]);
    Some(out)
}

/// `^0.5.1` → `^0.6.0`, `0.5` → `0.6`: the operator kept, the version
/// written to as many parts as it had.
fn rewrite(requirement: &str, to: Version) -> String {
    let digits = requirement.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
    let (operator, version) = requirement.split_at(digits);
    let parts = version.split('.').count();
    match parts {
        1 => format!("{operator}{}", to.major),
        2 => format!("{operator}{}.{}", to.major, to.minor),
        _ => format!("{operator}{}.{}.0", to.major, to.minor),
    }
}

/// The version of a requirement this tool can carry: one comparator
/// (`0.5`, `0.5.2`, `^0.5`, `~0.5.1`, `=0.5.0`), nothing else.
fn parse_requirement(requirement: &str) -> Option<Version> {
    let version = requirement.trim().trim_start_matches(['^', '~', '=']).trim();
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |minor| minor.parse().ok())?;
    Some(Version::new(major, minor))
}

/// Whether a dependency's name is one of the family.
fn of_the_family(name: &str) -> bool {
    name == "bunny-ui" || name.starts_with("bunny-ui-")
}

/// Every requirement string of the family in `manifest`, and whether a
/// member of the family is a workspace dependency.
fn scan(manifest: &str) -> (Vec<Found>, bool) {
    let mut found = Vec::new();
    let mut workspace = false;
    // the table the line is in: a dependency table, or one dependency's
    // own table (`[dependencies.bunny-ui]`)
    let mut in_dependencies = false;
    let mut own: Option<String> = None;
    let mut offset = 0;
    for line in manifest.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let header = trimmed.trim_start_matches('[').split(']').next().unwrap_or_default().trim();
            let keys = keys(header);
            in_dependencies = false;
            own = None;
            if let Some(position) = keys.iter().position(|key| is_dependency_table(key)) {
                match &keys[position + 1..] {
                    [] => in_dependencies = true,
                    [name] => own = Some(name.clone()),
                    _ => {}
                }
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else { continue };
        let key = unquote(key.trim());
        let value_at = start + line.find('=').unwrap_or(0) + 1;
        if let Some(name) = &own {
            // `version = "0.5"` / `package = "bunny-ui"` / `workspace = true`
            // inside one dependency's table: its name is the package's
            // unless `package` renames it — read the table's name only
            if of_the_family(name) {
                if key == "version" {
                    found.extend(string_at(manifest, value_at, line.len() - (value_at - start)));
                }
                if key == "workspace" && value.trim() == "true" {
                    workspace = true;
                }
            }
            continue;
        }
        if !in_dependencies {
            continue;
        }
        let value_text = &manifest[value_at..start + line.len()];
        // dotted keys: `bunny-ui.version = "0.5"`, `bunny-ui.workspace = true`
        if let Some((name, field)) = key.split_once('.') {
            if of_the_family(&unquote(name)) {
                match unquote(field).as_str() {
                    "version" => found.extend(string_at(manifest, value_at, value_text.len())),
                    "workspace" if value.trim() == "true" => workspace = true,
                    _ => {}
                }
            }
            continue;
        }
        if value.trim_start().starts_with('"') {
            if of_the_family(&key) {
                found.extend(string_at(manifest, value_at, value_text.len()));
            }
        } else if value.trim_start().starts_with('{') {
            let package = inline_value(value_text, "package").map(|(text, _)| text);
            let name = package.unwrap_or(&key);
            if !of_the_family(name) {
                continue;
            }
            if let Some((_, at)) = inline_value(value_text, "version") {
                found.extend(string_at(manifest, value_at + at, value_text.len() - at));
            }
            if inline_value(value_text, "workspace").is_some_and(|(value, _)| value == "true") {
                workspace = true;
            }
        }
    }
    (found, workspace)
}

/// A dotted table name's keys, quotes taken off: `target.'cfg(unix)'.dependencies`.
fn keys(header: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in header.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '.') => out.push(std::mem::take(&mut current).trim().to_string()),
            _ => current.push(c),
        }
    }
    out.push(current.trim().to_string());
    out
}

fn is_dependency_table(key: &str) -> bool {
    matches!(key, "dependencies" | "dev-dependencies" | "build-dependencies")
}

fn unquote(key: &str) -> String {
    key.trim_matches(['"', '\'']).to_string()
}

/// The quoted string that starts the `len` bytes at `at`, as a find.
fn string_at(manifest: &str, at: usize, len: usize) -> Option<Found> {
    let text = &manifest[at..at + len];
    let open = text.find('"')?;
    let close = text[open + 1..].find('"')? + open + 1;
    let (start, end) = (at + open + 1, at + close);
    Some(Found { start, end, version: parse_requirement(&manifest[start..end]) })
}

/// `key = "value"` inside an inline table: the value and where its
/// opening quote is, relative to `table`. A key is matched whole
/// (`version` is not `rust-version`).
fn inline_value<'a>(table: &'a str, key: &str) -> Option<(&'a str, usize)> {
    let mut from = 0;
    while let Some(found) = table[from..].find(key) {
        let at = from + found;
        from = at + key.len();
        let before = table[..at].trim_end().chars().last();
        if !matches!(before, Some('{' | ',')) {
            continue;
        }
        let rest = table[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let value = rest.trim_start();
        let quote_at = table.len() - value.len();
        if let Some(body) = value.strip_prefix('"') {
            let end = body.find('"')?;
            return Some((&body[..end], quote_at));
        }
        return Some((value.split([',', '}']).next().unwrap_or_default().trim(), quote_at));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirements_this_tool_can_carry() {
        assert_eq!(parse_requirement("0.5"), Some(Version::new(0, 5)));
        assert_eq!(parse_requirement("^0.5.3"), Some(Version::new(0, 5)));
        assert_eq!(parse_requirement("1"), Some(Version::new(1, 0)));
        assert_eq!(parse_requirement(">=0.5, <0.7"), None);
        assert_eq!(parse_requirement("*"), None);
        assert_eq!(rewrite("~0.5.1", Version::new(0, 6)), "~0.6.0");
    }

    #[test]
    fn a_key_is_matched_whole() {
        assert_eq!(inline_value("{ rust-version = \"1\", version = \"0.5\" }", "version").map(|(v, _)| v), Some("0.5"));
    }
}
