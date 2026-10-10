//! The names an app answers to: the crate's, the one people read, and
//! the reverse-DNS id — spelled the way each platform accepts it.
//!
//! One id is written down (`com.example.my_app`); Apple's bundle ids
//! take no `_`, so they get `-`, and Android's application id is a Java
//! package, so a segment that is a Java keyword gets a trailing `_`.

use crate::error::{Error, Result};

/// Rust's keywords, strict, reserved and edition-2024 (`gen`): a crate
/// named after one cannot be `use`d.
const RUST_KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate", "do",
    "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof", "unsafe",
    "unsized", "use", "virtual", "where", "while", "yield",
];

/// Names a crate cannot take without shadowing the standard library or
/// colliding with cargo's own directories.
const RESERVED: &[&str] =
    &["alloc", "build", "core", "deps", "examples", "incremental", "proc_macro", "std", "test"];

const JAVA_KEYWORDS: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "default", "do", "double", "else", "enum", "extends", "false", "final", "finally",
    "float", "for", "goto", "if", "implements", "import", "instanceof", "int", "interface", "long",
    "native", "new", "null", "package", "private", "protected", "public", "return", "short",
    "static", "strictfp", "super", "switch", "synchronized", "this", "throw", "throws",
    "transient", "true", "try", "void", "volatile", "while",
];

/// The id prefix `bunny new` uses when none is given — it works, and it
/// must be changed before a store will take the app.
pub const DEFAULT_ORG: &str = "com.example";

/// A crate name: lowercase snake_case, starting with a letter, and not a
/// word Rust or cargo already means something by.
pub fn check_crate_name(name: &str) -> Result<()> {
    let hint = "name it in snake_case, starting with a letter: `bunny new my_app`";
    let mut chars = name.chars();
    match chars.next() {
        None => return Err(Error::usage("the app needs a name").hint(hint)),
        Some(first) if !first.is_ascii_lowercase() => {
            return Err(Error::usage(format!("`{name}` must start with a lowercase letter")).hint(hint));
        }
        Some(_) => {}
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(Error::usage(format!("`{name}` may hold only a-z, 0-9 and `_`")).hint(hint));
    }
    if RUST_KEYWORDS.contains(&name) || RESERVED.contains(&name) {
        return Err(Error::usage(format!("`{name}` is a name Rust or cargo already uses")).hint(hint));
    }
    if name == "bunny_ui" || name.starts_with("bunny_ui_") {
        return Err(Error::usage(format!("`{name}` would shadow the framework's own crates")).hint(hint));
    }
    Ok(())
}

/// What people read, from the crate's name: `my_app` → `My App`.
pub fn display_name(crate_name: &str) -> String {
    crate_name
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A reverse-DNS prefix (`com.example`): one or more segments.
pub fn check_org(org: &str) -> Result<()> {
    check_segments(org, 1).map_err(|error| {
        error.hint("an org is a reverse-DNS prefix: `--org com.yourcompany`")
    })
}

/// An app id (`com.example.notes`): two or more segments, each a letter
/// followed by letters, digits or `_` — the rule every platform's
/// spelling of it can be derived from.
pub fn check_app_id(id: &str) -> Result<()> {
    check_segments(id, 2).map_err(|error| error.hint("an id looks like `com.yourcompany.my_app`"))
}

fn check_segments(id: &str, least: usize) -> Result<()> {
    let segments: Vec<&str> = id.split('.').collect();
    if segments.len() < least {
        return Err(Error::usage(format!("`{id}` needs at least {least} dot-separated parts")));
    }
    for segment in &segments {
        let mut chars = segment.chars();
        let starts = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
        if !starts || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::usage(format!(
                "`{id}`: each part starts with a letter and holds only letters, digits and `_` (`{segment}` does not)"
            )));
        }
    }
    Ok(())
}

/// The id `bunny new` derives: the org, then the crate's name.
pub fn default_id(org: &str, crate_name: &str) -> String {
    format!("{org}.{crate_name}")
}

/// Apple's spelling: bundle ids allow letters, digits, `-` and `.`.
pub fn apple_id(id: &str) -> String {
    id.replace('_', "-")
}

/// Android's spelling: a Java package, so a segment that is a Java
/// keyword takes a trailing `_` (`com.example.new` → `com.example.new_`).
pub fn android_id(id: &str) -> String {
    id.split('.')
        .map(|segment| {
            if JAVA_KEYWORDS.contains(&segment) { format!("{segment}_") } else { segment.to_string() }
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_names_follow_cargo_and_rust() {
        for good in ["my_app", "notes", "app2", "a_b_c"] {
            assert!(check_crate_name(good).is_ok(), "{good}");
        }
        for bad in ["", "My_App", "2app", "my-app", "my app", "fn", "gen", "std", "test", "bunny_ui", "bunny_ui_web"]
        {
            assert!(check_crate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_name_people_read() {
        assert_eq!(display_name("my_app"), "My App");
        assert_eq!(display_name("notes"), "Notes");
        assert_eq!(display_name("photo__lab_2"), "Photo Lab 2");
    }

    #[test]
    fn ids_have_two_parts_of_letters() {
        assert!(check_app_id("com.example.my_app").is_ok());
        assert!(check_app_id("io.bunny.Notes2").is_ok());
        for bad in ["notes", "com..app", "com.2app", "com.my-app", "com.app.", ".com.app"] {
            assert!(check_app_id(bad).is_err(), "{bad}");
        }
        assert!(check_org("com").is_ok());
        assert!(check_org("com.example").is_ok());
        assert!(check_org("com.").is_err());
    }

    #[test]
    fn each_platform_spells_the_id_its_way() {
        let id = default_id(DEFAULT_ORG, "my_app");
        assert_eq!(id, "com.example.my_app");
        assert_eq!(apple_id(&id), "com.example.my-app");
        assert_eq!(android_id(&id), "com.example.my_app");
        assert_eq!(android_id("com.example.new"), "com.example.new_");
        assert_eq!(android_id("com.int.app"), "com.int_.app");
    }
}
