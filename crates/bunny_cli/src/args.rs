//! The command line, read by hand: each command declares its options,
//! and what is left is positionals and what follows `--`.
//!
//! The grammar is the one every Unix tool speaks: `--name value`,
//! `--name=value`, `-n value`, `-nvalue`, flags bunched as `-ab`, and
//! `--` handing the rest to the app untouched.

use crate::error::{Error, Result};

/// One option a command accepts.
#[derive(Clone, Copy, Debug)]
pub struct Opt {
    pub long: &'static str,
    pub short: Option<char>,
    /// The placeholder of its value in the help (`NAME`); `None` for a
    /// flag.
    pub value: Option<&'static str>,
    pub help: &'static str,
    /// Left out of the help — a door for the project's own tests.
    pub hidden: bool,
}

impl Opt {
    pub const fn flag(long: &'static str, help: &'static str) -> Opt {
        Opt { long, short: None, value: None, help, hidden: false }
    }

    pub const fn value(long: &'static str, value: &'static str, help: &'static str) -> Opt {
        Opt { long, short: None, value: Some(value), help, hidden: false }
    }

    pub const fn short(mut self, short: char) -> Opt {
        self.short = Some(short);
        self
    }

    pub const fn hidden(mut self) -> Opt {
        self.hidden = true;
        self
    }
}

/// The help flag every command answers.
pub const HELP: Opt = Opt::flag("help", "Print this help").short('h');

/// What a command line said.
#[derive(Debug, Default)]
pub struct Matches {
    flags: Vec<&'static str>,
    values: Vec<(&'static str, String)>,
    pub positionals: Vec<String>,
    /// Everything after `--`, for the app.
    pub rest: Vec<String>,
}

impl Matches {
    pub fn flag(&self, long: &str) -> bool {
        self.flags.contains(&long)
    }

    /// The last value given for `long` — a later `--name` overrides an
    /// earlier one, as in every tool.
    pub fn value(&self, long: &str) -> Option<&str> {
        self.values.iter().rev().find(|(name, _)| *name == long).map(|(_, value)| value.as_str())
    }

