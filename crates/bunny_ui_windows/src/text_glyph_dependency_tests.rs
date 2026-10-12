//! Inspect the resolved native face and falsify source bounds inferred from
//! short feature contexts. This module changes no production shaping policy.

use super::super::super::super::{Com, TextError, com_ok, ffi};
use super::super::super::difference;
use super::{
    BaseFont, DirectWriteEngine, Family, FontSpec, Prepared, Shaper, mismatch, overlap, reference,
};
use std::ffi::c_void;

#[repr(C)]
struct FaceTables {
    _slots_0_11: [usize; 12],
    get: unsafe extern "system" fn(
        *mut ffi::Face,
        u32,
        *mut *const c_void,
        *mut u32,
        *mut *mut c_void,
        *mut i32,
    ) -> i32,
    release: unsafe extern "system" fn(*mut ffi::Face, *mut c_void),
}

struct TableLease<'a> {
    face: &'a Com<ffi::Face>,
    context: *mut c_void,
}
impl Drop for TableLease<'_> {
    fn drop(&mut self) {
        // SAFETY: TryGetFontTable supplied this context for this retained face.
        unsafe {
            let table = &**self.face.as_ptr().cast::<*const FaceTables>();
            (table.release)(self.face.as_ptr(), self.context);
        }
    }
}

fn font_table(face: &Com<ffi::Face>, tag: [u8; 4]) -> Option<Vec<u8>> {
    let mut data = std::ptr::null();
    let mut size = 0;
    let mut context = std::ptr::null_mut();
    let mut exists = 0;
    // SAFETY: pinned SDK IDWriteFontFace slots 12/13, a live resolved face and
    // initialized outputs. The lease retains native table memory until copied.
    unsafe {
        let table = &**face.as_ptr().cast::<*const FaceTables>();
        let status = (table.get)(
            face.as_ptr(),
            u32::from_le_bytes(tag),
            &raw mut data,
            &raw mut size,
            &raw mut context,
            &raw mut exists,
        );
        assert!(com_ok(status), "native table {tag:?}: {status:#x}");
        if exists == 0 {
            return None;
        }
        let _lease = TableLease { face, context };
        assert!(!data.is_null() && size > 0);
        Some(std::slice::from_raw_parts(data.cast::<u8>(), size as usize).to_vec())
    }
}

fn word(bytes: &[u8], at: usize) -> usize {
    usize::from(u16::from_be_bytes(bytes[at..at + 2].try_into().unwrap()))
}

fn family_name(bytes: &[u8]) -> Option<String> {
    let count = word(bytes, 2);
    let storage = word(bytes, 4);
    for identifier in [16, 1] {
        for record in 0..count {
            let at = 6 + record * 12;
            if word(bytes, at) == 3
                && matches!(word(bytes, at + 2), 1 | 10)
                && word(bytes, at + 4) == 0x409
                && word(bytes, at + 6) == identifier
            {
                let length = word(bytes, at + 8);
                let start = storage + word(bytes, at + 10);
                let (pairs, remainder) = bytes[start..start + length].as_chunks::<2>();
                assert!(
                    remainder.is_empty(),
                    "native UTF-16 name must contain complete units"
                );
                let text: Vec<_> = pairs.iter().copied().map(u16::from_be_bytes).collect();
                return Some(String::from_utf16(&text).unwrap());
            }
        }
    }
    None
}

