use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            spacer()
                .frame(160.0, 80.0)
                .background_color(WHITE)
                .corner_radius(12.0)
                .shadow_color(12.0, Color::rgba(0, 0, 0, 64))
                .test_id("card"),
            spacer().frame(80.0, 80.0).background_color(BLUE).shadow_color(6.0, Color::rgba(52, 120, 246, 128)).test_id("square"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(32.0)
        .padding_length(24.0),
    )
}
