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

// o tema-de-um-lápis do finder (tokens de verdade chegam com o port do tema)
const BACKDROP: Color = Color::hex_a(0x0F172A55);
const PANEL: Color = Color::WHITE;
const PANEL_BORDER: Color = Color::hex_a(0x64748B55);
const DIVIDER: Color = Color::hex_a(0x64748B2E);
const NAME: Color = Color::hex(0x111827);
const DIR: Color = Color::hex(0x8A94A6);
const FAINT: Color = Color::hex(0xB3BAC7);
const ACCENT: Color = Color::hex(0x3B82F6);
const ROW_HOVER: Color = Color::hex_a(0x3B82F617);
const ROW_PRESSED: Color = Color::hex_a(0x3B82F62E);

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
];

/// Subsequência case-insensitive — o piso honesto de um fuzzy.
fn matches(haystack: &str, needle: &str) -> bool {
    let mut haystack = haystack.chars().map(|c| c.to_ascii_lowercase());
    needle
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .all(|wanted| haystack.any(|c| c == wanted))
}

fn divider() -> impl UnaryView {
    spacer().frame(PANEL_W, 1.0).background_color(DIVIDER)
}

#[derive(Clone, Copy)]
struct Finder {
    query: State<String>,
    opened: State<String>,
}

impl Component for Finder {
    fn body(self, _ctx: &Context) -> impl View {
        let query = self.query.get();
        let items: Vec<(String, String, bool)> = FILES
            .iter()
            .filter(|(name, dir, _)| {
                query.is_empty() || matches(&format!("{dir}{name}"), &query)
            })
            .map(|(name, dir, recent)| (name.to_string(), dir.to_string(), *recent))
            .collect();
        let count = items.len();
        let opened = self.opened;

        let header = hstack!(
            text("›").font(Font::Headline).foreground_color(ACCENT),
            text_field("Search files by name…", self.query.binding()).monospaced(),
        )
        .spacing(10.0)
        .alignment(VerticalAlignment::Center)
        .padding_length(10.0);

        let results = list(
            items,
            |item| format!("{}{}", item.1, item.0),
            move |item| {
                let (name, dir, recent) = item.clone();
                let label = format!("{dir}{name}");
                hstack!(
                    text(name).font(Font::Body).foreground_color(NAME),
                    text(dir).font(Font::Subheadline).monospaced().foreground_color(DIR),
                    spacer(),
                    recent.then(|| {
                        text("recent").font(Font::Footnote).foreground_color(FAINT)
                    }),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Leading, 12.0)
                .padding_edge(Edge::Trailing, 12.0)
                .padding_edge(Edge::Top, 7.0)
                .padding_edge(Edge::Bottom, 7.0)
                .background_hovered(ROW_HOVER)
                .background_pressed(ROW_PRESSED)
                .on_click(move || opened.set(label.clone()))
            },
        );

        let body = if count == 0 {
            Either::Second(
                text("No matches")
                    .font(Font::Callout)
                    .foreground_color(FAINT)
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
        let footer = hstack!(
            text!("{count} files")
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(DIR),
            spacer(),
            text(status).font(Font::Subheadline).monospaced().foreground_color(DIR),
        )
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Leading, 12.0)
        .padding_edge(Edge::Trailing, 12.0)
        .padding_edge(Edge::Top, 8.0)
        .padding_edge(Edge::Bottom, 8.0);

        let panel = vstack!(header, divider(), body, divider(), footer)
            .alignment(HorizontalAlignment::Leading)
            .frame(PANEL_W, PANEL_H)
            .background_color(PANEL)
            .corner_radius(9.0)
            .border(PANEL_BORDER, 1.0);

        zstack!(
            spacer().background_color(BACKDROP),
            vstack!(spacer().frame(1.0, PANEL_TOP), panel, spacer()),
        )
    }
}

fn main() {
    bunny_ui_macos::run_window(
        "Finder",
        Size { width: 760.0, height: 640.0 },
        Finder { query: State::new(String::new()), opened: State::new(String::new()) },
    );
}
