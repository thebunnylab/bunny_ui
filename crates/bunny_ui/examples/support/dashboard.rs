//! The dashboard scene: the shape of a product screen, for the frame
//! benches.
//!
//! A tab strip, a sidebar, a status bar, and one of two large pages. The
//! board is a plain `scroll` of chart panels. Each panel has a header, a
//! box the app paints (bars, a line, a ring) and a legend that scrolls.
//! The table is a plain list of 400 rows. A few thousand nodes: enough
//! for a cost that grows with the tree to show.
//!
//! This file is not an example of its own. A bench includes it with
//! `#[path = "support/dashboard.rs"] mod dashboard;`.

use bunny_ui::prelude::*;

pub const PANELS: usize = 8;
pub const TABLE_ROWS: usize = 400;
pub const SIDEBAR_ROWS: usize = 40;
pub const VIEWPORT: Size = Size { width: 1280.0, height: 800.0 };

const SERIES: [Color; 6] = [
    Color::rgba(86, 156, 214, 255),
    Color::rgba(220, 140, 200, 255),
    Color::rgba(120, 200, 160, 255),
    Color::rgba(230, 170, 110, 255),
    Color::rgba(150, 130, 230, 255),
    Color::rgba(220, 110, 120, 255),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Board,
    Table,
}

/// A number in `0.0..1.0` that depends only on its two keys — the
/// fixture's data is the same on every machine and every run.
fn sample(panel: usize, index: usize) -> f64 {
    let mut state = (panel as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (index as u64 + 1).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    state ^= state >> 30;
    state = state.wrapping_mul(0x94D0_49BB_1331_11EB);
    state ^= state >> 31;
    (state % 10_000) as f64 / 10_000.0
}

// MARK: - The root

#[derive(Clone)]
pub struct Workbench {
    pub mode: State<Mode>,
    /// Legend rows in each panel.
    pub legend_rows: usize,
    /// One deep state for each panel: a toggle dirties ONE small body.
    pub flags: Rc<Vec<State<bool>>>,
    /// The status dot's colour index. A change starts a colour flight.
    pub pulse: State<usize>,
}

impl Workbench {
    pub fn new(legend_rows: usize) -> Workbench {
        Workbench {
            mode: State::new(Mode::Board),
            legend_rows,
            flags: Rc::new((0..PANELS).map(|_| State::new(false)).collect()),
            pulse: State::new(0),
        }
    }
}

impl Component for Workbench {
    fn body(self, _ctx: &Context) -> impl View {
        let page = match self.mode.get() {
            Mode::Board => erased(board(self.legend_rows, &self.flags)),
            Mode::Table => erased(table()),
        };
        vstack!(
            tab_strip(self.mode),
            hstack!(sidebar(), page).spacing(0.0),
            StatusBar { pulse: self.pulse },
        )
        .spacing(0.0)
        .alignment(HorizontalAlignment::Leading)
        .frame(VIEWPORT.width, VIEWPORT.height)
        .background_color(theme::panel())
    }
}

// MARK: - Chrome

fn tab_strip(mode: State<Mode>) -> impl View<Arity = Single> {
    let names = ["Code", "Board", "Table", "Infra", "Atrium", "X-Ray"];
    let tabs: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            erased(
                text(*name)
                    .foreground_color(theme::fg())
                    .padding_edge(Edge::Leading, 14.0)
                    .padding_edge(Edge::Trailing, 14.0)
                    .padding_edge(Edge::Top, 8.0)
                    .padding_edge(Edge::Bottom, 8.0)
                    .background_hovered(theme::row_pressed())
                    .on_click(move || {
                        mode.set(if index == 2 { Mode::Table } else { Mode::Board });
                    })
                    .id(format!("tab-{index}")),
            )
        })
        .collect();
    hstack!(tabs).spacing(2.0).frame_height(36.0)
}

fn sidebar() -> impl View<Arity = Single> {
    let rows: Vec<_> = (0..SIDEBAR_ROWS)
        .map(|index| {
            erased(
                hstack!(
                    rectangle()
                        .frame(10.0, 10.0)
                        .background_color(SERIES[index % SERIES.len()])
                        .corner_radius(2.0),
                    text(format!("Saved view {index:02}")).foreground_color(theme::fg()),
                    spacer(),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Leading, 10.0)
                .padding_edge(Edge::Top, 5.0)
                .padding_edge(Edge::Bottom, 5.0)
                .background_hovered(theme::row_pressed())
                .on_click(|| {})
                .id(format!("side-{index}")),
            )
        })
        .collect();
    scroll(vstack!(rows).spacing(0.0).alignment(HorizontalAlignment::Leading))
        .id("sidebar")
        .frame_width(220.0)
}

#[derive(Clone, Copy)]
struct StatusBar {
    pulse: State<usize>,
}

impl Component for StatusBar {
    fn body(self, _ctx: &Context) -> impl View {
        let dot = SERIES[self.pulse.get() % SERIES.len()];
        hstack!(
            text("main · 8 charts · ready").foreground_color(theme::fg_secondary()),
            spacer(),
            rectangle()
                .frame(10.0, 10.0)
                .background_color(dot)
                .corner_radius(5.0)
                .animated(Spring::smooth()),
        )
        .spacing(8.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Leading, 12.0)
        .padding_edge(Edge::Trailing, 12.0)
        .frame_height(24.0)
    }
}

