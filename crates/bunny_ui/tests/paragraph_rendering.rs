//! Display identity, CPU damage and GPU caches consume the same paragraph.
extern crate bunny_ui_core as bunny_ui;
#[path = "support/paragraph_engine.rs"]
mod support;
use bunny_ui::image_engine::RawImages;
use bunny_ui::layout::{Color, DisplayList, DrawCommand, Point};
use bunny_ui::raster::{Surface, rasterize_with};
use bunny_ui::text_engine::FontSpec;
use bunny_ui::text_source::ParagraphLine;
use support::{ContextEngine, contextual_line};

fn command(line: &ParagraphLine) -> DrawCommand {
    let bytes = line.bytes();
    DrawCommand::TextLine {
        origin: Point { x: 2.0, y: 2.0 },
        content: line.paragraph().clone().into(),
        range: (bytes.start, bytes.end),
        color: Color::BLACK,
        font: FontSpec::DEFAULT,
    }
}
#[test]
fn paragraph_rendering_context_only_edit_damages_and_repaints_the_line() {
    let a = contextual_line("\u{202e}");
    let b = contextual_line("\u{202d}");
    assert_eq!(a.text(), b.text());
    assert_ne!(command(&a), command(&b));
    for scale in [1, 2, 3] {
        let engine = ContextEngine::default();
        let mut surface = Surface::new(80 * scale, 32 * scale, scale, Color::WHITE);
        let first = DisplayList::from(vec![command(&a)]);
        surface.frame(first, &engine, &RawImages::default())
            .unwrap();
        let before = surface.bitmap().pixels().to_vec();
        let next = DisplayList::from(vec![command(&b)]);
        assert!(
            !surface
                .frame(next.clone(), &engine, &RawImages::default())
                .unwrap()
                .is_empty()
        );
        let expected = rasterize_with(
            &next,
            80 * scale,
            32 * scale,
            scale,
            Color::WHITE,
            &engine,
            &RawImages::default(),
        )
        .unwrap();
        assert_eq!(surface.bitmap().pixels(), expected.pixels());
        assert_ne!(before, expected.pixels());
        assert!(
            surface
                .frame(next, &engine, &RawImages::default())
                .unwrap()
                .is_empty()
        );
    }
}
#[test]
fn paragraph_rendering_unrelated_hard_paragraph_keeps_damage_identity() {
    let make = |source: &str| {
        let content = bunny_ui::text_source::TextContent::from(source);
        let at = content.find("def").unwrap();
        DrawCommand::TextLine {
            origin: Point::default(),
            content,
            range: (at, at + 3),
            color: Color::BLACK,
            font: FontSpec::DEFAULT,
        }
    };
    assert_eq!(
        make("first\n\u{202e}abc def\u{202c}"),
        make("a changed first paragraph\n\u{202e}abc def\u{202c}")
    );
}
#[cfg(feature = "gpu")]
#[test]
fn paragraph_rendering_atlas_uses_context_font_color_and_scale() {
    use bunny_ui::gpu::walk::{RecordingGround, RunAtlas};
    let engine = ContextEngine::default();
    let mut atlas = RunAtlas::new();
    let mut ground = RecordingGround::default();
    let a = contextual_line("\u{202e}");
    let b = contextual_line("\u{202d}");
    let font = FontSpec::DEFAULT;
    for _ in 0..3 {
        atlas
            .resolve_paragraph(&mut ground, &a, &font, Color::BLACK, 1, &engine)
            .unwrap()
            .unwrap();
        atlas
            .resolve_paragraph(&mut ground, &b, &font, Color::BLACK, 1, &engine)
            .unwrap()
            .unwrap();
    }
    assert_eq!(engine.rasters.get(), 2);
    for (font, color, scale) in [
        (FontSpec { size: 24.0, ..font }, Color::BLACK, 1),
        (font, Color::WHITE, 1),
        (font, Color::BLACK, 2),
    ] {
        atlas
            .resolve_paragraph(&mut ground, &a, &font, color, scale, &engine)
            .unwrap()
            .unwrap();
    }
    assert_eq!(engine.rasters.get(), 5);
    assert_eq!(ground.uploads.len(), 5);
}
