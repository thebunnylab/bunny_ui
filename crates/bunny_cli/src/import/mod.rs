//! `bunny import html`: a page, read by a browser and written as a bunny
//! view.
//!
//! The browser does the reading — the cascade, the variables, `calc()`,
//! every unit and every inherited value resolved, every box laid out —
//! and the importer writes what it found in bunny's words. What bunny has
//! no word for is listed, element by element, in a report beside the
//! module. Written as a fidelity case, the page goes along with every box
//! the view names, so the sweep can hold the two against each other box
//! by box.

pub mod css;
pub mod emit;
pub mod extract;
pub mod svg;

use std::fmt::Write as _;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{self, Error, Result};
use crate::fidelity::{capture, chrome, site};
use crate::term;

/// What to import and where to write it.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The page.
    pub page: PathBuf,
    /// The folder the page is served from (its own folder by default):
    /// `/fonts/…` and `/assets/…` are found under it.
    pub root: Option<PathBuf>,
    /// The module to write (the page's name with `.rs` by default).
    pub out: Option<PathBuf>,
    /// Write a fidelity case of this name instead: the module, the page
    /// with its boxes named, and the report, in `fidelity/cases/<name>/`.
    pub case: Option<String>,
    /// The browser.
    pub chrome: Option<String>,
}

/// Imports the page.
///
/// # Errors
///
/// A page or folder that cannot be read, a browser that cannot be found
/// or would not load the page, or an output that cannot be written.
pub fn run(options: &Options) -> Result<()> {
    let page = fs::canonicalize(&options.page).map_err(error::at(&options.page))?;
    let root = match &options.root {
        Some(root) => fs::canonicalize(root).map_err(error::at(root))?,
        None => page.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let served = page.strip_prefix(&root).map_err(|_| {
        Error::new(format!("{} is not inside {}", page.display(), root.display())).hint("--root names the folder the page is served from")
    })?;
    let url_path: Vec<String> = served.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
    let source = fs::read_to_string(&page).map_err(error::at(&page))?;
    let viewport = site::viewport_of(&source);

    let (module, html_out, prelude) = match &options.case {
        Some(name) => {
            let dir = crate::fidelity::find_root()?.join("fidelity/cases").join(name);
            fs::create_dir_all(&dir).map_err(error::at(&dir))?;
            (dir.join("case.rs"), Some(dir.join("case.html")), "use crate::prelude::*;")
        }
        None => (options.out.clone().unwrap_or_else(|| page.with_extension("rs")), None, "use bunny_ui::prelude::*;"),
    };
    let module_dir = module.parent().map(Path::to_path_buf).unwrap_or_default();
    let module_dir = fs::canonicalize(&module_dir).unwrap_or(module_dir);
    let drawings_dir = match &options.case {
        Some(_) => module_dir.join("drawings"),
        None => module_dir.join(format!("{}-drawings", module.file_stem().unwrap_or_default().to_string_lossy())),
    };

    let pages = crate::serve::Server::start(root.clone(), "127.0.0.1", 0).map_err(|error| Error::new(format!("the page server: {error}")))?;
    let url = format!("http://127.0.0.1:{}/{}", pages.address.port(), url_path.join("/"));
    let binary = chrome::find(options.chrome.as_deref(), &crate::fidelity::checkouts(&root))?;
    println!("{} {} in {}", term::bold("Reading"), page.display(), binary.display());
    let profile = std::env::temp_dir().join(format!("bunny-import-{}", std::process::id()));
    let mut browser = chrome::Chrome::launch(&binary, profile)?;
    let answer = capture::read(&mut browser, &url, viewport, extract::SCRIPT)?;
    drop(browser);
    let read = extract::parse(&answer)?;

    let converted = emit::convert(
        &read,
        &emit::Options {
            prelude: prelude.to_owned(),
            module_dir,
            site_root: root,
            page_dir: page.parent().map(Path::to_path_buf).unwrap_or_default(),
            drawings_dir: drawings_dir.clone(),
            source: served.display().to_string(),
        },
    );
    fs::write(&module, &converted.code).map_err(error::at(&module))?;
    println!("{} {}", term::green("wrote"), module.display());
    write_drawings(&drawings_dir, &converted.drawings)?;
    if let Some(html_out) = &html_out {
        // the case page lives in its own folder: its relative addresses
        // still resolve from the folder the page came from
        let folder = url_path[..url_path.len().saturating_sub(1)].iter().fold(String::from("/"), |mut folder, part| {
            folder.push_str(part);
            folder.push('/');
            folder
        });
        let named = with_base(&named_only(&read.html, &converted.named), &folder);
        fs::write(html_out, named).map_err(error::at(html_out))?;
        println!("{} {}", term::green("wrote"), html_out.display());
    }
    let report = gap_report(&served.display().to_string(), &converted);
    let report_path = html_out.as_ref().map_or_else(|| module.with_extension("gaps.md"), |html| html.with_file_name("gaps.md"));
    fs::write(&report_path, &report).map_err(error::at(&report_path))?;
    let features: BTreeMap<&str, usize> = converted.gaps.iter().fold(BTreeMap::new(), |mut counts, gap| {
        *counts.entry(gap.feature.as_str()).or_default() += 1;
        counts
    });
    println!(
        "{} {} views named, {} gaps in {} features: {}",
        term::bold("Imported"),
        converted.named.len(),
        converted.gaps.len(),
        features.len(),
        report_path.display()
    );
    Ok(())
}

/// The page's inline drawings, one file each, in place of the ones an
/// earlier import wrote there.
fn write_drawings(dir: &Path, drawings: &[(String, String)]) -> Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("drawing-") && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("svg")) {
                fs::remove_file(&path).map_err(error::at(&path))?;
            }
        }
    }
    if drawings.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(error::at(dir))?;
    for (file, markup) in drawings {
        let path = dir.join(file);
        fs::write(&path, markup).map_err(error::at(&path))?;
    }
    println!("{} {} drawings in {}", term::green("wrote"), drawings.len(), dir.display());
    Ok(())
}

