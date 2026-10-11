#![cfg(feature = "canvas")]
extern crate bunny_ui_core as bunny_ui;
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::{DisplayList, DrawCommand};
use bunny_ui::prelude::*;

fn sample(id: u64, x: f64, value: f64) -> LineSample {
    LineSample::new(id, x, value).unwrap()
}
fn series() -> LineSeries {
    LineSeries::new([
        sample(91, 0.0, 0.0),
        sample(7, 1.0, 50.0),
        sample(42, 4.0, 100.0),
    ])
    .unwrap()
}
#[derive(Clone, Copy)]
struct Plot {
    data: State<LineSeries>,
    selected: State<Option<u64>>,
}
impl Component for Plot {
    fn body(self) -> impl View {
        line_chart(move || self.data.get())
            .inspection(self.selected.binding(), |s| {
                format!("sample {} = {}", s.id(), s.value())
            })
            .id("history")
    }
}
fn setup() -> (Runtime, Plot) {
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let view = Plot {
        data: State::new(series()),
        selected: State::new(None),
    };
    frame(&runtime, view, 436.0, 236.0);
    (runtime, view)
}
fn frame(runtime: &Runtime, view: Plot, width: f64, height: f64) -> bunny_ui::layout::LayoutResult {
    runtime.layout(&view, Proposal::exact(Size { width, height }))
}
fn words(display: &DisplayList) -> String {
    display
        .iter()
        .filter_map(|c| match c {
            DrawCommand::TextLine { content, .. } => Some(content.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn points(display: &DisplayList) -> Vec<(f64, f64)> {
    display
        .iter()
        .find_map(|c| match c {
            DrawCommand::Image {
                rect,
                source: ImageSource::Path { verbs, .. },
            } => Some(
                verbs
                    .iter()
                    .map(|v| {
                        let (Verb::Move(x, y) | Verb::Line(x, y)) = v else {
                            panic!("unexpected curve verb")
                        };
                        (f64::from(*x) + rect.origin.x, f64::from(*y) + rect.origin.y)
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-5, "{a} != {b}");
}
fn key(runtime: &Runtime, k: Key) {
    assert!(runtime.key_stroke(KeyPattern::key(k)).handled);
}

#[test]
fn constructors_reject_nonfinite_disordered_duplicate_and_overflowing_data() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(LineSample::new(1, value, 0.0).is_err());
        assert!(LineSample::new(1, 0.0, value).is_err());
        assert!(ChartDomain::new(value..=1.0).is_err());
        assert!(ChartDomain::new(0.0..=value).is_err());
    }
    for bounds in [1.0..=1.0, 2.0..=1.0, -f64::MAX..=f64::MAX] {
        assert!(ChartDomain::new(bounds).is_err());
    }
    assert!(LineSeries::new([sample(1, 0.0, 0.0), sample(1, 1.0, 1.0)]).is_err());
    assert!(LineSeries::new([sample(1, 1.0, 0.0), sample(2, 1.0, 1.0)]).is_err());
    assert!(LineSeries::new([sample(1, 2.0, 0.0), sample(2, 1.0, 1.0)]).is_err());
    assert!(LineSeries::new([sample(1, -f64::MAX, 0.0), sample(2, f64::MAX, 1.0)]).is_err());
    assert!(LineSeries::new([sample(1, 0.0, -f64::MAX), sample(2, 1.0, f64::MAX)]).is_err());
    assert!(
        series()
            .with_y_domain(ChartDomain::new(0.0..=99.0).unwrap())
            .is_err()
    );
    assert!(
        series()
            .with_y_domain(ChartDomain::new(0.0..=100.0).unwrap())
            .is_ok()
    );
}

#[test]
fn empty_single_constant_and_subnormal_data_have_finite_geometry() {
    let (runtime, view) = setup();
    view.data.set(LineSeries::new([]).unwrap());
    assert!(points(&frame(&runtime, view, 436.0, 236.0).display).is_empty());
    runtime.pointer_moved(218.0, 100.0, false);
    assert_eq!(view.selected.get(), None);
    view.data
        .set(LineSeries::new([sample(8, f64::MAX, f64::MAX)]).unwrap());
    let single = frame(&runtime, view, 436.0, 236.0);
    assert!(
        single
            .display
            .iter()
            .any(|c| matches!(c, DrawCommand::FillRect { rect, .. }
        if (rect.origin.x + rect.size.width / 2.0 - 218.0).abs() < 1e-6
        && (rect.origin.y + rect.size.height / 2.0 - 118.0).abs() < 1e-6
        && rect.size.width <= 8.0))
    );
    view.data
        .set(LineSeries::new([sample(1, 0.0, 3.0), sample(2, f64::from_bits(1), 3.0)]).unwrap());
    let ps = points(&frame(&runtime, view, 436.0, 236.0).display);
    assert_eq!(ps.len(), 2);
    near(ps[0].0, 18.0);
    near(ps[1].0, 418.0);
    near(ps[0].1, 118.0);
    near(ps[1].1, 118.0);
}

#[test]
fn nearest_real_x_uses_stable_ids_and_earlier_x_on_a_tie() {
    let (runtime, view) = setup();
    runtime.pointer_moved(68.0, 100.0, false); // midpoint of x=0 and x=1
    assert_eq!(view.selected.get(), Some(91));
    runtime.pointer_moved(69.0, 100.0, false);
    assert_eq!(view.selected.get(), Some(7));
    runtime.pointer_moved(268.0, 100.0, false); // midpoint of x=1 and x=4
    assert_eq!(view.selected.get(), Some(7));
    runtime.pointer_moved(269.0, 100.0, false);
    assert_eq!(view.selected.get(), Some(42));
    assert!(words(&frame(&runtime, view, 436.0, 236.0).display).contains("sample 42 = 100"));
    runtime.pointer_exited();
    assert_eq!(view.selected.get(), None);
    assert!(words(&frame(&runtime, view, 436.0, 236.0).display).is_empty());
}

#[test]
fn keyboard_navigation_resize_and_rtl_share_current_geometry() {
    let (runtime, view) = setup();
    assert!(runtime.focus_named("history"));
    assert!(!runtime.focus_takes_text());
    key(&runtime, Key::Home);
    assert_eq!(view.selected.get(), Some(91));
    key(&runtime, Key::Right);
    assert_eq!(view.selected.get(), Some(7));
    key(&runtime, Key::End);
    assert_eq!(view.selected.get(), Some(42));
    key(&runtime, Key::Left);
    assert_eq!(view.selected.get(), Some(7));
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let ps = points(&frame(&runtime, view, 836.0, 136.0).display);
    near(ps[0].0, 818.0);
    near(ps[2].0, 18.0);
    key(&runtime, Key::Left);
    assert_eq!(view.selected.get(), Some(42));
    key(&runtime, Key::Right);
    assert_eq!(view.selected.get(), Some(7));
    runtime.pointer_moved(818.0, 70.0, false);
    assert_eq!(view.selected.get(), Some(91));
    runtime.pointer_moved(18.0, 70.0, false);
    assert_eq!(view.selected.get(), Some(42));
    key(&runtime, Key::Escape);
    assert_eq!(view.selected.get(), None);
    assert!(!runtime.key_stroke(KeyPattern::key(Key::Up)).handled);
}

#[test]
fn pointer_capture_and_touch_cancel_remove_inspection_without_a_late_commit() {
    let (runtime, view) = setup();
    runtime.touch_began(31, 118.0, 100.0, 1);
    assert_eq!(view.selected.get(), Some(7));
    runtime.touch_moved(31, 418.0, 100.0);
    assert_eq!(view.selected.get(), Some(42));
    runtime.touch_cancelled(31);
    assert_eq!(view.selected.get(), None);
    runtime.touch_moved(31, 118.0, 100.0);
    runtime.touch_ended(31, 118.0, 100.0);
    assert_eq!(view.selected.get(), None);
    runtime.pointer_pressed(118.0, 100.0);
    assert_eq!(view.selected.get(), Some(7));
    key(&runtime, Key::Escape);
    runtime.pointer_released(418.0, 100.0);
    assert_eq!(view.selected.get(), None);
}

#[test]
fn retained_updates_resolve_identity_and_hide_retired_samples_without_mutating_the_model() {
    let (runtime, view) = setup();
    let size = Size {
        width: 436.0,
        height: 236.0,
    };
    view.selected.set(Some(7));
    let before = runtime.display_frame(&view, size);
    assert!(words(&before).contains("sample 7 = 50"));
    view.data.set(
        LineSeries::new([
            sample(7, 1.0, 70.0),
            sample(42, 4.0, 100.0),
            sample(19, 6.0, 5.0),
        ])
        .unwrap(),
    );
    let after = runtime.display_frame(&view, size);
    assert!(words(&after).contains("sample 7 = 70"));
    assert_ne!(points(&before), points(&after));
    assert_eq!(view.data.get().sample(7).unwrap().value(), 70.0);
    view.data
        .set(LineSeries::new([sample(42, 4.0, 90.0), sample(19, 6.0, 5.0)]).unwrap());
    let retired = runtime.display_frame(&view, size);
    assert!(words(&retired).is_empty());
    assert_eq!(view.selected.get(), Some(7));
    assert!(view.data.get().sample(7).is_none());
}

#[test]
fn sixty_and_three_hundred_sample_series_preserve_every_visible_vertex_when_rolling() {
    let (runtime, view) = setup();
    for count in [60_u32, 300] {
        for first in [0_u32, 1, 1000] {
            view.data.set(
                LineSeries::new(
                    (first..first + count)
                        .map(|i| sample(u64::from(i), f64::from(i), f64::from(i % 100))),
                )
                .unwrap(),
            );
            let ps = points(&frame(&runtime, view, 836.0, 236.0).display);
            assert_eq!(ps.len(), usize::try_from(count).unwrap());
            near(ps[0].0, 18.0);
            near(ps.last().unwrap().0, 818.0);
            runtime.pointer_moved(818.0, 100.0, false);
            assert_eq!(view.selected.get(), Some(u64::from(first + count - 1)));
        }
    }
}

#[test]
fn sparkline_is_passive_and_tiny_viewports_do_not_emit_nonfinite_geometry() {
    let runtime = Runtime::new();
    let view = line_chart(series).sparkline().id("spark");
    let laid = runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 64.0,
            height: 24.0,
        }),
    );
    assert_eq!(points(&laid.display).len(), 3);
    near(points(&laid.display)[0].0, 2.0);
    assert!(!runtime.focus_named("spark"));
    for side in [0.0, 0.1, 1.0, 3.0] {
        let laid = runtime.layout(
            &view,
            Proposal::exact(Size {
                width: side,
                height: side,
            }),
        );
        for (x, y) in points(&laid.display) {
            assert!(x.is_finite() && y.is_finite());
        }
    }
}

#[test]
fn repeated_selection_does_not_write_again_and_paint_has_no_binding_side_effects() {
    use std::cell::Cell;
    let runtime = Runtime::new();
    let selected = State::new(Some(91));
    let writes = Rc::new(Cell::new(0));
    let counter = Rc::clone(&writes);
    let binding = Binding::new(
        move || selected.get(),
        move |next| {
            selected.set(next);
            counter.set(counter.get() + 1);
        },
    );
    let view = line_chart(series)
        .inspection(binding, |s| s.id().to_string())
        .id("chart");
    runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 436.0,
            height: 236.0,
        }),
    );
    assert_eq!(writes.get(), 0);
    runtime.focus_named("chart");
    key(&runtime, Key::Home);
    key(&runtime, Key::Left);
    runtime.pointer_moved(18.0, 100.0, false);
    assert_eq!(writes.get(), 0);
    runtime.pointer_moved(118.0, 100.0, false);
    assert_eq!(writes.get(), 1);
    runtime.pointer_exited();
    assert_eq!(writes.get(), 2);
    selected.set(Some(u64::MAX));
    runtime.display_frame(
        &view,
        Size {
            width: 436.0,
            height: 236.0,
        },
    );
    assert_eq!(writes.get(), 2);
}

