use super::*;
use crate::text_engine::{FontSpec, LineMetrics, MeasureCache, TextEngine, TextRaster};
use std::cell::Cell;

fn mixed() -> LineGeometry {
    let cells = [
        (0..1, 0.0, 10.0),
        (1..2, 10.0, 20.0),
        (2..4, 40.0, 30.0),
        (4..6, 30.0, 20.0),
        (6..7, 40.0, 50.0),
        (7..8, 50.0, 60.0),
    ]
    .into_iter()
    .map(|(bytes, leading, trailing)| TextCell::new(bytes, leading, trailing).unwrap())
    .collect();
    LineGeometry::new("abאבcd", cells).unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn a_logical_boundary_keeps_both_visual_affinities() {
    let geometry = mixed();
    close(geometry.caret_x(2, CaretAffinity::Downstream), 40.0);
    close(geometry.caret_x(2, CaretAffinity::Upstream), 20.0);
    close(geometry.caret_x(4, CaretAffinity::Downstream), 30.0);
    for x in [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0] {
        let hit = geometry.hit_test(x);
        close(geometry.caret_x(hit.byte, hit.affinity), x);
    }
}

#[test]
fn selection_preserves_visual_holes_between_logical_neighbors() {
    let geometry = mixed();
    let spans = geometry.selection_spans(1..4);
    assert_eq!(spans.len(), 2);
    for (span, (start, end)) in spans.iter().zip([(10.0, 20.0), (30.0, 40.0)]) {
        close(span.start, start);
        close(span.end, end);
    }
    let all = geometry.selection_spans(0..usize::MAX);
    assert_eq!(all.len(), 1);
    close(all[0].start, 0.0);
    close(all[0].end, 60.0);
    assert!(geometry.selection_spans(4..4).is_empty());
}

#[test]
fn malformed_native_cells_never_reach_layout() {
    assert_eq!(
        TextCell::new(0..0, 0.0, 1.0),
        Err(GeometryError::InvalidRange)
    );
    assert_eq!(
        TextCell::new(0..1, f64::NAN, 1.0),
        Err(GeometryError::NonFiniteCoordinate)
    );
    assert_eq!(
        LineGeometry::new("ab", vec![TextCell::new(0..1, 0.0, 1.0).unwrap()]).unwrap_err(),
        GeometryError::SourceCoverage
    );
    assert_eq!(
        LineGeometry::new(
            "é",
            vec![
                TextCell::new(0..1, 0.0, 1.0).unwrap(),
                TextCell::new(1..2, 1.0, 2.0).unwrap()
            ]
        )
        .unwrap_err(),
        GeometryError::SourceCoverage
    );
    let empty = LineGeometry::new("", Vec::new()).unwrap();
    assert_eq!(empty.hit_test(40.0), CaretHit::default());
    close(empty.caret_x(42, CaretAffinity::Upstream), 0.0);
}

#[test]
fn pointer_stops_skip_combining_and_emoji_cluster_interiors() {
    let text = "e\u{301}👨‍👩‍👧‍👦x";
    let cells = text
        .char_indices()
        .enumerate()
        .map(|(number, (byte, character))| {
            let leading = f64::from(u32::try_from(number).unwrap()) * 10.0;
            TextCell::new(byte..byte + character.len_utf8(), leading, leading + 10.0).unwrap()
        })
        .collect();
    let geometry = LineGeometry::new(text, cells).unwrap();
    let legal = [0, 3, text.len() - 1, text.len()];
    for step in 0..110 {
        assert!(legal.contains(&geometry.hit_test(f64::from(step)).byte));
    }
    // Explicit native scalar positions remain addressable inside a grapheme.
    close(geometry.caret_x(1, CaretAffinity::Downstream), 10.0);
}

#[test]
fn streaming_stops_match_every_unicode_17_case() {
    let source = include_str!("../tests/data/unicode-17.0.0/GraphemeBreakTest.txt");
    let mut cases = 0;
    for (line_number, line) in source.lines().enumerate() {
        let data = line.split('#').next().unwrap().trim();
        if data.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut expected = Vec::new();
        for token in data.split_whitespace() {
            match token {
                "÷" => expected.push(text.len()),
                "×" => (),
                code => text.push(char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap()),
            }
        }
        assert_eq!(
            crate::grapheme::boundaries(&text),
            expected,
            "Unicode fixture line {}",
            line_number + 1
        );
        cases += 1;
    }
    assert!(cases > 0, "the complete official fixture was read");
}

#[test]
fn long_regional_indicator_runs_keep_pair_boundaries() {
    let text = "🇦".repeat(20_000);
    let boundaries = crate::grapheme::boundaries(&text);
    assert_eq!(boundaries.len(), 10_001);
    assert!(
        boundaries
            .iter()
            .enumerate()
            .all(|(index, &byte)| byte == index * 8)
    );
}

struct Oracle {
    calls: Cell<u32>,
}
impl TextEngine for Oracle {
    fn line_geometry(&self, text: &str, _: &FontSpec) -> Option<LineGeometry> {
        self.calls.set(self.calls.get() + 1);
        (text == "abאבcd").then(mixed)
    }
    fn measure_line(&self, _: &str, _: &FontSpec) -> LineMetrics {
        LineMetrics {
            width: 60.0,
            ascent: 12.0,
            descent: 3.0,
        }
    }
    fn raster_line(
        &self,
        _: &str,
        _: &FontSpec,
        _: crate::layout::Color,
        _: usize,
    ) -> Option<TextRaster> {
        None
    }
}

#[test]
fn complete_line_geometry_is_reused_and_aged_with_the_cache() {
    let engine = Oracle {
        calls: Cell::new(0),
    };
    let cache = MeasureCache::with_floor(0);
    let a = cache
        .get_or_geometry("abאבcd", &FontSpec::DEFAULT, &engine)
        .unwrap();
    let b = cache
        .get_or_geometry("abאבcd", &FontSpec::DEFAULT, &engine)
        .unwrap();
    assert!(std::rc::Rc::ptr_eq(&a, &b));
    assert_eq!(engine.calls.get(), 1);
    assert!(
        cache
            .get_or_geometry("unsupported", &FontSpec::DEFAULT, &engine)
            .is_none()
    );
    assert!(
        cache
            .get_or_geometry("unsupported", &FontSpec::DEFAULT, &engine)
            .is_none()
    );
    assert_eq!(engine.calls.get(), 2);
    for _ in 0..10 {
        cache.begin_frame();
    }
    assert!(cache.is_empty());
    assert!(
        cache
            .get_or_geometry("abאבcd", &FontSpec::DEFAULT, &engine)
            .is_some()
    );
    assert_eq!(engine.calls.get(), 3);
}

#[test]
fn geometry_cache_distinguishes_text_and_effective_font() {
    let engine = Oracle {
        calls: Cell::new(0),
    };
    let cache = MeasureCache::default();
    for font in [
        FontSpec::DEFAULT,
        FontSpec {
            size: 24.0,
            ..FontSpec::DEFAULT
        },
        FontSpec {
            tracking: 1.0,
            ..FontSpec::DEFAULT
        },
    ] {
        for _ in 0..2 {
            assert!(cache.get_or_geometry("abאבcd", &font, &engine).is_some());
        }
    }
    assert_eq!(engine.calls.get(), 3);
    assert!(
        cache
            .get_or_geometry("changed", &FontSpec::DEFAULT, &engine)
            .is_none()
    );
    assert_eq!(engine.calls.get(), 4);
}

#[test]
fn observational_commands_and_unmark_preserve_native_affinity() {
    use crate::text_input::{CaretState, EditCommand, apply};
    for command in [EditCommand::Read, EditCommand::Copy, EditCommand::Unmark] {
        let mut text = "abאבcd".to_owned();
        let mut state = CaretState {
            caret: 2,
            affinity: CaretAffinity::Upstream,
            marked: Some((0, 2)),
            ..CaretState::default()
        };
        apply(&mut text, &mut state, command);
        assert_eq!(state.affinity, CaretAffinity::Upstream);
        assert_eq!(state.caret, 2);
    }
}
