use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;
const SIZE: Size = Size {
    width: 360.0,
    height: 180.0,
};
#[test]
fn plain_buttons_keep_the_custom_label_geometry_without_default_chrome() {
    let runtime = Runtime::new();
    let plain = button(spacer().frame(84.0, 32.0), || {})
        .plain()
        .id("plain");
    runtime.display_frame(&plain, SIZE);
    let plain_rect = runtime
        .hits_snapshot()
        .into_iter()
        .find(|(id, _)| id.contains("[plain]"))
        .unwrap()
        .1;
    assert_eq!(
        plain_rect.size,
        Size {
            width: 84.0,
            height: 32.0
        }
    );
    assert!(runtime.focus_named("plain"));
    let ordinary = button(spacer().frame(84.0, 32.0), || {}).id("ordinary");
    runtime.display_frame(&ordinary, SIZE);
    let ordinary_rect = runtime
        .hits_snapshot()
        .into_iter()
        .find(|(id, _)| id.contains("[ordinary]"))
        .unwrap()
        .1;
    assert!(ordinary_rect.size.width > plain_rect.size.width);
    assert!(ordinary_rect.size.height > plain_rect.size.height);
}
#[derive(Clone, Copy)]
struct CustomToggle {
    on: State<bool>,
    disabled: State<bool>,
}
impl Component for CustomToggle {
    fn body(self) -> impl View {
        button(spacer().frame(44.0, 44.0), move || {
            self.on.set(!self.on.get())
        })
        .plain()
        .checked(self.on.get())
        .disabled(self.disabled.get())
        .accessibility_label("Mail assistance")
        .id("custom-toggle")
    }
}
#[test]
fn plain_checked_buttons_keep_tab_space_touch_and_accessibility_without_a_glyph() {
    use bunny_ui::accessibility::{Action, Role};
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let view = CustomToggle {
        on: State::new(false),
        disabled: State::new(false),
    };
    let frame = runtime.display_frame(&view, SIZE);
    assert!(
        !frame
            .iter()
            .any(|draw| matches!(draw, bunny_ui::layout::DrawCommand::TextLine { .. })),
        "custom checked controls must add no glyph"
    );
    assert!(runtime.key_stroke(KeyPattern::key(Key::Tab)).handled);
    assert!(runtime.key_stroke(KeyPattern::key(Key::Char(' '))).handled);
    assert!(view.on.get());
    runtime.display_frame(&view, SIZE);
    let tree = runtime.accessibility_tree();
    let node = tree
        .nodes()
        .iter()
        .find(|node| node.role == Role::Checkbox)
        .unwrap();
    assert_eq!(node.label.as_ref(), "Mail assistance");
    assert_eq!(node.checked, Some(true));
    let id = node.id;
    let point = node.bounds.origin;
    runtime.touch_began(1, point.x + 10.0, point.y + 10.0, 1);
    runtime.touch_ended(1, point.x + 10.0, point.y + 10.0);
    assert!(!view.on.get());
    view.disabled.set(true);
    runtime.display_frame(&view, SIZE);
    assert!(!runtime.focus_named("custom-toggle"));
    assert!(runtime.accessibility_action(id, Action::Activate).is_err());
    runtime.touch_began(2, point.x + 10.0, point.y + 10.0, 1);
    runtime.touch_ended(2, point.x + 10.0, point.y + 10.0);
    assert!(!view.on.get());
}
