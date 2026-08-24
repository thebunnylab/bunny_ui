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
//!   footer shows what the page posted on the bus — the user script
//!   posts the title as soon as the document loads, no click needed;
//! - "read the title" asks the page (`document.title` by eval) and
//!   the answer lands beside the button;
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
/// instrumentation is in place before the page renders.
const REPORTER: &str = "addEventListener('DOMContentLoaded', function() { \
    window.bunny.post('the page says: ' + document.title); });";

#[derive(Clone)]
struct Browser {
    page: State<usize>,
    shown: State<bool>,
    committed: State<String>,
    posted: State<String>,
    title: State<String>,
    handle: WebviewHandle,
}

impl Component for Browser {
    fn body(self, _ctx: &Context) -> impl View {
        let (page, shown) = (self.page.clone(), self.shown.clone());
        let (committed, posted, title) = (self.committed, self.posted, self.title);
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

        let sidebar = vstack!(
            vstack(links).spacing(6.0).alignment(HorizontalAlignment::Leading),
            spacer(),
            ask,
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
            hstack!(
                chip("back".into()).on_click(move || back.back()),
                chip("forward".into()).on_click(move || forward.forward()),
                text(committed.get()).foreground_color(theme::fg_secondary())
            )
            .spacing(8.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(8.0)
        };

        let pane = webview(PAGES[page.get()].1)
            .user_script(REPORTER)
            .on_navigate(move |url| committed.set(url.to_string()))
            .on_message(move |body| posted.set(body.to_string()))
            .handle(&handle);

        let footer = text(self.posted.get())
            .foreground_color(theme::fg_secondary())
            .padding_length(8.0);

        hstack!(
            sidebar,
            if shown.get() {
                Either::First(vstack!(bar, pane, footer))
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
            committed: State::new(String::new()),
            posted: State::new(String::from("nothing posted yet")),
            title: State::new(String::new()),
            handle: WebviewHandle::new(),
        },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
