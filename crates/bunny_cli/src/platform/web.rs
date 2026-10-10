//! The browser: the app's library built for wasm, the page put together
//! around it — the project's `web/`, the framework's glue — and served.
//!
//! The glue comes from the `bunny-ui-web` cargo resolved for this very
//! build, so the page and the wasm always speak the same ABI; `bunny`
//! checks the two numbers anyway, before a browser shows a mismatch.

use std::fs;
use std::path::{Path, PathBuf};

use super::Options;
use crate::cargo::{self, Target};
use crate::error::{self, Error, Result};
use crate::project::Project;
use crate::template::{self, Escape};
use crate::templates;
use crate::wasm;

pub const TARGET: &str = "wasm32-unknown-unknown";

/// Which glue the page boots the wasm with — by the entry it exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `start`: the framework paints every pixel.
    Canvas,
    /// `start_dom`: the framework lowers to HTML elements.
    Dom,
}

impl Mode {
    /// The glue scripts, in the order the page loads them.
    pub fn scripts(self) -> &'static [&'static str] {
        match self {
            Mode::Canvas => &["surface.js", "glue_gl.js", "glue.js"],
            Mode::Dom => &["glue_gl.js", "glue_dom.js"],
        }
    }
}

/// A page ready to serve.
pub struct Site {
    pub dir: PathBuf,
    pub mode: Mode,
}

/// Builds the app for the browser and assembles its page. `dev` adds the
/// script that reloads the page and reports its errors to `bunny run`.
pub fn build(project: &Project, options: &Options, dev: bool) -> Result<Site> {
    if !project.has_lib {
        return Err(Error::new("the web runs the app's library, and this app has none")
            .hint("move the app into src/lib.rs, ending in `bunny_ui::app!(…)`"));
    }
    let release = options.release;
    let built = cargo::build(&cargo::Build {
        manifest: project.manifest.clone(),
        package: project.package.clone(),
        what: Target::CdylibLib,
        release,
        profile: (release && project.has_web_profile()).then(|| String::from("web")),
        target: Some(TARGET.to_string()),
        features: options.features.clone(),
        env: project.build_env(),
        rustc_args: Vec::new(),
        quiet: false,
    })?;
    let bytes = fs::read(&built.artifact).map_err(error::at(&built.artifact))?;
    let exports = wasm::exports(&bytes).map_err(|why| Error::new(format!("{}: {why}", built.artifact.display())))?;
    let mode = if exports.iter().any(|name| name == "start") {
        Mode::Canvas
    } else if exports.iter().any(|name| name == "start_dom") {
        Mode::Dom
    } else {
        return Err(Error::new("the wasm exports no `start` for the page to call")
            .hint("end src/lib.rs with `bunny_ui::app!(home)`"));
    };
    let glue = package_dir(&built.packages, "bunny_ui_web")
        .map(|dir| dir.join("glue"))
        .filter(|dir| dir.is_dir())
        .ok_or_else(|| Error::new("no web glue: the app does not build bunny-ui's web shell")
            .hint("depend on `bunny-ui` with its default features"))?;
    check_abi(&glue, package_dir(&built.packages, "bunny_ui_core").as_deref(), mode)?;

    let dir = project.out_dir("web", "site", release);
    super::fresh_dir(&dir)?;
    for script in mode.scripts().iter().chain(&["media.js"]) {
        let from = glue.join(script);
        if from.is_file() {
            fs::copy(&from, dir.join(script)).map_err(error::at(&from))?;
        }
    }
    let wasm_name = built.artifact.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    fs::write(dir.join(&wasm_name), &bytes).map_err(error::at(&dir))?;
    let web = project.platform_dir("web")?;
    copy_tree(&web, &dir, &web.join("index.html"))?;
    let stamp = format!("{:08x}", templates::fnv64(&bytes) as u32);
    let page = page(&fs::read_to_string(web.join("index.html")).map_err(error::at(&web))?, &project.name, &wasm_name, mode, &stamp, dev)?;
    fs::write(dir.join("index.html"), page).map_err(error::at(&dir))?;
    Ok(Site { dir, mode })
}