    /// Every value given for `long`, in order — for an option that may
    /// repeat.
    pub fn values(&self, long: &str) -> Vec<&str> {
        self.values.iter().filter(|(name, _)| *name == long).map(|(_, value)| value.as_str()).collect()
    }
}

/// Reads `args` against `options`. An unknown option is a usage error
/// that names the nearest known one.
pub fn parse(options: &[Opt], args: &[String]) -> Result<Matches> {
    let mut matches = Matches::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if arg == "--" {
            matches.rest.extend(args[index..].iter().cloned());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let Some(option) = options.iter().find(|option| option.long == name) else {
                return Err(unknown(&format!("--{name}"), options));
            };
            match option.value {
                Some(_) => {
                    let value = match inline {
                        Some(value) => value,
                        None => take_value(args, &mut index, &format!("--{name}"))?,
                    };
                    matches.values.push((option.long, value));
                }
                None => {
                    if inline.is_some() {
                        return Err(Error::usage(format!("`--{name}` takes no value")));
                    }
                    matches.flags.push(option.long);
                }
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            // a bunch of short flags, the last of which may take the
            // rest of the word (or the next word) as its value
            let letters: Vec<char> = arg[1..].chars().collect();
            let mut at = 0;
            while at < letters.len() {
                let letter = letters[at];
                at += 1;
                let Some(option) = options.iter().find(|option| option.short == Some(letter)) else {
                    return Err(unknown(&format!("-{letter}"), options));
                };
                if option.value.is_some() {
                    let attached: String = letters[at..].iter().collect();
                    let value = if attached.is_empty() {
                        take_value(args, &mut index, &format!("-{letter}"))?
                    } else {
                        attached
                    };
                    matches.values.push((option.long, value));
                    break;
                }
                matches.flags.push(option.long);
            }
        } else {
            matches.positionals.push(arg.clone());
        }
    }
    Ok(matches)
}

fn take_value(args: &[String], index: &mut usize, name: &str) -> Result<String> {
    match args.get(*index) {
        Some(value) => {
            *index += 1;
            Ok(value.clone())
        }
        None => Err(Error::usage(format!("`{name}` needs a value"))),
    }
}

fn unknown(given: &str, options: &[Opt]) -> Error {
    let names: Vec<String> =
        options.iter().filter(|option| !option.hidden).map(|option| format!("--{}", option.long)).collect();
    let error = Error::usage(format!("unknown option `{given}`"));
    match nearest(given, names.iter().map(String::as_str)) {
        Some(near) => error.hint(format!("did you mean `{near}`?")),
        None => error,
    }
}

/// The candidate within two edits of `word`, if one is — what a typo
/// most likely meant.
pub fn nearest<'a>(word: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .map(|candidate| (distance(word, candidate), candidate))
        .filter(|(distance, _)| *distance <= 2)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance, by characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb { diagonal } else { 1 + diagonal.min(above).min(row[j]) };
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The options block of a command's help.
pub fn help_block(options: &[Opt]) -> String {
    let lines: Vec<(String, &str)> = options
        .iter()
        .filter(|option| !option.hidden)
        .map(|option| {
            let short = option.short.map_or(String::from("    "), |short| format!("-{short}, "));
            let value = option.value.map_or(String::new(), |value| format!(" <{value}>"));
            (format!("{short}--{}{value}", option.long), option.help)
        })
        .collect();
    let width = lines.iter().map(|(left, _)| left.len()).max().unwrap_or(0);
    lines.iter().map(|(left, help)| format!("  {left:width$}  {help}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTIONS: &[Opt] = &[
        Opt::value("device", "ID", "").short('d'),
        Opt::flag("release", "").short('r'),
        Opt::flag("verbose", "").short('v'),
        Opt::value("name", "NAME", ""),
        Opt::value("secret", "X", "").hidden(),
    ];

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn every_spelling_of_a_value_reads_the_same() {
        for line in ["-d ios", "-dios", "--device ios", "--device=ios"] {
            let matches = parse(OPTIONS, &words(line)).unwrap();
            assert_eq!(matches.value("device"), Some("ios"), "{line}");
        }
    }

    #[test]
    fn bunched_flags_and_a_trailing_value() {
        let matches = parse(OPTIONS, &words("-rv -rvd web app")).unwrap();
        assert!(matches.flag("release") && matches.flag("verbose"));
        assert_eq!(matches.value("device"), Some("web"));
        assert_eq!(matches.positionals, vec!["app"]);
    }

    #[test]
    fn the_rest_goes_to_the_app_untouched() {
        let matches = parse(OPTIONS, &words("run -- --release -x")).unwrap();
        assert_eq!(matches.positionals, vec!["run"]);
        assert!(!matches.flag("release"));
        assert_eq!(matches.rest, vec!["--release", "-x"]);
    }

    #[test]
    fn the_last_value_wins_and_all_are_kept() {
        let matches = parse(OPTIONS, &words("--name a --name b")).unwrap();
        assert_eq!(matches.value("name"), Some("b"));
        assert_eq!(matches.values("name"), vec!["a", "b"]);
    }

    #[test]
    fn a_typo_names_what_it_meant() {
        let error = parse(OPTIONS, &words("--relase")).unwrap_err();
        assert_eq!(error.code, 2);
        assert_eq!(error.hint.as_deref(), Some("did you mean `--release`?"));
        // a hidden option is never suggested
        let error = parse(OPTIONS, &words("--secrets")).unwrap_err();
        assert_eq!(error.hint, None);
    }

    #[test]
    fn a_missing_value_and_a_value_on_a_flag_are_refused() {
        assert_eq!(parse(OPTIONS, &words("--device")).unwrap_err().code, 2);
        assert_eq!(parse(OPTIONS, &words("--release=yes")).unwrap_err().code, 2);
        assert!(parse(OPTIONS, &words("-x")).is_err());
    }

    #[test]
    fn distance_counts_edits() {
        assert_eq!(distance("doctor", "doctor"), 0);
        assert_eq!(distance("docter", "doctor"), 1);
        assert_eq!(distance("dcotor", "doctor"), 2);
        assert_eq!(nearest("bulid", ["build", "run", "new"].into_iter()), Some("build"));
        assert_eq!(nearest("xyzzy", ["build", "run"].into_iter()), None);
    }

    #[test]
    fn the_help_aligns_and_hides() {
        let block = help_block(OPTIONS);
        assert!(block.contains("-d, --device <ID>"));
        assert!(block.contains("    --name <NAME>"));
        assert!(!block.contains("secret"));
    }
}
