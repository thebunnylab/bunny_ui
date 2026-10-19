#![cfg(feature = "canvas")]
extern crate bunny_ui_core as bunny_ui;
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Control {
    value: State<f64>,
    disabled: State<bool>,
    range: SliderRange,
}
impl Component for Control {
    fn body(self) -> impl View {
        slider(self.value.binding(), self.range)
            .disabled(self.disabled.get())
            .frame(220.0, 32.0)
            .id("rate")
    }
}
fn setup(range: SliderRange, value: f64) -> (Runtime, Control) {
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let view = Control {
        value: State::new(value),
        disabled: State::new(false),
        range,
    };
    frame(&runtime, view);
    (runtime, view)
}
fn frame(runtime: &Runtime, view: Control) -> bunny_ui::layout::LayoutResult {
    runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 220.0,
            height: 32.0,
        }),
    )
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}
fn click(runtime: &Runtime, x: f64) {
    runtime.pointer_pressed(x, 16.0);
    runtime.pointer_released(x, 16.0);
}
fn key(runtime: &Runtime, key: Key) {
    assert!(runtime.key_stroke(KeyPattern::key(key)).handled);
}

#[test]
fn ranges_reject_nonfinite_reversed_overflowing_and_ineffective_steps() {
    for (lower, upper) in [
        (f64::NAN, 1.0),
        (0.0, f64::INFINITY),
        (1.0, 1.0),
        (2.0, 1.0),
        (-f64::MAX, f64::MAX),
    ] {
        assert!(SliderRange::new(lower..=upper).is_err());
    }
    let range = SliderRange::new(1.0..=60.0).unwrap();
    for step in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MIN_POSITIVE] {
        assert!(range.with_step(step).is_err());
    }
    assert!(
        SliderRange::new(1e16..=1e16 + 100.0)
            .unwrap()
            .with_step(0.1)
            .is_err()
    );
    assert!(
        SliderRange::new(-9e15..=9e15)
            .unwrap()
            .with_step(1.0)
            .is_err(),
        "the step index must retain integer precision across the entire span"
    );
    assert!(range.with_step(1.0).is_ok());
    assert!(
        range.with_step(100.0).is_ok(),
        "a coarse range still has both endpoints"
    );
}

#[test]
fn track_click_drag_capture_and_release_update_the_binding() {
    let (runtime, view) = setup(
        SliderRange::new(1.0..=60.0)
            .unwrap()
            .with_step(1.0)
            .unwrap(),
        10.0,
    );
    click(&runtime, 10.0);
    near(view.value.get(), 1.0);
    click(&runtime, 210.0);
    near(view.value.get(), 60.0);
    runtime.pointer_pressed(110.0, 16.0);
    near(view.value.get(), 31.0);
    runtime.pointer_moved(-100.0, 16.0, false);
    near(view.value.get(), 1.0);
    runtime.pointer_moved(400.0, 16.0, false);
    near(view.value.get(), 60.0);
    runtime.pointer_released(400.0, 16.0);
    runtime.pointer_moved(10.0, 16.0, false);
    near(view.value.get(), 60.0);
}

#[test]
fn pressing_the_thumb_keeps_the_grab_offset_without_a_jump() {
    let (runtime, view) = setup(SliderRange::new(0.0..=100.0).unwrap(), 50.0);
    runtime.pointer_pressed(115.0, 16.0);
    near(view.value.get(), 50.0);
    runtime.pointer_moved(135.0, 16.0, false);
    near(view.value.get(), 60.0);
    runtime.pointer_released(135.0, 16.0);
}

#[test]
fn arrows_pages_home_end_reach_both_endpoints_of_nondividing_steps() {
    let (runtime, view) = setup(
        SliderRange::new(0.0..=1.0).unwrap().with_step(0.3).unwrap(),
        0.0,
    );
    click(&runtime, 10.0);
    for expected in [0.3, 0.6, 0.9, 1.0, 1.0] {
        key(&runtime, Key::Right);
        near(view.value.get(), expected);
    }
    for expected in [0.9, 0.6, 0.3, 0.0, 0.0] {
        key(&runtime, Key::Left);
        near(view.value.get(), expected);
    }
    key(&runtime, Key::End);
    near(view.value.get(), 1.0);
    key(&runtime, Key::Home);
    near(view.value.get(), 0.0);
    key(&runtime, Key::PageUp);
    near(view.value.get(), 1.0);
    key(&runtime, Key::PageDown);
    near(view.value.get(), 0.0);
    key(&runtime, Key::Up);
    near(view.value.get(), 0.3);
    key(&runtime, Key::Down);
    near(view.value.get(), 0.0);
}

