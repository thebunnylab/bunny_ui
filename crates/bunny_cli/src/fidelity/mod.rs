//! The fidelity harness: a hand-written page and its bunny view, compared.
//!
//! Each case of `fidelity/cases` is a page written by hand and the bunny
//! view meant to look the same. One headless Chrome captures three
//! renderings of each — the page (A), the view lowered to elements (B)
//! and the view drawn by `WebGL` (C) — and three comparisons say who
//! disagrees with whom: A×B and A×C against the truth, B×C between
//! bunny's own two lowerings, which tells a shared mistake (layout, a
//! measured word) from one backend's.

pub mod baseline;
pub mod capture;
pub mod cdp;
pub mod chrome;
pub mod compare;
pub mod site;
pub mod ws;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use baseline::{Baseline, Pair, Verdict};
use capture::{Kind, Shot};
use compare::Comparison;

use crate::error::{self, Error, Result};
use crate::formats::png;
use crate::term;

/// What a sweep was asked.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The cases to capture; empty for every case.
    pub only: Vec<String>,
    /// Write the numbers that shrank into the baseline.
    pub bless: bool,
    /// Capture everything twice, in two browsers, and require the same
    /// bytes.
    pub check_determinism: bool,
    /// The browser binary.
    pub chrome: Option<String>,
}

/// The three captures of one case.
struct Captured {
    case: site::Case,
    shots: [Shot; 3],
}

/// The checkout holding `fidelity/cases`, from the working directory up.
pub(crate) fn find_root() -> Result<PathBuf> {
    let start = std::env::current_dir().map_err(|error| Error::new(error.to_string()))?;
    start
        .ancestors()
        .find(|dir| dir.join("fidelity/cases").is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::new("no fidelity/cases here or above").hint("run it inside the bunny_ui checkout"))
}

/// This checkout, and the main one when this is a worktree — where a
/// browser kept under `gitignore/` lives.
pub(crate) fn checkouts(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.to_path_buf()];
    let common = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(root)
        .output();
    if let Ok(out) = common
        && out.status.success()
        && let Some(main) = Path::new(String::from_utf8_lossy(&out.stdout).trim()).parent()
        && main != root
    {
        found.push(main.to_path_buf());
    }
    found
}

/// Captures every case in one browser.
fn capture_all(binary: &Path, profile: PathBuf, base: &str, cases: &[site::Case]) -> Result<(String, Vec<Captured>)> {
    let mut browser = chrome::Chrome::launch(binary, profile)?;
    let mut captured = Vec::new();
    for case in cases {
        let urls = [
            format!("{base}cases/{}/case.html", case.name),
            format!("{base}bunny/{}/dom.html", case.name),
            format!("{base}bunny/{}/gpu.html?present=gpu", case.name),
        ];
        let mut shots = Vec::new();
        for (kind, url) in Kind::ALL.into_iter().zip(&urls) {
            let shot = capture::capture(&mut browser, url, kind, case.viewport)
                .map_err(|error| Error::new(format!("{} {}: {}", case.name, kind.letter(), error.message)))?;
            shots.push(shot);
        }
        let shots: [Shot; 3] = shots.try_into().map_err(|_| Error::new("three captures"))?;
        print!("{}", term::dim("."));
        let _ = std::io::Write::flush(&mut std::io::stdout());
        captured.push(Captured { case: case.clone(), shots });
    }
    println!();
    Ok((browser.version.clone(), captured))
}

/// The captures two runs disagree on, by case and letter.
fn nondeterministic(first: &[Captured], second: &[Captured]) -> Vec<String> {
    let mut differ = Vec::new();
    for (a, b) in first.iter().zip(second) {
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            let (x, y) = (&a.shots[index], &b.shots[index]);
            if x.rgba != y.rgba {
                let pixels = x.rgba.as_chunks::<4>().0.iter().zip(y.rgba.as_chunks::<4>().0).filter(|(p, q)| p != q).count();
                differ.push(format!("{} {}: {pixels} pixels differ between runs", a.case.name, kind.letter()));
            }
            if x.boxes != y.boxes || x.texts != y.texts {
                differ.push(format!("{} {}: the boxes differ between runs", a.case.name, kind.letter()));
            }
        }
    }
    differ
}

