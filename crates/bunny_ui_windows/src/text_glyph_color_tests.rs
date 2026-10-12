use super::super::super::{
    native_float,
    tests::{reference_layout, shape},
};
use super::*;
use crate::text::{D2D1_DRAW_TEXT_COLOR_FONT, DirectWriteEngine, FontSpec};
use bunny_ui::text_engine::{Family, Slant};

fn reference(
    engine: &DirectWriteEngine,
    text: &str,
    font: &FontSpec,
    color: Color,
    scale: f32,
) -> Vec<u8> {
    let surface = Surface::new(engine.factories.as_ref().unwrap(), 640, 160, scale).unwrap();
    let layout = reference_layout(engine, text, font);
    let color = D2dColor {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    };
    let brush = brush(&surface, &color).unwrap();
    surface
        .paint(|target| {
            // SAFETY: high-level full TextLayout is the independent pixel reference;
            // its own native analysis/shaping pipeline is untouched.
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

fn rendered(
    engine: &DirectWriteEngine,
    text: &str,
    font: &FontSpec,
    color: Color,
    scale: f32,
    legacy: u8,
) -> Vec<u8> {
    eprintln!(
        "raster reference: text={text:?}, scale={scale}, path={legacy}, slant={:?}",
        font.slant
    );
    let factories = engine.factories.as_ref().unwrap();
    let surface = Surface::new(factories, 640, 160, scale).unwrap();
    let mut renderer = match legacy {
        0 => Renderer::new(&surface, factories, color).unwrap(),
        1 => {
            Renderer::with_mode(&surface, color, translated(&surface, factories).unwrap()).unwrap()
        }
        2 => Renderer::with_mode(
            &surface,
            color,
            Mode::Translated {
                factory: Translation::Layers(
                    ffi::query(&factories.dwrite, &ffi::FACTORY2, "legacy COLR").unwrap(),
                ),
                context: None,
            },
        )
        .unwrap(),
        _ => panic!("unknown native test path"),
    };
    let runs = shape(engine, text, font);
    let ascent = engine
        .with_slot(font, |slot| native_float(slot.ascent).unwrap())
        .unwrap();
    // TextLayout snaps its horizontal baseline in physical pixels. Preserve
    // that native policy while advances retain their fractional positions.
    let baseline = ((8.0 + ascent) * scale).round() / scale;
    surface
        .paint(|_| {
            let mut x = 8.0;
            for run in &runs {
                let width = native_float(run.width()).unwrap();
                let origin = if run.level.is_multiple_of(2) {
                    x
                } else {
                    assert_eq!(runs.len(), 1, "this reference case is a single RTL run");
                    x + width
                };
                renderer.draw(
                    run,
                    D2dPoint {
                        x: origin,
                        y: baseline,
                    },
                )?;
                x += width;
            }
            Ok(())
        })
        .unwrap()
}

fn assert_pixels(expected: &[u8], actual: &[u8], label: &str) {
    assert_eq!(expected.len(), actual.len(), "{label}: bitmap lengths");
    assert_eq!(expected.len() % 4, 0, "{label}: complete PBGRA pixels");
    let differences = expected
        .as_chunks::<4>()
        .0
        .iter()
        .zip(actual.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a != b)
        .count();
    if differences != 0 {
        let bounds = |pixels: &[u8]| {
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, p)| p[3] > 0)
                .fold((640, 160, 0, 0), |(l, t, r, b), (i, _)| {
                    (
                        l.min(i % 640),
                        t.min(i / 640),
                        r.max(i % 640),
                        b.max(i / 640),
                    )
                })
        };
        eprintln!(
            "{label}: expected bounds {:?}, actual bounds {:?}",
            bounds(expected),
            bounds(actual)
        );
    }
    assert_eq!(differences, 0, "{label}: mismatching PBGRA pixels");
}

#[test]
fn paragraph_raster_glyph_pixels_match_independent_native_text_layout() {
    let engine = DirectWriteEngine::new();
    for scale in [1.0, 2.0] {
        for text in [
            "Hello office",
            "a\u{301}e\u{327}\u{301}",
            "العربية",
            "😀",
            "Hello 😀",
        ] {
            for slant in [Slant::Upright, Slant::Italic] {
                let font = FontSpec {
                    size: 21.0,
                    slant,
                    tracking: 0.75,
                    ..FontSpec::DEFAULT
                };
                let color = Color::rgb(173, 29, 83);
                let expected = reference(&engine, text, &font, color, scale);
                let actual = rendered(&engine, text, &font, color, scale, 0);
                if actual != expected {
                    super::reference::trace(&reference_layout(&engine, text, &font), scale);
                    for run in shape(&engine, text, &font) {
                        eprintln!(
                            "shaped run: width={}, em={}, level={}, glyphs={:?}, advances={:?}, offsets={:?}",
                            run.width(),
                            run.em_size,
                            run.level,
                            run.glyphs,
                            run.advances,
                            run.offsets
                        );
                    }
                }
                assert_pixels(
                    &expected,
                    &actual,
                    &format!("{text:?}, {slant:?}, scale {scale}"),
                );
            }
        }
    }
}

fn color_engine() -> DirectWriteEngine {
    let engine = DirectWriteEngine::new();
    assert!(
        engine.register_font(include_bytes!("../tests/fixtures/paragraph-color.ttf")),
        "synthetic color fixture registers"
    );
    engine
}
fn color_font() -> FontSpec {
    FontSpec {
        family: Family::named("Bunny Paragraph Color"),
        size: 32.0,
        ..FontSpec::DEFAULT
    }
}
#[test]
fn paragraph_raster_modern_color_formats_match_native_pixels_and_keep_color() {
    let engine = color_engine();
    let font = color_font();
    let ink = Color::rgb(47, 47, 47);
    for scale in [1.0, 2.0] {
        for text in ["a", "b", "c", "d", "e", "abcde"] {
            let expected = reference(&engine, text, &font, ink, scale);
            let actual = rendered(&engine, text, &font, ink, scale, 0);
            assert_pixels(
                &expected,
                &actual,
                &format!("color fixture {text}, scale {scale}"),
            );
            if text != "a" {
                assert!(
                    actual
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|p| p[3] > 0 && (p[0] != p[1] || p[1] != p[2])),
                    "{text} retains native colors"
                );
            }
        }
    }
}

#[test]
fn paragraph_raster_translated_svg_bitmap_and_colr_paths_match_native_pixels() {
    let engine = color_engine();
    let font = color_font();
    let ink = Color::rgb(67, 31, 173);
    for scale in [1.0, 2.0] {
        for text in ["a", "b", "c", "e", "abce"] {
            let expected = reference(&engine, text, &font, ink, scale);
            let actual = rendered(&engine, text, &font, ink, scale, 1);
            assert_pixels(
                &expected,
                &actual,
                &format!("translated fixture {text}, scale {scale}"),
            );
        }
        for text in ["a", "b", "ab"] {
            let expected = reference(&engine, text, &font, ink, scale);
            let actual = rendered(&engine, text, &font, ink, scale, 2);
            assert_pixels(
                &expected,
                &actual,
                &format!("COLR layers {text}, scale {scale}"),
            );
        }
    }
}

#[test]
fn paragraph_raster_color_abi_keeps_cpp_base_tail_padding() {
    assert_eq!(std::mem::size_of::<GlyphRun<'_>>(), 48);
    assert_eq!(std::mem::size_of::<native::Layer<'_>>(), 88);
    assert_eq!(std::mem::offset_of!(native::Layer1<'_>, image_format), 88);
    assert_eq!(std::mem::size_of::<native::Layer1<'_>>(), 96);
}
