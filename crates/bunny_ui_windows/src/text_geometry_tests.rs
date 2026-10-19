//! Runtime geometry is checked against DirectWrite's native text-range boxes.
use super::*;
use bunny_ui::prelude::*;
use bunny_ui::text_geometry::CaretAffinity;
use std::rc::Rc;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 0.02, "expected {b}, got {a}");
}
fn native_boxes(
    engine: &DirectWriteEngine,
    text: &str,
    font: &FontSpec,
    start: u32,
    length: u32,
) -> Vec<geometry::HitMetrics> {
    engine
        .with_slot(font, |slot| {
            let layout = create_layout(engine.factories.as_ref().unwrap(), slot, text).unwrap();
            let mut boxes: Vec<_> = (0..32).map(|_| geometry::HitMetrics::default()).collect();
            let mut count = 0;
            let hr = unsafe {
                ((*(*layout.as_ptr()).vtbl).hit_test_text_range)(
                    layout.as_ptr(),
                    start,
                    length,
                    0.,
                    0.,
                    boxes.as_mut_ptr(),
                    32,
                    &raw mut count,
                )
            };
            assert!(com_ok(hr), "HitTestTextRange: {hr:#x}");
            boxes.truncate(count as usize);
            boxes.sort_by(|a, b| a.left.total_cmp(&b.left));
            boxes
        })
        .unwrap()
}
#[test]
fn mixed_geometry_runtime_matches_native_range_boxes() {
    for text in ["abc אבג def", "אבג abc 123"] {
        let engine = Rc::new(DirectWriteEngine::new());
        let runtime = Runtime::new().text_engine(engine.clone());
        let content = State::new(text.to_owned());
        let view = text_field("", content.binding())
            .frame(600., 30.)
            .id("field");
        runtime.render_stable(&view);
        let layout = runtime.layout(
            &view,
            Proposal::exact(Size {
                width: 600.,
                height: 100.,
            }),
        );
        let field = &layout.fields[0];
        runtime.focus_named("field");
        let byte = text.find('ב').unwrap();
        let index = bunny_ui::text_input::byte_to_utf16(text, byte);
        let boxes = native_boxes(&engine, text, &field.font, u32::try_from(index).unwrap(), 1);
        assert_eq!(boxes.len(), 1);
        let native = &boxes[0];
        assert_eq!(native.bidi_level % 2, 1);
        let expected = field.text_origin.x + f64::from(native.left + native.width);
        close(runtime.ime_rect_for(index).unwrap().origin.x, expected);
        assert_eq!(
            runtime.ime_index_at(expected, field.run.origin.y + 5.),
            Some(index)
        );
    }
}
#[test]
fn mixed_geometry_native_selection_has_two_visual_ranges() {
    let engine = DirectWriteEngine::new();
    let text = "abאבcd";
    let font = FontSpec::DEFAULT;
    let geometry = engine.line_geometry(text, &font).unwrap();
    let expected = native_boxes(&engine, text, &font, 1, 2);
    let spans = geometry.selection_spans(1..4);
    assert_eq!(spans.len(), 2);
    assert_eq!(expected.len(), 2);
    for (span, native) in spans.iter().zip(&expected) {
        close(span.start, f64::from(native.left));
        close(span.end, f64::from(native.left + native.width));
    }
    assert!(
        (geometry.caret_x(2, CaretAffinity::Upstream)
            - geometry.caret_x(2, CaretAffinity::Downstream))
        .abs()
            > 1.
    );
}
#[test]
fn mixed_geometry_native_clusters_and_utf16_keep_legal_hits() {
    let engine = DirectWriteEngine::new();
    for (text, legal) in [
        ("e\u{301}x", vec![0, 3, 4]),
        ("a😀ב", vec![0, 1, 5, 7]),
        ("👨‍👩‍👧‍👦x", vec![0, 25, 26]),
    ] {
        let geometry = engine.line_geometry(text, &FontSpec::DEFAULT).unwrap();
        let width = engine.measure_line(text, &FontSpec::DEFAULT).width;
        assert_eq!(geometry.cells().len(), text.chars().count());
        for step in 0..=200 {
            assert!(legal.contains(&geometry.hit_test(width * f64::from(step) / 200.).byte));
        }
        let full = geometry.selection_spans(0..text.len());
        close(full[0].start, 0.);
        close(full.last().unwrap().end, width);
    }
}
#[test]
fn mixed_geometry_ligatures_and_arabic_use_native_full_line_extents() {
    let engine = DirectWriteEngine::new();
    for text in ["office", "العربية 123 Latin", "a\u{2067}אב\u{2069}z"] {
        let font = FontSpec {
            size: 24.,
            ..FontSpec::DEFAULT
        };
        let geometry = engine.line_geometry(text, &font).unwrap();
        let spans = geometry.selection_spans(0..text.len());
        let boxes = native_boxes(
            &engine,
            text,
            &font,
            0,
            u32::try_from(text.encode_utf16().count()).unwrap(),
        );
        close(spans[0].start, f64::from(boxes[0].left));
        let right = boxes
            .iter()
            .map(|b| f64::from(b.left + b.width))
            .fold(0., f64::max);
        close(spans.last().unwrap().end, right);
        assert!(engine.raster_line(text, &font, Color::BLACK, 1).is_some());
    }
}
