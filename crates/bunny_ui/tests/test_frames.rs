use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 300.0, height: 200.0 };

fn frames(runtime: &Runtime) -> Vec<(String, Rect)> {
    runtime.test_frames().into_iter().map(|(id, rect)| (id.to_string(), rect)).collect()
}

const fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect { origin: Point { x, y }, size: Size { width, height } }
}

/// A view a `.test_id(…)` names keeps its frame in the pixel pipeline's
/// table: a stack, a box around a padded child and the child inside it,
/// whatever wraps them, each where the layout put it.
#[test]
fn a_named_view_keeps_its_frame_in_the_pixel_pipeline() {
    let runtime = Runtime::new();
    let view = vstack!(
        rectangle().frame(40.0, 10.0).test_id("bar"),
        rectangle().frame(20.0, 20.0).test_id("square").padding_length(5.0).test_id("padded"),
    )
    .spacing(4.0)
    .test_id("column");
    runtime.display_frame(&view, SIZE);
    assert_eq!(
        frames(&runtime),
        [
            ("column".to_owned(), rect(0.0, 0.0, 40.0, 44.0)),
            ("bar".to_owned(), rect(0.0, 0.0, 40.0, 10.0)),
            ("padded".to_owned(), rect(5.0, 14.0, 30.0, 30.0)),
            ("square".to_owned(), rect(10.0, 19.0, 20.0, 20.0)),
        ]
    );
}

/// The text table stays empty until a harness asks for it, and then
/// holds every text the layout placed.
#[test]
fn the_text_frames_are_kept_on_request() {
    let runtime = Runtime::new();
    let view = vstack!(text("one"), text("two")).spacing(2.0);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.text_frames().is_empty());
    bunny_ui::layout::record_text_frames(true);
    let changed = vstack!(text("one"), text("two"), text("three")).spacing(2.0);
    runtime.display_frame(&changed, SIZE);
    bunny_ui::layout::record_text_frames(false);
    let texts = runtime.text_frames();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert!(texts.windows(2).all(|pair| pair[1].origin.y > pair[0].origin.y), "{texts:?}");
}
