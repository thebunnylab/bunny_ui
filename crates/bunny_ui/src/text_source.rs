//! A visual line keeps the paragraph that determines its directional context.
//!
//! ## Wiring
//! `TextContent` indexes the layout's LF-delimited paragraphs once. Validated
//! `ParagraphLine` values carry a shared paragraph and relative UTF-8 range to
//! measurement, editing and raster adapters. Native indices are mapped lazily
//! once per paragraph. No native handle or mutable application text lives here.
//!
//! The runtime's bounded content cache interns unchanged paragraphs across
//! edits. A cached line retains its paragraph, not a hidden copy of the entire
//! document; hashing a line never rescans the document or paragraph.

use motor::hash::{FxHashMap as HashMap, FxHasher};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, Range};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// A source range rejected before it reaches a native text adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRangeError {
    /// The range is reversed or extends beyond its source.
    InvalidRange,
    /// An endpoint splits a UTF-8 scalar.
    NotBoundary,
    /// A single visual line crosses the layout's hard LF separator.
    CrossesParagraph,
}
impl std::fmt::Display for TextRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRange => "text range is reversed or outside its source",
            Self::NotBoundary => "text range splits a UTF-8 scalar",
            Self::CrossesParagraph => "text line crosses a hard paragraph separator",
        })
    }
}
impl std::error::Error for TextRangeError {}

#[derive(Debug)]
struct ParagraphData {
    text: Arc<str>,
    fingerprint: u64,
    indices: OnceLock<Vec<(usize, usize)>>,
}

/// Immutable paragraph text shared by its visual lines and prepared adapters.
/// Equality compares the complete text; its cached hash is only a lookup hint.
#[derive(Clone, Debug)]
pub struct Paragraph(Arc<ParagraphData>);
impl Paragraph {
    fn new(text: Arc<str>, fingerprint: u64) -> Self {
        Self(Arc::new(ParagraphData {
            text,
            fingerprint,
            indices: OnceLock::new(),
        }))
    }

    /// The complete original paragraph, without the layout's terminating LF.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.0.text
    }

    /// The complete paragraph as a validated line.
    #[must_use]
    pub fn full_line(&self) -> ParagraphLine {
        ParagraphLine {
            paragraph: self.clone(),
            bytes: 0..self.text().len(),
        }
    }

    /// Validate a UTF-8 line range relative to this paragraph.
    ///
    /// # Errors
    /// Rejects reversed, out-of-bounds and mid-scalar ranges.
    pub fn line(&self, bytes: Range<usize>) -> Result<ParagraphLine, TextRangeError> {
        validate(self.text(), &bytes)?;
        Ok(ParagraphLine {
            paragraph: self.clone(),
            bytes,
        })
    }

    fn indices(&self) -> &[(usize, usize)] {
        self.0.indices.get_or_init(|| {
            let mut result = Vec::with_capacity(self.text().len() + 1);
            let mut unit = 0;
            for (byte, character) in self.text().char_indices() {
                result.push((byte, unit));
                unit += character.len_utf16();
            }
            result.push((self.text().len(), unit));
            result
        })
    }
}
impl PartialEq for Paragraph {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || Arc::ptr_eq(&self.0.text, &other.0.text)
            || (self.0.fingerprint == other.0.fingerprint && self.text() == other.text())
    }
}
impl Eq for Paragraph {}
impl Hash for Paragraph {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.fingerprint.hash(state);
    }
}

