//! Root views keep input for the scene whose frame the user sees.
use bunny_ui_core::{
    menu::{Edit, Pick},
    prelude::*,
    text_input::EditCommand,
};
use std::{cell::Cell, rc::Rc};

const SIZE: Size = Size {
    width: 320.0,
    height: 120.0,
};

#[test]
fn bare_fields_keep_input_selection_and_history_across_three_scenes() {
    let scenes = ["root-a", "root-b", "root-c"].map(|name| {
        let runtime = Runtime::scene(name);
        let value = State::new(String::new());
        let field = text_field("Value", value.binding()).id("field");
        (runtime, value, field)
    });
    for (runtime, _, field) in &scenes {
        runtime.display_frame(field, SIZE);
    }
    for (index, (runtime, value, field)) in scenes.iter().enumerate() {
        assert!(runtime.focus_named("field"));
        let original = format!("value{index}");
        // Initial input is the regression, before any selection/history command.
        assert!(runtime.key(EditCommand::Insert(original.clone())).applied);
        assert_eq!(value.get(), original);
        runtime.key(EditCommand::SelectAll);
        assert_eq!(
            runtime.key(EditCommand::Copy).output.as_deref(),
            Some(original.as_str())
        );
        let changed = format!("changed{index}");
        assert!(runtime.key(EditCommand::Insert(changed.clone())).applied);
        for (other, _, other_field) in &scenes {
            other.display_frame(other_field, SIZE);
        }
        assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
        assert_eq!(value.get(), original);
        assert_eq!(
            runtime.key(EditCommand::Copy).output.as_deref(),
            Some(original.as_str())
        );
        assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
        assert_eq!(value.get(), changed);
        runtime.display_frame(field, SIZE);
    }
    for (index, (_, value, _)) in scenes.iter().enumerate() {
        assert_eq!(value.get(), format!("changed{index}"));
    }
}

#[test]
fn replacing_a_root_clears_its_controls_without_retiring_another_scene() {
    let first = Runtime::scene("root-replaced");
    let second = Runtime::scene("root-kept");
    let a = State::new(String::new());
    let b = State::new(String::new());
    let left = text_field("A", a.binding()).id("field");
    let right = text_field("B", b.binding()).id("field");
    first.display_frame(&left, SIZE);
    assert!(first.focus_named("field"));
    assert!(first.key(EditCommand::Insert("old".into())).applied);
    second.display_frame(&right, SIZE);
    first.display_frame(&text("Removed"), SIZE);
    assert!(!first.focus_named("field"));
    assert!(!first.key(EditCommand::Insert("gone".into())).applied);
    assert_eq!(a.get(), "old");
    assert!(second.focus_named("field"));
    assert!(second.key(EditCommand::Insert("kept".into())).applied);
    first.display_frame(&left, SIZE);
    assert!(first.focus_named("field"));
    assert!(
        !first.menu_answers(Pick::Edit(Edit::Undo)),
        "unmount retired local history"
    );
    second.display_frame(&right, SIZE);
    assert!(first.key(EditCommand::Insert("new".into())).applied);
    assert_eq!(b.get(), "kept");
}

#[test]
fn closing_and_recreating_a_scene_releases_its_root_callbacks() {
    let first = Runtime::scene("root-closed");
    let second = Runtime::scene("root-neighbor");
    let a = State::new(String::new());
    let b = State::new(String::new());
    let marker = Rc::new(Cell::new(0));
    let weak = Rc::downgrade(&marker);
    let left = text_field("A", a.binding())
        .on_focus(move |_| marker.set(marker.get() + 1))
        .id("field");
    let right = text_field("B", b.binding()).id("field");
    first.display_frame(&left, SIZE);
    assert!(first.focus_named("field"));
    assert!(first.key(EditCommand::Insert("old".into())).applied);
    second.display_frame(&right, SIZE);
    drop(left);
    drop(first);
    assert!(
        weak.upgrade().is_none(),
        "a closed scene must release its root callbacks"
    );
    assert!(second.focus_named("field"));
    assert!(second.key(EditCommand::Insert("neighbor".into())).applied);
    let reopened = Runtime::scene("root-closed");
    let fresh = State::new(String::new());
    reopened.display_frame(&text_field("Fresh", fresh.binding()).id("field"), SIZE);
    assert!(reopened.focus_named("field"));
    assert!(!reopened.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(reopened.key(EditCommand::Insert("fresh".into())).applied);
    assert_eq!(a.get(), "old");
    assert_eq!(b.get(), "neighbor");
    assert_eq!(fresh.get(), "fresh");
}

#[test]
fn closing_thread_local_scenes_is_safe_during_thread_teardown() {
    const CHILD: &str = "BUNNY_SCENE_INPUT_TEARDOWN_CHILD";
    if std::env::var_os(CHILD).is_some() {
        for owner_first in [true, false] {
            std::thread::spawn(move || {
                thread_local! {
                    static OWNER: std::cell::RefCell<Option<Runtime>> = const {
                        std::cell::RefCell::new(None)
                    };
                }
                if owner_first {
                    OWNER.with(|owner| assert!(owner.borrow().is_none()));
                }
                let runtime = Runtime::scene("thread-local-root");
                OWNER.with(|owner| *owner.borrow_mut() = Some(runtime));
                OWNER.with(|owner| {
                    let owner = owner.borrow();
                    let runtime = owner.as_ref().unwrap();
                    let value = State::new(String::new());
                    runtime.display_frame(&text_field("Field", value.binding()).id("field"), SIZE);
                    assert!(runtime.focus_named("field"));
                    assert!(runtime.key(EditCommand::Insert("last".into())).applied);
                });
                // TLS owns the last Runtime; both initialization orders must
                // tolerate input registries that have already been destroyed.
            })
            .join()
            .unwrap();
        }
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "closing_thread_local_scenes_is_safe_during_thread_teardown",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
