use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 180.0,
};

#[derive(Clone, Copy)]
struct Form {
    value: State<String>,
    presses: State<usize>,
}

impl Form {
    fn new() -> Self {
        Self {
            value: State::new(String::new()),
            presses: State::new(0),
        }
    }
}

impl Component for Form {
    fn body(self) -> impl View {
        vstack!(
            text_field("First", self.value.binding()).id("first"),
            button(text("Save"), move || self.presses.add(1)).id("save"),
            text_field("Last", self.value.binding()).id("last"),
        )
    }
}

fn focused(runtime: &Runtime, name: &str) {
    let path = runtime.focused().expect("a control owns the keyboard");
    assert!(
        path.split('/').any(|part| part == format!("[{name}]")),
        "{path} does not name {name}"
    );
}

fn tab(runtime: &Runtime, reverse: bool) {
    let mut stroke = KeyPattern::key(Key::Tab);
    stroke.shift = reverse;
    assert!(
        runtime.key_stroke(stroke).handled,
        "Tab must reach a control"
    );
}

#[test]
fn named_button_focus_activates_once_without_becoming_a_text_editor() {
    let runtime = Runtime::new();
    let form = Form::new();
    runtime.display_frame(&form, SIZE);
    assert!(runtime.focus_named("save"));
    focused(&runtime, "save");
    assert!(!runtime.focus_takes_text());
    assert!(runtime.key_stroke(KeyPattern::key(Key::Enter)).handled);
    assert_eq!(form.presses.get(), 1);
    runtime.display_frame(&form, SIZE);
    focused(&runtime, "save");
    assert!(runtime.key_stroke(KeyPattern::key(Key::Char(' '))).handled);
    assert_eq!(form.presses.get(), 2);
}

#[test]
fn tab_follows_mixed_reading_order_and_wraps_both_ways() {
    let runtime = Runtime::new();
    let form = Form::new();
    runtime.display_frame(&form, SIZE);
    for name in ["first", "save", "last", "first"] {
        tab(&runtime, false);
        focused(&runtime, name);
        runtime.display_frame(&form, SIZE);
    }
    for name in ["last", "save", "first", "last"] {
        tab(&runtime, true);
        focused(&runtime, name);
        runtime.display_frame(&form, SIZE);
    }
    assert_eq!(form.presses.get(), 0);
}

#[test]
fn separate_runtime_scenes_keep_their_own_keyboard_order_and_callbacks() {
    let first = Runtime::scene("first-window");
    let second = Runtime::scene("second-window");
    let a = Form::new();
    let b = Form::new();
    first.display_frame(&a, SIZE);
    second.display_frame(&b, SIZE);
    assert!(first.focus_named("save"));
    tab(&second, true);
    focused(&second, "last");
    assert!(first.key_stroke(KeyPattern::key(Key::Enter)).handled);
    assert_eq!(a.presses.get(), 1);
    assert_eq!(b.presses.get(), 0);
    tab(&first, false);
    focused(&first, "last");
}

#[test]
fn permanently_clipped_controls_do_not_take_invisible_keyboard_focus() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = vstack!(
        button(text("Visible"), || {}).id("visible"),
        vstack!(
            button(text("Hidden"), || {})
                .frame(100.0, 30.0)
                .id("hidden-button"),
            text_field("Hidden", value.binding())
                .frame(100.0, 30.0)
                .id("hidden-field"),
        )
        .frame(100.0, 0.0)
        .clipped(),
    );
    runtime.display_frame(&view, SIZE);
    assert!(!runtime.focus_named("hidden-button"));
    assert!(!runtime.focus_named("hidden-field"));
    for reverse in [false, false, true] {
        tab(&runtime, reverse);
        focused(&runtime, "visible");
    }
}

#[derive(Clone, Copy)]
struct Choices {
    checked: State<bool>,
    disabled: State<bool>,
    presses: State<usize>,
}
impl Component for Choices {
    fn body(self) -> impl View {
        vstack!(
            checkbox(text("Reimbursable"), self.checked.binding())
                .disabled(self.disabled.get())
                .id("reimburse"),
            button(text("Save"), move || self.presses.add(1))
                .disabled(self.disabled.get())
                .id("save"),
            button(text("Cancel"), || {}).id("cancel"),
        )
    }
}

