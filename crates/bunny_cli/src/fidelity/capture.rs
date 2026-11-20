//! One page, captured: loaded at a fixed size and ratio with time and
//! chance frozen, waited on until nothing is left to arrive and two
//! frames show the same picture, then read for its boxes and shot.

use std::time::{Duration, Instant};

use super::cdp::{self, Session};
use super::chrome::Chrome;
use crate::error::{Error, Result};
use crate::formats::{base64, png};
use crate::json::Value;

/// The three renderings of one case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A: the hand-written page — the truth.
    Html,
    /// B: the bunny view, lowered to elements.
    Dom,
    /// C: the same view, drawn by the WebGL tier.
    Gpu,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Html, Self::Dom, Self::Gpu];

    #[must_use]
    pub const fn letter(self) -> &'static str {
        match self {
            Self::Html => "A",
            Self::Dom => "B",
            Self::Gpu => "C",
        }
    }
}

/// A box in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The page's size and pixel ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub ratio: u32,
}

/// What one capture saw.
#[derive(Clone, Debug)]
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Every named box, by its id, in document or placement order.
    pub boxes: Vec<(String, Rect)>,
    /// Where text is drawn.
    pub texts: Vec<Rect>,
    /// What the page said on its console while it loaded.
    pub notes: Vec<String>,
}

/// The instant every page starts at: 2026-01-01T00:00:00Z.
const INSTANT_SECONDS: u64 = 1_767_225_600;

/// How much virtual time a page's load is given: the faces, the wasm,
/// the first frame and a picture's decode all land well inside it.
const VIRTUAL_BUDGET_MS: u64 = 5000;

/// Runs before any of the page's own scripts: the clock reads one
/// instant, and chance is a fixed sequence.
const FREEZE: &str = r"(() => {
  const instant = 1767225600000;
  const Real = Date;
  class Frozen extends Real {
    constructor(...args) { if (args.length) { super(...args); } else { super(instant); } }
    static now() { return instant; }
  }
  globalThis.Date = Frozen;
  let seed = 0x2545f491;
  Math.random = () => {
    seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5;
    return (seed >>> 0) / 4294967296;
  };
})();";

/// Waits for the page's faces and pictures, and for the bunny shell to
/// have drawn, then for two animation frames.
fn ready_script(kind: Kind) -> String {
    let drawn = match kind {
        Kind::Html => "true",
        Kind::Dom => "!!(window.__bunnyBoot && window.__bunnyBoot.start !== undefined)",
        Kind::Gpu => "!!(window.__bunny && window.__bunny.bunny_test_frames_json && window.__bunny.bunny_test_frames_json() > 2)",
    };
    format!(
        r"(async () => {{
  const until = performance.now() + 20000;
  const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
  while (!({drawn})) {{
    if (performance.now() > until) return 'the page never drew';
    await sleep(10);
  }}
  await document.fonts.ready;
  for (const image of document.images) {{
    if (!image.complete) await new Promise((done) => {{ image.onload = done; image.onerror = done; }});
    if (image.decode) await image.decode().catch(() => {{}});
  }}
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  return 'ready';
}})()"
    )
}

/// Every box and every run of text, as JSON text — read from the page
/// itself for A and B, from the shell's layout export for C.
const fn geometry_script(kind: Kind) -> &'static str {
    match kind {
        Kind::Html | Kind::Dom => {
            r"((attribute) => {
  const box = (rect) => ({ x: rect.x, y: rect.y, w: rect.width, h: rect.height });
  const ids = [...document.querySelectorAll('[' + attribute + ']')]
    .map((element) => Object.assign({ id: element.getAttribute(attribute) }, box(element.getBoundingClientRect())));
  const texts = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  while (walker.nextNode()) {
    const node = walker.currentNode;
    if (!node.textContent.trim()) continue;
    const style = getComputedStyle(node.parentElement);
    if (style.visibility === 'hidden' || style.display === 'none') continue;
    range.selectNodeContents(node);
    for (const rect of range.getClientRects()) if (rect.width > 0 && rect.height > 0) texts.push(box(rect));
  }
  for (const field of document.querySelectorAll('input, textarea')) texts.push(box(field.getBoundingClientRect()));
  return JSON.stringify({ ids, texts });
})"
        }
        Kind::Gpu => {
            r"(() => {
  const wasm = window.__bunny;
  const len = wasm.bunny_test_frames_json() >>> 0;
  const ptr = wasm.bunny_probe_ptr() >>> 0;
  const answer = JSON.parse(new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, ptr, len)));
  // the tier says whether it presents: a page taller than the device
  // draws shows its bands on a 2D surface
  answer.gpu = wasm.bunny_gpu_presenting() === 1;
  return JSON.stringify(answer);
})()"
        }
    }
}

