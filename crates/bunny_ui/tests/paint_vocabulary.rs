//! The paint a design system reaches for beyond fills and rings: a
//! dashed or dotted border, a striped ground, a glyph turned at any
//! angle, and a line of text turned a quarter. Each is asked of a view
//! the way an app asks it, and read back from the list the runtime
//! paints and the pixels the CPU compositor makes of it.

#![cfg(feature = "canvas")]

use bunny_ui_core as bunny_ui;

use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;

const INK: Color = Color { r: 20, g: 40, b: 200, a: 255 };
const GROUND: Color = Color { r: 255, g: 255, b: 255, a: 255 };

fn alpha_at(bitmap: &bunny_ui::raster::Bitmap, x: usize, y: usize) -> u8 {
    // the ground is white and the ink blue: how far a pixel left the
    // white is how much ink it took
    let pixel = bitmap.pixels()[y * bitmap.width() + x].to_be_bytes();
    255 - pixel[0]
}

#[test]
fn a_dashed_border_paints_dashes_and_air_and_a_solid_one_keeps_its_ring() {
    let runtime = Runtime::new();
    let dashed = rectangle()
        .foreground_color(Color::rgba(0, 0, 0, 0))
        .frame(40.0, 20.0)
        .border(INK, 1.0)
        .border_style(BorderStyle::Dashed)
        .padding_length(10.0);
    let display = runtime.display_frame(&dashed, Size { width: 60.0, height: 40.0 });
    assert!(
        !display.iter().any(|command| matches!(command, DrawCommand::StrokeRect { .. })),
        "a broken line is no ring"
    );
    assert!(
        display.iter().any(|command| matches!(
            command,
            DrawCommand::Image { source: ImageSource::Path { .. }, .. }
        )),
        "it is a traced path"
    );
    let bitmap = bunny_ui::raster::rasterize(&display, 60, 40, GROUND);
    // the top edge, one pen high, from the corner: a dash of three at a
    // thin pen, then air
    assert!(alpha_at(&bitmap, 10, 10) > 200, "the corner carries ink");
    assert!(alpha_at(&bitmap, 11, 10) > 200, "inside the first dash");
    assert!(alpha_at(&bitmap, 14, 10) < 30, "the air after it");
    // the same box, solid: the ring every pipeline draws
    let solid = rectangle()
        .foreground_color(Color::rgba(0, 0, 0, 0))
        .frame(40.0, 20.0)
        .border(INK, 1.0)
        .border_style(BorderStyle::Solid)
        .padding_length(10.0);
    let display = runtime.display_frame(&solid, Size { width: 60.0, height: 40.0 });
    assert!(display.iter().any(|command| matches!(command, DrawCommand::StrokeRect { .. })));
}

#[test]
fn stripes_lay_their_bands_where_the_browser_lays_them() {
    let runtime = Runtime::new();
    // `repeating-linear-gradient(90deg, ink 0 2px, transparent 2px 4px)`:
    // the bands repeat to the right, from the left edge
    let view = rectangle()
        .foreground_color(Color::rgba(0, 0, 0, 0))
        .frame(16.0, 8.0)
        .background_stripes(Stripes::new(Angle::degrees(90.0), 2.0, 4.0, INK));
    let display = runtime.display_frame(&view, Size { width: 16.0, height: 8.0 });
    let bitmap = bunny_ui::raster::rasterize(&display, 16, 8, GROUND);
    for (x, inked) in [(0, true), (1, true), (2, false), (3, false), (4, true), (13, true), (14, false)] {
        assert_eq!(alpha_at(&bitmap, x, 4) > 200, inked, "column {x}");
        if !inked {
            assert!(alpha_at(&bitmap, x, 4) < 10, "column {x} is air");
        }
    }
}

#[test]
fn a_turned_icon_is_its_own_tile_and_an_upright_one_keeps_its_identity() {
    let runtime = Runtime::new();
    let key_of = |view: &dyn Fn() -> Icon| {
        let display = runtime.display_frame(&view().frame(16.0, 16.0), Size { width: 16.0, height: 16.0 });
        display
            .iter()
            .find_map(|command| match command {
                DrawCommand::Image { source: source @ ImageSource::Symbol { .. }, .. } => Some(source.key()),
                _ => None,
            })
            .expect("the icon paints a glyph")
    };
    let upright = key_of(&|| icon(symbol::CHEVRON_RIGHT));
    let zero = key_of(&|| icon(symbol::CHEVRON_RIGHT).rotation(Angle::ZERO));
    let open = key_of(&|| icon(symbol::CHEVRON_RIGHT).rotation(Angle::degrees(90.0)));
    assert_eq!(upright, zero, "no turn is the glyph it always was");
    assert_ne!(upright, open, "a turned glyph is its own tile");
    assert_eq!(
        ImageSource::symbol(symbol::CHEVRON_RIGHT, INK).key(),
        ImageSource::symbol_turned(symbol::CHEVRON_RIGHT, INK, false, Angle::ZERO).key(),
    );
}

