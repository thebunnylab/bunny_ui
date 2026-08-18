//! The table: ten thousand rows, four columns, both axes on the wheel
//! — the header stays put vertically and slides with its columns.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example table_window
//! ```

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

const KINDS: [&str; 4] = ["Rust", "TOML", "JSON", "Markdown"];

#[derive(Clone, Copy)]
struct Sheet;

impl Component for Sheet {
    fn body(self, _ctx: &Context) -> impl View {
        vstack!(
            text("ten thousand rows, a screenful materialized").bold(),
            table(
                vec![
                    column("Name", 240.0),
                    column("Kind", 110.0),
                    column("Size", 90.0),
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
    bunny_ui_macos::run_window("bunny_ui — table", Size { width: 560.0, height: 480.0 }, Sheet);
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
