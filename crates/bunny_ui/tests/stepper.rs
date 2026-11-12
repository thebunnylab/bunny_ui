//! `stepper(binding, bounds)`: a number moved one step at a time.
#![cfg(feature = "canvas")]

extern crate bunny_ui_core as bunny_ui;

use bunny_ui::accessibility::{Action, ActionError, Role};
use bunny_ui::layout::Rect;
use bunny_ui::prelude::*;

const SIZE: Size = Size {
    width: 240.0,
    height: 60.0,
};

#[derive(Clone, Copy)]
struct Party {
    guests: State<u32>,
    locked: State<bool>,
}

impl Party {
    fn of(guests: u32) -> Self {
        Self {
            guests: State::new(guests),
            locked: State::new(false),
        }
    }
}

impl Component for Party {
    fn body(self) -> impl View {
        stepper(self.guests.binding(), 1..=8)
            .disabled(self.locked.get())
            .accessibility_label("Guests")
            .id("guests")
    }
}

/// Where the box is on the glass.
fn frame(runtime: &Runtime, party: &Party) -> Rect {
    let layout = runtime.layout(party, Proposal::exact(SIZE));
    layout
        .customs
        .iter()
        .find(|placement| placement.path.contains("[guests]"))
        .expect("the stepper is placed")
        .frame
}

fn click(runtime: &Runtime, x: f64, y: f64) {
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
}

/// A press on the step-down half, then on the step-up half.
fn halves(frame: Rect) -> ((f64, f64), (f64, f64)) {
    let y = frame.origin.y + frame.size.height / 2.0;
    (
        (frame.origin.x + frame.size.width * 0.25, y),
        (frame.origin.x + frame.size.width * 0.75, y),
    )
}

fn key(runtime: &Runtime, key: Key) -> bool {
    runtime.key_stroke(KeyPattern::key(key)).handled
}

#[test]
fn it_is_a_small_box_of_its_own_size() {
    let runtime = Runtime::new();
    let party = Party::of(2);
    let frame = frame(&runtime, &party);
    assert_eq!(frame.size, Size { width: 72.0, height: 26.0 }, "it does not stretch with the room it is given");
}

/// A row and a column with room to spare, the stepper in both.
#[derive(Clone, Copy)]
struct Roomy {
    guests: State<u32>,
}

impl Component for Roomy {
    fn body(self) -> impl View {
        vstack!(
            hstack!(text("Guests"), stepper(self.guests.binding(), 1..=8).id("in-a-row")),
            stepper(self.guests.binding(), 1..=8).id("in-a-column"),
            text("below"),
        )
    }
}

#[test]
fn it_takes_no_leftover_room_from_the_stack_that_holds_it() {
    let runtime = Runtime::new();
    let roomy = Roomy { guests: State::new(2) };
    let layout = runtime.layout(&roomy, Proposal::exact(Size { width: 400.0, height: 400.0 }));
    for name in ["[in-a-row]", "[in-a-column]"] {
        let frame = layout.customs.iter().find(|placement| placement.path.contains(name)).expect("placed").frame;
        assert_eq!(frame.size, Size { width: 72.0, height: 26.0 }, "{name} keeps its size in a 400 by 400 window");
    }
}

#[test]
fn the_two_halves_take_one_step_and_stop_at_the_bounds() {
    let runtime = Runtime::new();
    let party = Party::of(2);
    let (down, up) = halves(frame(&runtime, &party));

    click(&runtime, up.0, up.1);
    assert_eq!(party.guests.get(), 3);
    click(&runtime, down.0, down.1);
    click(&runtime, down.0, down.1);
    assert_eq!(party.guests.get(), 1);
    click(&runtime, down.0, down.1);
    assert_eq!(party.guests.get(), 1, "the lower bound holds");

    party.guests.set(8);
    frame(&runtime, &party);
    click(&runtime, up.0, up.1);
    assert_eq!(party.guests.get(), 8, "the upper bound holds");
}

#[test]
fn the_keys_step_page_and_reach_the_bounds() {
    let runtime = Runtime::new();
    let party = Party::of(4);
    frame(&runtime, &party);
    assert!(key(&runtime, Key::Tab), "one Tab stop");
    assert!(runtime.focused().is_some_and(|path| path.contains("[guests]")));

    for (pressed, expected) in [
        (Key::Up, 5),
        (Key::Right, 6),
        (Key::Down, 5),
        (Key::Left, 4),
        (Key::End, 8),
        (Key::Home, 1),
        (Key::PageUp, 8),
        (Key::PageDown, 1),
    ] {
        assert!(key(&runtime, pressed), "{pressed:?}");
        assert_eq!(party.guests.get(), expected, "{pressed:?}");
        frame(&runtime, &party);
    }
}

