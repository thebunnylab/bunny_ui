//! A box inside a popover is told the anchor the popover hangs from, so a
//! bubble can point its arrow at it — even when the popover was pushed
//! aside to stay inside the window.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use std::cell::Cell;
use std::rc::Rc;

use bunny_ui::custom::canvas;
use bunny_ui::layout::{Proposal, Side, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const SIZE: Size = Size {
    width: 400.0,
    height: 300.0,
};

fn told(seen: &Rc<Cell<Option<f64>>>) -> impl View<Arity = Single> + use<> {
    let seen = Rc::clone(seen);
    canvas(move |ctx, _| seen.set(ctx.anchor_x())).frame(160.0, 60.0)
}

fn anchored(at: f64, seen: &Rc<Cell<Option<f64>>>, open: State<bool>) -> impl View + use<> {
    let seen = Rc::clone(seen);
    hstack((
        spacer().frame(at, 10.0),
        text("anchor")
            .frame(40.0, 20.0)
            .popover(open.binding(), Side::Bottom, move |_| {
                bunny_ui::erased::erased(told(&seen))
            })
            .id("anchor"),
        spacer(),
    ))
    .frame_max(f64::INFINITY, f64::INFINITY, Alignment::TopLeading)
}

fn popover_frame(runtime: &Runtime, root: &impl View) -> bunny_ui::layout::Rect {
    let layout = runtime.settled_layout(root, Proposal::exact(SIZE));
    layout.overlays.first().expect("the popover is open").frame
}

#[test]
fn a_box_in_a_popover_is_told_where_the_anchor_stands() {
    let runtime = Runtime::new();
    let (seen, open) = (Rc::new(Cell::new(None)), State::new(true));
    let root = anchored(150.0, &seen, open);
    runtime.display_frame(&root, SIZE);
    let frame = popover_frame(&runtime, &root);
    let middle = 150.0 + 20.0 - frame.origin.x;
    assert_eq!(seen.get(), Some(middle));
    assert_eq!(
        middle, 80.0,
        "a popover with room hangs centred under its anchor"
    );
}

#[test]
fn the_anchor_stays_true_when_the_popover_is_pushed_aside() {
    let runtime = Runtime::new();
    let (seen, open) = (Rc::new(Cell::new(None)), State::new(true));
    let root = anchored(350.0, &seen, open);
    runtime.display_frame(&root, SIZE);
    let frame = popover_frame(&runtime, &root);
    assert_eq!(frame.origin.x + frame.size.width, SIZE.width);
    assert_eq!(seen.get(), Some(350.0 + 20.0 - frame.origin.x));
}

#[test]
fn a_box_outside_a_popover_has_no_anchor() {
    let runtime = Runtime::new();
    let seen = Rc::new(Cell::new(Some(1.0)));
    let root = told(&seen);
    runtime.display_frame(&root, SIZE);
    assert_eq!(seen.get(), None);
}
