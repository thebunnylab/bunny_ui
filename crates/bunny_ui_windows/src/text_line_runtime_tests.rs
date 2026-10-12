use super::*;
use crate::text::{
    Color, Com, D2D1_DRAW_TEXT_COLOR_FONT, D2dColor, D2dPoint, DirectWriteEngine,
    DwriteTextMetrics, FontSpec, TextEngine, TextRaster, com_ok,
};
use bunny_ui::text_engine::Slant;
use bunny_ui::text_source::TextContent;
use glyphs::raster::Surface;
use std::ffi::c_void;

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "This native fixture checks finite nonnegative values within u32 before converting integral pixels."
)]
fn integer_pixel(value: f64) -> u32 {
    assert!(value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX));
    value as u32
}

fn native_raster(
    engine: &DirectWriteEngine,
    text: &str,
    font: &FontSpec,
    color: Color,
    scale: u32,
) -> TextRaster {
    let layout = glyphs::tests::reference_layout(engine, text, font);
    let mut dimensions = DwriteTextMetrics::default();
    let mut line = super::tests::Metrics::default();
    let mut count = 0;
    // SAFETY: the independent native layout supplies its own width, line box
    // and baseline through the exact SDK ABI; no candidate metrics are reused.
    unsafe {
        assert!(com_ok(((*(*layout.as_ptr()).vtbl).get_metrics)(
            layout.as_ptr(),
            &raw mut dimensions
        )));
        let pointer = layout.as_ptr().cast::<c_void>();
        let table = &**pointer.cast::<*const super::tests::LayoutVtbl>();
        assert!(com_ok((table.lines)(
            pointer,
            &raw mut line,
            1,
            &raw mut count
        )));
    }
    assert_eq!(count, 1);
    let width = integer_pixel(
        (f64::from(dimensions.width_including_trailing_whitespace) * f64::from(scale)).ceil(),
    );
    let height = integer_pixel((f64::from(line.height) * f64::from(scale)).ceil());
    let slack = f64::from(height) / f64::from(scale) - f64::from(line.height);
    let surface = Surface::new(
        engine.factories.as_ref().unwrap(),
        width,
        height,
        glyphs::native_float(f64::from(scale)).unwrap(),
    )
    .unwrap();
    let ink = D2dColor {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    };
    let raw = surface
        .paint(|target| {
            let mut pointer = std::ptr::null_mut();
            // SAFETY: live target, brush and independent complete TextLayout.
            unsafe {
                assert!(com_ok(((*(*target.as_ptr()).vtbl)
                    .create_solid_color_brush)(
                    target.as_ptr(),
                    &raw const ink,
                    std::ptr::null(),
                    &raw mut pointer
                )));
                let brush = Com::from_raw(pointer).unwrap();
                ((*(*target.as_ptr()).vtbl).draw_text_layout)(
                    target.as_ptr(),
                    D2dPoint {
                        x: 0.0,
                        y: glyphs::native_float(slack).unwrap(),
                    },
                    layout.as_ptr(),
                    brush.as_ptr(),
                    D2D1_DRAW_TEXT_COLOR_FONT,
                );
            }
            Ok(())
        })
        .unwrap();
    // Independent test-side PBGRA conversion, including foreground opacity and
    // native emoji palette alpha. Production never calls this reference helper.
    let rgba = raw
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let straight = |channel| {
                if pixel[3] == 0 {
                    pixel[channel]
                } else {
                    u8::try_from(
                        ((u32::from(pixel[channel]) * 255 + u32::from(pixel[3]) / 2)
                            / u32::from(pixel[3]))
                        .min(255),
                    )
                    .unwrap()
                }
            };
            [straight(2), straight(1), straight(0), pixel[3]]
        })
        .collect();
    TextRaster {
        width: usize::try_from(width).unwrap(),
        height: usize::try_from(height).unwrap(),
        baseline: usize::try_from(integer_pixel(
            ((f64::from(line.baseline) + slack) * f64::from(scale)).round(),
        ))
        .unwrap(),
        rgba,
    }
}

#[test]
fn paragraph_runtime_public_raster_matches_native_metrics_baseline_and_rgba() {
    let engine = DirectWriteEngine::new();
    let color = Color {
        r: 19,
        g: 87,
        b: 193,
        a: 143,
    };
    for text in [
        "office a\u{301}",
        "abc العربية 😀",
        "\u{202e}abc def\u{202c}",
    ] {
        let content = TextContent::from(text);
        let source = content.line(0..text.len()).unwrap();
        for slant in [Slant::Upright, Slant::Italic] {
            for tracking in [-0.75, 0.75] {
                let font = FontSpec {
                    size: 21.0,
                    slant,
                    tracking,
                    ..FontSpec::DEFAULT
                };
                for scale in [1_u32, 2] {
                    let expected = native_raster(&engine, text, &font, color, scale);
                    let actual = engine
                        .raster_paragraph_line(
                            &source,
                            &font,
                            color,
                            usize::try_from(scale).unwrap(),
                        )
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        (actual.width, actual.height, actual.baseline),
                        (expected.width, expected.height, expected.baseline),
                        "{text:?} {font:?} scale{scale}"
                    );
                    let differences = actual
                        .rgba
                        .iter()
                        .zip(&expected.rgba)
                        .filter(|(a, b)| a != b)
                        .count();
                    assert_eq!(
                        differences, 0,
                        "{text:?} {font:?} scale{scale}: differing RGBA bytes"
                    );
                }
            }
        }
    }
}
