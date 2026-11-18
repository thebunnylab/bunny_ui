use crate::prelude::*;

/// The page's `/assets/pattern.png`, the same bytes.
fn pattern() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/pattern.png").as_slice())
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            hstack!(
                image(pattern()).resizable().frame(64.0, 48.0).test_id("down"),
                image(pattern()).resizable().aspect_ratio(ContentMode::Fit).frame(96.0, 96.0).test_id("fit"),
                image(pattern()).resizable().aspect_ratio(ContentMode::Fill).frame(96.0, 96.0).clipped().test_id("fill"),
            )
            .alignment(VerticalAlignment::Top)
            .spacing(12.0),
            image(pattern()).resizable().frame(200.0, 150.0).test_id("up"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(12.0),
    )
}
