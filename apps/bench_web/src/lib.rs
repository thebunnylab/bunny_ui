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

    /// One row: the component wears the `<tr>` (its identity group IS
    /// the element), and its body is the cells — DIRECT children, the
    /// exact nesting the harness pierces.
    #[derive(Clone, Copy)]
    struct KeyedRow {
        seed: RowSeed,
        rows: State<Rc<Vec<RowSeed>>>,
        selected: State<Option<RowSeed>>,
    }

    impl Component for KeyedRow {
        fn body(self, _ctx: &Context) -> impl View {
            let seed = self.seed;
            let id = seed.id;
            let rows = self.rows;
            let selected = self.selected;
            (
                // the row's OWN selection flag flips its own <tr>
                boundary_class(if seed.selected.get() { "danger" } else { "" }),
                text(id.to_string())
                    .foreground_color(theme::fg())
                    .element("td")
                    .css_class("col-md-1"),
                hstack!(
                    text(seed.label.get().to_string())
                        .foreground_color(theme::fg())
                        .element("a")
                        .on_click(move || {
                            // the two rows that change are the only
                            // ones that hear about it
                            if let Some(was) = selected.get() {
                                was.selected.set(false);
                            }
                            seed.selected.set(true);
                            selected.set(Some(seed));
                        })
                )
                .element("td")
                .css_class("col-md-4"),
                hstack!(
                    hstack!(
                        text("x")
                            .foreground_color(theme::fg_secondary())
                            .element("span")
                            .css_class("glyphicon glyphicon-remove")
                    )
                    .element("a")
                    .on_click(move || {
                        let mut kept = (*rows.get()).clone();
                        kept.retain(|seed| seed.id != id);
                        rows.set(Rc::new(kept));
                    })
                )
                .element("td")
                .css_class("col-md-1"),
                hstack!(text(String::new())).element("td").css_class("col-md-6"),
            )
        }
    }

    /// One row's signals: the label and the selection flag are the
    /// row's OWN state, so updating a label or moving the selection
    /// dirties that row alone — the reuse promise covers the rest.
    /// (Handler-created states live at app scope for now: the state
    /// lifecycle for collections is an open design note.)
    #[derive(Clone, Copy)]
    pub struct RowSeed {
        pub id: usize,
        pub label: State<Rc<str>>,
        pub selected: State<bool>,
    }

    #[derive(Clone)]
    pub struct App {
        pub rows: State<Rc<Vec<RowSeed>>>,
        pub selected: State<Option<RowSeed>>,
        pub next_id: State<usize>,
    }

    pub fn app() -> App {
        App {
            rows: State::new(Rc::new(Vec::new())),
            selected: State::new(None),
            next_id: State::new(1),
        }
    }

    fn build(from: usize, count: usize) -> Vec<RowSeed> {
        (0..count)
            .map(|i| RowSeed {
                id: from + i,
                label: State::new(Rc::from(label_for(from + i).as_str())),
                selected: State::new(false),
            })
            .collect()
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
                    // one hundred signals flip; nine hundred rows
                    // never hear about it
                    for seed in rows.get().iter().step_by(10) {
                        let grown = format!("{} !!!", seed.label.get());
                        seed.label.set(Rc::from(grown.as_str()));
                    }
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

            let table = for_each(
                (*data).clone(),
                |seed| seed.id.to_string(),
                move |seed| KeyedRow { seed: *seed, rows, selected }.element("tr"),
            );

            vstack!(
                controls,
                hstack!(table.element("tbody"))
                    .element("table")
                    .css_class("table table-hover table-striped test-data"),
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
