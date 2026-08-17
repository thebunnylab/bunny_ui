//! Rasterizador CPU — 100% std, o mesmo para os quatro alvos.
//!
//! Pinta uma [`DisplayList`] num [`Bitmap`] RGBA. É o capital portátil do
//! primeiro pixel: no Mac o buffer vira `CGImage`, na web `putImageData`,
//! no Android `Bitmap` — o backend de plataforma só blita. GPU chega
//! quando o benchmark mandar; a interface (a display list) não muda.
//!
//! O snapping acontece AQUI, uma vez, ao converter as coordenadas lógicas
//! em pixels: arestas arredondadas em espaço de dispositivo (vizinhos que
//! convergem para a mesma coluna fecham sem fresta), decisão documentada e
//! localizada — nunca espalhada pelo layout.
//!
//! Texto entra pelo [`TextEngine`] do frame: o engine rasteriza a linha
//! num retângulo RGBA de alfa reto e o compositor daqui blenda no bitmap
//! — um único caminho de composição para a fonte pixel da casa e para o
//! CoreText do Mac (e para o canvas da web, um dia).

use crate::layout::{Color, DisplayList, DrawCommand, Rect};
use crate::text_engine::{PixelFont, TextEngine, TextRaster};

/// Um buffer RGBA (um `u32` `0xRRGGBBAA` por pixel, linhas de cima para
/// baixo) — o que o backend de plataforma blita na janela.
pub struct Bitmap {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    /// Pilha de clip em px físicos (interseções já resolvidas) — `set`
    /// consulta o topo; fill, stroke e texto respeitam de graça.
    clip: Vec<(i64, i64, i64, i64)>,
}

fn pack(color: Color) -> u32 {
    ((color.r as u32) << 24) | ((color.g as u32) << 16) | ((color.b as u32) << 8) | color.a as u32
}

/// Divisão exata por 255 com arredondamento — o clássico de compositing
/// inteiro: `round(x / 255)` sem float.
fn div255(x: u32) -> u32 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

/// Source-over de `src` sobre `dst` (ambos `0xRRGGBBAA`, alfa reto):
/// `out = src·sa + dst·(1−sa)` por canal. Fast paths: opaco sobrescreve,
/// invisível não toca.
fn blend_px(src: u32, dst: u32) -> u32 {
    let sa = src & 0xFF;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let inv = 255 - sa;
    let channel = |shift: u32| {
        let s = (src >> shift) & 0xFF;
        let d = (dst >> shift) & 0xFF;
        div255(s * sa + d * inv)
    };
    let a = sa + div255((dst & 0xFF) * inv);
    (channel(24) << 24) | (channel(16) << 16) | (channel(8) << 8) | a
}

