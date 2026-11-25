//! `bunny migrate`: the crate's source carried to the next bunny-ui by
//! rules, the diff shown first, written only on a yes.

use std::io::IsTerminal;
use std::path::PathBuf;

use crate::args::{HELP, Matches, Opt};
use crate::error::{Error, Result};
use crate::json::{self, Value};
use crate::migrate::{self, Plan, Version};
use crate::term;

pub const SUMMARY: &str = "Carry the crate's source to a newer bunny-ui, the diff shown first";

pub const USAGE: &str = "bunny migrate [PATH] [OPTIONS]";

pub const ABOUT: &str = "\
Rewrites the crate's Rust sources for the API of a newer bunny-ui, and the
bunny-ui version its Cargo.toml asks for, by rules: each change of the API
between two versions is a rewrite of its call sites. The diff of every file
is shown before anything is written; on a terminal you are asked, elsewhere
--yes writes. A place no rule can rewrite surely is listed with its file,
line and what to do, and left as it is. The version to start from is the
one Cargo.toml asks for; the default target is the newest this bunny knows.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("from", "VERSION", "The bunny-ui the source is written for (default: what Cargo.toml asks for)"),
    Opt::value("to", "VERSION", "The bunny-ui to migrate to (default: the newest this bunny knows)"),
    Opt::flag("dry-run", "Show the diff and write nothing"),
    Opt::flag("yes", "Write the changes without asking").short('y'),
    Opt::flag("json", "Answer in JSON — the plan, each file's diff and the gaps — for a tool such as Studio"),
    HELP,
];

/// Runs `bunny migrate`.
///
/// # Errors
///
/// A command line it cannot read, no crate or version, or a file that
/// cannot be read or written.
pub fn run(matches: &Matches) -> Result<()> {
    let dir = match matches.positionals.as_slice() {
        [] => PathBuf::from("."),
        [path] => PathBuf::from(path),
        _ => return Err(Error::usage("bunny migrate takes one crate").hint(USAGE)),
    };
    let version = |name: &str| -> Result<Option<Version>> {
        matches
            .value(name)
            .map(|text| Version::parse(text).ok_or_else(|| Error::usage(format!("--{name} {text}: a version is written like 0.5"))))
            .transpose()
    };
    let plan = migrate::plan(&dir, version("from")?, version("to")?)?;
    let dry = matches.flag("dry-run");
    if matches.flag("json") {
        let written = !dry && matches.flag("yes") && !plan.changes.is_empty();
        if written {
            plan.write()?;
        }
        println!("{}", json::write(&answer(&plan, written)));
        return Ok(());
    }
    if plan.from == plan.to {
        println!("{}", term::dim(&format!("already at bunny-ui {}: nothing to migrate", plan.to)));
        return Ok(());
    }
    show(&plan);
    if plan.changes.is_empty() {
        println!("{}", term::dim("no rule changes this crate: it reads the same in both"));
        return Ok(());
    }
    if dry {
        println!("{}", term::dim("--dry-run: nothing was written"));
        return Ok(());
    }
    let yes = matches.flag("yes")
        || (std::io::stdin().is_terminal() && term::confirm(&format!("Write these changes to {}? [y/N] ", files(plan.changes.len())), false));
    if !yes {
        println!("{}", term::dim("nothing was written: answer yes on a terminal, or run again with --yes"));
        return Ok(());
    }
    plan.write()?;
    println!("{}", term::ok(&format!("migrated to bunny-ui {}: {} written", plan.to, files(plan.changes.len()))));
    if !plan.gaps.is_empty() {
        let places = if plan.gaps.len() == 1 { String::from("1 place") } else { format!("{} places", plan.gaps.len()) };
        println!("{}", term::dim(&format!("{places} above left for you: `cargo check` shows where they no longer build")));
    }
    Ok(())
}

/// `1 file`, `3 files`.
fn files(count: usize) -> String {
    if count == 1 { String::from("1 file") } else { format!("{count} files") }
}

/// The plan on the terminal: the versions, each file's diff, the gaps,
/// the notes.
fn show(plan: &Plan) {
    println!("{}", term::bold(&format!("bunny-ui {} → {}  {}", plan.from, plan.to, plan.dir.display())));
    for change in &plan.changes {
        println!();
        for line in change.diff().lines() {
            let painted = if line.starts_with("+++") || line.starts_with("---") {
                term::bold(line)
            } else if line.starts_with('+') {
                term::green(line)
            } else if line.starts_with('-') {
                term::red(line)
            } else if line.starts_with("@@") {
                term::cyan(line)
            } else {
                line.to_string()
            };
            println!("{painted}");
        }
    }
    if !plan.gaps.is_empty() {
        println!();
        println!("{}", term::bold(&format!("Left as they are — no rule rewrites these surely ({}):", plan.gaps.len())));
        for (path, gap) in &plan.gaps {
            println!("  {} {}", term::yellow(&format!("{path}:{}:{}", gap.line, gap.column)), term::bold(&gap.found));
            println!("    {}", gap.note);
        }
    }
    for note in &plan.notes {
        println!("{}", term::dim(&format!("note: {note}")));
    }
    println!();
}

/// The plan as one JSON document.
fn answer(plan: &Plan, written: bool) -> Value {
    let text = |text: &str| Value::String(text.to_string());
    let files = plan
        .changes
        .iter()
        .map(|change| Value::Object(vec![(String::from("path"), text(&change.path)), (String::from("diff"), text(&change.diff()))]))
        .collect();
    let gaps = plan
        .gaps
        .iter()
        .map(|(path, gap)| {
            Value::Object(vec![
                (String::from("path"), text(path)),
                (String::from("line"), Value::Number(gap.line.to_string())),
                (String::from("column"), Value::Number(gap.column.to_string())),
                (String::from("found"), text(&gap.found)),
                (String::from("note"), text(gap.note)),
            ])
        })
        .collect();
    Value::Object(vec![
        (String::from("from"), text(&plan.from.to_string())),
        (String::from("to"), text(&plan.to.to_string())),
        (String::from("files"), Value::Array(files)),
        (String::from("gaps"), Value::Array(gaps)),
        (String::from("notes"), Value::Array(plan.notes.iter().map(|note| text(note)).collect())),
        (String::from("written"), Value::Bool(written)),
    ])
}
