use bunny_ui_core::{
    custom::{CustomElement, ElementEvent, EventCtx, PaintCtx, Painter, Response},
    menu::{Edit, Pick},
    prelude::*,
    text_input::EditCommand,
};

const SIZE: Size = Size {
    width: 480.0,
    height: 360.0,
};

#[test]
fn keys_and_native_menu_use_the_nearest_focused_responder() {
    let runtime = Runtime::new();
    let outer = State::new(0usize);
    let inner = State::new(0usize);
    let redone = State::new(0usize);
    let view = hstack!(
        vstack!(
            button(text("Outer"), || {}).id("outer"),
            button(text("Inner"), || {})
                .id("inner")
                .on_edit(Edit::Undo, move || inner.add(1)),
        )
        .on_edit(Edit::Undo, move || outer.add(1))
        .on_edit(Edit::Redo, move || redone.add(1)),
        button(text("Unrelated"), || {}).id("unrelated"),
    );
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("inner"));
    assert!(runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(inner.get(), 2);
    assert_eq!(outer.get(), 0);
    assert!(runtime.menu_pick(Pick::Edit(Edit::Redo)));
    assert_eq!(redone.get(), 1);
    assert!(runtime.focus_named("outer"));
    assert!(runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert_eq!(outer.get(), 1);
    assert!(runtime.focus_named("unrelated"));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(!runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert!(!runtime.menu_pick(Pick::Edit(Edit::Undo)));
}

#[derive(Clone, Copy)]
struct Editor {
    calls: State<usize>,
    takes: bool,
}
impl CustomElement for Editor {
    fn paint(&self, _: &PaintCtx, _: &mut Painter) {}
    fn accepts_keys(&self) -> bool {
        true
    }
    fn event(&self, event: &ElementEvent, _: &EventCtx) -> Response {
        if let ElementEvent::Key(stroke) = event
            && stroke.pattern == Edit::Undo.stroke()
            && self.takes
        {
            self.calls.add(1);
            return Response::handled();
        }
        Response::ignored()
    }
}

#[test]
fn fields_and_custom_editors_never_undo_the_enclosing_application() {
    for takes in [false, true] {
        let runtime = Runtime::new();
        let app = State::new(0usize);
        let own = State::new(0usize);
        let value = State::new(String::new());
        let view = vstack!(
            text_field("Filter", value.binding()).id("filter"),
            custom(Editor { calls: own, takes })
                .frame(100.0, 40.0)
                .id("editor"),
        )
        .on_edit(Edit::Undo, move || app.add(1));
        runtime.display_frame(&view, SIZE);
        assert!(runtime.focus_named("filter"));
        assert!(runtime.key(EditCommand::Insert("probe".into())).applied);
        // Fields have no local history yet. An absent local Undo is not
        // permission to alter the application behind the field.
        assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
        assert!(!runtime.key_stroke(Edit::Undo.stroke()).handled);
        assert!(!runtime.menu_pick(Pick::Edit(Edit::Undo)));
        assert_eq!(value.get(), "probe");
        assert_eq!(app.get(), 0);
        assert!(runtime.focus_named("editor"));
        assert_eq!(runtime.key_stroke(Edit::Undo.stroke()).handled, takes);
        assert_eq!(runtime.menu_pick(Pick::Edit(Edit::Undo)), takes);
        assert_eq!(own.get(), usize::from(takes) * 2);
        assert_eq!(app.get(), 0);
    }
}

#[derive(Clone, Copy)]
struct Panel {
    calls: State<usize>,
}
impl Component for Panel {
    fn body(self) -> impl View {
        button(text("Panel"), || {})
            .id("panel")
            .on_edit(Edit::Undo, move || self.calls.add(1))
    }
}

#[derive(Clone, Copy)]
struct MountedPanel {
    mounted: State<bool>,
    calls: State<usize>,
}
impl Component for MountedPanel {
    fn body(self) -> impl View {
        vstack!(
            self.mounted.get().then_some(Panel { calls: self.calls }),
            button(text("Outside"), || {}).id("outside"),
        )
    }
}

#[test]
fn retained_responder_survives_a_clean_frame_and_leaves_on_unmount() {
    let runtime = Runtime::new();
    let mounted = State::new(true);
    let calls = State::new(0usize);
    let view = MountedPanel { mounted, calls };
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("panel"));
    runtime.display_frame(&view, SIZE);
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(calls.get(), 1);
    mounted.set(false);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("outside"));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(!runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert_eq!(calls.get(), 1);
}

#[test]
fn separate_window_scenes_do_not_share_edit_responders() {
    let first = Runtime::scene("history-a");
    let second = Runtime::scene("history-b");
    let a = State::new(0usize);
    let b = State::new(0usize);
    let left = Panel { calls: a };
    let right = Panel { calls: b };
    first.display_frame(&left, SIZE);
    second.display_frame(&right, SIZE);
    assert!(first.focus_named("panel"));
    assert!(second.focus_named("panel"));
    assert!(first.menu_pick(Pick::Edit(Edit::Undo)));
    assert!(second.key_stroke(Edit::Undo.stroke()).handled);
    assert_eq!(a.get(), 1);
    assert_eq!(b.get(), 1);
}

#[test]
fn read_only_copy_keeps_its_local_answer() {
    let runtime = Runtime::new();
    let app = State::new(0usize);
    let view = text("Report")
        .on_copy(|| Some("selected report".into()))
        .id("report")
        .on_edit(Edit::Copy, move || app.add(1));
    let layout = runtime.settled_layout(&view, Proposal::exact(SIZE));
    let (path, rect) = layout
        .hits
        .iter()
        .find(|(path, _)| path.contains("[report]"))
        .unwrap();
    runtime.pointer_pressed(rect.origin.x + 2.0, rect.origin.y + 2.0);
    runtime.pointer_released(rect.origin.x + 2.0, rect.origin.y + 2.0);
    assert_eq!(runtime.focused().as_deref(), Some(path.as_str()));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Copy)));
    assert_eq!(
        bunny_ui_core::clipboard::read().as_deref(),
        Some("selected report")
    );
    assert_eq!(app.get(), 0);
}

