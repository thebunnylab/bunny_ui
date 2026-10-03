//! The bunny_ui landing page, written in bunny_ui.
//!
//! The scene lowers to elements: the browser lays the page out, selects
//! its text and scrolls it at home, and the engine only says what the
//! page is. The build renders the same scene to HTML (examples/render.rs),
//! so the page paints before the wasm arrives and the boot adopts it.
//!
//! The width decides the shape — two columns or one, how many targets
//! to a row — read from the Viewport, so only this body runs again when
//! the window moves.

use bunny_ui::prelude::*;

// MARK: - The Trinity palette

const BG: Color = Color::hex(0x0A0710);
const BG_RAISED: Color = Color::hex(0x130B24);
const BG_EDITOR: Color = Color::hex(0x0C0817);
const PURPLE: Color = Color::hex(0x5035C0);
const PURPLE_HOVER: Color = Color::hex(0x6244D6);
const VIOLET: Color = Color::hex(0xAA69FB);
const PINK: Color = Color::hex(0xF690FD);
const PINK_HOVER: Color = Color::hex(0xF9AEFE);
const FG1: Color = Color::hex(0xF2EEFB);
const FG2: Color = Color::hex(0xB5ABCD);
const FG3: Color = Color::hex(0x8F86A8);
const FG4: Color = Color::hex(0x5B5470);
const HAIRLINE: Color = Color::rgba(170, 105, 251, 20);
const SOFT: Color = Color::rgba(170, 105, 251, 36);
const STRONG: Color = Color::rgba(170, 105, 251, 89);

const DISPLAY: &str = "Space Grotesk";
const BODY: &str = "Instrument Sans";
const MONO: &str = "IBM Plex Mono";

const GITHUB: &str = "https://github.com/thebunnylab/bunny_ui";
const TRINITY: &str = "https://trinity.thebunnylab.com";
const LAB: &str = "https://thebunnylab.com";
const DOCS: &str = "https://github.com/thebunnylab/bunny_ui#readme";

const CONTAINER: f64 = 1120.0;

// MARK: - The shape the width decides

/// What the window's width says about the page — every number the
/// layout bends with, decided once per body.
#[derive(Clone, Copy)]
struct Shape {
    width: f64,
    /// The room on each side: the gutter, or what centres the column.
    side: f64,
    /// Two columns side by side, or one under the other.
    split: bool,
    /// Targets to a row.
    targets: usize,
    /// Paint cards to a row.
    cards: usize,
    /// The hero's display size — the design's `clamp(48px, 7vw, 96px)`.
    hero: f64,
    /// The vertical rhythm of a section.
    section: f64,
}

impl Shape {
    fn of(width: f64) -> Shape {
        let gutter = if width < 640.0 { 20.0 } else { 32.0 };
        let column = (width - 2.0 * gutter).min(CONTAINER);
        Shape {
            width,
            side: ((width - column) / 2.0).max(gutter),
            split: column >= 860.0,
            targets: if column >= 1060.0 {
                6
            } else if column >= 520.0 {
                3
            } else {
                2
            },
            cards: if column >= 1000.0 { 3 } else { 1 },
            hero: (width * 0.07).clamp(48.0, 96.0),
            section: if width < 640.0 { 72.0 } else { 110.0 },
        }
    }

    /// The column's own width, inside the sides.
    fn column(&self) -> f64 {
        self.width - 2.0 * self.side
    }
}

// MARK: - Type

fn mono(words: &str, size: f64, ink: Color) -> impl View<Arity = Single> + use<> {
    text(std::sync::Arc::<str>::from(words)).monospaced().font_family(MONO).font_size(size).foreground_color(ink)
}

fn eyebrow(words: &str, ink: Color) -> impl View<Arity = Single> + use<> {
    mono(words, 12.0, ink).tracking_em(0.18)
}

fn heading(words: &str) -> impl View<Arity = Single> + use<> {
    text(words)
        .font_family(DISPLAY)
        .bold()
        .font_size(40.0)
        .line_height(44.0)
        .tracking_em(-0.02)
        .foreground_color(FG1)
        .element("h2")
}

fn paragraph(words: &str) -> impl View<Arity = Single> + use<> {
    text(words).font_family(BODY).font_size(16.0).line_height(25.6).foreground_color(FG2)
}

/// A run of a sentence — the words around an inline `code`.
fn run(words: &str) -> impl View<Arity = Single> + use<> {
    text(words).element("span")
}

/// A word set apart in a sentence.
fn code(words: &str) -> impl View<Arity = Single> + use<> {
    mono(words, 13.0, FG1).element("code")
}

// MARK: - Boxes

/// Takes the width it is offered, never the height: the column's
/// children reach both edges, and the page stays as tall as it reads.
fn wide<V: View<Arity = Single>>(view: V) -> impl View<Arity = Single> {
    view.frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
}

/// A page band: edge to edge, its content held in the centred column.
fn band<V: View<Arity = Single>>(shape: Shape, top: f64, bottom: f64, content: V) -> impl View<Arity = Single> {
    wide(content)
        .padding_edge(Edge::Leading, shape.side)
        .padding_edge(Edge::Trailing, shape.side)
        .padding_edge(Edge::Top, top)
        .padding_edge(Edge::Bottom, bottom)
}

