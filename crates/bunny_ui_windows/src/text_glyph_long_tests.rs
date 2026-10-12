use super::*;
use crate::text::{DirectWriteEngine, TextEngine};
use bunny_ui::text_source::TextContent;

#[derive(Debug, PartialEq)]
struct Cluster {
    source: Range<usize>,
    em_size: f32,
    level: u32,
    glyphs: Vec<u16>,
    advances: Vec<f32>,
    offsets: Vec<ffi::Offset>,
}

fn clusters(run: &raster::NativeRun) -> Vec<Cluster> {
    let mut boundaries = run.clusters.clone();
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut source = run.source.start;
    run.clusters
        .chunk_by(|a, b| a == b)
        .map(|units| {
            let start = usize::from(units[0]);
            let index = boundaries.binary_search(&units[0]).unwrap();
            let end = boundaries
                .get(index + 1)
                .map_or(run.glyphs.len(), |&value| usize::from(value));
            let range = source..source + units.len();
            source = range.end;
            Cluster {
                source: range,
                em_size: run.em_size,
                level: run.level,
                glyphs: run.glyphs[start..end].to_vec(),
                advances: run.advances[start..end].to_vec(),
                offsets: run.offsets[start..end].to_vec(),
            }
        })
        .collect()
}

#[test]
fn paragraph_long_spans_match_independent_complete_native_glyphs() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let shaper = Shaper::new(factories).unwrap();
    let mut failures = Vec::new();
    for (name, text) in [
        ("plain", "a".repeat(70_000)),
        ("latin-ligatures", "office affine ".repeat(6_000)),
        ("arabic-words", "العربية ".repeat(10_000)),
        ("arabic-connected", "ب".repeat(70_000)),
        ("combining-clusters", "a\u{301}".repeat(40_000)),
    ] {
        let font = FontSpec {
            size: 19.0,
            family: if name == "latin-ligatures" {
                bunny_ui::text_engine::Family::named("Calibri")
            } else {
                bunny_ui::text_engine::Family::default()
            },
            ..FontSpec::DEFAULT
        };
        let prepared = Prepared::new(factories, &text).unwrap();
        let layout = tests::reference_layout(&engine, &text, &font);
        let native = raster::reference_records(&layout);
        eprintln!(
            "long native {name}: units {}, glyph runs {:?}",
            prepared.utf16().len(),
            native
                .iter()
                .map(|run| (run.source.clone(), run.glyphs.len()))
                .collect::<Vec<_>>()
        );
        if name == "arabic-connected" {
            for run in &native {
                eprintln!(
                    "native joining {:?}: first {:?}, last {:?}",
                    run.source,
                    &run.glyphs[..3],
                    &run.glyphs[run.glyphs.len() - 3..]
                );
            }
            let sample = tests::shape(&engine, &"ب".repeat(32), &font);
            eprintln!("native joining short complete word: {:?}", sample[0].glyphs);
        }
        let mut expected: Vec<_> = native.iter().flat_map(clusters).collect();
        expected.sort_by_key(|cluster| cluster.source.start);
        assert_eq!(expected.first().unwrap().source.start, 0, "{name}");
        assert_eq!(
            expected.last().unwrap().source.end,
            prepared.utf16().len(),
            "{name}"
        );
        assert!(
            expected
                .windows(2)
                .all(|pair| pair[0].source.end == pair[1].source.start),
            "{name}"
        );
        let candidate = engine
            .with_slot(&font, |slot| {
                let base = BaseFont::new(slot, &font).unwrap();
                shaper.shape(&prepared, 0..prepared.utf16().len(), &base, &font)
            })
            .unwrap();
        match candidate {
            Err(error) => failures.push(format!("{name}: {error:?}")),
            Ok(runs) => {
                let mut actual: Vec<_> = runs
                    .into_iter()
                    .flat_map(|run| {
                        clusters(&raster::NativeRun {
                            source: run.source,
                            em_size: run.em_size,
                            level: u32::from(run.level),
                            clusters: run.clusters,
                            glyphs: run.glyphs,
                            advances: run.advances,
                            offsets: run.offsets,
                        })
                    })
                    .collect();
                actual.sort_by_key(|cluster| cluster.source.start);
                let difference = actual.iter().zip(&expected).position(|(a, b)| a != b);
                if actual.len() != expected.len() || difference.is_some() {
                    let first = difference.unwrap_or_else(|| actual.len().min(expected.len()));
                    failures.push(format!(
                        "{name}: {} vs {} clusters, first difference {first}: {:?} vs {:?}",
                        actual.len(),
                        expected.len(),
                        actual.get(first),
                        expected.get(first)
                    ));
                }
            }
        }
        let content = TextContent::from(text.as_str());
        let source = content.line(0..text.len()).unwrap();
        if let Err(error) = engine.measure_paragraph_line(&source, &font) {
            failures.push(format!("public {name}: {error:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "long paragraph failures: {failures:#?}"
    );
}

// Characterize the native ABI before choosing a production split policy. Only
// successful GetGlyphs output may be inspected; failed buffers are undefined.
#[test]
fn paragraph_long_native_capacity_and_safe_boundary_witness() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let shaper = Shaper::new(factories).unwrap();
    let font = FontSpec::DEFAULT;
    engine.with_slot(&font, |slot| {
        let base = BaseFont::new(slot, &font).unwrap();
        let face = create_face(&base.font).unwrap();
        for (name, text) in [
            ("plain", "a".repeat(70_000)),
            ("latin-ligatures", "office affine ".repeat(6_000)),
            ("arabic-words", "العربية ".repeat(10_000)),
            ("arabic-connected", "ب".repeat(70_000)),
            ("combining-clusters", "a\u{301}".repeat(40_000)),
            ("one-combining-cluster", format!("a{}", "\u{301}".repeat(70_000))),
        ] {
            let prepared = Prepared::new(factories, &text).unwrap();
            assert!(prepared.scripts.iter().all(|&script| script == prepared.scripts[0]), "{name}: homogeneous script required");
            let text = prepared.utf16();
            for (length, capacity) in [
                (4096, 8208),
                (4096, 65_535),
                (4096, 65_536),
                (text.len(), 65_535),
                (text.len(), 65_536),
                (text.len(), text.len() * 2 + 16),
            ] {
                let text = &text[..length];
                let mut output = Output::new(text.len()).unwrap();
                output.glyphs = vec![0;capacity];
                output.glyph_properties = vec![0;capacity];
                let mut count = 0;
                // SAFETY: exact SDK ABI, homogeneous native script/level, a
                // live font face and buffers with their declared capacities.
                let status = unsafe {
                    ((*(*shaper.analyzer.as_ptr()).vtbl).glyphs)(
                        shaper.analyzer.as_ptr(),text.as_ptr(),u32::try_from(text.len()).unwrap(),
                        face.as_ptr(),0,i32::from(!prepared.levels[0].resolved.is_multiple_of(2)),
                        &raw const prepared.scripts[0],(*prepared.source.as_ptr()).locale.as_ptr(),
                        std::ptr::null_mut(),std::ptr::null(),std::ptr::null(),0,
                        u32::try_from(capacity).unwrap(),output.clusters.as_mut_ptr(),output.text_properties.as_mut_ptr(),
                        output.glyphs.as_mut_ptr(),output.glyph_properties.as_mut_ptr(),&raw mut count,
                    )
                };
                if com_ok(status) {
                    assert!(count as usize <= capacity);
                    let safe: Vec<_> = output.text_properties.iter().enumerate().filter_map(|(i,&flags)| (flags & 4 != 0).then_some(i+1)).collect();
                    let reversals = output.clusters.windows(2).filter(|pair| pair[0] > pair[1]).count();
                    eprintln!("native capacity {name}: units {}, capacity {capacity}, glyphs {count}, max cluster {:?}, map decreases {reversals}, safe count {}, first {:?}, last {:?}",text.len(),output.clusters.iter().max(),safe.len(),&safe[..safe.len().min(8)],&safe[safe.len().saturating_sub(8)..]);
                } else {
                    eprintln!("native capacity {name}: units {}, capacity {capacity}, status {status:#x}; no failure output consumed",text.len());
                }
            }
        }
    }).unwrap();
}
