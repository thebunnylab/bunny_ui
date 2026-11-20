use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const PAGE: Size = Size { width: 400.0, height: 300.0 };

fn frames_of(view: &impl View) -> Vec<(String, Rect)> {
    let runtime = Runtime::new();
    runtime.display_frame(view, PAGE);
    runtime.test_frames().into_iter().map(|(id, rect)| (id.to_string(), rect)).collect()
}

fn frame(frames: &[(String, Rect)], name: &str) -> Rect {
    frames.iter().find(|(id, _)| id == name).map(|(_, rect)| *rect).expect(name)
}

const A: &str = "Shrinking labels wrap";
const B: &str = "when the row is narrow";

/// Two texts in a row narrower than both: each gives up width in
/// proportion to its natural one, the row fits, and the texts wrap —
/// what a CSS flex line does with the same two spans.
#[test]
fn texts_in_a_narrow_row_shrink_in_proportion_and_wrap() {
    let natural = frames_of(&hstack!(text(A).test_id("a"), text(B).test_id("b")));
    let (wide_a, wide_b) = (frame(&natural, "a"), frame(&natural, "b"));

    let narrow = frames_of(
        &hstack!(text(A).test_id("a"), text(B).test_id("b"))
            .alignment(VerticalAlignment::Top)
            .spacing(8.0)
            .frame_width_aligned(200.0, Alignment::Leading),
    );
    let (a, b) = (frame(&narrow, "a"), frame(&narrow, "b"));
    assert!((a.size.width + 8.0 + b.size.width - 200.0).abs() < 1e-6, "the row fits: {a:?} {b:?}");
    assert!((a.origin.x + a.size.width + 8.0 - b.origin.x).abs() < 1e-6, "side by side: {a:?} {b:?}");
    let ratio = (a.size.width / b.size.width) / (wide_a.size.width / wide_b.size.width);
    assert!((ratio - 1.0).abs() < 1e-9, "in proportion to their naturals: {a:?} {b:?}");
    assert!(a.size.height > wide_a.size.height && b.size.height > wide_b.size.height, "and both wrap: {a:?} {b:?}");
}

/// A word is a floor: a text does not shrink past its longest word, a
/// fixed frame not past itself, and the row overflows at the floors.
#[test]
fn a_row_shrinks_no_text_past_its_longest_word() {
    let word = "Supercalifragilistic";
    let alone = frames_of(&hstack!(text(word).test_id("word")));
    let narrow = frames_of(
        &hstack!(text(word).test_id("word"), rectangle().frame(40.0, 10.0).test_id("box"), text("a b c").test_id("short"))
            .frame_width_aligned(60.0, Alignment::Leading),
    );
    assert_eq!(frame(&narrow, "word").size.width, frame(&alone, "word").size.width);
    assert_eq!(frame(&narrow, "box").size.width, 40.0);
}