/// A hairline across whatever holds it: a row of nothing but a spacer
/// takes the width it is offered, and the frame pins it one point tall.
fn hrule(ink: Color) -> impl View<Arity = Single> {
    hstack!(spacer()).frame_height(1.0).background_color(ink)
}

/// The upright twin: as tall as the row it stands in.
fn vrule(ink: Color) -> impl View<Arity = Single> {
    vstack!(spacer()).frame_width(1.0).background_color(ink)
}

/// A hairline above a band, the design's `border-top`.
fn ruled<V: View<Arity = Single>>(content: V) -> impl View<Arity = Single> {
    vstack!(hrule(HAIRLINE), wide(content))
}

/// Two columns side by side when the width allows, else one under the
/// other — the design's `repeat(auto-fit, minmax(380px, 1fr))`.
fn columns<A, B>(shape: Shape, gap: f64, first: A, second: B) -> impl View<Arity = Single>
where
    A: View<Arity = Single>,
    B: View<Arity = Single>,
{
    if shape.split {
        Either::First(
            hstack!(wide(first), wide(second))
                .spacing(gap)
                .alignment(VerticalAlignment::Top),
        )
    } else {
        Either::Second(
            vstack!(wide(first), wide(second))
                .spacing(48.0)
                .alignment(HorizontalAlignment::Leading),
        )
    }
}

/// A file in an editor pane: its name on a bar, its lines under it.
fn pane(name: &str, status: Option<&str>, ground: Color, lines: &[&[(&str, Color)]]) -> impl View<Arity = Single> + use<> {
    let bar = hstack!(
        mono(name, 11.0, FG4),
        spacer(),
        match status {
            Some(status) => Either::First(mono(status, 11.0, FG4)),
            None => Either::Second(empty()),
        },
    )
    .padding_edge(Edge::Top, 10.0)
    .padding_edge(Edge::Bottom, 10.0)
    .padding_edge(Edge::Leading, 16.0)
    .padding_edge(Edge::Trailing, 16.0);
    vstack!(
        wide(bar),
        hrule(HAIRLINE),
        wide(listing(lines, 13.5, 23.6).padding_edge(Edge::Leading, 24.0)
            .padding_edge(Edge::Trailing, 24.0)
            .padding_edge(Edge::Top, 22.0)
            .padding_edge(Edge::Bottom, 22.0)),
    )
    .alignment(HorizontalAlignment::Leading)
    .background_color(ground)
    .border(SOFT, 1.0)
    .corner_radius(8.0)
    .clipped()
}

/// Lines of code, each a sentence of coloured runs: the whitespace
/// stays, a long line wraps in a narrow pane.
fn listing(lines: &[&[(&str, Color)]], size: f64, leading: f64) -> impl View<Arity = Single> + use<> {
    let lines: Vec<Vec<(String, Color)>> = lines
        .iter()
        .map(|line| line.iter().map(|(words, ink)| (words.to_string(), *ink)).collect())
        .collect();
    vstack!(for_each(
        (0..lines.len()).collect::<Vec<_>>(),
        |index| format!("{index}"),
        move |index| {
            let runs = lines[*index].clone();
            // an empty line still holds its height
            let runs = if runs.is_empty() { vec![(" ".to_string(), FG2)] } else { runs };
            for_each(
                (0..runs.len()).collect::<Vec<_>>(),
                |index| format!("{index}"),
                move |at| {
                    let (words, ink) = &runs[*at];
                    text(words.clone()).foreground_color(*ink).element("span")
                },
            )
            .horizontal()
        },
    ))
    .alignment(HorizontalAlignment::Leading)
    .monospaced()
    .font_family(MONO)
    .font_size(size)
    .line_height(leading)
    .foreground_color(FG2)
}

/// The primary press: a filled button that leaves for `url`.
fn primary(label: &str, url: &str, fill: Color, hover: Color, ink: Color) -> impl View<Arity = Single> + use<> {
    hstack!(text(label).font_family(BODY).font_weight(Weight::Medium).font_size(14.0))
        .spacing(8.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Top, 12.0)
        .padding_edge(Edge::Bottom, 12.0)
        .padding_edge(Edge::Leading, 20.0)
        .padding_edge(Edge::Trailing, 20.0)
        .foreground_color(ink)
        .background_color(fill)
        .background_hovered(hover)
        .corner_radius(4.0)
        .animated(Spring::snappy())
        .link(url)
}

/// The quiet press: an outline that lights under the pointer.
fn outline(label: &str, url: &str) -> impl View<Arity = Single> + use<> {
    text(label)
        .font_family(BODY)
        .font_weight(Weight::Medium)
        .font_size(14.0)
        .foreground_color(FG1)
        .padding_edge(Edge::Top, 11.0)
        .padding_edge(Edge::Bottom, 11.0)
        .padding_edge(Edge::Leading, 19.0)
        .padding_edge(Edge::Trailing, 19.0)
        .border(STRONG, 1.0)
        .background_hovered(Color::rgba(170, 105, 251, 24))
        .corner_radius(4.0)
        .animated(Spring::snappy())
        .link(url)
}

