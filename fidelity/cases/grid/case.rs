use crate::prelude::*;

fn cell(color: Color, id: &str) -> impl View<Arity = Single> + use<> {
    rectangle().foreground_color(color).frame_height(40.0).test_id(id)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        grid!(
            cell(BLUE, "g1"),
            cell(GREEN, "g2"),
            cell(ORANGE, "g3"),
            cell(PURPLE, "g4").span(2, 1),
            cell(BLUE, "g5"),
            cell(GREEN, "g6"),
            cell(ORANGE, "g7"),
        )
        .columns(3)
        .spacing(8.0)
        .row_spacing(12.0)
        .frame_width(300.0)
        .test_id("grid"),
    )
}
