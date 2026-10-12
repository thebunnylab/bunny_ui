//! A native composition carries a selection, not just its active endpoint.
use super::{CaretState, EditCommand, apply};

#[test]
fn marked_selection_retains_scalar_ranges_and_clamps_native_utf16() {
    let composition = "😀a\u{301}ב";
    for (range, expected) in [
        ((0, 2), (0, 4)),
        ((2, 1), (4, 5)),
        ((3, 1), (5, 7)),
        ((2, 0), (4, 4)),
        ((0, usize::MAX), (0, 9)),
        ((2, usize::MAX), (4, 9)),
        ((usize::MAX, usize::MAX), (9, 9)),
    ] {
        let mut text = String::from("LR");
        let mut state = CaretState {
            caret: 1,
            ..CaretState::default()
        };
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked {
                text: composition.into(),
                caret_utf16: range,
            },
        );
        assert_eq!(text, format!("L{composition}R"));
        assert_eq!(state.marked, Some((1, 10)));
        assert_eq!(state.caret, 1 + expected.1, "{range:?}");
        let selected = (expected.0 != expected.1).then_some((1 + expected.0, 1 + expected.1));
        assert_eq!(state.selection(), selected, "{range:?}");
    }
}
