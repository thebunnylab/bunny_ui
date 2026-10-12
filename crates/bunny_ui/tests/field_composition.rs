//! Native selection survives incremental composition and retained field layout.
extern crate bunny_ui_core as bunny_ui;
use bunny_ui::prelude::{EditCommand, Runtime, Size, State, StateExt, ViewExt, text_editor};

#[test]
fn paragraph_composition_preserves_selection_replacement_and_commit() {
    let runtime = Runtime::new();
    let draft = State::new(String::from("😀 before\n"));
    let view = text_editor("", draft.binding())
        .frame(120.0, 240.0)
        .id("composition");
    let size = Size {
        width: 120.0,
        height: 240.0,
    };
    runtime.display_frame(&view, size).unwrap();
    assert!(runtime.focus_named("composition"));
    assert!(runtime.key(EditCommand::End(false)).unwrap().applied);
    let prefix = draft.get();
    let offset = prefix.encode_utf16().count();
    for (text, range, selected) in [
        ("😀a\u{301}ב", (3, 1), "\u{301}"),
        ("אבג 😀 xyz", (7, 3), "xyz"),
    ] {
        assert!(
            runtime
                .key(EditCommand::SetMarked {
                    text: text.into(),
                    caret_utf16: range,
                })
                .unwrap()
                .applied
        );
        runtime.display_frame(&view, size).unwrap();
        let snapshot = runtime.ime_snapshot().unwrap().unwrap();
        assert_eq!(snapshot.text.as_ref(), format!("{prefix}{text}"));
        assert_eq!(snapshot.marked, Some((offset, text.encode_utf16().count())));
        assert_eq!(snapshot.selected, (offset + range.0, range.1));
        assert_eq!(
            runtime.key(EditCommand::Copy).unwrap().output.as_deref(),
            Some(selected)
        );
        assert_eq!(
            runtime.ime_snapshot().unwrap().unwrap().marked,
            snapshot.marked
        );
    }
    assert!(runtime.key(EditCommand::Unmark).unwrap().applied);
    let snapshot = runtime.ime_snapshot().unwrap().unwrap();
    assert_eq!(snapshot.marked, None);
    assert_eq!(snapshot.selected, (offset + 7, 3));
    assert!(
        runtime
            .key(EditCommand::Insert("ok".into()))
            .unwrap()
            .applied
    );
    runtime.display_frame(&view, size).unwrap();
    assert_eq!(draft.get(), format!("{prefix}אבג 😀 ok"));
    assert_eq!(
        runtime.ime_snapshot().unwrap().unwrap().selected,
        (offset + 9, 0)
    );
}
