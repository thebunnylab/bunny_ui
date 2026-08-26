//! The OS browser in a bunny box.
//!
//! The webview is a native host (`docs/webview.md`): the framework
//! places the box, and WKWebView composites above the scene inside
//! it — its own renderer, its own scroll, its own input, zero bytes
//! of bundled engine. The instrumentation floor rides along: a user
//! script at document start, the message bus, navigation reports and
//! eval with the value coming back.
//!
//! What to check by hand:
//! - the page renders and scrolls natively inside the pane, and the
//!   framework's own chrome paints around it, never over it;
//! - the bar shows the COMMITTED url (the delegate reporting), and the
//!   footer shows what the page posted on the bus, said on its console
//!   and fetched — the user script does all three as soon as the
//!   document loads, no click needed;
//! - "read the title" asks the page (`document.title` by eval) and
//!   the answer lands beside the button; "snapshot" asks for the
//!   pixels and reports the size it got back;
//! - type a url in the bar and press enter (a bare host name gets
//!   https:// for free); a committed navigation writes the real url
//!   back into the field;
//! - "popover" opens a card over the page — presented on its own
//!   child panel, the overlay road;
//! - the PROBE is the page's own witness: the user script draws it at
//!   a known place and reports every event it feels, with the
//!   coordinates and whether the page trusts it. Click its target
//!   with the real mouse, then press "drive the page" and read the
//!   two lines: the app's hand says `trusted=true` like the hand on
//!   the desk, which is what a synthetic DOM event cannot say;
//! - "a dead url" points the engine at a host that does not resolve:
//!   the refusal lands in the footer by name instead of hanging for
//!   ever waiting for a commit that is not coming;
//! - the toast in the corner is IN-SCENE content over the page — the
//!   sandwich: paint order is the truth over the island too. Click it
//!   and it answers; click the clear space beside it and the PAGE
//!   answers;
//! - switch pages in the sidebar: the SAME view navigates; back and
//!   forward walk the engine's own history;
//! - hide the pane: the subtree goes and the view goes with it; show
//!   it again and a fresh one mounts;
//! - resize the window: the pane follows the layout, the page reflows.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::action::Modifiers;
use bunny_ui::host::{MouseButton, WebviewInput, webview};
use bunny_ui::prelude::*;
#[cfg(target_os = "macos")]
use bunny_ui_macos::CoreTextEngine;
#[cfg(target_os = "macos")]
use std::rc::Rc;

const PAGES: [(&str, &str); 3] = [
    ("example", "https://example.com/"),
    ("rust std", "https://doc.rust-lang.org/std/"),
    ("wikipedia", "https://en.wikipedia.org/"),
];

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
/// 270,106, in the CSS pixels the handle's own doors take, so a
/// report can be read against the coordinates that were asked for.
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
    handle: WebviewHandle,
    /// `--drive`: the hand runs itself once the first page commits,
    /// so the whole vocabulary reports on stdout with no click.
    drive: bool,
    /// Once is once — every later navigation is a commit too.
    fired: State<bool>,
    /// `--workbench`: the rows the heavy rail carries — the dial that turns
    /// this fluid example into the workbench's resize. Zero is the example
    /// as it always was.
    rows: usize,
    /// `--blink`: the workbench's OTHER half — a caret-style clock that
    /// keeps tick frames coming through a resize drag. The costume without
    /// it stays fluid; the heartbeat is what races the resize presenter.
    blink: bool,
    caret: State<bool>,
}

