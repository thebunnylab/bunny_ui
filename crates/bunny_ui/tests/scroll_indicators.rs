//! A scroll region's thumb is chrome the region may keep to itself:
//! always shown, shown only mid-gesture (an overlay scrollbar), or never
//! — and the region travels the same in all three.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::layout::{DrawCommand, LayoutResult, Proposal, Size};
use bunny_ui::prelude::*;

const VIEWPORT: Size = Size { width: 200.0, height: 100.0 };

/// Forty 20 pt rows in a 100 pt region: 700 pt of travel.
fn list(indicators: ScrollIndicators) -> impl View {
    scroll(vstack!(for_each((0..40).collect::<Vec<usize>>(), |row| row.to_string(), |row| {
        text(format!("row {row}")).frame_height(20.0)
    }))
    .spacing(0.0))
    .scroll_indicators(indicators)
    .id("list")
}

fn laid(runtime: &Runtime, view: &impl View) -> LayoutResult {
    runtime.layout(view, Proposal::exact(VIEWPORT))
}

/// Does the frame paint the region's vertical thumb, and can it be grabbed?
fn thumb(layout: &LayoutResult) -> (bool, bool) {
    let ink = theme::current().scrollbar;
    let painted = layout
        .display
        .iter()
        .any(|command| matches!(command, DrawCommand::FillRect { color, .. } if *color == ink));
    let grabbable = layout.hits.iter().any(|(path, _)| path.ends_with("/#thumb-v"));
    (painted, grabbable)
}

fn offset(runtime: &Runtime, layout: &LayoutResult) -> f64 {
    let region = layout.scrolls.iter().find(|region| region.path.ends_with("[list]")).expect("the list");
    runtime.scroll_offset(&region.path).y
}

#[test]
fn a_visible_thumb_paints_whenever_the_content_overflows() {
    let runtime = Runtime::new();
    let view = list(ScrollIndicators::Visible);
    assert_eq!(thumb(&laid(&runtime, &view)), (true, true));
}

#[test]
fn a_thumb_that_is_never_shown_is_never_grabbed_and_the_region_still_travels() {
    let runtime = Runtime::new();
    let view = list(ScrollIndicators::Never);
    assert_eq!(thumb(&laid(&runtime, &view)), (false, false));

    assert!(runtime.wheel(100.0, 50.0, 0.0, -60.0), "the wheel still moves it");
    let layout = laid(&runtime, &view);
    assert_eq!(offset(&runtime, &layout), 60.0);
    assert_eq!(thumb(&layout), (false, false), "not even mid-gesture");
}

#[test]
fn an_overlay_thumb_shows_while_the_gesture_moves_it_and_leaves_with_the_gesture() {
    let runtime = Runtime::new();
    let view = list(ScrollIndicators::Hidden);
    assert_eq!(thumb(&laid(&runtime, &view)), (false, false), "gone at rest");

    assert!(runtime.wheel(100.0, 50.0, 0.0, -40.0));
    let layout = laid(&runtime, &view);
    assert_eq!(offset(&runtime, &layout), 40.0);
    assert_eq!(thumb(&layout), (true, true), "shown while the wheel turns");

    // one slow tick is not the end of a gesture
    assert!(!runtime.wheel_tick());
    assert_eq!(thumb(&laid(&runtime, &view)), (true, true));

    // the second ends it, and the shell is told the scene changed
    assert!(runtime.wheel_tick(), "the thumb that leaves asks for a frame");
    assert_eq!(thumb(&laid(&runtime, &view)), (false, false));
    assert!(!runtime.wheel_tick(), "and a clock with no gesture asks for none");
}

#[test]
fn the_end_of_a_gesture_over_a_visible_thumb_asks_for_nothing() {
    let runtime = Runtime::new();
    let view = list(ScrollIndicators::Visible);
    laid(&runtime, &view);
    assert!(runtime.wheel(100.0, 50.0, 0.0, -40.0));
    laid(&runtime, &view);
    assert!(!runtime.wheel_tick());
    assert!(!runtime.wheel_tick(), "nothing on screen depended on the gesture");
}

#[test]
fn a_scroll_target_keeps_the_regions_indicators() {
    let runtime = Runtime::new();
    let view = scroll(vstack!(for_each((0..40).collect::<Vec<usize>>(), |row| row.to_string(), |row| {
        text(format!("row {row}")).frame_height(20.0).id(format!("row-{row}"))
    }))
    .spacing(0.0))
    .scroll_indicators(ScrollIndicators::Never)
    .scroll_target("row-30")
    .id("list");
    assert_eq!(thumb(&laid(&runtime, &view)), (false, false));
}

#[test]
fn served_indicators_say_scrollbar_width() {
    let css = |indicators| bunny_ui::ssr::render(&list(indicators), VIEWPORT).css;
    assert!(css(ScrollIndicators::Never).contains("scrollbar-width:none"));
    assert!(css(ScrollIndicators::Hidden).contains("scrollbar-width:thin"));
    assert!(!css(ScrollIndicators::Visible).contains("scrollbar-width"), "the browser's own bar");
}