impl Bitmap {
    pub fn new(width: usize, height: usize, background: Color) -> Self {
        Bitmap { width, height, pixels: vec![pack(background); width * height], clip: Vec::new() }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Os bytes crus, para o blit do backend.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn pixel(&self, x: usize, y: usize) -> Option<u32> {
        (x < self.width && y < self.height).then(|| self.pixels[y * self.width + x])
    }

    /// Bytes `R,G,B,A` por pixel, linha a linha — o formato que os blits de
    /// plataforma esperam sem discussão de endianness.
    pub fn to_rgba_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 4);
        for pixel in &self.pixels {
            bytes.push((pixel >> 24) as u8);
            bytes.push((pixel >> 16) as u8);
            bytes.push((pixel >> 8) as u8);
            bytes.push(*pixel as u8);
        }
        bytes
    }

    fn set(&mut self, x: i64, y: i64, color: u32) {
        if x < 0 || y < 0 || (x as usize) >= self.width || (y as usize) >= self.height {
            return;
        }
        if let Some((cx0, cy0, cx1, cy1)) = self.clip.last().copied()
            && (x < cx0 || y < cy0 || x >= cx1 || y >= cy1)
        {
            return;
        }
        let index = y as usize * self.width + x as usize;
        self.pixels[index] = blend_px(color, self.pixels[index]);
    }

    /// Empilha o clip snapado, já intersectado com o topo corrente.
    fn push_clip(&mut self, rect: Rect) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        let clipped = match self.clip.last().copied() {
            Some((cx0, cy0, cx1, cy1)) => (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1)),
            None => (x0, y0, x1, y1),
        };
        self.clip.push(clipped);
    }

    fn pop_clip(&mut self) {
        self.clip.pop();
    }

    /// Arestas arredondadas em device px — o ponto único de snapping.
    fn snap(rect: Rect) -> (i64, i64, i64, i64) {
        let x0 = rect.origin.x.round() as i64;
        let y0 = rect.origin.y.round() as i64;
        let x1 = (rect.origin.x + rect.size.width).round() as i64;
        let y1 = (rect.origin.y + rect.size.height).round() as i64;
        (x0, y0, x1, y1)
    }

    /// Preenchimento com cantos opcionais: inset por linha de varredura,
    /// círculo por canto, UMA raiz quadrada por linha — nunca por pixel.
    /// `corner_radius: 0.0` reproduz o retângulo reto byte a byte.
    fn fill_rect(&mut self, rect: Rect, color: Color, corner_radius: f64) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let packed = pack(color);
        let height = (y1 - y0) as f64;
        let radius = corner_radius
            .max(0.0)
            .min((x1 - x0) as f64 / 2.0)
            .min(height / 2.0);
        for y in y0..y1 {
            // distância do centro da linha à banda reta entre os cantos
            let center = (y - y0) as f64 + 0.5;
            let dy = if center < radius {
                radius - center
            } else if center > height - radius {
                center - (height - radius)
            } else {
                0.0
            };
            let inset = if dy > 0.0 {
                (radius - (radius * radius - dy * dy).sqrt()).round() as i64
            } else {
                0
            };
            for x in (x0 + inset)..(x1 - inset) {
                self.set(x, y, packed);
            }
        }
    }

    /// Moldura para dentro da aresta, em 4 barras SEM sobreposição — uma
    /// borda translúcida não pode blendar o canto duas vezes.
    fn stroke_rect(&mut self, rect: Rect, color: Color, width: f64) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let packed = pack(color);
        let thickness = width.max(1.0).round() as i64;
        let top_end = (y0 + thickness).min(y1);
        let bottom_start = (y1 - thickness).max(top_end);
        let left_end = (x0 + thickness).min(x1);
        let right_start = (x1 - thickness).max(left_end);
        for y in y0..top_end {
            for x in x0..x1 {
                self.set(x, y, packed);
            }
        }
        for y in bottom_start..y1 {
            for x in x0..x1 {
                self.set(x, y, packed);
            }
        }
        for y in top_end..bottom_start {
            for x in x0..left_end {
                self.set(x, y, packed);
            }
            for x in right_start..x1 {
                self.set(x, y, packed);
            }
        }
    }

    /// Compõe uma linha rasterizada pelo engine no origin lógico, snapado
    /// UMA vez (o raster já vem em pixels físicos).
    fn composite_text(&mut self, origin_x: f64, origin_y: f64, scale: usize, raster: &TextRaster) {
        let base_x = (origin_x * scale as f64).round() as i64;
        let base_y = (origin_y * scale as f64).round() as i64;
        for row in 0..raster.height {
            for col in 0..raster.width {
                let index = (row * raster.width + col) * 4;
                let alpha = raster.rgba[index + 3];
                if alpha == 0 {
                    continue;
                }
                let packed = ((raster.rgba[index] as u32) << 24)
                    | ((raster.rgba[index + 1] as u32) << 16)
                    | ((raster.rgba[index + 2] as u32) << 8)
                    | alpha as u32;
                self.set(base_x + col as i64, base_y + row as i64, packed);
            }
        }
    }
}

fn scale_rect(rect: Rect, scale: f64) -> Rect {
    Rect {
        origin: crate::layout::Point { x: rect.origin.x * scale, y: rect.origin.y * scale },
        size: crate::layout::Size {
            width: rect.size.width * scale,
            height: rect.size.height * scale,
        },
    }
}

/// Pinta a lista na ordem — quem vem depois pinta por cima. Texto sai da
/// fonte pixel da casa (o engine default).
pub fn rasterize(display: &DisplayList, width: usize, height: usize, background: Color) -> Bitmap {
    rasterize_scaled(display, width, height, 1, background)
}

/// Como [`rasterize`], mas com `width`/`height` em pixels FÍSICOS e as
/// coordenadas lógicas da display list multiplicadas por `scale` — o
/// caminho retina (o backend consulta o scale factor da janela).
pub fn rasterize_scaled(
    display: &DisplayList,
    width: usize,
    height: usize,
    scale: usize,
    background: Color,
) -> Bitmap {
    rasterize_with(display, width, height, scale, background, &PixelFont)
}