#[test]
fn right_to_left_mirrors_pointer_and_horizontal_keys_only() {
    let (runtime, view) = setup(
        SliderRange::new(1.0..=60.0)
            .unwrap()
            .with_step(1.0)
            .unwrap(),
        10.0,
    );
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    frame(&runtime, view);
    click(&runtime, 10.0);
    near(view.value.get(), 60.0);
    key(&runtime, Key::Right);
    near(view.value.get(), 59.0);
    key(&runtime, Key::Up);
    near(view.value.get(), 60.0);
    key(&runtime, Key::Home);
    near(view.value.get(), 1.0);
    key(&runtime, Key::Left);
    near(view.value.get(), 2.0);
    click(&runtime, 219.0);
    near(view.value.get(), 1.0);
}

#[test]
fn disabled_and_escape_stop_writes_including_mid_gesture() {
    let (runtime, view) = setup(SliderRange::new(0.0..=100.0).unwrap(), 50.0);
    runtime.pointer_pressed(110.0, 16.0);
    runtime.pointer_moved(130.0, 16.0, false);
    near(view.value.get(), 60.0);
    key(&runtime, Key::Escape);
    runtime.pointer_moved(170.0, 16.0, false);
    near(view.value.get(), 60.0);
    runtime.pointer_released(170.0, 16.0);
    view.disabled.set(true);
    frame(&runtime, view);
    click(&runtime, 210.0);
    near(view.value.get(), 60.0);
    assert!(!runtime.key_stroke(KeyPattern::key(Key::End)).handled);
    assert!(runtime.focused().is_none());
    view.disabled.set(false);
    frame(&runtime, view);
    runtime.pointer_pressed(50.0, 16.0);
    near(view.value.get(), 20.0);
    view.disabled.set(true);
    frame(&runtime, view);
    runtime.pointer_moved(170.0, 16.0, false);
    runtime.pointer_released(170.0, 16.0);
    near(view.value.get(), 20.0);
}

#[test]
fn paint_never_rewrites_external_or_nonfinite_values() {
    use std::cell::Cell;
    let writes = Rc::new(Cell::new(0));
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -10.0,
        150.0,
        33.0,
    ] {
        let runtime = Runtime::new();
        let counted = Rc::clone(&writes);
        let binding = Binding::new(move || value, move |_| counted.set(counted.get() + 1));
        let view = slider(
            binding,
            SliderRange::new(0.0..=100.0)
                .unwrap()
                .with_step(10.0)
                .unwrap(),
        );
        let laid = runtime.layout(
            &view,
            Proposal::exact(Size {
                width: 220.0,
                height: 32.0,
            }),
        );
        assert!(!laid.display.is_empty());
        for command in laid.display.iter() {
            if let bunny_ui::layout::DrawCommand::FillRect { rect, .. } = command {
                assert!(rect.origin.x.is_finite() && rect.origin.y.is_finite());
                assert!(rect.size.width.is_finite() && rect.size.height.is_finite());
            }
        }
    }
    assert_eq!(writes.get(), 0);
}

#[test]
fn keyboard_reads_latest_binding_and_geometry_tracks_resize() {
    let (runtime, view) = setup(
        SliderRange::new(0.0..=100.0)
            .unwrap()
            .with_step(1.0)
            .unwrap(),
        0.0,
    );
    click(&runtime, 10.0);
    view.value.set(79.0);
    key(&runtime, Key::Right);
    near(view.value.get(), 80.0);
    let value = State::new(0.0);
    let view = slider(value.binding(), SliderRange::new(0.0..=100.0).unwrap());
    let runtime = Runtime::new();
    runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 420.0,
            height: 32.0,
        }),
    );
    click(&runtime, 210.0);
    near(value.get(), 50.0);
    click(&runtime, 410.0);
    near(value.get(), 100.0);
}