/// The page with a `<base>` at the head of its `<head>`, so its relative
/// addresses resolve from `folder` wherever it is served; a page that
/// names its own base keeps it.
#[must_use]
pub fn with_base(html: &str, folder: &str) -> String {
    if html.contains("<base ") || folder == "/" {
        return html.to_owned();
    }
    html.find("<head>").map_or_else(
        || html.to_owned(),
        |at| format!("{}<base href=\"{folder}\">{}", &html[..at + "<head>".len()], &html[at + "<head>".len()..]),
    )
}

/// The page's markup with only the names the view kept: a box the view
/// does not name would be a box one side lacks.
#[must_use]
pub fn named_only(html: &str, named: &std::collections::BTreeSet<String>) -> String {
    let marker = " data-fid=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(marker) {
        let value_start = at + marker.len();
        let Some(length) = rest[value_start..].find('"') else { break };
        let value = &rest[value_start..value_start + length];
        out.push_str(&rest[..at]);
        if named.contains(value) {
            out.push_str(&rest[at..=value_start + length]);
        }
        rest = &rest[value_start + length + 1..];
    }
    out.push_str(rest);
    out
}

/// The report: every feature the view could not say, how often, and
/// where.
#[must_use]
pub fn gap_report(source: &str, converted: &emit::Converted) -> String {
    let mut by_feature: BTreeMap<&str, Vec<&emit::Gap>> = BTreeMap::new();
    for gap in &converted.gaps {
        by_feature.entry(gap.feature.as_str()).or_default().push(gap);
    }
    let mut report = format!(
        "# Import gaps — {source}\n\n{} views named; {} gaps in {} features.\n\n| feature | count | where | what |\n|---|---|---|---|\n",
        converted.named.len(),
        converted.gaps.len(),
        by_feature.len()
    );
    let mut rows: Vec<(&str, Vec<&emit::Gap>)> = by_feature.into_iter().collect();
    rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (feature, gaps) in rows {
        let ids: Vec<&str> = gaps.iter().take(4).map(|gap| gap.id.as_str()).collect();
        let more = if gaps.len() > 4 { ", …" } else { "" };
        let detail = gaps[0].detail.replace('|', "\\|");
        let _ = writeln!(report, "| `{feature}` | {} | {}{more} | {detail} |", gaps.len(), ids.join(", "));
    }
    // the table names one reason per feature: every drawing that stayed
    // a picture is listed with its own
    let pictures: Vec<&emit::Gap> = converted.gaps.iter().filter(|gap| gap.feature == emit::KEPT_AS_PICTURE).collect();
    if !pictures.is_empty() {
        report.push_str("\n## Drawings kept as pictures\n\n");
        for gap in pictures {
            let _ = writeln!(report, "- `{}`: {}", gap.id, gap.detail);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A case page served from its own folder keeps its relative
    /// addresses: they resolve from the folder the page came from.
    #[test]
    fn a_case_page_resolves_from_its_source_folder() {
        let page = "<html><head><title>t</title></head><body><img src=\"../assets/a.svg\"></body></html>";
        assert_eq!(
            with_base(page, "/apps/"),
            "<html><head><base href=\"/apps/\"><title>t</title></head><body><img src=\"../assets/a.svg\"></body></html>"
        );
        assert_eq!(with_base(page, "/"), page, "a page at the root needs none");
        let based = "<html><head><base href=\"/x/\"></head></html>";
        assert_eq!(with_base(based, "/apps/"), based, "a page's own base stays");
    }

    #[test]
    fn only_the_named_boxes_keep_their_names() {
        let named = std::collections::BTreeSet::from(["a-1".to_owned()]);
        let html = r#"<div data-fid="a-1"><span data-fid="b-2">x</span></div>"#;
        assert_eq!(named_only(html, &named), r#"<div data-fid="a-1"><span>x</span></div>"#);
    }
}
