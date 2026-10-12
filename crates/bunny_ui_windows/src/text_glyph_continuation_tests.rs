//! Falsify bounded continuation before selecting a production shaping policy.
//!
//! These witnesses use native safe-cut flags together with complete cluster
//! boundaries. They never authorize fixed overlap or a cut from failed output.

use super::*;

#[derive(Debug)]
struct NoCut {
    position: usize,
    probed: usize,
    safe_flags: usize,
    cluster_boundaries: usize,
}

fn continue_at_native_cuts(
    shaper: &Shaper,
    input: &Input<'_>,
    tracking: f32,
    bound: usize,
) -> Result<Vec<Run>, NoCut> {
    let mut position = 0;
    let mut runs = Vec::new();
    while position < input.text.len() {
        let mut end = (position + bound).min(input.text.len());
        while validate_range(input.text, &(position..end)).is_err() {
            end -= 1;
        }
        assert!(end > position, "probe must contain a complete scalar");
        let probe = Input {
            text: &input.text[position..end],
            ..*input
        };
        let output = shaper.glyph_indices(&probe, MAX_GLYPHS).unwrap();
        let length = if end == input.text.len() {
            probe.text.len()
        } else {
            // Exclude the artificial end of the probe and reject a native flag
            // inside one cluster (seen with arbitrarily many combining marks).
            (1..probe.text.len())
                .rev()
                .find(|&cut| {
                    output.text_properties[cut - 1] & 4 != 0
                        && output.clusters[cut - 1] != output.clusters[cut]
                        && validate_range(input.text, &(position..position + cut)).is_ok()
                })
                .ok_or(NoCut {
                    position,
                    probed: probe.text.len(),
                    safe_flags: output
                        .text_properties
                        .iter()
                        .filter(|&&flags| flags & 4 != 0)
                        .count(),
                    cluster_boundaries: output
                        .clusters
                        .windows(2)
                        .filter(|pair| pair[0] != pair[1])
                        .count(),
                })?
        };
        let chunk = Input {
            text: &input.text[position..position + length],
            ..*input
        };
        let output = shaper.produce(&chunk, tracking, MAX_GLYPHS).unwrap();
        runs.push(Run {
            source: position..position + length,
            face: input.face.clone(),
            em_size: input.em_size,
            level: input.level,
            visibility: Visibility::Ink,
            clusters: output.clusters,
            glyphs: output.glyphs,
            advances: output.advances,
            offsets: output.offsets,
        });
        position += length;
    }
    Ok(runs)
}

fn paragraph_at_native_cuts(
    engine: &DirectWriteEngine,
    prepared: &Prepared,
    font: &FontSpec,
    bound: usize,
) -> Result<Vec<Run>, NoCut> {
    let shaper = Shaper::new(engine.factories.as_ref().unwrap()).unwrap();
    engine
        .with_slot(font, |slot| {
            let base = BaseFont::new(slot, font).unwrap();
            let mut result = Vec::new();
            let mut position = 0;
            while position < prepared.utf16().len() {
                let script = prepared.scripts[position];
                let level = prepared.levels[position].resolved;
                let end = (position + 1..prepared.utf16().len())
                    .find(|&index| {
                        prepared.scripts[index] != script
                            || prepared.levels[index].resolved != level
                    })
                    .unwrap_or(prepared.utf16().len());
                let (face, scale, mapped) =
                    shaper.map(prepared, position..end, &base, font).unwrap();
                let input = Input {
                    text: &prepared.utf16()[position..position + mapped],
                    script,
                    level,
                    face: &face,
                    em_size: native_float(font.size).unwrap() * scale,
                    // SAFETY: Prepared retains the source and locale for this witness.
                    locale: unsafe { &(*prepared.source.as_ptr()).locale },
                };
                match continue_at_native_cuts(
                    &shaper,
                    &input,
                    native_float(font.tracking).unwrap(),
                    bound,
                ) {
                    Ok(runs) => result.extend(runs.into_iter().map(|mut run| {
                        run.source = position + run.source.start..position + run.source.end;
                        run
                    })),
                    Err(mut error) => {
                        error.position += position;
                        return Err(error);
                    }
                }
                position += mapped;
            }
            Ok(result)
        })
        .unwrap()
}

