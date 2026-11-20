//! The corpus on disk, and the site a sweep serves: every case's page as
//! written, the bunny views built once into one wasm, and for each case
//! the two pages that boot that wasm — on elements and on WebGL.

use std::fs;
use std::path::{Path, PathBuf};

use super::capture::Viewport;
use crate::cargo::{self, Target};
use crate::error::{self, Error, Result};

/// The size every case is captured at unless its page names another
/// (`<meta name="fidelity" content="width=400, height=300">`). The
/// ratio is the browser's, one for the whole sweep.
pub const DEFAULT_VIEWPORT: Viewport = Viewport { width: 400, height: 300, ratio: super::chrome::RATIO };

/// The wasm's name on the site.
const WASM: &str = "corpus.wasm";

/// One case of the corpus.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub viewport: Viewport,
}

/// The corpus: `fidelity/cases/<name>/case.html` beside `case.rs`.
///
/// # Errors
///
/// No cases folder, a case named in `only` that it lacks, or a page with
/// no `case.rs` beside it.
pub fn cases(fidelity: &Path, only: &[&str]) -> Result<Vec<Case>> {
    let dir = fidelity.join("cases");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map_err(error::at(&dir))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().join("case.html").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    if let Some(unknown) = only.iter().find(|name| !names.iter().any(|known| known == *name)) {
        return Err(Error::usage(format!("no case `{unknown}` in {}", dir.display())));
    }
    names.retain(|name| only.is_empty() || only.contains(&name.as_str()));
    names
        .into_iter()
        .map(|name| {
            let page = dir.join(&name).join("case.html");
            let text = fs::read_to_string(&page).map_err(error::at(&page))?;
            if !dir.join(&name).join("case.rs").is_file() {
                return Err(Error::new(format!("{name}: case.html has no case.rs beside it")));
            }
            Ok(Case { viewport: viewport_of(&text), name })
        })
        .collect()
}

/// The viewport a page asks for in its `fidelity` meta, or the default.
#[must_use]
pub fn viewport_of(page: &str) -> Viewport {
    let Some(at) = page.find("name=\"fidelity\"") else { return DEFAULT_VIEWPORT };
    let tag_end = page[at..].find('>').map_or(page.len(), |end| at + end);
    let tag_start = page[..at].rfind('<').unwrap_or(at);
    let tag = &page[tag_start..tag_end];
    let Some(content) = tag.split("content=\"").nth(1).and_then(|rest| rest.split('"').next()) else {
        return DEFAULT_VIEWPORT;
    };
    let mut viewport = DEFAULT_VIEWPORT;
    for pair in content.split(',') {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let Ok(value) = value.trim().parse::<u32>() else { continue };
        match key.trim() {
            "width" => viewport.width = value,
            "height" => viewport.height = value,
            _ => {}
        }
    }
    viewport
}

/// Builds the corpus crate for the browser, and answers the wasm and the
/// folder of the web shell's glue that came with it.
///
/// # Errors
///
/// The build failed.
pub fn build_wasm(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let built = cargo::build(&cargo::Build {
        manifest: root.join("fidelity/corpus/Cargo.toml"),
        package: String::from("fidelity-corpus"),
        what: Target::CdylibLib,
        release: true,
        target: Some(String::from(crate::platform::web::TARGET)),
        rustc_args: crate::platform::web::stack_args(),
        ..cargo::Build::default()
    })?;
    let glue = built
        .packages
        .iter()
        .find(|(name, _)| name == "bunny_ui_web")
        .map(|(_, dir)| dir.join("glue"))
        .ok_or_else(|| Error::new("the corpus does not build the web shell"))?;
    Ok((built.artifact, glue))
}

/// The entries a case needs that the wasm lacks.
///
/// # Errors
///
/// The bytes are no wasm.
pub fn missing_entries(wasm: &[u8], cases: &[Case]) -> Result<Vec<String>> {
    let exports = crate::wasm::exports(wasm).map_err(|why| Error::new(format!("the corpus wasm: {why}")))?;
    Ok(cases
        .iter()
        .flat_map(|case| [format!("start_dom_{}", case.name), format!("start_gpu_{}", case.name)])
        .filter(|entry| !exports.contains(entry))
        .collect())
}

