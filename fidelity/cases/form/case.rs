use crate::prelude::*;

/// A label over a field: the words 12 px from the field's edge, the
/// stroke drawn over the box.
fn group(label: &str, placeholder: &str, id: &str) -> impl View<Arity = Single> + use<> {
    vstack!(
        text(label.to_owned()).font_size(13.0).foreground_color(GRAY).test_id(format!("{id}-label")),
        text(placeholder.to_owned())
            .foreground_color(GRAY)
            .test_id(format!("{id}-placeholder"))
            .padding_edge(Edge::Leading, 12.0)
            .frame_aligned(300.0, 40.0, Alignment::Leading)
            .background_color(WHITE)
            .corner_radius(8.0)
            .border(LIGHT, 1.0)
            .test_id(format!("{id}-field")),
    )
    .alignment(HorizontalAlignment::Leading)
    .spacing(6.0)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        360.0,
        vstack!(
            text("Sign in").font_size(24.0).font_weight(Weight::Bold).test_id("title"),
            group("Email", "you@example.com", "email"),
            group("Password", "••••••••", "password"),
            text("Continue")
                .foreground_color(WHITE)
                .font_weight(Weight::Semibold)
                .test_id("submit-label")
                .frame(300.0, 44.0)
                .background_color(BLUE)
                .corner_radius(10.0)
                .test_id("submit"),
            text("Forgot password?").font_size(13.0).foreground_color(BLUE).test_id("forgot"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(16.0)
        .padding_length(24.0)
        .test_id("form"),
    )
}
