use super::*;
use crate::text_engine::FontSpec;
use crate::text_source::TextContent;
fn line(text: &str) -> ParagraphLine {
    TextContent::from(text).line(0..text.len()).unwrap()
}
const METRICS: LineMetrics = LineMetrics {
    width: 1.0,
    ascent: 1.0,
    descent: 0.0,
};
#[test]
fn paragraph_cache_entry_limit_evicts_cold_queries_and_keeps_recent_geometry() {
    let mut cache = ContextCache::default();
    let font = FontSpec::DEFAULT.key();
    let hot = line("hot");
    cache.set_geometry(font, &hot, None);
    let cold = line("cold");
    cache.set_metrics(font, &cold, METRICS);
    for index in 0..MAX_ENTRIES * 2 {
        assert!(matches!(cache.geometry(font, &hot), Geometry::Unsupported));
        cache.set_metrics(font, &line(&index.to_string()), METRICS);
        assert!(cache.len() <= MAX_ENTRIES);
        assert!(cache.weight <= MAX_WEIGHT);
    }
    assert!(cache.metrics(font, &cold).is_none());
    assert!(matches!(cache.geometry(font, &hot), Geometry::Unsupported));
}
#[test]
fn paragraph_cache_accounts_for_hidden_context_and_bypasses_oversized_entries() {
    let mut cache = ContextCache::default();
    let font = FontSpec::DEFAULT.key();
    for index in 0..50 {
        let content = TextContent::from(format!("{index}{}", "a".repeat(40_000)));
        let small = content.line(content.len() - 1..content.len()).unwrap();
        cache.set_metrics(font, &small, METRICS);
        assert!(cache.weight <= MAX_WEIGHT);
    }
    assert!(cache.len() < 50);
    let content = TextContent::from("x".repeat(MAX_WEIGHT / 24 + 1));
    let oversized = content.line(0..1).unwrap();
    let count = cache.len();
    cache.set_metrics(font, &oversized, METRICS);
    cache.set_geometry(font, &oversized, None);
    assert_eq!(cache.len(), count);
    assert!(cache.metrics(font, &oversized).is_none());
    assert!(matches!(
        cache.geometry(font, &oversized),
        Geometry::Unqueried
    ));
}