/// Builds the corpus, captures every case, compares, reports, and holds
/// the numbers against the baseline. `Ok(false)` when a number grew or
/// two runs differed.
///
/// # Errors
///
/// No corpus, a build that failed, no browser, or a page that never
/// drew.
pub fn sweep(options: &Options) -> Result<bool> {
    let root = find_root()?;
    let fidelity = root.join("fidelity");
    let only: Vec<&str> = options.only.iter().map(String::as_str).collect();
    let cases = site::cases(&fidelity, &only)?;
    let out = root.join("target/fidelity");
    println!("{} the corpus for the browser", term::bold("Building"));
    let (wasm, glue) = site::build_wasm(&root)?;
    let bytes = fs::read(&wasm).map_err(error::at(&wasm))?;
    let missing = site::missing_entries(&bytes, &cases)?;
    if !missing.is_empty() {
        return Err(Error::new(format!("the corpus wasm lacks {}", missing.join(", ")))
            .hint("each case's case.rs needs `pub fn view() -> impl View`; the corpus's build script exports it"));
    }
    let site_dir = out.join("site");
    site::assemble(&fidelity, &site_dir, &wasm, &glue, &cases)?;
    let server = crate::serve::Server::start(site_dir, "127.0.0.1", 0).map_err(|error| Error::new(format!("the corpus server: {error}")))?;
    let base = format!("http://127.0.0.1:{}/", server.address.port());
    let binary = chrome::find(options.chrome.as_deref(), &checkouts(&root))?;
    println!("{} {} case{} in {}", term::bold("Capturing"), cases.len(), if cases.len() == 1 { "" } else { "s" }, binary.display());
    let (version, captured) = capture_all(&binary, out.join("profile-0"), &base, &cases)?;
    let mut steady = true;
    if options.check_determinism {
        let (_, again) = capture_all(&binary, out.join("profile-1"), &base, &cases)?;
        let differ = nondeterministic(&captured, &again);
        if differ.is_empty() {
            println!("{} two runs, the same bytes", term::green("deterministic:"));
        } else {
            steady = false;
            for line in &differ {
                println!("{} {line}", term::red("nondeterministic:"));
            }
        }
    }

    let mut measured = Vec::new();
    for item in &captured {
        let dir = out.join("cases").join(&item.case.name);
        fs::create_dir_all(&dir).map_err(error::at(&dir))?;
        for (shot, kind) in item.shots.iter().zip(Kind::ALL) {
            write_png(&dir.join(format!("{}.png", kind.letter())), shot.width, shot.height, &shot.rgba)?;
        }
        let ratio = item.case.viewport.ratio;
        let [a, b, c] = &item.shots;
        let pairs = [(Pair::AB, compare::compare(a, b, ratio)), (Pair::AC, compare::compare(a, c, ratio)), (Pair::BC, compare::compare(b, c, ratio))];
        for (pair, comparison) in &pairs {
            write_png(&dir.join(format!("diff-{}.png", pair.name())), a.width, a.height, &comparison.image)?;
        }
        measured.push((item, pairs));
    }

    let path = fidelity.join("baseline.tsv");
    let kept = match fs::read_to_string(&path) {
        Ok(text) => Baseline::parse(&text).map_err(|why| Error::new(format!("{}: {why}", path.display())))?,
        Err(_) => Baseline::default(),
    };
    if let Some(was) = kept.about("chrome")
        && was != version
    {
        println!("{} the baseline was measured with {was}, this sweep with {version}", term::yellow("warning:"));
    }
    let mut verdicts = Vec::new();
    for (item, pairs) in &measured {
        for (pair, comparison) in pairs {
            let key = (item.case.name.clone(), *pair);
            verdicts.push((key.clone(), baseline::judge(kept.scores.get(&key).copied(), comparison.score()), comparison.score()));
        }
    }
    let report = report(&version, &measured, &verdicts);
    fs::write(out.join("report.md"), &report).map_err(error::at(&out))?;
    print_table(&measured, &verdicts);

    let worse: Vec<_> = verdicts.iter().filter(|(_, verdict, _)| *verdict == Verdict::Worse).collect();
    for ((case, pair), _, score) in &worse {
        let was = kept.scores.get(&(case.clone(), *pair)).copied().unwrap_or_default();
        println!("{} {case} {}: {:?} → {:?} ({})", term::red("worse:"), pair.name(), was, score, baseline::COLUMNS.join(", "));
    }
    let moved = verdicts.iter().any(|(_, verdict, _)| matches!(verdict, Verdict::Better | Verdict::New));
    if options.bless {
        if worse.is_empty() {
            let mut next = kept;
            next.about = vec![
                ("chrome".to_owned(), version),
                ("flags".to_owned(), chrome::FLAGS.join(" ")),
            ];
            for (key, _, score) in &verdicts {
                next.scores.insert(key.clone(), *score);
            }
            fs::write(&path, next.write()).map_err(error::at(&path))?;
            println!("{} {}", term::green("blessed:"), path.display());
        } else {
            println!("{} a number grew: nothing is blessed until it shrinks back", term::red("refused:"));
        }
    } else if moved && worse.is_empty() {
        println!("{} numbers shrank or cases are new: `bunny fidelity sweep --bless` records them", term::green("better:"));
    }
    println!("{} {}", term::dim("report:"), out.join("report.md").display());
    Ok(worse.is_empty() && steady)
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    fs::write(path, png::encode(width, height, rgba)?).map_err(error::at(path))
}

