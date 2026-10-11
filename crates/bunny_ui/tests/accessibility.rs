use bunny_ui::accessibility::{Action, Role};
use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

#[derive(Clone, Copy)]
struct Form {
    title: State<String>,
    value: State<String>,
    count: State<u32>,
    rows: State<Vec<u32>>,
}

impl Form {
    fn new() -> Self {
        Self {
            title: State::new("Description".into()),
            value: State::new("Lunch".into()),
            count: State::new(0),
            rows: State::new(vec![1, 2]),
        }
    }
}

impl Component for Form {
    fn body(self) -> impl View {
        vstack!(
            text_field("Fallback description", self.value.binding())
                .accessibility_label(self.title)
                .id("description"),
            button(text("Save"), move || self.count.add(1)).id("save"),
            for_each(
                self.rows,
                |id| id.to_string(),
                move |id| {
                    let id = *id;
                    button(text(format!("Row {id}")), move || self.count.add(id)).id("row-action")
                }
            ),
        )
    }
}

const SIZE: Size = Size {
    width: 480.0,
    height: 640.0,
};

#[test]
fn an_initial_modal_action_survives_another_window_frame() {
    let app = ProtectedForm {
        password: State::new("secret".into()),
        show_sheet: State::new(true),
        count: State::new(0),
    };
    let runtime = Runtime::scene("modal-owner");
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let button = runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|node| node.label.as_ref() == "In modal")
        .unwrap()
        .id;
    assert!(
        runtime
            .accessibility_action(button, Action::Activate)
            .is_ok(),
        "the initial modal is actionable before switching scenes"
    );
    let other = Runtime::scene("other-window");
    other.set_accessibility_enabled(true);
    other.display_frame(&text("Keeper"), SIZE);
    assert!(
        runtime
            .accessibility_action(button, Action::Activate)
            .is_ok()
    );
    assert_eq!(app.count.get(), 20);
    other.set_environment(|values| values.locale = Locale::new("fr-FR"));
    other.display_frame(&text("Keeper"), SIZE);
    assert!(
        runtime
            .accessibility_action(button, Action::Activate)
            .is_ok()
    );
    bunny_ui::code_changed();
    other.display_frame(&text("Keeper"), SIZE);
    assert!(
        runtime
            .accessibility_action(button, Action::Activate)
            .is_ok()
    );
    bunny_ui::theme::install(bunny_ui::theme::Theme::dark());
    other.display_frame(&text("Keeper"), SIZE);
    assert!(
        runtime
            .accessibility_action(button, Action::Activate)
            .is_ok()
    );
    runtime.display_frame(&app, SIZE);
    assert_eq!(
        app.count.get(),
        50,
        "scene invalidation preserves the real model"
    );
    assert_eq!(
        runtime
            .accessibility_tree()
            .nodes()
            .iter()
            .find(|node| node.label.as_ref() == "In modal")
            .unwrap()
            .id,
        button
    );
}

#[test]
fn semantic_names_values_and_actions_follow_the_real_controls() {
    let app = Form::new();
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let tree = runtime.accessibility_tree();
    let field = tree
        .nodes()
        .iter()
        .find(|n| n.role == Role::TextField)
        .unwrap();
    assert_eq!(field.label.as_ref(), "Description");
    assert_eq!(field.value.as_deref(), Some("Lunch"));
    let field_id = field.id;
    let save = tree
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Save")
        .unwrap()
        .id;
    assert!(runtime.accessibility_action(save, Action::Activate).is_ok());
    assert_eq!(app.count.get(), 1);
    assert!(
        runtime
            .accessibility_action(field_id, Action::SetText("Dinner".into()))
            .is_ok()
    );
    assert_eq!(app.value.get(), "Dinner");
    app.title.set("Expense description".into());
    runtime.display_frame(&app, SIZE);
    let tree = runtime.accessibility_tree();
    let field = tree.node(field_id).unwrap();
    assert_eq!(field.label.as_ref(), "Expense description");
    assert_eq!(field.value.as_deref(), Some("Dinner"));
}

#[test]
fn keyed_reordering_keeps_identity_and_removed_actions_are_rejected() {
    let app = Form::new();
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let row = runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Row 2")
        .unwrap()
        .id;
    app.rows.set(vec![2, 1]);
    runtime.display_frame(&app, SIZE);
    assert_eq!(
        runtime
            .accessibility_tree()
            .node(row)
            .unwrap()
            .label
            .as_ref(),
        "Row 2"
    );
    assert!(runtime.accessibility_action(row, Action::Activate).is_ok());
    assert_eq!(app.count.get(), 2);
    app.rows.set(vec![1]);
    runtime.display_frame(&app, SIZE);
    assert!(runtime.accessibility_tree().node(row).is_none());
    assert!(!runtime.accessibility_action(row, Action::Activate).is_ok());
    assert_eq!(app.count.get(), 2);
}