// MARK: - The board

fn board(legend_rows: usize, flags: &Rc<Vec<State<bool>>>) -> impl View<Arity = Single> {
    let panels: Vec<_> = (0..PANELS)
        .map(|index| erased(Panel { index, legend_rows, flag: flags[index] }.id(format!("panel-{index}"))))
        .collect();
    scroll(vstack!(panels).spacing(12.0).alignment(HorizontalAlignment::Leading).padding_length(12.0))
        .id("board")
}

#[derive(Clone, Copy)]
struct Panel {
    index: usize,
    legend_rows: usize,
    flag: State<bool>,
}

impl Component for Panel {
    fn body(self, _ctx: &Context) -> impl View {
        // the plot and its legend share the room the panel is given: the
        // size comes back through a probe, and the second pass places the
        // legend with it — a product's mount is two passes for this reason
        let measured = State::new(Size { width: 480.0, height: 300.0 });
        let side = measured.get().width > 700.0;
        let index = self.index;
        let plot = canvas(move |ctx, painter| chart(index, ctx, painter))
            .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center);
        let legend = legend(index, self.legend_rows, self.flag);
        let content = if side {
            erased(hstack!(plot, legend.frame_width(260.0)).spacing(12.0))
        } else {
            erased(vstack!(plot, legend.frame_height(96.0)).spacing(12.0))
        };
        vstack!(
            header(index),
            content.frame_height(240.0).on_measure(move |size| {
                let old = measured.get();
                if (old.width - size.width).abs() > 0.5 || (old.height - size.height).abs() > 0.5 {
                    measured.set(size);
                }
            }),
        )
        .spacing(8.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(10.0)
        .background_color(theme::row_pressed())
        .corner_radius(8.0)
    }
}

fn header(index: usize) -> impl View<Arity = Single> {
    let kinds = ["Grouped revenue", "Revenue trend", "Region share"];
    let tools: Vec<_> = (0..3)
        .map(|tool| {
            erased(
                rectangle()
                    .frame(16.0, 16.0)
                    .background_color(theme::border())
                    .corner_radius(3.0)
                    .background_hovered(theme::accent())
                    .on_click(|| {})
                    .tooltip("A panel tool")
                    .id(format!("tool-{tool}")),
            )
        })
        .collect();
    hstack!(
        rectangle().frame(3.0, 16.0).background_color(SERIES[index % SERIES.len()]),
        text(format!("{} {index}", kinds[index % kinds.len()])).foreground_color(theme::fg()),
        spacer(),
        hstack!(tools).spacing(6.0),
    )
    .spacing(8.0)
    .alignment(VerticalAlignment::Center)
}

fn legend(panel: usize, rows: usize, flag: State<bool>) -> ScrollView<impl View> {
    let mut lines: Vec<Erased> = Vec::with_capacity(rows + 1);
    lines.push(erased(LegendFlag { flag }.id("flag")));
    for row in 0..rows {
        lines.push(erased(LegendRow { panel, row }.id(format!("legend-{row}"))));
    }
    scroll(vstack!(lines).spacing(4.0).alignment(HorizontalAlignment::Leading))
}

/// One legend row. It is a body of its own, it answers a click and it
/// paints a hover — a product's legend hides and shows a series.
#[derive(Clone, Copy)]
struct LegendRow {
    panel: usize,
    row: usize,
}

impl Component for LegendRow {
    fn body(self, _ctx: &Context) -> impl View {
        let (panel, row) = (self.panel, self.row);
        let label = format!("Region {panel} · Series {row:03} · a long label that does not fit");
        let value = format!("{:.0}", sample(panel, row) * 250_000.0);
        hstack!(
            rectangle()
                .frame(8.0, 8.0)
                .background_color(SERIES[row % SERIES.len()])
                .corner_radius(2.0),
            text(label.clone())
                .font(Font::Subheadline)
                .foreground_color(theme::fg_secondary())
                .truncation_mode(Truncation::End),
            spacer(),
            text(value).font(Font::Subheadline).monospaced().foreground_color(theme::fg_secondary()),
        )
        .spacing(6.0)
        .alignment(VerticalAlignment::Center)
        .background_hovered(theme::row_pressed())
        .on_click(|| {})
        .tooltip(label)
    }
}

/// One small body deep in the tree: the scenario that changes one state
/// far from the root toggles this and nothing else.
#[derive(Clone, Copy)]
struct LegendFlag {
    flag: State<bool>,
}

impl Component for LegendFlag {
    fn body(self, _ctx: &Context) -> impl View {
        let on = self.flag.get();
        text(if on { "Totals shown" } else { "Totals hidden" })
            .font(Font::Subheadline)
            .foreground_color(if on { theme::accent() } else { theme::fg_secondary() })
    }
}

// MARK: - The paint