/// A scalar-aligned line range bound to its complete immutable paragraph.
///
/// Byte and UTF-16 offsets are relative to that paragraph, never a document
/// prefix. Native geometry returned for it uses offsets relative to `text()`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ParagraphLine {
    paragraph: Paragraph,
    bytes: Range<usize>,
}
impl ParagraphLine {
    /// The complete paragraph that supplies directional and shaping context.
    #[must_use]
    pub const fn paragraph(&self) -> &Paragraph {
        &self.paragraph
    }
    /// The visible logical text, unchanged and in source order.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.paragraph.text()[self.bytes.clone()]
    }
    /// The validated UTF-8 range within the paragraph.
    #[must_use]
    pub fn bytes(&self) -> Range<usize> {
        self.bytes.clone()
    }
    /// The corresponding UTF-16 range, including astral scalar widths.
    /// A shared boundary map is initialized once; each query is logarithmic.
    #[must_use]
    pub fn utf16(&self) -> Range<usize> {
        let indices = self.paragraph.indices();
        // Constructors guarantee scalar boundaries within this paragraph;
        // the table contains every such boundary, including its final byte.
        let start = indices.partition_point(|&(byte, _)| byte < self.bytes.start);
        let end = indices.partition_point(|&(byte, _)| byte < self.bytes.end);
        indices[start].1..indices[end].1
    }
}

#[derive(Debug)]
struct ContentData {
    source: Arc<str>,
    starts: Vec<usize>,
    paragraphs: Mutex<HashMap<usize, Paragraph>>,
    interned: Arc<Mutex<InternPool>>,
}

/// Original document text with an immutable index of shared paragraphs.
///
/// Clone this value across draw commands; each line resolves its context by
/// indexed lookup without copying the document or scanning its prefix.
#[derive(Clone, Debug)]
pub struct TextContent(Arc<ContentData>);
impl TextContent {
    fn build(source: Arc<str>, interned: Arc<Mutex<InternPool>>) -> Self {
        // Exact capacity avoids a power-of-two metadata allocation becoming
        // larger than a long note. Paragraph text is materialized only on use.
        let count = source.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let mut starts = Vec::with_capacity(count);
        starts.push(0);
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(at, byte)| (byte == b'\n').then_some(at + 1)),
        );
        Self(Arc::new(ContentData {
            source,
            starts,
            paragraphs: Mutex::default(),
            interned,
        }))
    }

    /// The application's original shared UTF-8 text.
    #[must_use]
    pub fn source(&self) -> &Arc<str> {
        &self.0.source
    }

    /// Bind a document byte range to one of its indexed paragraphs.
    /// Empty ranges at a paragraph end or after a final LF remain valid.
    ///
    /// # Errors
    /// Rejects invalid UTF-8 ranges and ranges crossing a hard LF separator.
    pub fn line(&self, range: Range<usize>) -> Result<ParagraphLine, TextRangeError> {
        validate(self, &range)?;
        // The table always includes a paragraph at zero, even for empty text.
        let at = self.0.starts.partition_point(|start| *start <= range.start) - 1;
        let start = self.0.starts[at];
        let end = self
            .0
            .starts
            .get(at + 1)
            .map_or(self.len(), |next| next - 1);
        if range.end > end {
            return Err(TextRangeError::CrossesParagraph);
        }
        let bytes = range.start - start..range.end - start;
        if let Some(paragraph) = self
            .0
            .paragraphs
            .lock()
            .ok()
            .and_then(|entries| entries.get(&at).cloned())
        {
            return paragraph.line(bytes);
        }
        let text = &self[start..end];
        // A poisoned optimization cache is bypassed, never trusted; rebuilding
        // immutable values preserves semantics without hiding a native error.
        let paragraph = self.0.interned.lock().map_or_else(
            |_| Paragraph::new(Arc::from(text), fingerprint(text)),
            |mut pool| pool.intern(text),
        );
        if let Ok(mut entries) = self.0.paragraphs.lock() {
            return entries.entry(at).or_insert(paragraph).line(bytes);
        }
        paragraph.line(bytes)
    }
}
impl Deref for TextContent {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0.source
    }
}
impl AsRef<str> for TextContent {
    fn as_ref(&self) -> &str {
        self
    }
}
impl From<Arc<str>> for TextContent {
    fn from(source: Arc<str>) -> Self {
        Self::build(source, Arc::default())
    }
}
impl From<&str> for TextContent {
    fn from(source: &str) -> Self {
        Self::from(Arc::<str>::from(source))
    }
}
impl From<String> for TextContent {
    fn from(source: String) -> Self {
        Self::from(Arc::<str>::from(source))
    }
}
impl From<Paragraph> for TextContent {
    fn from(paragraph: Paragraph) -> Self {
        let source = paragraph.0.text.clone();
        let mut paragraphs = HashMap::default();
        paragraphs.insert(0, paragraph);
        Self(Arc::new(ContentData {
            source,
            starts: vec![0],
            paragraphs: Mutex::new(paragraphs),
            interned: Arc::default(),
        }))
    }
}

