//! Ten thousand rows of six columns in a virtual list: at rest, under the
//! wheel, or soaking.
use arena::{Args, FONT_FAMILY, Step, WINDOW, rows, scripted};
use bunny_ui::layout::{Color, Size};
use bunny_ui::prelude::*;

const ROW_H: f64 = 24.0;
/// A point over the rows, in layout points.
const OVER: (f64, f64) = (WINDOW.0 * 0.5, WINDOW.1 * 0.5);
const WIDTHS: [f64; 6] = [70.0, 300.0, 90.0, 140.0, 90.0, 110.0];

#[derive(Clone)]
struct Table {
    rows: std::rc::Rc<Vec<[String; 6]>>,
}

impl Component for Table {
    fn body(self, _ctx: &Context) -> impl View {
        let rows = self.rows.clone();
        let count = rows.len();
        vstack!(
            hstack!(
                text!("{count} rows").font_size(13.0).bold(),
                spacer(),
                text!("the arena · table")
                    .font_size(12.0)
                    .foreground_color(Color::OUTLINE),
            )
            .padding_length(10.0)
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
                        text!(c0).font_size(12.0).frame_width(WIDTHS[0]),
                        text!(c1).font_size(12.0).frame_width(WIDTHS[1]),
                        text!(c2).font_size(12.0).frame_width(WIDTHS[2]),
                        text!(c3).font_size(12.0).frame_width(WIDTHS[3]),
                        text!(c4).font_size(12.0).frame_width(WIDTHS[4]),
                        text!(c5).font_size(12.0).frame_width(WIDTHS[5]),
                    )
                    .spacing(8.0)
                    .padding_edge(Edge::Leading, 12.0)
                    .frame_height(ROW_H)
                    .background_color(shade)
                }
            )
            .row_height(ROW_H),
        )
        .spacing(0.0)
        .font_family(FONT_FAMILY)
    }
}

fn main() {
    let args = Args::parse();
    let table = Table {
        rows: std::rc::Rc::new(rows(args.rows)),
    };
    bunny_ui_macos::run_window(
        "arena — table",
        Size {
            width: WINDOW.0,
            height: WINDOW.1,
        },
        scripted(table, args, OVER, |_: Step| {}),
    );
}
