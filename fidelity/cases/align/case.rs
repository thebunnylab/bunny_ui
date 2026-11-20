use crate::prelude::*;

const TRACK: Color = GROUND;

/// Two blocks, grouped, placed in a 300 × 36 track by the track's own
/// alignment: `justify-content` is where the frame puts its child.
fn pair(id: &str, alignment: Alignment) -> impl View<Arity = Single> + use<> {
    hstack!(block(40.0, 20.0, BLUE, &format!("{id}-a")), block(60.0, 20.0, GREEN, &format!("{id}-b")))
        .spacing(8.0)
        .frame_aligned(300.0, 36.0, alignment)
        .background_color(TRACK)
        .test_id(id)
}

/// Three blocks of three heights, aligned across the row.
fn steps(id: &str, alignment: VerticalAlignment) -> impl View<Arity = Single> + use<> {
    hstack!(
        block(20.0, 12.0, BLUE, &format!("{id}-a")),
        block(20.0, 24.0, GREEN, &format!("{id}-b")),
        block(20.0, 36.0, ORANGE, &format!("{id}-c")),
    )
    .alignment(alignment)
    .spacing(8.0)
    .frame_aligned(300.0, 44.0, Alignment::Leading)
    .background_color(TRACK)
    .test_id(id)
}

/// Three bars of three widths, aligned across the column.
fn bars(id: &str, prefix: &str, alignment: HorizontalAlignment) -> impl View<Arity = Single> + use<> {
    vstack!(
        block(80.0, 10.0, BLUE, &format!("{prefix}-a")),
        block(40.0, 10.0, GREEN, &format!("{prefix}-b")),
        block(120.0, 10.0, ORANGE, &format!("{prefix}-c")),
    )
    .alignment(alignment)
    .spacing(4.0)
    .test_id(id)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        340.0,
        vstack!(
            pair("leading", Alignment::Leading),
            pair("center", Alignment::Center),
            pair("trailing", Alignment::Trailing),
            hstack!(block(40.0, 20.0, BLUE, "between-a"), spacer(), block(60.0, 20.0, GREEN, "between-b"))
                .spacing(8.0)
                .frame(300.0, 36.0)
                .background_color(TRACK)
                .test_id("between"),
            steps("top", VerticalAlignment::Top),
            steps("bottom", VerticalAlignment::Bottom),
            hstack!(bars("v-leading", "vl", HorizontalAlignment::Leading), bars("v-trailing", "vt", HorizontalAlignment::Trailing))
                .alignment(VerticalAlignment::Top)
                .spacing(24.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(10.0),
    )
}
