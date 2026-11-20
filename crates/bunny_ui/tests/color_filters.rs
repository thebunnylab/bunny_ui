use bunny_ui::angle::Angle;
use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const RUST: Color = Color::rgb(200, 100, 50);
const SIZE: Size = Size { width: 40.0, height: 40.0 };

/// A 20-point square of `color` in the middle of a 40-point scene.
fn square(color: Color) -> impl View<Arity = Single> {
    rectangle().foreground_color(color).frame(20.0, 20.0)
}

/// The scene's commands and the color at its centre.
fn painted(view: impl View<Arity = Single>) -> (Vec<DrawCommand>, [u8; 3]) {
    let runtime = Runtime::new();
    let display = runtime.display_frame(&view.padding_length(10.0), SIZE);
    let bitmap = bunny_ui::raster::rasterize(&display, 40, 40, Color::rgb(255, 255, 255));
    let [r, g, b, _] = bitmap.pixels()[20 * bitmap.width() + 20].to_be_bytes();
    (display.iter().cloned().collect(), [r, g, b])
}

fn assert_near(got: [u8; 3], want: [u8; 3], label: &str) {
    let off = got.iter().zip(want).map(|(got, want)| got.abs_diff(want)).max().unwrap_or(0);
    assert!(off <= 1, "{label}: {got:?}, the page paints {want:?}");
}

/// Each filter changes the colors as CSS Filter Effects 1 defines its
/// shorthand function, on the sRGB channels as Chrome runs them: a red
/// gone gray is 54, its luminance weight; in linear light it would be
/// 127.
#[test]
fn each_filter_changes_the_colors_as_the_page_does() {
    assert_near(painted(square(Color::rgb(255, 0, 0)).grayscale(1.0)).1, [54, 54, 54], "grayscale(1)");
    assert_near(painted(square(RUST).grayscale(1.0)).1, [118, 118, 118], "grayscale(1)");
    assert_near(painted(square(RUST).grayscale(0.5)).1, [159, 109, 84], "grayscale(0.5)");
    assert_near(painted(square(RUST).saturation(0.0)).1, [118, 118, 118], "saturate(0)");
    assert_near(painted(square(RUST).saturation(1.4)).1, [233, 93, 23], "saturate(1.4)");
    assert_near(painted(square(RUST).contrast(1.3)).1, [222, 92, 27], "contrast(1.3)");
    assert_near(painted(square(RUST).hue_rotation(Angle::degrees(180.0))).1, [35, 135, 185], "hue-rotate(180deg)");
    assert_near(painted(square(RUST).color_invert()).1, [55, 155, 205], "invert(1)");
    // a negative contrast inverts as it flattens, as in SwiftUI
    assert_near(painted(square(RUST).contrast(-1.0)).1, [55, 155, 205], "contrast(-1)");
}

/// Chained, the filters apply in the order they are written, each
/// clamped before the next: a contrast that pushes red past its top and
/// blue past its floor, then a saturation that grays them, is not the
/// gray pushed apart.
#[test]
fn chained_filters_apply_in_the_order_written() {
    assert_near(painted(square(RUST).contrast(3.0).saturation(0.0)).1, [86, 86, 86], "contrast(3) saturate(0)");
    assert_near(painted(square(RUST).saturation(0.0).contrast(3.0)).1, [98, 98, 98], "saturate(0) contrast(3)");
}

/// The filtered subtree is one picture, laid where it was: its ground
/// and its children are gone from the list, and its layout and its name
/// stay where they were. A filter that changes nothing paints the
/// subtree as it is.
#[test]
fn a_filtered_view_is_one_picture_and_keeps_its_place() {
    let laid = |amount: f64| {
        let view = vstack!(
            hstack!(square(RUST), text("x")).background_color(Color::rgb(0, 90, 0)).grayscale(amount).test_id("tile"),
            square(RUST).test_id("next"),
        );
        let runtime = Runtime::new();
        let display = runtime.display_frame(&view, Size { width: 120.0, height: 80.0 });
        (display.iter().cloned().collect::<Vec<_>>(), runtime.test_frames())
    };
    let (display, frames) = laid(1.0);
    let sprites = display.iter().filter(|command| matches!(command, DrawCommand::Sprite { .. })).count();
    let fills = display.iter().filter(|command| matches!(command, DrawCommand::FillRect { .. })).count();
    let lines = display.iter().filter(|command| matches!(command, DrawCommand::TextLine { .. })).count();
    assert_eq!((sprites, fills, lines), (1, 1, 0), "the tile is a picture, the next square a fill: {display:?}");
    assert_eq!(frames, laid(0.0).1, "the frames of the boxes stay where they were");
    let (commands, color) = painted(square(RUST).saturation(1.0).hue_rotation(Angle::ZERO));
    assert!(!commands.iter().any(|command| matches!(command, DrawCommand::Sprite { .. })), "{commands:?}");
    assert_eq!(color, [200, 100, 50]);
}

/// A filter changes what the view paints, not what is laid around it
/// later: a ground set outside it keeps its color, one set inside goes
/// gray with the rest.
#[test]
fn a_ground_set_outside_the_filter_keeps_its_color() {
    fn ground(view: &impl View) -> [u8; 3] {
        let runtime = Runtime::new();
        let display = runtime.display_frame(view, SIZE);
        let bitmap = bunny_ui::raster::rasterize(&display, 40, 40, Color::rgb(255, 255, 255));
        let [r, g, b, _] = bitmap.pixels()[2 * bitmap.width() + 2].to_be_bytes();
        [r, g, b]
    }
    let red = Color::rgb(255, 0, 0);
    let outside = ground(&square(RUST).padding_length(10.0).grayscale(1.0).background_color(red));
    let inside = ground(&square(RUST).padding_length(10.0).background_color(red).grayscale(1.0));
    assert_near(outside, [255, 0, 0], "a ground outside");
    assert_near(inside, [54, 54, 54], "a ground inside");
}

/// The page writes the filters as one CSS `filter` list, in order; a
/// negative contrast becomes an inversion and the contrast it keeps,
/// which CSS reads as one; a blur written before them stays the inner
/// element's.
#[test]
fn the_page_writes_the_filters_in_order() {
    let page = bunny_ui::ssr::render(
        &vstack!(
            text("gray").grayscale(1.0).hue_rotation(Angle::degrees(90.0)),
            text("loud").contrast(3.0).saturation(0.0),
            text("negative").contrast(-0.5),
            text("inverted").color_invert(),
            text("soft").blur(2.0).saturation(2.0),
        ),
        Size { width: 200.0, height: 200.0 },
    );
    for list in ["filter:grayscale(1) hue-rotate(90deg)", "filter:contrast(3) saturate(0)", "filter:invert(1) contrast(0.5)", "filter:invert(1)", "filter:blur(2px)", "filter:saturate(2)"] {
        assert!(page.css.contains(list), "{list}: {}", page.css);
    }
}
