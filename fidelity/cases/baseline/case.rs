use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            hstack!(
                text("Big").font_size(32.0).test_id("big"),
                text("medium").font_size(18.0).test_id("medium"),
                text("small").font_size(12.0).test_id("small"),
                block(24.0, 24.0, BLUE, "box"),
            )
            .alignment(VerticalAlignment::Baseline)
            .spacing(8.0)
            .background_color(GROUND)
            .test_id("row"),
            hstack!(
                text("Label").font_size(24.0).test_id("label"),
                vstack!(text("first line").test_id("first"), text("second line").font_size(12.0).test_id("second"))
                    .alignment(HorizontalAlignment::Leading)
                    .test_id("pair"),
            )
            .alignment(VerticalAlignment::Baseline)
            .spacing(12.0)
            .background_color(GROUND)
            .test_id("lines"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(24.0)
        .padding_length(24.0),
    )
}
