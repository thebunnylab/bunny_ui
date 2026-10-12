//! Color-aware drawing of the already shaped native glyph stream.
//!
//! ## Wiring
//! Prefer the platform's complete color-glyph renderer. Older native interfaces
//! translate supported color formats to layers; only `DWRITE_E_NOCOLOR` selects
//! monochrome drawing. Unsupported interfaces and failed native calls differ.

use super::super::Run;
use super::{
    CONTEXT4, CONTEXT7, Com, Factories, Guid, Hresult, Surface, TextError, failure, ffi, owned,
};
use crate::text::{Color, D2dColor, D2dPoint, ID2D1Brush, com_ok};
use std::collections::HashMap;

#[path = "text_glyph_color_ffi.rs"]
mod native;
use native::{GlyphRun, Translation};

enum Mode {
    Native(Com<native::Context7>),
    Translated {
        factory: Translation,
        context: Option<Com<native::Context4>>,
    },
}
struct Brushes {
    foreground: Com<ID2D1Brush>,
    palette: HashMap<[u32; 4], Com<ID2D1Brush>>,
}
pub(super) struct Renderer<'a> {
    surface: &'a Surface,
    mode: Mode,
    brushes: Brushes,
}
impl<'a> Renderer<'a> {
    pub(super) fn new(
        surface: &'a Surface,
        factories: &Factories,
        color: Color,
    ) -> Result<Self, TextError> {
        let mode = ffi::query_optional(&surface.target, &CONTEXT7, "ID2D1DeviceContext7")?
            .map_or_else(
                || translated(surface, factories),
                |context| Ok(Mode::Native(context)),
            )?;
        Self::with_mode(surface, color, mode)
    }
    fn with_mode(surface: &'a Surface, color: Color, mode: Mode) -> Result<Self, TextError> {
        let ink = D2dColor {
            r: f32::from(color.r) / 255.0,
            g: f32::from(color.g) / 255.0,
            b: f32::from(color.b) / 255.0,
            a: f32::from(color.a) / 255.0,
        };
        let foreground = brush(surface, &ink)?;
        Ok(Self {
            surface,
            mode,
            brushes: Brushes {
                foreground,
                palette: HashMap::new(),
            },
        })
    }

    pub(super) fn draw(&mut self, run: &Run, origin: D2dPoint) -> Result<(), TextError> {
        if !origin.x.is_finite() || !origin.y.is_finite() {
            return Err(failure("glyph origin"));
        }
        let glyphs = GlyphRun::from_run(run)?;
        match &self.mode {
            Mode::Native(context) => {
                // SAFETY: the context, brush and all glyph buffers have owners
                // spanning the synchronous draw. EndDraw reports deferred errors.
                unsafe {
                    ((*(*context.as_ptr()).vtbl).draw_color)(
                        context.as_ptr(),
                        origin,
                        &raw const glyphs,
                        std::ptr::null(),
                        self.brushes.foreground.as_ptr(),
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                    );
                }
                Ok(())
            }
            Mode::Translated { factory, context } => {
                let transform = [self.surface.scale, 0.0, 0.0, self.surface.scale, 0.0, 0.0];
                match factory {
                    Translation::Modern(factory) => {
                        let mut output = std::ptr::null_mut();
                        // SAFETY: Factory4 slot 40 accepts the borrowed run and
                        // returns a separately owned color enumerator.
                        let status = unsafe {
                            ((*(*factory.as_ptr()).vtbl).translate)(
                                factory.as_ptr(),
                                origin,
                                &raw const glyphs,
                                std::ptr::null(),
                                0xff,
                                0,
                                &raw const transform,
                                0,
                                &raw mut output,
                            )
                        };
                        if status == native::NO_COLOR {
                            self.monochrome(origin, &glyphs);
                            return Ok(());
                        }
                        let layers = owned(status, output, "TranslateColorGlyphRun4")?;
                        draw_layers1(self.surface, context.as_ref(), &mut self.brushes, &layers)
                    }
                    Translation::Layers(factory) => {
                        let mut output = std::ptr::null_mut();
                        // SAFETY: Factory2 slot 28 uses separate FLOAT origins.
                        let status = unsafe {
                            ((*(*factory.as_ptr()).vtbl).translate)(
                                factory.as_ptr(),
                                origin.x,
                                origin.y,
                                &raw const glyphs,
                                std::ptr::null(),
                                0,
                                &raw const transform,
                                0,
                                &raw mut output,
                            )
                        };
                        if status == native::NO_COLOR {
                            self.monochrome(origin, &glyphs);
                            return Ok(());
                        }
                        let layers = owned(status, output, "TranslateColorGlyphRun2")?;
                        draw_layers(self.surface, &mut self.brushes, &layers)
                    }
                }
            }
        }
    }

    fn monochrome(&self, origin: D2dPoint, glyphs: &GlyphRun<'_>) {
        draw_outline(
            self.surface,
            origin,
            glyphs,
            self.brushes.foreground.as_ptr(),
            0,
        );
    }
}

