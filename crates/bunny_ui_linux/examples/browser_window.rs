//! The OS browser in a bunny box — on Linux.
//!
//! The webview is a native host (`docs/webview.md`): the framework
//! places the box, and WPE WebKit renders the page out of process —
//! its own renderer, its own scroll, zero bytes of bundled engine. On
//! this shell the page's pixels come BACK: every frame the engine
//! renders is painted into the scene where the host stood, so the
//! scene painted after it is above the page by paint order alone, on
//! the CPU raster, GL and Vulkan alike. The instrumentation floor
//! rides along: a user script at document start, the message bus,
//! navigation reports, eval with the value coming back, the snapshot,
//! and synthetic input as native events the page trusts.
//!
//! ```text
//! cargo run -p bunny-ui-linux --example browser_window_linux
//! cargo run -p bunny-ui-linux --example browser_window_linux -- --drive
//! cargo run -p bunny-ui-linux --example browser_window_linux -- --drive --editor
//! cargo run -p bunny-ui-linux --example browser_window_linux -- --page https://example.com/
//! ```
//!
//! The first page is one the example carries (a `data:` url), so a box
//! with no network — the container — has a page to drive. With
//! `--drive` the hand runs itself once that page commits and reads the
//! probe's reports back: every step of the vocabulary, the two hooks,
//! the two legs of navigation, the eval and the snapshot; exit 0 is
//! the proof. `--editor` mounts an editable document instead and types
//! into it: the change report is the proof.
//!
//! What to check by hand on a desktop: the page renders and scrolls
//! inside the pane; the toast in the corner is IN-SCENE content over
//! the page and still answers a click; a click on the clear space
//! beside it reaches the page; the popover rides its own panel over
//! the island; switching pages in the sidebar navigates the SAME view.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::cell::RefCell;

use bunny_ui::action::Modifiers;
use bunny_ui::host::{EditorCommand, MouseButton, NetworkPolicy, WebviewInput, webview, webview_html};
use bunny_ui::prelude::*;
#[cfg(target_os = "linux")]
use std::rc::Rc;

/// The page the example carries: a title the eval reads, a body the
/// probe draws over, and room below to scroll.
const HOME: &str = "data:text/html,<!doctype html><title>bunny home</title>\
<body style='font:16px sans-serif;margin:20px 20px 20px 300px'>\
<h1>bunny home</h1><p>a page the example carries: the probe draws on the left, \
and the shell paints this page into its own frame.</p>\
<div style='height:1200px;background:linear-gradient(%23eef,%23ffe)'></div></body>";

const PAGES: [(&str, &str); 3] = [
    ("home", HOME),
    ("example", "https://example.com/"),
    ("rust std", "https://doc.rust-lang.org/std/"),
];

/// `--page <url>`: the first page, in place of the one the example
/// carries — a way to point the engine at any page from the shell.
fn first_page() -> String {
    std::env::args()
        .skip_while(|arg| arg != "--page")
        .nth(1)
        .unwrap_or_else(|| HOME.to_string())
}

/// Posted on the bus by the page itself, at document load — the
/// instrumentation is in place before the page renders. It also
/// speaks on the console and fetches its own page, so the two hooks
/// have something to catch without a click.
const REPORTER: &str = "addEventListener('DOMContentLoaded', function() { \
    window.bunny.post('the page says: ' + document.title); \
    console.log('hello from the page'); \
    fetch(location.href); });";

