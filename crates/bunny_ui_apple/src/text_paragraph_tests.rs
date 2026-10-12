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
    runtime.display_frame(&view, size);
    let layout = runtime.layout(&view, Proposal::exact(size));
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
            let observed = runtime.ime_rect_for(unit).unwrap().origin.x;
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