fn translated(surface: &Surface, factories: &Factories) -> Result<Mode, TextError> {
    let factory = ffi::query_optional(&factories.dwrite, &native::FACTORY4, "IDWriteFactory4")?
        .map_or_else(
            || {
                ffi::query(&factories.dwrite, &ffi::FACTORY2, "IDWriteFactory2")
                    .map(Translation::Layers)
            },
            |factory| Ok(Translation::Modern(factory)),
        )?;
    let context = ffi::query_optional(&surface.target, &CONTEXT4, "ID2D1DeviceContext4")?;
    Ok(Mode::Translated { factory, context })
}
fn brush(surface: &Surface, color: &D2dColor) -> Result<Com<ID2D1Brush>, TextError> {
    let mut output = std::ptr::null_mut();
    // SAFETY: surface owns the target and the native call copies the color.
    let status = unsafe {
        ((*(*surface.target.as_ptr()).vtbl).create_solid_color_brush)(
            surface.target.as_ptr(),
            color,
            std::ptr::null(),
            &raw mut output,
        )
    };
    owned(status, output, "CreateSolidColorBrush")
}
impl Brushes {
    fn layer(
        &mut self,
        surface: &Surface,
        layer: &native::Layer<'_>,
    ) -> Result<*mut ID2D1Brush, TextError> {
        if layer.palette == u16::MAX {
            return Ok(self.foreground.as_ptr());
        }
        let values = [layer.color.r, layer.color.g, layer.color.b, layer.color.a];
        if values
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(failure("color palette metrics"));
        }
        let key = values.map(f32::to_bits);
        if let Some(brush) = self.palette.get(&key) {
            return Ok(brush.as_ptr());
        }
        let brush = brush(surface, &layer.color)?;
        let pointer = brush.as_ptr();
        self.palette
            .try_reserve(1)
            .map_err(|_| failure("color brush allocation"))?;
        self.palette.insert(key, brush);
        Ok(pointer)
    }
}
fn draw_outline(
    surface: &Surface,
    origin: D2dPoint,
    glyphs: &GlyphRun<'_>,
    brush: *mut ID2D1Brush,
    mode: u32,
) {
    // SAFETY: ID2D1RenderTarget slot 29 draws a live borrowed native run. The
    // render scope retains all brushes until EndDraw has consumed its batch.
    unsafe {
        let target = surface.target.as_ptr().cast::<native::Target>();
        ((*(*target).vtbl).draw)(target, origin, glyphs, brush, mode);
    }
}
fn draw_layer(
    surface: &Surface,
    context: Option<&Com<native::Context4>>,
    brushes: &mut Brushes,
    layer: &native::Layer<'_>,
    format: u32,
    mode: u32,
) -> Result<(), TextError> {
    match format {
        1 | 2 | 4 => {
            let brush = brushes.layer(surface, layer)?;
            draw_outline(surface, layer.origin, &layer.run, brush, mode);
        }
        8 => {
            let context = context.ok_or_else(|| failure("SVG glyph context"))?;
            // SAFETY: this is one SVG run returned by native color translation.
            unsafe {
                ((*(*context.as_ptr()).vtbl).draw_svg)(
                    context.as_ptr(),
                    layer.origin,
                    &raw const layer.run,
                    brushes.foreground.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    mode,
                );
            }
        }
        16 | 32 | 64 | 128 => {
            let context = context.ok_or_else(|| failure("bitmap glyph context"))?;
            // SAFETY: only one native bitmap format is selected for this run.
            unsafe {
                ((*(*context.as_ptr()).vtbl).draw_bitmap)(
                    context.as_ptr(),
                    format,
                    layer.origin,
                    &raw const layer.run,
                    mode,
                    0,
                );
            }
        }
        _ => {
            eprintln!(
                "unsupported native glyph format={format:#x}, mode={mode}, palette={}, glyphs={}",
                layer.palette, layer.run.count
            );
            return Err(failure("color glyph format"));
        }
    }
    Ok(())
}
fn draw_layers1(
    surface: &Surface,
    context: Option<&Com<native::Context4>>,
    brushes: &mut Brushes,
    owner: &Com<native::Enumerator1>,
) -> Result<(), TextError> {
    // SAFETY: the owner remains live; each borrowed layer is consumed before
    // MoveNext invalidates its native buffers. No native pointer escapes.
    unsafe {
        let pointer = owner.as_ptr();
        let table = &*(*pointer).vtbl;
        loop {
            let mut present = 0;
            if !com_ok((table.next)(pointer, &raw mut present)) {
                return Err(failure("color layer advance"));
            }
            if present == 0 {
                break;
            }
            let mut layer = std::ptr::null();
            if !com_ok((table.current)(pointer, &raw mut layer)) || layer.is_null() {
                return Err(failure("color layer access"));
            }
            let layer = &*layer;
            draw_layer(
                surface,
                context,
                brushes,
                &layer.base,
                layer.image_format,
                layer.measuring_mode,
            )?;
        }
    }
    Ok(())
}
fn draw_layers(
    surface: &Surface,
    brushes: &mut Brushes,
    owner: &Com<native::Enumerator>,
) -> Result<(), TextError> {
    // SAFETY: as above, but Factory2 returns the original COLR-layer ABI.
    unsafe {
        let pointer = owner.as_ptr();
        let table = &*(*pointer).vtbl;
        loop {
            let mut present = 0;
            if !com_ok((table.next)(pointer, &raw mut present)) {
                return Err(failure("color layer advance"));
            }
            if present == 0 {
                break;
            }
            let mut layer = std::ptr::null();
            if !com_ok((table.current)(pointer, &raw mut layer)) || layer.is_null() {
                return Err(failure("color layer access"));
            }
            draw_layer(surface, None, brushes, &*layer, 4, 0)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "text_glyph_color_tests.rs"]
mod tests;