impl Component for Browser {
    fn body(self, _ctx: &Context) -> impl View {
        let (page, shown) = (self.page, self.shown);
        let (address, posted, title) = (self.address, self.posted, self.title);
        let handle = self.handle;

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
                let page = page.clone();
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

        let toggle = {
            let shown = shown.clone();
            chip(if shown.get() { "hide the pane" } else { "show the pane" }.into())
                .on_click(move || shown.update(|shown| *shown = !*shown))
        };

        let ask = {
            let handle = handle.clone();
            let title = title.clone();
            chip("read the title".into()).on_click(move || {
                let title = title.clone();
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
            let title = title;
            chip("snapshot".into()).on_click(move || {
                let title = title;
                handle.snapshot(move |answer| {
                    title.set(match answer {
                        Ok(shot) => format!("{} x {} px", shot.width, shot.height),
                        Err(error) => format!("refused: {error}"),
                    });
                });
            })
        };

        // the hand, spelled out: the two short doors, the held right
        // press the vocabulary keeps for a hand, and the text that
        // lands as a commit
        let drive = {
            let handle = handle.clone();
            chip("drive the page".into()).on_click(move || drive_the_page(&handle))
        };

        let dead = {
            let handle = handle.clone();
            chip("a dead url".into()).on_click(move || handle.navigate(DEAD))
        };

        let beat = self.blink.then(|| {
            let caret = self.caret;
            hstack!(
                text("beat").foreground_color(theme::fg_secondary()),
                spacer().frame(8.0, 14.0).background_color(if caret.get() {
                    theme::control_hovered()
                } else {
                    theme::panel()
                })
            )
            .spacing(6.0)
            .alignment(VerticalAlignment::Center)
            .task(move || async move {
                loop {
                    task::sleep(std::time::Duration::from_millis(450)).await;
                    caret.update(|on| *on = !*on);
                }
            })
        });

        let sidebar = vstack!(
            vstack(links).spacing(6.0).alignment(HorizontalAlignment::Leading),
            spacer(),
            beat,
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
                // enter navigates, and a committed navigation (a link
                // click included) writes the real url back
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
                // the island contract, demonstrated: on this OS a
                // popover rides its own child panel, so it composites
                // over the page below it
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

        let (spoke, fetched, refused) = (self.spoke, self.fetched, self.refused);
        let (driving, fired) = (self.drive, self.fired);
        let armed = handle.clone();
        let pane = webview(PAGES[page.get()].1)
            .user_script(REPORTER)
            .user_script(PROBE)
            .on_navigate(move |url| {
                address.set(url.to_string());
                // `--drive`: the hand needs a page under it, and a
                // commit is when there is one
                if driving && !fired.get() {
                    fired.set(true);
                    let armed = armed.clone();
                    task::spawn(async move {
                        // the page has to be under the hand: a commit
                        // is when there is one, plus a beat for the
                        // probe's own script to draw itself
                        task::sleep(std::time::Duration::from_millis(1200)).await;
                        drive_the_page(&armed);
                    })
                    .detach();
                }
            })
            // the footer says it, and stdout keeps the whole run: the
            // probe reports faster than a person reads
            .on_navigate_failed(move |url, why| {
                println!("[{}] refused: {url} — {why}", stamp());
                refused.set(format!("{url} — {why}"));
            })
            .on_message(move |body| {
                println!("[{}] bus: {body}", stamp());
                posted.set(body.to_string());
            })
            .on_console(move |line| spoke.set(line.to_string()))
            .on_request(move |line| fetched.set(line.to_string()))
            .handle(&handle);

        // the sandwich, demonstrated: this is IN-SCENE content over
        // the page — no panel, no popover, just paint order. Its
        // pixels claim the pointer; the clear space around it lets
        // clicks fall through to the page.
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
            text(format!("refused: {}", self.refused.get()))
        )
        .spacing(2.0)
        .alignment(HorizontalAlignment::Leading)
        .foreground_color(theme::fg_secondary())
        .padding_length(8.0);

        let center = hstack!(
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
        );

        // `--workbench`: the same island, wearing the workbench's anatomy —
        // fixed flanks left and right, and a rail whose layout costs real
        // milliseconds per resize step. The island's margins stay fixed, so
        // the autoresize spring is as exact here as in the bare example; what
        // changes is only how long OUR side of a resize step takes. If this
        // costume trembles, the trembling is the present pipeline meeting a
        // heavy scene, not the app that happens to own the scene.
        if self.rows == 0 {
            return Either::First(center);
        }
        let rows: Vec<_> = (0..self.rows)
            .map(|i| {
                text(format!("row {i} — the rail the workbench carries"))
                    .font_size(11.0)
                    .foreground_color(theme::fg_secondary())
            })
            .collect();
        let rail = vstack(rows)
        .spacing(2.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(8.0)
        .frame_width(300.0)
        .background_color(theme::panel())
        .clipped();
        let flank = vstack!(
            text("the far dock").foreground_color(theme::fg_secondary()),
            spacer()
        )
        .padding_length(8.0)
        .frame_width(220.0)
        .background_color(theme::panel());
        Either::Second(hstack!(rail, center, flank))
    }
}

/// The whole vocabulary, once, at the probe's own coordinates: the
/// pointer arrives, presses, presses twice, presses the other button,
/// then the field takes the keyboard and the page takes the wheel.
/// Each step is its own beat so the page's reports read in order.
fn drive_the_page(handle: &WebviewHandle) {
    let handle = handle.clone();
    // detached on purpose: the hand outlives the click that asked for
    // it, and a dropped handle would cancel it mid-sequence
    task::spawn(async move {
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
        // the OTHER pair: a load that answers by refusal. The page on
        // screen does not move, so the probe is still there for the
        // last step
        handle.navigate(DEAD);
        beat().await;
        beat().await;
        // the held half of the vocabulary LAST: the right press leaves
        // the page's own menu open, and a menu takes the machine until
        // a person closes it
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
        println!("[{}] the hand is done — the page's menu is open, escape closes it", stamp());
    })
    .detach();
}

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

#[cfg(target_os = "macos")]
fn main() {
    let runtime = Runtime::new().text_engine(Rc::new(CoreTextEngine::new()));
    bunny_ui_macos::run_window_with(
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
            handle: WebviewHandle::new(),
            drive: std::env::args().any(|arg| arg == "--drive"),
            fired: State::new(false),
            rows: if std::env::args().any(|arg| arg == "--workbench") {
                std::env::args()
                    .skip_while(|arg| arg != "--rows")
                    .nth(1)
                    .and_then(|arg| arg.parse().ok())
                    .unwrap_or(1500)
            } else {
                0
            },
            blink: std::env::args().any(|arg| arg == "--blink"),
            caret: State::new(false),
        },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
