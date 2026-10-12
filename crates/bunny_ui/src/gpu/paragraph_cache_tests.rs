use super::*;
use crate::text_engine::PixelFont;
use crate::text_source::TextContent;
#[test]
fn paragraph_atlas_source_budget_drops_keys_without_recycling_live_texels() {
    let mut atlas = RunAtlas::new();
    let mut ground = RecordingGround::default();
    let mut previous = None;
    let mut observed_eviction = false;
    for index in 0..40 {
        let content = TextContent::from(format!("{index}{}", "a".repeat(40_000)));
        let line = content.line(content.len() - 1..content.len()).unwrap();
        let prior = atlas.entries.values().map(Vec::len).sum::<usize>();
        let entry = atlas
            .resolve_paragraph(
                &mut ground,
                &line,
                &FontSpec::DEFAULT,
                Color::BLACK,
                1,
                &PixelFont,
            )
            .unwrap()
            .unwrap();
        let tile = (entry.tiles[0].x, entry.tiles[0].y);
        assert_ne!(previous, Some(tile));
        previous = Some(tile);
        let current = atlas.entries.values().map(Vec::len).sum::<usize>();
        observed_eviction |= current < prior;
        assert!(atlas.source_weight <= 16 * 1024 * 1024);
        assert!(atlas.transient.is_none());
    }
    assert!(observed_eviction);
    assert_eq!(ground.uploads.len(), 40);
    assert_eq!(ground.drops, 0);
    let content = TextContent::from("x".repeat(800_000));
    let line = content.line(0..1).unwrap();
    let count = atlas.entries.len();
    let transient = atlas
        .resolve_paragraph(
            &mut ground,
            &line,
            &FontSpec::DEFAULT,
            Color::BLACK,
            1,
            &PixelFont,
        )
        .unwrap()
        .unwrap();
    assert!(
        transient.content.is_none(),
        "the transient tiles must not pin oversized source"
    );
    assert_eq!(atlas.entries.len(), count);
    atlas.reset(&mut ground, false);
    assert_eq!(atlas.source_weight, 0);
    assert!(atlas.transient.is_none());
}
