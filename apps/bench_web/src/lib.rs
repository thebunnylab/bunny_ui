//! The web ruler: a 200-row stateful table on the element lowering.
//!
//! The same fixture the headless `bench_dom` example drives, in a
//! real browser: every row owns a toggle, one chip flips them all,
//! one chip filters the table down to ten rows and back. The driver
//! page (`web/driver.js`) dispatches real pointer events and times
//! the full input → state → patches → elements path.
#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use bunny_ui::prelude::*;

const ROWS: usize = 200;
const FILTERED: usize = 10;
const CLEAR: Color = Color::rgba(0, 0, 0, 0);

fn name_of(index: usize) -> String {
    format!("service_{index:03}.rs")
}

fn tools_of(index: usize) -> String {
    format!("tools {}", 5000 + index * 7)
}

fn value_of(index: usize) -> String {
    format!("${}.{}M", 90 + index % 20, index % 10)
}

/// One row, one component: the toggle read lives in THIS body, so a
/// flip dirties this row alone — the reuse promise covers the rest.
/// The same shape a signals framework uses for its O(change) story.
#[derive(Clone, Copy)]
struct Row {
    index: usize,
    on: State<bool>,
}

impl Component for Row {
    fn body(self, _ctx: &Context) -> impl View {
        let on = self.on.get();
        let toggle = self.on;
        hstack!(
            text(name_of(self.index)).foreground_color(theme::fg()),
            text(tools_of(self.index))
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            spacer(),
            text(value_of(self.index))
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            rectangle()
                .frame(12.0, 12.0)
                .background_color(if on { theme::accent() } else { theme::border() })
                .corner_radius(3.0),
        )
        .spacing(8.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Leading, 12.0)
        .padding_edge(Edge::Trailing, 12.0)
        .padding_edge(Edge::Top, 5.0)
        .padding_edge(Edge::Bottom, 5.0)
        .background_color(if on { theme::row_pressed() } else { CLEAR })
        .on_click(move || toggle.set(!toggle.get()))
    }
}

#[derive(Clone)]
struct Bench {
    filtered: State<bool>,
    toggles: Rc<Vec<State<bool>>>,
}

impl Component for Bench {
    fn body(self, _ctx: &Context) -> impl View {
        let count = if self.filtered.get() { FILTERED } else { ROWS };
        let toggles = self.toggles.clone();
        let items: Vec<usize> = (0..count).collect();

        let toggle_all = {
            let all = self.toggles.clone();
            text("toggle all")
                .foreground_color(theme::fg())
                .padding_length(8.0)
                .background_color(theme::control())
                .corner_radius(6.0)
                .on_click(move || {
                    for toggle in all.iter() {
                        toggle.set(!toggle.get());
                    }
                })
                .id("toggle_all")
        };
        let filter_chip = {
            let filtered = self.filtered;
            text("filter")
                .foreground_color(theme::fg())
                .padding_length(8.0)
                .background_color(theme::control())
                .corner_radius(6.0)
                .on_click(move || filtered.set(!filtered.get()))
                .id("filter")
        };

        let rows = list(
            items,
            |index| index.to_string(),
            move |index| Row { index: *index, on: toggles[*index] },
        );

        vstack!(
            hstack!(toggle_all, filter_chip, spacer(), text!("{count} rows")
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()))
            .spacing(8.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(10.0),
            rows,
        )
        .alignment(HorizontalAlignment::Leading)
        .frame(760.0, 640.0)
        .background_color(theme::panel())
    }
}

/// The bench page calls this once, with the box geometry.
#[unsafe(no_mangle)]
pub extern "C" fn start_dom(width: f64, height: f64, scale: f64) {
    let bench = Bench {
        filtered: State::new(false),
        toggles: Rc::new((0..ROWS).map(|_| State::new(false)).collect()),
    };
    bunny_ui_web::start_dom(width, height, scale, bench);
}
