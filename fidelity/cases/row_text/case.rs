use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            hstack!(text("Wi-Fi").test_id("setting-name"), spacer(), text("Home").foreground_color(GRAY).test_id("setting-value"))
                .padding_length(12.0)
                .frame_width(320.0)
                .background_color(GROUND)
                .test_id("setting"),
            hstack!(
                block(40.0, 40.0, BLUE, "avatar"),
                vstack!(
                    text("Ana Souza").font_weight(Weight::Semibold).test_id("name"),
                    text("The cabin is booked for Saturday; bring boots, the trail is still wet.")
                        .font_size(14.0)
                        .foreground_color(GRAY)
                        .test_id("text"),
                )
                .alignment(HorizontalAlignment::Leading)
                .spacing(2.0)
                .test_id("body"),
            )
            .alignment(VerticalAlignment::Top)
            .spacing(12.0)
            .padding_length(12.0)
            .frame_width_aligned(320.0, Alignment::Leading)
            .background_color(GROUND)
            .test_id("message"),
            hstack!(
                text("A very long title that will not fit").truncation_mode(Truncation::End).test_id("clipped"),
                block(40.0, 20.0, ORANGE, "tag"),
            )
            .spacing(8.0)
            .frame_width_aligned(220.0, Alignment::Leading)
            .background_color(TINT)
            .test_id("titled"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(12.0)
        .padding_length(16.0),
    )
}
