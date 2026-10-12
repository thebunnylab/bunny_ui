//! `.hidden()` against a living runtime: the box keeps its place, paints
//! nothing and takes no hit. The catalogue carried the modifier and the
//! pixel pipelines answered it with nothing until the override road
//! needed it — `Override::Hide` is `.hidden()` put on a site.
use bunny_ui_core as bunny_ui;

use bunny_ui::layout::{DisplayList, DrawCommand, LayoutResult, Proposal, Size, hit_test};
use bunny_ui::prelude::*;

const SIZE: Size = Size { width: 240.0, height: 120.0 };

fn laid(view: &impl View) -> LayoutResult {
    Runtime::new().layout(view, Proposal::exact(SIZE))
}

fn origin_of(display: &DisplayList, word: &str) -> Option<Point> {
    display.iter().find_map(|command| match command {
        DrawCommand::TextLine { origin, content, .. } if &**content == word => Some(*origin),
        _ => None,
    })
}

#[test]
fn a_hidden_box_keeps_its_place_paints_nothing_and_takes_no_hit() {
    // the same tree twice, the box styled either way — a style never
    // changes a measure, so the two lay out alike
    let shown = laid(&vstack!(button(text("tap"), || {}).id("tap").opacity(1.0), text!("after")));
    let hidden = laid(&vstack!(button(text("tap"), || {}).id("tap").hidden(), text!("after")));

    // the place is kept: what follows sits where it sat
    assert_eq!(hidden.size, shown.size);
    assert_eq!(origin_of(&hidden.display, "after"), origin_of(&shown.display, "after"));

    // nothing of the box paints
    assert!(origin_of(&shown.display, "tap").is_some(), "the button paints when shown");
    assert!(origin_of(&hidden.display, "tap").is_none(), "{:?}", hidden.display);

    // and no hit inside it exists — the button's frame holds its label
    let label = origin_of(&shown.display, "tap").expect("the button paints when shown");
    let (x, y) = (label.x + 1.0, label.y + 1.0);
    assert!(hit_test(&shown.hits, x, y).is_some(), "{:?}", shown.hits);
    assert!(hit_test(&hidden.hits, x, y).is_none(), "{:?}", hidden.hits);
}