/// The page's own witness, at document start: a box at a KNOWN place,
/// and a line on the bus for every event that reaches it — what it
/// was, where the page thinks it landed, and whether the page trusts
/// it. The target sits at 30,30 to 270,70 and the field at 30,82 to
/// 270,106, in the CSS pixels the handle's own doors take.
const PROBE: &str = "addEventListener('DOMContentLoaded', function() { \
    var box = document.createElement('div'); \
    box.style.cssText = 'position:fixed;box-sizing:border-box;left:20px;top:20px;\
width:260px;height:110px;border:2px solid #55f;background:#eef;z-index:2147483647;\
font:12px sans-serif;color:#224'; \
    var target = document.createElement('div'); \
    target.id = 'target'; \
    target.textContent = 'the probe: press me'; \
    target.style.cssText = 'position:absolute;left:8px;top:8px;width:240px;height:40px;\
background:#dde;text-align:center;line-height:40px'; \
    var field = document.createElement('input'); \
    field.id = 'field'; \
    field.style.cssText = 'position:absolute;left:8px;top:60px;width:240px;height:24px'; \
    box.appendChild(target); box.appendChild(field); document.body.appendChild(box); \
    function say(event) { \
        var who = event.target && event.target.id ? '#' + event.target.id \
            : (event.target && event.target.tagName || '?'); \
        var line = event.type + ' on ' + who; \
        if (event.clientX !== undefined) { \
            line += ' at ' + Math.round(event.clientX) + ',' + Math.round(event.clientY); } \
        if (event.type === 'wheel') { line += ' dy=' + Math.round(event.deltaY); } \
        if (event.type === 'keydown') { line += ' key=' + event.key; } \
        if (event.type === 'input') { line += ' value=' + event.target.value; } \
        window.bunny.post(line + ' trusted=' + event.isTrusted); } \
    ['mousemove', 'mousedown', 'mouseup', 'click', 'dblclick', 'contextmenu', \
     'wheel', 'keydown', 'input'].forEach(function(name) { \
        document.addEventListener(name, say, true); }); });";

/// Where the probe's two halves are, in the CSS pixels the handle
/// takes — the numbers the drive sequence aims at.
const TARGET: (f64, f64) = (150.0, 50.0);
const FIELD: (f64, f64) = (150.0, 94.0);

/// A host that does not resolve — the refusal the second hook was
/// written for.
const DEAD: &str = "https://a-host-that-does-not-resolve.invalid/";

/// The letter `--editor` opens, editable.
const DRAFT: &str = "<p>a draft, editable in place</p>";

