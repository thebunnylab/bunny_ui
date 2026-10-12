//! Native refusal must reach callers without caching or painting a false success.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::image_engine::RawImages;
use bunny_ui::layout::{Color, DisplayList, DrawCommand, Point};
use bunny_ui::raster::{Surface, rasterize_with};
use bunny_ui::text_engine::{
    FontSpec, LineMetrics, MeasureCache, PixelFont, TextEngine, TextRaster,
};
use bunny_ui::text_error::TextError;
use bunny_ui::text_geometry::LineGeometry;
use bunny_ui::text_source::{ParagraphLine, TextContent};
use std::cell::Cell;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Failure {
    None,
    Measure,
    Geometry,
    Raster,
}
struct Engine {
    failure: Cell<Failure>,
}
const fn refused() -> TextError {
    TextError::Preparation {
        engine: "test",
        operation: "paragraph",
    }
}
impl TextEngine for Engine {
    fn measure_line(&self, _: &str, _: &FontSpec) -> LineMetrics {
        panic!("a contextual failure must not measure an isolated fallback")
    }
    fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
        panic!("a contextual failure must not rasterize an isolated fallback")
    }
    fn line_geometry(&self, _: &str, _: &FontSpec) -> Option<LineGeometry> {
        panic!("a contextual failure must not query isolated geometry")
    }
    fn measure_paragraph_line(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<LineMetrics, TextError> {
        if self.failure.get() == Failure::Measure {
            return Err(refused());
        }
        Ok(PixelFont.measure_line(line.text(), font))
    }
    fn paragraph_line_geometry(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<Option<LineGeometry>, TextError> {
        if self.failure.get() == Failure::Geometry {
            return Err(refused());
        }
        let text = line.text();
        let cells = text
            .char_indices()
            .map(|(start, c)| {
                let end = start + c.len_utf8();
                bunny_ui::text_geometry::TextCell::new(
                    start..end,
                    PixelFont.measure_line(&text[..start], font).width,
                    PixelFont.measure_line(&text[..end], font).width,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(LineGeometry::new(text, cells)?))
    }
    fn raster_paragraph_line(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Result<Option<TextRaster>, TextError> {
        if self.failure.get() == Failure::Raster {
            return Err(refused());
        }
        Ok(PixelFont.raster_line(line.text(), font, color, scale))
    }
}
fn line() -> ParagraphLine {
    TextContent::from("native paragraph").line(0..16).unwrap()
}
fn display(text: &str, color: Color) -> DisplayList {
    DisplayList::from(vec![DrawCommand::TextLine {
        origin: Point { x: 2.0, y: 2.0 },
        content: TextContent::from(text),
        range: (0, text.len()),
        font: FontSpec::DEFAULT,
        color,
    }])
}
#[test]
fn failed_measurement_is_not_cached_and_recovers() {
    let engine = Engine {
        failure: Cell::new(Failure::Measure),
    };
    let cache = MeasureCache::default();
    let line = line();
    for _ in 0..2 {
        assert_eq!(
            cache.get_or_measure_paragraph(&line, &FontSpec::DEFAULT, &engine),
            Err(refused())
        );
    }
    engine.failure.set(Failure::None);
    assert!(
        cache
            .get_or_measure_paragraph(&line, &FontSpec::DEFAULT, &engine)
            .unwrap()
            .width
            > 0.0
    );
}
#[test]
fn failed_geometry_is_not_unsupported_and_never_uses_prefix_measurement() {
    let engine = Engine {
        failure: Cell::new(Failure::Geometry),
    };
    let cache = MeasureCache::default();
    let line = line();
    assert!(
        matches!(cache.get_or_paragraph_geometry(&line, &FontSpec::DEFAULT, &engine), Err(error) if error == refused())
    );
    engine.failure.set(Failure::None);
    assert!(
        cache
            .get_or_paragraph_geometry(&line, &FontSpec::DEFAULT, &engine)
            .unwrap()
            .is_some()
    );
}
#[test]
fn failed_full_raster_returns_no_bitmap() {
    let engine = Engine {
        failure: Cell::new(Failure::Raster),
    };
    let result = rasterize_with(
        &display("native", Color::BLACK),
        120,
        30,
        1,
        Color::WHITE,
        &engine,
        &RawImages::default(),
    );
    assert!(matches!(result, Err(error) if error == refused()));
}
#[test]
fn failed_retained_frame_preserves_pixels_and_retries_the_same_list() {
    let engine = Engine {
        failure: Cell::new(Failure::None),
    };
    let images = RawImages::default();
    let mut surface = Surface::new(180, 50, 1, Color::WHITE);
    surface
        .frame(display("before", Color::BLACK), &engine, &images)
        .unwrap();
    let before = surface.bitmap().pixels().to_vec();
    let next = display("after failure", Color::rgb(170, 10, 30));
    engine.failure.set(Failure::Raster);
    assert_eq!(
        surface.frame(next.clone(), &engine, &images),
        Err(refused())
    );
    assert_eq!(surface.bitmap().pixels(), before);
    engine.failure.set(Failure::None);
    assert!(
        !surface
            .frame(next.clone(), &engine, &images)
            .unwrap()
            .is_empty()
    );
    let expected = rasterize_with(&next, 180, 50, 1, Color::WHITE, &engine, &images).unwrap();
    assert_eq!(surface.bitmap().pixels(), expected.pixels());
    assert!(surface.frame(next, &engine, &images).unwrap().is_empty());
}
#[cfg(feature = "gpu")]
#[test]
fn native_failure_is_not_atlas_exhaustion_and_does_not_upload_a_fallback() {
    use bunny_ui::gpu::walk::{RecordingGround, RenderError, RunAtlas};
    let engine = Engine {
        failure: Cell::new(Failure::Raster),
    };
    let mut atlas = RunAtlas::new();
    let mut ground = RecordingGround::default();
    let line = line();
    assert!(
        matches!(atlas.resolve_paragraph(&mut ground, &line, &FontSpec::DEFAULT, Color::BLACK, 1, &engine), Err(RenderError::Text(error)) if error == refused())
    );
    assert!(ground.uploads.is_empty());
    engine.failure.set(Failure::None);
    assert!(
        atlas
            .resolve_paragraph(
                &mut ground,
                &line,
                &FontSpec::DEFAULT,
                Color::BLACK,
                1,
                &engine
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn runtime_layout_reports_preparation_failure_and_retries() {
    use bunny_ui::prelude::*;
    use std::rc::Rc;
    let engine = Rc::new(Engine {
        failure: Cell::new(Failure::Measure),
    });
    let runtime = Runtime::new().text_engine(engine.clone());
    let view = text("native paragraph");
    let size = Size {
        width: 200.0,
        height: 50.0,
    };
    assert!(matches!(runtime.display_frame(&view, size), Err(error) if error == refused()));
    engine.failure.set(Failure::None);
    assert!(!runtime.display_frame(&view, size).unwrap().is_empty());
}

#[test]
fn focused_ime_geometry_reports_failure_without_an_isolated_fallback() {
    use bunny_ui::prelude::*;
    use std::rc::Rc;
    let engine = Rc::new(Engine {
        failure: Cell::new(Failure::Geometry),
    });
    let runtime = Runtime::new().text_engine(engine.clone());
    let state = State::new("native paragraph".to_owned());
    let view = text_field("", state.binding()).id("field");
    let size = Size {
        width: 240.0,
        height: 50.0,
    };
    runtime.display_frame(&view, size).unwrap();
    assert!(runtime.focus_named("field"));
    assert!(matches!(runtime.ime_rect_for(2), Err(error) if error == refused()));
    assert!(matches!(runtime.ime_snapshot(), Err(error) if error == refused()));
    engine.failure.set(Failure::None);
    assert!(runtime.ime_rect_for(2).unwrap().is_some());
    assert!(runtime.ime_snapshot().unwrap().is_some());
}
