use crate::prelude::*;

fn chip(width: f64, height: f64, color: Color, id: &str) -> impl View<Arity = Single> + use<> {
    spacer().frame(width, height).background_color(color).corner_radius(14.0).test_id(id)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        hstack!(
            chip(60.0, 28.0, BLUE, "c1"),
            chip(90.0, 28.0, GREEN, "c2"),
            chip(40.0, 36.0, ORANGE, "c3"),
            chip(120.0, 28.0, PURPLE, "c4"),
            chip(70.0, 28.0, BLUE, "c5"),
            chip(50.0, 28.0, GREEN, "c6"),
            chip(100.0, 28.0, ORANGE, "c7"),
        )
        .wrapping()
        .spacing(8.0)
        .line_spacing(8.0)
        .frame_width_aligned(240.0, Alignment::Leading)
        .test_id("wrap"),
    )
}
