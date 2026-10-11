#![cfg(feature = "canvas")]
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::accessibility::{Action, ActionError, Node, Role};
use bunny_ui::prelude::*;

const SIZE: Size = Size {
    width: 240.0,
    height: 100.0,
};

#[derive(Clone, Copy)]
struct RangeForm {
    value: State<f64>,
    disabled: State<bool>,
    shown: State<bool>,
    modal: State<bool>,
    label: State<String>,
    range: SliderRange,
}
impl Component for RangeForm {
    fn body(self) -> impl View {
        vstack!(if self.shown.get() {
            erased(
                slider(self.value.binding(), self.range)
                    .disabled(self.disabled.get())
                    .accessibility_label(self.label)
                    .frame(220.0, 32.0)
                    .id("rate"),
            )
        } else {
            erased(text("").accessibility_hidden())
        })
        .sheet(self.modal.binding(), |_| erased(text("Modal content")))
    }
}
fn setup(range: SliderRange, value: f64) -> (Runtime, RangeForm) {
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let form = RangeForm {
        value: State::new(value),
        disabled: State::new(false),
        shown: State::new(true),
        modal: State::new(false),
        label: State::new("Refresh rate".into()),
        range,
    };
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&form, SIZE);
    (runtime, form)
}
fn node(runtime: &Runtime) -> Node {
    runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|node| node.role == Role::Slider)
        .expect("exposed slider")
        .clone()
}
fn stepped() -> SliderRange {
    SliderRange::new(0.0..=1.0).unwrap().with_step(0.3).unwrap()
}
fn near(value: f64, expected: f64) {
    assert!((value - expected).abs() < 1e-12, "{value} != {expected}");
}

#[test]
fn range_snapshot_names_bounds_values_and_identity_follow_the_binding() {
    let (runtime, form) = setup(stepped(), 0.6);
    let before = node(&runtime);
    assert_eq!(before.label.as_ref(), "Refresh rate");
    assert!(before.enabled);
    assert!(!before.focused);
    assert!(before.value.is_none());
    let range = before.range.unwrap();
    near(range.value(), 0.6);
    near(range.minimum(), 0.0);
    near(range.maximum(), 1.0);
    near(range.small_change(), 0.3);
    near(range.large_change(), 1.0);
    form.label.set("Polling interval".into());
    form.value.set(0.9);
    runtime.display_frame(&form, SIZE);
    let after = node(&runtime);
    assert_eq!(after.id, before.id);
    assert_eq!(after.label.as_ref(), "Polling interval");
    near(after.range.unwrap().value(), 0.9);
}

#[test]
fn adjustments_reuse_nondividing_keyboard_steps_without_stealing_focus() {
    let (runtime, form) = setup(stepped(), 0.0);
    let id = node(&runtime).id;
    for expected in [0.3, 0.6, 0.9, 1.0, 1.0] {
        runtime.accessibility_action(id, Action::Increment).unwrap();
        near(form.value.get(), expected);
    }
    for expected in [0.9, 0.6, 0.3, 0.0, 0.0] {
        runtime.accessibility_action(id, Action::Decrement).unwrap();
        near(form.value.get(), expected);
    }
    assert!(!node(&runtime).focused);
    runtime.accessibility_action(id, Action::Focus).unwrap();
    assert!(node(&runtime).focused);
    assert!(runtime.key_stroke(KeyPattern::key(Key::Right)).handled);
    near(form.value.get(), 0.3);
    runtime.accessibility_action(id, Action::Increment).unwrap();
    near(form.value.get(), 0.6);
    assert!(node(&runtime).focused);
}

#[test]
fn numeric_sets_snap_like_pointer_input_and_reject_invalid_values_without_writes() {
    let (runtime, form) = setup(stepped(), 0.0);
    let id = node(&runtime).id;
    for (requested, expected) in [(0.14, 0.0), (0.2, 0.3), (0.99, 1.0), (1.0, 1.0)] {
        runtime
            .accessibility_action(id, Action::SetValue(requested))
            .unwrap();
        near(form.value.get(), expected);
    }
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
        assert_eq!(
            runtime.accessibility_action(id, Action::SetValue(invalid)),
            Err(ActionError::InvalidValue)
        );
        near(form.value.get(), 1.0);
    }
    assert_eq!(
        runtime.accessibility_action(id, Action::Activate),
        Err(ActionError::Unsupported)
    );
    assert_eq!(
        runtime.accessibility_action(id, Action::SetText("0.5".into())),
        Err(ActionError::Unsupported)
    );
}

#[test]
fn continuous_and_coarse_ranges_publish_finite_keyboard_increments() {
    let (runtime, form) = setup(SliderRange::new(1.0..=101.0).unwrap(), 20.0);
    let range = node(&runtime).range.unwrap();
    near(range.small_change(), 1.0);
    near(range.large_change(), 10.0);
    runtime
        .accessibility_action(node(&runtime).id, Action::SetValue(20.25))
        .unwrap();
    near(form.value.get(), 20.25);
    runtime
        .accessibility_action(node(&runtime).id, Action::Increment)
        .unwrap();
    near(form.value.get(), 21.25);
    let (runtime, _) = setup(
        SliderRange::new(0.0..=1.0)
            .unwrap()
            .with_step(f64::MAX)
            .unwrap(),
        0.0,
    );
    let range = node(&runtime).range.unwrap();
    near(range.small_change(), 1.0);
    near(range.large_change(), 1.0);
}