#[test]
fn right_to_left_puts_the_step_down_on_the_right_and_swaps_the_arrows() {
    let runtime = Runtime::new();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let party = Party::of(4);
    let (left, right) = halves(frame(&runtime, &party));
    click(&runtime, right.0, right.1);
    assert_eq!(party.guests.get(), 3, "the leading half is on the right, and it steps down");
    click(&runtime, left.0, left.1);
    assert_eq!(party.guests.get(), 4);

    assert!(key(&runtime, Key::Tab));
    assert!(key(&runtime, Key::Left));
    assert_eq!(party.guests.get(), 5, "left is forward where the scene reads right to left");
    assert!(key(&runtime, Key::Up));
    assert_eq!(party.guests.get(), 6, "up is up in every direction");
}

#[test]
fn assistive_technology_hears_a_stepper_with_its_range() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let party = Party::of(3);
    frame(&runtime, &party);
    let tree = runtime.accessibility_tree();
    let node = tree.nodes().iter().find(|node| node.role == Role::Stepper).expect("a stepper");
    assert_eq!(&*node.label, "Guests");
    let range = node.range.expect("a range");
    assert_eq!((range.minimum(), range.maximum(), range.value(), range.small_change()), (1.0, 8.0, 3.0, 1.0));

    assert_eq!(runtime.accessibility_action(node.id, Action::Increment), Ok(()));
    assert_eq!(party.guests.get(), 4);
    assert_eq!(runtime.accessibility_action(node.id, Action::SetValue(7.0)), Ok(()));
    assert_eq!(party.guests.get(), 7);
    assert_eq!(runtime.accessibility_action(node.id, Action::SetValue(9.0)), Err(ActionError::InvalidValue));
    assert_eq!(runtime.accessibility_action(node.id, Action::Activate), Err(ActionError::Unsupported));
    assert_eq!(party.guests.get(), 7);
}

#[test]
fn a_disabled_stepper_takes_nothing() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let party = Party::of(3);
    party.locked.set(true);
    let (down, up) = halves(frame(&runtime, &party));
    click(&runtime, up.0, up.1);
    click(&runtime, down.0, down.1);
    assert_eq!(party.guests.get(), 3);
    assert!(!key(&runtime, Key::Tab), "no Tab stop");
    let tree = runtime.accessibility_tree();
    let node = tree.nodes().iter().find(|node| node.role == Role::Stepper).expect("still in the tree");
    assert!(!node.enabled);
    assert!(runtime.accessibility_action(node.id, Action::Increment).is_err());
    assert_eq!(party.guests.get(), 3);
}

/// A float behind the same control, with a step of its own.
#[derive(Clone, Copy)]
struct Zoom {
    scale: State<f64>,
}

impl Component for Zoom {
    fn body(self) -> impl View {
        stepper(self.scale.binding(), 0.5..=2.0).step(0.25).id("zoom")
    }
}

#[test]
fn a_float_steps_by_the_step_it_is_given() {
    let runtime = Runtime::new();
    let zoom = Zoom { scale: State::new(1.0) };
    runtime.layout(&zoom, Proposal::exact(SIZE));
    assert!(key(&runtime, Key::Tab));
    assert!(key(&runtime, Key::Up));
    assert_eq!(zoom.scale.get(), 1.25);
    assert!(key(&runtime, Key::End));
    assert_eq!(zoom.scale.get(), 2.0);
    assert!(key(&runtime, Key::Down));
    assert_eq!(zoom.scale.get(), 1.75);
}

#[test]
fn every_number_type_goes_through_the_same_door() {
    fn steps_up<T: bunny_ui::views::StepValue + PartialEq + std::fmt::Debug + Clone>(start: T, bounds: std::ops::RangeInclusive<T>, expected: T) {
        #[derive(Clone)]
        struct One<T: 'static> {
            value: State<T>,
            bounds: std::ops::RangeInclusive<T>,
        }
        impl<T: bunny_ui::views::StepValue + Clone> Component for One<T> {
            fn body(self) -> impl View {
                stepper(self.value.binding(), self.bounds).id("one")
            }
        }
        let runtime = Runtime::new();
        let one = One { value: State::new(start), bounds };
        runtime.layout(&one, Proposal::exact(SIZE));
        assert!(runtime.key_stroke(KeyPattern::key(Key::Tab)).handled);
        assert!(runtime.key_stroke(KeyPattern::key(Key::Up)).handled);
        assert_eq!(one.value.get(), expected);
    }
    steps_up(3_i32, -5..=5, 4);
    steps_up(250_u8, 0..=255, 251);
    steps_up(7_usize, 0..=10, 8);
    steps_up(-2_i64, -10..=10, -1);
    steps_up(1.0_f32, 0.0..=4.0, 2.0);
}