#[test]
fn collection_is_opt_in_and_does_not_change_the_picture() {
    let app = Form::new();
    let runtime = Runtime::new();
    let before = runtime.display_frame(&app, SIZE);
    assert!(runtime.accessibility_tree().nodes().is_empty());
    runtime.set_accessibility_enabled(true);
    let after = runtime.display_frame(&app, SIZE);
    assert_eq!(before.as_slice(), after.as_slice());
    assert!(!runtime.accessibility_tree().nodes().is_empty());
    runtime.set_accessibility_enabled(false);
    assert!(runtime.accessibility_tree().nodes().is_empty());
}

#[derive(Clone, Copy)]
struct ProtectedForm {
    password: State<String>,
    show_sheet: State<bool>,
    count: State<u32>,
}
impl Component for ProtectedForm {
    fn body(self) -> impl View {
        vstack!(
            text("Decoration").accessibility_hidden(),
            text_field("Password", self.password.binding()).secret(true),
            button(text("Behind modal"), move || self.count.add(1)),
        )
        .sheet(self.show_sheet.binding(), move |_| {
            erased(button(text("In modal"), move || self.count.add(10)))
        })
    }
}

#[test]
fn password_values_are_redacted_and_modal_controls_are_the_only_targets() {
    let app = ProtectedForm {
        password: State::new("unexportable password".into()),
        show_sheet: State::new(false),
        count: State::new(0),
    };
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let tree = runtime.accessibility_tree();
    assert!(!format!("{tree:?}").contains("unexportable"));
    assert!(
        !tree
            .nodes()
            .iter()
            .any(|n| n.label.as_ref() == "Decoration")
    );
    let password = tree
        .nodes()
        .iter()
        .find(|n| n.role == Role::PasswordField)
        .unwrap();
    assert!(password.value.is_none());
    runtime
        .accessibility_action(password.id, Action::SetText("replacement".into()))
        .unwrap();
    assert_eq!(app.password.get(), "replacement");
    assert!(
        runtime
            .accessibility_tree()
            .node(password.id)
            .unwrap()
            .focused
    );
    let behind = tree
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Behind modal")
        .unwrap()
        .id;
    app.show_sheet.set(true);
    runtime.display_frame(&app, SIZE);
    let tree = runtime.accessibility_tree();
    assert_eq!(tree.nodes().len(), 1);
    assert_eq!(tree.nodes()[0].label.as_ref(), "In modal");
    assert_eq!(
        tree.nodes()[0].surface.as_deref(),
        Some(runtime.overlays()[0].path.as_str())
    );
    assert!(
        runtime
            .accessibility_action(behind, Action::Activate)
            .is_err()
    );
    runtime
        .accessibility_action(tree.nodes()[0].id, Action::Activate)
        .unwrap();
    assert_eq!(app.count.get(), 10);
}

#[test]
fn unsupported_actions_do_not_move_focus_or_call_a_different_control() {
    let app = Form::new();
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let tree = runtime.accessibility_tree();
    let save = tree
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Save")
        .unwrap()
        .id;
    assert_eq!(
        runtime.accessibility_action(save, Action::SetText("bad".into())),
        Err(bunny_ui::accessibility::ActionError::Unsupported)
    );
    assert!(runtime.focused().is_none());
    assert_eq!(app.value.get(), "Lunch");
    assert_eq!(app.count.get(), 0);
}

#[test]
fn a_removed_then_reinserted_key_cannot_inherit_an_old_native_handle() {
    let app = Form::new();
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let old = runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Row 2")
        .unwrap()
        .id;
    app.rows.set(vec![1]);
    runtime.display_frame(&app, SIZE);
    app.rows.set(vec![1, 2]);
    runtime.display_frame(&app, SIZE);
    let current = runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Row 2")
        .unwrap()
        .id;
    assert_ne!(old, current);
    assert!(runtime.accessibility_action(old, Action::Activate).is_err());
    runtime
        .accessibility_action(current, Action::Activate)
        .unwrap();
    assert_eq!(app.count.get(), 2);
}

#[test]
fn explicit_names_do_not_replace_values_or_duplicate_button_labels() {
    #[derive(Clone, Copy)]
    struct Label {
        value: State<String>,
        title: State<String>,
    }
    impl Component for Label {
        fn body(self) -> impl View {
            vstack!(
                button(text(self.title), || {}).accessibility_label("Explicit name"),
                text_editor("Notes", self.value.binding()).accessibility_label("Message"),
                text(self.title).accessibility_label(self.value),
            )
        }
    }
    let app = Label {
        value: State::new("Value".into()),
        title: State::new("Title".into()),
    };
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let tree = runtime.accessibility_tree();
    assert_eq!(tree.nodes().len(), 3);
    assert_eq!(tree.nodes()[0].label.as_ref(), "Explicit name");
    assert_eq!(tree.nodes()[1].label.as_ref(), "Message");
    assert_eq!(tree.nodes()[1].value.as_deref(), Some("Value"));
    assert!(tree.nodes()[1].multiline);
    assert_eq!(tree.nodes()[2].label.as_ref(), "Value");
    app.title.set("Changed title".into());
    app.value.set("Changed value".into());
    runtime.display_frame(&app, SIZE);
    assert_eq!(
        runtime.accessibility_tree().nodes()[2].label.as_ref(),
        "Changed value"
    );
}