#[test]
fn thumb_press_preserves_an_unsnapped_external_value_and_keyboard_remains_numeric() {
    let (runtime, view) = setup(
        SliderRange::new(0.0..=100.0)
            .unwrap()
            .with_step(10.0)
            .unwrap(),
        33.0,
    );
    runtime.pointer_pressed(76.0, 16.0);
    near(view.value.get(), 33.0);
    runtime.pointer_released(76.0, 16.0);
    assert!(
        !runtime.focus_takes_text(),
        "a slider must not summon text input"
    );
    key(&runtime, Key::Right);
    near(view.value.get(), 40.0);
    key(&runtime, Key::Left);
    near(view.value.get(), 30.0);
}

#[test]
fn touch_capture_cancels_without_late_motion_or_a_commit_on_release() {
    let (runtime, view) = setup(SliderRange::new(0.0..=100.0).unwrap(), 50.0);
    runtime.touch_began(41, 110.0, 16.0, 1);
    runtime.touch_moved(41, 150.0, 16.0);
    near(view.value.get(), 70.0);
    runtime.touch_cancelled(41);
    runtime.touch_moved(41, 200.0, 16.0);
    runtime.touch_ended(41, 200.0, 16.0);
    near(view.value.get(), 70.0);
}

#[test]
fn a_small_span_at_a_large_offset_still_moves_by_keyboard() {
    let (runtime, view) = setup(SliderRange::new(1e16..=1e16 + 100.0).unwrap(), 1e16);
    click(&runtime, 10.0);
    key(&runtime, Key::Right);
    assert!(
        view.value.get() > 1e16,
        "one percent must not round to the current value"
    );
    key(&runtime, Key::End);
    key(&runtime, Key::Left);
    assert!(view.value.get() < 1e16 + 100.0);
}

#[test]
fn a_disabled_slider_declines_named_focus() {
    let (runtime, view) = setup(SliderRange::new(1.0..=60.0).unwrap(), 10.0);
    assert!(runtime.focus_named("rate"));
    assert!(runtime.focused().is_some());
    view.disabled.set(true);
    frame(&runtime, view);
    runtime.blur();
    assert!(!runtime.focus_named("rate"));
    assert!(!runtime.key_stroke(KeyPattern::key(Key::Right)).handled);
    near(view.value.get(), 10.0);
}

#[test]
fn retained_frames_move_the_thumb_after_an_external_value_change() {
    let (runtime, view) = setup(SliderRange::new(0.0..=100.0).unwrap(), 0.0);
    let thumb = |display: &bunny_ui::layout::DisplayList| {
        display
            .iter()
            .find_map(|command| match command {
                bunny_ui::layout::DrawCommand::FillRect { rect, .. }
                    if (rect.size.height - 16.0).abs() < 0.01 =>
                {
                    Some(rect.origin.x)
                }
                _ => None,
            })
            .unwrap()
    };
    let size = Size {
        width: 220.0,
        height: 32.0,
    };
    let before = thumb(&runtime.display_frame(&view, size));
    view.value.set(100.0);
    let after = thumb(&runtime.display_frame(&view, size));
    near(after - before, 200.0);
}

#[test]
fn repeated_endpoint_keys_do_not_emit_redundant_binding_writes() {
    use std::cell::Cell;
    let value = State::new(100.0);
    let writes = Rc::new(Cell::new(0));
    let counted = Rc::clone(&writes);
    let binding = Binding::new(
        move || value.get(),
        move |next| {
            counted.set(counted.get() + 1);
            value.set(next);
        },
    );
    let control = slider(binding, SliderRange::new(0.0..=100.0).unwrap());
    let runtime = Runtime::new();
    let layout = runtime.layout(
        &control,
        Proposal::exact(Size {
            width: 220.0,
            height: 32.0,
        }),
    );
    runtime.focus(&layout.customs[0].path);
    for _ in 0..4 {
        key(&runtime, Key::Right);
        key(&runtime, Key::End);
    }
    assert_eq!(writes.get(), 0);
    key(&runtime, Key::Left);
    near(value.get(), 99.0);
    assert_eq!(writes.get(), 1);
}

