//! Inline forms keep a return address across pointer and keyboard child focus.
use bunny_ui_core::{
    menu::{Edit, Pick},
    prelude::*,
    text_input::EditCommand,
};

const SIZE: Size = Size {
    width: 400.0,
    height: 360.0,
};

#[derive(Clone, Copy)]
struct Form {
    open: State<bool>,
    opener: State<bool>,
    value: State<String>,
    undos: State<usize>,
}

impl Form {
    fn new() -> Self {
        Self {
            open: State::new(false),
            opener: State::new(true),
            value: State::new(String::new()),
            undos: State::new(0),
        }
    }
}

impl Component for Form {
    fn body(self) -> impl View {
        vstack!(
            self.opener
                .get()
                .then(|| button(text("Open"), move || self.open.set(true)).id("open")),
            self.open.get().then(|| vstack!(
                text_field("Value", self.value.binding()).id("value"),
                button(text("Apply"), move || self.open.set(false)).id("apply"),
                button(text("Cancel"), move || self.open.set(false)).id("cancel"),
            )
            .id("form")
            .restore_focus_on_close()),
            button(text("Outside"), || {}).id("outside"),
        )
        .on_edit(Edit::Undo, move || self.undos.add(1))
    }
}

fn open(runtime: &Runtime, form: &Form) -> String {
    runtime.display_frame(form, SIZE);
    assert!(runtime.focus_named("open"));
    let owner = runtime.focused().unwrap();
    assert!(runtime.key_stroke(KeyPattern::key(Key::Enter)).handled);
    runtime.display_frame(form, SIZE);
    assert_eq!(
        runtime.focused().as_deref(),
        Some(owner.as_str()),
        "opening is nonmodal"
    );
    owner
}

#[test]
fn a_pointer_close_returns_to_the_invoker_and_its_edit_responder() {
    let runtime = Runtime::new();
    let form = Form::new();
    let owner = open(&runtime, &form);
    assert!(runtime.focus_named("value"));
    assert!(runtime.key(EditCommand::Insert("draft".into())).applied);
    let layout = runtime.settled_layout(&form, Proposal::exact(SIZE));
    let hit = layout
        .hits
        .iter()
        .find(|hit| hit.0.contains("[apply]"))
        .unwrap();
    let point = hit.1.origin;
    assert!(runtime.pointer_pressed(point.x + 2.0, point.y + 2.0));
    assert!(
        runtime
            .pointer_released(point.x + 2.0, point.y + 2.0)
            .is_some()
    );
    runtime.display_frame(&form, SIZE);
    assert_eq!(runtime.focused(), Some(owner));
    assert!(runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(form.undos.get(), 1);
}

#[test]
fn tab_to_cancel_keeps_the_return_address_across_reopenings() {
    let runtime = Runtime::new();
    let form = Form::new();
    for _ in 0..2 {
        let owner = open(&runtime, &form);
        assert!(runtime.focus_named("value"));
        for _ in 0..2 {
            assert!(runtime.key_stroke(KeyPattern::key(Key::Tab)).handled);
        }
        assert!(runtime.focused().unwrap().contains("[cancel]"));
        assert!(runtime.key_stroke(KeyPattern::key(Key::Enter)).handled);
        runtime.display_frame(&form, SIZE);
        assert_eq!(runtime.focused(), Some(owner));
    }
}

#[test]
fn an_outside_move_or_a_missing_invoker_is_never_overridden() {
    for outside in [true, false] {
        let runtime = Runtime::new();
        let form = Form::new();
        open(&runtime, &form);
        assert!(runtime.focus_named(if outside { "outside" } else { "value" }));
        let chosen = runtime.focused();
        form.opener.set(outside);
        form.open.set(false);
        runtime.display_frame(&form, SIZE);
        assert_eq!(runtime.focused(), if outside { chosen } else { None });
    }
}

#[test]
fn live_text_history_and_composition_do_not_reach_the_parent() {
    let runtime = Runtime::new();
    let form = Form::new();
    open(&runtime, &form);
    assert!(runtime.focus_named("value"));
    assert!(runtime.key(EditCommand::Insert("draft".into())).applied);
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(form.value.get(), "");
    assert_eq!(form.undos.get(), 0);
    assert!(
        runtime
            .key(EditCommand::SetMarked {
                text: "に".into(),
                caret_utf16: (1, 0)
            })
            .applied
    );
    runtime.display_frame(&form, SIZE);
    assert!(runtime.ime_snapshot().unwrap().marked.is_some());
    assert_eq!(form.undos.get(), 0);
    assert!(runtime.focused().unwrap().contains("[value]"));
}
