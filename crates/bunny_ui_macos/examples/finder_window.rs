//! Um "go to file" completo — a primeira TELA de verdade do bunny_ui.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example finder_window
//! ```
//!
//! Tudo junto num lugar só: backdrop translúcido, painel flutuante, campo
//! de busca REAL (foco, caret, IME, clipboard), lista filtrada reativa
//! (digite e ela responde — só este body re-roda), rows com hover e
//! clique sem chrome de botão, rolagem com clip e thumb. O rodapé mostra
//! o que o clique "abriu".

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

const PANEL_W: f64 = 640.0;
const PANEL_H: f64 = 480.0;
const PANEL_TOP: f64 = 120.0;

/// (nome, diretório, recente?) — o mock de um projeto plausível.
const FILES: &[(&str, &str, bool)] = &[
    ("main.rs", "src/", true),
    ("layout.rs", "crates/ui/src/", true),
    ("raster.rs", "crates/ui/src/", false),
    ("runtime.rs", "crates/ui/src/", true),
    ("reconciler.rs", "crates/ui/src/", false),
    ("text_engine.rs", "crates/ui/src/", false),
    ("text_input.rs", "crates/ui/src/", false),
    ("views.rs", "crates/ui/src/", false),
    ("modifier.rs", "crates/ui/src/", false),
    ("identity.rs", "crates/engine/src/", false),
    ("state.rs", "crates/engine/src/", true),
    ("combine.rs", "crates/engine/src/", false),
    ("loadable.rs", "crates/engine/src/", false),
    ("ffi.rs", "crates/shell/src/", false),
    ("window.rs", "crates/shell/src/", false),
    ("text.rs", "crates/shell/src/", false),
    ("Cargo.toml", "", false),
    ("README.md", "", false),
    ("countries_list.rs", "apps/countries/src/ui/", false),
    ("country_cell.rs", "apps/countries/src/ui/", false),
    ("country_details.rs", "apps/countries/src/ui/", false),
    ("image_view.rs", "apps/countries/src/ui/", false),
    ("error_view.rs", "apps/countries/src/ui/", false),
    ("detail_row.rs", "apps/countries/src/ui/", false),
    ("root.rs", "apps/countries/src/", false),
    ("finder_window.rs", "examples/", false),
    ("counter_window.rs", "examples/", false),
    ("benchmark.rs", "tools/src/", false),
    ("glyphs.rs", "tools/src/", false),
    ("palette.rs", "tools/src/", false),
    (
        "deep_file.rs",
        "a/very/long/nested/path/that/keeps/going/and/going/until/it/cannot/possibly/fit/",
        false,
    ),
];

/// Subsequência case-insensitive com as posições casadas (ranges de byte
/// coalescidos) — o piso honesto de um fuzzy, com highlight de verdade.
fn match_ranges(haystack: &str, needle: &str) -> Option<Vec<(usize, usize)>> {
    if needle.is_empty() {
        return Some(Vec::new());
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut haystack = haystack.char_indices();
    'wanted: for wanted in needle.chars() {
        let wanted = wanted.to_ascii_lowercase();
        for (index, candidate) in haystack.by_ref() {
            if candidate.to_ascii_lowercase() == wanted {
                let end = index + candidate.len_utf8();
                match ranges.last_mut() {
                    Some((_, last_end)) if *last_end == index => *last_end = end,
                    _ => ranges.push((index, end)),
                }
                continue 'wanted;
            }
        }
        return None;
    }
    Some(ranges)
}

#[derive(Clone)]
struct Row {
    name: String,
    dir: String,
    recent: bool,
    name_ranges: Vec<(usize, usize)>,
    dir_ranges: Vec<(usize, usize)>,
}

fn divider() -> impl UnaryView {
    spacer().frame(PANEL_W, 1.0).background_color(theme::divider())
}

#[derive(Clone, Copy)]
struct Finder {
    query: State<String>,
    opened: State<String>,
    dark: State<bool>,
}

