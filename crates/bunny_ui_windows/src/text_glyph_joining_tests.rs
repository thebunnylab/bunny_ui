//! Original-source overlap is an experiment, never a production cut policy.
//!
//! Selected periodic witnesses can verify joining; distant font dependencies
//! and native clusters crossing a body edge must remain explicit counterexamples.

use super::super::{
    BaseFont, Cluster, DirectWriteEngine, FontSpec, Input, MAX_GLYPHS, Prepared, Shaper, clusters,
    native_float, raster, tests,
};
use bunny_ui::text_engine::Family;

// These fixtures contain only GCB=Other bases and U+064E/U+0650 Extend.
// GB9 attaches every mark to its preceding base. Reject any other character:
// this test helper is not a general grapheme segmenter or a public API.
fn boundaries(text: &str) -> Vec<usize> {
    let mut edges = vec![0];
    let mut units = 0;
    for character in text.chars() {
        assert!(matches!(
            character,
            'a' | 'b'
                | 'c'
                | 'e'
                | 'f'
                | 'i'
                | 'o'
                | 'ب'
                | 'ت'
                | 'ث'
                | 'ل'
                | 'ا'
                | '\u{64e}'
                | '\u{650}'
        ));
        if units > 0 && !matches!(character, '\u{64e}' | '\u{650}') {
            edges.push(units);
        }
        units += character.len_utf16();
    }
    if units > 0 {
        edges.push(units);
    }
    edges
}

fn reference(engine: &DirectWriteEngine, text: &str, font: &FontSpec) -> Vec<Cluster> {
    let layout = tests::reference_layout(engine, text, font);
    let mut result: Vec<_> = raster::reference_records(&layout)
        .iter()
        .flat_map(clusters)
        .collect();
    result.sort_by_key(|cluster| cluster.source.start);
    assert_eq!(result.first().unwrap().source.start, 0);
    assert_eq!(
        result.last().unwrap().source.end,
        text.encode_utf16().count()
    );
    assert!(
        result
            .windows(2)
            .all(|pair| pair[0].source.end == pair[1].source.start)
    );
    result
}

fn mismatch(actual: &[Cluster], expected: &[Cluster]) -> Option<String> {
    let first = actual.iter().zip(expected).position(|(a, b)| a != b);
    if actual.len() == expected.len() && first.is_none() {
        return None;
    }
    let first = first.unwrap_or_else(|| actual.len().min(expected.len()));
    Some(format!(
        "{} vs {} clusters, first difference {first}: {:?} vs {:?}",
        actual.len(),
        expected.len(),
        actual.get(first),
        expected.get(first)
    ))
}

// This helper deliberately accepts only homogeneous witnesses. It never
// recomputes script, direction or fallback from the shortened window.
fn overlap(
    engine: &DirectWriteEngine,
    text: &str,
    font: &FontSpec,
    body_graphemes: usize,
    context_graphemes: usize,
) -> Result<Vec<Cluster>, String> {
    assert!(body_graphemes > 0);
    let factories = engine.factories.as_ref().unwrap();
    let prepared = Prepared::new(factories, text).unwrap();
    let edges = boundaries(text);
    let count = edges.len() - 1;
    let script = prepared.scripts[0];
    let level = prepared.levels[0].resolved;
    assert!(prepared.scripts.iter().all(|value| *value == script));
    assert!(prepared.levels.iter().all(|value| value.resolved == level));
    let shaper = Shaper::new(factories).unwrap();
    engine
        .with_slot(font, |slot| {
            let base = BaseFont::new(slot, font).unwrap();
            let (face, scale, mapped) = shaper
                .map(&prepared, 0..prepared.utf16().len(), &base, font)
                .unwrap();
            assert_eq!(mapped, prepared.utf16().len());
            let mut result = Vec::new();
            for begin in (0..count).step_by(body_graphemes) {
                let end = (begin + body_graphemes).min(count);
                let body = edges[begin]..edges[end];
                let window = edges[begin.saturating_sub(context_graphemes)]
                    ..edges[end.saturating_add(context_graphemes).min(count)];
                if window.len() > 16_000 {
                    return Err(format!(
                        "oversized original context: body={body:?}, window={window:?}"
                    ));
                }
                let input = Input {
                    text: &prepared.utf16()[window.clone()],
                    script,
                    level,
                    face: &face,
                    em_size: native_float(font.size).unwrap() * scale,
                    // SAFETY: the complete Prepared source retains this locale.
                    locale: unsafe { &(*prepared.source.as_ptr()).locale },
                };
                let output = shaper
                    .produce(&input, native_float(font.tracking).unwrap(), MAX_GLYPHS)
                    .map_err(|error| format!("native window {window:?}: {error:?}"))?;
                let run = raster::NativeRun {
                    source: window,
                    em_size: input.em_size,
                    level: u32::from(level),
                    clusters: output.clusters,
                    glyphs: output.glyphs,
                    advances: output.advances,
                    offsets: output.offsets,
                };
                for cluster in clusters(&run) {
                    if cluster.source.start >= body.end || cluster.source.end <= body.start {
                        continue;
                    }
                    if cluster.source.start < body.start || cluster.source.end > body.end {
                        return Err(format!(
                            "native cluster {:?} straddles body {body:?}",
                            cluster.source
                        ));
                    }
                    result.push(cluster);
                }
            }
            Ok(result)
        })
        .unwrap()
}

