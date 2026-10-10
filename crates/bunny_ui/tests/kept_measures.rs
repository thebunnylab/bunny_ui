//! The measures a boundary keeps are by what it holds. When a bound text
//! under it reads a new value — no body runs; the binding moves the text on
//! the pixel path — whatever sits beside the text must move with it. The
//! dirty binding's key used to be taken and dropped on every pixel-path
//! frame, and the kept measure above it answered with the old size until
//! some body happened to run.
extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const SIZE: Size = Size { width: 400.0, height: 100.0 };

#[derive(Clone)]
struct Row {
    word: State<String>,
}

impl Component for Row {
    fn body(self) -> impl View {
        // the word is handed to the text, never read here: a write moves
        // the text through its binding and runs no body
        let word = self.word;
        hstack![text!("{}", word), text!("|").on_click(|| {}).id("tail")]
    }
}

fn tail_x(runtime: &Runtime, view: &impl View) -> f64 {
    let laid = runtime.settled_layout(view, Proposal::exact(SIZE));
    laid.hits
        .iter()
        .find(|(path, _)| path.contains("[tail]"))
        .map(|(_, rect)| rect.origin.x)
        .expect("the tail is a hit")
}

#[test]
fn a_bound_text_that_grew_moves_what_sits_beside_it() {
    let word = State::new(String::from("ab"));
    let runtime = Runtime::new();
    let view = Row { word };
    let _ = runtime.display_frame(&view, SIZE);
    let before = tail_x(&runtime, &view);
    word.set(String::from("abcdefghijklmnopqrstuvwxyz"));
    let _ = runtime.display_frame(&view, SIZE);
    assert!(runtime.body_runs().is_empty(), "a binding moves no body: {:?}", runtime.body_runs());
    let after = tail_x(&runtime, &view);
    assert!(after > before + 10.0, "the tail moved right with the longer word: {before} -> {after}");
}
