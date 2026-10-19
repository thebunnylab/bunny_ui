//! Ordinary fields own their edits, even inside an application history responder.
use bunny_ui_core::{
    menu::{Edit, Pick},
    prelude::*,
    text_input::EditCommand,
};

const SIZE: Size = Size {
    width: 400.0,
    height: 240.0,
};

#[test]
fn typing_undo_redo_stays_in_the_field_across_renders() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let application = State::new(0usize);
    let view = text_field("Name", value.binding())
        .id("name")
        .on_edit(Edit::Undo, move || application.add(1));
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("name"));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    for letter in ["a", "b", "c"] {
        assert!(runtime.key(EditCommand::Insert(letter.into())).applied);
        runtime.display_frame(&view, SIZE);
    }
    assert!(runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert_eq!(value.get(), "");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(value.get(), "abc");
    assert_eq!(application.get(), 0);
}

#[test]
fn replacement_restores_the_selected_unicode_text_and_new_edit_discards_redo() {
    let runtime = Runtime::new();
    let value = State::new("Olá 🐰".to_owned());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("name"));
    runtime.key(EditCommand::SelectAll);
    runtime.key(EditCommand::Insert("New".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "Olá 🐰");
    assert_eq!(
        runtime.key(EditCommand::Copy).output.as_deref(),
        Some("Olá 🐰")
    );
    runtime.key(EditCommand::Insert("Other".into()));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Redo)));
    assert!(!runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(value.get(), "Other");
}

#[test]
fn external_value_changes_invalidate_local_history_before_another_render() {
    let runtime = Runtime::new();
    let value = State::new("initial".to_owned());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("name"));
    runtime.key(EditCommand::Insert("!".into()));
    assert!(runtime.menu_answers(Pick::Edit(Edit::Undo)));
    value.set("external".into());
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(!runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "external");
}

