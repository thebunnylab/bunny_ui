use crate::prelude::*;

/// A block that takes what the row leaves: `flex: 1 1 0`.
fn grower(color: Color, id: &str) -> impl View<Arity = Single> + use<> {
    rectangle().foreground_color(color).frame_height(32.0).test_id(id)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            hstack!(block(60.0, 32.0, BLUE, "fixed-a"), grower(GREEN, "grow"), block(40.0, 32.0, ORANGE, "fixed-b"))
                .spacing(0.0)
                .frame_width(300.0)
                .test_id("row1"),
            hstack!(grower(GREEN, "half-a"), grower(PURPLE, "half-b")).spacing(10.0).frame_width(300.0).test_id("row2"),
            hstack!(text("Shrinking labels wrap").test_id("shrink-a"), text("when the row is narrow").test_id("shrink-b"))
                .alignment(VerticalAlignment::Top)
                .spacing(8.0)
                .frame_width_aligned(200.0, Alignment::Leading)
                .test_id("row3"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(12.0),
    )
}
