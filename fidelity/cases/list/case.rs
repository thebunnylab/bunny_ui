use crate::prelude::*;

const SEPARATOR: Color = Color::rgb(0xe5, 0xe5, 0xea);

fn row(icon: Color, title: &str, subtitle: &str, id: &str) -> impl View<Arity = Single> + use<> {
    hstack!(
        spacer().frame(28.0, 28.0).background_color(icon).corner_radius(7.0).test_id(format!("{id}-icon")),
        vstack!(
            text(title.to_owned()).test_id(format!("{id}-title")),
            text(subtitle.to_owned()).font_size(13.0).foreground_color(GRAY).test_id(format!("{id}-subtitle")),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(2.0),
        spacer(),
        text("›").font_size(20.0).foreground_color(LIGHT).test_id(format!("{id}-chevron")),
    )
    .spacing(12.0)
    .padding_edge(Edge::Leading, 16.0)
    .padding_edge(Edge::Trailing, 16.0)
    .padding_edge(Edge::Top, 10.0)
    .padding_edge(Edge::Bottom, 10.0)
    .test_id(id)
}

fn separator(id: &str) -> impl View<Arity = Single> + use<> {
    rectangle().foreground_color(SEPARATOR).frame_height(1.0).test_id(id).padding_edge(Edge::Leading, 56.0)
}

#[must_use]
pub fn view() -> impl View {
    page_on(
        GROUND,
        400.0,
        320.0,
        vstack!(
            row(BLUE, "Wi-Fi", "Home network", "wifi"),
            separator("separator-1"),
            row(GREEN, "Battery", "82% — about 9 hours left", "battery"),
            separator("separator-2"),
            row(ORANGE, "Notifications", "Banners, sounds, badges", "alerts"),
        )
        .spacing(0.0)
        .frame_width(368.0)
        .background_color(WHITE)
        .corner_radius(12.0)
        .test_id("list")
        .padding_length(16.0),
    )
}
