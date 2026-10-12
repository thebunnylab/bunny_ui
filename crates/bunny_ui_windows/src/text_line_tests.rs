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
    let mut discrepancies = Vec::new();
    for text in [
        "\u{202e}abc def ghi jkl mno\u{202c}",
        "\u{202d}אבג דהו זחט יכל מנס\u{202c}",
        "abc \u{2067}אבג (123) דהו\u{2069} xyz",
        "abc العربية 123 אבג def",
        "אבג\u{2003}\u{2002}דהו\u{2003}\u{2002}",
        "\u{202e}ab 😀 cd ef gh\u{202c}  ",
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
                    if u32::from(level) != metrics.bidi_level {
                        discrepancies.push(format!("{text:?}, width {width}, range {range:?}, scalar {character:?}, index {index}: level {level} vs {}, left {}", metrics.bidi_level, metrics.left));
                    }
                    logical.push((index, level, metrics.left));
                }
                let mut expected = logical.clone();
                expected.sort_by(|left, right| left.2.total_cmp(&right.2));
                bidi::reorder(&mut logical, |&(_, level, _)| level);
                let actual: Vec<_> = logical.iter().map(|&(index, _, _)| index).collect();
                let expected: Vec<_> = expected.iter().map(|&(index, _, _)| index).collect();
                if actual != expected {
                    discrepancies.push(format!(
                        "{text:?}, width {width}, range {range:?}: order {actual:?} vs {expected:?}"
                    ));
                }
            }
        }
        assert_eq!(prepared.levels, raw);
    }
    assert!(
        discrepancies.is_empty(),
        "native discrepancies: {discrepancies:#?}"
    );
}

#[test]
fn paragraph_line_boundaries_refuse_split_surrogates_and_keep_original_levels() {
    let engine = DirectWriteEngine::new();
    let prepared =
        Prepared::new(engine.factories.as_ref().unwrap(), "\u{202e}a😀 b \u{202c}").unwrap();
    let raw = prepared.levels.clone();
    for range in [0..3, 3..4, 2..3, 0..99, Range { start: 5, end: 4 }] {
        assert!(prepared.line_levels(range).is_err());
    }
    let whole = prepared.line_levels(0..prepared.utf16().len()).unwrap();
    assert_eq!(whole[2], whole[3]);
    assert_eq!(&whole[5..], &[1, 0, 0]);
    assert_eq!(prepared.levels, raw);
}

#[test]
fn paragraph_line_visual_clusters_match_native_positions_without_reshaping_joiners() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let font = FontSpec {
        size: 21.0,
        ..FontSpec::DEFAULT
    };
    let shaper = glyphs::Shaper::new(factories).unwrap();
    for text in [
        "\u{202e}abc def ghi jkl mno\u{202c}",
        "\u{202d}אבג דהו זחט יכל מנס\u{202c}",
        "abc \u{2067}אבג (123) דהו\u{2069} xyz",
        "abc العربية 123 אבג def",
        "\u{202e}ab 😀 cd ef gh\u{202c}  ",
        "ب\u{200d} ب\u{200c} ب\u{200d}",
        "אבג\u{2003}\u{2002}דהו\u{2003}\u{2002}",
    ] {
        let prepared = Prepared::new(factories, text).unwrap();
        for width in [65.0, 130.0, 400.0] {
            let (layout, ranges) = wrapped_layout(&engine, text, width);
            engine.with_slot(&font, |slot| {
                let base = glyphs::BaseFont::new(slot, &font).unwrap();
                for range in ranges {
                    let runs = shaper.shape(&prepared, range.clone(), &base, &font).unwrap();
                    let original: Vec<_> = runs.iter().map(|run| (run.glyphs.clone(), run.advances.clone())).collect();
                    let line = VisualLine::new(&prepared, range.clone(), runs).unwrap();
                    assert_eq!(line.runs.iter().map(|run| (run.glyphs.clone(), run.advances.clone())).collect::<Vec<_>>(), original);
                    for cluster in &line.clusters {
                        let Some(position) = cluster.source.clone().find(|&index| prepared.classes[index] == bidi::Class::Other) else { continue; };
                        let metrics = hit(&layout, position);
                        if metrics.width == 0.0 { continue; }
                        if (cluster.left - f64::from(metrics.left)).abs() >= 0.03 {
                            eprintln!("line levels {:?}; clusters {:?}", prepared.line_levels(range.clone()), line.clusters);
                            for run in &line.runs { eprintln!("source {:?} level {} glyphs {:?} advances {:?}", run.source, run.level, run.glyphs, run.advances); }
                        }
                        assert!((cluster.left - f64::from(metrics.left)).abs() < 0.03,
                            "{text:?}, width {width}, line {range:?}, cluster {:?}: left {} vs {} (glyph map {:?})",
                            cluster.source, cluster.left, metrics.left, line.runs[cluster.run].clusters);
                        assert!((cluster.width - f64::from(metrics.width)).abs() < 0.03,
                            "{text:?}, line {range:?}, cluster {:?}: width {} vs {}", cluster.source, cluster.width, metrics.width);
                    }
                    assert!((line.width - line.runs.iter().map(glyphs::Run::width).sum::<f64>()).abs() < 0.00001);
                }
            }).unwrap();
        }
    }
}

