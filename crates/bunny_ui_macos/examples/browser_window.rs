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

use bunny_ui::host::webview;
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

#[derive(Clone)]
struct Browser {
    page: State<usize>,
    shown: State<bool>,
    address: State<String>,
    popped: State<bool>,
    posted: State<String>,
    spoke: State<String>,
    fetched: State<String>,
    title: State<String>,
    handle: WebviewHandle,
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

        let sidebar = vstack!(
            vstack(links).spacing(6.0).alignment(HorizontalAlignment::Leading),
            spacer(),
            ask,
            shoot,
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

        let (spoke, fetched) = (self.spoke, self.fetched);
        let pane = webview(PAGES[page.get()].1)
            .user_script(REPORTER)
            .on_navigate(move |url| address.set(url.to_string()))
            .on_message(move |body| posted.set(body.to_string()))
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
            text(format!("network: {}", self.fetched.get()))
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
            title: State::new(String::new()),
            handle: WebviewHandle::new(),
        },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