/// A link set in mono, faint until the pointer finds it.
fn quiet_link(label: &str, url: &str) -> impl View<Arity = Single> + use<> {
    mono(label, 12.0, FG3).foreground_hovered(FG1).link(url)
}

// MARK: - The page

#[derive(Clone)]
struct Landing {
    plain: ImageSource,
    skeleton: ImageSource,
    texture: ImageSource,
}

impl Component for Landing {
    fn body(self, ctx: &Context) -> impl View {
        let shape = Shape::of(ctx.environment::<Viewport>().width.max(320.0));
        scroll(
            vstack!(
                hero(shape, self.texture.clone()),
                quick_look(shape),
                targets(shape),
                work_that_waits(shape),
                a_box_of_its_own(shape),
                paint(shape),
                rules(shape),
                status(shape),
                trinity(shape, self.plain.clone(), self.skeleton.clone()),
                footer(shape),
            )
            .alignment(HorizontalAlignment::Leading),
        )
        // the bar floats over the page: what scrolls under it shows
        // through its glass
        .overlay(UnitPoint::TOP, nav(shape, self.plain.clone()))
        .background_color(BG)
        .font_family(BODY)
        .foreground_color(FG2)
    }
}

fn nav(shape: Shape, mark: ImageSource) -> impl View<Arity = Single> {
    let brand = hstack!(
        image(mark).resizable().frame(13.4, 22.0),
        text("bunny_ui")
            .font_family(DISPLAY)
            .font_weight(Weight::Medium)
            .font_size(14.0)
            .tracking_em(-0.01)
            .foreground_color(FG1),
        if shape.width >= 640.0 {
            Either::First(mono("/ the bunny lab", 11.0, FG4))
        } else {
            Either::Second(empty())
        },
    )
    .spacing(10.0)
    .alignment(VerticalAlignment::Center)
    .link("#top");
    let links = hstack!(
        if shape.width >= 760.0 {
            Either::First(
                hstack!(
                    quiet_link("quick look", "#quick"),
                    quiet_link("targets", "#targets"),
                    quiet_link("docs", DOCS),
                    quiet_link("trinity", TRINITY),
                )
                .spacing(22.0)
                .alignment(VerticalAlignment::Center),
            )
        } else {
            Either::Second(empty())
        },
        mono("github →", 12.0, FG1)
            .padding_edge(Edge::Top, 6.0)
            .padding_edge(Edge::Bottom, 6.0)
            .padding_edge(Edge::Leading, 12.0)
            .padding_edge(Edge::Trailing, 12.0)
            .border(STRONG, 1.0)
            .background_hovered(Color::rgba(170, 105, 251, 24))
            .corner_radius(4.0)
            .animated(Spring::snappy())
            .link(GITHUB),
    )
    .spacing(22.0)
    .alignment(VerticalAlignment::Center);
    let gutter = if shape.width < 640.0 { 20.0 } else { 32.0 };
    vstack!(
        wide(
            hstack!(brand, spacer(), links)
                .alignment(VerticalAlignment::Center)
                .frame_height(55.0)
                .padding_edge(Edge::Leading, gutter)
                .padding_edge(Edge::Trailing, gutter),
        ),
        hrule(HAIRLINE),
    )
    .glass(
        Glass::frosted()
            .blur(14.0)
            .tint(Color::rgba(10, 7, 16, 184))
            .highlight(Color::rgba(0, 0, 0, 0), 0.0, 0.0)
            .saturation(1.0)
            .brightness(1.0),
    )
    .frame_height(56.0)
    .element("nav")
}

fn hero(shape: Shape, texture: ImageSource) -> impl View<Arity = Single> {
    let calls = hstack!(
        primary("view the source  →", GITHUB, PURPLE, PURPLE_HOVER, FG1),
        outline("read the docs", DOCS),
        mono("rust, end to end · std only · the api is not stable", 12.0, FG4)
            .padding_edge(Edge::Leading, 8.0),
    )
    .spacing(12.0)
    .line_spacing(12.0)
    .wrapping()
    .alignment(VerticalAlignment::Center);
    let content = vstack!(
        eyebrow("FRAMEWORK / BUNNY_UI · EARLY DEVELOPMENT", VIOLET),
        text("One codebase. Every surface.")
            .font_family(DISPLAY)
            .bold()
            .font_size(shape.hero)
            .line_height(shape.hero)
            .tracking_em(-0.03)
            .foreground_color(FG1)
            .frame_max(900.0, f64::MAX, Alignment::Leading)
            .element("h1"),
        text(
            "bunny_ui is a declarative UI framework for Rust, inspired by SwiftUI. \
             Write views as value types. The framework finds the views that read \
             changed state and runs only those.",
        )
        .font_size(if shape.width < 640.0 { 17.0 } else { 19.0 })
        .line_height(if shape.width < 640.0 { 26.0 } else { 29.5 })
        .foreground_color(FG2)
        .frame_max(620.0, f64::MAX, Alignment::Leading),
        calls,
    )
    .spacing(28.0)
    .alignment(HorizontalAlignment::Leading);
    let top = if shape.width < 640.0 { 140.0 } else { 190.0 };
    let bottom = if shape.width < 640.0 { 90.0 } else { 130.0 };
    // the glow over the fade over the texture — the design's two
    // backgrounds, a box each, the first drawn on top
    let washed = band(shape, top, bottom, content)
        .background_gradient(
            Gradient::radial(Color::rgba(80, 53, 192, 72), Color::rgba(80, 53, 192, 0))
                .center(UnitPoint::new(0.5, 0.3))
                .radius(0.0, shape.width * 0.7 * 0.7)
                .aspect(0.6 * 900.0 / (shape.width * 0.7)),
        )
        .background_gradient(
            Gradient::linear(Color::rgba(10, 7, 16, 51), BG),
        );
    // the texture lies under the words and takes their box: it gives
    // the hero no size of its own (the page bleeds it past the edges)
    washed
        .css_class("hero-body")
        .background(
            UnitPoint::CENTER,
            image(texture)
                .resizable()
                .aspect_ratio(ContentMode::Fill)
                .opacity(0.55)
                .css_class("hero-texture"),
        )
        .clipped()
        .css_class("hero")
        .element_id("top")
        .element("header")
}

