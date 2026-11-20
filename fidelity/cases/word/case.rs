use crate::prelude::*;

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        300.0,
        vstack!(
            text("Typography").test_id("w16"),
            text("Hamburgefonstiv").font_size(24.0).test_id("w24"),
            text("AVATAR Wave").font_size(20.0).test_id("kern"),
            text("1,234.56 office").test_id("figures"),
            text("Word").font_size(13.0).test_id("w13"),
            text("Bold?").font_size(32.0).font_weight(Weight::Bold).test_id("w32"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(6.0),
    )
}
