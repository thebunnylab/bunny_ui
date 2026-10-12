//! Compare retained editor geometry with the actual native line that draws it.
use super::*;
use bunny_ui::prelude::*;
use std::rc::Rc;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTLineGetOffsetForStringIndex(line: CTLineRef, index: isize, secondary: *mut f64) -> f64;
}

fn native_offset(engine: &CoreTextEngine, text: &str, font: &FontSpec, index: usize) -> f64 {
    let index = isize::try_from(index).expect("small fixture");
    unsafe {
        let line = make_line(text, engine.font(font), font);
        assert!(!line.is_null());
        let result = CTLineGetOffsetForStringIndex(line, index, std::ptr::null_mut());
        CFRelease(line);
        result
    }
}

fn assert_native_caret(text: &str, utf16: usize, pointer: bool) {
    let engine = Rc::new(CoreTextEngine::new());
    let runtime = Runtime::new().text_engine(engine.clone());
    let content = State::new(text.to_owned());
    let view = text_field("", content.binding())
        .frame(600.0, 30.0)
        .id("field");
    runtime.render_stable(&view);
    let layout = runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 600.0,
            height: 100.0,
        }),
    )
        .unwrap();
    let field = &layout.fields[0];
    assert!(runtime.focus_named("field"));
    let native = field.text_origin.x + native_offset(&engine, text, &field.font, utf16);
    if pointer {
        let y = field.run.origin.y + field.line_height / 2.0;
        assert_eq!(
            runtime.ime_index_at(native, y).unwrap(),
            Some(utf16),
            "native pointer index for {text:?}"
        );
    } else {
        let actual = runtime.ime_rect_for(utf16)
            .unwrap().expect("focused field").origin.x;
        assert!(
            (actual - native).abs() < 0.01,
            "native caret for {text:?} at UTF-16 {utf16}: expected {native}, got {actual}"
        );
    }
}

#[test]
fn mixed_geometry_ltr_caret_matches_native_line() {
    assert_native_caret("abc אבג def", 5, false);
}

#[test]
fn mixed_geometry_rtl_caret_matches_native_line() {
    assert_native_caret("אבג abc 123", 5, false);
}

#[test]
fn mixed_geometry_pointer_round_trip_uses_native_line() {
    assert_native_caret("abc אבג def", 5, true);
}

#[test]
fn mixed_geometry_native_edges_keep_split_affinity() {
    use bunny_ui::text_geometry::CaretAffinity;
    let engine = CoreTextEngine::new();
    for text in ["abאבcd", "e\u{301}x", "a😀b", "office"] {
        let font = FontSpec::DEFAULT;
        let raw = unsafe { make_line(text, engine.font(&font), &font) };
        assert!(!raw.is_null());
        let line = geometry::OwnedLine(raw);
        let edges = geometry::caret_edges(line.0);
        let cells = engine.line_geometry(text, &font).unwrap();
        for edge in edges {
            let (units, affinity) = if edge.leading {
                (edge.index, CaretAffinity::Downstream)
            } else {
                (edge.index + 1, CaretAffinity::Upstream)
            };
            let byte = bunny_ui::text_input::utf16_to_byte(text, units);
            assert!(
                (cells.caret_x(byte, affinity) - edge.offset).abs() < 0.01,
                "native edge {text:?} at {units}: {affinity:?}"
            );
        }
    }
}

#[test]
fn mixed_geometry_native_selection_preserves_the_unselected_hebrew_cell() {
    let engine = CoreTextEngine::new();
    let text = "abאבcd";
    let font = FontSpec::DEFAULT;
    let geometry = engine.line_geometry(text, &font).unwrap();
    let spans = geometry.selection_spans(1..4);
    assert_eq!(spans.len(), 2);
    // Native offsets at directional interiors delimit b and alef independently.
    let b_left = native_offset(&engine, text, &font, 1);
    let bet_alef = native_offset(&engine, text, &font, 3);
    assert!((spans[0].start - b_left).abs() < 0.01);
    assert!((spans[1].start - bet_alef).abs() < 0.01);
    assert!(spans[0].end < spans[1].start);
}

#[test]
fn mixed_geometry_wrapped_rows_match_their_native_line_and_alignment() {
    use bunny_ui::layout::DrawCommand;
    for alignment in [
        TextAlignment::Leading,
        TextAlignment::Center,
        TextAlignment::Trailing,
    ] {
        let engine = Rc::new(CoreTextEngine::new());
        let runtime = Runtime::new().text_engine(engine.clone());
        let text = "אבג abc def אבג abc def אבג abc def";
        let content = State::new(text.to_owned());
        let view = text_editor("", content.binding())
            .multiline_text_alignment(alignment)
            .frame(130.0, 150.0)
            .id("field");
        runtime.render_stable(&view);
        let layout = runtime.layout(
            &view,
            Proposal::exact(Size {
                width: 130.0,
                height: 150.0,
            }),
        )
            .unwrap();
        runtime.focus_named("field");
        let mut checked = 0;
        let mut context_changed = 0;
        for command in layout.display.iter() {
            if let DrawCommand::TextLine {
                content,
                range,
                origin,
                font,
                ..
            } = command
            {
                let line = &content[range.0..range.1];
                for (local, _) in line
                    .char_indices()
                    .filter(|(_, ch)| *ch == 'ב' || *ch == 'b')
                {
                    let unit = bunny_ui::text_input::byte_to_utf16(line, local);
                    let global = bunny_ui::text_input::byte_to_utf16(text, range.0 + local);
                    let isolated = native_offset(&engine, line, font, unit);
                    let contextual = super::paragraph_tests::paragraph_offset(
                        &engine,
                        text,
                        range.0..range.1,
                        font,
                        global,
                    );
                    context_changed += usize::from((isolated - contextual).abs() > 0.01);
                    // Soft wrapping keeps the paragraph base direction and its
                    // resolved neutrals. A standalone substring is a different oracle.
                    let expected = origin.x + contextual;
                    assert!(
                        (runtime.ime_rect_for(global).unwrap().unwrap().origin.x - expected).abs() < 0.01
                    );
                    assert_eq!(runtime.ime_index_at(expected, origin.y + 5.0).unwrap(), Some(global));
                    checked += 1;
                }
            }
        }
        assert!(
            context_changed > 0,
            "the fixture distinguishes paragraph context from isolated lines"
        );
        assert!(
            checked >= 6,
            "all repeated mixed runs across wrapped lines were checked"
        );
    }
}

#[test]
fn mixed_geometry_emoji_and_combining_marks_only_offer_grapheme_hits() {
    let engine = CoreTextEngine::new();
    for (text, legal) in [
        ("e\u{301}x", vec![0, 3, 4]),
        ("a😀ב", vec![0, 1, 5, 7]),
        ("👨‍👩‍👧‍👦x", vec![0, 25, 26]),
    ] {
        let geometry = engine.line_geometry(text, &FontSpec::DEFAULT).unwrap();
        let width = engine.measure_line(text, &FontSpec::DEFAULT).width;
        for step in 0..=200 {
            assert!(legal.contains(&geometry.hit_test(width * f64::from(step) / 200.0).byte));
        }
    }
}
