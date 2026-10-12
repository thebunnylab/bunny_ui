use super::*;

#[test]
fn paragraph_line_reset_uses_original_classes_and_retained_controls() {
    // Retained BN/formatting inherits the preceding level before L1. ZWJ is
    // included here only for ordering; its original shaping context is intact.
    let text = "a\u{202e}b \u{2067}\u{200d}\tq\u{2003}\u{202c}\u{2069}";
    let classes: Vec<_> = text.chars().map(Class::of).collect();
    let mut levels = vec![0, 9, 1, 1, 1, 9, 1, 2, 2, 9, 2];
    reset_line(&classes, &mut levels, 0);
    assert_eq!(levels, [0, 0, 1, 0, 0, 0, 0, 2, 0, 0, 0]);
}

#[test]
fn paragraph_line_reset_continuation_keeps_preceding_paragraph_level() {
    let mut levels = [7, 7, 3, 3];
    reset_line(
        &[Class::Retained, Class::Retained, Class::Other, Class::Reset],
        &mut levels,
        2,
    );
    assert_eq!(levels, [2, 2, 3, 0]);
    reset_line(&[], &mut [], 0);
    let mut spaces = [1, 1];
    reset_line(&[Class::Reset; 2], &mut spaces, 1);
    assert_eq!(spaces, [0, 0]);
}

#[test]
fn paragraph_line_unicode_classes_are_not_rust_whitespace() {
    assert_eq!(Class::of('\u{a0}'), Class::Other); // NBSP is CS.
    assert_eq!(Class::of('\u{202f}'), Class::Other); // narrow NBSP is CS.
    assert_eq!(Class::of('\u{2003}'), Class::Reset);
    assert_eq!(Class::of('\u{2028}'), Class::Reset);
    assert_eq!(Class::of('\u{2029}'), Class::Separator);
    assert_eq!(Class::of('\u{200c}'), Class::Retained);
    assert_eq!(Class::of('\u{e0001}'), Class::Retained);
    assert_eq!(Class::of('😀'), Class::Other);
    assert!(data::CLASSES.windows(2).all(|pair| pair[0].1 < pair[1].0));
}

#[test]
fn paragraph_line_l2_reverses_through_missing_intermediate_levels() {
    for (levels, expected) in [
        (vec![0, 1, 2, 2, 1, 0], vec![0, 4, 2, 3, 1, 5]),
        (vec![0, 3, 3, 0], vec![0, 2, 1, 3]),
        (vec![0, 2, 4, 4, 2, 0], vec![0, 1, 2, 3, 4, 5]),
        (vec![3, 2, 2, 3], vec![0, 1, 2, 3]),
        (vec![], vec![]),
    ] {
        let mut items: Vec<_> = levels.iter().copied().enumerate().collect();
        reorder(&mut items, |&(_, level)| level);
        assert_eq!(
            items.iter().map(|&(index, _)| index).collect::<Vec<_>>(),
            expected,
            "{levels:?}"
        );
    }
}

#[test]
fn paragraph_line_l1_matches_official_unicode_17_trailing_pdf_case() {
    // Unicode 17 BidiTest.txt, line 111785: RLO L WS PDF; 3
    // @Levels: x 1 0 x; @Reorder: 1 2 (auto-LTR and explicit-LTR).
    // Explicit formatting levels marked x are excluded by the official oracle.
    let classes = [Class::Retained, Class::Other, Class::Reset, Class::Retained];
    let mut levels = [0, 1, 1, 0];
    reset_line(&classes, &mut levels, 0);
    assert_eq!(&levels[1..3], &[1, 0]);
    let mut visible = [(1, levels[1]), (2, levels[2])];
    reorder(&mut visible, |&(_, level)| level);
    assert_eq!(visible.map(|(index, _)| index), [1, 2]);
}
