//! `bunny fidelity sweep`: every case of the fidelity corpus captured
//! three ways in one headless Chrome and compared — see `fidelity/README.md`.

use crate::args::{HELP, Matches, Opt};
use crate::error::{Error, Result};
use crate::fidelity::{self, Options};

pub const SUMMARY: &str = "Compare bunny's two web lowerings against hand-written pages";

pub const USAGE: &str = "bunny fidelity sweep [OPTIONS]";

pub const ABOUT: &str = "\
Captures every case of fidelity/cases three ways in one headless Chrome — the
hand-written case.html (A), the bunny view on elements (B) and on WebGL (C) —
and compares A×B, A×C and B×C: the named boxes first, then the pixels, with
the pixels under text in a bucket of their own. The numbers are checked
against fidelity/baseline.tsv: the sweep fails when one grows. Pictures and a
report land in target/fidelity/.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("only", "CASE", "Capture this case alone (may repeat)"),
    Opt::flag("bless", "Write the numbers that shrank, and new cases, into the baseline"),
    Opt::flag("check-determinism", "Capture everything twice, in two browsers, and require the same bytes"),
    Opt::value("chrome", "PATH", "The browser to capture with (default: BUNNY_CHROME, then gitignore/chrome, then the system's)"),
    HELP,
];

/// Runs `bunny fidelity sweep`.
///
/// # Errors
///
/// A command line it cannot read, a sweep that could not run, or one
/// that found a regression.
pub fn run(matches: &Matches) -> Result<()> {
    match matches.positionals.first().map(String::as_str) {
        Some("sweep") if matches.positionals.len() == 1 => {}
        Some(other) => return Err(Error::usage(format!("unknown fidelity command `{other}`")).hint("bunny fidelity sweep")),
        None => return Err(Error::usage("bunny fidelity sweep — the one command").hint("bunny help fidelity")),
    }
    let options = Options {
        only: matches.values("only").into_iter().map(str::to_owned).collect(),
        bless: matches.flag("bless"),
        check_determinism: matches.flag("check-determinism"),
        chrome: matches.value("chrome").map(str::to_owned),
    };
    if fidelity::sweep(&options)? {
        Ok(())
    } else {
        Err(Error::new("the sweep found a regression").hint("target/fidelity/report.md says where"))
    }
}
