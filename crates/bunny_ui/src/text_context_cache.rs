//! Bounded memoization keys include the full paragraph and relative line range.
use super::{FontKey, LineMetrics};
use crate::text_geometry::LineGeometry;
use crate::text_source::ParagraphLine;
use motor::hash::FxHashMap as HashMap;
use std::cell::Cell;
use std::rc::Rc;

const MAX_ENTRIES: usize = 4096;
const MAX_WEIGHT: usize = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key(FontKey, ParagraphLine);
#[derive(Clone, Default)]
pub(super) enum Geometry {
    #[default]
    Unqueried,
    Unsupported,
    Ready(Rc<LineGeometry>),
}
#[derive(Default)]
struct Entry {
    metrics: Option<LineMetrics>,
    geometry: Geometry,
    used: Cell<u64>,
    weight: usize,
}

#[derive(Default)]
pub(super) struct ContextCache {
    entries: HashMap<Key, Entry>,
    tick: Cell<u64>,
    weight: usize,
}
impl ContextCache {
    fn next_tick(&self) -> u64 {
        let next = self.tick.get().wrapping_add(1);
        self.tick.set(next);
        next
    }
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
    pub(super) fn metrics(&self, font: FontKey, line: &ParagraphLine) -> Option<LineMetrics> {
        let entry = self.entries.get(&Key(font, line.clone()))?;
        entry.used.set(self.next_tick());
        entry.metrics
    }
    pub(super) fn geometry(&self, font: FontKey, line: &ParagraphLine) -> Geometry {
        let Some(entry) = self.entries.get(&Key(font, line.clone())) else {
            return Geometry::Unqueried;
        };
        entry.used.set(self.next_tick());
        entry.geometry.clone()
    }
    pub(super) fn set_metrics(
        &mut self,
        font: FontKey,
        line: &ParagraphLine,
        metrics: LineMetrics,
    ) {
        if let Some(entry) = self.entry(&Key(font, line.clone())) {
            entry.metrics = Some(metrics);
        }
    }
    pub(super) fn set_geometry(
        &mut self,
        font: FontKey,
        line: &ParagraphLine,
        value: Option<Rc<LineGeometry>>,
    ) {
        if let Some(entry) = self.entry(&Key(font, line.clone())) {
            entry.geometry = value.map_or(Geometry::Unsupported, Geometry::Ready);
        }
    }
    fn entry(&mut self, key: &Key) -> Option<&mut Entry> {
        let used = self.next_tick();
        if !self.entries.contains_key(key) {
            // Charge the shared paragraph conservatively for every key. This
            // overestimates sharing, but bounds retained source and geometry.
            let weight = key
                .1
                .paragraph()
                .text()
                .len()
                .saturating_mul(24)
                .saturating_add(key.1.text().len().saturating_mul(192))
                .saturating_add(256);
            if weight > MAX_WEIGHT {
                return None;
            }
            while self.entries.len() >= MAX_ENTRIES || self.weight + weight > MAX_WEIGHT {
                self.evict_batch(used);
            }
            self.weight += weight;
            self.entries.insert(
                key.clone(),
                Entry {
                    weight,
                    ..Entry::default()
                },
            );
        }
        let entry = self.entries.get_mut(key)?;
        entry.used.set(used);
        Some(entry)
    }
    fn evict_batch(&mut self, now: u64) {
        let count = self.entries.len().div_ceil(4);
        if count == 0 {
            return;
        }
        let mut oldest: Vec<_> = self
            .entries
            .iter()
            .map(|(key, entry)| {
                (
                    std::cmp::Reverse(now.wrapping_sub(entry.used.get())),
                    key.clone(),
                )
            })
            .collect();
        oldest.select_nth_unstable_by_key(count - 1, |(age, _)| *age);
        for (_, key) in oldest.into_iter().take(count) {
            if let Some(entry) = self.entries.remove(&key) {
                self.weight -= entry.weight;
            }
        }
    }
}

#[cfg(test)]
#[path = "text_context_cache_tests.rs"]
mod tests;
