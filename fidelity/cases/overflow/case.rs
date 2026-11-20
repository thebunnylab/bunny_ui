use crate::prelude::*;

/// Three fixed blocks, wider together than their 240 px track, placed
/// by the track's alignment: a fixed frame never gives up its width.
fn track(id: &str, alignment: Alignment) -> impl View<Arity = Single> + use<> {
    hstack!(
        block(100.0, 24.0, BLUE, &format!("{id}-a")),
        block(100.0, 24.0, GREEN, &format!("{id}-b")),
        block(100.0, 24.0, ORANGE, &format!("{id}-c")),
    )
    .spacing(8.0)
    .frame_width_aligned(240.0, alignment)
    .background_color(GROUND)
    .test_id(id)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            track("start", Alignment::Leading),
            track("middle", Alignment::Center),
            hstack!(
                block(160.0, 24.0, BLUE, "wide-a"),
                rectangle().foreground_color(GREEN).frame_height(24.0).test_id("squeezed"),
                block(100.0, 24.0, ORANGE, "wide-b"),
            )
            .spacing(8.0)
            .frame_width_aligned(240.0, Alignment::Leading)
            .background_color(GROUND)
            .test_id("squeeze"),
            vstack!(
                block(80.0, 30.0, BLUE, "tall-a"),
                block(80.0, 30.0, GREEN, "tall-b"),
                block(80.0, 30.0, ORANGE, "tall-c"),
            )
            .spacing(4.0)
            .frame_aligned(120.0, 60.0, Alignment::Top)
            .background_color(GROUND)
            .test_id("column"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(16.0)
        .padding_length(40.0),
    )
}