// A selected metamorphic oracle, not a theorem about arbitrary Arabic or fonts.
// Keep three motifs at each real word end and require the native interior to
// repeat. An independent 97-motif whole layout must validate extrapolation.
fn periodic_sample(engine: &DirectWriteEngine, motif: &str, font: &FontSpec) -> Vec<Vec<Cluster>> {
    let width = motif.encode_utf16().count();
    let sample = reference(engine, &motif.repeat(9), font);
    let mut parts = vec![Vec::new(); 9];
    for mut cluster in sample {
        let part = cluster.source.start / width;
        assert!(
            cluster.source.end <= (part + 1) * width,
            "motif edge splits native cluster"
        );
        cluster.source = cluster.source.start - part * width..cluster.source.end - part * width;
        parts[part].push(cluster);
    }
    assert_eq!(parts[3], parts[4]);
    assert_eq!(parts[4], parts[5]);
    let expected = extrapolate(&parts, width, 97);
    let actual = reference(engine, &motif.repeat(97), font);
    assert!(
        mismatch(&actual, &expected).is_none(),
        "periodic oracle: {:?}",
        mismatch(&actual, &expected)
    );
    parts
}

fn extrapolate(parts: &[Vec<Cluster>], width: usize, count: usize) -> Vec<Cluster> {
    assert!(count >= 9);
    (0..count)
        .flat_map(|part| {
            let index = if part < 3 {
                part
            } else if part >= count - 3 {
                9 - (count - part)
            } else {
                4
            };
            parts[index].iter().cloned().map(move |mut cluster| {
                cluster.source =
                    cluster.source.start + part * width..cluster.source.end + part * width;
                cluster
            })
        })
        .collect()
}

#[test]
fn paragraph_long_original_context_windows_match_selected_joining_oracles() {
    let engine = DirectWriteEngine::new();
    let tails = format!("ب{}", "\u{64e}".repeat(127));
    let mut failures = Vec::new();
    for (name, motif) in [
        ("beh", "ب"),
        ("mixed-joining", "بتث"),
        ("joining-marks", "بَتِ"),
        ("lam-alef", "بَلَا"),
        ("transparent-tails", tails.as_str()),
    ] {
        let width = motif.encode_utf16().count();
        let count = 70_000_usize.div_ceil(width);
        let text = motif.repeat(count);
        let body = (4096 / width).max(1) * (boundaries(motif).len() - 1);
        for family in ["Segoe UI", "Arial", "Times New Roman"] {
            for tracking in [-0.75, 0.0, 0.75] {
                let font = FontSpec {
                    size: 19.0,
                    family: Family::named(family),
                    tracking,
                    ..FontSpec::DEFAULT
                };
                let sample = periodic_sample(&engine, motif, &font);
                let expected = extrapolate(&sample, width, count);
                for context in [1, 8] {
                    let label = format!(
                        "{name}, font={family}, tracking={tracking}, context={context}, body={body}"
                    );
                    let error = match overlap(&engine, &text, &font, body, context) {
                        Ok(actual) => mismatch(&actual, &expected),
                        Err(error) => Some(error),
                    };
                    if let Some(error) = error {
                        eprintln!("original-context mismatch: {label}: {error}");
                        failures.push(format!("{label}: {error}"));
                    } else {
                        eprintln!(
                            "original-context selected parity: {label}, units={}",
                            width * count
                        );
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "selected context failures: {failures:#?}"
    );
}

#[test]
fn paragraph_original_context_windows_expose_distant_font_dependencies() {
    let engine = DirectWriteEngine::new();
    assert!(engine.register_font(include_bytes!("../tests/fixtures/paragraph-context.ttf")));
    let font = FontSpec {
        size: 19.0,
        family: Family::named("Bunny Paragraph Context"),
        ..FontSpec::DEFAULT
    };
    let text = format!("a{}c", "b".repeat(128));
    let expected = reference(&engine, &text, &font);
    assert_eq!(expected[0].glyphs, [4]);
    let mut insufficient = None;
    for context in [1, 8, 128] {
        let actual = overlap(&engine, &text, &font, 1, context).unwrap();
        assert!(
            mismatch(&actual, &expected).is_some(),
            "insufficient overlap must be detected"
        );
        assert_eq!(actual[0].glyphs, [1]);
        if let Some(previous) = &insufficient {
            assert_eq!(&actual, previous, "stable overlap can still be wrong");
        }
        insufficient = Some(actual);
        eprintln!(
            "original-context counterexample: context={context} loses the 130-character feature"
        );
    }
    let actual = overlap(&engine, &text, &font, 1, 129).unwrap();
    assert!(mismatch(&actual, &expected).is_none());
    eprintln!("original-context distant feature matches only with sufficient original context");
}

#[test]
fn paragraph_original_context_windows_refuse_split_clusters_and_oversized_graphemes() {
    let engine = DirectWriteEngine::new();
    let font = FontSpec {
        size: 19.0,
        family: Family::named("Calibri"),
        ..FontSpec::DEFAULT
    };
    let text = "office".repeat(10);
    let expected = reference(&engine, &text, &font);
    assert!(
        expected.iter().any(|cluster| cluster.source.len() > 1),
        "witness requires a ligature cluster"
    );
    let error = overlap(&engine, &text, &font, 1, 8).unwrap_err();
    assert!(error.contains("straddles body"), "{error}");
    eprintln!("original-context explicit refusal: {error}");
    let font = FontSpec {
        family: Family::named("Segoe UI"),
        ..font
    };
    let text = format!("ب{}", "\u{64e}".repeat(20_000));
    assert_eq!(boundaries(&text), [0, 20_001]);
    let error = overlap(&engine, &text, &font, 1, 8).unwrap_err();
    assert!(error.contains("oversized original context"), "{error}");
    eprintln!("original-context explicit refusal: {error}");
}

#[path = "text_glyph_dependency_tests.rs"]
mod dependencies;