#[test]
fn one_composition_is_one_undo_and_cannot_be_undone_while_marked() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("name"));
    for composing in ["に", "日本"] {
        runtime.key(EditCommand::SetMarked {
            text: composing.into(),
            caret_utf16: (1, 0),
        });
        assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    }
    runtime.key(EditCommand::Insert("日本".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(value.get(), "日本");
}

#[test]
fn focus_and_navigation_split_typing_without_losing_each_fields_history() {
    let runtime = Runtime::new();
    let a = State::new(String::new());
    let b = State::new(String::new());
    let view = vstack!(
        text_field("A", a.binding()).id("a"),
        text_field("B", b.binding()).id("b")
    );
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("a"));
    runtime.key(EditCommand::Insert("a".into()));
    assert!(runtime.focus_named("b"));
    runtime.key(EditCommand::Insert("b".into()));
    assert!(runtime.focus_named("a"));
    runtime.key(EditCommand::Insert("x".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(a.get(), "a");
    assert_eq!(b.get(), "b");
    runtime.key(EditCommand::Left(false));
    runtime.key(EditCommand::Right(false));
    runtime.key(EditCommand::Insert("y".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(a.get(), "a");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(a.get(), "");
    assert!(runtime.focus_named("b"));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(b.get(), "");
}

#[test]
fn composition_cancellation_preserves_the_previous_redo() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::Insert("a".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    runtime.key(EditCommand::SetMarked {
        text: "日".into(),
        caret_utf16: (1, 0),
    });
    runtime.key(EditCommand::SetMarked {
        text: String::new(),
        caret_utf16: (0, 0),
    });
    assert_eq!(value.get(), "");
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(value.get(), "a");
}

#[test]
fn separate_workspace_scenes_have_separate_field_history() {
    let first = Runtime::scene("field-history-first");
    let second = Runtime::scene("field-history-second");
    let a = State::new(String::new());
    let b = State::new(String::new());
    let left = text_field("A", a.binding())
        .id("field")
        .on_edit(Edit::Undo, || panic!("application history must not run"));
    let right = text_field("B", b.binding())
        .id("field")
        .on_edit(Edit::Undo, || panic!("application history must not run"));
    first.display_frame(&left, SIZE);
    second.display_frame(&right, SIZE);
    first.focus_named("field");
    first.key(EditCommand::Insert("a".into()));
    second.focus_named("field");
    second.key(EditCommand::Insert("b".into()));
    assert!(first.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(a.get(), "");
    assert_eq!(b.get(), "b");
    assert!(second.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(b.get(), "");
}

#[derive(Clone, Copy)]
struct Mounted {
    value: State<String>,
    shown: State<bool>,
    nested: State<bool>,
}
impl Component for Mounted {
    fn body(self) -> impl View {
        self.shown.get().then(|| {
            if self.nested.get() {
                Either::First(vstack!(
                    text("Moved"),
                    text_field("Name", self.value.binding()).id("name")
                ))
            } else {
                Either::Second(text_field("Name", self.value.binding()).id("name"))
            }
        })
    }
}

#[test]
fn a_named_move_retains_history_but_unmount_discards_it() {
    let runtime = Runtime::new();
    let view = Mounted {
        value: State::new(String::new()),
        shown: State::new(true),
        nested: State::new(false),
    };
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::Insert("old".into()));
    view.nested.set(true);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(view.value.get(), "");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    view.shown.set(false);
    runtime.display_frame(&view, SIZE);
    view.shown.set(true);
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert_eq!(view.value.get(), "old");
}

#[test]
fn cut_and_grapheme_deletion_restore_exact_bytes_and_selection() {
    let runtime = Runtime::new();
    let value = State::new("é👩🏽‍🚀".to_owned());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::Backspace);
    assert_eq!(value.get(), "é");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "é👩🏽‍🚀");
    runtime.key(EditCommand::SelectAll);
    assert_eq!(runtime.key(EditCommand::Cut).output.as_deref(), Some("é👩🏽‍🚀"));
    assert_eq!(value.get(), "");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(
        runtime.key(EditCommand::Copy).output.as_deref(),
        Some("é👩🏽‍🚀")
    );
}

#[test]
fn history_retention_is_bounded_and_an_oversized_edit_does_not_leave_partial_undo() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    for _ in 0..140 {
        runtime.key(EditCommand::Insert("ab".into()));
    }
    let mut undone = 0;
    while runtime.menu_pick(Pick::Edit(Edit::Undo)) {
        undone += 1;
    }
    assert_eq!(undone, 128);
    assert_eq!(value.get(), "ab".repeat(12));
    runtime.key(EditCommand::SelectAll);
    runtime.key(EditCommand::Insert("x".repeat(1024 * 1024 + 1)));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Redo)));
}

#[test]
fn a_composition_evicted_by_the_budget_never_restores_a_partial_preedit() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::SetMarked {
        text: "x".repeat(1024 * 1024 + 1),
        caret_utf16: (1, 0),
    });
    runtime.key(EditCommand::SetMarked {
        text: "日".into(),
        caret_utf16: (1, 0),
    });
    runtime.key(EditCommand::SetMarked {
        text: "日本".into(),
        caret_utf16: (2, 0),
    });
    runtime.key(EditCommand::Insert("日本".into()));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    runtime.key(EditCommand::Insert("!".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "日本");
}

#[test]
fn secret_field_history_never_exports_text_to_the_clipboard() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_field("Secret", value.binding())
        .secret(true)
        .id("secret");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("secret");
    runtime.key(EditCommand::Insert("private".into()));
    runtime.key(EditCommand::SelectAll);
    bunny_ui_core::clipboard::write("public");
    for edit in [Edit::Copy, Edit::Cut] {
        assert!(!runtime.menu_answers(Pick::Edit(edit)));
        assert!(!runtime.menu_pick(Pick::Edit(edit)));
    }
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(value.get(), "private");
    assert_eq!(bunny_ui_core::clipboard::read().as_deref(), Some("public"));
}

#[test]
fn a_different_binding_with_equal_text_does_not_inherit_old_edits() {
    let runtime = Runtime::new();
    let a = State::new(String::new());
    runtime.display_frame(&text_field("Name", a.binding()).id("name"), SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::Insert("same".into()));
    let b = State::new("same".to_owned());
    runtime.display_frame(&text_field("Name", b.binding()).id("name"), SIZE);
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(!runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(a.get(), "same");
    assert_eq!(b.get(), "same");
}

#[test]
fn pasting_one_letter_splits_the_typing_group() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::Insert("a".into()));
    bunny_ui_core::clipboard::write("b");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Paste)));
    runtime.key(EditCommand::Insert("c".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "ab");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "a");
}

