use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            text("Layout is the quiet part of an interface: when it works, nobody notices the twelve pixels between a title and its body.")
                .test_id("text1")
                .frame_width_aligned(240.0, Alignment::Leading)
                .test_id("box1"),
            text("Line height sets the rhythm of a paragraph, and one pixel of drift a line adds up fast.")
                .font_size(15.0)
                .line_height(24.0)
                .frame_width_aligned(240.0, Alignment::Leading)
                .test_id("box2"),
            text("Centered lines break the same way and sit in the middle.")
                .multiline_text_alignment(TextAlignment::Center)
                .test_id("text3")
                .frame_width(240.0)
                .test_id("box3"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(20.0),
    )
}