#[test]
fn checkbox_has_real_state_and_disabled_controls_reject_every_activation_road() {
    use bunny_ui::accessibility::{Action, Role};
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let form = Choices {
        checked: State::new(false),
        disabled: State::new(false),
        presses: State::new(0),
    };
    runtime.display_frame(&form, SIZE);
    let tree = runtime.accessibility_tree();
    let check = tree
        .nodes()
        .iter()
        .find(|n| n.role == Role::Checkbox)
        .unwrap();
    assert_eq!(check.label.as_ref(), "Reimbursable");
    assert_eq!(check.checked, Some(false));
    assert!(check.enabled);
    let id = check.id;
    let at = check.bounds.origin;
    assert!(runtime.accessibility_action(id, Action::Focus).is_ok());
    assert!(!runtime.key_stroke(KeyPattern::key(Key::Enter)).handled);
    assert!(runtime.key_stroke(KeyPattern::key(Key::Char(' '))).handled);
    assert!(form.checked.get());
    runtime.display_frame(&form, SIZE);
    assert_eq!(
        runtime.accessibility_tree().node(id).unwrap().checked,
        Some(true)
    );
    assert!(runtime.accessibility_tree().node(id).unwrap().focused);
    form.checked.set(false);
    runtime.display_frame(&form, SIZE);
    assert_eq!(
        runtime.accessibility_tree().node(id).unwrap().checked,
        Some(false)
    );
    runtime.pointer_pressed(at.x + 2.0, at.y + 2.0);
    form.disabled.set(true);
    runtime.display_frame(&form, SIZE);
    runtime.pointer_released(at.x + 2.0, at.y + 2.0);
    assert!(
        !form.checked.get(),
        "disabling during a press cancels its action"
    );
    assert!(!runtime.focus_named("reimburse"));
    assert!(!runtime.focus_named("save"));
    assert!(runtime.focused().is_none());
    assert!(runtime.accessibility_action(id, Action::Activate).is_err());
    assert!(runtime.accessibility_action(id, Action::Focus).is_err());
    let tree = runtime.accessibility_tree();
    assert!(!tree.node(id).unwrap().enabled);
    assert_eq!(tree.node(id).unwrap().checked, Some(false));
    tab(&runtime, false);
    focused(&runtime, "cancel");
    tab(&runtime, false);
    focused(&runtime, "cancel");
    assert_eq!(form.presses.get(), 0);
}

#[test]
fn tab_reveals_all_twelve_rows_and_reverse_walk_returns_to_the_top() {
    #[derive(Clone, Copy)]
    struct Row(usize);
    impl Component for Row {
        fn body(self) -> impl View {
            button(text(format!("Row {}", self.0)), || {}).frame(160.0, 40.0)
        }
    }
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let view = scroll(
        vstack(
            (0..12)
                .map(|row| Row(row).id(format!("row-{row}")))
                .collect::<Vec<_>>(),
        )
        .spacing(0.0),
    )
    .frame(200.0, 80.0)
    .id("rows");
    let size = Size {
        width: 200.0,
        height: 80.0,
    };
    runtime.display_frame(&view, size);
    for row in 0..12 {
        tab(&runtime, false);
        focused(&runtime, &format!("row-{row}"));
        let laid = runtime.layout(&view, Proposal::exact(size));
        let held = runtime.focused().unwrap();
        let hit = laid
            .hits
            .iter()
            .find(|(path, _)| path == &held)
            .unwrap_or_else(|| panic!("focused row {row} is visible: {laid:?}"));
        assert!(hit.1.origin.y >= 0.0 && hit.1.origin.y + hit.1.size.height <= 80.0);
    }
    for row in (0..11).rev() {
        tab(&runtime, true);
        focused(&runtime, &format!("row-{row}"));
        runtime.display_frame(&view, size);
    }
    let laid = runtime.layout(&view, Proposal::exact(size));
    assert!(
        laid.hits
            .iter()
            .any(|(path, bounds)| path.contains("[row-0]") && bounds.origin.y >= 0.0)
    );
}

#[derive(Clone, Copy)]
struct Popup {
    open: State<bool>,
    presses: State<usize>,
}
impl Component for Popup {
    fn body(self) -> impl View {
        button(text("Choose"), move || self.open.set(true))
            .id("opener")
            .popover(self.open.binding(), Side::Bottom, move |_| {
                erased(vstack!(
                    button(text("First"), move || {
                        self.presses.add(1);
                        self.open.set(false);
                    })
                    .id("choice"),
                    button(text("Last"), || {}).id("other"),
                ))
            })
    }
}

#[test]
fn popup_traps_the_tab_walk_and_returns_focus_to_its_opener() {
    let runtime = Runtime::new();
    let view = Popup {
        open: State::new(false),
        presses: State::new(0),
    };
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("opener"));
    assert!(runtime.key_stroke(KeyPattern::key(Key::Enter)).handled);
    runtime.display_frame(&view, SIZE);
    assert!(!runtime.focus_named("opener"));
    assert!(runtime.focus_named("choice"));
    tab(&runtime, true);
    focused(&runtime, "other");
    tab(&runtime, false);
    focused(&runtime, "choice");
    assert!(runtime.key_stroke(KeyPattern::key(Key::Enter)).handled);
    runtime.display_frame(&view, SIZE);
    assert_eq!(view.presses.get(), 1);
    focused(&runtime, "opener");
}