const PRINCIPLES: [(&str, &str, &str); 4] = [
    (
        "00",
        "One codebase, five targets",
        "iOS, Android, macOS, Linux and the web from a single Rust crate. No bridges, \
         no JS runtime, no second team keeping the ports honest.",
    ),
    (
        "01",
        "Fast is the default",
        "No virtual DOM, no diffing the world. A render pass runs only the views that \
         read changed state.",
    ),
    (
        "10",
        "GPU-accelerated renderer",
        "The desktop composites every frame on the GPU, and the web can too. Same \
         rasterizer everywhere — your pixels agree byte for byte.",
    ),
    (
        "11",
        "Zero bloat",
        "The framework crates use only the Rust standard library. Nothing to audit, \
         nothing to carry, nothing between you and the frame.",
    ),
];

fn quick_look(shape: Shape) -> impl View<Arity = Single> {
    let principles = vstack!(for_each(
        PRINCIPLES.to_vec(),
        |(number, _, _)| number.to_string(),
        |(number, title, detail)| {
            ruled(
                hstack!(
                    mono(*number, 12.0, FG4).padding_edge(Edge::Top, 3.0).frame_width(40.0),
                    wide(
                        vstack!(
                            text(*title)
                                .font_family(DISPLAY)
                                .font_weight(Weight::Medium)
                                .font_size(16.0)
                                .foreground_color(FG1),
                            text(*detail).font_size(14.5).line_height(22.5).foreground_color(FG3),
                        )
                        .spacing(5.0)
                        .alignment(HorizontalAlignment::Leading),
                    ),
                )
                .spacing(14.0)
                .alignment(VerticalAlignment::Top)
                .padding_edge(Edge::Top, 16.0)
                .padding_edge(Edge::Bottom, 16.0),
            )
        },
    ))
    .alignment(HorizontalAlignment::Leading)
    .padding_edge(Edge::Top, 8.0);
    let story = vstack!(
        eyebrow("QUICK LOOK", VIOLET),
        heading("A tap re-runs one view. Not your app."),
        wide(
            hstack!(
                run("The display of "),
                code("count"),
                run(" records a read. A tap changes the state, and the framework runs only this view again."),
            )
            .font_size(16.0)
            .line_height(25.6)
            .foreground_color(FG2)
            .frame_max(480.0, f64::MAX, Alignment::Leading),
        ),
        wide(principles),
    )
    .spacing(22.0)
    .alignment(HorizontalAlignment::Leading);
    let k = VIOLET;
    let t = FG1;
    let s = PINK;
    let c = FG4;
    let p = FG2;
    let counter: &[&[(&str, Color)]] = &[
        &[("#[derive(Clone, Copy)]", c)],
        &[("struct", k), (" ", p), ("Counter", t), (" {", p)],
        &[("    count: ", p), ("State", t), ("<", p), ("i32", t), (">,", p)],
        &[("}", p)],
        &[],
        &[("impl", k), (" ", p), ("Component", t), (" ", p), ("for", k), (" ", p), ("Counter", t), (" {", p)],
        &[
            ("    ", p),
            ("fn", k),
            (" ", p),
            ("body", t),
            ("(", p),
            ("self", k),
            (", _ctx: &", p),
            ("Context", t),
            (") -> ", p),
            ("impl", k),
            (" ", p),
            ("View", t),
            (" {", p),
        ],
        &[("        ", p), ("vstack!", t), ("(", p)],
        &[("            ", p), ("text!", t), ("(", p), ("\"Count: {}\"", s), (", ", p), ("self", k), (".count),", p)],
        &[
            ("            ", p),
            ("button", t),
            ("(", p),
            ("text", t),
            ("(", p),
            ("\"Tap\"", s),
            ("), ", p),
            ("move", k),
            (" || ", p),
            ("self", k),
            (".count.", p),
            ("add", t),
            ("(", p),
            ("1", s),
            (")),", p),
        ],
        &[("        )", p)],
        &[("    }", p)],
        &[("}", p)],
    ];
    ruled(band(
        shape,
        shape.section,
        shape.section,
        columns(shape, 64.0, story, pane("counter.rs", Some("cargo run · ✓"), BG_EDITOR, counter)),
    ))
    .element_id("quick")
    .element("section")
}

