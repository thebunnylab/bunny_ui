//! The table: ten thousand rows, four columns, both axes on the wheel
//! — the header stays put vertically and slides with its columns — and
//! the ROW as the unit: a selection band, a hover wash, an accent bar
//! and one click, none of which a grid of cells can say.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example table_window
//! ```

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

const KINDS: [&str; 4] = ["Rust", "TOML", "JSON", "Markdown"];

#[derive(Clone, Copy)]
struct Sheet {
    /// The row the hand chose — the state a band paints itself from.
    picked: State<Option<usize>>,
}

impl Component for Sheet {
    fn body(self, _ctx: &Context) -> impl View {
        let picked = self.picked;
        vstack!(
            text("ten thousand rows, a screenful materialized").bold(),
            table(
                vec![
                    column("Name", 240.0),
                    column("Kind", 110.0),
                    column("Size", 90.0).aligned(Alignment::Trailing),
                    column("Path", 320.0),
                ],
                10_000,
                |row| row.to_string(),
                |row, col| match col {
                    0 => erased(text(format!("file_{row:04}.rs"))),
                    1 => erased(
                        text(KINDS[row % KINDS.len()]).foreground_color(theme::fg_secondary()),
                    ),
                    2 => erased(
                        text(format!("{} KB", (row * 7) % 900 + 1))
                            .foreground_color(theme::fg_secondary()),
                    ),
                    _ => erased(
                        text(format!("src/mod_{:02}/deep/nested/dir", row % 40))
                            .monospaced()
                            .foreground_color(theme::fg_faint()),
                    ),
                },
            )
            .row_height(28.0)
            .header_height(24.0)
            .row_divider(theme::border())
            // the whole reason a table is not a list of cells: ONE ink
            // over the ONE band, and one click for the row it belongs to
            .row(move |row, band| {
                let chosen = picked.get() == Some(row);
                zstack!(
                    chosen.then(|| hstack!(
                        spacer().frame_width(2.0).background_color(theme::accent()),
                        spacer(),
                    )),
                    band,
                )
                .background_color(if chosen { theme::selection() } else { theme::selection().fade() })
                .background_hovered(theme::panel())
                .on_click(move || picked.set(Some(row)))
            })
            .header(|strip| {
                strip
                    .font_size(10.0)
                    .bold()
                    .foreground_color(theme::fg_secondary())
                    .background_color(theme::panel())
                    .border(theme::border(), 1.0)
            })
            .frame(520.0, 420.0)
            .background_color(theme::panel())
            .border(theme::border(), 1.0)
            .corner_radius(8.0)
            .clipped(),
        )
        .spacing(10.0)
        .padding_length(16.0)
    }
}

#[cfg(target_os = "macos")]
fn main() {
    bunny_ui_macos::run_window(
        "bunny_ui — table",
        Size { width: 560.0, height: 480.0 },
        Sheet { picked: State::new(None) },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
