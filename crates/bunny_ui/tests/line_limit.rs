use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 300.0, height: 200.0 };
/// The house face's line box: every line of the tests below is this tall.
const LINE: f64 = 16.0;

/// A column 30 points wide, `lines` lines tall: three letters to a line.
const fn column(lines: f64) -> Size {
    Size { width: 30.0, height: lines * LINE }
}

fn frames(view: &impl View) -> Vec<(String, Rect)> {
    let runtime = Runtime::new();
    runtime.display_frame(view, SIZE);
    runtime.test_frames().into_iter().map(|(id, rect)| (id.to_string(), rect)).collect()
}

fn frame(frames: &[(String, Rect)], name: &str) -> Rect {
    frames.iter().find(|(id, _)| id == name).map(|(_, rect)| *rect).expect(name)
}

/// The words each painted line shows, top to bottom, without the space
/// a wrap left at its end.
fn lines(view: &impl View) -> Vec<String> {
    let display = Runtime::new().display_frame(view, SIZE);
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, .. } => Some(content[range.0..range.1].trim_end().to_string()),
            _ => None,
        })
        .collect()
}

/// `.line_limit(2)` is `SwiftUI`'s `.lineLimit(2)`: four lines of words
/// show two, and the second says the rest was cut with an ellipsis.
#[test]
fn a_limit_shows_its_lines_and_closes_the_last_with_an_ellipsis() {
    let words = || text("aaa bbb ccc ddd").frame_width(30.0);
    assert_eq!(frame(&frames(&words().test_id("free")), "free").size, column(4.0));
    let limited = words().line_limit(2).test_id("cut");
    assert_eq!(frame(&frames(&limited), "cut").size, column(2.0));
    let shown = lines(&limited);
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_eq!(shown[0], "aaa");
    assert!(shown[1].ends_with('…'), "{shown:?}");
}

/// A text that fits its limit shows whole, with no ellipsis.
#[test]
fn a_text_inside_its_limit_shows_whole() {
    assert_eq!(lines(&text("aaa bbb").frame_width(30.0).line_limit(2)), ["aaa", "bbb"]);
    assert_eq!(lines(&text("aaa").line_limit(1)), ["aaa"]);
}

/// The limit is inherited like the font, the nearer one wins, and zero
/// is no limit: a text under it shows every line again.
#[test]
fn the_limit_is_inherited_and_zero_is_none() {
    let found = frames(
        &vstack!(
            text("aaa bbb ccc").frame_width(30.0).test_id("inherits"),
            text("aaa bbb ccc").frame_width(30.0).line_limit(2).test_id("nearer"),
            text("aaa bbb ccc").frame_width(30.0).line_limit(0).test_id("none"),
        )
        .line_limit(1),
    );
    assert_eq!(frame(&found, "inherits").size, column(1.0));
    assert_eq!(frame(&found, "nearer").size, column(2.0));
    assert_eq!(frame(&found, "none").size, column(3.0));
}

/// The limit moves with the state that sets it: the card under it is cut
/// again on the next frame.
#[test]
fn a_limit_that_moves_cuts_the_card_again() {
    #[derive(Clone, Copy, PartialEq)]
    struct Card;
    impl Component for Card {
        fn body(self) -> impl View {
            text("aaa bbb ccc").frame_width(30.0).test_id("card")
        }
    }
    #[derive(Clone)]
    struct Board {
        limit: State<u8>,
    }
    impl Component for Board {
        fn body(self) -> impl View {
            vstack!(Card).line_limit(self.limit.get())
        }
    }
    let limit = State::new(0);
    let runtime = Runtime::new();
    let board = Board { limit };
    runtime.display_frame(&board, SIZE);
    let card = |runtime: &Runtime| {
        runtime.test_frames().into_iter().find(|(id, _)| &**id == "card").map(|(_, rect)| rect.size)
    };
    assert_eq!(card(&runtime), Some(column(3.0)));
    limit.set(1);
    runtime.display_frame(&board, SIZE);
    assert_eq!(card(&runtime), Some(column(1.0)));
}

/// The page clamps the text the same way: a box of vertical lines,
/// shown up to the limit, the rest hidden, and the space a wrap leaves
/// at a line's end dropped, so the ellipsis follows the last word —
/// and a text with no limit writes none of it.
#[test]
fn the_page_clamps_the_lines() {
    let page = bunny_ui::ssr::render(&vstack!(text("words")).line_limit(2), SIZE);
    let clamp = ["display:-webkit-box", "-webkit-box-orient:vertical", "-webkit-line-clamp:2", "overflow:hidden", "white-space:pre-line"];
    for declaration in clamp {
        assert!(page.css.contains(declaration), "{declaration}: {}", page.css);
    }
    let free = bunny_ui::ssr::render(&text("words"), SIZE);
    assert!(!free.css.contains("line-clamp"), "{}", free.css);
}
