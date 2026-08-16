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
//! A fonte é nossa: 3×5 pixels por glifo, desenhada à mão, desenhada ×2
//! dentro da célula de 8×16 das métricas do layout. Legível o suficiente
//! para o counter provar o pipeline; o sistema de texto real a substitui
//! sem tocar na display list.

use crate::layout::{Color, DisplayList, DrawCommand, Rect};

/// Um buffer RGBA (um `u32` `0xRRGGBBAA` por pixel, linhas de cima para
/// baixo) — o que o backend de plataforma blita na janela.
pub struct Bitmap {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}

fn pack(color: Color) -> u32 {
    ((color.r as u32) << 24) | ((color.g as u32) << 16) | ((color.b as u32) << 8) | color.a as u32
}

impl Bitmap {
    pub fn new(width: usize, height: usize, background: Color) -> Self {
        Bitmap { width, height, pixels: vec![pack(background); width * height] }
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
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.pixels[y as usize * self.width + x as usize] = color;
        }
    }

    /// Arestas arredondadas em device px — o ponto único de snapping.
    fn snap(rect: Rect) -> (i64, i64, i64, i64) {
        let x0 = rect.origin.x.round() as i64;
        let y0 = rect.origin.y.round() as i64;
        let x1 = (rect.origin.x + rect.size.width).round() as i64;
        let y1 = (rect.origin.y + rect.size.height).round() as i64;
        (x0, y0, x1, y1)
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        let packed = pack(color);
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x, y, packed);
            }
        }
    }

    fn stroke_rect(&mut self, rect: Rect, color: Color) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        let packed = pack(color);
        for x in x0..x1 {
            self.set(x, y0, packed);
            self.set(x, y1 - 1, packed);
        }
        for y in y0..y1 {
            self.set(x0, y, packed);
            self.set(x1 - 1, y, packed);
        }
    }

    /// Uma linha de texto na célula das métricas do layout: avanço de 8px
    /// lógicos, glifo 3×5 desenhado ×(2·scale) com folga de (1, 3) lógicos
    /// na célula 8×16.
    fn draw_text_line(
        &mut self,
        origin_x: f64,
        origin_y: f64,
        scale: i64,
        content: &str,
        color: Color,
    ) {
        let packed = pack(color);
        let base_x = (origin_x * scale as f64).round() as i64;
        let base_y = (origin_y * scale as f64).round() as i64;
        let block = 2 * scale;
        for (index, ch) in content.chars().enumerate() {
            let Some(rows) = glyph(ch) else { continue };
            let cell_x = base_x + (index as i64 * 8 + 1) * scale;
            let cell_y = base_y + 3 * scale;
            for row in 0..5i64 {
                for col in 0..3i64 {
                    let bit = 14 - (row * 3 + col);
                    if rows >> bit & 1 == 1 {
                        for dy in 0..block {
                            for dx in 0..block {
                                self.set(
                                    cell_x + col * block + dx,
                                    cell_y + row * block + dy,
                                    packed,
                                );
                            }
                        }
                    }
                }
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

/// Pinta a lista na ordem — quem vem depois pinta por cima.
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
    let mut bitmap = Bitmap::new(width, height, background);
    let factor = scale as f64;
    for command in display.iter() {
        match command {
            DrawCommand::FillRect { rect, color } => {
                bitmap.fill_rect(scale_rect(*rect, factor), *color)
            }
            DrawCommand::StrokeRect { rect, color } => {
                bitmap.stroke_rect(scale_rect(*rect, factor), *color)
            }
            DrawCommand::TextLine { origin, content, color } => {
                bitmap.draw_text_line(origin.x, origin.y, scale as i64, content, *color)
            }
        }
    }
    bitmap
}

/// A fonte da casa: 15 bits por glifo (3 colunas × 5 linhas, MSB = canto
/// superior esquerdo). Maiúsculas caem nas minúsculas; o que não existe
/// não pinta (a caixa vazia é honesta).
fn glyph(ch: char) -> Option<u16> {
    let rows = match ch.to_ascii_lowercase() {
        '0' => 0b111_101_101_101_111,
        '1' => 0b010_110_010_010_111,
        '2' => 0b111_001_111_100_111,
        '3' => 0b111_001_111_001_111,
        '4' => 0b101_101_111_001_001,
        '5' => 0b111_100_111_001_111,
        '6' => 0b111_100_111_101_111,
        '7' => 0b111_001_001_010_010,
        '8' => 0b111_101_111_101_111,
        '9' => 0b111_101_111_001_111,
        'a' => 0b010_101_111_101_101,
        'b' => 0b110_101_110_101_110,
        'c' => 0b011_100_100_100_011,
        'd' => 0b110_101_101_101_110,
        'e' => 0b111_100_110_100_111,
        'f' => 0b111_100_110_100_100,
        'g' => 0b011_100_101_101_011,
        'h' => 0b101_101_111_101_101,
        'i' => 0b111_010_010_010_111,
        'j' => 0b001_001_001_101_010,
        'k' => 0b101_110_100_110_101,
        'l' => 0b100_100_100_100_111,
        'm' => 0b101_111_111_101_101,
        'n' => 0b110_101_101_101_101,
        'o' => 0b010_101_101_101_010,
        'p' => 0b110_101_110_100_100,
        'q' => 0b011_101_101_011_001,
        'r' => 0b110_101_110_110_101,
        's' => 0b011_100_010_001_110,
        't' => 0b111_010_010_010_010,
        'u' => 0b101_101_101_101_111,
        'v' => 0b101_101_101_101_010,
        'w' => 0b101_101_111_111_101,
        'x' => 0b101_101_010_101_101,
        'y' => 0b101_101_010_010_010,
        'z' => 0b111_001_010_100_111,
        ':' => 0b000_010_000_010_000,
        '.' => 0b000_000_000_000_010,
        ',' => 0b000_000_000_010_100,
        '!' => 0b010_010_010_000_010,
        '-' => 0b000_000_111_000_000,
        '(' => 0b010_100_100_100_010,
        ')' => 0b010_001_001_001_010,
        ' ' => 0b000_000_000_000_000,
        _ => return None,
    };
    Some(rows)
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
            content: "1".to_string(),
            color: Color::BLACK,
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
        });
        let bitmap = rasterize(&display, 8, 8, Color::WHITE);

        let fill = super::pack(Color::FILL);
        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(2, 2), Some(fill));
        assert_eq!(bitmap.pixel(5, 5), Some(fill), "aresta [2,6): o 5 é o último dentro");
        assert_eq!(bitmap.pixel(6, 6), Some(white), "o 6 já é fora");
        assert_eq!(bitmap.pixel(1, 1), Some(white));
    }
}
