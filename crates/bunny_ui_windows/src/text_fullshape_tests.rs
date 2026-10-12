//! Test-only whole-source shaping; never a production fallback or native ABI.
//! The independent native references and their long-text counterexamples remain
//! mandatory. A successful shape call is not proof of complete font behavior.

use super::super::super::super::{Cluster, joined_beh};
use super::super::{BaseFont, DirectWriteEngine, Family, FontSpec, Prepared, Shaper, reference};
use super::{com_ok, ffi};
use harfrust::{Buffer, BufferFlags, Direction, ShapeOptions, ShaperFont};
use std::ops::Range;

#[path = "text_fullshape_font_tests.rs"]
mod font;

#[repr(C)]
#[derive(Default)]
struct ScriptProperties {
    code: u32,
    number: u32,
    lookahead: u32,
    justification: u32,
    flags: u32,
}
#[repr(C)]
struct ScriptApi {
    _slots_0_13: [usize; 14],
    properties: unsafe extern "system" fn(
        *mut ffi::Analyzer,
        super::super::super::super::Script,
        *mut ScriptProperties,
    ) -> i32,
}

fn buffer(prepared: &Prepared, shaper: &Shaper) -> Buffer {
    let script = prepared.scripts[0];
    let level = prepared.levels[0].resolved;
    assert!(prepared.scripts.iter().all(|value| *value == script));
    assert!(prepared.levels.iter().all(|value| value.resolved == level));
    let mut properties = ScriptProperties::default();
    // SAFETY: GetScriptProperties is IDWriteTextAnalyzer1 slot 14; its argument
    // is the original paragraph's Script, and the output has five UINT32 fields.
    unsafe {
        let api = &**shaper.analyzer.as_ptr().cast::<*const ScriptApi>();
        assert!(com_ok((api.properties)(
            shaper.analyzer.as_ptr(),
            script,
            &raw mut properties
        )));
    }
    let mut result = Buffer::new();
    let text = String::from_utf16(prepared.utf16()).unwrap();
    let mut offset = 0;
    for character in text.chars() {
        result.push(u32::from(character), u32::try_from(offset).unwrap());
        offset += character.len_utf16();
    }
    assert_eq!(offset, prepared.utf16().len());
    assert_eq!(result.len(), text.chars().count());
    assert!(result.allocation_successful());
    let tag = harfrust::Tag::new(&properties.code.to_le_bytes());
    result.set_script(harfrust::Script::from_iso15924_tag(tag));
    assert!(result.script().is_some(), "native ISO script {tag:?}");
    result.set_direction(if level.is_multiple_of(2) {
        Direction::LeftToRight
    } else {
        Direction::RightToLeft
    });
    // SAFETY: Prepared owns the complete source and zero-terminated locale.
    let locale = unsafe { &(*prepared.source.as_ptr()).locale };
    let end = locale
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(locale.len());
    let locale = String::from_utf16(&locale[..end]).unwrap();
    result.set_language(Some(locale.parse().unwrap()));
    result.set_flags(BufferFlags::BEGINNING_OF_TEXT | BufferFlags::END_OF_TEXT);
    result
}