/// Lays the site out under `site`: the corpus's own files as they are,
/// the wasm and the glue under `bunny/`, and the two shell pages of each
/// case.
///
/// # Errors
///
/// A file that would not copy or write.
pub fn assemble(fidelity: &Path, site: &Path, wasm: &Path, glue: &Path, cases: &[Case]) -> Result<()> {
    let _ = fs::remove_dir_all(site);
    for part in ["cases", "fonts", "assets"] {
        let from = fidelity.join(part);
        if from.is_dir() {
            copy_tree(&from, &site.join(part))?;
        }
    }
    let bunny = site.join("bunny");
    fs::create_dir_all(&bunny).map_err(error::at(&bunny))?;
    fs::copy(wasm, bunny.join(WASM)).map_err(error::at(wasm))?;
    for script in ["surface.js", "glue_gl.js", "glue.js", "glue_dom.js", "media.js"] {
        let from = glue.join(script);
        if from.is_file() {
            fs::copy(&from, bunny.join(script)).map_err(error::at(&from))?;
        }
    }
    for case in cases {
        let dir = bunny.join(&case.name);
        fs::create_dir_all(&dir).map_err(error::at(&dir))?;
        fs::write(dir.join("dom.html"), shell(case, "dom")).map_err(error::at(&dir))?;
        fs::write(dir.join("gpu.html"), shell(case, "gpu")).map_err(error::at(&dir))?;
    }
    Ok(())
}

/// The page that boots one case's bunny view: the corpus's faces loaded
/// first — no text is measured in a face that has not arrived — and the
/// glue of the mode after them, in order.
fn shell(case: &Case, mode: &str) -> String {
    let scripts = if mode == "dom" { r#"["glue_gl.js", "glue_dom.js"]"# } else { r#"["surface.js", "glue_gl.js", "glue.js"]"# };
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>{name} — bunny {mode}</title>
    <link rel="stylesheet" href="/fonts/fonts.css" />
    <style>
      html, body {{ margin: 0; background: #fff; }}
      #app {{ position: fixed; left: 0; top: 0; width: {width}px; height: {height}px; outline: none; }}
    </style>
  </head>
  <body>
    <div id="app" tabindex="0"></div>
    <script>
      window.BUNNY_WASM = "/bunny/{wasm}";
      window.BUNNY_START = "start_{mode}_{name}";
      (async () => {{
        await Promise.all([...document.fonts].map((face) => face.load()));
        for (const script of {scripts}) {{
          await new Promise((done, failed) => {{
            const element = document.createElement("script");
            element.src = "/bunny/" + script;
            element.onload = done;
            element.onerror = failed;
            document.body.appendChild(element);
          }});
        }}
      }})();
    </script>
  </body>
</html>
"#,
        name = case.name,
        width = case.viewport.width,
        height = case.viewport.height,
        wasm = WASM,
    )
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).map_err(error::at(to))?;
    for entry in fs::read_dir(from).map_err(error::at(from))? {
        let path = entry.map_err(error::at(from))?.path();
        let target = to.join(path.file_name().unwrap_or_default());
        if path.is_dir() {
            copy_tree(&path, &target)?;
        } else if path.extension().is_none_or(|ext| ext != "rs") {
            fs::copy(&path, &target).map_err(error::at(&path))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_names_its_viewport() {
        let page = r#"<head><meta charset="utf-8"><meta name="fidelity" content="width=320, height=480"></head>"#;
        assert_eq!(viewport_of(page), Viewport { width: 320, height: 480, ratio: 2 });
        assert_eq!(viewport_of("<head></head>"), DEFAULT_VIEWPORT);
        let reordered = r#"<meta content="height=200" name="fidelity">"#;
        assert_eq!(viewport_of(reordered), Viewport { height: 200, ..DEFAULT_VIEWPORT });
    }

    #[test]
    fn the_shell_boots_the_cases_entry_after_the_faces() {
        let case = Case { name: String::from("stack_gap"), viewport: DEFAULT_VIEWPORT };
        let page = shell(&case, "gpu");
        assert!(page.contains(r#"window.BUNNY_START = "start_gpu_stack_gap";"#));
        assert!(page.contains("width: 400px; height: 300px;"));
        let fonts = page.find("document.fonts").unwrap();
        assert!(fonts < page.find("surface.js").unwrap());
    }
}
