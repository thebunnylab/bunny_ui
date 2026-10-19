//! `bunny build web`: the page as a folder any static host serves. The
//! wasm is the release build — the `web` profile when the workspace has
//! one, `wasm-opt` after it when it is installed — and every file `bunny`
//! puts in the page is named after its content: a host may keep those
//! forever, and only `index.html`, which names them, is asked for again.

use std::fs;
use std::path::Path;
use std::time::Duration;

use super::{Info, Log, Options};
use crate::error::{self, Error, Result};
use crate::platform::{self, web};
use crate::process;
use crate::project::Project;
use crate::templates;
use crate::term;
use crate::wasm;

/// Builds the site into `out`, and answers its files.
pub fn build(project: &Project, options: &Options, out: &Path, log: &mut Log, info: &mut Info) -> Result<()> {
    let run_options = platform::Options { release: options.release, features: options.features.clone(), ..Default::default() };
    let site = web::build(project, &run_options, false)?;
    log.note(&format!("cargo built {} for {}", site.wasm, web::TARGET));
    let mut bytes = fs::read(site.dir.join(&site.wasm)).map_err(error::at(&site.dir))?;
    if options.release {
        bytes = optimize(bytes, &site.dir, log)?;
    }
    let entry = match site.mode {
        web::Mode::Canvas => "start",
        web::Mode::Dom => "start_dom",
    };
    let exports = wasm::exports(&bytes).map_err(|why| Error::new(format!("the optimized wasm: {why}")))?;
    if !exports.iter().any(|name| name == entry) {
        return Err(Error::new(format!("the optimized wasm lost its `{entry}` export")));
    }

    // the page's own files as they are, and those `bunny` puts in it
    // under names of their content
    let injected: Vec<&str> = std::iter::once(site.wasm.as_str()).chain(site.mode.scripts().iter().copied()).collect();
    copy_except(&site.dir, out, &site.dir, &injected)?;
    let wasm_name = hashed(&site.wasm, &bytes);
    fs::write(out.join(&wasm_name), &bytes).map_err(error::at(out))?;
    let mut scripts = Vec::new();
    for script in site.mode.scripts() {
        let text = fs::read(site.dir.join(script)).map_err(error::at(&site.dir))?;
        let name = hashed(script, &text);
        fs::write(out.join(&name), text).map_err(error::at(out))?;
        scripts.push(name);
    }
    let template_path = project.platform_dir("web")?.join("index.html");
    let template = fs::read_to_string(&template_path).map_err(error::at(&template_path))?;
    let page = web::page(&template, &project.name, &wasm_name, &scripts, false)?;
    fs::write(out.join("index.html"), page).map_err(error::at(out))?;
    let immutable: Vec<&str> = std::iter::once(wasm_name.as_str()).chain(scripts.iter().map(String::as_str)).collect();
    fs::write(out.join("_headers"), headers(&immutable)).map_err(error::at(out))?;

    info.field("wasm", &wasm_name);
    info.field("renderer", if site.mode == web::Mode::Canvas { "canvas" } else { "dom" });
    for file in ["index.html", "_headers"].into_iter().chain(immutable.iter().copied()) {
        info.file(file);
    }
    println!("{} {}", term::dim(&format!("{wasm_name}:")), super::size(bytes.len() as u64));
    Ok(())
}

/// `wasm-opt -Oz`, when it is installed: the download is the web's
/// border, and binaryen shaves what the compiler leaves. It reads the
/// wasm's own features section, so it uses no feature the build did not.
fn optimize(bytes: Vec<u8>, dir: &Path, log: &mut Log) -> Result<Vec<u8>> {
    let Some(wasm_opt) = process::which("wasm-opt") else {
        log.note("no wasm-opt on the PATH: the wasm stays as cargo built it");
        println!("{}", term::dim("wasm-opt is not installed: the wasm stays as cargo built it (binaryen has it)"));
        return Ok(bytes);
    };
    let input = dir.join("input.wasm");
    let output = dir.join("optimized.wasm");
    fs::write(&input, &bytes).map_err(error::at(&input))?;
    let args = [input.as_os_str(), std::ffi::OsStr::new("-Oz"), std::ffi::OsStr::new("-o"), output.as_os_str()];
    let out = log.run(&wasm_opt.to_string_lossy(), &args, Duration::from_secs(600))?;
    if !out.ok() {
        return Err(Error::new(format!("wasm-opt failed: {}", out.stderr.trim())).hint("build.log has its whole answer"));
    }
    let optimized = fs::read(&output).map_err(error::at(&output))?;
    let _ = fs::remove_file(&input);
    let _ = fs::remove_file(&output);
    log.note(&format!("wasm-opt: {} → {} bytes", bytes.len(), optimized.len()));
    Ok(optimized)
}

/// `name.<hash>.ext`: the file named after its content.
pub fn hashed(name: &str, bytes: &[u8]) -> String {
    let hash = format!("{:016x}", templates::fnv64(bytes));
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}.{}.{ext}", &hash[..10]),
        None => format!("{name}.{}", &hash[..10]),
    }
}

/// The `_headers` file Netlify and Cloudflare Pages read: the files named
/// after their content kept for a year, the page asked for every time,
/// the wasm served as wasm.
pub fn headers(immutable: &[&str]) -> String {
    let mut text = String::from("/index.html\n  Cache-Control: no-cache\n\n/\n  Cache-Control: no-cache\n");
    for file in immutable {
        text.push_str(&format!("\n/{file}\n  Cache-Control: public, max-age=31536000, immutable\n"));
        if file.ends_with(".wasm") {
            text.push_str("  Content-Type: application/wasm\n");
        }
    }
    text
}

/// Copies `from` into `to`, the files named in `except` (at the top of
/// `root`) left out.
fn copy_except(from: &Path, to: &Path, root: &Path, except: &[&str]) -> Result<()> {
    for entry in fs::read_dir(from).map_err(error::at(from))? {
        let path = entry.map_err(error::at(from))?.path();
        let name = path.file_name().unwrap_or_default();
        let skip = from == root && (name == "index.html" || except.iter().any(|file| name == *file));
        if skip {
            continue;
        }
        let target = to.join(name);
        if path.is_dir() {
            fs::create_dir_all(&target).map_err(error::at(&target))?;
            copy_except(&path, &target, root, except)?;
        } else {
            fs::copy(&path, &target).map_err(error::at(&path))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_named_after_its_content() {
        let one = hashed("glue.js", b"let a = 1;");
        assert!(one.starts_with("glue.") && one.ends_with(".js") && one.len() == "glue.".len() + 10 + ".js".len());
        assert_eq!(one, hashed("glue.js", b"let a = 1;"), "the same content, the same name");
        assert_ne!(one, hashed("glue.js", b"let a = 2;"));
        assert!(hashed("app.wasm", b"\0asm").ends_with(".wasm"));
    }

    #[test]
    fn the_hashed_files_are_kept_and_the_page_is_not() {
        let text = headers(&["app.0123456789.wasm", "glue.abcdef0123.js"]);
        assert!(text.starts_with("/index.html\n  Cache-Control: no-cache\n"));
        assert!(text.contains("/app.0123456789.wasm\n  Cache-Control: public, max-age=31536000, immutable\n  Content-Type: application/wasm\n"));
        assert!(text.contains("/glue.abcdef0123.js\n  Cache-Control: public, max-age=31536000, immutable\n"));
    }
}