/// The project's page with its markers filled: the name, and the scripts
/// that boot the wasm.
pub fn page(template_text: &str, name: &str, wasm: &str, mode: Mode, stamp: &str, dev: bool) -> Result<String> {
    if !template_text.contains("id=\"app\"") {
        return Err(Error::new("web/index.html has no element with id=\"app\"").hint("the app draws into <div id=\"app\"></div>"));
    }
    let mut scripts = String::new();
    if dev {
        scripts.push_str("<script src=\"/__bunny/dev.js\"></script>\n    ");
    }
    scripts.push_str(&format!("<script>window.BUNNY_WASM = \"{wasm}?v={stamp}\";</script>"));
    for script in mode.scripts() {
        scripts.push_str(&format!("\n    <script src=\"{script}?v={stamp}\"></script>"));
    }
    let name = Escape::Xml.apply(name);
    template::render("web/index.html", template_text, &[("APP_NAME", &name), ("SCRIPTS", &scripts)], Escape::Raw)
}

fn package_dir(packages: &[(String, PathBuf)], name: &str) -> Option<PathBuf> {
    packages.iter().find(|(package, _)| package == name).map(|(_, dir)| dir.clone())
}

/// The glue's `EXPECTED_ABI` against the core's `ABI_VERSION`: one build
/// of the two always agrees, a mixed one would show the visitor a notice.
fn check_abi(glue: &Path, core: Option<&Path>, mode: Mode) -> Result<()> {
    let script = glue.join(if mode == Mode::Canvas { "glue.js" } else { "glue_dom.js" });
    let page_abi = fs::read_to_string(&script).ok().and_then(|text| number_after(&text, "const EXPECTED_ABI = "));
    let core_abi = core
        .and_then(|dir| fs::read_to_string(dir.join("src/dom.rs")).ok())
        .and_then(|text| number_after(&text, "pub const ABI_VERSION: u32 = "));
    match (page_abi, core_abi) {
        (Some(page), Some(core)) if page != core => Err(Error::new(format!(
            "the web glue speaks ABI {page} and the framework ABI {core}: bunny-ui's crates are of different versions"
        ))
        .hint("give every bunny-ui crate the same version, then `cargo update`")),
        _ => Ok(()),
    }
}

fn number_after(text: &str, prefix: &str) -> Option<u32> {
    let after = &text[text.find(prefix)? + prefix.len()..];
    after.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Copies `from` into `to`, skipping `except` (the page, filled separately)
/// and the stamp `bunny new` left.
fn copy_tree(from: &Path, to: &Path, except: &Path) -> Result<()> {
    for entry in fs::read_dir(from).map_err(error::at(from))? {
        let path = entry.map_err(error::at(from))?.path();
        let name = path.file_name().unwrap_or_default();
        if path == except || name == templates::STAMP {
            continue;
        }
        let target = to.join(name);
        if path.is_dir() {
            fs::create_dir_all(&target).map_err(error::at(&target))?;
            copy_tree(&path, &target, except)?;
        } else {
            fs::copy(&path, &target).map_err(error::at(&path))?;
        }
    }
    Ok(())
}

/// Opens `url` in the default browser.
pub fn open(url: &str) {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<title>@BUNNY_APP_NAME@</title>\n<div id=\"app\"></div>\n    @BUNNY_SCRIPTS@\n";

    #[test]
    fn the_page_boots_the_wasm_with_its_glue() {
        let page = page(PAGE, "Tom & Jerry", "notes.wasm", Mode::Canvas, "1a2b3c4d", true).unwrap();
        assert!(page.contains("<title>Tom &amp; Jerry</title>"));
        let dev = page.find("/__bunny/dev.js").unwrap();
        let wasm = page.find("window.BUNNY_WASM = \"notes.wasm?v=1a2b3c4d\"").unwrap();
        let surface = page.find("surface.js?v=1a2b3c4d").unwrap();
        let glue = page.find("\"glue.js?v=1a2b3c4d\"").unwrap();
        assert!(dev < wasm && wasm < surface && surface < glue, "{page}");
        let dom = super::page(PAGE, "N", "n.wasm", Mode::Dom, "0", false).unwrap();
        assert!(dom.contains("glue_dom.js") && !dom.contains("surface.js") && !dom.contains("dev.js"));
    }

    #[test]
    fn a_page_without_the_app_element_is_refused() {
        assert!(page("<body>@BUNNY_SCRIPTS@</body>", "N", "n.wasm", Mode::Canvas, "0", true).is_err());
    }

    #[test]
    fn the_abi_numbers_are_read_off_the_sources() {
        assert_eq!(number_after("const EXPECTED_ABI = 20;\n", "const EXPECTED_ABI = "), Some(20));
        assert_eq!(number_after("pub const ABI_VERSION: u32 = 21;", "pub const ABI_VERSION: u32 = "), Some(21));
        assert_eq!(number_after("nothing", "const EXPECTED_ABI = "), None);
    }
}