#[test]
fn keyboard_and_escape_survive_the_runtime_reread_of_a_stationary_pointer() {
    let (runtime, view) = setup();
    let size = Size {
        width: 436.0,
        height: 236.0,
    };
    runtime.pointer_moved(118.0, 100.0, false);
    assert_eq!(view.selected.get(), Some(7));
    runtime.focus_named("history");
    key(&runtime, Key::End);
    let frame = runtime.display_frame(&view, size);
    assert!(words(&frame).contains("sample 42 = 100"));
    assert_eq!(view.selected.get(), Some(42));
    key(&runtime, Key::Escape);
    assert!(words(&runtime.display_frame(&view, size)).is_empty());
    assert_eq!(view.selected.get(), None);
    runtime.pointer_moved(119.0, 100.0, false);
    assert_eq!(view.selected.get(), Some(7));
}

#[test]
fn a_stationary_hover_selects_the_nearest_sample_after_rolling_data_changes() {
    let (runtime, view) = setup();
    runtime.pointer_moved(118.0, 100.0, false);
    assert_eq!(view.selected.get(), Some(7));
    view.data
        .set(LineSeries::new([sample(42, 4.0, 70.0), sample(19, 8.0, 50.0)]).unwrap());
    runtime.display_frame(
        &view,
        Size {
            width: 436.0,
            height: 236.0,
        },
    );
    assert_eq!(view.selected.get(), Some(42));
}

