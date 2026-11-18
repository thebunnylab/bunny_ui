use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            hstack!(block(48.0, 24.0, BLUE, "row-a"), block(32.0, 24.0, GREEN, "row-b"), block(64.0, 24.0, ORANGE, "row-c"))
                .spacing(12.0)
                .test_id("row"),
            block(120.0, 40.0, PURPLE, "wide"),
            hstack!(block(20.0, 20.0, BLUE, "tight-a"), block(20.0, 40.0, GREEN, "tight-b"), block(20.0, 30.0, ORANGE, "tight-c"))
                .spacing(4.0)
                .test_id("tight"),
        )
        .spacing(8.0)
        .test_id("column"),
    )
}
