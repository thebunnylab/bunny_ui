//! Public editing witnesses independent of the grapheme implementation.
use bunny_ui_core::text_input::{CaretState, EditCommand, apply, byte_to_utf16};

fn at(caret: usize) -> CaretState {
    CaretState {
        caret,
        ..CaretState::default()
    }
}

fn conformance_cases() -> impl Iterator<Item = (usize, String, Vec<usize>)> {
    include_str!("data/unicode-17.0.0/GraphemeBreakTest.txt")
        .lines()
        .enumerate()
        .filter_map(|(line, source)| {
            let source = source.split('#').next().unwrap().trim();
            if source.is_empty() {
                return None;
            }
            let mut text = String::new();
            let mut boundaries = Vec::new();
            for token in source.split_whitespace() {
                match token {
                    "÷" => boundaries.push(text.len()),
                    "×" => (),
                    hex => {
                        text.push(char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap())
                    }
                }
            }
            Some((line + 1, text, boundaries))
        })
}

#[test]
fn unicode_17_conformance_through_public_arrow_commands() {
    let mut cases = 0;
    for (line, mut text, boundaries) in conformance_cases() {
        cases += 1;
        let original = text.clone();
        let mut caret = at(0);
        for &expected in boundaries.iter().skip(1) {
            apply(&mut text, &mut caret, EditCommand::Right(false));
            assert_eq!(
                caret.caret, expected,
                "forward, Unicode line {line}: {original:?}"
            );
        }
        for &expected in boundaries.iter().rev().skip(1) {
            apply(&mut text, &mut caret, EditCommand::Left(false));
            assert_eq!(
                caret.caret, expected,
                "backward, Unicode line {line}: {original:?}"
            );
        }
        // Native hit testing/IME may give any scalar boundary, not just a cluster edge.
        for index in original
            .char_indices()
            .map(|(index, _)| index)
            .chain([original.len()])
        {
            let mut caret = at(index);
            apply(&mut text, &mut caret, EditCommand::Left(false));
            assert_eq!(
                caret.caret,
                boundaries
                    .iter()
                    .copied()
                    .rev()
                    .find(|&b| b < index)
                    .unwrap_or(0),
                "random left, Unicode line {line} at {index}"
            );
            caret = at(index);
            apply(&mut text, &mut caret, EditCommand::Right(false));
            assert_eq!(
                caret.caret,
                boundaries
                    .iter()
                    .copied()
                    .find(|&b| b > index)
                    .unwrap_or(text.len()),
                "random right, Unicode line {line} at {index}"
            );
        }
        assert_eq!(text, original);
    }
    assert_eq!(cases, 766, "retain every case from the pinned Unicode file");
}

#[test]
fn one_delete_removes_one_user_perceived_character() {
    for cluster in [
        "e\u{301}",
        "👨‍👩‍👧‍👦",
        "👩🏽‍🚀",
        "🇧🇷",
        "1\u{fe0f}\u{20e3}",
        "\r\n",
        "각",
        "क्ष",
    ] {
        let original = format!("a{cluster}z");
        let mut text = original.clone();
        let mut caret = at(1 + cluster.len());
        apply(&mut text, &mut caret, EditCommand::Backspace);
        assert_eq!(text, "az", "backspace {cluster:?}");
        assert_eq!(caret.caret, 1);
        text = original.clone();
        caret = at(1);
        apply(&mut text, &mut caret, EditCommand::Delete);
        assert_eq!(text, "az", "delete {cluster:?}");
        assert_eq!(caret.caret, 1);
        text = original;
        caret = at(1);
        apply(&mut text, &mut caret, EditCommand::Right(true));
        assert_eq!(caret.selection(), Some((1, 1 + cluster.len())));
        assert_eq!(
            apply(&mut text, &mut caret, EditCommand::Copy),
            Some(cluster.into())
        );
        apply(&mut text, &mut caret, EditCommand::Backspace);
        assert_eq!(text, "az");
    }
}

#[test]
fn an_interior_caret_deletes_the_containing_cluster() {
    for command in [EditCommand::Backspace, EditCommand::Delete] {
        let mut text = String::from("ae\u{301}z");
        let mut caret = at(2); // valid UTF-8, between the base and its accent
        apply(&mut text, &mut caret, command);
        assert_eq!(text, "az");
        assert_eq!(caret.caret, 1);
    }
}

#[test]
fn native_composition_and_explicit_ranges_keep_their_utf16_contract() {
    let mut text = String::from("az");
    let mut caret = at(1);
    apply(
        &mut text,
        &mut caret,
        EditCommand::SetMarked {
            text: "e\u{301}".into(),
            caret_utf16: (1, 0),
        },
    );
    assert_eq!(
        caret.caret, 2,
        "IME can place its caret inside a composing cluster"
    );
    assert_eq!(byte_to_utf16(&text, caret.caret), 2);
    assert_eq!(caret.marked, Some((1, 4)));
    apply(&mut text, &mut caret, EditCommand::Insert("é".into()));
    assert_eq!(text, "aéz");
    assert_eq!(caret.caret, 3);
    assert_eq!(caret.marked, None);

    text = "ae\u{301}z".into();
    caret = CaretState {
        caret: 4,
        anchor: Some(2),
        marked: None,
    };
    apply(&mut text, &mut caret, EditCommand::Cut);
    assert_eq!(text, "aez", "an explicit native selection is not widened");
}

#[test]
#[cfg(feature = "canvas")]
fn retained_field_deletion_invalidates_the_whole_cluster_for_ime() {
    use bunny_ui_core::layout::{Proposal, Size};
    use bunny_ui_core::prelude::*;
    #[derive(Clone, Copy)]
    struct Form {
        value: State<String>,
    }
    impl Component for Form {
        fn body(self) -> impl View {
            text_field("Name", self.value.binding())
        }
    }
    for (initial, steps, marked, position, commit, expected) in [
        ("az", 1, "👨‍👩‍👧‍👦", 2, true, "a👨‍👩‍👧‍👦z"),
        ("a👩z", 2, "🏽‍🚀", 0, false, "a👩🏽‍🚀z"),
    ] {
        for command in [EditCommand::Backspace, EditCommand::Delete] {
            let runtime = Runtime::new();
            let value = State::new(String::from(initial));
            let field = Form { value };
            runtime.render_stable(&field);
            let proposal = Proposal::exact(Size {
                width: 300.0,
                height: 50.0,
            });
            let frame = runtime.layout(&field, proposal);
            runtime.focus(&frame.hits.last().unwrap().0);
            runtime.key(EditCommand::Home(false));
            for _ in 0..steps {
                runtime.key(EditCommand::Right(false));
            }
            runtime.key(EditCommand::SetMarked {
                text: marked.into(),
                caret_utf16: (position, 0),
            });
            if commit {
                runtime.key(EditCommand::Unmark);
            }
            runtime.render_stable(&field);
            runtime.layout(&field, proposal);
            let before = runtime.ime_snapshot().unwrap();
            assert_eq!(before.selected, (3, 0));
            assert_eq!(before.text.as_ref(), expected);
            assert!(runtime.key(command).applied);
            runtime.render_stable(&field);
            runtime.layout(&field, proposal);
            let after = runtime.ime_snapshot().unwrap();
            assert_eq!(after.text.as_ref(), "az");
            assert_eq!(after.selected, (1, 0));
            assert_eq!(value.get(), "az");
        }
    }
}