fn reference_pixels(
    surface: &glyphs::raster::Surface,
    layout: &Com<IDWriteTextLayout>,
    color: crate::text::Color,
) -> Vec<u8> {
    use crate::text::{D2D1_DRAW_TEXT_COLOR_FONT, D2dColor, D2dPoint};
    let ink = D2dColor {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    };
    surface
        .paint(|target| {
            let mut pointer = std::ptr::null_mut();
            // SAFETY: live surface and exact brush ABI with initialized output.
            unsafe {
                assert!(com_ok(((*(*target.as_ptr()).vtbl)
                    .create_solid_color_brush)(
                    target.as_ptr(),
                    &raw const ink,
                    std::ptr::null(),
                    &raw mut pointer
                )));
            }
            let brush = Com::from_raw(pointer).unwrap();
            // SAFETY: reference TextLayout owns the complete original text
            // and independently performs native analysis/wrapping/shaping.
            unsafe {
                ((*(*target.as_ptr()).vtbl).draw_text_layout)(
                    target.as_ptr(),
                    D2dPoint { x: 8.0, y: 8.0 },
                    layout.as_ptr(),
                    brush.as_ptr(),
                    D2D1_DRAW_TEXT_COLOR_FONT,
                );
            }
            Ok(())
        })
        .unwrap()
}

#[test]
fn paragraph_line_visual_spans_match_independent_wrapped_native_pixels() {
    use crate::text::{Color, D2dPoint};
    use glyphs::raster::{Renderer, Surface};
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let font = FontSpec {
        size: 21.0,
        ..FontSpec::DEFAULT
    };
    let shaper = glyphs::Shaper::new(factories).unwrap();
    for text in [
        "\u{202e}abc def ghi jkl mno\u{202c}",
        "\u{202d}אבג דהו זחט יכל מנס\u{202c}",
        "abc \u{2067}אבג (123) דהו\u{2069} xyz",
        "abc العربية 123 אבג def",
        "ב\u{200d} ب\u{200c} ب\u{200d}",
        "\u{202e}ab 😀 cd ef gh\u{202c}  ",
    ] {
        let prepared = Prepared::new(factories, text).unwrap();
        for width in [65.0, 130.0] {
            let (layout, ranges) = wrapped_layout(&engine, text, width);
            for scale in [1.0_f32, 2.0] {
                let surface = Surface::new(factories, 640, 640, scale).unwrap();
                let expected = reference_pixels(&surface, &layout, Color::BLACK);
                let actual = engine
                    .with_slot(&font, |slot| {
                        let base = glyphs::BaseFont::new(slot, &font).unwrap();
                        let mut renderer =
                            Renderer::new(&surface, factories, Color::BLACK).unwrap();
                        surface
                            .paint(|_| {
                                let mut top = 8.0;
                                for range in &ranges {
                                    let runs =
                                        shaper.shape(&prepared, range.clone(), &base, &font)?;
                                    let line = VisualLine::new(&prepared, range.clone(), runs)?;
                                    let baseline = ((top + glyphs::native_float(slot.ascent)?)
                                        * scale)
                                        .round()
                                        / scale;
                                    for span in line.spans() {
                                        let run = &line.runs[span.run];
                                        let left = glyphs::native_float(span.left)?;
                                        let advance = glyphs::native_float(span.width)?;
                                        let x = 8.0
                                            + left
                                            + if run.level.is_multiple_of(2) {
                                                0.0
                                            } else {
                                                advance
                                            };
                                        renderer.draw_range(
                                            run,
                                            span.glyphs,
                                            D2dPoint { x, y: baseline },
                                        )?;
                                    }
                                    top += glyphs::native_float(slot.ascent + slot.descent)?;
                                }
                                Ok(())
                            })
                            .unwrap()
                    })
                    .unwrap();
                let differences = expected
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(actual.as_chunks::<4>().0.iter())
                    .filter(|(a, b)| a != b)
                    .count();
                if differences != 0 {
                    pixel_diagnostics(&expected, &actual);
                    for range in &ranges {
                        let metric = hit(&layout, range.start);
                        eprintln!(
                            "native range {range:?}, top {}, height {}",
                            metric.top, metric.height
                        );
                    }
                }
                assert_eq!(
                    differences, 0,
                    "{text:?}, width {width}, scale {scale}: pixel differences"
                );
            }
        }
    }
}