fn glyph_clusters(
    buffer: &Buffer,
    units: usize,
    em_size: f32,
    upem: u16,
    level: u8,
) -> Vec<Cluster> {
    assert!(
        buffer.allocation_successful(),
        "shaper exhausted allocation/work budget"
    );
    let infos = buffer.glyph_infos();
    let positions = buffer.glyph_positions();
    assert_eq!(infos.len(), positions.len());
    assert!(!infos.is_empty());
    let mut boundaries: Vec<_> = infos
        .iter()
        .map(|info| usize::try_from(info.cluster).unwrap())
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    assert_eq!(boundaries[0], 0);
    assert!(*boundaries.last().unwrap() < units);
    boundaries.push(units);
    let mut index = 0;
    let mut result: Vec<_> = infos
        .chunk_by(|a, b| a.cluster == b.cluster)
        .map(|group| {
            let start = usize::try_from(group[0].cluster).unwrap();
            let boundary = boundaries.binary_search(&start).unwrap();
            let mut glyphs = Vec::new();
            let mut advances = Vec::new();
            let mut offsets = Vec::new();
            for item in group {
                let position = positions[index];
                assert_eq!(position.y_advance, 0);
                glyphs.push(u16::try_from(item.glyph_id).unwrap());
                // Font-unit shaping preserves fractional native em sizes. Values
                // are converted explicitly; no HarfRust/native structures alias.
                let scale = |value: i32| f64::from(value) * f64::from(em_size) / f64::from(upem);
                advances.push(super::super::native_float(scale(position.x_advance)).unwrap());
                offsets.push(ffi::Offset {
                    advance: super::super::native_float(
                        scale(position.x_offset) * if level.is_multiple_of(2) { 1.0 } else { -1.0 },
                    )
                    .unwrap(),
                    ascender: super::super::native_float(scale(position.y_offset)).unwrap(),
                });
                index += 1;
            }
            if !level.is_multiple_of(2) {
                glyphs.reverse();
                advances.reverse();
                offsets.reverse();
            }
            Cluster {
                source: start..boundaries[boundary + 1],
                em_size,
                level: u32::from(level),
                glyphs,
                advances,
                offsets,
            }
        })
        .collect();
    result.sort_by_key(|cluster| cluster.source.start);
    assert_eq!(
        result.len() + 1,
        boundaries.len(),
        "non-monotone output clusters"
    );
    assert_eq!(result.last().unwrap().source.end, units);
    result
}

fn whole_source(engine: &DirectWriteEngine, text: &str, spec: &FontSpec) -> Vec<Cluster> {
    assert_eq!(
        spec.tracking, 0.0,
        "tracking compatibility needs a separate native proof"
    );
    let factories = engine.factories.as_ref().unwrap();
    let prepared = Prepared::new(factories, text).unwrap();
    let shaper = Shaper::new(factories).unwrap();
    let mut buffer = buffer(&prepared, &shaper);
    engine.with_slot(spec, |slot| {
        let base = BaseFont::new(slot, spec).unwrap();
        let (face, scale, mapped) = shaper.map(&prepared, 0..prepared.utf16().len(), &base, spec).unwrap();
        assert_eq!(mapped, prepared.utf16().len(), "fixture must resolve to one face");
        let font = font::exact_face(&face);
        let shaping = ShaperFont::new(&font);
        let started = std::time::Instant::now();
        harfrust::shape(&shaping, &mut buffer, ShapeOptions::default()).unwrap();
        eprintln!("full-source shaping observation: units={mapped}, glyphs={}, successful={}, elapsed_us={}; debug probe, not a benchmark", buffer.len(), buffer.allocation_successful(), started.elapsed().as_micros());
        glyph_clusters(&buffer, mapped, super::super::native_float(spec.size).unwrap() * scale, font.units_per_em(), prepared.levels[0].resolved)
    }).unwrap()
}

// Compare source boundaries and IDs exactly. Float conversion is accepted only
// below 1/1000 pixel per value; total advance error is measured independently.
fn compare(actual: &[Cluster], expected: &[Cluster]) -> Result<(), String> {
    if actual.len() != expected.len() {
        return Err(format!(
            "cluster count {} != {}",
            actual.len(),
            expected.len()
        ));
    }
    let mut drift = 0.0_f64;
    for (index, (a, b)) in actual.iter().zip(expected).enumerate() {
        if a.source != b.source
            || a.glyphs != b.glyphs
            || a.level != b.level
            || a.em_size.to_bits() != b.em_size.to_bits()
        {
            return Err(format!("cluster {index}: {a:?} != {b:?}"));
        }
        for (x, y) in a.advances.iter().zip(&b.advances) {
            drift += f64::from(*x) - f64::from(*y);
        }
        let metrics_a = a
            .advances
            .iter()
            .copied()
            .chain(a.offsets.iter().flat_map(|v| [v.advance, v.ascender]));
        let metrics_b = b
            .advances
            .iter()
            .copied()
            .chain(b.offsets.iter().flat_map(|v| [v.advance, v.ascender]));
        if metrics_a
            .zip(metrics_b)
            .any(|(x, y)| !x.is_finite() || !y.is_finite() || (x - y).abs() > 0.001)
        {
            return Err(format!("metrics cluster {index}: {a:?} != {b:?}"));
        }
    }
    eprintln!(
        "full-source metric drift: clusters={}, total_advance_delta={drift}",
        actual.len()
    );
    if drift.abs() > 0.1 {
        return Err(format!("accumulated advance drift {drift}"));
    }
    Ok(())
}

