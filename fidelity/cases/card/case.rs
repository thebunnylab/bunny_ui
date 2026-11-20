use crate::prelude::*;

fn pattern() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/pattern.png").as_slice())
}

fn action(label: &str, filled: bool, id: &str) -> impl View<Arity = Single> + use<> {
    let (ink, ground) = if filled { (WHITE, BLUE) } else { (BLUE, WHITE) };
    text(label.to_owned())
        .font_size(15.0)
        .foreground_color(ink)
        .test_id(format!("{id}-label"))
        .frame(88.0, 36.0)
        .background_color(ground)
        .corner_radius(8.0)
        .border(BLUE, if filled { 0.0 } else { 1.0 })
        .test_id(id)
}

#[must_use]
pub fn view() -> impl View {
    page_on(
        GROUND,
        400.0,
        380.0,
        vstack!(
            image(pattern()).resizable().aspect_ratio(ContentMode::Fill).frame(320.0, 140.0).clipped().test_id("card-image"),
            vstack!(
                text("Weekend in the hills").font_size(20.0).font_weight(Weight::Semibold).test_id("card-title"),
                text("Two days, three trails, and a cabin with a view of the valley. Pack light; the weather turns fast.")
                    .font_size(14.0)
                    .foreground_color(GRAY)
                    .frame_width_aligned(288.0, Alignment::Leading)
                    .test_id("card-text"),
                hstack!(action("Share", false, "share"), action("Open", true, "open")).spacing(8.0),
            )
            .alignment(HorizontalAlignment::Leading)
            .spacing(8.0)
            .padding_length(16.0)
            .test_id("card-body"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(0.0)
        .frame_width(320.0)
        .background_color(WHITE)
        .corner_radius(16.0)
        .clipped()
        .shadow_color(16.0, Color::rgba(0, 0, 0, 46))
        .test_id("card")
        .padding_length(24.0),
    )
}
