//! The house glyphs on one screen: sixteen symbols, three fonts,
//! three inks, a hover — the whole icon story in a window.
//!
//! ```sh
//! cargo run -p bunny-ui-linux --example icon_window_linux
//! ```

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use bunny_ui::icon::Symbol;
use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

fn tile(symbol: Symbol) -> impl View {
    vstack!(
        icon(symbol)
            .font_size(19.0)
            .foreground_color(theme::fg())
            .foreground_hovered(theme::accent()),
        text(symbol.name).font(Font::Caption).foreground_color(theme::fg_faint()),
    )
    .spacing(6.0)
    .frame(96.0, 56.0)
    .background_hovered(theme::panel())
    .corner_radius(8.0)
    // rest on a tile: the bubble waits, shows, and can leave the window
    .tooltip(format!("Symbol::new({:?}, …)", symbol.name))
    .on_click(move || println!("{symbol:?}"))
}

#[cfg(target_os = "linux")]
fn main() {
    let rows: Vec<Vec<Symbol>> =
        symbol::ALL.chunks(4).map(|chunk| chunk.to_vec()).collect();

    bunny_ui_linux::run_window(
        "bunny_ui — icons",
        Size { width: 460.0, height: 560.0 },
        vstack!(
            text("sixteen glyphs the house draws").bold(),
            // the gallery: hover any tile for the accent ink
            for_each(
                rows,
                |row| row[0].name.to_string(),
                |row| {
                    for_each(row.clone(), |s| s.name.to_string(), |s| tile(*s))
                        .horizontal()
                        .spacing(8.0)
                },
            )
            .spacing(8.0),
            spacer().frame_height(6.0),
            text("the same glyph, sized by its font").bold(),
            hstack!(
                icon(symbol::SEARCH).font(Font::Caption),
                icon(symbol::SEARCH),
                icon(symbol::SEARCH).font(Font::Headline),
                icon(symbol::SEARCH).font_size(28.0),
                icon(symbol::SEARCH).resizable().frame(56.0, 56.0),
            )
            .spacing(14.0)
            .alignment(VerticalAlignment::Center),
            spacer().frame_height(6.0),
            text("beside text, it takes the ink").bold(),
            hstack!(
                icon(symbol::FOLDER).foreground_color(theme::accent()),
                text("Documents").foreground_color(theme::accent()),
                spacer().frame_width(18.0),
                icon(symbol::WARNING).foreground_color(Color::hex(0xD97706)),
                text("check the path").foreground_color(Color::hex(0xD97706)),
            )
            .spacing(6.0)
            .alignment(VerticalAlignment::Center),
        )
        .spacing(10.0)
        .padding_length(18.0),
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