fn pixel_diagnostics(expected: &[u8], actual: &[u8]) {
    for (label, pixels) in [("native", expected), ("candidate", actual)] {
        let mut rows = std::collections::BTreeMap::<usize, (usize, usize, usize)>::new();
        for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] > 0 {
                let row = rows.entry(index / 640).or_insert((640, 0, 0));
                row.0 = row.0.min(index % 640);
                row.1 = row.1.max(index % 640);
                row.2 += 1;
            }
        }
        eprintln!("{label} nonempty rows: {rows:?}");
    }
}

#[test]
fn paragraph_line_unicode_l1_records_native_trailing_format_control_divergence() {
    let engine = DirectWriteEngine::new();
    for text in [
        "\u{202e}ab  \u{202c}",
        "\u{202e}ab  \u{2069}\u{202c}",
        "\u{202e}ab  \u{2069}\u{202c}\ncd ef",
    ] {
        let prepared = Prepared::new(engine.factories.as_ref().unwrap(), text).unwrap();
        let (layout, ranges) = wrapped_layout(&engine, text, 130.0);
        let levels = prepared.line_levels(ranges[0].clone()).unwrap();
        assert_eq!(&levels[1..5], &[1, 1, 0, 0]);
        let mut visible = [
            (1, levels[1]),
            (2, levels[2]),
            (3, levels[3]),
            (4, levels[4]),
        ];
        bidi::reorder(&mut visible, |&(_, level)| level);
        assert_eq!(visible.map(|(index, _)| index), [2, 1, 3, 4]);
        let mut native: Vec<_> = (1..5).map(|index| (index, hit(&layout, index))).collect();
        native.sort_by(|a, b| a.1.left.total_cmp(&b.1.left));
        let order: Vec<_> = native.iter().map(|(index, _)| *index).collect();
        // Unicode 17 BidiTest RLO L WS PDF (source manifest) requires WS=0.
        // Server 2025 keeps WS=1 before a retained PDF, putting it before the
        // letters. Preserve this exact discrepancy rather than using that native
        // result as the normative oracle or claiming whole-corpus native parity.
        if order == [2, 1, 3, 4] {
            assert_eq!(hit(&layout, 3).bidi_level, 0);
        } else {
            assert_eq!(
                order,
                [4, 3, 2, 1],
                "unclassified native L1 divergence: {text:?}"
            );
            assert_eq!(hit(&layout, 3).bidi_level, 1);
            assert_eq!(hit(&layout, 4).bidi_level, 1);
            eprintln!(
                "Unicode17 L1/native divergence: {text:?}; normative [2,1,3,4], native {order:?}"
            );
        }
    }
}