#[test]
fn paragraph_long_native_safe_cut_continuation_matches_complete_source() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let mut failures = Vec::new();
    for (name, text, family) in [
        ("ligatures", "office affine ".repeat(6_000), "Calibri"),
        ("arabic-words", "العربية ".repeat(10_000), "Segoe UI"),
        ("combining-pairs", "a\u{301}".repeat(40_000), "Segoe UI"),
        (
            "combining-tails",
            format!("a{} b ", "\u{301}".repeat(127)).repeat(512),
            "Segoe UI",
        ),
        (
            "kerning-ligatures",
            "AV To ffi office WAVE ".repeat(4_000),
            "Calibri",
        ),
        ("arabic-marks", "بَتِ لَا بَتِ ".repeat(6_000), "Segoe UI"),
    ] {
        for tracking in [-0.75, 0.0, 0.75] {
            let font = FontSpec {
                size: 19.0,
                family: bunny_ui::text_engine::Family::named(family),
                tracking,
                ..FontSpec::DEFAULT
            };
            let prepared = Prepared::new(factories, &text).unwrap();
            let layout = tests::reference_layout(&engine, &text, &font);
            let native = raster::reference_records(&layout);
            let mut expected: Vec<_> = native.iter().flat_map(clusters).collect();
            expected.sort_by_key(|cluster| cluster.source.start);
            assert_eq!(expected.first().unwrap().source.start, 0);
            assert_eq!(expected.last().unwrap().source.end, prepared.utf16().len());
            assert!(
                expected
                    .windows(2)
                    .all(|pair| pair[0].source.end == pair[1].source.start)
            );
            for bound in [4095, 16_000] {
                let label = format!("{name}, tracking={tracking}, bound={bound}");
                match paragraph_at_native_cuts(&engine, &prepared, &font, bound) {
                    Ok(runs) => {
                        let count = runs.len();
                        if let Some(error) = difference(&label, runs, &expected) {
                            eprintln!("native safe-cut mismatch: {error}");
                            failures.push(error);
                        } else {
                            eprintln!("native safe-cut parity: {label}, runs={count}");
                        }
                    }
                    Err(error) => {
                        eprintln!("native safe-cut unavailable: {label}: {error:?}");
                        failures.push(format!("{label}: {error:?}"));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "native safe-cut failures: {failures:#?}"
    );
}

#[test]
fn paragraph_long_native_safe_cut_refuses_connected_and_single_cluster_sources() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let shaper = Shaper::new(factories).unwrap();
    let font = FontSpec::DEFAULT;
    for (name, text) in [
        ("connected-beh", "ب".repeat(70_000)),
        (
            "one-combining-cluster",
            format!("a{}", "\u{301}".repeat(70_000)),
        ),
    ] {
        let prepared = Prepared::new(factories, &text).unwrap();
        engine.with_slot(&font, |slot| {
            let base = BaseFont::new(slot, &font).unwrap();
            let (face, scale, mapped) = shaper.map(&prepared, 0..prepared.utf16().len(), &base, &font).unwrap();
            assert_eq!(mapped, prepared.utf16().len());
            let input = Input {
                text: prepared.utf16(),
                script: prepared.scripts[0],
                level: prepared.levels[0].resolved,
                face: &face,
                em_size: native_float(font.size).unwrap() * scale,
                // SAFETY: Prepared owns the source for the complete test call.
                locale: unsafe { &(*prepared.source.as_ptr()).locale },
            };
            for bound in [4095,16_000] {
                let Err(error) = continue_at_native_cuts(&shaper, &input, 0.0, bound) else {
                    panic!("{name}: native flags cannot authorize this continuation");
                };
                assert_eq!(error.position, 0);
                assert_eq!(error.probed, bound);
                eprintln!("native safe-cut unavailable: {name}, bound={bound}; this is not candidate support");
            }
        }).unwrap();
    }
}

#[test]
fn paragraph_native_safe_cuts_refuse_incomplete_distant_opentype_context() {
    let engine = DirectWriteEngine::new();
    assert!(engine.register_font(include_bytes!("../tests/fixtures/paragraph-context.ttf")));
    let font = FontSpec {
        size: 19.0,
        family: bunny_ui::text_engine::Family::named("Bunny Paragraph Context"),
        ..FontSpec::DEFAULT
    };
    let text = format!("a{}c", "b".repeat(128));
    let factories = engine.factories.as_ref().unwrap();
    let prepared = Prepared::new(factories, &text).unwrap();
    let layout = tests::reference_layout(&engine, &text, &font);
    let native = raster::reference_records(&layout);
    let mut expected: Vec<_> = native.iter().flat_map(clusters).collect();
    expected.sort_by_key(|cluster| cluster.source.start);
    assert_eq!(expected.len(), 130);
    assert_eq!(
        expected[0].glyphs,
        [4],
        "complete context must activate the custom feature"
    );
    let shaper = Shaper::new(factories).unwrap();
    engine
        .with_slot(&font, |slot| {
            let base = BaseFont::new(slot, &font).unwrap();
            let (face, scale, mapped) = shaper
                .map(&prepared, 0..prepared.utf16().len(), &base, &font)
                .unwrap();
            assert_eq!(mapped, 130);
            let input = Input {
                text: prepared.utf16(),
                script: prepared.scripts[0],
                level: prepared.levels[0].resolved,
                face: &face,
                em_size: native_float(font.size).unwrap() * scale,
                // SAFETY: Prepared owns the source and locale for this entire witness.
                locale: unsafe { &(*prepared.source.as_ptr()).locale },
            };
            for bound in [31, 127] {
                let Err(error) = continue_at_native_cuts(&shaper, &input, 0.0, bound) else {
                    panic!("incomplete distant context must not authorize a cut");
                };
                assert_eq!(error.position, 0);
                assert_eq!(error.probed, bound);
                assert_eq!(error.safe_flags, 0);
                assert_eq!(error.cluster_boundaries, bound - 1);
                eprintln!("native distant-context refuses bound={bound}: independent clusters with no safe flag");
            }
            let runs = continue_at_native_cuts(&shaper, &input, 0.0, 511).unwrap();
            assert!(difference("complete distant context", runs, &expected).is_none());
            eprintln!("native distant-context complete 130-unit input matches the feature oracle");
        })
        .unwrap();
}