#[test]
fn scrolling_exposes_only_intersecting_controls_with_clipped_bounds() {
    #[derive(Clone, Copy)]
    struct Scrolling;
    impl Component for Scrolling {
        fn body(self) -> impl View {
            scroll(
                vstack!(
                    button(text("Visible"), || {}).frame(120.0, 100.0),
                    button(text("Outside"), || {}).frame(120.0, 100.0),
                )
                .spacing(0.0),
            )
            .frame(120.0, 80.0)
        }
    }
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(
        &Scrolling,
        Size {
            width: 120.0,
            height: 80.0,
        },
    );
    let tree = runtime.accessibility_tree();
    assert_eq!(tree.nodes().len(), 1, "{tree:?}");
    assert_eq!(tree.nodes()[0].label.as_ref(), "Visible");
    assert!(tree.nodes()[0].bounds.origin.y + tree.nodes()[0].bounds.size.height <= 80.0);
}

#[test]
fn handles_are_scoped_to_their_runtime_and_disabling_retires_them() {
    let first = Runtime::scene("first");
    let second = Runtime::scene("second");
    let app = Form::new();
    first.set_accessibility_enabled(true);
    second.set_accessibility_enabled(true);
    first.display_frame(&app, SIZE);
    second.display_frame(&app, SIZE);
    let id = first
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Save")
        .unwrap()
        .id;
    assert!(second.accessibility_action(id, Action::Activate).is_err());
    first.set_accessibility_enabled(false);
    assert!(first.accessibility_action(id, Action::Activate).is_err());
    first.set_accessibility_enabled(true);
    first.display_frame(&app, SIZE);
    assert!(first.accessibility_action(id, Action::Activate).is_err());
    let current = first
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|n| n.label.as_ref() == "Save")
        .unwrap()
        .id;
    first
        .accessibility_action(current, Action::Activate)
        .unwrap();
    assert_eq!(app.count.get(), 1);
}

#[test]
fn snapshots_do_not_schedule_idle_frames_and_repeated_frames_keep_handles() {
    let app = Form::new();
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let before = runtime.accessibility_tree();
    assert!(!runtime.needs_frame());
    for _ in 0..5 {
        assert_eq!(runtime.accessibility_tree(), before);
    }
    assert!(!runtime.needs_frame());
    runtime.display_frame(&app, SIZE);
    assert_eq!(runtime.accessibility_tree(), before);
    assert!(!runtime.needs_frame());
}

#[test]
fn set_text_replaces_the_whole_value_even_during_ime_composition() {
    use bunny_ui::text_input::EditCommand;
    let app = Form::new();
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&app, SIZE);
    let field = runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .find(|n| n.role == Role::TextField)
        .unwrap()
        .id;
    runtime.accessibility_action(field, Action::Focus).unwrap();
    runtime.key(EditCommand::SetMarked {
        text: " composing".into(),
        caret_utf16: (2, 0),
    });
    runtime
        .accessibility_action(field, Action::SetText("replacement".into()))
        .unwrap();
    assert_eq!(app.value.get(), "replacement");
}

#[test]
fn the_window_viewport_excludes_controls_beyond_its_content_area() {
    #[derive(Clone, Copy)]
    struct Overflow;
    impl Component for Overflow {
        fn body(self) -> impl View {
            vstack!(
                text("Filler").frame(120.0, 200.0),
                button(text("Outside window"), || {}),
            )
            .spacing(0.0)
        }
    }
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(
        &Overflow,
        Size {
            width: 120.0,
            height: 80.0,
        },
    );
    let tree = runtime.accessibility_tree();
    assert!(
        !tree
            .nodes()
            .iter()
            .any(|n| n.label.as_ref() == "Outside window"),
        "{tree:?}"
    );
    assert!(
        tree.nodes()
            .iter()
            .all(|n| n.bounds.origin.y + n.bounds.size.height <= 80.0)
    );
}

#[test]
fn a_partly_visible_button_keeps_the_name_of_its_clipped_label() {
    #[derive(Clone, Copy)]
    struct TallButton;
    impl Component for TallButton {
        fn body(self) -> impl View {
            button(text("Complete button name").frame_height(200.0), || {})
        }
    }
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(
        &TallButton,
        Size {
            width: 200.0,
            height: 40.0,
        },
    );
    let tree = runtime.accessibility_tree();
    assert_eq!(tree.nodes().len(), 1);
    assert_eq!(tree.nodes()[0].role, Role::Button);
    assert_eq!(tree.nodes()[0].label.as_ref(), "Complete button name");
    assert_eq!(tree.nodes()[0].bounds.size.height, 40.0);
}