#[test]
fn a_quarter_turn_swaps_the_box_and_turns_the_lines_in_it() {
    let runtime = Runtime::new();
    let turned = text("Maturity").quarter_turn(QuarterTurn::Counterclockwise);
    let size = Size { width: 300.0, height: 300.0 };
    let display = runtime.display_frame(&hstack!(turned, text("x"), spacer()).alignment(VerticalAlignment::Top), size);
    let (pivot, turn) = display
        .iter()
        .find_map(|command| match command {
            DrawCommand::TurnedText { pivot, turn, content, range, .. } if &content[range.0..range.1] == "Maturity" => {
                Some((*pivot, *turn))
            }
            _ => None,
        })
        .expect("the turned line paints as a turned line");
    assert_eq!(turn, QuarterTurn::Counterclockwise);
    // the x beside it starts after the turned box, which is as wide as
    // the line was tall — far less than the line was wide
    let x = display
        .iter()
        .find_map(|command| match command {
            DrawCommand::TextLine { origin, content, range, .. } if &content[range.0..range.1] == "x" => Some(origin.x),
            _ => None,
        })
        .expect("the neighbour paints");
    assert!(x > 0.0 && x < 30.0, "the row made room for a box the line's height wide: {x}");
    // reading upward, the line starts at the bottom of its box, on a
    // baseline inside it
    assert!(pivot.x > 0.0 && pivot.x < x, "{pivot:?}");
    assert!(pivot.y > 30.0, "the start is at the bottom of a box the line's width tall: {pivot:?}");
}

#[test]
fn a_turned_line_composites_its_upright_raster_turned() {
    let font = FontSpec::DEFAULT;
    let pivot = Point { x: 12.0, y: 40.0 };
    let turned = bunny_ui::layout::DisplayList::from(vec![DrawCommand::TurnedText {
        pivot,
        content: "Ab".into(),
        range: (0, 2),
        color: INK,
        font,
        turn: QuarterTurn::Counterclockwise,
    }]);
    // the same bytes by hand: the engine's upright raster, turned, at the
    // corner the pivot names
    let upright = PixelFont.raster_line("Ab", &font, INK, 1).expect("the house font paints");
    let picture = bunny_ui::raster::turn_raster(&upright, QuarterTurn::Counterclockwise);
    assert_eq!((picture.width, picture.height), (upright.height, upright.width), "a quarter swaps the axes");
    let (x, y) = bunny_ui::raster::turned_corner(
        pivot,
        (upright.width, upright.height, upright.baseline),
        QuarterTurn::Counterclockwise,
        1,
    );
    let by_hand = bunny_ui::layout::DisplayList::from(vec![DrawCommand::Image {
        rect: Rect {
            origin: Point { x: x as f64, y: y as f64 },
            size: Size { width: picture.width as f64, height: picture.height as f64 },
        },
        source: ImageSource::rgba(7, (picture.width as u32, picture.height as u32), picture.rgba),
        placement: bunny_ui::layout::ImagePlacement::Pixel,
    }]);
    let painted = bunny_ui::raster::rasterize(&turned, 40, 60, GROUND);
    let expected = bunny_ui::raster::rasterize(&by_hand, 40, 60, GROUND);
    assert_eq!(painted.pixels(), expected.pixels(), "the compositor turns the raster and nothing else");
    assert!(painted.pixels().iter().any(|pixel| *pixel != expected.pixels()[0] || pixel.to_be_bytes()[0] != 255));
}

#[test]
fn the_painter_dashes_turns_and_stripes_what_it_paints() {
    let runtime = Runtime::new();
    let line: Vec<Verb> = vec![Verb::Move(0.0, 5.0), Verb::Line(30.0, 5.0)];
    let view = canvas(move |_, painter| {
        painter.path(&line, Paint::Dashed { width: 2.0, dash: Dash::new(4.0, 4.0) }, INK);
    })
    .frame(40.0, 10.0);
    let display = runtime.display_frame(&view, Size { width: 40.0, height: 10.0 });
    let bitmap = bunny_ui::raster::rasterize(&display, 40, 10, GROUND);
    assert!(alpha_at(&bitmap, 1, 5) > 200, "a dash");
    assert!(alpha_at(&bitmap, 5, 5) < 10, "the air after it — a butt end stops square");
    assert!(alpha_at(&bitmap, 9, 5) > 200, "the next dash");
    let view = canvas(|_, painter| {
        painter.text_turned(Point { x: 10.0, y: 30.0 }, "Up", INK, QuarterTurn::Counterclockwise);
        painter.icon_turned(
            Rect { origin: Point { x: 20.0, y: 0.0 }, size: Size { width: 16.0, height: 16.0 } },
            symbol::CHEVRON_RIGHT,
            INK,
            Angle::degrees(90.0),
        );
        painter.stripes(
            Rect { origin: Point { x: 0.0, y: 32.0 }, size: Size { width: 40.0, height: 8.0 } },
            Stripes::new(Angle::degrees(45.0), 2.0, 6.0, INK),
            4.0,
        );
        painter.border(
            Rect { origin: Point::ZERO, size: Size { width: 40.0, height: 40.0 } },
            INK,
            1.0,
            BorderStyle::Dotted,
            0.0,
        );
    })
    .frame(40.0, 40.0)
    .padding_length(5.0);
    let display = runtime.display_frame(&view, Size { width: 50.0, height: 50.0 });
    let turned = display.iter().find_map(|command| match command {
        DrawCommand::TurnedText { pivot, .. } => Some(*pivot),
        _ => None,
    });
    assert_eq!(turned, Some(Point { x: 15.0, y: 35.0 }), "the pivot shifts with the box like any point");
    let images = display.iter().filter(|command| matches!(command, DrawCommand::Image { .. })).count();
    assert_eq!(images, 3, "the glyph, the bands and the dots");
    assert!(
        display.iter().any(|command| matches!(command, DrawCommand::PushClip { .. })),
        "rounded bands are cut to their corners"
    );
}