fn rect_of(value: &Value) -> Option<Rect> {
    let number = |key: &str| value.get(key).and_then(|number| match number {
        Value::Number(text) => text.parse::<f64>().ok(),
        _ => None,
    });
    Some(Rect { x: number("x")?, y: number("y")?, w: number("w")?, h: number("h")? })
}

/// Captures `url` as `kind` sees it.
///
/// # Errors
///
/// The page never drew, never held still, or the browser failed; a C
/// page that fell back from the `WebGL` tier to the CPU is refused.
pub fn capture(chrome: &mut Chrome, url: &str, kind: Kind, viewport: Viewport) -> Result<Shot> {
    let mut page = chrome.open_page()?;
    let shot = capture_on(&mut page.session, url, kind, viewport);
    chrome.close_page(page)?;
    shot
}

/// One page loaded the way a capture loads it — frozen clock and
/// chance, the virtual load, every face and picture in — and asked one
/// question: the value `script` evaluates to.
///
/// # Errors
///
/// A page that would not load or settle, or a script that threw.
pub fn read(chrome: &mut Chrome, url: &str, viewport: Viewport, script: &str) -> Result<Value> {
    let mut page = chrome.open_page()?;
    let answer = load(&mut page.session, url, viewport)
        .and_then(|()| settle(&mut page.session, Kind::Html))
        .and_then(|()| page.session.evaluate(script));
    chrome.close_page(page)?;
    answer
}

fn capture_on(session: &mut Session, url: &str, kind: Kind, viewport: Viewport) -> Result<Shot> {
    load(session, url, viewport)?;
    settle(session, kind)?;
    finish(session, kind)
}

/// Loads `url` at `viewport`, with the clock and chance frozen and the
/// load run on the virtual clock.
fn load(session: &mut Session, url: &str, viewport: Viewport) -> Result<()> {
    for domain in ["Page.enable", "Network.enable", "Runtime.enable"] {
        session.call(domain, cdp::object(&[]))?;
    }
    session.call(
        "Emulation.setDeviceMetricsOverride",
        cdp::object(&[
            ("width", cdp::number(viewport.width)),
            ("height", cdp::number(viewport.height)),
            // the device's own ratio (`--force-device-scale-factor`), not an
            // emulated one
            ("deviceScaleFactor", cdp::number(0)),
            ("mobile", Value::Bool(false)),
            ("screenWidth", cdp::number(viewport.width)),
            ("screenHeight", cdp::number(viewport.height)),
        ]),
    )?;
    session.call("Emulation.setEmulatedMedia", cdp::object(&[(
        "features",
        Value::Array(vec![
            cdp::object(&[("name", cdp::text("prefers-color-scheme")), ("value", cdp::text("light"))]),
            cdp::object(&[("name", cdp::text("prefers-reduced-motion")), ("value", cdp::text("reduce"))]),
        ]),
    )]))?;
    session.call("Page.addScriptToEvaluateOnNewDocument", cdp::object(&[("source", cdp::text(FREEZE))]))?;
    // the load runs on the browser's virtual clock. Paused at a fixed
    // instant while the navigation starts, then given a fixed budget that
    // stands still while a fetch is pending: timers and frames see the
    // same time whatever the machine's speed. A budget granted before the
    // navigation would be spent at once on the blank page.
    session.call(
        "Emulation.setVirtualTimePolicy",
        cdp::object(&[("policy", cdp::text("pause")), ("initialVirtualTime", cdp::number(INSTANT_SECONDS))]),
    )?;
    session.call("Page.navigate", cdp::object(&[("url", cdp::text(url))]))?;
    session.call(
        "Emulation.setVirtualTimePolicy",
        cdp::object(&[("policy", cdp::text("pauseIfNetworkFetchesPending")), ("budget", cdp::number(VIRTUAL_BUDGET_MS))]),
    )?;
    session.wait_for("Page.loadEventFired", Duration::from_secs(30))?;
    session.wait_for("Emulation.virtualTimeBudgetExpired", Duration::from_secs(60))?;
    // then the clock runs again, as fast as the page lets it, for the
    // waits that confirm the page is still
    session.call("Emulation.setVirtualTimePolicy", cdp::object(&[("policy", cdp::text("advance"))]))?;
    Ok(())
}

