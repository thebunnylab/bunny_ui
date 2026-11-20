//! `bunny import html`: a page written as a bunny view.

use std::path::PathBuf;

use crate::args::{HELP, Matches, Opt};
use crate::error::{Error, Result};
use crate::import::{self, Options};

pub const SUMMARY: &str = "Write an HTML page as a bunny view";

pub const USAGE: &str = "bunny import html <PAGE> [OPTIONS]";

pub const ABOUT: &str = "\
Opens the page in a headless Chrome, reads every box it laid out and the style
it computed for it, and writes the page as a bunny module with
`pub fn view() -> impl View`: flex lines as stacks, blocks as columns, words
as paragraphs of runs, grids and tables cut into stacks, positioned boxes as
overlays, and each box's padding, size, fill, corners, border and shadow as
modifiers. What bunny has no word for is listed, element by element, in a
report beside the module. With --case the page becomes a fidelity case: the
module, the page with every box the view names, and the report.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("root", "DIR", "The folder the page is served from (default: the page's own)"),
    Opt::value("out", "FILE", "The module to write (default: the page's name with .rs)"),
    Opt::value("case", "NAME", "Write fidelity/cases/NAME instead: case.rs, case.html and gaps.md"),
    Opt::value("chrome", "PATH", "The browser to read with (default: BUNNY_CHROME, then gitignore/chrome, then the system's)"),
    HELP,
];

/// Runs `bunny import html`.
///
/// # Errors
///
/// A command line it cannot read, or an import that could not run.
pub fn run(matches: &Matches) -> Result<()> {
    let page = match matches.positionals.as_slice() {
        [kind, page] if kind == "html" => PathBuf::from(page),
        [kind, ..] if kind != "html" => return Err(Error::usage(format!("unknown import `{kind}`")).hint(USAGE)),
        _ => return Err(Error::usage("bunny import html <PAGE>").hint("bunny help import")),
    };
    import::run(&Options {
        page,
        root: matches.value("root").map(PathBuf::from),
        out: matches.value("out").map(PathBuf::from),
        case: matches.value("case").map(str::to_owned),
        chrome: matches.value("chrome").map(str::to_owned),
    })
}
