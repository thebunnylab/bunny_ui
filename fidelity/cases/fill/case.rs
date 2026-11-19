use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            text("Full width")
                .test_id("full-text")
                .padding_length(8.0)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                .background_color(TINT)
                .test_id("full"),
            text("Trailing")
                .test_id("end-text")
                .padding_length(8.0)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Trailing)
                .background_color(TINT)
                .test_id("end"),
            rectangle().foreground_color(BLUE).frame_height(20.0).frame_max(200.0, f64::MAX, Alignment::Leading).test_id("capped"),
            hstack!(
                rectangle().foreground_color(GREEN).frame_height(24.0).test_id("share-a"),
                rectangle().foreground_color(PURPLE).frame_height(24.0).frame_max(60.0, f64::MAX, Alignment::Leading).test_id("share-b"),
            )
            .spacing(8.0)
            .test_id("share"),
            block(80.0, 20.0, ORANGE, "short"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(10.0)
        .frame_width_aligned(300.0, Alignment::Leading)
        .background_color(GROUND)
        .test_id("column"),
    )
}