type Measured<'a> = (&'a Captured, [(Pair, Comparison); 3]);
type Judged = ((String, Pair), Verdict, baseline::Score);

const fn verdict_word(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::New => "new",
        Verdict::Same => "same",
        Verdict::Better => "better",
        Verdict::Worse => "WORSE",
    }
}

fn print_table(measured: &[Measured<'_>], verdicts: &[Judged]) {
    let width = measured.iter().map(|(item, _)| item.case.name.len()).max().unwrap_or(4).max(4);
    println!("\n{:width$}  {:>17}  {:>17}  {:>17}", "case", "A×B geo/out/text", "A×C geo/out/text", "B×C geo/out/text");
    for (item, pairs) in measured {
        let cells: Vec<String> = pairs
            .iter()
            .map(|(pair, comparison)| {
                let [geometry, outside, in_text] = comparison.score();
                let verdict = verdicts
                    .iter()
                    .find(|((case, at), _, _)| case == &item.case.name && at == pair)
                    .map_or(Verdict::New, |(_, verdict, _)| *verdict);
                let mark = match verdict {
                    Verdict::Worse => "!",
                    Verdict::Better => "↓",
                    _ => " ",
                };
                format!("{geometry}/{outside}/{in_text}{mark}")
            })
            .collect();
        println!("{:width$}  {:>17}  {:>17}  {:>17}", item.case.name, cells[0], cells[1], cells[2]);
    }
    println!();
}

fn rect(rect: capture::Rect) -> String {
    format!("({:.2}, {:.2}) {:.2}×{:.2}", rect.x, rect.y, rect.w, rect.h)
}

/// The sweep in Markdown: the table, then each case's disagreements.
fn report(version: &str, measured: &[Measured<'_>], verdicts: &[Judged]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Fidelity sweep\n\n{version}; flags `{}`.\n", chrome::FLAGS.join(" "));
    let _ = writeln!(out, "Each cell: nodes whose geometry differs / pixels that differ outside text / pixels that differ under text.\n");
    let _ = writeln!(out, "| case | A×B | A×C | B×C |\n|---|---|---|---|");
    for (item, pairs) in measured {
        let cells: Vec<String> = pairs
            .iter()
            .map(|(pair, comparison)| {
                let [geometry, outside, in_text] = comparison.score();
                let verdict = verdicts
                    .iter()
                    .find(|((case, at), _, _)| case == &item.case.name && at == pair)
                    .map_or(Verdict::New, |(_, verdict, _)| *verdict);
                format!("{geometry} / {outside} / {in_text} ({})", verdict_word(verdict))
            })
            .collect();
        let _ = writeln!(out, "| {} | {} |", item.case.name, cells.join(" | "));
    }
    for (item, pairs) in measured {
        let viewport = item.case.viewport;
        let _ = writeln!(out, "\n## {}\n\n{}×{} at {}×.", item.case.name, viewport.width, viewport.height, viewport.ratio);
        for (shot, kind) in item.shots.iter().zip(Kind::ALL) {
            for note in &shot.notes {
                let _ = writeln!(out, "- {} said: {note}", kind.letter());
            }
        }
        for (pair, comparison) in pairs {
            let _ = writeln!(
                out,
                "\n### {}\n\n{} node(s) differ; {} pixel(s) differ outside text, {} under text; largest channel difference {}. Picture: `cases/{}/diff-{}.png`.\n",
                pair.name(),
                comparison.nodes.len(),
                comparison.outside,
                comparison.in_text,
                comparison.max_delta,
                item.case.name,
                pair.name(),
            );
            let mut nodes = comparison.nodes.clone();
            nodes.sort_by(|a, b| b.worst().total_cmp(&a.worst()));
            for node in &nodes {
                let sides = format!(
                    "{} → {}",
                    node.first.map_or_else(|| String::from("none"), rect),
                    node.second.map_or_else(|| String::from("none"), rect)
                );
                let _ = writeln!(out, "- `{}` {}: {sides}", node.id, node.describe());
            }
            for region in comparison.regions.iter().take(8) {
                let _ = writeln!(
                    out,
                    "- region {}: {} px{}",
                    rect(region.bounds),
                    region.pixels,
                    if region.in_text { " (text)" } else { "" }
                );
            }
            if comparison.regions.len() > 8 {
                let _ = writeln!(out, "- … {} more regions", comparison.regions.len() - 8);
            }
        }
    }
    out
}
