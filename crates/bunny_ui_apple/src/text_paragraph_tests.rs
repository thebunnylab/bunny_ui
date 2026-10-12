//! Whole-paragraph native lines are the oracle; isolated line slices are the work under test.
use super::*;
use bunny_ui::layout::{DrawCommand, Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::text_input::byte_to_utf16;
use std::ops::Range;
use std::rc::Rc;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTTypesetterCreateWithAttributedString(string: *const c_void) -> *const c_void;
    fn CTTypesetterCreateLine(typesetter: *const c_void, range: CFRange) -> CTLineRef;
    fn CTLineGetOffsetForStringIndex(line: CTLineRef, index: isize, secondary: *mut f64) -> f64;
}

struct OwnedCf(*const c_void);
impl OwnedCf {
    fn new(value: *const c_void) -> Self {
        assert!(!value.is_null(), "native test allocation failed");
        Self(value)
    }
}
impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: this test wrapper owns exactly one Create-rule reference.
        unsafe { CFRelease(self.0) };
    }
}

fn paragraph_line(
    engine: &CoreTextEngine,
    text: &str,
    range: Range<usize>,
    font: &FontSpec,
) -> OwnedCf {
    assert_eq!(
        font.tracking, 0.0,
        "the oracle fixture uses default tracking"
    );
    // SAFETY: all input objects are live and retained under the CF Create rule.
    // The string and checked range use the SDK's UTF-16 CFIndex vocabulary.
    unsafe {
        let string = OwnedCf::new(cf_string(text));
        let attributed = OwnedCf::new(CFAttributedStringCreateMutable(std::ptr::null(), 0));
        CFAttributedStringReplaceString(
            attributed.0.cast_mut(),
            CFRange {
                location: 0,
                length: 0,
            },
            string.0,
        );
        CFAttributedStringSetAttribute(
            attributed.0.cast_mut(),
            CFRange {
                location: 0,
                length: CFStringGetLength(string.0),
            },
            kCTFontAttributeName,
            engine.font(font),
        );
        let typesetter = OwnedCf::new(CTTypesetterCreateWithAttributedString(attributed.0));
        let location = isize::try_from(byte_to_utf16(text, range.start)).unwrap();
        let end = isize::try_from(byte_to_utf16(text, range.end)).unwrap();
        OwnedCf::new(CTTypesetterCreateLine(
            typesetter.0,
            CFRange {
                location,
                length: end - location,
            },
        ))
    }
}

/// Independent SDK oracle shared by mixed-line alignment checks. Native string
/// indices refer to the original paragraph even when the line starts later.
pub(super) fn paragraph_offset(
    engine: &CoreTextEngine,
    text: &str,
    range: Range<usize>,
    font: &FontSpec,
    utf16: usize,
) -> f64 {
    let line = paragraph_line(engine, text, range, font);
    // SAFETY: the retained line was made for this original paragraph range.
    unsafe {
        CTLineGetOffsetForStringIndex(
            line.0,
            isize::try_from(utf16).unwrap(),
            std::ptr::null_mut(),
        )
    }
}

