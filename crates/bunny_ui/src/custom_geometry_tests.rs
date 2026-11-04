use super::Metrics;
use crate::layout::Color;
use crate::text_engine::{FontSpec, LineMetrics, MeasureCache, TextEngine, TextRaster};
use crate::text_geometry::{CaretAffinity, LineGeometry, TextCell};
use std::cell::Cell;
use std::rc::Rc;

const SOURCE: &str = "a界e\u{301}🙂;";

#[derive(Default)]
struct GeometryEngine {
    calls: Cell<usize>,
}

impl TextEngine for GeometryEngine {
    fn line_geometry(&self, text: &str, font: &FontSpec) -> Option<LineGeometry> {
        self.calls.set(self.calls.get() + 1);
        if text != SOURCE {
            return None;
        }
        // Nonuniform native advances: wide CJK and emoji, a zero-width mark.
        let edges = [0.0, 1.0, 3.0, 4.0, 4.0, 6.0, 7.0];
        let cells = text
            .char_indices()
            .enumerate()
            .map(|(index, (byte, character))| {
                TextCell::new(
                    byte..byte + character.len_utf8(),
                    edges[index] * font.size,
                    edges[index + 1] * font.size,
                )
                .unwrap()
            })
            .collect();
        Some(LineGeometry::new(text, cells).unwrap())
    }

    fn measure_line(&self, _: &str, font: &FontSpec) -> LineMetrics {
        LineMetrics {
            width: 7.0 * font.size,
            ascent: font.size,
            descent: 0.0,
        }
    }

    fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
        None
    }
}

#[test]
fn custom_metrics_preserve_the_complete_native_geometry() {
    let engine = GeometryEngine::default();
    let cache = MeasureCache::default();
    let font = FontSpec {
        size: 10.0,
        ..FontSpec::DEFAULT
    };
    let metrics = Metrics::new(&engine, &cache, font);
    let geometry = metrics.line_geometry(SOURCE).unwrap();
    let again = metrics.line_geometry_in(SOURCE, &font).unwrap();
    assert!(Rc::ptr_eq(&geometry, &again));
    assert_eq!(engine.calls.get(), 1);
    assert!((geometry.caret_x(4, CaretAffinity::Downstream) - 30.0).abs() < 1e-9);
    assert!((geometry.caret_x(11, CaretAffinity::Downstream) - 60.0).abs() < 1e-9);
    assert_eq!(geometry.hit_test(59.0).byte, 11);
    assert_eq!(geometry.selection_spans(1..4), vec![10.0..30.0]);
    assert_eq!(geometry.cells()[3].bytes(), 5..7);
}

#[test]
fn custom_geometry_tracks_font_changes_and_cache_aging() {
    let engine = GeometryEngine::default();
    let cache = MeasureCache::with_floor(0);
    let metrics = Metrics::new(&engine, &cache, FontSpec::DEFAULT);
    let original = metrics.line_geometry(SOURCE).unwrap();
    let larger = FontSpec {
        size: 28.0,
        ..FontSpec::DEFAULT
    };
    let changed = metrics.line_geometry_in(SOURCE, &larger).unwrap();
    assert!(!Rc::ptr_eq(&original, &changed));
    assert!((changed.caret_x(11, CaretAffinity::Downstream) - 168.0).abs() < 1e-9);
    assert_eq!(engine.calls.get(), 2);
    for _ in 0..10 {
        cache.begin_frame();
    }
    assert!(cache.is_empty());
    let fresh = metrics.line_geometry(SOURCE).unwrap();
    assert!(!Rc::ptr_eq(&original, &fresh));
    assert_eq!(engine.calls.get(), 3);
}

#[test]
fn custom_metrics_cache_unsupported_geometry_without_inventing_cells() {
    let engine = GeometryEngine::default();
    let cache = MeasureCache::default();
    let metrics = Metrics::new(&engine, &cache, FontSpec::DEFAULT);
    for _ in 0..2 {
        assert!(metrics.line_geometry("unsupported").is_none());
        assert!(
            metrics
                .line_geometry_in("unsupported", &FontSpec::DEFAULT)
                .is_none()
        );
    }
    assert_eq!(engine.calls.get(), 1);
    assert!(metrics.line_geometry(SOURCE).is_some());
    assert_eq!(engine.calls.get(), 2);
}
