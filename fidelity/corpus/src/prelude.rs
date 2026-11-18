//! What every case shares with its page: the face, the inks, and the
//! page the content stands on at the top-left corner, as a body with no
//! margin holds it.

pub use bunny_ui::prelude::*;

/// The corpus's one face (`fidelity/fonts/fonts.css`).
pub const FACE: &str = "Fid Sans";

/// The body's ink: `color: #1d1d1f`.
pub const INK: Color = Color::rgb(0x1d, 0x1d, 0x1f);
pub const WHITE: Color = Color::rgb(0xff, 0xff, 0xff);
pub const BLUE: Color = Color::rgb(0x34, 0x78, 0xf6);
pub const GREEN: Color = Color::rgb(0x34, 0xc7, 0x59);
pub const ORANGE: Color = Color::rgb(0xff, 0x95, 0x00);
pub const PURPLE: Color = Color::rgb(0xaf, 0x52, 0xde);
pub const GRAY: Color = Color::rgb(0x8e, 0x8e, 0x93);
pub const LIGHT: Color = Color::rgb(0xc7, 0xc7, 0xcc);
pub const TINT: Color = Color::rgb(0xe8, 0xf0, 0xfe);
pub const GROUND: Color = Color::rgb(0xf2, 0xf2, 0xf7);

/// The page: `width × height`, white, the content at its top-left
/// corner, in the corpus's face at 16 px — the `body` and `.page` rules
/// every `case.html` starts with.
pub fn page(width: f64, height: f64, content: impl View<Arity = Single>) -> impl View {
    page_on(WHITE, width, height, content)
}

/// [`page`] on another ground.
pub fn page_on(ground: Color, width: f64, height: f64, content: impl View<Arity = Single>) -> impl View {
    content
        .frame_aligned(width, height, Alignment::TopLeading)
        .background_color(ground)
        .test_id("page")
        .font_family(FACE)
        .font_size(16.0)
        .foreground_color(INK)
}

/// A filled box of a fixed size: a `div` with a width, a height and a
/// background.
#[must_use]
pub fn block(width: f64, height: f64, color: Color, id: &str) -> impl View<Arity = Single> + use<> {
    rectangle().foreground_color(color).frame(width, height).test_id(id)
}
