use bunny_ui::layout::{Gradient, GradientPaint, Stops, ramp_pieces};
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const RED: Color = Color::rgb(255, 0, 0);
const GREEN: Color = Color::rgb(0, 255, 0);
const BLUE: Color = Color::rgb(0, 0, 255);
const BOX: Rect = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: 100.0, height: 100.0 } };

fn down(paint: &GradientPaint, y: f64) -> Color {
    paint.at(Point { x: 50.0, y })
}

/// `.stop(location, color)` is `SwiftUI`'s `Gradient.Stop`: the ramp
/// passes through the color at its place and mixes straight on either
/// side of it.
#[test]
fn a_stop_bends_the_ramp_through_its_color() {
    let paint = Gradient::linear(RED, BLUE).stop(0.5, GREEN).resolve(BOX);
    assert_eq!(down(&paint, 0.0), RED);
    assert_eq!(down(&paint, 25.0), Color::rgb(128, 128, 0));
    assert_eq!(down(&paint, 50.0), GREEN);
    assert_eq!(down(&paint, 75.0), Color::rgb(0, 128, 128));
    assert_eq!(down(&paint, 100.0), BLUE);
    let rings = Gradient::radial(RED, BLUE).radius(0.0, 40.0).stop(0.25, GREEN).resolve(BOX);
    assert_eq!(rings.at(Point { x: 60.0, y: 50.0 }), GREEN, "ten px out of forty");
}

/// Two stops at one place draw a hard edge: the later owns the line.
#[test]
fn two_stops_at_one_place_draw_a_hard_edge() {
    let paint = Gradient::linear(RED, BLUE).stop(0.5, RED).stop(0.5, BLUE).resolve(BOX);
    assert_eq!(down(&paint, 49.0), RED);
    assert_eq!(down(&paint, 50.0), BLUE);
    assert_eq!(down(&paint, 51.0), BLUE);
}

/// A ramp holds `Stops::MOST` stops between its ends; one more drops the
/// stop whose loss changes the ramp least — here the one that sits on
/// the straight line its neighbours draw anyway.
#[test]
fn a_full_ramp_drops_the_stop_it_misses_least() {
    let grey = |level: u8| Color::rgb(level, level, level);
    let mut ramp = Gradient::linear(grey(0), grey(255));
    for (location, level) in [(0.1, 200), (0.2, 10), (0.3, 220), (0.5, 128), (0.7, 30), (0.8, 240)] {
        ramp = ramp.stop(location, grey(level));
    }
    assert_eq!(ramp.stops().len(), Stops::MOST);
    let ramp = ramp.stop(0.9, grey(20));
    assert_eq!(ramp.stops().len(), Stops::MOST);
    let kept: Vec<f64> = ramp.stops().iter().map(|(location, _)| (location * 10.0).round() / 10.0).collect();
    assert!(!kept.contains(&0.5), "the grey half way between 0.3 and 0.7 is the one the ramp misses least: {kept:?}");
}

/// A painter that mixes two colors at a time paints the ramp piece by
/// piece: every place of the ramp belongs to exactly one piece, whose
/// mix is the ramp's own color there.
#[test]
fn the_pieces_paint_every_place_once_with_the_ramps_color() {
    let ramp = Gradient::linear(RED, BLUE).stop(0.25, GREEN).stop(0.5, RED).stop(0.5, BLUE).stop(0.75, GREEN);
    let paint = ramp.resolve(BOX);
    let pieces = ramp_pieces(RED, BLUE, ramp.stops());
    assert_eq!(pieces.len(), 4, "the hard edge is no piece");
    assert!(!pieces[0].cuts_below && pieces[0].cuts_above);
    assert!(pieces[3].cuts_below && !pieces[3].cuts_above);
    for step in 0..=200 {
        let t = f64::from(step) / 200.0;
        let owners: Vec<_> = pieces
            .iter()
            .filter(|piece| {
                let local = (t - piece.from.0) / (piece.to.0 - piece.from.0);
                (!piece.cuts_below || local >= 0.0) && (!piece.cuts_above || local < 1.0)
            })
            .collect();
        assert_eq!(owners.len(), 1, "t = {t}");
        let piece = owners[0];
        let local = ((t - piece.from.0) / (piece.to.0 - piece.from.0)).clamp(0.0, 1.0);
        let two = Gradient::linear(piece.from.1, piece.to.1).resolve(BOX);
        assert_eq!(down(&two, local * 100.0), down(&paint, t * 100.0), "t = {t}");
    }
}

/// The CPU raster paints the stops.
#[test]
fn the_raster_paints_the_stops() {
    let runtime = Runtime::new();
    let view = spacer().frame(10.0, 100.0).background_gradient(Gradient::linear(RED, BLUE).stop(0.5, GREEN));
    let display = runtime.display_frame(&view, Size { width: 10.0, height: 100.0 });
    let bitmap = bunny_ui::raster::rasterize(&display, 10, 100, Color::rgb(255, 255, 255));
    let at = |y: usize| bitmap.pixels()[y * bitmap.width() + 5].to_be_bytes();
    let [r, g, b, _] = at(50);
    assert!(g > 240 && r < 10 && b < 10, "green half way: {:?}", at(50));
    assert!(at(2)[0] > 240 && at(97)[2] > 240, "red on top, blue at the bottom");
}

/// The page writes each stop at its place along the ramp; a two-color
/// ramp keeps the CSS it had.
#[test]
fn the_page_places_each_stop() {
    let page = bunny_ui::ssr::render(
        &vstack!(
            spacer().frame(10.0, 10.0).background_gradient(Gradient::linear(RED, BLUE).stop(0.25, GREEN)),
            spacer().frame(10.0, 10.0).background_gradient(Gradient::linear(GREEN, BLUE)),
            spacer().frame(10.0, 10.0).background_gradient(Gradient::radial(RED, BLUE).radius(10.0, 50.0).stop(0.5, GREEN)),
        ),
        Size { width: 100.0, height: 100.0 },
    );
    assert!(
        page.css.contains("linear-gradient(180.00deg, rgba(255, 0, 0, 1) 0%, rgba(0, 255, 0, 1) 25.00%, rgba(0, 0, 255, 1) 100%)"),
        "{}",
        page.css
    );
    assert!(page.css.contains("linear-gradient(180.00deg, rgba(0, 255, 0, 1), rgba(0, 0, 255, 1))"), "{}", page.css);
    assert!(page.css.contains("rgba(255, 0, 0, 1) 10px, rgba(0, 255, 0, 1) 30.00px, rgba(0, 0, 255, 1) 50px"), "{}", page.css);
}