#[test]
fn captured_escape_is_delivered_once_and_cancels_a_risen_parent_click() {
    use std::cell::Cell;
    #[derive(Clone)]
    struct BoxInput {
        escapes: Rc<Cell<u32>>,
        ups: Rc<Cell<u32>>,
        handles: bool,
    }
    impl CustomElement for BoxInput {
        fn paint(&self, _: &PaintCtx, _: &mut Painter) {}
        fn accepts_keys(&self) -> bool {
            true
        }
        fn event(&self, event: &ElementEvent, _: &EventCtx) -> Response {
            match event {
                ElementEvent::PointerDown { .. } => Response::handled_rising(),
                ElementEvent::PointerUp { .. } => {
                    self.ups.set(self.ups.get() + 1);
                    Response::handled()
                }
                ElementEvent::Key(stroke) if stroke.pattern.key == Key::Escape => {
                    self.escapes.set(self.escapes.get() + 1);
                    if self.handles {
                        Response::handled()
                    } else {
                        Response::ignored()
                    }
                }
                _ => Response::ignored(),
            }
        }
    }
    #[derive(Clone)]
    struct Pane {
        input: BoxInput,
        clicks: Rc<Cell<u32>>,
    }
    impl Component for Pane {
        fn body(self) -> impl View {
            custom(self.input)
                .frame(100.0, 40.0)
                .on_click(move || self.clicks.set(self.clicks.get() + 1))
                .id("pane")
        }
    }
    for handles in [false, true] {
        let escapes = Rc::new(Cell::new(0));
        let ups = Rc::new(Cell::new(0));
        let clicks = Rc::new(Cell::new(0));
        let pane = Pane {
            input: BoxInput {
                escapes: Rc::clone(&escapes),
                ups: Rc::clone(&ups),
                handles,
            },
            clicks: Rc::clone(&clicks),
        };
        let runtime = Runtime::new();
        let layout = runtime.layout(
            &pane,
            Proposal::exact(Size {
                width: 100.0,
                height: 40.0,
            }),
        );
        if !handles {
            runtime.focus(&layout.customs[0].path);
        }
        runtime.pointer_pressed(10.0, 10.0);
        assert_eq!(
            runtime.key_stroke(KeyPattern::key(Key::Escape)).handled,
            handles
        );
        assert_eq!(escapes.get(), 1);
        runtime.pointer_released(10.0, 10.0);
        assert_eq!(clicks.get(), u32::from(!handles));
        assert_eq!(ups.get(), u32::from(!handles));
    }
}

#[test]
fn named_focus_rejects_ambiguous_groups_and_prefers_an_exact_field() {
    #[derive(Clone, Copy)]
    struct Pair {
        value: State<f64>,
        input: State<String>,
        with_field: bool,
    }
    impl Component for Pair {
        fn body(self) -> impl View {
            let range = SliderRange::new(0.0..=1.0).unwrap();
            vstack((
                hstack((
                    slider(self.value.binding(), range),
                    slider(self.value.binding(), range),
                ))
                .id("group"),
                self.with_field
                    .then(move || text_field("Name", self.input.binding()).id("group")),
            ))
        }
    }
    for with_field in [false, true] {
        let runtime = Runtime::new();
        let page = Pair {
            value: State::new(0.5),
            input: State::new(String::new()),
            with_field,
        };
        runtime.layout(
            &page,
            Proposal::exact(Size {
                width: 440.0,
                height: 80.0,
            }),
        );
        assert_eq!(runtime.focus_named("group"), with_field);
        assert_eq!(runtime.focus_takes_text(), with_field);
        assert!(!runtime.focus_named("missing"));
    }
}

#[test]
fn element_mode_canvas_island_can_be_focused_by_name() {
    let runtime = Runtime::new();
    let page = Control {
        value: State::new(10.0),
        disabled: State::new(false),
        range: SliderRange::new(1.0..=60.0)
            .unwrap()
            .with_step(1.0)
            .unwrap(),
    };
    runtime.dom_frame(
        &page,
        Size {
            width: 220.0,
            height: 32.0,
        },
    );
    assert!(runtime.focus_named("rate"));
    key(&runtime, Key::Right);
    near(page.value.get(), 11.0);
    assert!(!runtime.focus_takes_text());
}