impl Component for Finder {
    fn body(self, _ctx: &Context) -> impl View {
        let query = self.query.get();
        let items: Vec<Row> = FILES
            .iter()
            .filter_map(|(name, dir, recent)| {
                let full = format!("{dir}{name}");
                let ranges = match_ranges(&full, &query)?;
                // reparte os ranges do caminho completo entre dir e nome
                let split = dir.len();
                let mut dir_ranges = Vec::new();
                let mut name_ranges = Vec::new();
                for (start, end) in ranges {
                    if end <= split {
                        dir_ranges.push((start, end));
                    } else if start >= split {
                        name_ranges.push((start - split, end - split));
                    } else {
                        dir_ranges.push((start, split));
                        name_ranges.push((0, end - split));
                    }
                }
                Some(Row {
                    name: name.to_string(),
                    dir: dir.to_string(),
                    recent: *recent,
                    name_ranges,
                    dir_ranges,
                })
            })
            .collect();
        let count = items.len();
        let opened = self.opened;

        let header = hstack!(
            text("›").font(Font::Headline).foreground_color(theme::accent()),
            text_field("Search files by name…", self.query.binding()).monospaced(),
        )
        .spacing(10.0)
        .alignment(VerticalAlignment::Center)
        .padding_length(10.0);

        let results = list(
            items,
            |row| format!("{}{}", row.dir, row.name),
            move |row| {
                let row = row.clone();
                let label = format!("{}{}", row.dir, row.name);
                hstack!(
                    // nome: elipse no MEIO, teto de 240 como manda a anatomia
                    text(row.name)
                        .foreground_color(theme::fg())
                        .highlight(row.name_ranges, theme::accent())
                        .truncation_mode(Truncation::Middle)
                        .frame_max(240.0, f64::INFINITY, Alignment::Leading),
                    // caminho: preenche o resto, elipse no COMEÇO (o fim
                    // do path é o que importa)
                    text(row.dir)
                        .font(Font::Subheadline)
                        .monospaced()
                        .foreground_color(theme::fg_secondary())
                        .highlight(row.dir_ranges, theme::accent())
                        .truncation_mode(Truncation::Start)
                        .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Leading),
                    row.recent.then(|| {
                        text("recent").font(Font::Footnote).foreground_color(theme::fg_faint())
                    }),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Leading, 12.0)
                .padding_edge(Edge::Trailing, 12.0)
                .padding_edge(Edge::Top, 7.0)
                .padding_edge(Edge::Bottom, 7.0)
                .background_hovered(theme::row_hover())
                .background_pressed(theme::row_pressed())
                .on_click(move || opened.set(label.clone()))
            },
        );

        let body = if count == 0 {
            Either::Second(
                text("No matches")
                    .font(Font::Callout)
                    .foreground_color(theme::fg_faint())
                    .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center),
            )
        } else {
            Either::First(results)
        };

        let opened_name = self.opened.get();
        let status = if opened_name.is_empty() {
            "click a row to open".to_string()
        } else {
            format!("opened: {opened_name}")
        };
        let dark_on = self.dark.get();
        let footer = hstack!(
            text!("{count} files")
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            // retheme AO VIVO: o install reconstrói a cena no próximo pass
            button(
                text(if dark_on { "light" } else { "dark" }).font(Font::Footnote),
                move || {
                    let next = !self.dark.get();
                    self.dark.set(next);
                    theme::install(if next { Theme::dark() } else { Theme::light() });
                },
            ),
            spacer(),
            text(status).font(Font::Subheadline).monospaced().foreground_color(theme::fg_secondary()),
        )
        .spacing(12.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Leading, 12.0)
        .padding_edge(Edge::Trailing, 12.0)
        .padding_edge(Edge::Top, 8.0)
        .padding_edge(Edge::Bottom, 8.0);

        let panel = vstack!(header, divider(), body, divider(), footer)
            .alignment(HorizontalAlignment::Leading)
            .frame(PANEL_W, PANEL_H)
            .background_color(theme::panel())
            .corner_radius(9.0)
            .border(theme::border(), 1.0);

        zstack!(
            spacer().background_color(theme::backdrop()),
            vstack!(spacer().frame(1.0, PANEL_TOP), panel, spacer()),
        )
    }
}

fn main() {
    bunny_ui_macos::run_window(
        "Finder",
        Size { width: 760.0, height: 640.0 },
        Finder {
            query: State::new(String::new()),
            opened: State::new(String::new()),
            dark: State::new(false),
        },
    );
}
