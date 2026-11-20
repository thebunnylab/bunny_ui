use bunny_ui::layout::{DrawCommand, Gradient};
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const RED: Color = Color::rgb(255, 0, 0);
const BLUE: Color = Color::rgb(0, 0, 255);

fn ink(view: &impl View, size: Size) -> (Vec<DrawCommand>, bunny_ui::raster::Bitmap) {
    let runtime = Runtime::new();
    let display = runtime.display_frame(view, size);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bitmap = bunny_ui::raster::rasterize(&display, size.width as usize, size.height as usize, Color::rgb(255, 255, 255));
    (display.iter().cloned().collect(), bitmap)
}

/// The inked pixels of a row band: how red and how blue they lean.
fn leaning(bitmap: &bunny_ui::raster::Bitmap, columns: std::ops::Range<usize>) -> (u32, u32) {
    let (mut red, mut blue) = (0, 0);
    for y in 0..bitmap.height() {
        for x in columns.clone() {
            let [r, g, b, _] = bitmap.pixels()[y * bitmap.width() + x].to_be_bytes();
            if g < 200 {
                red += u32::from(u16::from(r) > u16::from(b) + 40);
                blue += u32::from(u16::from(b) > u16::from(r) + 40);
            }
        }
    }
    (red, blue)
}

/// `.foreground_gradient(…)` is `SwiftUI`'s `.foregroundStyle(gradient)`:
/// the letters cut one ramp out of the box — red where it starts, blue
/// where it ends — and no line keeps a flat color.
#[test]
fn the_letters_cut_the_ramp_out_of_the_box() {
    let view = text("WWWWWWWWWWWW")
        .font_size(24.0)
        .foreground_gradient(Gradient::linear(RED, BLUE).direction(UnitPoint::LEADING, UnitPoint::TRAILING))
        .frame(240.0, 40.0);
    let (commands, bitmap) = ink(&view, Size { width: 240.0, height: 40.0 });
    assert!(!commands.iter().any(|command| matches!(command, DrawCommand::TextLine { .. })), "{commands:?}");
    let Some(DrawCommand::Sprite { rect, .. }) = commands.iter().find(|command| matches!(command, DrawCommand::Sprite { .. })) else {
        panic!("the line is a picture: {commands:?}")
    };
    // the ramp runs across the 240-point box; the line stands in its middle
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (left, right) = (rect.origin.x as usize, (rect.origin.x + rect.size.width) as usize);
    let third = (right - left) / 3;
    let (left_red, left_blue) = leaning(&bitmap, left..left + third);
    let (right_red, right_blue) = leaning(&bitmap, right - third..right);
    assert!(left_red > 4 * left_blue && left_red > 20, "red leaning at the start: {left_red} {left_blue}");
    assert!(right_blue > 4 * right_red && right_blue > 20, "blue leaning at the end: {right_red} {right_blue}");
}

/// A color set further in wins over the ramp, as in `SwiftUI`.
#[test]
fn a_color_set_inside_wins() {
    let view = vstack!(text("inked"), text("plain").foreground_color(Color::rgb(0, 160, 0)))
        .foreground_gradient(Gradient::linear(RED, BLUE));
    let (commands, _) = ink(&view, Size { width: 200.0, height: 80.0 });
    let lines: Vec<_> = commands.iter().filter(|command| matches!(command, DrawCommand::TextLine { .. })).collect();
    assert_eq!(lines.len(), 1, "only the green line stays a line: {commands:?}");
    assert!(matches!(lines[0], DrawCommand::TextLine { color, .. } if color.g == 160));
}


/// The page cuts the ramp out of the box with the text — the top
/// background layer, clipped to the text, and the text left with no fill
/// of its own — and a box inside that sets its own ink paints its text
/// in it again. The box's own ground keeps its box.
#[test]
fn the_page_clips_the_ramp_to_the_text() {
    let page = bunny_ui::ssr::render(
        &vstack!(text("inked"), text("plain").foreground_color(Color::rgb(0, 160, 0)))
            .foreground_gradient(Gradient::linear(RED, BLUE))
            .background_color(Color::rgb(250, 250, 250)),
        Size { width: 200.0, height: 80.0 },
    );
    assert!(
        page.css.contains("background-image:linear-gradient(180.00deg, rgba(255, 0, 0, 1), rgba(0, 0, 255, 1)), none"),
        "{}",
        page.css
    );
    assert!(page.css.contains("background-clip:text, border-box"), "{}", page.css);
    assert!(page.css.contains("-webkit-text-fill-color:transparent"), "{}", page.css);
    assert!(page.css.contains("-webkit-text-fill-color:currentcolor"), "the green line: {}", page.css);
}
