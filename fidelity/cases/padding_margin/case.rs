use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            block(60.0, 20.0, BLUE, "pad-inner").padding_length(12.0).background_color(TINT).test_id("pad"),
            block(60.0, 20.0, GREEN, "edges-inner")
                .padding_edge(Edge::Leading, 24.0)
                .padding_edge(Edge::Top, 6.0)
                .padding_edge(Edge::Bottom, 2.0)
                .background_color(TINT)
                .test_id("edges"),
            // a margin is a padding the background does not reach
            block(60.0, 20.0, ORANGE, "margin-inner").padding_length(10.0).background_color(TINT).test_id("margin-box"),
            hstack!(block(30.0, 30.0, PURPLE, "row-a").padding_edge(Edge::Trailing, 20.0), block(30.0, 30.0, BLUE, "row-b"))
                .spacing(0.0)
                .test_id("row"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(16.0),
    )
}
