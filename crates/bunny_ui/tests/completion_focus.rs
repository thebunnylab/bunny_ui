use bunny_ui_core::{
    accessibility::Action,
    layout::{PopoverFocus, Side},
    prelude::*,
    text_input::EditCommand,
};

const SIZE: Size = Size {
    width: 360.0,
    height: 240.0,
};

#[derive(Clone, Copy)]
struct Completion {
    open: State<bool>,
    value: State<String>,
    selected: State<usize>,
}

impl Component for Completion {
    fn body(self) -> impl View {
        vstack!(
            text_field("Template", self.value.binding()).id("template"),
            spacer().frame(180.0, 1.0).popover_with_focus(
                self.open.binding(),
                Side::Bottom,
                PopoverFocus::Preserve,
                move |_| erased(
                    button(text("likes (State<i32>)"), move || {
                        self.selected.add(1);
                        self.open.set(false);
                    })
                    .leaves_keyboard()
                    .id("suggestion")
                )
            ),
            button(text("Next"), || {}).id("next"),
        )
    }
}

#[test]
fn completion_keeps_typing_and_exposes_operable_accessible_choices() {
    for pointer in [true, false] {
        let runtime = Runtime::new();
        runtime.set_accessibility_enabled(true);
        let view = Completion {
            open: State::new(false),
            value: State::new(String::new()),
            selected: State::new(0),
        };
        runtime.display_frame(&view, SIZE);
        assert!(runtime.focus_named("template"));
        let owner = runtime.focused();
        view.open.set(true);
        runtime.display_frame(&view, SIZE);
        assert_eq!(runtime.focused(), owner);
        assert!(runtime.key(EditCommand::Insert("{likes".into())).applied);
        assert_eq!(view.value.get(), "{likes");
        runtime.display_frame(&view, SIZE);
        let tree = runtime.accessibility_tree();
        let choice = tree
            .nodes()
            .iter()
            .find(|node| node.label.as_ref() == "likes (State<i32>)")
            .unwrap();
        assert!(choice.supports(&Action::Activate));
        if pointer {
            let at = choice.bounds.origin;
            runtime.pointer_pressed(at.x + 2.0, at.y + 2.0);
            runtime.pointer_released(at.x + 2.0, at.y + 2.0);
        } else {
            runtime
                .accessibility_action(choice.id, Action::Activate)
                .unwrap();
        }
        runtime.display_frame(&view, SIZE);
        assert_eq!(view.selected.get(), 1);
        assert!(!view.open.get());
        assert_eq!(runtime.focused(), owner);
    }
}

#[test]
fn completion_dismisses_and_does_not_trap_navigation() {
    let runtime = Runtime::new();
    let view = Completion {
        open: State::new(false),
        value: State::new(String::new()),
        selected: State::new(0),
    };
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("template"));
    view.open.set(true);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("next"));
    assert!(runtime.focus_named("suggestion"));
    assert!(runtime.focus_named("template"));
    let dismiss = runtime.match_key(&KeyPattern::key(Key::Escape)).unwrap();
    assert!(runtime.dispatch_action(dismiss));
    runtime.display_frame(&view, SIZE);
    assert!(!view.open.get());
    assert!(runtime.focused().unwrap().contains("[template]"));
    view.open.set(true);
    runtime.display_frame(&view, SIZE);
    runtime.pointer_pressed(355.0, 235.0);
    runtime.pointer_released(355.0, 235.0);
    assert!(!view.open.get());
    assert_eq!(view.selected.get(), 0);
}