fn wrapped_carets_match_paragraph(text: &str, width: f64) {
    let engine = Rc::new(CoreTextEngine::new());
    let runtime = Runtime::new().text_engine(engine.clone());
    let draft = State::new(text.to_owned());
    let view = text_editor("", draft.binding())
        .font_size(15.0)
        .frame(width, 400.0)
        .id("editor");
    let size = Size {
        width,
        height: 400.0,
    };
    runtime.display_frame(&view, size).unwrap();
    let layout = runtime.layout(&view, Proposal::exact(size)).unwrap();
    assert!(runtime.focus_named("editor"));
    let mut rows = 0;
    let mut checked = 0;
    let mut mismatches = Vec::new();
    for command in layout.display.iter() {
        let DrawCommand::TextLine {
            content,
            range,
            origin,
            font,
            ..
        } = command
        else {
            continue;
        };
        if content.as_ref() != text || range.0 == range.1 {
            continue;
        }
        rows += 1;
        let native = paragraph_line(&engine, text, range.0..range.1, font);
        let scalar_positions: Vec<_> = text[range.0..range.1].char_indices().collect();
        // Interiors avoid choosing one of two valid insertion affinities at a
        // directional boundary; the line itself is shaped from the full text.
        for triple in scalar_positions.windows(3) {
            let [(_, previous), (local, character), (_, following)] = triple else {
                unreachable!()
            };
            if !previous.is_alphabetic() || !character.is_alphabetic() || !following.is_alphabetic()
            {
                continue;
            }
            let unit = byte_to_utf16(text, range.0 + local);
            let index = isize::try_from(unit).unwrap();
            // SAFETY: a live native line and scalar-aligned UTF-16 index in its source range.
            let expected = origin.x
                + unsafe { CTLineGetOffsetForStringIndex(native.0, index, std::ptr::null_mut()) };
            let observed = runtime.ime_rect_for(unit).unwrap().unwrap().origin.x;
            checked += 1;
            if (observed - expected).abs() > 0.01 {
                mismatches.push(format!(
                    "row {range:?}, UTF16 {unit}, expected {expected}, observed {observed}"
                ));
            }
        }
    }
    assert!(
        rows >= 2 && checked >= 3,
        "the fixture must exercise wrapped native interiors"
    );
    assert!(
        mismatches.is_empty(),
        "paragraph context lost for {text:?}:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn paragraph_override_survives_soft_wrapped_latin() {
    wrapped_carets_match_paragraph("\u{202e}abc def ghi jkl mno\u{202c}", 70.0);
}

#[test]
fn paragraph_override_survives_soft_wrapped_hebrew() {
    wrapped_carets_match_paragraph("\u{202d}אבג דהו זחט יכל מנס\u{202c}", 70.0);
}

#[test]
fn paragraph_bracket_context_survives_soft_wraps() {
    wrapped_carets_match_paragraph("אבג (abc def ghi) דהו", 70.0);
}

#[test]
fn paragraph_plain_latin_position_control() {
    wrapped_carets_match_paragraph("abc def ghi jkl mno", 70.0);
}

#[test]
fn prepared_paragraph_ranges_keep_original_native_indices() {
    use bunny_ui::text_geometry::CaretAffinity;
    use bunny_ui::text_source::TextContent;
    let engine = CoreTextEngine::new();
    let font = FontSpec::DEFAULT;
    for text in [
        "😀 \u{202e}abc def ghi\u{202c}",
        "😀 \u{202d}אבג דהו זחט\u{202c}",
        "אבג (abc def ghi) דהו",
        "אבג \u{2066}abc def ghi\u{2069} דהו",
    ] {
        let source = TextContent::from(text);
        let start = text.find("def").or_else(|| text.find("דהו")).unwrap();
        let stop = start
            + text[start..]
                .chars()
                .take(3)
                .map(char::len_utf8)
                .sum::<usize>();
        let line = source.line(start..stop).unwrap();
        let reference = paragraph_line(&engine, text, start..stop, &font);
        let measured = engine.try_measure_paragraph_line(&line, &font).unwrap();
        // SAFETY: a retained oracle CTLine and optional omitted font metrics.
        let expected_width = unsafe {
            CTLineGetTypographicBounds(
                reference.0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert!((measured.width - expected_width).abs() < 0.001);
        let geometry = engine
            .try_paragraph_line_geometry(&line, &font)
            .unwrap()
            .unwrap();
        let middle = text[start..stop].chars().next().unwrap().len_utf8();
        let index = isize::try_from(byte_to_utf16(text, start + middle)).unwrap();
        // SAFETY: an original-source UTF-16 scalar boundary in the retained oracle line.
        let expected_x =
            unsafe { CTLineGetOffsetForStringIndex(reference.0, index, std::ptr::null_mut()) };
        assert!((geometry.caret_x(middle, CaretAffinity::Downstream) - expected_x).abs() < 0.001);
    }
}

#[test]
fn prepared_paragraph_empty_range_never_draws_the_remaining_paragraph() {
    use bunny_ui::text_source::TextContent;
    let engine = CoreTextEngine::new();
    let source = TextContent::from("\u{202e}abc def\u{202c}");
    let start = source.find("def").unwrap();
    let empty = source.line(start..start).unwrap();
    let metrics = engine
        .try_measure_paragraph_line(&empty, &FontSpec::DEFAULT)
        .unwrap();
    assert!(metrics.width.abs() < f64::EPSILON);
    assert!(metrics.height() > 0.0);
    assert!(
        engine
            .try_raster_paragraph_line(&empty, &FontSpec::DEFAULT, Color::BLACK, 2)
            .unwrap()
            .is_none()
    );
    let geometry = engine
        .try_paragraph_line_geometry(&empty, &FontSpec::DEFAULT)
        .unwrap()
        .unwrap();
    assert!(geometry.selection_spans(0..0).is_empty());
}

#[test]
fn prepared_paragraph_pixels_follow_carried_override_at_both_scales() {
    use bunny_ui::text_source::TextContent;
    let engine = CoreTextEngine::new();
    let font = FontSpec::DEFAULT;
    let source = TextContent::from("\u{202e}abc def ghi\u{202c}");
    let start = source.find("def").unwrap();
    let line = source.line(start..start + 3).unwrap();
    for scale in [1, 2] {
        let color = Color::rgb(71, 33, 190);
        let actual = engine
            .try_raster_paragraph_line(&line, &font, color, scale)
            .unwrap()
            .unwrap();
        let expected = engine
            .raster_line("\u{202e}def\u{202c}", &font, color, scale)
            .unwrap();
        assert_eq!(
            (actual.width, actual.height, actual.baseline),
            (expected.width, expected.height, expected.baseline)
        );
        assert_eq!(actual.rgba, expected.rgba);
        assert_ne!(
            actual.rgba,
            engine.raster_line("def", &font, color, scale).unwrap().rgba
        );
    }
}