#[test]
fn a_field_strategy_keeps_its_tab_until_it_declines() {
    use bunny_ui::text_input::{CaretState, EditingStrategy};
    struct Policy(State<bool>);
    impl EditingStrategy for Policy {
        fn takes_text(&self) -> bool {
            true
        }
        fn key(
            &self,
            stroke: &bunny_ui::action::Stroke,
            _: &mut String,
            _: &mut CaretState,
        ) -> bool {
            self.0.get() && stroke.pattern.key == Key::Tab
        }
    }
    let runtime = Runtime::new();
    let holds_tab = State::new(true);
    let value = State::new(String::new());
    let view = vstack!(
        text_field("Editor", value.binding())
            .editing_strategy(Some(Rc::new(Policy(holds_tab))))
            .id("editor"),
        button(text("Next"), || {}).id("next"),
    );
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("editor"));
    tab(&runtime, false);
    focused(&runtime, "editor");
    holds_tab.set(false);
    tab(&runtime, false);
    focused(&runtime, "next");
}

#[test]
fn focus_ring_is_retained_without_a_caret_timer_and_pointer_focus_matches_keyboard_focus() {
    use bunny_ui::layout::DrawCommand;
    let runtime = Runtime::new();
    let form = Form::new();
    let initial = runtime.layout(&form, Proposal::exact(SIZE));
    let (path, bounds) = initial
        .hits
        .iter()
        .find(|(path, _)| path.ends_with("[save]"))
        .unwrap();
    let path = path.clone();
    runtime.pointer_pressed(bounds.origin.x + 2.0, bounds.origin.y + 2.0);
    runtime.pointer_released(bounds.origin.x + 2.0, bounds.origin.y + 2.0);
    focused(&runtime, "save");
    assert_eq!(form.presses.get(), 1);
    let focused_frame = runtime.layout(&form, Proposal::exact(SIZE));
    assert!(!runtime.blink(), "a button never starts caret polling");
    assert!(focused_frame.display.iter().any(|command| matches!(command,
        DrawCommand::StrokeRect { color, width: 2.0, .. } if *color == bunny_ui::theme::current().focus)));
    let retained = runtime.layout(&form, Proposal::exact(SIZE));
    assert_eq!(
        focused_frame.display.as_slice(),
        retained.display.as_slice()
    );
    runtime.render_full(&form);
    let full = runtime.layout(&form, Proposal::exact(SIZE));
    assert_eq!(retained.display.as_slice(), full.display.as_slice());
    assert_eq!(runtime.focused(), Some(path));
}

#[test]
fn keyed_reorder_follows_the_control_and_removal_releases_it() {
    #[derive(Clone, Copy)]
    struct Rows(State<Vec<u32>>);
    impl Component for Rows {
        fn body(self) -> impl View {
            vstack(for_each(
                self.0,
                |row| row.to_string(),
                |row| button(text(format!("Row {row}")), || {}).id(format!("row-{row}")),
            ))
        }
    }
    let runtime = Runtime::new();
    let view = Rows(State::new(vec![1, 2, 3]));
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("row-2"));
    let identity = runtime.focused();
    view.0.set(vec![3, 2, 1]);
    runtime.display_frame(&view, SIZE);
    assert_eq!(runtime.focused(), identity);
    tab(&runtime, false);
    focused(&runtime, "row-1");
    view.0.set(vec![3, 2]);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focused().is_none());
    assert!(!runtime.focus_named("row-1"));
    tab(&runtime, false);
    focused(&runtime, "row-3");
}

#[test]
fn named_focus_reveals_a_control_through_nested_scroll_regions() {
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let view = scroll(vstack!(
        text("Top").frame(200.0, 200.0),
        scroll(vstack!(
            text("Inner").frame(200.0, 200.0),
            button(text("Destination"), || {}).id("destination"),
        ))
        .frame(200.0, 80.0)
        .id("inner"),
    ))
    .frame(220.0, 100.0)
    .id("outer");
    let size = Size {
        width: 220.0,
        height: 100.0,
    };
    runtime.display_frame(&view, size);
    assert!(runtime.focus_named("destination"));
    let laid = runtime.layout(&view, Proposal::exact(size));
    let held = runtime.focused().unwrap();
    let (_, visible) = laid
        .hits
        .iter()
        .find(|(path, _)| path == &held)
        .expect("both ancestors reveal the control");
    assert!(visible.origin.y >= 0.0 && visible.origin.y + visible.size.height <= size.height);
}

