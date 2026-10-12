use super::*;
use crate::text::{Com, DirectWriteEngine, FontSpec, Hresult, IDWriteTextLayout, com_ok, geometry};
use std::ffi::c_void;

// dwrite.h at c41ef7458f008ba4ccea85ee33163be71b9112bb: exact
// IDWriteTextLayout inherited slots and DWRITE_LINE_METRICS (BOOL is INT32).
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Metrics {
    length: u32,
    trailing_whitespace: u32,
    newline: u32,
    height: f32,
    baseline: f32,
    trimmed: i32,
}
#[repr(C)]
struct LayoutVtbl {
    _pad_0_4: [usize; 5],
    wrapping: unsafe extern "system" fn(*mut c_void, u32) -> Hresult,
    _pad_6_27: [usize; 22],
    width: unsafe extern "system" fn(*mut c_void, f32) -> Hresult,
    _pad_29_58: [usize; 30],
    lines: unsafe extern "system" fn(*mut c_void, *mut Metrics, u32, *mut u32) -> Hresult,
}

fn wrapped_layout(
    engine: &DirectWriteEngine,
    text: &str,
    width: f32,
) -> (Com<IDWriteTextLayout>, Vec<Range<usize>>) {
    let font = FontSpec {
        size: 21.0,
        ..FontSpec::DEFAULT
    };
    let layout = glyphs::tests::reference_layout(engine, text, &font);
    let mut metrics = vec![Metrics::default(); text.encode_utf16().count() + 1];
    let mut count = 0;
    // SAFETY: exact verified SDK slots, live native layout and initialized
    // storage. The independent engine chooses every line break itself.
    unsafe {
        let pointer = layout.as_ptr().cast::<c_void>();
        let table = &**pointer.cast::<*const LayoutVtbl>();
        assert!(com_ok((table.wrapping)(pointer, 0)));
        assert!(com_ok((table.width)(pointer, width)));
        assert!(com_ok((table.lines)(
            pointer,
            metrics.as_mut_ptr(),
            u32::try_from(metrics.len()).unwrap(),
            &raw mut count
        )));
    }
    let mut offset = 0;
    let lines = metrics[..usize::try_from(count).unwrap()]
        .iter()
        .map(|line| {
            let start = offset;
            offset += usize::try_from(line.length).unwrap();
            start..offset
        })
        .collect();
    assert_eq!(offset, text.encode_utf16().count());
    (layout, lines)
}

fn hit(layout: &Com<IDWriteTextLayout>, position: usize) -> geometry::HitMetrics {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut metrics = geometry::HitMetrics::default();
    // SAFETY: source-relative code-unit position and the independent layout
    // remain live for this synchronous native hit test.
    unsafe {
        assert!(com_ok(((*(*layout.as_ptr()).vtbl).hit_test_text_position)(
            layout.as_ptr(),
            u32::try_from(position).unwrap(),
            0,
            &raw mut x,
            &raw mut y,
            &raw mut metrics
        )));
    }
    metrics
}

#[test]
fn paragraph_line_levels_and_order_match_independent_native_wraps() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    for text in [
        "\u{202e}abc def ghi jkl mno\u{202c}",
        "\u{202d}אבג דהו זחט יכל מנס\u{202c}",
        "abc \u{2067}אבג (123) דהו\u{2069} xyz",
        "abc العربية 123 אבג def",
        "אבג\u{2003}\u{2002}דהו\u{2003}\u{2002}",
        "\u{202e}ab 😀 cd ef gh\u{202c}  ",
        "\u{202e}ab  \u{2069}\u{202c}\ncd ef",
        "\u{202e}ab  \u{2029}cd ef",
    ] {
        let prepared = Prepared::new(factories, text).unwrap();
        let raw = prepared.levels.clone();
        for width in [65.0, 130.0, 400.0] {
            let (layout, ranges) = wrapped_layout(&engine, text, width);
            // Reverse query order must not mutate the cached paragraph's levels.
            for range in ranges.iter().rev() {
                let levels = prepared.line_levels(range.clone()).unwrap();
                let mut logical = Vec::new();
                let mut offset = range.start;
                for character in char::decode_utf16(prepared.utf16()[range.clone()].iter().copied())
                {
                    let character = character.unwrap();
                    let index = offset;
                    offset += character.len_utf16();
                    if matches!(
                        bidi::Class::of(character),
                        bidi::Class::Retained | bidi::Class::Separator
                    ) || matches!(character, '\u{2066}'..='\u{2069}')
                    {
                        continue;
                    }
                    let metrics = hit(&layout, index);
                    if usize::try_from(metrics.position).unwrap() != index || metrics.width <= 0.0 {
                        continue;
                    }
                    let level = levels[index - range.start];
                    assert_eq!(
                        u32::from(level),
                        metrics.bidi_level,
                        "{text:?}, width {width}, range {range:?}, scalar {character:?}, index {index}"
                    );
                    logical.push((index, level, metrics.left));
                }
                let mut expected = logical.clone();
                expected.sort_by(|left, right| left.2.total_cmp(&right.2));
                bidi::reorder(&mut logical, |&(_, level, _)| level);
                assert_eq!(
                    logical
                        .iter()
                        .map(|&(index, _, _)| index)
                        .collect::<Vec<_>>(),
                    expected
                        .iter()
                        .map(|&(index, _, _)| index)
                        .collect::<Vec<_>>(),
                    "{text:?}, width {width}, range {range:?}"
                );
            }
        }
        assert_eq!(prepared.levels, raw);
    }
}

#[test]
fn paragraph_line_boundaries_refuse_split_surrogates_and_keep_original_levels() {
    let engine = DirectWriteEngine::new();
    let prepared =
        Prepared::new(engine.factories.as_ref().unwrap(), "\u{202e}a😀 b \u{202c}").unwrap();
    let raw = prepared.levels.clone();
    for range in [0..3, 3..4, 2..3, 0..99, 5..4] {
        assert!(prepared.line_levels(range).is_err());
    }
    let whole = prepared.line_levels(0..prepared.utf16().len()).unwrap();
    assert_eq!(whole[2], whole[3]);
    assert_eq!(&whole[5..], &[1, 0, 0]);
    assert_eq!(prepared.levels, raw);
}