#[test]
fn disabled_ranges_remain_discoverable_and_reject_all_mutating_actions() {
    let (runtime, form) = setup(stepped(), 0.3);
    let id = node(&runtime).id;
    runtime.accessibility_action(id, Action::Focus).unwrap();
    form.disabled.set(true);
    runtime.display_frame(&form, SIZE);
    let disabled = node(&runtime);
    assert_eq!(disabled.id, id);
    assert!(!disabled.enabled);
    assert!(!disabled.focused);
    for action in [
        Action::Increment,
        Action::Decrement,
        Action::SetValue(0.9),
        Action::Focus,
    ] {
        assert!(!disabled.supports(&action));
        assert_eq!(
            runtime.accessibility_action(id, action),
            Err(ActionError::Unsupported)
        );
    }
    near(form.value.get(), 0.3);
}

#[test]
fn modal_exclusion_and_removal_retire_range_handles() {
    let (runtime, form) = setup(stepped(), 0.3);
    let first = node(&runtime).id;
    form.modal.set(true);
    runtime.display_frame(&form, SIZE);
    assert!(runtime.accessibility_tree().node(first).is_none());
    assert_eq!(
        runtime.accessibility_action(first, Action::Increment),
        Err(ActionError::Unavailable)
    );
    form.modal.set(false);
    runtime.display_frame(&form, SIZE);
    let second = node(&runtime).id;
    assert_ne!(first, second);
    form.shown.set(false);
    runtime.display_frame(&form, SIZE);
    assert_eq!(
        runtime.accessibility_action(second, Action::SetValue(1.0)),
        Err(ActionError::Unavailable)
    );
    form.shown.set(true);
    runtime.display_frame(&form, SIZE);
    assert_ne!(second, node(&runtime).id);
    near(form.value.get(), 0.3);
}

#[test]
fn range_reads_are_bounded_without_repairing_invalid_model_writes_or_scheduling_frames() {
    let (runtime, form) = setup(stepped(), 0.0);
    for (raw, bounded) in [
        (f64::NAN, 0.0),
        (f64::INFINITY, 1.0),
        (-20.0, 0.0),
        (20.0, 1.0),
    ] {
        form.value.set(raw);
        runtime.display_frame(&form, SIZE);
        near(node(&runtime).range.unwrap().value(), bounded);
        assert_eq!(form.value.get().to_bits(), raw.to_bits());
        assert!(!runtime.needs_frame());
        let before = runtime.accessibility_tree();
        assert_eq!(runtime.accessibility_tree(), before);
        assert!(!runtime.needs_frame());
    }
    runtime
        .accessibility_action(node(&runtime).id, Action::Decrement)
        .unwrap();
    near(form.value.get(), 0.9);
}

#[test]
fn range_handles_are_runtime_scoped_and_numeric_adjustment_does_not_reverse_in_rtl() {
    let (runtime, form) = setup(stepped(), 0.3);
    let id = node(&runtime).id;
    let other = Runtime::scene("other-range-window");
    other.set_accessibility_enabled(true);
    other.display_frame(&text("Other window"), SIZE);
    assert_eq!(
        other.accessibility_action(id, Action::Increment),
        Err(ActionError::Unavailable)
    );
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    runtime.display_frame(&form, SIZE);
    runtime.accessibility_action(id, Action::Increment).unwrap();
    near(form.value.get(), 0.6);
    runtime.set_accessibility_enabled(false);
    assert!(runtime.accessibility_tree().nodes().is_empty());
    assert_eq!(
        runtime.accessibility_action(id, Action::Increment),
        Err(ActionError::Unavailable)
    );
}

#[test]
fn clipped_and_hidden_ranges_follow_the_existing_exposure_rules() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let value = State::new(0.3);
    let content = vstack!(
        slider(value.binding(), stepped())
            .frame(120.0, 100.0)
            .accessibility_label("Visible"),
        slider(value.binding(), stepped())
            .frame(120.0, 100.0)
            .accessibility_label("Outside"),
    )
    .spacing(0.0);
    runtime.display_frame(
        &content,
        Size {
            width: 120.0,
            height: 80.0,
        },
    );
    let tree = runtime.accessibility_tree();
    assert_eq!(tree.nodes().len(), 1);
    assert_eq!(tree.nodes()[0].label.as_ref(), "Visible");
    assert_eq!(tree.nodes()[0].bounds.size.height, 80.0);
    let old = tree.nodes()[0].id;
    runtime.display_frame(&content.accessibility_hidden(), SIZE);
    assert!(runtime.accessibility_tree().nodes().is_empty());
    assert_eq!(
        runtime.accessibility_action(old, Action::Increment),
        Err(ActionError::Unavailable)
    );
}
