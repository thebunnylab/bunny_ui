use bunny_ui::angle::Angle;
use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 200.0, height: 200.0 };

fn frame(runtime: &Runtime, name: &str) -> Rect {
    runtime.test_frames().into_iter().find(|(id, _)| &**id == name).map(|(_, rect)| rect).expect(name)
}

/// `.rotation_effect(…)` turns what the view draws and leaves the layout
/// alone, as `SwiftUI`'s does: the next box stands where it stood, and
/// the turned one is found at the box it covers once turned — the page's
/// `getBoundingClientRect` of a turned element.
#[test]
fn a_turned_view_keeps_its_place_and_reports_the_box_it_covers() {
    let runtime = Runtime::new();
    let view = vstack!(
        rectangle().frame(40.0, 10.0).test_id("bar").rotation_effect(Angle::degrees(90.0)),
        rectangle().frame(40.0, 10.0).test_id("next"),
    );
    runtime.display_frame(&view, SIZE);
    assert_eq!(frame(&runtime, "next").origin, Point { x: 0.0, y: 10.0 });
    let bar = frame(&runtime, "bar");
    assert!((bar.origin.x - 15.0).abs() < 1e-9 && (bar.origin.y + 15.0).abs() < 1e-9, "{bar:?}");
    assert!((bar.size.width - 10.0).abs() < 1e-9 && (bar.size.height - 40.0).abs() < 1e-9, "{bar:?}");
}

/// The subtree is drawn once, upright, and laid on the scene turned
/// about its centre: one sprite under the turn's map.
#[test]
fn a_turned_view_is_one_sprite_under_its_turn() {
    let runtime = Runtime::new();
    let view = rectangle().frame(40.0, 10.0).rotation_effect(Angle::degrees(30.0)).padding_length(50.0);
    let display = runtime.display_frame(&view, SIZE);
    let sprites: Vec<_> = display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::Sprite { transform, .. } => Some(*transform),
            _ => None,
        })
        .collect();
    assert_eq!(sprites.len(), 1, "{display:?}");
    let expected = bunny_ui::turn::about(30.0, Point { x: 70.0, y: 55.0 });
    let map = sprites[0];
    for (got, want) in [(map.a, expected.a), (map.b, expected.b), (map.c, expected.c), (map.d, expected.d), (map.e, expected.e), (map.f, expected.f)] {
        assert!((got - want).abs() < 1e-9, "{map:?} against {expected:?}");
    }
}

/// And the CPU raster paints where the turned box lies, and no longer
/// where the upright one did.
#[test]
fn the_raster_paints_the_turned_box() {
    let runtime = Runtime::new();
    let view = rectangle().foreground_color(Color::rgb(0, 0, 255)).frame(60.0, 10.0).rotation_effect(Angle::degrees(90.0)).padding_length(40.0);
    let display = runtime.display_frame(&view, Size { width: 140.0, height: 90.0 });
    let bitmap = bunny_ui::raster::rasterize(&display, 140, 90, Color::rgb(255, 255, 255));
    let blue = |x: usize, y: usize| bitmap.pixels()[y * bitmap.width() + x].to_be_bytes()[2] > 200 && bitmap.pixels()[y * bitmap.width() + x].to_be_bytes()[0] < 60;
    // the bar ran from x 40 to 100 at y 40–50; turned about (70, 45) it
    // runs from y 15 to 75 at x 65–75
    assert!(blue(70, 20), "the turned bar reaches up");
    assert!(blue(70, 70), "and down");
    assert!(!blue(45, 45), "the upright bar's end is gone");
    assert!(!blue(95, 45), "both ends");
}

/// The page turns the element itself: CSS `rotate` about the pivot,
/// which the layout around it never feels.
#[test]
fn the_page_turns_the_element_about_its_pivot() {
    let page = bunny_ui::ssr::render(
        &vstack!(
            text("tilted").rotation_effect(Angle::degrees(-3.0)),
            text("swung").rotation_effect_at(Angle::degrees(45.0), UnitPoint::TOP_LEADING),
        ),
        SIZE,
    );
    assert!(page.css.contains("rotate:-3deg;") && page.css.contains("transform-origin:50% 50%"), "{}", page.css);
    assert!(page.css.contains("rotate:45deg;") && page.css.contains("transform-origin:0% 0%"), "{}", page.css);
}