// Only inspect declared default-language feature headers. This does not decide
// which features the script engine enables, or certify any dependency bound.
fn declared_features(bytes: &[u8], script: [u8; 4], extension_type: usize) -> Vec<String> {
    let scripts = word(bytes, 4);
    let features = word(bytes, 6);
    let lookups = word(bytes, 8);
    let script_offset = [script, *b"DFLT"].into_iter().find_map(|tag| {
        (0..word(bytes, scripts)).find_map(|index| {
            let at = scripts + 2 + index * 6;
            (bytes[at..at + 4] == tag).then(|| scripts + word(bytes, at + 4))
        })
    });
    let Some(script_offset) = script_offset else {
        return Vec::new();
    };
    let default = word(bytes, script_offset);
    if default == 0 {
        return Vec::new();
    }
    let language = script_offset + default;
    let required = word(bytes, language + 2);
    let mut indices: Vec<_> = (0..word(bytes, language + 4))
        .map(|index| word(bytes, language + 6 + index * 2))
        .collect();
    if required != 0xffff {
        indices.push(required);
    }
    indices.sort_unstable();
    indices.dedup();
    indices
        .into_iter()
        .map(|index| {
            assert!(index < word(bytes, features));
            let at = features + 2 + index * 6;
            let tag = std::str::from_utf8(&bytes[at..at + 4]).unwrap();
            let feature = features + word(bytes, at + 4);
            let headers: Vec<_> = (0..word(bytes, feature + 2))
                .map(|item| {
                    let lookup = word(bytes, feature + 4 + item * 2);
                    assert!(lookup < word(bytes, lookups));
                    let entry = lookups + word(bytes, lookups + 2 + lookup * 2);
                    let kind = word(bytes, entry);
                    let mut resolved: Vec<_> = (0..word(bytes, entry + 4))
                        .map(|item| {
                            let subtable = entry + word(bytes, entry + 6 + item * 2);
                            if kind == extension_type {
                                assert_eq!(word(bytes, subtable), 1);
                                let offset = u32::from_be_bytes(
                                    bytes[subtable + 4..subtable + 8].try_into().unwrap(),
                                ) as usize;
                                (word(bytes, subtable + 2), word(bytes, subtable + offset))
                            } else {
                                (kind, word(bytes, subtable))
                            }
                        })
                        .collect();
                    resolved.sort_unstable();
                    resolved.dedup();
                    format!(
                        "{lookup}:type{kind}:flags{:#x}:resolved{resolved:?}",
                        word(bytes, entry + 2)
                    )
                })
                .collect();
            format!("{tag}={headers:?}")
        })
        .collect()
}

fn metadata(
    engine: &DirectWriteEngine,
    text: &str,
    font: &FontSpec,
    script: [u8; 4],
) -> (String, Option<usize>) {
    let factories = engine.factories.as_ref().unwrap();
    let prepared = Prepared::new(factories, text).unwrap();
    let shaper = Shaper::new(factories).unwrap();
    engine.with_slot(font, |slot| {
        let base = BaseFont::new(slot, font).unwrap();
        let (face, scale, mapped) = shaper.map(&prepared, 0..prepared.utf16().len(), &base, font).unwrap();
        assert_eq!(mapped, prepared.utf16().len());
        let name = family_name(&font_table(&face, *b"name").unwrap()).unwrap();
        let os2 = font_table(&face, *b"OS/2").unwrap();
        let max_context = (word(&os2, 0) >= 2).then(|| word(&os2, 94));
        eprintln!("native font dependency metadata: resolved={name}, scale={scale}, usMaxContext={max_context:?}");
        for tag in [*b"GSUB", *b"GPOS"] {
            let data = font_table(&face, tag);
            let features = data.as_ref().map(|data| declared_features(data, script, if tag == *b"GSUB" { 7 } else { 9 }));
            eprintln!("native font declared features: resolved={name}, table={}, script={}, features={features:?}", std::str::from_utf8(&tag).unwrap(), std::str::from_utf8(&script).unwrap());
        }
        (name, max_context)
    }).unwrap()
}

#[test]
fn paragraph_native_fonts_record_declared_dependency_headers() {
    let engine = DirectWriteEngine::new();
    for family in ["Segoe UI", "Arial", "Times New Roman"] {
        let font = FontSpec {
            size: 19.0,
            family: Family::named(family),
            ..FontSpec::DEFAULT
        };
        let (resolved, _) = metadata(&engine, "بَتِ", &font, *b"arab");
        assert_eq!(
            resolved, family,
            "record actual resolved identity, not just requested family"
        );
    }
}

