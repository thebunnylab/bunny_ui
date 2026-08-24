//! The OS browser in a bunny box.
//!
//! The webview is a native host (`docs/webview.md`): the framework
//! places the box, and WKWebView composites above the scene inside
//! it — its own renderer, its own scroll, its own input, zero bytes
//! of bundled engine.
//!
//! What to check by hand:
//! - the page renders and scrolls natively inside the pane, and the
//!   framework's own chrome paints around it, never over it;
//! - resize the window: the pane follows the layout, the page reflows;
//! - switch pages in the sidebar: the SAME view navigates (state such
//!   as scroll position is the engine's to keep or drop);
//! - hide the pane: the subtree goes and the view goes with it; show
//!   it again and a fresh one mounts;
//! - the footer reports the rectangle the host reported to the app.

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

#[derive(Clone, Copy)]
struct Browser {
    page: State<usize>,
    shown: State<bool>,
    reported: State<(f64, f64)>,
}

impl Component for Browser {
    fn body(self, _ctx: &Context) -> impl View {
        let (page, shown, reported) = (self.page, self.shown, self.reported);

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

        let toggle = text(if shown.get() { "hide the pane" } else { "show the pane" })
            .padding_length(8.0)
            .background_color(theme::control())
            .background_hovered(theme::control_hovered())
            .corner_radius(6.0)
            .on_click(move || shown.update(|shown| *shown = !*shown));

        let sidebar = vstack!(
            vstack(links).spacing(6.0).alignment(HorizontalAlignment::Leading),
            spacer(),
            toggle
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_length(12.0)
        .frame_width(160.0)
        .background_color(theme::panel());

        let (width, height) = reported.get();
        let footer = text(format!("the host reports {width:.0} x {height:.0}"))
            .foreground_color(theme::fg_secondary())
            .padding_length(8.0);

        let pane = webview(PAGES[page.get()].1)
            .on_measure(move |size| reported.set((size.width, size.height)));

        hstack!(
            sidebar,
            if shown.get() {
                Either::First(vstack!(pane, footer))
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
        Size { width: 900.0, height: 620.0 },
        runtime,
        Browser {
            page: State::new(0),
            shown: State::new(true),
            reported: State::new((0.0, 0.0)),
        },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
