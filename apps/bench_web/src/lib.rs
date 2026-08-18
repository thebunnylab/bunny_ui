//! The web ruler: a 200-row stateful table on the element lowering.
//!
//! The same fixture the headless `bench_dom` example drives, in a
//! real browser: every row owns a toggle, one chip flips them all,
//! one chip filters the table down to ten rows and back. The driver
//! page (`web/driver.js`) dispatches real pointer events and times
//! the full input → state → patches → elements path.
// The VIEW compiles on every target: the build renders the page
// natively (see examples/render.rs) and the wasm hydrates on top —
// only the FFI exports below stay web-gated.

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
pub struct Bench {
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

// MARK: - The keyed benchmark (the official shape)

/// The exact application the js-framework-benchmark drives: a keyed
/// table, six operations behind ids the harness clicks, rows the
/// harness inspects as real `<tr>`s. The hints make the markup true;
/// the identity of each row IS its key.
pub mod keyed {
    use super::*;

    fn adjectives() -> &'static [&'static str] {
        &[
            "pretty", "large", "big", "small", "tall", "short", "long", "handsome",
            "plain", "quaint", "clean", "elegant", "easy", "angry", "crazy", "helpful",
            "mushy", "odd", "unsightly", "adorable", "important", "inexpensive",
            "cheap", "expensive", "fancy",
        ]
    }

    fn colours() -> &'static [&'static str] {
        &[
            "red", "yellow", "blue", "green", "pink", "brown", "purple", "brown",
            "white", "black", "orange",
        ]
    }

    fn nouns() -> &'static [&'static str] {
        &[
            "table", "chair", "house", "bbq", "desk", "car", "pony", "cookie",
            "sandwich", "burger", "pizza", "mouse", "keyboard",
        ]
    }

    /// The reference's own deterministic-enough label mix. No random
    /// source exists in the engine; a linear congruence stands in and
    /// keeps every run comparable.
    fn label_for(seed: usize) -> String {
        let a = adjectives();
        let c = colours();
        let n = nouns();
        let mix = seed.wrapping_mul(2654435761);
        format!(
            "{} {} {}",
            a[mix % a.len()],
            c[(mix / 31) % c.len()],
            n[(mix / 997) % n.len()]
        )
    }

    #[derive(Clone)]
    pub struct App {
        pub rows: State<Rc<Vec<(usize, String)>>>,
        pub selected: State<Option<usize>>,
        pub next_id: State<usize>,
    }

    pub fn app() -> App {
        App {
            rows: State::new(Rc::new(Vec::new())),
            selected: State::new(None),
            next_id: State::new(1),
        }
    }

    fn build(from: usize, count: usize) -> Vec<(usize, String)> {
        (0..count).map(|i| (from + i, label_for(from + i))).collect()
    }

    impl Component for App {
        fn body(self, _ctx: &Context) -> impl View {
            let rows = self.rows;
            let selected = self.selected;
            let next_id = self.next_id;
            let data = rows.get();

            let chip = |label: &str, id: &str| {
                text(label.to_string())
                    .foreground_color(theme::fg())
                    .padding_length(8.0)
                    .background_color(theme::control())
                    .corner_radius(4.0)
                    .element("button")
                    .element_id(id)
            };

            let controls = hstack!(
                chip("Create 1,000 rows", "run").on_click(move || {
                    let from = next_id.get();
                    rows.set(Rc::new(build(from, 1_000)));
                    next_id.set(from + 1_000);
                }),
                chip("Create 10,000 rows", "runlots").on_click(move || {
                    let from = next_id.get();
                    rows.set(Rc::new(build(from, 10_000)));
                    next_id.set(from + 10_000);
                }),
                chip("Append 1,000 rows", "add").on_click(move || {
                    let from = next_id.get();
                    let mut grown = (*rows.get()).clone();
                    grown.extend(build(from, 1_000));
                    rows.set(Rc::new(grown));
                    next_id.set(from + 1_000);
                }),
                chip("Update every 10th row", "update").on_click(move || {
                    let mut touched = (*rows.get()).clone();
                    for entry in touched.iter_mut().step_by(10) {
                        entry.1.push_str(" !!!");
                    }
                    rows.set(Rc::new(touched));
                }),
                chip("Clear", "clear").on_click(move || {
                    rows.set(Rc::new(Vec::new()));
                    selected.set(None);
                }),
                chip("Swap Rows", "swaprows").on_click(move || {
                    let mut swapped = (*rows.get()).clone();
                    if swapped.len() > 998 {
                        swapped.swap(1, 998);
                    }
                    rows.set(Rc::new(swapped));
                }),
            )
            .spacing(6.0)
            .padding_length(8.0);

            let table = list(
                (*data).clone(),
                |(id, _)| id.to_string(),
                move |(id, label)| {
                    let id = *id;
                    let is_selected = selected.get() == Some(id);
                    hstack!(
                        text(id.to_string())
                            .foreground_color(theme::fg())
                            .element("td")
                            .css_class("col-md-1"),
                        text(label.clone())
                            .foreground_color(theme::fg())
                            .element("a")
                            .on_click(move || selected.set(Some(id)))
                            .element("td")
                            .css_class("col-md-4"),
                        text("x")
                            .foreground_color(theme::fg_secondary())
                            .element("a")
                            .on_click(move || {
                                let mut kept = (*rows.get()).clone();
                                kept.retain(|(kept_id, _)| *kept_id != id);
                                rows.set(Rc::new(kept));
                            })
                            .element("td")
                            .css_class("col-md-1"),
                        spacer(),
                    )
                    .spacing(8.0)
                    .background_color(if is_selected {
                        theme::selection()
                    } else {
                        CLEAR
                    })
                    .element("tr")
                    .css_class(if is_selected { "danger" } else { "" })
                },
            );

            vstack!(
                controls,
                table.element("tbody").css_class("test-data"),
            )
            .alignment(HorizontalAlignment::Leading)
            .frame(900.0, 800.0)
            .background_color(theme::panel())
            .element_id("main")
        }
    }
}

/// The keyed page's boot.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn start_keyed(width: f64, height: f64, scale: f64, hydrate: u32) {
    if hydrate != 0 {
        bunny_ui_web::start_dom_hydrated(width, height, scale, keyed::app());
    } else {
        bunny_ui_web::start_dom(width, height, scale, keyed::app());
    }
}

/// The scene, shared by the wasm boot and the native page builder.
pub fn bench() -> Bench {
    Bench {
        filtered: State::new(false),
        toggles: Rc::new((0..ROWS).map(|_| State::new(false)).collect()),
    }
}

/// The bench page calls this once, with the box geometry. `hydrate`
/// says the page shipped painted: adopt instead of mounting.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn start_dom(width: f64, height: f64, scale: f64, hydrate: u32) {
    if hydrate != 0 {
        bunny_ui_web::start_dom_hydrated(width, height, scale, bench());
    } else {
        bunny_ui_web::start_dom(width, height, scale, bench());
    }
}
