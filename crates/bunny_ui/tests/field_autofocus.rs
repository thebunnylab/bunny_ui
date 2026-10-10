//! Auto-focus must show the insertion point before the first edit.
extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::text_input::EditCommand;

#[test]
fn a_long_auto_focused_editor_reveals_its_caret_before_the_first_key() {
    let content = State::new((0..100).map(|n| format!("line {n}\n")).collect::<String>());
    let view = text_editor("", content.binding()).auto_focus().id("editor");
    let runtime = Runtime::new();
    let size = Size {
        width: 240.0,
        height: 160.0,
    };
    runtime.display_frame(&view, size);
    let caret = runtime
        .ime_snapshot()
        .expect("auto-focused editor")
        .caret_rect;
    assert!(
        caret.origin.y >= 0.0 && caret.origin.y + caret.size.height <= size.height,
        "first-frame caret must be visible: {caret:?}"
    );
    let path = runtime.focused().expect("keyboard owner");
    let offset = runtime.scroll_offset(&path);
    assert!(
        offset.y > 0.0,
        "the initial viewport must follow the end caret"
    );
    assert!(runtime.key(EditCommand::Insert("x".into())).applied);
    runtime.display_frame(&view, size);
    assert_eq!(
        runtime.scroll_offset(&path).y,
        offset.y,
        "the first key must not jump to another page"
    );
}
