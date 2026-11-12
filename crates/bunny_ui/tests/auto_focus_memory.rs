//! What an input already answered is remembered for as long as it lives.
//!
//! `.auto_focus()` takes a free keyboard on the field's first appearance
//! and never again: a blur by the reader is final. The memory must hold
//! for a field at any path, also one that ends in a position (`#0`), and
//! must leave when the field leaves.

use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 240.0,
};

/// The field has no name: its path ends in its place in the stack, `#0`.
#[derive(Clone, Copy)]
struct Form {
    name: State<String>,
    ticks: State<u32>,
    shown: State<bool>,
}

impl Form {
    fn new() -> Self {
        Self {
            name: State::new(String::new()),
            ticks: State::new(0),
            shown: State::new(true),
        }
    }
}

impl Component for Form {
    fn body(self) -> impl View {
        vstack!(
            self.shown
                .get()
                .then(|| text_field("Name", self.name.binding()).auto_focus()),
            text!("ticks {}", self.ticks),
        )
    }
}

#[test]
fn a_first_appearance_at_a_position_respects_blur() {
    let runtime = Runtime::new();
    let form = Form::new();
    runtime.display_frame(&form, SIZE);
    let field = runtime.focused().expect("the first appearance took the free keyboard");
    assert!(
        field.rsplit('/').next().is_some_and(|last| last.starts_with('#')),
        "the field stands at a position, not under a name: {field}"
    );

    assert!(runtime.blur());
    assert_eq!(runtime.focused(), None);
    // any pass at all: the form writes a state it reads
    for tick in 1..=3 {
        form.ticks.set(tick);
        runtime.display_frame(&form, SIZE);
        assert_eq!(
            runtime.focused(),
            None,
            "a blur is final; pass {tick} gave the keyboard back to the field"
        );
    }
}

#[test]
fn a_field_that_returns_is_a_first_appearance_again() {
    let runtime = Runtime::new();
    let form = Form::new();
    runtime.display_frame(&form, SIZE);
    assert!(runtime.focused().is_some());
    assert!(runtime.blur());

    form.shown.set(false);
    runtime.display_frame(&form, SIZE);
    assert_eq!(runtime.focused(), None);

    form.shown.set(true);
    runtime.display_frame(&form, SIZE);
    assert!(
        runtime.focused().is_some(),
        "the field left and came back: the memory left with it, and this is a first appearance"
    );
}

/// Two fields, each summoned by its own beat, one at a position and one
/// under a name: a beat is answered once for each.
#[derive(Clone, Copy)]
struct Summons {
    first: State<String>,
    second: State<String>,
    beat: State<u64>,
}

impl Component for Summons {
    fn body(self) -> impl View {
        vstack!(
            text_field("First", self.first.binding()).auto_focus_beat(self.beat.get()),
            text_field("Second", self.second.binding()).id("second"),
        )
    }
}

#[test]
fn a_beat_at_a_position_is_answered_once() {
    let runtime = Runtime::new();
    let scene = Summons {
        first: State::new(String::new()),
        second: State::new(String::new()),
        beat: State::new(1),
    };
    runtime.display_frame(&scene, SIZE);
    let first = runtime.focused().expect("beat one summoned the first field");
    assert!(runtime.focus_named("second"));
    let second = runtime.focused();
    assert_ne!(second, Some(first.clone()));

    // passes with the same beat: the reader moved away and stays away
    for _ in 0..3 {
        runtime.display_frame(&scene, SIZE);
        assert_eq!(runtime.focused(), second, "the beat was already answered");
    }
    // a new beat is a new intent
    scene.beat.set(2);
    runtime.display_frame(&scene, SIZE);
    assert_eq!(runtime.focused(), Some(first));
}
