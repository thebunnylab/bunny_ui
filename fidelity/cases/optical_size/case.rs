use crate::prelude::*;

/// hop's face (`fidelity/fonts/fonts.css`): a variable face whose
/// optical-size axis follows the size it is set at. Set at the device's
/// size, its 14 px is the display cut, 7% narrower than the page's.
const INTER: &str = "Inter";

fn line(size: f64, height: f64, words: &'static str, id: &str) -> impl View<Arity = Single> + use<> {
    text(words).font_size(size).line_height(height).test_id(id)
}

#[must_use]
pub fn view() -> impl View {
    page(
        400.0,
        240.0,
        vstack!(
            line(12.0, 16.0, "Compiled to WebAssembly, running in your tab.", "s12"),
            line(14.0, 20.0, "Compiled to WebAssembly, in your tab.", "s14"),
            line(16.0, 22.0, "Compiled to WebAssembly.", "s16"),
            line(16.0, 22.0, "Weight and size together.", "s16-bold").font_weight(Weight::Semibold),
            line(20.0, 26.0, "Running in your tab.", "s20"),
            line(28.0, 34.0, "Optical sizes", "s28"),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(8.0)
        .padding_length(16.0)
        .font_family(INTER)
        .test_id("column"),
    )
}