const TARGETS: [(&str, &str, &str); 6] = [
    ("TARGET 01", "iOS", "bunny_ui_ios · metal"),
    ("TARGET 02", "Android", "bunny_ui_android · vulkan"),
    ("TARGET 03", "macOS", "bunny_ui_macos · metal"),
    ("TARGET 04", "Linux", "bunny_ui_linux · vulkan"),
    ("TARGET 05", "Windows", "bunny_ui_windows"),
    ("TARGET 06", "Web ×3", "canvas · dom · gpu tier"),
];

fn targets(shape: Shape) -> impl View<Arity = Single> {
    let per_row = shape.targets;
    let rows: Vec<usize> = (0..TARGETS.len().div_ceil(per_row)).collect();
    let cell = move |index: usize| {
        let (key, name, via) = TARGETS[index];
        let last_in_row = (index + 1) % per_row == 0;
        let last_row = index / per_row == (TARGETS.len() - 1) / per_row;
        let content = vstack!(
            mono(key, 11.0, FG4).tracking_em(0.14),
            text(name).font_family(DISPLAY).font_weight(Weight::Medium).font_size(18.0).foreground_color(FG1),
            mono(via, 12.0, FG3),
        )
        .spacing(6.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Top, 22.0)
        .padding_edge(Edge::Bottom, 22.0)
        .padding_edge(Edge::Leading, 24.0)
        .padding_edge(Edge::Trailing, 24.0);
        // the cells' hairlines: right of each, under each row
        hstack!(
            wide(vstack!(
                wide(content),
                if last_row {
                    Either::First(empty())
                } else {
                    Either::Second(hrule(HAIRLINE))
                },
            )),
            if last_in_row {
                Either::First(empty())
            } else {
                Either::Second(vrule(HAIRLINE))
            },
        )
        .alignment(VerticalAlignment::Top)
    };
    let grid = vstack!(for_each(
        rows,
        |row| format!("{row}"),
        move |row| {
            let first = row * per_row;
            let cells: Vec<usize> = (first..(first + per_row).min(TARGETS.len())).collect();
            wide(for_each(cells, |index| format!("{index}"), move |index| wide(cell(*index))).horizontal())
        },
    ))
    .alignment(HorizontalAlignment::Leading)
    .background_color(BG_RAISED)
    .border(SOFT, 1.0)
    .corner_radius(8.0)
    .clipped();
    band(
        shape,
        0.0,
        shape.section,
        vstack!(
            wide(grid),
            mono("one tree of plain rust values · one cargo build · the pixels agree byte for byte", 12.0, FG4),
        )
        .spacing(14.0)
        .alignment(HorizontalAlignment::Leading),
    )
    .element_id("targets")
    .element("section")
}

fn work_that_waits(shape: Shape) -> impl View<Arity = Single> {
    let story = vstack!(
        eyebrow("01 — WORK THAT WAITS", VIOLET),
        heading("A view can own asynchronous work."),
        wide(
            hstack!(
                code(".task"),
                run(" starts it on the view's first appearance and ends it when the view leaves the tree."),
            )
            .font_size(16.0)
            .line_height(25.6)
            .frame_max(500.0, f64::MAX, Alignment::Leading),
        ),
        paragraph(
            "The framework reads no file and opens no socket. The application does that on \
             its own thread — or through its own browser callback — and hands the results \
             over a channel. The sender is the only part that crosses a thread boundary, and \
             it carries a signal, not a scene: the shell answers with the frame it already \
             knows how to draw.",
        )
        .frame_max(500.0, f64::MAX, Alignment::Leading),
        wide(
            hstack!(
                run("Cancellation is a drop. A view that leaves the tree ends its task, the reader dies, and the next "),
                code("send"),
                run(" answers "),
                code("Err"),
                run(" — the sign for the worker to stop. "),
                code(".task_id(id)"),
                run(" restarts the work when the id moves, so a details panel that switches files cancels the read in flight."),
            )
            .font_size(16.0)
            .line_height(25.6)
            .frame_max(500.0, f64::MAX, Alignment::Leading),
        ),
    )
    .spacing(22.0)
    .alignment(HorizontalAlignment::Leading);
    let k = VIOLET;
    let t = FG1;
    let p = FG2;
    let log: &[&[(&str, Color)]] = &[
        &[("row.", p), ("task", t), ("(", p), ("move", k), (" || ", p), ("async move", k), (" {", p)],
        &[("    ", p), ("let", k), (" (lines, reader) = task::", p), ("channel", t), ("();", p)],
        &[("    std::thread::", p), ("spawn", t), ("(", p), ("move", k), (" || ", p), ("read_the_log", t), ("(lines));", p)],
        &[
            ("    ", p),
            ("while let", k),
            (" ", p),
            ("Some", t),
            ("(line) = reader.", p),
            ("recv", t),
            ("().", p),
            ("await", k),
            (" {", p),
        ],
        &[("        log.", p), ("update", t), ("(|all| all.", p), ("push", t), ("(line));", p)],
        &[("    }", p)],
        &[("})", p)],
    ];
    ruled(band(
        shape,
        shape.section,
        shape.section,
        columns(shape, 64.0, story, pane("log_view.rs", None, BG_EDITOR, log)),
    ))
    .element_id("task")
    .element("section")
}