#[test]
fn each_standard_edit_uses_its_own_responder_and_exact_accelerator() {
    for edit in Edit::ALL {
        let runtime = Runtime::new();
        let calls = State::new(0usize);
        let view = button(text("Panel"), || {})
            .id("panel")
            .on_edit(edit, move || calls.add(1));
        runtime.display_frame(&view, SIZE);
        assert!(runtime.focus_named("panel"));
        let mut other = edit.stroke();
        other.option = true;
        assert!(!runtime.key_stroke(other).handled);
        assert_eq!(calls.get(), 0);
        assert!(runtime.key_stroke(edit.stroke()).handled);
        assert!(runtime.menu_pick(Pick::Edit(edit)));
        assert_eq!(calls.get(), 2);
    }
}

#[test]
fn an_alert_cannot_dispatch_the_application_history_behind_it() {
    let runtime = Runtime::new();
    let open = State::new(false);
    let calls = State::new(0usize);
    let view = button(text("Panel"), || {})
        .id("panel")
        .alert(
            open.binding(),
            bunny_ui_core::layout::AlertSpec::new("Confirm", 240.0),
            move |_| erased(button(text("Close"), move || open.set(false)).id("close")),
        )
        .on_edit(Edit::Undo, move || calls.add(1));
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("panel"));
    open.set(true);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("close"));
    assert!(!runtime.menu_answers(Pick::Edit(Edit::Undo)));
    assert!(!runtime.key_stroke(Edit::Undo.stroke()).handled);
    assert!(!runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(calls.get(), 0);
    open.set(false);
    runtime.display_frame(&view, SIZE);
    assert!(runtime.focus_named("panel"));
    assert!(runtime.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(calls.get(), 1);
}

#[test]
fn bare_scene_roots_keep_their_responder_when_another_window_renders() {
    let first = Runtime::scene("bare-history-a");
    let second = Runtime::scene("bare-history-b");
    let a = State::new(0usize);
    let b = State::new(0usize);
    let left = button(text("First"), || {})
        .id("first")
        .on_edit(Edit::Undo, move || a.add(1));
    let right = button(text("Second"), || {})
        .id("second")
        .on_edit(Edit::Undo, move || b.add(1));
    first.display_frame(&left, SIZE);
    second.display_frame(&right, SIZE);
    assert!(first.focus_named("first"));
    assert!(second.focus_named("second"));
    assert!(first.menu_pick(Pick::Edit(Edit::Undo)));
    assert!(second.menu_pick(Pick::Edit(Edit::Undo)));
    assert_eq!(a.get(), 1);
    assert_eq!(b.get(), 1);
}