/// O caminho completo: pinta a lista com o [`TextEngine`] do frame — é o
/// que o `Runtime` chama (PixelFont no headless, CoreText no Mac).
pub fn rasterize_with(
    display: &DisplayList,
    width: usize,
    height: usize,
    scale: usize,
    background: Color,
    text: &dyn TextEngine,
) -> Bitmap {
    let mut bitmap = Bitmap::new(width, height, background);
    let factor = scale as f64;
    for command in display.iter() {
        match command {
            DrawCommand::FillRect { rect, color, corner_radius } => {
                bitmap.fill_rect(scale_rect(*rect, factor), *color, corner_radius * factor)
            }
            DrawCommand::StrokeRect { rect, color, width } => {
                bitmap.stroke_rect(scale_rect(*rect, factor), *color, width * factor)
            }
            DrawCommand::TextLine { origin, content, range, color, font } => {
                let slice = &content[range.0..range.1];
                if let Some(raster) = text.raster_line(slice, font, *color, scale) {
                    bitmap.composite_text(origin.x, origin.y, scale, &raster);
                }
            }
            DrawCommand::PushClip { rect } => bitmap.push_clip(scale_rect(*rect, factor)),
            DrawCommand::PopClip => bitmap.pop_clip(),
        }
    }
    bitmap
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{DrawCommand, Point};

    /// Desenha o retrato ascii de um trecho do bitmap — golden legível.
    fn portrait(bitmap: &Bitmap, x: usize, y: usize, w: usize, h: usize, ink: u32) -> String {
        let mut out = String::new();
        for row in y..y + h {
            for col in x..x + w {
                out.push(if bitmap.pixel(col, row) == Some(ink) { '#' } else { '.' });
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn the_glyph_cell_respects_the_layout_metrics() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::TextLine {
            origin: Point { x: 0.0, y: 0.0 },
            content: std::rc::Rc::from("1"),
            range: (0, 1),
            color: Color::BLACK,
            font: crate::text_engine::FontSpec::DEFAULT,
        });
        let bitmap = rasterize(&display, 8, 16, Color::WHITE);

        let ink = super::pack(Color::BLACK);
        let picture = portrait(&bitmap, 0, 0, 8, 16, ink);
        // folga vertical de 3px no topo, glifo de 10px, resto limpo
        assert!(picture.lines().take(3).all(|line| !line.contains('#')));
        assert!(picture.lines().nth(13).is_some_and(|line| !line.contains('#')));
        assert!(picture.contains('#'), "o corpo do glifo tem tinta:\n{picture}");
    }

    #[test]
    fn fill_and_stroke_land_on_snapped_edges() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 2.0, y: 2.0 },
                size: crate::layout::Size { width: 4.0, height: 4.0 },
            },
            color: Color::FILL,
            corner_radius: 0.0,
        });
        let bitmap = rasterize(&display, 8, 8, Color::WHITE);

        let fill = super::pack(Color::FILL);
        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(2, 2), Some(fill));
        assert_eq!(bitmap.pixel(5, 5), Some(fill), "aresta [2,6): o 5 é o último dentro");
        assert_eq!(bitmap.pixel(6, 6), Some(white), "o 6 já é fora");
        assert_eq!(bitmap.pixel(1, 1), Some(white));
    }

    #[test]
    fn source_over_blends_the_veil_exactly() {
        // véu de azul a 128/255 sobre branco: cada canal sai do div255,
        // valor inteiro exato — nada de "quase"
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size { width: 1.0, height: 1.0 },
            },
            color: Color { r: 0, g: 0, b: 255, a: 128 },
            corner_radius: 0.0,
        });
        let bitmap = rasterize(&display, 1, 1, Color::WHITE);

        // r = g = round(255·127/255) = 127; b = 255; a = 128 + 127 = 255
        assert_eq!(bitmap.pixel(0, 0), Some(0x7F7F_FFFF));
    }

    #[test]
    fn zero_alpha_is_a_no_op_and_full_alpha_overwrites() {
        let rect = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: crate::layout::Size { width: 1.0, height: 1.0 },
        };
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect,
            color: Color { r: 9, g: 9, b: 9, a: 0 },
            corner_radius: 0.0,
        });
        display.push(DrawCommand::FillRect {
            rect,
            color: Color { r: 1, g: 2, b: 3, a: 255 },
            corner_radius: 0.0,
        });
        let bitmap = rasterize(&display, 1, 1, Color::WHITE);

        assert_eq!(bitmap.pixel(0, 0), Some(super::pack(Color { r: 1, g: 2, b: 3, a: 255 })));
    }

    #[test]
    fn corner_radius_insets_the_scanlines() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size { width: 8.0, height: 8.0 },
            },
            color: Color::BLACK,
            corner_radius: 3.0,
        });
        let bitmap = rasterize(&display, 8, 8, Color::WHITE);

        let ink = super::pack(Color::BLACK);
        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(0, 0), Some(white), "canto recuado");
        assert_eq!(bitmap.pixel(7, 7), Some(white), "canto oposto recuado");
        assert_eq!(bitmap.pixel(4, 0), Some(ink), "meio da aresta superior pintado");
        assert_eq!(bitmap.pixel(0, 4), Some(ink), "meio da aresta esquerda pintado");
        assert_eq!(bitmap.pixel(4, 4), Some(ink), "centro pintado");
    }

    #[test]
    fn stroke_width_never_double_blends_the_corners() {
        // moldura translúcida: canto e aresta têm o MESMO valor — cada
        // pixel da borda blendou exatamente uma vez
        let mut display = DisplayList::default();
        display.push(DrawCommand::StrokeRect {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size { width: 6.0, height: 6.0 },
            },
            color: Color { r: 0, g: 0, b: 0, a: 128 },
            width: 2.0,
        });
        let bitmap = rasterize(&display, 6, 6, Color::WHITE);

        assert_eq!(bitmap.pixel(0, 0), bitmap.pixel(3, 0), "canto == topo");
        assert_eq!(bitmap.pixel(0, 3), bitmap.pixel(3, 0), "lateral == topo");
        assert_eq!(bitmap.pixel(3, 3), Some(super::pack(Color::WHITE)), "interior intocado");
    }
}