fn a_box_of_its_own(shape: Shape) -> impl View<Arity = Single> {
    let story = vstack!(
        eyebrow("02 — A BOX THE APPLICATION OWNS", VIOLET),
        heading("Some content has no interface vocabulary."),
        paragraph(
            "A code editor, a terminal grid, a waveform. It gets a box of its own, painted \
             with the same commands every built-in view emits.",
        )
        .frame_max(500.0, f64::MAX, Alignment::Leading),
        paragraph(
            "The box paints in its own coordinates and cannot escape them — the clip around \
             it is the framework's. It hears how much of it the clip lets through, so a long \
             document costs one screen, and it inherits the ink and the font of the scope \
             above it.",
        )
        .frame_max(500.0, f64::MAX, Alignment::Leading),
        paragraph(
            "Nothing forks: the desktop composites the box on the GPU, the web canvas mode \
             on the CPU, and the element mode turns it into a canvas island.",
        )
        .frame_max(500.0, f64::MAX, Alignment::Leading),
        mono(
            "use it for content that has no views. a rounded corner, a hover state or a \
             gradient belongs in the framework.",
            12.0,
            FG4,
        )
        .line_height(19.2)
        .frame_max(500.0, f64::MAX, Alignment::Leading),
    )
    .spacing(22.0)
    .alignment(HorizontalAlignment::Leading);
    let t = FG1;
    let n = PINK;
    let c = FG4;
    let p = FG2;
    let custom: &[&[(&str, Color)]] = &[
        &[("// the short door: a box that only draws", c)],
        &[("canvas", t), ("(|ctx, p| p.", p), ("fill_rounded", t), ("(ctx.", p), ("bounds", t), ("(), ink, ", p), ("6.0", n), ("))", p)],
        &[],
        &[("// the full one: it measures, paints, and answers the pointer,", c)],
        &[("// the keyboard and the input system", c)],
        &[("custom", t), ("(", p), ("SketchPad", t), (" { strokes, caption })", p)],
    ];
    let code = pane("custom.rs", None, BG, custom);
    // the pane leads on a wide page; the words lead when they stack
    let body = if shape.split {
        Either::First(columns(shape, 64.0, code, story))
    } else {
        Either::Second(columns(shape, 64.0, story, code))
    };
    ruled(band(shape, shape.section, shape.section, body))
        .background_color(BG_EDITOR)
        .element_id("box")
        .element("section")
}

const PAINT: [(&str, &str, &str); 3] = [
    (
        "Gradients",
        "A two-stop ramp is a property of a view, declared in the box's own proportions so \
         it survives every resize. The placement resolves it to pixels once; the \
         rasterizers only evaluate.",
        "panel.background_gradient(\n    Gradient::radial(violet, violet.fade())\n        .center(UnitPoint::TOP)\n        .radius(0.0, 420.0),\n)",
    ),
    (
        "Clipping",
        ".clipped() cuts the subtree to the box — and the cut follows the .corner_radius(…) \
         already on it. There is no radius to repeat and no order to remember: the two fuse \
         into one node.",
        "vstack((toolbar(), panels()))\n    .background_color(surface)\n    .border(outline, 1.0)\n    .corner_radius(6.0)\n    .clipped()",
    ),
    (
        "Icons",
        "A glyph is a recipe, never pixels: verbs on a fixed 24 grid, plus the paint that \
         turns contours into ink. Crisp at sixteen, crisp at sixty-four.",
        "icon(symbol::CHEVRON_RIGHT)\nicon(symbol::SEARCH).font(Font::Title)\nicon(symbol::FOLDER)\n    .resizable().frame(24.0, 24.0)\nicon(acme::LOGO)",
    ),
];

