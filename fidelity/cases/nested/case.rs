use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        340.0,
        vstack!(
            hstack!(
                vstack!(
                    text("Inbox").font_size(18.0).font_weight(Weight::Semibold).test_id("title"),
                    text("3 unread").font_size(13.0).foreground_color(GRAY).test_id("subtitle"),
                )
                .alignment(HorizontalAlignment::Leading)
                .spacing(2.0)
                .test_id("heading"),
                spacer(),
                block(64.0, 28.0, BLUE, "action"),
            )
            .padding_length(12.0)
            .frame_width(360.0)
            .background_color(GROUND)
            .test_id("header"),
            hstack!(
                vstack!(block(60.0, 20.0, BLUE, "nav-a"), block(60.0, 20.0, GREEN, "nav-b"), block(60.0, 20.0, ORANGE, "nav-c"))
                    .spacing(6.0)
                    .padding_length(10.0)
                    .background_color(TINT)
                    .test_id("sidebar"),
                vstack!(block(200.0, 40.0, PURPLE, "hero"), block(140.0, 20.0, GRAY, "line-a"), block(170.0, 20.0, LIGHT, "line-b"))
                    .alignment(HorizontalAlignment::Leading)
                    .spacing(8.0)
                    .test_id("main"),
            )
            .alignment(VerticalAlignment::Top)
            .spacing(12.0)
            .test_id("split"),
            vstack!(block(120.0, 20.0, BLUE, "c-wide"), block(60.0, 20.0, GREEN, "c-narrow"), text("Centered caption").test_id("c-caption"))
                .spacing(6.0)
                .frame_width(300.0)
                .background_color(GROUND)
                .test_id("centered"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(16.0)
        .padding_length(16.0),
    )
}