fn assert_parity(engine: &DirectWriteEngine, text: &str, font: &FontSpec, expected: &[Cluster]) {
    let actual = whole_source(engine, text, font);
    let difference = compare(&actual, expected);
    assert!(difference.is_ok(), "whole-source parity: {difference:?}");
    eprintln!(
        "full-source parity passed: family={:?}, units={}",
        font.family,
        text.encode_utf16().count()
    );
}

#[test]
fn paragraph_full_source_short_native_parity() {
    let engine = DirectWriteEngine::new();
    let mut failures = Vec::new();
    for family in ["Segoe UI", "Arial", "Times New Roman", "Calibri"] {
        let font = FontSpec {
            family: Family::named(family),
            size: 19.0,
            ..FontSpec::DEFAULT
        };
        for text in [
            "office affine ffi fi fl",
            "a\u{301} e\u{301}x",
            "بببببببب",
            "بَتِثَ",
            "لالالا",
        ] {
            let actual = whole_source(&engine, text, &font);
            let expected = reference(&engine, text, &font);
            match compare(&actual, &expected) {
                Ok(()) => {
                    eprintln!("full-source short parity passed: family={family}, text={text:?}");
                }
                Err(error) => {
                    eprintln!(
                        "full-source short mismatch: family={family}, text={text:?}: {error}"
                    );
                    failures.push(format!("{family}/{text:?}: {error}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "short parity failures: {failures:#?}");
}

#[test]
fn paragraph_full_source_preserves_long_connected_beh() {
    let engine = DirectWriteEngine::new();
    let text = "ب".repeat(70_000);
    for family in ["Segoe UI", "Arial", "Times New Roman"] {
        let font = FontSpec {
            family: Family::named(family),
            size: 19.0,
            ..FontSpec::DEFAULT
        };
        let expected = joined_beh(&engine, &font, 70_000);
        assert_parity(&engine, &text, &font, &expected);
    }
}

#[test]
fn paragraph_full_source_preserves_contextual_cascade() {
    let engine = DirectWriteEngine::new();
    assert!(engine.register_font(include_bytes!("../tests/fixtures/paragraph-cascade.ttf")));
    let font = FontSpec {
        family: Family::named("Bunny Paragraph Cascade"),
        size: 19.0,
        ..FontSpec::DEFAULT
    };
    let sample = reference(&engine, &format!("a{}", "b".repeat(8191)), &font);
    assert!(sample.iter().all(|cluster| cluster.glyphs == [1]));
    let expected: Vec<_> = (0..70_000)
        .map(|index| {
            let mut cluster = sample[0].clone();
            cluster.source = Range {
                start: index,
                end: index + 1,
            };
            cluster
        })
        .collect();
    assert_parity(
        &engine,
        &format!("a{}", "b".repeat(69_999)),
        &font,
        &expected,
    );
}

#[test]
fn paragraph_full_source_preserves_distant_font_context() {
    let engine = DirectWriteEngine::new();
    assert!(engine.register_font(include_bytes!("../tests/fixtures/paragraph-context.ttf")));
    let font = FontSpec {
        family: Family::named("Bunny Paragraph Context"),
        size: 19.0,
        ..FontSpec::DEFAULT
    };
    let text = format!("a{}c", "b".repeat(128));
    let expected = reference(&engine, &text, &font);
    assert_eq!(expected[0].glyphs, [4]);
    assert_parity(&engine, &text, &font, &expected);
}

#[test]
fn paragraph_full_source_preserves_long_input_context() {
    let engine = DirectWriteEngine::new();
    assert!(engine.register_font(include_bytes!(
        "../tests/fixtures/paragraph-input-context.ttf"
    )));
    let font = FontSpec {
        family: Family::named("Bunny Paragraph Input Context"),
        size: 19.0,
        ..FontSpec::DEFAULT
    };
    let text = format!("a{}c", "b".repeat(128));
    let expected = reference(&engine, &text, &font);
    assert_eq!(expected[0].glyphs, [4], "native matched-input witness");
    assert_parity(&engine, &text, &font, &expected);
}
