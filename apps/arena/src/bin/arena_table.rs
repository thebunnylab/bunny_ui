//! Ten thousand rows of six columns in a virtual list: at rest, under the
//! wheel, or soaking.
use arena::{Args, FONT_FAMILY, WINDOW, rows};
use bunny_ui::layout::Color;
use bunny_ui::prelude::*;

const ROW_H: f64 = 24.0;
const WIDTHS: [f64; 6] = [70.0, 300.0, 90.0, 140.0, 90.0, 110.0];

#[derive(Clone)]
struct Table {
    rows: std::rc::Rc<Vec<[String; 6]>>,
}

impl Component for Table {
    fn body(self) -> impl View {
        let rows = self.rows.clone();
        let count = rows.len();
        vstack!(
            hstack!(text!("{count} rows").font_size(13.0), spacer(),)
                .padding_edge(Edge::Leading, 12.0)
                .frame_height(40.0),
            virtual_list(
                count,
                |i| format!("row-{i}"),
                move |i| {
                    let [c0, c1, c2, c3, c4, c5] = rows[i].clone();
                    let shade = if i % 2 == 0 {
                        Color::hex(0x1C1C21)
                    } else {
                        Color::hex(0x17171C)
                    };
                    hstack!(
                        text!(c0)
                            .font_size(12.0)
                            .frame_width_aligned(WIDTHS[0], Alignment::Leading),
                        text!(c1)
                            .font_size(12.0)
                            .frame_width_aligned(WIDTHS[1], Alignment::Leading),
                        text!(c2)
                            .font_size(12.0)
                            .frame_width_aligned(WIDTHS[2], Alignment::Leading),
                        text!(c3)
                            .font_size(12.0)
                            .frame_width_aligned(WIDTHS[3], Alignment::Leading),
                        text!(c4)
                            .font_size(12.0)
                            .frame_width_aligned(WIDTHS[4], Alignment::Leading),
                        text!(c5)
                            .font_size(12.0)
                            .frame_width_aligned(WIDTHS[5], Alignment::Leading),
                    )
                    .spacing(8.0)
                    .padding_edge(Edge::Leading, 12.0)
                    .frame_aligned(WINDOW.0, ROW_H, Alignment::Leading)
                    .background_color(shade)
                }
            )
            .row_height(ROW_H),
        )
        .spacing(0.0)
        .font_family(FONT_FAMILY)
        .foreground_color(Color::hex(0xE6E6EA))
        .background_color(Color::hex(0x17171C))
    }
}

fn main() {
    let args = Args::parse();
    let table = Table {
        rows: std::rc::Rc::new(rows(args.rows)),
    };
    let count = table.rows.len();
    arena::scene::run(
        "arena — table",
        "table",
        table,
        args,
        |_| {},
        move || serde_json::json!({"item_count": count}),
    );
}