#[cfg(feature = "canvas")]
#[test]
fn custom_slider_joins_the_order_and_retains_its_own_keys() {
    let runtime = Runtime::new();
    let value = State::new(5.0);
    let view = vstack!(
        button(text("Before"), || {}).id("before"),
        slider(
            value.binding(),
            SliderRange::new(0.0..=10.0)
                .unwrap()
                .with_step(1.0)
                .unwrap()
        )
        .id("range"),
        button(text("After"), || {}).id("after"),
    );
    runtime.display_frame(&view, SIZE);
    tab(&runtime, false);
    focused(&runtime, "before");
    tab(&runtime, false);
    focused(&runtime, "range");
    assert!(runtime.key_stroke(KeyPattern::key(Key::Right)).handled);
    assert_eq!(value.get(), 6.0);
    tab(&runtime, false);
    focused(&runtime, "after");
    tab(&runtime, true);
    focused(&runtime, "range");
}

#[test]
fn a_sheet_blocks_background_named_focus_and_restores_the_previous_owner() {
    #[derive(Clone, Copy)]
    struct Sheet(State<bool>);
    impl Component for Sheet {
        fn body(self) -> impl View {
            button(text("Open"), move || self.0.set(true))
                .id("open")
                .sheet(self.0.binding(), move |_| {
                    erased(button(text("Close"), move || self.0.set(false)).id("close"))
                })
        }
    }
    let runtime = Runtime::new();
    let view = Sheet(State::new(false));
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("open"));
    runtime.key_stroke(KeyPattern::key(Key::Enter));
    runtime.display_frame(&view, SIZE);
    assert!(!runtime.focus_named("open"));
    tab(&runtime, false);
    focused(&runtime, "close");
    tab(&runtime, true);
    focused(&runtime, "close");
    runtime.key_stroke(KeyPattern::key(Key::Enter));
    runtime.display_frame(&view, SIZE);
    focused(&runtime, "open");
}

#[cfg(feature = "canvas")]
#[test]
fn disabling_a_focused_custom_control_releases_keyboard_ownership() {
    #[derive(Clone, Copy)]
    struct Range {
        disabled: State<bool>,
        value: State<f64>,
    }
    impl Component for Range {
        fn body(self) -> impl View {
            slider(self.value.binding(), SliderRange::new(0.0..=10.0).unwrap())
                .disabled(self.disabled.get())
                .id("range")
        }
    }
    let runtime = Runtime::new();
    let view = Range {
        disabled: State::new(false),
        value: State::new(5.0),
    };
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("range"));
    view.disabled.set(true);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focused().is_none());
    assert!(!runtime.key_stroke(KeyPattern::key(Key::Right)).handled);
    assert_eq!(view.value.get(), 5.0);
}

#[test]
fn background_auto_focus_cannot_take_the_keyboard_from_a_popup() {
    #[derive(Clone, Copy)]
    struct Page {
        value: State<String>,
        beat: State<u64>,
        open: State<bool>,
    }
    impl Component for Page {
        fn body(self) -> impl View {
            vstack!(
                text_field("Editor", self.value.binding())
                    .auto_focus_beat(self.beat.get())
                    .id("editor"),
                button(text("Open"), move || self.open.set(true))
                    .id("open")
                    .popover(self.open.binding(), Side::Bottom, |_| {
                        erased(button(text("Choice"), || {}).id("choice"))
                    }),
            )
        }
    }
    let runtime = Runtime::new();
    let view = Page {
        value: State::new(String::new()),
        beat: State::new(1),
        open: State::new(false),
    };
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("open"));
    runtime.key_stroke(KeyPattern::key(Key::Enter));
    runtime.display_frame(&view, SIZE);
    view.beat.set(2);
    runtime.display_frame(&view, SIZE);
    focused(&runtime, "choice");
    tab(&runtime, false);
    focused(&runtime, "choice");
    assert_eq!(view.value.get(), "");
}

#[cfg(feature = "canvas")]
#[test]
fn a_custom_editor_can_consume_tab_then_release_it_to_the_framework() {
    struct Editor(State<bool>);
    impl CustomElement for Editor {
        fn paint(&self, _: &PaintCtx, _: &mut Painter) {}
        fn accepts_keys(&self) -> bool {
            true
        }
        fn event(&self, event: &ElementEvent, _: &EventCtx) -> Response {
            if matches!(event, ElementEvent::Key(stroke) if stroke.pattern.key == Key::Tab)
                && self.0.get()
            {
                Response::handled()
            } else {
                Response::ignored()
            }
        }
    }
    let held = State::new(true);
    let runtime = Runtime::new();
    let view = vstack!(
        custom(Editor(held)).frame(200.0, 40.0).id("editor"),
        button(text("Next"), || {}).id("next"),
    );
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("editor"));
    tab(&runtime, false);
    focused(&runtime, "editor");
    held.set(false);
    tab(&runtime, false);
    focused(&runtime, "next");
}