thread_local! {
    /// Every line the page posted — the sheet reads the probe's
    /// reports back from here.
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// What the eval and the snapshot answered.
    static EVAL: RefCell<Option<Result<String, String>>> = const { RefCell::new(None) };
    static SHOT: RefCell<Option<Result<(usize, usize, usize), String>>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct Browser {
    page: State<usize>,
    shown: State<bool>,
    address: State<String>,
    popped: State<bool>,
    posted: State<String>,
    spoke: State<String>,
    fetched: State<String>,
    /// What a refused load answered — the other leg of the pair.
    refused: State<String>,
    title: State<String>,
    /// `--editor`: the body of the letter, as the editor reports it.
    body: State<String>,
    handle: WebviewHandle,
    /// `--drive`: the hand runs itself once the first page commits.
    drive: bool,
    /// `--editor`: the editable document instead of the pages.
    editor: bool,
    /// Once is once — every later navigation is a commit too.
    fired: State<bool>,
}

impl Component for Browser {
    fn body(self, _ctx: &Context) -> impl View {
        let (page, shown) = (self.page, self.shown);
        let (address, posted, title) = (self.address, self.posted, self.title);
        let handle = self.handle.clone();
        let me = self.clone();

        let chip = |label: String| {
            text(label)
                .padding_length(8.0)
                .background_color(theme::control())
                .background_hovered(theme::control_hovered())
                .corner_radius(6.0)
        };

        let links: Vec<_> = PAGES
            .iter()
            .enumerate()
            .map(|(index, (name, _))| {
                text(*name)
                    .padding_length(8.0)
                    .background_color(if page.get() == index {
                        theme::control_hovered()
                    } else {
                        theme::control()
                    })
                    .corner_radius(6.0)
                    .on_click(move || page.set(index))
            })
            .collect();

        let toggle = chip(if shown.get() { "hide the pane" } else { "show the pane" }.into())
            .on_click(move || shown.update(|shown| *shown = !*shown));

        let ask = {
            let handle = handle.clone();
            chip("read the title".into()).on_click(move || {
                handle.eval("document.title", move |answer| {
                    title.set(match answer {
                        Ok(value) => value,
                        Err(error) => format!("the page threw: {error}"),
                    });
                });
            })
        };

        let shoot = {
            let handle = handle.clone();
            chip("snapshot".into()).on_click(move || {
                handle.snapshot(move |answer| {
                    title.set(match answer {
                        Ok(shot) => format!("{} x {} px", shot.width, shot.height),
                        Err(error) => format!("refused: {error}"),
                    });
                });
            })
        };

        let drive = {
            let handle = handle.clone();
            chip("drive the page".into()).on_click(move || drive_the_page(&handle))
        };

        let dead = {
            let handle = handle.clone();
            chip("a dead url".into()).on_click(move || handle.navigate(DEAD))
        };

        let sidebar = vstack!(
            vstack(links).spacing(6.0).alignment(HorizontalAlignment::Leading),
            spacer(),
            ask,
            shoot,
            drive,
            dead,
            text(title.get()).foreground_color(theme::fg_secondary()),
            spacer().frame_height(12.0),
            toggle
        )
        .spacing(6.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(12.0)
        .frame_width(190.0)
        .background_color(theme::panel());

        let bar = {
            let back = handle.clone();
            let forward = handle.clone();
            let go = handle.clone();
            let popped = self.popped;
            hstack!(
                chip("back".into()).on_click(move || back.back()),
                chip("forward".into()).on_click(move || forward.forward()),
                // the address doubles as the report: typing edits it,
                // enter navigates, and a committed navigation writes
                // the real url back
                text_field("type a url and press enter", address.binding()).on_submit(
                    move || {
                        let typed = address.get();
                        let target = if typed.contains("://") {
                            typed
                        } else {
                            format!("https://{typed}")
                        };
                        go.navigate(target);
                    },
                ),
                // the island contract, demonstrated: a popover rides
                // its own panel, so it composites over the page below
                chip("popover".into()).on_click(move || popped.set(true)).popover(
                    popped.binding(),
                    Side::Bottom,
                    |_| {
                        erased(
                            vstack!(
                                text("a card over the page"),
                                text("this popover rides its own panel, so it may cross the island")
                                    .foreground_color(theme::fg_secondary()),
                                text("escape or click outside to close")
                                    .foreground_color(theme::fg_secondary())
                            )
                            .spacing(4.0)
                            .alignment(HorizontalAlignment::Leading)
                            .padding_length(12.0),
                        )
                    },
                )
            )
            .spacing(8.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(8.0)
        };

        let (spoke, fetched, refused, body) = (self.spoke, self.fetched, self.refused, self.body);
        let (driving, fired) = (self.drive, self.fired);
        // `--drive`: the hand needs a page under it, and a commit is
        // when there is one — once
        let arm = move || {
            if driving && !fired.get() {
                fired.set(true);
                let me = me.clone();
                task::spawn(async move { the_sheet(me).await }).detach();
            }
        };
        let pane = if self.editor {
            Either::First(
                webview_html(DRAFT, "", NetworkPolicy::Deny)
                    .editable()
                    .focus_on_appear()
                    .on_navigate(move |url| {
                        address.set(url.to_string());
                        arm();
                    })
                    .on_html_change(move |html| {
                        println!("[{}] changed: {html}", stamp());
                        body.set(html.to_string());
                    })
                    .handle(&handle),
            )
        } else {
            Either::Second(
                webview(if page.get() == 0 { first_page() } else { PAGES[page.get()].1.to_string() })
                    .user_script(REPORTER)
                    .user_script(PROBE)
                    .on_navigate(move |url| {
                        address.set(url.to_string());
                        arm();
                    })
                    // the footer says it, and stdout keeps the whole
                    // run: the probe reports faster than a person reads
                    .on_navigate_failed(move |url, why| {
                        println!("[{}] refused: {url} — {why}", stamp());
                        refused.set(format!("{url} — {why}"));
                    })
                    .on_message(move |line| {
                        println!("[{}] bus: {line}", stamp());
                        LOG.with(|log| log.borrow_mut().push(line.to_string()));
                        posted.set(line.to_string());
                    })
                    .on_console(move |line| {
                        println!("[{}] console: {line}", stamp());
                        spoke.set(line.to_string());
                    })
                    .on_request(move |line| {
                        println!("[{}] network: {line}", stamp());
                        fetched.set(line.to_string());
                    })
                    .handle(&handle),
            )
        };

        // paint order over the island, demonstrated: this is IN-SCENE
        // content over the page — no panel, no popover. Its pixels
        // claim the pointer; the clear space around it lets clicks
        // fall through to the page.
        let toast = {
            let title = self.title;
            vstack!(
                spacer(),
                hstack!(
                    spacer(),
                    text("an in-scene toast, over the page")
                        .padding_length(10.0)
                        .background_color(theme::panel())
                        .corner_radius(8.0)
                        .on_click(move || title.set("the toast took the click".into()))
                )
            )
            .padding_length(16.0)
        };

        let footer = vstack!(
            text(format!("bus: {}", self.posted.get())),
            text(format!("console: {}", self.spoke.get())),
            text(format!("network: {}", self.fetched.get())),
            text(format!("refused: {}", self.refused.get())),
            text(format!("body: {}", self.body.get()))
        )
        .spacing(2.0)
        .alignment(HorizontalAlignment::Leading)
        .foreground_color(theme::fg_secondary())
        .padding_length(8.0);

        hstack!(
            sidebar,
            if shown.get() {
                Either::First(vstack!(bar, zstack!(pane, toast), footer))
            } else {
                Either::Second(
                    vstack!(
                        spacer(),
                        text("the pane is gone, and the view went with it")
                            .foreground_color(theme::fg_secondary()),
                        spacer()
                    )
                    .alignment(HorizontalAlignment::Center),
                )
            }
        )
    }
}

/// The whole vocabulary, once, at the probe's own coordinates — the
/// button on the sidebar, for a person at the desk.
fn drive_the_page(handle: &WebviewHandle) {
    let handle = handle.clone();
    task::spawn(async move {
        the_hand(&handle).await;
        println!("[{}] the hand is done", stamp());
    })
    .detach();
}

/// The pointer arrives, presses, presses twice, the field takes the
/// keyboard and the page takes the wheel, then the other button. Each
/// step is its own beat so the page's reports read in order.
async fn the_hand(handle: &WebviewHandle) {
    let (x, y) = TARGET;
    handle.hover(x, y);
    beat().await;
    handle.click(x, y);
    beat().await;
    handle.input(WebviewInput::Click { x, y, clicks: 2, button: MouseButton::Left });
    beat().await;
    let (field_x, field_y) = FIELD;
    handle.click(field_x, field_y);
    beat().await;
    handle.type_text("a hand the app lends");
    beat().await;
    handle.key("Enter");
    beat().await;
    handle.scroll(400.0, 400.0, 0.0, 240.0);
    beat().await;
    handle.input(WebviewInput::Down {
        x,
        y,
        button: MouseButton::Right,
        clicks: 1,
        modifiers: Modifiers::NONE,
    });
    handle.input(WebviewInput::Up {
        x,
        y,
        button: MouseButton::Right,
        clicks: 1,
        modifiers: Modifiers::NONE,
    });
    beat().await;
}

/// `--drive`: the hand, then the reading. The page's own reports are
/// the witness; the verdict is the exit code.
#[cfg(target_os = "linux")]
async fn the_sheet(browser: Browser) {
    use bunny_ui_linux::drive;
    let mut passed = true;
    let mut check = |name: &str, held: bool| {
        println!("[{}] {} — {name}", stamp(), if held { "ok" } else { "FAILED" });
        passed &= held;
    };
    // the page has to be under the hand: a commit is when there is
    // one, plus a beat for the probe's own script to draw itself
    task::sleep(std::time::Duration::from_millis(1200)).await;
    let before = drive::presents();
    let handle = browser.handle.clone();

    if browser.editor {
        check("the letter committed", !browser.address.get().is_empty());
        handle.type_text("Dear reader, ");
        beat().await;
        beat().await;
        check(
            "typing without a click lands: the keyboard was taken on appearing",
            browser.body.get().contains("Dear reader,"),
        );
        handle.exec(EditorCommand::SelectAll);
        handle.exec(EditorCommand::Bold);
        beat().await;
        beat().await;
        check("bold from the allowlist wraps the selection", browser.body.get().contains("<b>"));
        check("the letter's frames reached the glass", drive::presents() > before);
        verdict(passed, "editor");
        return;
    }

    check("the home page committed", browser.address.get().starts_with("data:"));
    the_hand(&handle).await;
    handle.eval("document.title", |answer| EVAL.with(|slot| *slot.borrow_mut() = Some(answer)));
    handle.snapshot(|answer| {
        SHOT.with(|slot| {
            *slot.borrow_mut() = Some(answer.map(|shot| {
                let painted = shot.rgba.chunks_exact(4).filter(|px| px[3] != 0).count();
                (shot.width, shot.height, painted)
            }));
        });
    });
    // the OTHER leg: a load that answers by refusal
    handle.navigate(DEAD);
    task::sleep(std::time::Duration::from_millis(2500)).await;

    let log = LOG.with(|log| log.borrow().clone());
    let has = |needle: &str| log.iter().any(|line| line.contains(needle));
    check("the page posted on the bus at load", has("the page says: bunny home"));
    check("the console hook heard the page", browser.spoke.get().contains("hello from the page"));
    check("the network wrap saw the page's fetch", browser.fetched.get().starts_with("GET "));
    check("a hover reaches the probe, trusted", has("mousemove on #target at 150,50 trusted=true"));
    check("a click lands on the probe, trusted", has("click on #target at 150,50 trusted=true"));
    check("a double click counts as one", has("dblclick on #target"));
    check(
        "typed text lands in the field as a commit",
        has("input on #field value=a hand the app lends"),
    );
    check("a named key reaches the field", has("keydown on #field key=Enter trusted=true"));
    let wheel = log.iter().find(|line| line.starts_with("wheel on"));
    if let Some(line) = wheel {
        println!("[{}] the wheel said: {line}", stamp());
    }
    let down = wheel.is_some_and(|line| {
        line.split(" dy=")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|dy| dy.parse::<i64>().ok())
            .is_some_and(|dy| dy > 0)
    });
    check("the wheel scrolls DOWN when asked to", down);
    check("the other button reaches the probe", has("contextmenu on #target"));
    check("a dead url refuses by name", browser.refused.get().contains("does-not-resolve"));
    let eval = EVAL.with(|slot| slot.borrow().clone());
    println!("[{}] eval: {eval:?}", stamp());
    check("eval answers the title", eval.is_some_and(|answer| answer.is_ok_and(|v| v.contains("bunny home"))));
    let shot = SHOT.with(|slot| slot.borrow().clone());
    println!("[{}] snapshot: {shot:?}", stamp());
    check(
        "the snapshot is the page's own pixels",
        shot.is_some_and(|shot| shot.is_ok_and(|(w, h, painted)| w > 0 && h > 0 && painted > 0)),
    );
    check("the page's frames reached the glass", drive::presents() > before);
    verdict(passed, "browser");
}

#[cfg(target_os = "linux")]
fn verdict(passed: bool, what: &str) {
    println!(
        "[{}] {} — {what}, backend={} presents={}",
        stamp(),
        if passed { "the sheet holds" } else { "the sheet has a hole" },
        bunny_ui_linux::drive::backend(),
        bunny_ui_linux::drive::presents()
    );
    std::process::exit(if passed { 0 } else { 1 });
}

#[cfg(not(target_os = "linux"))]
async fn the_sheet(_browser: Browser) {}

/// Milliseconds since the process started — the ruler for how long a
/// report takes to come back.
fn stamp() -> u128 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis()
}

/// One beat between steps — long enough for the engine to answer, and
/// for the reports to arrive in the order they were asked for.
async fn beat() {
    task::sleep(std::time::Duration::from_millis(250)).await;
}

#[cfg(target_os = "linux")]
fn main() {
    let drive = std::env::args().any(|arg| arg == "--drive");
    let editor = std::env::args().any(|arg| arg == "--editor");
    if drive {
        // the engine's processes take their time to come up on a
        // software GL; the sheet itself takes about five seconds
        bunny_ui_linux::drive::watchdog(60);
    }
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_linux::FreeTypeEngine::new()))
        .image_engine(Rc::new(bunny_ui_linux::LinuxImageEngine::new()));
    bunny_ui_linux::run_window_with(
        "browser",
        Size { width: 960.0, height: 640.0 },
        runtime,
        Browser {
            page: State::new(0),
            shown: State::new(true),
            address: State::new(String::new()),
            popped: State::new(false),
            posted: State::new(String::from("nothing yet")),
            spoke: State::new(String::from("nothing yet")),
            fetched: State::new(String::from("nothing yet")),
            refused: State::new(String::from("nothing yet")),
            title: State::new(String::new()),
            body: State::new(String::new()),
            handle: WebviewHandle::new(),
            drive,
            editor,
            fired: State::new(false),
        },
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