fn paint(shape: Shape) -> impl View<Arity = Single> {
    let card = |index: usize| {
        let (title, detail, code) = PAINT[index];
        vstack!(
            wide(
                vstack!(
                    text(title).font_family(DISPLAY).font_weight(Weight::Medium).font_size(20.0).foreground_color(FG1),
                    text(detail).font_size(14.5).line_height(23.2).foreground_color(FG3),
                )
                .spacing(10.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_edge(Edge::Top, 22.0)
                .padding_edge(Edge::Bottom, 18.0)
                .padding_edge(Edge::Leading, 24.0)
                .padding_edge(Edge::Trailing, 24.0),
            ),
            // the code sits on the card's floor, however long the words
            spacer(),
            hrule(HAIRLINE),
            wide(
                mono(code, 12.5, FG2)
                    .line_height(21.25)
                    .padding_edge(Edge::Top, 16.0)
                    .padding_edge(Edge::Bottom, 20.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .padding_edge(Edge::Trailing, 24.0),
            )
            .background_color(BG_EDITOR),
        )
        .alignment(HorizontalAlignment::Leading)
        .background_color(BG_RAISED)
        .border(SOFT, 1.0)
        .corner_radius(8.0)
        .clipped()
    };
    let per_row = shape.cards;
    let rows: Vec<usize> = (0..PAINT.len().div_ceil(per_row)).collect();
    let grid = vstack!(for_each(
        rows,
        |row| format!("{row}"),
        move |row| {
            let first = row * per_row;
            let cells: Vec<usize> = (first..(first + per_row).min(PAINT.len())).collect();
            wide(
                for_each(cells, |index| format!("{index}"), move |index| wide(card(*index)))
                    .horizontal()
                    .spacing(14.0),
            )
        },
    ))
    .spacing(14.0)
    .alignment(HorizontalAlignment::Leading);
    ruled(band(
        shape,
        shape.section,
        shape.section,
        vstack!(
            vstack!(
                eyebrow("03 — PAINT", VIOLET),
                heading("Gradients, clipping, icons. Declared once, agreed everywhere.")
                    .frame_max(720.0, f64::MAX, Alignment::Leading),
            )
            .spacing(18.0)
            .alignment(HorizontalAlignment::Leading),
            wide(grid),
            mono(
                "sixteen symbols ship with the framework (bunny_ui::symbol). an app converts its \
                 own icon files offline — the tool prints rust const data to paste into the app; \
                 the default build carries no parser.",
                12.0,
                FG4,
            )
            .line_height(20.4)
            .frame_max(720.0, f64::MAX, Alignment::Leading),
        )
        .spacing(48.0)
        .alignment(HorizontalAlignment::Leading),
    ))
    .element_id("paint")
    .element("section")
}

const RULES: [(&str, &str); 4] = [
    ("01", "The framework crates use only the Rust standard library."),
    ("02", "Views are plain values. State lives in typed arenas behind small handles."),
    ("03", "A render pass runs only the views that read changed state."),
    ("04", "The layout protocol is a proposal from the parent and a response from the child."),
];

fn rules(shape: Shape) -> impl View<Arity = Single> {
    let list = vstack!(for_each(
        RULES.to_vec(),
        |(number, _)| number.to_string(),
        |(number, rule)| {
            vstack!(
                hrule(SOFT),
                wide(
                    hstack!(
                        mono(*number, 12.0, VIOLET).padding_edge(Edge::Top, 5.0).frame_width(36.0),
                        wide(text(*rule).font_size(17.0).line_height(25.5).foreground_color(FG1)),
                    )
                    .spacing(14.0)
                    .alignment(VerticalAlignment::Top)
                    .padding_edge(Edge::Top, 18.0)
                    .padding_edge(Edge::Bottom, 18.0),
                ),
            )
            .element("li")
        },
    ))
    .alignment(HorizontalAlignment::Leading)
    .element("ol");
    let story = vstack!(eyebrow("04 — DESIGN RULES", VIOLET), heading("Four rules. No exceptions."))
        .spacing(18.0)
        .alignment(HorizontalAlignment::Leading);
    ruled(band(shape, shape.section, shape.section, columns(shape, 64.0, story, list)))
        .background_color(BG_EDITOR)
        .element_id("rules")
        .element("section")
}

const DEMOS: [(&str, &str); 6] = [
    ("cargo run -p bunny-ui --example counter_headless", "Prints a small interface to the terminal."),
    ("cargo run -p bunny-ui-macos --example counter_window", "Opens a native macOS window."),
    (
        "cargo run -p bunny-ui-macos --example git_window",
        "Reads this repository's own git log from a worker thread and fills the window while it scrolls.",
    ),
    (
        "cargo run -p bunny-ui-macos --example sketch_window",
        "One box the application owns: draws its own ink with the pointer, sizes its brush with \
         the wheel, types into a caption of its own.",
    ),
    ("cargo run -p bunny-ui-macos --example icon_window", "The sixteen house glyphs across fonts and inks."),
    ("cargo run -p countries-pure", "A full sample application."),
];

fn status(shape: Shape) -> impl View<Arity = Single> {
    let c = FG4;
    let p = FG2;
    let build: &[&[(&str, Color)]] = &[
        &[("$", c), (" cargo build", p)],
        &[("$", c), (" cargo test", p)],
        &[("$", c), (" cargo test --features svg   ", p), ("# the icon converter's parser rides the flag", c)],
    ];
    let terminal = vstack!(
        wide(mono("build and test", 11.0, FG4).padding_edge(Edge::Top, 10.0)
            .padding_edge(Edge::Bottom, 10.0)
            .padding_edge(Edge::Leading, 16.0)
            .padding_edge(Edge::Trailing, 16.0)),
        hrule(HAIRLINE),
        wide(listing(build, 13.5, 25.65).padding_edge(Edge::Top, 18.0)
            .padding_edge(Edge::Bottom, 18.0)
            .padding_edge(Edge::Leading, 24.0)
            .padding_edge(Edge::Trailing, 24.0)),
    )
    .alignment(HorizontalAlignment::Leading)
    .background_color(BG_EDITOR)
    .border(SOFT, 1.0)
    .corner_radius(8.0)
    .clipped();
    let story = vstack!(
        eyebrow("05 — STATUS", VIOLET),
        heading("Early development. The API is not stable."),
        paragraph("It compiles on your machine. That is the whole install.")
            .frame_max(480.0, f64::MAX, Alignment::Leading),
        wide(terminal),
    )
    .spacing(22.0)
    .alignment(HorizontalAlignment::Leading);
    let demos = vstack!(
        eyebrow("DEMOS", FG4),
        wide(
            vstack!(for_each(
                DEMOS.to_vec(),
                |(command, _)| command.to_string(),
                |(command, detail)| {
                    ruled(
                        vstack!(
                            mono(*command, 13.0, FG1).element("code"),
                            text(*detail).font_size(14.0).line_height(21.0).foreground_color(FG3),
                        )
                        .spacing(6.0)
                        .alignment(HorizontalAlignment::Leading)
                        .padding_edge(Edge::Top, 14.0)
                        .padding_edge(Edge::Bottom, 14.0),
                    )
                },
            ))
            .alignment(HorizontalAlignment::Leading),
        ),
    )
    .spacing(18.0)
    .alignment(HorizontalAlignment::Leading);
    ruled(band(shape, shape.section, shape.section, columns(shape, 64.0, story, demos)))
        .element_id("run")
        .element("section")
}

fn trinity(shape: Shape, plain: ImageSource, skeleton: ImageSource) -> impl View<Arity = Single> {
    let story = vstack!(
        eyebrow("BUILT WITH IT — TRINITY", PINK),
        heading("The editor is built in the framework it edits."),
        paragraph(
            "Every panel, every canvas, every cursor in Trinity is bunny_ui rendering itself. \
             A native, GPU-rendered workbench. Rust end to end, no Electron.",
        )
        .frame_max(500.0, f64::MAX, Alignment::Leading),
        hstack!(
            primary("open trinity →", TRINITY, PINK, PINK_HOVER, BG),
            outline("code it by hand", GITHUB),
        )
        .spacing(12.0)
        .line_spacing(12.0)
        .wrapping(),
    )
    .spacing(22.0)
    .alignment(HorizontalAlignment::Leading);
    // the mark shows its bones under the pointer: both pictures share
    // one cell, and the pointer over the GROUP trades their fades — the
    // browser's own hover, not a patch
    let height = 260.0;
    let width = height * 1736.0 / 2848.0;
    let mark = zstack!(
        image(plain).resizable().frame(width, height).opacity(1.0).opacity_hovered(0.0).group_hovered(),
        image(skeleton).resizable().frame(width, height).opacity(0.0).opacity_hovered(1.0).group_hovered(),
    )
    .hover_group();
    let art = hstack!(spacer(), mark, spacer()).alignment(VerticalAlignment::Center);
    let top = if shape.width < 640.0 { 90.0 } else { 130.0 };
    ruled(
        band(shape, top, top, columns(shape, 64.0, story, art))
            .background_gradient(
                Gradient::radial(Color::rgba(246, 144, 253, 26), Color::rgba(246, 144, 253, 0))
                    .center(UnitPoint::new(0.8, 0.5))
                    .radius(0.0, shape.width * 0.6 * 0.7)
                    .aspect(0.7 * 640.0 / (shape.width * 0.6)),
            )
            .background_gradient(
                Gradient::radial(Color::rgba(80, 53, 192, 64), Color::rgba(80, 53, 192, 0))
                    .center(UnitPoint::new(0.15, 0.6))
                    .radius(0.0, shape.width * 0.5 * 0.7)
                    .aspect(0.6 * 640.0 / (shape.width * 0.5)),
            ),
    )
    .element_id("trinity")
    .element("section")
}

fn footer(shape: Shape) -> impl View<Arity = Single> {
    let links = hstack!(
        quiet_link("github", GITHUB),
        quiet_link("docs", DOCS),
        quiet_link("trinity", TRINITY),
        quiet_link("the bunny lab", LAB),
    )
    .spacing(20.0);
    let line = mono("bunny_ui · a bunny lab framework · rust, end to end", 12.0, FG4);
    let body = if shape.column() >= 720.0 {
        Either::First(hstack!(line, spacer(), links).alignment(VerticalAlignment::Center))
    } else {
        Either::Second(vstack!(line, links).spacing(20.0).alignment(HorizontalAlignment::Leading))
    };
    ruled(band(shape, 40.0, 40.0, body)).element("footer")
}

// MARK: - The boot

pub fn landing() -> impl View {
    Landing {
        plain: ImageSource::from_bytes(include_bytes!("../assets/bunnylab-plain.svg").to_vec()),
        skeleton: ImageSource::from_bytes(include_bytes!("../assets/bunnylab-animated.svg").to_vec()),
        texture: ImageSource::from_bytes(include_bytes!("../assets/tex-hero.jpg").to_vec()),
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn start_dom(width: f64, height: f64, scale: f64, hydrate: u32) {
    if hydrate != 0 {
        bunny_ui_web::start_dom_hydrated(width, height, scale, landing());
    } else {
        bunny_ui_web::start_dom(width, height, scale, landing());
    }
}