fn finish(session: &mut Session, kind: Kind) -> Result<Shot> {
    let geometry = match kind {
        Kind::Html => format!("{}('data-fid')", geometry_script(kind)),
        Kind::Dom => format!("{}('data-testid')", geometry_script(kind)),
        Kind::Gpu => geometry_script(kind).to_owned(),
    };
    let answer = session.evaluate(&geometry)?;
    let answer = crate::json::parse(answer.as_str().unwrap_or("{}")).map_err(Error::new)?;
    if kind == Kind::Gpu && answer.get("gpu").and_then(Value::as_bool) != Some(true) {
        return Err(Error::new("the WebGL tier did not come up: the page fell back to the CPU rasterizer"));
    }
    let boxes = answer
        .get("ids")
        .map(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| Some((entry.str_at(&["id"])?.to_owned(), rect_of(entry)?)))
        .collect();
    let texts = answer.get("texts").map(Value::as_array).unwrap_or_default().iter().filter_map(rect_of).collect();
    let (width, height, rgba) = steady_screenshot(session)?;
    let notes = console_notes(session);
    Ok(Shot { width, height, rgba, boxes, texts, notes })
}

/// Waits until the page drew, the network is quiet, and two frames in a
/// row hold the same pixels.
fn settle(session: &mut Session, kind: Kind) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let said = session.evaluate(&ready_script(kind))?;
        if said.as_str() != Some("ready") {
            let notes = console_notes(session).join("\n");
            return Err(Error::new(format!("{}: {}\n{notes}", kind.letter(), said.as_str().unwrap_or("not ready"))));
        }
        session.pump()?;
        if session.in_flight() == 0 {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(Error::new(format!("{} requests never finished", session.in_flight())));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn screenshot(session: &mut Session) -> Result<(u32, u32, Vec<u8>)> {
    let shot = session.call("Page.captureScreenshot", cdp::object(&[("format", cdp::text("png"))]))?;
    let data = shot.str_at(&["data"]).ok_or_else(|| Error::new("the browser sent no picture"))?;
    let bytes = base64::decode(data).ok_or_else(|| Error::new("the picture is not base64"))?;
    png::decode(&bytes)
}

/// A screenshot that the next frame repeats byte for byte.
fn steady_screenshot(session: &mut Session) -> Result<(u32, u32, Vec<u8>)> {
    let mut last = screenshot(session)?;
    for _ in 0..20 {
        session.evaluate("new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(() => done(1))))")?;
        let next = screenshot(session)?;
        if next == last {
            return Ok(next);
        }
        last = next;
    }
    Err(Error::new("the page never held still: twenty frames in a row differed"))
}

/// What the page logged as a warning or an error, and what it threw.
fn console_notes(session: &mut Session) -> Vec<String> {
    let _ = session.pump();
    let mut notes = Vec::new();
    for event in session.take("Runtime.consoleAPICalled") {
        let level = event.str_at(&["params", "type"]).unwrap_or("log");
        if level != "error" && level != "warning" {
            continue;
        }
        let words: Vec<String> = event
            .path(&["params", "args"])
            .map(Value::as_array)
            .unwrap_or_default()
            .iter()
            .map(|arg| arg.str_at(&["value"]).map_or_else(|| arg.str_at(&["description"]).unwrap_or("?").to_owned(), str::to_owned))
            .collect();
        notes.push(format!("console.{level}: {}", words.join(" ")));
    }
    for event in session.take("Runtime.exceptionThrown") {
        let words = event
            .str_at(&["params", "exceptionDetails", "exception", "description"])
            .or_else(|| event.str_at(&["params", "exceptionDetails", "text"]))
            .unwrap_or("an exception");
        notes.push(format!("threw: {words}"));
    }
    notes
}
