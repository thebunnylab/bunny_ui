//! Unicode wrap boundaries are an application-visible contract independent of glyph width.
#![cfg(feature = "canvas")]
extern crate bunny_ui_core as bunny_ui;

#[path = "support/grapheme_wrap.rs"]
mod support;

use bunny_ui::text_engine::{FontSpec, MeasureCache, PixelFont, break_lines};
use std::rc::Rc;

#[test]
fn soft_wrap_preserves_clusters_and_overwide_singletons() {
    support::clusters_stay_whole(&PixelFont);
}

#[test]
fn retained_editor_draw_and_vertical_motion_preserve_clusters() {
    support::retained_editor_keeps_atomic_lines(Rc::new(PixelFont));
}

#[test]
fn every_unicode_17_fixture_only_soft_wraps_at_declared_boundaries() {
    let mut cases = 0;
    for (line, source) in include_str!("data/unicode-17.0.0/GraphemeBreakTest.txt")
        .lines()
        .enumerate()
    {
        let source = source.split('#').next().unwrap().trim();
        if source.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut boundaries = Vec::new();
        for token in source.split_whitespace() {
            match token {
                "÷" => boundaries.push(text.len()),
                "×" => (),
                hex => text.push(char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()),
            }
        }
        cases += 1;
        // LF is the existing hard separator. CR/LF is one grapheme, but the
        // renderer deliberately excludes LF from both neighboring line ranges.
        let allowed = |at| boundaries.contains(&at) || text.as_bytes().get(at) == Some(&b'\n');
        for width in [0.0, 1.0, 8.0, 16.0, 32.0, 80.0] {
            let lines = break_lines(
                &text,
                &FontSpec::DEFAULT,
                width,
                &PixelFont,
                &MeasureCache::default(),
            );
            let mut next = 0;
            for (start, end) in lines {
                assert_eq!(start, next, "Unicode line {}: lost bytes", line + 1);
                assert!(
                    allowed(start) && allowed(end),
                    "Unicode line {} at width {width}: {start}..{end} of {text:?}",
                    line + 1
                );
                next = end + usize::from(text.as_bytes().get(end) == Some(&b'\n'));
            }
            assert_eq!(
                next,
                text.len(),
                "Unicode line {}: incomplete coverage",
                line + 1
            );
        }
    }
    assert_eq!(cases, 766);
}

#[test]
fn hard_newlines_empty_lines_and_ascii_word_policy_remain_stable() {
    for (text, width, expected) in [
        ("", 0.0, vec![(0, 0)]),
        ("\n", 8.0, vec![(0, 0), (1, 1)]),
        ("a\n\nz", 8.0, vec![(0, 1), (2, 2), (3, 4)]),
        ("aa bb cc", 24.0, vec![(0, 3), (3, 6), (6, 8)]),
        ("abcdef", 16.0, vec![(0, 2), (2, 4), (4, 6)]),
        ("a", 1.0, vec![(0, 1)]),
    ] {
        assert_eq!(
            break_lines(
                text,
                &FontSpec::DEFAULT,
                width,
                &PixelFont,
                &MeasureCache::default()
            ),
            expected
        );
    }
}

#[test]
fn incremental_rewrap_matches_fresh_unicode_wrapping() {
    let cache = MeasureCache::default();
    for content in [
        "first\n👨‍👩‍👧‍👦x\ne\u{301} tail",
        "first\n🇧🇷👨‍👩‍👧‍👦x\ne\u{301} tail",
        "fir \u{301}st\n🇧🇷👨‍👩‍👧‍👦x\ne\u{301} tail",
        "fir \u{301}st\n🇧🇷\n👨‍👩‍👧‍👦x\ne\u{301} tail",
    ] {
        for width in [1.0, 16.0, 40.0] {
            let kept = cache.field_lines(
                "editor",
                content,
                None,
                &FontSpec::DEFAULT,
                width,
                &PixelFont,
            );
            let fresh = break_lines(
                content,
                &FontSpec::DEFAULT,
                width,
                &PixelFont,
                &MeasureCache::default(),
            );
            assert_eq!(*kept, fresh, "{content:?} at {width}");
        }
    }
}
