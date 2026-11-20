use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 300.0, height: 200.0 };

fn frames(view: &impl View) -> Vec<(String, Rect)> {
    let runtime = Runtime::new();
    runtime.display_frame(view, SIZE);
    runtime.test_frames().into_iter().map(|(id, rect)| (id.to_string(), rect)).collect()
}

fn frame(frames: &[(String, Rect)], name: &str) -> Rect {
    frames.iter().find(|(id, _)| id == name).map(|(_, rect)| *rect).expect(name)
}

/// `.frame_min(…)` is `SwiftUI`'s `.frame(minWidth:minHeight:)`: the box
/// hugs its content and is never smaller than the floor — a badge as
/// wide as its count, no narrower than a circle.
#[test]
fn a_floor_holds_a_small_content_and_lets_a_wide_one_through() {
    let found = frames(&hstack!(
        rectangle().frame(10.0, 6.0).frame_min(18.0, 18.0, Alignment::Center).test_id("small"),
        rectangle().frame(40.0, 6.0).frame_min(18.0, 18.0, Alignment::Center).test_id("wide"),
        rectangle().frame(10.0, 30.0).test_id("tall"),
    )
    .alignment(VerticalAlignment::Top)
    .spacing(0.0));
    assert_eq!(frame(&found, "small").size, Size { width: 18.0, height: 18.0 });
    assert_eq!(frame(&found, "wide").size, Size { width: 40.0, height: 18.0 });
}

/// The content sits by the alignment when the floor is the wider.
#[test]
fn the_content_sits_by_the_alignment() {
    let found = frames(&vstack!(
        rectangle().frame(10.0, 10.0).test_id("dot").frame_min(50.0, 0.0, Alignment::Leading).test_id("box"),
    ));
    let (dot, whole) = (frame(&found, "dot"), frame(&found, "box"));
    assert_eq!(whole.size.width, 50.0);
    assert_eq!(dot.origin.x, whole.origin.x, "on the leading edge");
}

/// Proposed less than its floor, the box is its floor anyway — it
/// overflows what holds it, as `SwiftUI`'s does — and a floor of zero is
/// no floor: the box is its content.
#[test]
fn the_floor_beats_a_narrow_proposal_and_zero_is_none() {
    let found = frames(&hstack!(
        rectangle().frame(10.0, 10.0).frame_min(100.0, 0.0, Alignment::Center).test_id("floored"),
    )
    .frame(60.0, 40.0));
    assert_eq!(frame(&found, "floored").size.width, 100.0);
    let hugged = frames(&rectangle().frame(10.0, 10.0).frame_min(0.0, 0.0, Alignment::Center).test_id("hug"));
    assert_eq!(frame(&hugged, "hug").size, Size { width: 10.0, height: 10.0 });
}

/// The page writes the floor as the box's own `min-width` and
/// `min-height`, and `frame_flex` keeps its floor there too.
#[test]
fn the_page_writes_the_floor() {
    let page = bunny_ui::ssr::render(
        &vstack!(
            text("3").frame_min(18.0, 18.0, Alignment::Center),
            text("lane").frame_flex(120.0, 0.0, Alignment::Leading),
        ),
        SIZE,
    );
    assert!(page.css.contains("min-height:18px") && page.css.contains("min-width:18px"), "{}", page.css);
    assert!(page.css.contains("min-width:120px"), "{}", page.css);
}
