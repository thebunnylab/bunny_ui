use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        340.0,
        vstack!(
            zstack!(block(160.0, 100.0, BLUE, "back"), block(40.0, 40.0, ORANGE, "front"))
                .alignment(Alignment::BottomTrailing)
                .test_id("stack"),
            zstack!(block(160.0, 60.0, GREEN, "plate"), text("Centered").test_id("plate-text")).test_id("plate-stack"),
            block(120.0, 60.0, TINT, "card").overlay(UnitPoint::TOP_TRAILING, block(16.0, 16.0, PURPLE, "badge")),
            block(120.0, 40.0, LIGHT, "tab")
                .overlay(UnitPoint::BOTTOM, spacer().frame_height(3.0).background_color(BLUE).test_id("rule")),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(16.0)
        .padding_length(24.0),
    )
}
