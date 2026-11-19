use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            hstack!(
                block(40.0, 24.0, BLUE, "even-a"),
                spacer(),
                block(40.0, 24.0, GREEN, "even-b"),
                spacer(),
                block(40.0, 24.0, ORANGE, "even-c"),
            )
            .frame(300.0, 24.0)
            .background_color(GROUND)
            .test_id("even"),
            vstack!(block(80.0, 20.0, BLUE, "push-top"), spacer(), block(80.0, 20.0, GREEN, "push-bottom"))
                .frame(120.0, 120.0)
                .background_color(GROUND)
                .test_id("push"),
            hstack!(spacer(), text("Total").test_id("total-label"), block(48.0, 20.0, PURPLE, "total-value"))
                .spacing(8.0)
                .frame_width(300.0)
                .background_color(GROUND)
                .test_id("trailing"),
            hstack!(text("Left").test_id("left"), spacer(), text("Right").test_id("right"))
                .padding_length(8.0)
                .frame_width(300.0)
                .background_color(TINT)
                .test_id("ends"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(16.0)
        .padding_length(16.0),
    )
}
