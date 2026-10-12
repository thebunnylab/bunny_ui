//! Public paragraph geometry is checked against independent native range boxes.
//! The native layout chooses wraps; original paragraph/document offsets remain
//! part of each public query rather than being rebuilt from an isolated row.

use super::super::tests::wrapped_layout_with_font;
use crate::text::{
    Com, DirectWriteEngine, FontSpec, IDWriteTextLayout, TextEngine, com_ok, geometry,
};
use bunny_ui::text_engine::Family;
use bunny_ui::text_source::TextContent;
use std::collections::BTreeMap;
use std::ops::Range;

fn native_boxes(layout: &Com<IDWriteTextLayout>, units: Range<usize>) -> Vec<geometry::HitMetrics> {
    assert!(!units.is_empty());
    let start = u32::try_from(units.start).unwrap();
    let length = u32::try_from(units.len()).unwrap();
    let mut needed = 0;
    // SAFETY: exact SDK slot66; null/zero is the documented buffer-size query.
    let status = unsafe {
        ((*(*layout.as_ptr()).vtbl).hit_test_text_range)(
            layout.as_ptr(),
            start,
            length,
            0.0,
            0.0,
            std::ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    assert!(
        com_ok(status) || status == -2_147_024_774,
        "native range size: {status:#x}"
    );
    if needed == 0 {
        assert!(com_ok(status));
        return Vec::new();
    }
    let mut boxes: Vec<_> = (0..needed)
        .map(|_| geometry::HitMetrics::default())
        .collect();
    let mut written = 0;
    // SAFETY: live layout and enough initialized storage for the queried count.
    let status = unsafe {
        ((*(*layout.as_ptr()).vtbl).hit_test_text_range)(
            layout.as_ptr(),
            start,
            length,
            0.0,
            0.0,
            boxes.as_mut_ptr(),
            needed,
            &raw mut written,
        )
    };
    assert!(com_ok(status), "native range boxes: {status:#x}");
    assert!(written <= needed);
    boxes.truncate(written as usize);
    assert!(boxes.iter().all(|value| value.left.is_finite()
        && value.top.is_finite()
        && value.width.is_finite()
        && value.width >= 0.0
        && value.height.is_finite()
        && value.height > 0.0
        && value.is_trimmed == 0));
    boxes
}

// Compare painted area. Native boxes split at direction/font boundaries, while
// public spans merge touching rectangles. Zero-width controls paint no area.
fn area(mut spans: Vec<Range<f64>>) -> Vec<Range<f64>> {
    spans.retain(|span| span.end > span.start + 1e-6);
    spans.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut result: Vec<Range<f64>> = Vec::new();
    for span in spans {
        if let Some(previous) = result.last_mut()
            && span.start <= previous.end + 0.001
        {
            previous.end = previous.end.max(span.end);
        } else {
            result.push(span);
        }
    }
    result
}

fn native_area(layout: &Com<IDWriteTextLayout>, units: Range<usize>) -> Vec<Range<f64>> {
    let boxes = native_boxes(layout, units);
    let visible: Vec<_> = boxes.iter().filter(|value| value.width > 1e-6).collect();
    if let Some(first) = visible.first() {
        assert!(
            visible
                .iter()
                .all(|value| (value.top - first.top).abs() < 0.03),
            "selection must stay within its native row"
        );
    }
    area(
        boxes
            .iter()
            .map(|value| f64::from(value.left)..f64::from(value.left + value.width))
            .collect(),
    )
}

// LineGeometry selects the full advance of every touched scalar cell. Native
// cells can share an indivisible glyph cluster. HitTestTextRange instead snaps
// some interior endpoints down, even returning an empty box for just a base
// without its combining mark. Resolve the touched regions independently through
// HitTestTextPosition before comparing that existing whole-cell policy. Raw
// range differences remain counted below; cluster-aligned ranges are unchanged.
fn native_region(layout: &Com<IDWriteTextLayout>, unit: usize) -> Range<usize> {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut metrics = geometry::HitMetrics::default();
    // SAFETY: live independent layout; valid source position and SDK out fields.
    let status = unsafe {
        ((*(*layout.as_ptr()).vtbl).hit_test_text_position)(
            layout.as_ptr(),
            u32::try_from(unit).unwrap(),
            0,
            &raw mut x,
            &raw mut y,
            &raw mut metrics,
        )
    };
    assert!(com_ok(status));
    let region = metrics.position as usize..(metrics.position + metrics.length) as usize;
    assert!(
        region.contains(&unit),
        "native region {region:?} omits {unit}"
    );
    region
}

fn same_area(a: &[Range<f64>], b: &[Range<f64>]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| (a.start - b.start).abs() < 0.03 && (a.end - b.end).abs() < 0.03)
}

fn indices(text: &str) -> Vec<(usize, usize)> {
    let mut unit = 0;
    let mut map: Vec<_> = text
        .char_indices()
        .map(|(byte, character)| {
            let pair = (byte, unit);
            unit += character.len_utf16();
            pair
        })
        .collect();
    map.push((text.len(), unit));
    map
}

const CORPUS: [&str; 12] = [
    "office affine ffi fi fl",
    "a\u{301} e\u{301}x",
    "العربية لا ل\u{200d}ا",
    "abc אבג 123 def",
    "a\u{2067}אב\u{2069}z",
    "a\u{202e}bc\u{202c}z",
    "\u{202e}abc\u{202c} xyz",
    "\u{202d}אבג\u{202c} xyz",
    "\u{2067}אבג\u{2069}",
    "\u{202e}abc def ghi jkl\u{202c}",
    "a😀b 👨‍👩‍👧‍👦x",
    "नमस्तेक्ष हिन्दी",
];

#[test]
fn paragraph_public_wrapped_selection_matches_native_range_boxes() {
    let engine = DirectWriteEngine::new();
    let mut failures = Vec::new();
    let mut failed = 0;
    let mut checked = 0;
    let mut raw_differences = BTreeMap::new();
    let mut aligned = 0;
    for family in [Family::default(), Family::named("Calibri")] {
        for tracking in [-0.75, 0.0, 0.75] {
            let font = FontSpec {
                family,
                size: 21.0,
                tracking,
                ..FontSpec::DEFAULT
            };
            let mut configuration_count = 0;
            for text in CORPUS {
                let prefix = "unrelated א😀\n";
                let content = TextContent::from(format!("{prefix}{text}\nfollowing paragraph"));
                let map = indices(text);
                for width in [65.0, 130.0, 400.0] {
                    let (layout, lines) = wrapped_layout_with_font(&engine, text, width, &font);
                    for units in lines {
                        let edges: Vec<_> = map
                            .iter()
                            .copied()
                            .filter(|&(_, unit)| units.start <= unit && unit <= units.end)
                            .collect();
                        assert_eq!(edges.first().unwrap().1, units.start);
                        assert_eq!(edges.last().unwrap().1, units.end);
                        let bytes = edges.first().unwrap().0..edges.last().unwrap().0;
                        let source = content
                            .line(prefix.len() + bytes.start..prefix.len() + bytes.end)
                            .unwrap();
                        assert_eq!(source.paragraph().text(), text);
                        assert_eq!(source.bytes(), bytes);
                        assert_eq!(source.utf16(), units);
                        let actual = engine
                            .paragraph_line_geometry(&source, &font)
                            .unwrap()
                            .unwrap();
                        assert!(actual.selection_spans(0..0).is_empty());
                        let regions: Vec<_> = edges[..edges.len() - 1]
                            .iter()
                            .map(|&(_, unit)| native_region(&layout, unit))
                            .collect();
                        for (index, &(start, start_unit)) in edges.iter().enumerate() {
                            for (last, &(end, end_unit)) in edges.iter().enumerate().skip(index + 1)
                            {
                                let selected = start - bytes.start..end - bytes.start;
                                let requested = start_unit..end_unit;
                                let covered = regions[index].start..regions[last - 1].end;
                                assert!(units.start <= covered.start && covered.end <= units.end);
                                let raw = native_area(&layout, requested.clone());
                                let expected = if covered == requested {
                                    aligned += 1;
                                    raw.clone()
                                } else {
                                    native_area(&layout, covered.clone())
                                };
                                let spans = area(actual.selection_spans(selected.clone()));
                                checked += 1;
                                configuration_count += 1;
                                if !same_area(&spans, &raw) {
                                    *raw_differences.entry(text).or_insert(0) += 1;
                                }
                                if !same_area(&spans, &expected) {
                                    failed += 1;
                                    if failures.len() < 32 {
                                        failures.push(format!("{text:?}, {family:?}, tracking={tracking}, width={width}, row={units:?}, bytes={selected:?}, units={requested:?}, covered={covered:?}: {spans:?} vs {expected:?}"));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            eprintln!(
                "public paragraph selection matrix: family={family:?}, tracking={tracking}, comparisons={configuration_count}"
            );
        }
    }
    eprintln!("raw native range differences by corpus: {raw_differences:?}");
    eprintln!("cluster-aligned native range comparisons: {aligned}/{checked}");
    assert!(checked > 1000 && aligned > 1000);
    assert_eq!(
        failed, 0,
        "{failed}/{checked} native selection differences: {failures:#?}"
    );
}

#[test]
fn paragraph_partial_cluster_selection_keeps_the_native_cell_visible() {
    let engine = DirectWriteEngine::new();
    let text = "a\u{301}x";
    let font = FontSpec {
        size: 21.0,
        ..FontSpec::DEFAULT
    };
    let (native, _) = wrapped_layout_with_font(&engine, text, 400.0, &font);
    assert_eq!(native_region(&native, 0), 0..2);
    assert_eq!(native_region(&native, 1), 0..2);
    assert!(native_area(&native, 0..1).is_empty());
    let whole = native_area(&native, 0..2);
    assert_eq!(whole.len(), 1);
    let content = TextContent::from(text);
    let source = content.line(0..text.len()).unwrap();
    let geometry = engine
        .paragraph_line_geometry(&source, &font)
        .unwrap()
        .unwrap();
    assert!(same_area(&area(geometry.selection_spans(0..1)), &whole));
    assert!(same_area(&area(geometry.selection_spans(1..3)), &whole));
    assert!(same_area(&area(geometry.selection_spans(0..3)), &whole));
    eprintln!(
        "partial native cluster: raw base selection is empty; public base/mark/whole selections preserve the full native cell"
    );
}

#[path = "text_paragraph_composition_tests.rs"]
mod composition;
