use super::*;
#[path = "../../../bunny_ui/tests/support/paragraph_engine.rs"]
mod support;
use bunny_ui::image_engine::RawImages;
use bunny_ui::layout::Point;
use bunny_ui::text_source::TextContent;
use support::{ContextEngine, contextual_line};

#[test]
fn paragraph_cached_decorator_forwards_context_and_does_not_alias_rasters() {
    let engine = ContextEngine::default();
    let cache = RasterCache::default();
    let cached = CachedText {
        engine: &engine,
        cache: &cache,
    };
    let a = contextual_line("\u{202e}");
    let b = contextual_line("\u{202d}");
    let font = FontSpec::DEFAULT;
    let mut images = Vec::new();
    for line in [&a, &b, &a, &b] {
        assert!(cached.measure_paragraph_line(line, &font).unwrap().width > 0.0);
        assert!(cached.paragraph_line_geometry(line, &font).unwrap().is_some());
        images.push(
            cached
                .raster_paragraph_line(line, &font, Color::BLACK, 1)
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(engine.rasters.get(), 2);
    assert_eq!(engine.geometries.get(), 4);
    assert_eq!(images[0].rgba, images[2].rgba);
    assert_eq!(images[1].rgba, images[3].rgba);
    assert_ne!(images[0].rgba, images[1].rgba);
    assert_eq!(cache.entries.borrow().len(), 2);
    let source = TextContent::from("x".repeat(100_000));
    let oversized = source.line(0..1).unwrap();
    cached
        .raster_paragraph_line(&oversized, &font, Color::BLACK, 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        cache.entries.borrow().len(),
        2,
        "oversized paragraph bypasses retention"
    );
    assert!(cache.bytes.get() <= 2 * 1024 * 1024);
}

#[test]
fn paragraph_patch_compaction_keeps_context_and_releases_unrelated_document() {
    for scale in [1_u32, 2, 3] {
        let factor = f64::from(scale);
        let physical = (0, 0, i64::from(80 * scale), i64::from(32 * scale));
        let scale = usize::try_from(scale).unwrap();
        let source: std::sync::Arc<str> = format!(
            "{}\u{202e}abc def\u{202c}\n{}",
            "hidden\n".repeat(4096),
            "tail\n".repeat(4096)
        )
        .into();
        let lifetime = std::sync::Arc::downgrade(&source);
        let content = TextContent::from(source);
        let at = content.find("def").unwrap();
        let display = DisplayList::from(vec![DrawCommand::TextLine {
            origin: Point { x: 2.0, y: 2.0 },
            content,
            range: (at, at + 3),
            color: Color::BLACK,
            font: FontSpec::DEFAULT,
        }]);
        let engine = ContextEngine::default();
        let expected = rasterize_with(
            &display,
            80 * scale,
            32 * scale,
            scale,
            Color::WHITE,
            &engine,
            &RawImages::default(),
        )
        .unwrap();
        let compact = software_patch::patch_coordinates(&display, physical, factor).unwrap();
        assert_eq!(display.as_slice(), compact.as_slice());
        let DrawCommand::TextLine { content, range, .. } = &compact.as_slice()[0] else {
            panic!("text retained")
        };
        assert_eq!(
            content.line(range.0..range.1).unwrap().paragraph().text(),
            "\u{202e}abc def\u{202c}"
        );
        let scene = software_patch::Scene::prepare_scene(
            &display,
            physical,
            scale,
            Color::WHITE,
            &engine,
            &MeasureCache::default(),
        )
        .unwrap()
        .unwrap();
        drop(display);
        assert!(lifetime.upgrade().is_none());
        assert_eq!(
            scene.raster(&engine, &RawImages::default()).unwrap().pixels(),
            expected.pixels()
        );
        let mut retained = None;
        assert_eq!(
            scene
                .raster_in(&mut retained, &engine, &RawImages::default())
                .unwrap()
                .pixels(),
            expected.pixels()
        );
    }
}