#[test]
fn unchanged_first_preedit_keeps_the_original_selection_for_undo() {
    let runtime = Runtime::new();
    let value = State::new("name".to_owned());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::SelectAll);
    runtime.key(EditCommand::SetMarked {
        text: "name".into(),
        caret_utf16: (4, 0),
    });
    runtime.blur();
    runtime.focus_named("name");
    runtime.key(EditCommand::SetMarked {
        text: "名前".into(),
        caret_utf16: (2, 0),
    });
    runtime.key(EditCommand::Insert("名前".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "name");
    assert_eq!(
        runtime.key(EditCommand::Copy).output.as_deref(),
        Some("name")
    );
}

#[test]
fn an_empty_composition_notification_does_not_discard_redo() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_field("Name", value.binding()).id("name");
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::SetMarked {
        text: "日".into(),
        caret_utf16: (1, 0),
    });
    runtime.key(EditCommand::Insert("日".into()));
    runtime.key(EditCommand::Insert("!".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    runtime.key(EditCommand::SetMarked {
        text: String::new(),
        caret_utf16: (0, 0),
    });
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(value.get(), "日!");
}

#[test]
fn a_custom_editing_strategy_keeps_its_own_undo() {
    use bunny_ui_core::{
        action::Stroke,
        text_input::{CaretState, EditingStrategy},
    };
    use std::rc::Rc;
    struct Policy(State<usize>);
    impl EditingStrategy for Policy {
        fn takes_text(&self) -> bool {
            true
        }
        fn key(&self, stroke: &Stroke, _: &mut String, _: &mut CaretState) -> bool {
            if stroke.pattern == Edit::Undo.stroke() {
                self.0.add(1);
                true
            } else {
                false
            }
        }
    }
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let calls = State::new(0usize);
    let view = text_field("Code", value.binding())
        .editing_strategy(Some(Rc::new(Policy(calls))))
        .id("code")
        .on_edit(Edit::Undo, || panic!("application history must not run"));
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("code");
    runtime.key(EditCommand::Insert("source".into()));
    assert!(runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert_eq!(calls.get(), 1);
    assert_eq!(value.get(), "source");
}

#[test]
fn an_alert_does_not_undo_the_field_behind_it() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let open = State::new(false);
    let view = text_field("Name", value.binding()).id("name").alert(
        open.binding(),
        bunny_ui_core::layout::AlertSpec::new("Confirm", 240.0),
        move |_| erased(button(text("Close"), move || open.set(false)).id("close")),
    );
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    runtime.key(EditCommand::Insert("keep".into()));
    open.set(true);
    runtime.display_frame(&view, SIZE);
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(!runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert_eq!(value.get(), "keep");
    open.set(false);
    runtime.display_frame(&view, SIZE);
    runtime.focus_named("name");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "");
}

#[test]
fn supplied_paste_text_is_one_history_step_between_typing_and_other_pastes() {
    let runtime = Runtime::new();
    let value = State::new(String::new());
    let view = text_editor("Text", value.binding()).id("text");
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("text"));
    runtime.key(EditCommand::Insert("before".into()));
    runtime.key(EditCommand::SelectAll);
    assert!(runtime.paste_text("café 🐇\nΩ".into()));
    assert_eq!(value.get(), "café 🐇\nΩ");
    assert!(runtime.paste_text(" β".into()));
    assert_eq!(value.get(), "café 🐇\nΩ β");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "café 🐇\nΩ");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "before");
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(value.get(), "café 🐇\nΩ");
    runtime.key(EditCommand::Insert("typed".into()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(value.get(), "café 🐇\nΩ");
    assert!(runtime.blur());
    assert!(!runtime.paste_text("unfocused".into()));
    assert_eq!(value.get(), "café 🐇\nΩ");
}
