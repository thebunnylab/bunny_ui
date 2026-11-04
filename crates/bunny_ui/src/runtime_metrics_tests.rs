use super::*;
use crate::custom::Metrics;
use crate::layout::Color;
use crate::text_geometry::{LineGeometry, TextCell};

struct WideEngine;

impl TextEngine for WideEngine {
    fn line_geometry(&self, text: &str, font: &FontSpec) -> Option<LineGeometry> {
        (text == "界").then(|| {
            LineGeometry::new(
                text,
                vec![TextCell::new(0..3, 0.0, font.size * 2.0).unwrap()],
            )
            .unwrap()
        })
    }

    fn measure_line(&self, _: &str, font: &FontSpec) -> crate::text_engine::LineMetrics {
        crate::text_engine::LineMetrics {
            width: font.size * 2.0,
            ascent: font.size,
            descent: 0.0,
        }
    }

    fn raster_line(
        &self,
        _: &str,
        _: &FontSpec,
        _: Color,
        _: usize,
    ) -> Option<crate::text_engine::TextRaster> {
        None
    }
}

#[test]
fn scoped_metrics_share_the_custom_surface_cache() {
    let runtime = Runtime::new().text_engine(Rc::new(WideEngine));
    let font = FontSpec::DEFAULT;
    let custom = Metrics::new(&*runtime.text, &runtime.cache, font);
    let painted = custom.line_geometry("界").unwrap();
    let command = runtime.with_metrics(font, |metrics| metrics.line_geometry("界").unwrap());
    assert!(Rc::ptr_eq(&painted, &command));
    let again = runtime.with_metrics(font, |metrics| metrics.line_geometry("界").unwrap());
    assert!(Rc::ptr_eq(&command, &again));
}

#[test]
fn scoped_metrics_keep_the_requested_font_and_unsupported_answer() {
    let runtime = Runtime::new().text_engine(Rc::new(WideEngine));
    let first = runtime.with_metrics(FontSpec::DEFAULT, |metrics| {
        metrics.line_geometry("界").unwrap()
    });
    let font = FontSpec {
        size: 30.0,
        ..FontSpec::DEFAULT
    };
    runtime.with_metrics(font, |metrics| {
        assert_eq!(metrics.font, font);
        assert_eq!(metrics.line("界").width, 60.0);
        assert!(!Rc::ptr_eq(&first, &metrics.line_geometry("界").unwrap()));
        assert!(metrics.line_geometry("unsupported").is_none());
    });
}
