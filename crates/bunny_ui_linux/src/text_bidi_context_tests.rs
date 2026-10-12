//! Prepared levels survive overlapping soft-line queries without mutation.
use super::bidi::{Direction, Error, ResolvedText, Run};
use std::ops::Range;

fn word(text: &str, word: &str) -> Range<usize> {
    let start = text.find(word).unwrap();
    start..start + word.len()
}
fn only(text: &str, selected: &str, direction: Direction) {
    let prepared = ResolvedText::new(text).unwrap();
    let bytes = word(text, selected);
    assert_eq!(
        prepared.line_runs(bytes.clone()).unwrap(),
        vec![Run { bytes, direction }]
    );
}

#[test]
fn mixed_paragraph_context_carries_explicit_overrides() {
    only("\u{202e}abc def ghi\u{202c}", "def", Direction::RightToLeft);
    only("\u{202d}אבג דהו זחט\u{202c}", "דהו", Direction::LeftToRight);
}

#[test]
fn mixed_paragraph_context_keeps_isolate_scope() {
    let text = "\u{202e}abc \u{2066}def ghi\u{2069} jkl\u{202c}";
    only(text, "def", Direction::LeftToRight);
    only(text, "jkl", Direction::RightToLeft);
}

#[test]
fn mixed_paragraph_context_resets_after_hard_paragraph() {
    only(
        "\u{202e}abc\nxyz mno\u{202c}",
        "xyz",
        Direction::LeftToRight,
    );
    only(
        "\u{202e}abc\u{2029}xyz mno\u{202c}",
        "xyz",
        Direction::LeftToRight,
    );
}

#[test]
fn mixed_paragraph_context_line_reset_does_not_mutate_shared_levels() {
    let text = "\u{202e}abc def ghi\u{202c}";
    let prepared = ResolvedText::new(text).unwrap();
    let before = prepared.line_runs(0..text.len()).unwrap();
    let line = word(text, "def ");
    let expected = vec![
        Run {
            bytes: line.start..line.end - 1,
            direction: Direction::RightToLeft,
        },
        Run {
            bytes: line.end - 1..line.end,
            direction: Direction::LeftToRight,
        },
    ];
    for _ in 0..3 {
        assert_eq!(prepared.line_runs(line.clone()).unwrap(), expected);
        assert_eq!(prepared.line_runs(0..text.len()).unwrap(), before);
    }
}

#[test]
fn mixed_paragraph_context_keeps_original_utf8_offsets() {
    only("😀\u{202e}abc def\u{202c}", "def", Direction::RightToLeft);
    only("אבג \u{202d}דהו זחט\u{202c}", "זחט", Direction::LeftToRight);
}

#[test]
fn mixed_paragraph_context_rejects_invalid_ranges() {
    let text = "אבג";
    let prepared = ResolvedText::new(text).unwrap();
    for range in [
        1..text.len(),
        0..1,
        0..text.len() + 1,
        Range { start: 4, end: 2 },
    ] {
        assert!(matches!(
            prepared.line_runs(range),
            Err(Error::InvalidRange)
        ));
    }
    assert!(prepared.line_runs(2..2).unwrap().is_empty());
}

#[test]
fn mixed_paragraph_context_ascii_and_empty_ranges_keep_fast_path() {
    let text = "abc def";
    only(text, "def", Direction::LeftToRight);
    let prepared = ResolvedText::new(text).unwrap();
    assert!(matches!(prepared.line_runs(0..8), Err(Error::InvalidRange)));
    assert!(matches!(
        prepared.line_runs(Range { start: 4, end: 2 }),
        Err(Error::InvalidRange)
    ));
    assert!(prepared.line_runs(7..7).unwrap().is_empty());
    assert!(
        ResolvedText::new("")
            .unwrap()
            .line_runs(0..0)
            .unwrap()
            .is_empty()
    );
}