fn chart(index: usize, ctx: &PaintCtx, painter: &mut Painter) {
    let size = ctx.size();
    if size.width < 8.0 || size.height < 8.0 {
        return;
    }
    // the axis: four labels and four rules
    for tick in 0..4 {
        let y = size.height * (tick as f64 + 0.5) / 4.0;
        painter.fill(
            Rect { origin: Point { x: 36.0, y }, size: Size { width: size.width - 36.0, height: 1.0 } },
            theme::border(),
        );
        painter.text(Point { x: 0.0, y: y - 7.0 }, format!("{}k", (4 - tick) * 50), theme::fg_secondary());
    }
    let plot = Rect {
        origin: Point { x: 40.0, y: 4.0 },
        size: Size { width: size.width - 44.0, height: size.height - 8.0 },
    };
    match index % 3 {
        0 => bars(index, plot, painter),
        1 => line(index, plot, painter),
        _ => ring(index, plot, painter),
    }
}

fn bars(index: usize, plot: Rect, painter: &mut Painter) {
    let count = 24;
    let step = plot.size.width / count as f64;
    for bar in 0..count {
        let height = plot.size.height * (0.15 + 0.8 * sample(index, bar));
        painter.fill_rounded(
            Rect {
                origin: Point {
                    x: plot.origin.x + step * bar as f64 + 2.0,
                    y: plot.origin.y + plot.size.height - height,
                },
                size: Size { width: (step - 4.0).max(1.0), height },
            },
            SERIES[bar % SERIES.len()],
            2.0,
        );
    }
}

fn line(index: usize, plot: Rect, painter: &mut Painter) {
    let count = 48;
    let at = |point: usize| {
        let x = plot.origin.x + plot.size.width * point as f64 / (count - 1) as f64;
        let y = plot.origin.y + plot.size.height * (0.9 - 0.75 * sample(index, point));
        (x as f32, y as f32)
    };
    let mut verbs = Vec::with_capacity(count + 3);
    let (x, y) = at(0);
    verbs.push(Verb::Move(x, y));
    for point in 1..count {
        let (x, y) = at(point);
        verbs.push(Verb::Line(x, y));
    }
    painter.path(&verbs, Paint::Stroke { width: 2.0 }, SERIES[index % SERIES.len()]);
    // the wash under the line: the same contour, closed along the floor
    let floor = (plot.origin.y + plot.size.height) as f32;
    verbs.push(Verb::Line((plot.origin.x + plot.size.width) as f32, floor));
    verbs.push(Verb::Line(plot.origin.x as f32, floor));
    verbs.push(Verb::Close);
    let wash = SERIES[index % SERIES.len()];
    painter.path(&verbs, Paint::Fill(Rule::NonZero), Color::rgba(wash.r, wash.g, wash.b, 48));
}

fn ring(index: usize, plot: Rect, painter: &mut Painter) {
    let slices = 5;
    let steps = 64;
    let center = (
        plot.origin.x + plot.size.width / 2.0,
        plot.origin.y + plot.size.height / 2.0,
    );
    let outer = plot.size.height.min(plot.size.width) / 2.0;
    let inner = outer * 0.6;
    let weights: Vec<f64> = (0..slices).map(|slice| 0.4 + sample(index, slice)).collect();
    let total: f64 = weights.iter().sum();
    let mut start = -std::f64::consts::FRAC_PI_2;
    for (slice, weight) in weights.iter().enumerate() {
        let sweep = std::f64::consts::TAU * weight / total;
        let mut verbs = Vec::with_capacity(2 * steps + 3);
        for step in 0..=steps {
            let angle = start + sweep * step as f64 / steps as f64;
            let (x, y) = ((center.0 + outer * angle.cos()) as f32, (center.1 + outer * angle.sin()) as f32);
            verbs.push(if step == 0 { Verb::Move(x, y) } else { Verb::Line(x, y) });
        }
        for step in (0..=steps).rev() {
            let angle = start + sweep * step as f64 / steps as f64;
            verbs.push(Verb::Line(
                (center.0 + inner * angle.cos()) as f32,
                (center.1 + inner * angle.sin()) as f32,
            ));
        }
        verbs.push(Verb::Close);
        painter.path(&verbs, Paint::Fill(Rule::NonZero), SERIES[slice % SERIES.len()]);
        start += sweep;
    }
}

// MARK: - The table

fn table() -> impl View<Arity = Single> {
    let rows: Vec<usize> = (0..TABLE_ROWS).collect();
    let body = list(
        rows,
        |row| row.to_string(),
        |row| {
            let row = *row;
            let cells: Vec<_> = (0..6)
                .map(|column| {
                    erased(
                        text(format!("r{row:03} c{column} {:.2}", sample(row, column) * 1000.0))
                            .font(Font::Subheadline)
                            .monospaced()
                            .foreground_color(theme::fg_secondary())
                            .frame_width(150.0),
                    )
                })
                .collect();
            hstack!(cells)
                .spacing(8.0)
                .padding_edge(Edge::Leading, 12.0)
                .padding_edge(Edge::Top, 4.0)
                .padding_edge(Edge::Bottom, 4.0)
                .background_hovered(theme::row_pressed())
                .on_click(|| {})
        },
    );
    scroll(body).id("table")
}
