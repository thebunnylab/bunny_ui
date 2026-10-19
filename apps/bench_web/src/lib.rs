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
    fn body(self) -> impl View {
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
    fn body(self) -> impl View {
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
    /// exact nesting the harness pierces. The body runs ONCE per key:
    /// the label and the selection flag are read by their nodes, so a
    /// write to either moves one element and runs no body at all.
    #[derive(Clone, Copy)]
    struct KeyedRow {
        seed: RowSeed,
        rows: State<Rc<Vec<RowSeed>>>,
        selected: State<Option<RowSeed>>,
    }

    impl Component for KeyedRow {
        fn body(self) -> impl View {
            let seed = self.seed;
            let id = seed.id;
            let rows = self.rows;
            let selected = self.selected;
            (
                // the row's OWN selection flag flips its own <tr>, read
                // by the element — no body hears about it
                boundary_class_when(seed.selected, "danger"),
                text(id.to_string())
                    .element("td")
                    .css_class("col-md-1"),
                hstack!(
                    text!(seed.label)
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
                        // the page's stylesheet draws the glyph through
                        // the class, in its own icon font: an empty
                        // element, not a text wearing a face of ours
                        hstack!(empty())
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
                // the last cell holds nothing at all: no text node to
                // fill on every row the list mounts
                hstack!(empty()).element("td").css_class("col-md-6"),
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

    /// The hybrid page's root: the same controls as real elements, the
    /// table as one canvas island our layout positions and paints.
    #[cfg(feature = "gpu")]
    #[derive(Clone)]
    pub struct HybridApp(pub App);

    #[cfg(feature = "gpu")]
    pub fn hybrid_app() -> HybridApp {
        HybridApp(app())
    }

    /// The pixel page's root: the whole page through the pixel
    /// pipeline, the table scrolling inside the pane.
    #[cfg(feature = "gpu")]
    #[derive(Clone)]
    pub struct PixelApp(pub App);

    #[cfg(feature = "gpu")]
    pub fn pixel_app() -> PixelApp {
        PixelApp(app())
    }

    /// The selected row's wash: the theme's accent, a quarter strong.
    #[cfg(feature = "gpu")]
    fn selection_tint() -> Color {
        let accent = bunny_ui::theme::current().accent;
        Color::rgba(accent.r, accent.g, accent.b, 64)
    }

    /// One row as pixels: the same three cells a table row has, in a
    /// row of fixed lanes — the element hints mean nothing to a canvas,
    /// so the row says its shape itself.
    #[cfg(feature = "gpu")]
    #[derive(Clone, Copy)]
    struct PixelRow {
        seed: RowSeed,
        rows: State<Rc<Vec<RowSeed>>>,
        selected: State<Option<RowSeed>>,
    }

    #[cfg(feature = "gpu")]
    impl Component for PixelRow {
        fn body(self) -> impl View {
            let seed = self.seed;
            let id = seed.id;
            let rows = self.rows;
            let selected = self.selected;
            // the selection is the row's own flag: this body reads it,
            // so a select re-runs the two rows that change and paints
            // them — one row's worth of pixels each
            let marked = seed.selected.get();
            hstack!(
                text(id.to_string()).foreground_color(theme::fg()).frame_width(72.0),
                // the cells carry names, so a runner finds a row's
                // label or its remove on the canvas by them
                text!(seed.label)
                    .foreground_color(theme::fg())
                    .on_click(move || {
                        if let Some(was) = selected.get() {
                            was.selected.set(false);
                        }
                        seed.selected.set(true);
                        selected.set(Some(seed));
                    })
                    .id("label")
                    .frame_width_aligned(300.0, Alignment::Leading),
                text("x")
                    .foreground_color(theme::fg_secondary())
                    .on_click(move || {
                        let mut kept = (*rows.get()).clone();
                        kept.retain(|seed| seed.id != id);
                        rows.set(Rc::new(kept));
                    })
                    .id("remove")
                    .frame_width(72.0),
            )
            .spacing(8.0)
            .padding_length(8.0)
            .frame_max(f64::INFINITY, 36.0, Alignment::Leading)
            .background_color(if marked { selection_tint() } else { CLEAR })
        }
    }

    #[cfg(feature = "gpu")]
    impl App {
        /// The table as pixels: the keyed list of pixel rows, scrolling
        /// inside the pane.
        fn pixel_table(self) -> impl View<Arity = bunny_ui::view::Single> {
            let rows = self.rows;
            let selected = self.selected;
            scroll(
                for_each(rows, |seed| seed.id.to_string(), move |seed| PixelRow { seed: *seed, rows, selected })
                    .once_per_key(),
            )
        }
    }

    #[cfg(feature = "gpu")]
    impl Component for PixelApp {
        fn body(self) -> impl View {
            vstack!(self.0.clone().controls(), self.0.pixel_table())
                .alignment(HorizontalAlignment::Leading)
                .frame(900.0, 800.0)
                .background_color(theme::panel())
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

    /// The page's own typography — the face and the size its stylesheet
    /// gives every other implementation by inheritance. A face named
    /// matches once in the browser and is cached; the system face, a
    /// generic the browser resolves per text run, costs the layout a
    /// millisecond per thousand rows.
    const PAGE_FACE: &str = "Helvetica Neue";
    const PAGE_SIZE: f64 = 14.0;

    impl Component for App {
        fn body(self) -> impl View {
            vstack!(self.clone().controls(), self.table())
                .alignment(HorizontalAlignment::Leading)
                .frame(900.0, 800.0)
                .background_color(theme::panel())
                .font_family(PAGE_FACE)
                .font_size(PAGE_SIZE)
                .element_id("main")
        }
    }

    #[cfg(feature = "gpu")]
    impl Component for HybridApp {
        fn body(self) -> impl View {
            vstack!(
                self.0.clone().controls(),
                // the island: the table's rows are pixels the engine
                // paints, positioned by the page's own flow. The rows
                // scroll inside the pane — the island is the pane's
                // size, and the rows outside it are never rasterized
                self.0.pixel_table().rendering(bunny_ui::layout::Rendering::Gpu),
            )
            .alignment(HorizontalAlignment::Leading)
            .frame(900.0, 800.0)
            .background_color(theme::panel())
            .font_family(PAGE_FACE)
            .font_size(PAGE_SIZE)
            .element_id("main")
        }
    }

    impl App {
        /// The six chips, as real buttons with the ids the harness clicks.
        fn controls(self) -> impl View<Arity = bunny_ui::view::Single> {
            let rows = self.rows;
            let selected = self.selected;
            let next_id = self.next_id;

            // the id is the element's for the page and the identity's
            // for the hit table: a page without elements finds the
            // chip by the same name — so the name wraps the click,
            // and the action's path carries it
            let chip = |label: &str, id: &str, action: fn(State<Rc<Vec<RowSeed>>>, State<Option<RowSeed>>, State<usize>)| {
                text(label.to_string())
                    .foreground_color(theme::fg())
                    .padding_length(8.0)
                    .background_color(theme::control())
                    .corner_radius(4.0)
                    .element("button")
                    .element_id(id)
                    .on_click(move || action(rows, selected, next_id))
                    .id(id)
            };

            hstack!(
                chip("Create 1,000 rows", "run", |rows, _, next_id| {
                    let from = next_id.get();
                    rows.set(Rc::new(build(from, 1_000)));
                    next_id.set(from + 1_000);
                }),
                chip("Create 10,000 rows", "runlots", |rows, _, next_id| {
                    let from = next_id.get();
                    rows.set(Rc::new(build(from, 10_000)));
                    next_id.set(from + 10_000);
                }),
                chip("Append 1,000 rows", "add", |rows, _, next_id| {
                    let from = next_id.get();
                    let mut grown = (*rows.get()).clone();
                    grown.extend(build(from, 1_000));
                    rows.set(Rc::new(grown));
                    next_id.set(from + 1_000);
                }),
                chip("Update every 10th row", "update", |rows, _, _| {
                    // one hundred signals flip; nine hundred rows
                    // never hear about it
                    for seed in rows.get().iter().step_by(10) {
                        let grown = format!("{} !!!", seed.label.get());
                        seed.label.set(Rc::from(grown.as_str()));
                    }
                }),
                chip("Clear", "clear", |rows, selected, _| {
                    rows.set(Rc::new(Vec::new()));
                    selected.set(None);
                }),
                chip("Swap Rows", "swaprows", |rows, _, _| {
                    let mut swapped = (*rows.get()).clone();
                    if swapped.len() > 998 {
                        swapped.swap(1, 998);
                    }
                    rows.set(Rc::new(swapped));
                }),
            )
            .spacing(6.0)
            .padding_length(8.0)
        }

        /// The table: the keyed list of rows. It takes the pane's
        /// width — a table left to its own width is measured whole
        /// once for the width and once more to lay out.
        fn table(self) -> impl View<Arity = bunny_ui::view::Single> {
            let rows = self.rows;
            let selected = self.selected;
            // the LIST reads the rows: a change to them runs the list's
            // key diff and nothing above it — a new key runs its row
            // once, a key that left takes its row along, a swap moves.
            // A row is its seed's, for as long as the seed's id stays:
            // the closure builds it once per key
            let table = for_each(
                rows,
                |seed| seed.id.to_string(),
                move |seed| KeyedRow { seed: *seed, rows, selected }.element("tr"),
            )
            .once_per_key();
            hstack!(table.element("tbody"))
                .element("table")
                .css_class("table table-hover table-striped test-data")
                .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Leading)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A row is the markup the harness pierces: four cells, the label
        /// as a link in the second, the glyph's link in the third, and a
        /// last cell with nothing in it — no element, no text node.
        #[test]
        fn a_row_is_the_markup_the_harness_reads() {
            let page = app();
            page.rows.set(Rc::new(build(1, 2)));
            let served = bunny_ui::ssr::render(&page, bunny_ui::layout::Size { width: 1200.0, height: 800.0 });
            let html = &served.html;
            let rows: Vec<&str> = html.split("<tr").skip(1).collect();
            assert_eq!(rows.len(), 2, "{html}");
            for (id, row) in (1..).zip(rows) {
                let row = &row[..row.find("</tr>").expect("a closed row")];
                assert_eq!(row.matches("<td").count(), 4, "four cells: {row}");
                let cells: Vec<&str> = row.split("<td").skip(1).collect();
                assert!(cells[0].contains(&format!(">{id}</td>")), "the id cell: {row}");
                assert!(cells[1].contains("<a") && cells[1].contains(&label_for(id)), "the label is a link: {row}");
                assert!(cells[2].contains("<a") && cells[2].contains("glyphicon glyphicon-remove"), "the glyph's link: {row}");
                let last = cells[3];
                let body = &last[last.find('>').expect("the cell opens") + 1..];
                assert!(body.starts_with("</td>"), "the last cell is empty: {row}");
            }
        }

        /// One frame, replayed on the page the way the glue applies it:
        /// every target on it must send the path the engine knows it by.
        fn frame(page: &App, runtime: &bunny_ui::runtime::Runtime, replay: &mut bunny_ui::ssr::Replay) {
            replay.apply(&runtime.dom_frame(page, SIZE));
            assert_eq!(replay.action_paths(), runtime.dom_action_paths());
        }

        /// A click on the target whose whole path ends so, by the path
        /// the page resolves for it.
        fn click(replay: &bunny_ui::ssr::Replay, runtime: &bunny_ui::runtime::Runtime, ends_with: &str) {
            let path = replay
                .action_paths()
                .into_values()
                .find(|path| path.ends_with(ends_with))
                .unwrap_or_else(|| panic!("no target ends with {ends_with}"));
            assert!(runtime.dom_action(&path, 1), "a live action: {path}");
        }

        const SIZE: bunny_ui::layout::Size = bunny_ui::layout::Size { width: 1200.0, height: 800.0 };

        /// The official session, clicked through the paths the page
        /// resolves — a row's label and remove cross told against the
        /// row, the chips against the app — and every target on the page
        /// sends, after every operation, the path it sent when paths
        /// crossed whole.
        #[test]
        fn every_click_sends_the_engines_path() {
            let page = app();
            let runtime = bunny_ui::runtime::Runtime::new();
            let mut replay = bunny_ui::ssr::Replay::new(SIZE);
            frame(&page, &runtime, &mut replay);

            click(&replay, &runtime, "[run]");
            frame(&page, &runtime, &mut replay);
            assert_eq!(page.rows.get().len(), 1_000);
            assert!(replay.html().contains("data-path=\"~/#2/#0\""), "a row's label is told against the row");

            // select row 2 by its label, remove row 5 by its glyph
            click(&replay, &runtime, "[2]/KeyedRow/#2/#0");
            frame(&page, &runtime, &mut replay);
            assert!(page.rows.get()[1].selected.get());
            for chip in ["[update]", "[swaprows]"] {
                click(&replay, &runtime, chip);
                frame(&page, &runtime, &mut replay);
            }
            click(&replay, &runtime, "[5]/KeyedRow/#3/#0");
            frame(&page, &runtime, &mut replay);
            assert!(page.rows.get().iter().all(|seed| seed.id != 5));
            assert_eq!(page.rows.get().len(), 999);
            for chip in ["[add]", "[run]", "[clear]", "[run]"] {
                click(&replay, &runtime, chip);
                frame(&page, &runtime, &mut replay);
            }
            assert_eq!(page.rows.get().len(), 1_000);
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

/// The hybrid page's boot: real elements around one canvas island.
#[cfg(all(target_arch = "wasm32", feature = "gpu"))]
#[unsafe(no_mangle)]
pub extern "C" fn start_keyed_hybrid(width: f64, height: f64, scale: f64, _hydrate: u32) {
    bunny_ui_web::start_dom(width, height, scale, keyed::hybrid_app());
}

/// The pixel page's boot: the whole keyed page through the pixel
/// pipeline — the WebGL tier where the page has it, the CPU surface
/// where it does not.
#[cfg(all(target_arch = "wasm32", feature = "gpu"))]
#[unsafe(no_mangle)]
pub extern "C" fn start_keyed_gpu(width: f64, height: f64, scale: f64) {
    bunny_ui_web::start(width, height, scale, keyed::pixel_app());
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