const fn validate(text: &str, range: &Range<usize>) -> Result<(), TextRangeError> {
    if range.start > range.end || range.end > text.len() {
        Err(TextRangeError::InvalidRange)
    } else if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
        Err(TextRangeError::NotBoundary)
    } else {
        Ok(())
    }
}
fn fingerprint(text: &str) -> u64 {
    let mut state = FxHasher::default();
    text.hash(&mut state);
    state.finish()
}

const SOURCE_LIMIT: usize = 8;
const INTERN_LIMIT: usize = 4096;

/// Bounded weak reuse cannot pin an obsolete document after frame collection.
#[derive(Default)]
pub(crate) struct ContentCache {
    sources: VecDeque<Weak<ContentData>>,
    interned: Arc<Mutex<InternPool>>,
}
impl ContentCache {
    pub(crate) fn paragraph(&self, text: &str) -> Result<Paragraph, TextRangeError> {
        if text.contains('\n') {
            return Err(TextRangeError::CrossesParagraph);
        }
        Ok(self.interned.lock().map_or_else(
            |_| Paragraph::new(Arc::from(text), fingerprint(text)),
            |mut pool| pool.intern(text),
        ))
    }

    pub(crate) fn get(&mut self, source: &Arc<str>) -> TextContent {
        self.take(source.clone())
    }
    pub(crate) fn take(&mut self, source: Arc<str>) -> TextContent {
        if let Some((at, data)) = self.sources.iter().enumerate().find_map(|(at, weak)| {
            weak.upgrade()
                .filter(|data| Arc::ptr_eq(&data.source, &source))
                .map(|data| (at, data))
        }) {
            self.sources.remove(at);
            self.sources.push_back(Arc::downgrade(&data));
            return TextContent(data);
        }
        self.sources.retain(|entry| entry.strong_count() > 0);
        if let Ok(mut pool) = self.interned.lock() {
            pool.prune();
        }
        let text = TextContent::build(source, self.interned.clone());
        while self.sources.len() >= SOURCE_LIMIT {
            self.sources.pop_front();
        }
        self.sources.push_back(Arc::downgrade(&text.0));
        text
    }
}

#[derive(Debug, Default)]
struct InternPool {
    entries: HashMap<u64, Vec<Weak<ParagraphData>>>,
    count: usize,
}
impl InternPool {
    fn prune(&mut self) {
        self.entries.retain(|_, entries| {
            entries.retain(|entry| entry.strong_count() > 0);
            !entries.is_empty()
        });
        self.count = self.entries.values().map(Vec::len).sum();
    }
    fn intern(&mut self, text: &str) -> Paragraph {
        let hash = fingerprint(text);
        if let Some(found) = self.entries.get(&hash).and_then(|entries| {
            entries
                .iter()
                .filter_map(Weak::upgrade)
                .find(|entry| entry.text.as_ref() == text)
        }) {
            return Paragraph(found);
        }
        let paragraph = Paragraph::new(Arc::from(text), hash);
        if self.count < INTERN_LIMIT {
            self.entries
                .entry(hash)
                .or_default()
                .push(Arc::downgrade(&paragraph.0));
            self.count += 1;
        }
        paragraph
    }
}

#[cfg(test)]
#[path = "text_source_tests.rs"]
mod tests;
