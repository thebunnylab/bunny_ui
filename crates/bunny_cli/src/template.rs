//! `@BUNNY_NAME@` placeholders, filled in with values escaped for the
//! file they land in.
//!
//! The marker cannot be mistaken for anything the files already hold —
//! Gradle's `${…}`, Xcode's `$(…)` and HTML's `{…}` pass through — and
//! a marker nobody filled is an error naming the file and the line, not
//! a string that ships.

use crate::error::{Error, Result};

/// How a value is written where the placeholder sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escape {
    /// As given: Rust source, a dependency line.
    Raw,
    /// Inside a TOML basic string (`"…"`).
    Toml,
    /// Inside XML or HTML text and attributes: plists, manifests, pages.
    Xml,
}

impl Escape {
    /// The escape a file's own syntax asks for, by its name.
    pub fn for_path(path: &str) -> Escape {
        let path = path.strip_suffix(".tmpl").unwrap_or(path);
        if path.ends_with(".toml") {
            Escape::Toml
        } else if [".plist", ".xml", ".html"].iter().any(|ext| path.ends_with(ext)) {
            Escape::Xml
        } else {
            Escape::Raw
        }
    }

    fn apply(self, value: &str) -> String {
        match self {
            Escape::Raw => value.to_string(),
            Escape::Toml => {
                let mut out = String::with_capacity(value.len());
                for c in value.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
                        c => out.push(c),
                    }
                }
                out
            }
            Escape::Xml => value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;"),
        }
    }
}

/// Fills every `@BUNNY_X@` in `text` from `values` (keys without the
/// `BUNNY_` prefix). `name` is the file's, for the error.
pub fn render(name: &str, text: &str, values: &[(&str, &str)], escape: Escape) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("@BUNNY_") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "@BUNNY_".len()..];
        let length = after.find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'));
        let key = match length {
            Some(length) if after[length..].starts_with('@') && length > 0 => &after[..length],
            _ => {
                // not a placeholder after all: an email, a stray `@`
                out.push_str("@BUNNY_");
                rest = after;
                continue;
            }
        };
        let Some((_, value)) = values.iter().find(|(name, _)| *name == key) else {
            let line = text[..text.len() - rest.len() + at].matches('\n').count() + 1;
            return Err(Error::new(format!("{name}:{line}: nothing fills `@BUNNY_{key}@`")));
        };
        out.push_str(&escape.apply(value));
        rest = &after[key.len() + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_fill_and_the_rest_passes_through() {
        let text = "name = \"@BUNNY_APP_NAME@\"\nlabel = \"${appLabel}\" $(PRODUCT) {css} me@BUNNY_x.com";
        let out = render("Cargo.toml", text, &[("APP_NAME", "Notes")], Escape::Toml).unwrap();
        assert_eq!(out, "name = \"Notes\"\nlabel = \"${appLabel}\" $(PRODUCT) {css} me@BUNNY_x.com");
    }

    #[test]
    fn values_are_escaped_for_their_file() {
        let toml = render("a.toml", "\"@BUNNY_N@\"", &[("N", "Ada \"Bunny\" \\ Co")], Escape::Toml).unwrap();
        assert_eq!(toml, "\"Ada \\\"Bunny\\\" \\\\ Co\"");
        let xml = render("a.plist", "<string>@BUNNY_N@</string>", &[("N", "Tom & Jerry <3")], Escape::Xml)
            .unwrap();
        assert_eq!(xml, "<string>Tom &amp; Jerry &lt;3</string>");
    }

    #[test]
    fn an_unfilled_placeholder_names_its_line() {
        let error = render("Info.plist", "a\nb\n<string>@BUNNY_NOPE@</string>", &[], Escape::Xml).unwrap_err();
        assert_eq!(error.message, "Info.plist:3: nothing fills `@BUNNY_NOPE@`");
    }

    #[test]
    fn the_escape_follows_the_file_name() {
        assert_eq!(Escape::for_path("app/Cargo.toml.tmpl"), Escape::Toml);
        assert_eq!(Escape::for_path("ios/Info.plist"), Escape::Xml);
        assert_eq!(Escape::for_path("web/index.html"), Escape::Xml);
        assert_eq!(Escape::for_path("app/src/lib.rs.tmpl"), Escape::Raw);
    }
}