#[test]
fn paragraph_contextual_substitution_can_propagate_beyond_declared_context() {
    let engine = DirectWriteEngine::new();
    assert!(engine.register_font(include_bytes!("../tests/fixtures/paragraph-cascade.ttf")));
    let text = format!("a{}", "b".repeat(8191));
    for tracking in [-0.75, 0.0, 0.75] {
        let font = FontSpec {
            size: 19.0,
            family: Family::named("Bunny Paragraph Cascade"),
            tracking,
            ..FontSpec::DEFAULT
        };
        let (name, max_context) = metadata(&engine, &text, &font, *b"latn");
        assert_eq!(name, "Bunny Paragraph Cascade");
        // In a chaining lookup, the maximum target+lookahead excludes backtrack.
        assert_eq!(max_context, Some(1));
        let expected = reference(&engine, &text, &font);
        assert_eq!(expected.len(), text.len());
        assert!(
            expected.iter().all(|cluster| cluster.glyphs == [1]),
            "native cascade must propagate across the entire complete witness"
        );
        for context in [2, 8, 128, 1024] {
            let actual = overlap(&engine, &text, &font, 1024, context).unwrap();
            let first = actual
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b)
                .unwrap();
            assert_eq!(first, (context / 1024 + 1) * 1024);
            assert_eq!(actual[first].glyphs, [2]);
            assert!(mismatch(&actual, &expected).is_some());
            eprintln!(
                "native cascade counterexample: tracking={tracking}, context={context}, declared=1, first_difference={first}"
            );
        }
        let complete = overlap(&engine, &text, &font, 1024, text.len()).unwrap();
        assert!(mismatch(&complete, &expected).is_none());
        eprintln!(
            "native cascade complete original context matches: tracking={tracking}, units={}",
            text.len()
        );
    }
}

#[test]
fn paragraph_production_never_silently_restarts_a_contextual_cascade() {
    let engine = DirectWriteEngine::new();
    assert!(engine.register_font(include_bytes!("../tests/fixtures/paragraph-cascade.ttf")));
    let text = format!("a{}", "b".repeat(69_999));
    let prepared = Prepared::new(engine.factories.as_ref().unwrap(), &text).unwrap();
    for tracking in [-0.75, 0.0, 0.75] {
        let font = FontSpec {
            size: 19.0,
            family: Family::named("Bunny Paragraph Cascade"),
            tracking,
            ..FontSpec::DEFAULT
        };
        let sample = reference(&engine, &format!("a{}", "b".repeat(8191)), &font);
        assert_eq!(sample.len(), 8192);
        assert!(sample.iter().all(|cluster| cluster.glyphs == [1]));
        let interior: Vec<_> = sample[1..sample.len() - 1]
            .iter()
            .cloned()
            .map(|mut cluster| {
                cluster.source = 0..1;
                cluster
            })
            .collect();
        assert!(interior.windows(2).all(|pair| pair[0] == pair[1]));
        // This source-generated single rule propagates the initial `a` through
        // every following `b`. The uniform native sample supplies exact metrics.
        let expected: Vec<_> = (0..text.len())
            .map(|index| {
                let mut cluster = sample[if index == 0 {
                    0
                } else if index + 1 == text.len() {
                    sample.len() - 1
                } else {
                    1
                }]
                .clone();
                cluster.source = index..index + 1;
                cluster
            })
            .collect();
        let result = engine
            .with_slot(&font, |slot| {
                let base = BaseFont::new(slot, &font).unwrap();
                Shaper::new(engine.factories.as_ref().unwrap())
                    .unwrap()
                    .shape(&prepared, 0..prepared.utf16().len(), &base, &font)
            })
            .unwrap();
        match result {
            Ok(runs) => {
                let error = difference("production cascade", runs, &expected);
                assert!(
                    error.is_none(),
                    "production must not return a silently broken cascade: {error:?}"
                );
                eprintln!(
                    "native cascade production exact parity: tracking={tracking}, units={}",
                    text.len()
                );
            }
            Err(TextError::Preparation {
                operation: "glyph continuation boundary",
                ..
            }) => {
                eprintln!(
                    "native cascade production explicit refusal: tracking={tracking}, units={}; not capability acceptance",
                    text.len()
                );
            }
            Err(error) => panic!("unexpected cascade failure: {error:?}"),
        }
    }
}
