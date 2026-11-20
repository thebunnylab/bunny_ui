use crate::prelude::*;

/// An empty box of a fixed size, for the paint around it alone.
fn plate(width: f64, height: f64) -> impl View<Arity = Single> {
    spacer().frame(width, height)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            hstack!(
                plate(120.0, 48.0).background_color(TINT).corner_radius(8.0).border(GRAY, 1.0).test_id("r8"),
                plate(120.0, 40.0).background_color(BLUE).corner_radius(20.0).test_id("pill"),
                plate(64.0, 64.0).background_color(ORANGE).corner_radius(32.0).border(INK, 2.0).test_id("circle"),
            )
            .alignment(VerticalAlignment::Top)
            .spacing(12.0),
            hstack!(
                plate(120.0, 48.0).border(INK, 2.0).test_id("square"),
                plate(100.0, 48.0).background_color(TINT).corner_radius(12.0).border(BLUE, 3.0).test_id("r12"),
                plate(80.0, 48.0).background_color(GREEN).corner_radius(6.0).test_id("r6"),
            )
            .alignment(VerticalAlignment::Top)
            .spacing(12.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(12.0),
    )
}
