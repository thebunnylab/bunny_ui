use super::*;

use motor::state::LayoutDirection;

use crate::layout::{Color, Corners, DrawCommand, Point, Proposal, Rect, Size};
use crate::runtime::Runtime;
use crate::theme::{ProgressBar, Theme};

const WIDTH: f64 = 200.0;

fn offered(width: f64) -> Proposal {
    Proposal { width: Some(width), height: None }
}

/// Every rounded fill the bar paints, in paint order.
fn fills(view: &ProgressView, direction: Option<LayoutDirection>) -> (Size, Vec<(Rect, Color, Corners)>) {
    themed(view, direction, Theme::light())
}

fn themed(
    view: &ProgressView,
    direction: Option<LayoutDirection>,
    theme: Theme,
) -> (Size, Vec<(Rect, Color, Corners)>) {
    let runtime = Runtime::new().theme(theme);
    runtime.set_layout_direction(direction);
    let result = runtime.layout(view, offered(WIDTH));
    let fills = result
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, corner_radius } => Some((*rect, *color, *corner_radius)),
            _ => None,
        })
        .collect();
    (result.size, fills)
}

/// The fill on top of the track, when the bar paints one.
fn fill(value: f32, direction: Option<LayoutDirection>) -> Option<Rect> {
    let (_, fills) = fills(&progress_view().value(value), direction);
    let theme = Theme::light();
    assert_eq!(fills[0].1, theme.divider, "the track paints first, in the divider ink");
    fills.iter().skip(1).find(|(_, color, _)| *color == theme.accent).map(|(rect, _, _)| *rect)
}

#[test]
fn without_a_value_the_view_is_the_twenty_point_box_it_always_was() {
    let result = Runtime::new().layout(&progress_view(), offered(WIDTH));
    assert_eq!(result.size, Size { width: 20.0, height: 20.0 });
    assert_eq!(progress_view().fraction(), None);
    assert_eq!(progress_view(), ProgressView::default());
}

#[test]
fn with_a_value_the_bar_takes_the_width_and_the_theme_height() {
    let (size, fills) = fills(&progress_view().value(0.25), None);
    let bar = Theme::light().progress;
    assert_eq!(bar, ProgressBar::CLASSIC);
    assert_eq!(size, Size { width: WIDTH, height: bar.height });
    let (track, _, corners) = fills[0];
    assert_eq!(track, Rect { origin: Point { x: 0.0, y: 0.0 }, size });
    assert_eq!(corners, Corners::all(bar.radius));
}

#[test]
fn the_value_is_clamped_to_zero_through_one() {
    assert_eq!(progress_view().value(-0.5).fraction(), Some(0.0));
    assert_eq!(progress_view().value(1.5).fraction(), Some(1.0));
    assert_eq!(progress_view().value(f32::NAN).fraction(), Some(0.0));
    assert_eq!(progress_view().value(f32::INFINITY).fraction(), Some(1.0));
    assert_eq!(progress_view().value(0.3).fraction(), Some(0.3));
    assert_eq!(fill(7.0, None), fill(1.0, None));
    assert_eq!(fill(-7.0, None), None);
}

#[test]
fn the_fill_is_as_long_as_the_share_the_value_names() {
    assert_eq!(fill(0.0, None), None, "nothing done paints only the track");
    let half = fill(0.5, None).expect("half a bar");
    assert_eq!(half.origin.x, 0.0);
    assert_eq!(half.size.width, WIDTH / 2.0);
    assert_eq!(half.size.height, ProgressBar::CLASSIC.height);
    let full = fill(1.0, None).expect("a full bar");
    assert_eq!(full, Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: WIDTH, height: 4.0 } });
    let (_, fills) = fills(&progress_view().value(0.5), None);
    assert_eq!(fills[1].2, Corners::all(ProgressBar::CLASSIC.radius), "the fill's ends are round too");
}

#[test]
fn a_right_to_left_bar_fills_from_the_right() {
    let rtl = Some(LayoutDirection::RightToLeft);
    assert_eq!(fill(0.0, rtl), None);
    for value in [0.25, 0.5, 1.0] {
        let ltr = fill(value, None).expect("left to right");
        let mirrored = fill(value, rtl).expect("right to left");
        assert_eq!(mirrored.size, ltr.size);
        assert_eq!(mirrored.origin.x, WIDTH - ltr.origin.x - ltr.size.width, "{value}: x' = W − x − w");
        assert_eq!(mirrored.origin.y, ltr.origin.y);
    }
    assert_eq!(fill(0.25, rtl).expect("a quarter").origin.x, WIDTH * 0.75);
}

#[test]
fn the_theme_sizes_and_paints_the_bar() {
    let theme = Theme {
        accent: Color::hex(0xAA3311),
        divider: Color::hex(0x112233),
        progress: ProgressBar { height: 10.0, radius: 5.0 },
        ..Theme::light()
    };
    let (size, fills) = themed(&progress_view().value(0.5), None, theme);
    assert_eq!(size.height, 10.0);
    assert_eq!(fills[0].1, Color::hex(0x112233));
    assert_eq!(fills[0].2, Corners::all(5.0));
    let (rect, color, _) = fills[1];
    assert_eq!(color, Color::hex(0xAA3311));
    assert_eq!(rect.size, Size { width: WIDTH / 2.0, height: 10.0 });
}

#[test]
fn the_served_bar_is_a_grid_whose_fill_spans_the_share() {
    let page = crate::ssr::render(&progress_view().value(0.5), Size { width: WIDTH, height: 40.0 });
    assert!(
        page.css.contains(&format!("grid-template-columns:repeat({RESOLUTION},minmax(0,1fr))")),
        "{}",
        page.css
    );
    assert!(page.html.contains("grid-column:span 500"), "{}", page.html);
}
