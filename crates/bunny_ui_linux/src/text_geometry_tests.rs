//! Native `HarfBuzz` glyph positions are the reference, not prefix measurements.
use super::*;
use bunny_ui::prelude::*;
use bunny_ui::text_engine::Family;
use bunny_ui::text_geometry::CaretAffinity;
use std::rc::Rc;

fn font() -> FontSpec {
    FontSpec {
        family: Family::named("DejaVu Sans"),
        ..FontSpec::DEFAULT
    }
}
fn px(value: i64) -> f64 {
    f64::from(i32::try_from(value).expect("small native font fixture")) / 64.0
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 0.02, "expected {b}, got {a}");
}

#[test]
fn mixed_geometry_runtime_matches_native_hebrew_glyph_positions() {
    for text in ["abc אבג def", "אבג abc 123"] {
        let engine = Rc::new(FreeTypeEngine::new());
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
        assert!(runtime.focus_named("field"));
        let byte = text.find('ב').unwrap();
        let (glyphs, _) = engine
            .shape(text, &FreeTypeEngine::key(&field.font, 1), 0)
            .unwrap();
        let glyph = glyphs.iter().find(|g| g.cluster as usize == byte).unwrap();
        assert!(glyph.rtl);
        let expected = field.text_origin.x + px(glyph.pen + glyph.advance);
        let utf16 = bunny_ui::text_input::byte_to_utf16(text, byte);
        close(runtime.ime_rect_for(utf16).unwrap().origin.x, expected);
        assert_eq!(
            runtime.ime_index_at(expected, field.run.origin.y + 5.),
            Some(utf16)
        );
    }
}

#[test]
fn mixed_geometry_preserves_native_split_affinity_and_disjoint_selection() {
    let engine = FreeTypeEngine::new();
    let text = "abאבcd";
    let font = font();
    let geometry = engine.line_geometry(text, &font).unwrap();
    let (glyphs, _) = engine
        .shape(text, &FreeTypeEngine::key(&font, 1), 0)
        .unwrap();
    let b = glyphs.iter().find(|g| g.cluster == 1).unwrap();
    let alef = glyphs.iter().find(|g| g.cluster == 2).unwrap();
    close(
        geometry.caret_x(2, CaretAffinity::Upstream),
        px(b.pen + b.advance),
    );
    close(
        geometry.caret_x(2, CaretAffinity::Downstream),
        px(alef.pen + alef.advance),
    );
    let spans = geometry.selection_spans(1..4);
    assert_eq!(spans.len(), 2);
    close(spans[0].start, px(b.pen));
    close(spans[0].end, px(b.pen + b.advance));
    close(spans[1].start, px(alef.pen));
    close(spans[1].end, px(alef.pen + alef.advance));
}

#[test]
fn mixed_geometry_covers_clusters_fallback_controls_and_tracking() {
    let engine = FreeTypeEngine::new();
    for text in [
        "office",
        "e\u{301}x",
        "👨‍👩‍👧‍👦x",
        "a😀ב",
        "العربية 123 Latin",
        "a\u{2067}אב\u{2069}z",
    ] {
        for tracking in [0., 1.25] {
            let font = FontSpec { tracking, ..font() };
            let geometry = engine.line_geometry(text, &font).unwrap();
            assert_eq!(geometry.cells().len(), text.chars().count());
            let full = geometry.selection_spans(0..text.len());
            assert!(!full.is_empty());
            close(full[0].start, 0.);
            close(
                full.last().unwrap().end,
                engine.measure_line(text, &font).width,
            );
            for cell in geometry.cells() {
                assert!(cell.leading().is_finite() && cell.trailing().is_finite());
            }
            assert!(engine.raster_line(text, &font, Color::BLACK, 1).is_some());
        }
    }
}

#[test]
fn mixed_geometry_native_clusters_do_not_admit_interior_pointer_stops() {
    let engine = FreeTypeEngine::new();
    for (text, legal) in [
        ("e\u{301}x", vec![0, 3, 4]),
        ("a😀ב", vec![0, 1, 5, 7]),
        ("👨‍👩‍👧‍👦x", vec![0, 25, 26]),
    ] {
        let geometry = engine.line_geometry(text, &font()).unwrap();
        let width = engine.measure_line(text, &font()).width;
        for step in 0..=200 {
            let hit = geometry.hit_test(width * f64::from(step) / 200.);
            assert!(legal.contains(&hit.byte), "{text:?}: {hit:?}");
        }
    }
}