#[test]
fn themes_repaint_existing_plots_and_explicit_ink_remains_stable() {
    let runtime = Runtime::new();
    let chart = line_chart(series);
    let size = Size {
        width: 436.0,
        height: 236.0,
    };
    let ink = |display: &DisplayList| {
        display
            .iter()
            .find_map(|c| match c {
                DrawCommand::Image {
                    source: ImageSource::Path { ink, .. },
                    ..
                } => Some(ink.clone()),
                _ => None,
            })
            .unwrap()
    };
    theme::install(Theme::light());
    let before = ink(&runtime.display_frame(&chart, size));
    let mut next = Theme::dark();
    next.accent = Color::hex(0x0012_34ab);
    theme::install(next);
    let after = ink(&runtime.display_frame(&chart, size));
    assert_ne!(before, after);
    let runtime = Runtime::new();
    let explicit = line_chart(series).color(Color::hex(0x00ab_12cd));
    let before = ink(&runtime.display_frame(&explicit, size));
    theme::install(Theme::light());
    assert_eq!(before, ink(&runtime.display_frame(&explicit, size)));
}

#[test]
fn element_mode_canvas_island_uses_the_same_named_keyboard_inspection() {
    let runtime = Runtime::new();
    let view = Plot {
        data: State::new(series()),
        selected: State::new(None),
    };
    runtime.dom_frame(
        &view,
        Size {
            width: 436.0,
            height: 236.0,
        },
    );
    assert!(runtime.focus_named("history"));
    key(&runtime, Key::End);
    assert_eq!(view.selected.get(), Some(42));
    assert!(!runtime.focus_takes_text());
}
