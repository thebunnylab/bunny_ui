//! A borda plugável de texto — medição e raster de UMA linha.
//!
//! O layout é sempre nosso, em todos os alvos; o que a plataforma empresta
//! é a MEDIÇÃO e o desenho dos glifos: a fonte pixel da casa no headless,
//! CoreText no Mac, `measureText`/DOM na web um dia. Nenhuma API de
//! componente sabe qual engine está ativo — [`TextEngine`] é a única
//! porta (uma borda declarada: `Rc<dyn TextEngine>` no `Runtime`).
//!
//! O [`MeasureCache`] é double-buffer por passada (prev/current): um hit
//! promove, a troca de frame descarta o que ninguém pediu — LRU de frame
//! exato, sem timer. Nota para o sistema de wrap real (shape separado de
//! quebra, cache em 2 níveis): a chave GANHA o modo da sondagem — cache de
//! linha envenenado por proposta é bug clássico.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::layout::{Color, Px};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FontDesign {
    /// A fonte de interface do sistema.
    Default,
    /// Monoespaçada — cidadã de primeira classe (grades de código).
    Mono,
}

/// Fonte resolvida — o que o layout carrega e o engine consome. `size` é
/// fracionário por contrato (10.5px é caso real de UI densa).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FontSpec {
    pub size: Px,
    pub weight: Weight,
    pub design: FontDesign,
}

impl FontSpec {
    pub const DEFAULT: FontSpec =
        FontSpec { size: 13.0, weight: Weight::Regular, design: FontDesign::Default };

    /// Os estilos de texto da API, em métricas de desktop.
    pub fn resolve(font: motor::views::Font) -> FontSpec {
        use motor::views::Font;
        let (size, weight) = match font {
            Font::LargeTitle => (26.0, Weight::Regular),
            Font::Title => (22.0, Weight::Regular),
            Font::Headline => (13.0, Weight::Semibold),
            Font::Subheadline => (11.0, Weight::Regular),
            Font::Body => (13.0, Weight::Regular),
            Font::Callout => (12.0, Weight::Regular),
            Font::Footnote => (10.0, Weight::Regular),
            Font::Caption => (10.0, Weight::Regular),
            Font::Caption2 => (10.0, Weight::Regular),
        };
        FontSpec { size, weight, design: FontDesign::Default }
    }

    /// A chave hasheável (f64 não é `Eq`): tamanho quantizado em milésimos
    /// de ponto — nenhum uso real distingue menos que isso.
    pub fn key(&self) -> FontKey {
        FontKey {
            size_milli: (self.size * 1000.0).round() as u32,
            weight: self.weight,
            design: self.design,
        }
    }
}

/// A identidade de cache/fonte de um [`FontSpec`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontKey {
    size_milli: u32,
    weight: Weight,
    design: FontDesign,
}

/// Patch parcial de fonte para a herança: `.font(…)` seta os três campos;
/// `.bold()` só o peso; `.monospaced()` só o design — cada um aplica por
/// cima do herdado, campo a campo.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct FontPatch {
    pub size: Option<Px>,
    pub weight: Option<Weight>,
    pub design: Option<FontDesign>,
}

impl FontPatch {
    pub fn full(spec: FontSpec) -> FontPatch {
        FontPatch { size: Some(spec.size), weight: Some(spec.weight), design: Some(spec.design) }
    }

    /// Merge dos modifiers empilhados — o definido (mais próximo) vence.
    pub fn or(self, outer: FontPatch) -> FontPatch {
        FontPatch {
            size: self.size.or(outer.size),
            weight: self.weight.or(outer.weight),
            design: self.design.or(outer.design),
        }
    }

    pub fn apply_over(&self, base: FontSpec) -> FontSpec {
        FontSpec {
            size: self.size.unwrap_or(base.size),
            weight: self.weight.unwrap_or(base.weight),
            design: self.design.unwrap_or(base.design),
        }
    }
}

/// Métricas de UMA linha. A altura de linha é DERIVADA — `ascent +
/// descent`; o engine dobra o leading dentro do descent (uma soma, um
/// contrato).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LineMetrics {
    pub width: Px,
    pub ascent: Px,
    pub descent: Px,
}

impl LineMetrics {
    pub fn height(&self) -> Px {
        self.ascent + self.descent
    }
}

/// Uma linha rasterizada: retângulo RGBA de alfa RETO (não pré-
/// multiplicado — o compositor da casa blenda reto, em todos os alvos),
/// origem no topo-esquerda da caixa de linha, já em pixels FÍSICOS.
/// `baseline` é a linha de base a partir do topo (informativo/testes — a
/// composição usa o topo-esquerda direto).
pub struct TextRaster {
    pub width: usize,
    pub height: usize,
    pub baseline: usize,
    pub rgba: Vec<u8>,
}

/// A borda: quem mede e desenha texto. Object-safe de propósito —
/// `Rc<dyn TextEngine>` é a forma que atravessa o `Runtime`.
pub trait TextEngine {
    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics;

    /// `None` = nada a pintar (string vazia, largura zero). `scale` é o
    /// fator retina — o raster sai em pixels físicos.
    fn raster_line(
        &self,
        text: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster>;
}

// MARK: - PixelFont, o engine default

/// A fonte da casa: 3×5 pixels por glifo, célula FIXA de 8×16 (ignora
/// size/weight/design de propósito) — métricas determinísticas que mantêm
/// os testes headless byte-estáveis. Maiúsculas caem nas minúsculas; o que
/// não existe não pinta (a caixa vazia é honesta).
pub struct PixelFont;

/// Célula da fonte pixel: baseline no pé do glifo (3 de folga no topo +
/// 10 de corpo), 13 acima + 3 abaixo = os 16 da célula.
const PIXEL_ASCENT: Px = 13.0;
const PIXEL_DESCENT: Px = 3.0;
const PIXEL_ADVANCE: Px = 8.0;

impl TextEngine for PixelFont {
    fn measure_line(&self, text: &str, _font: &FontSpec) -> LineMetrics {
        LineMetrics {
            width: text.chars().count() as Px * PIXEL_ADVANCE,
            ascent: PIXEL_ASCENT,
            descent: PIXEL_DESCENT,
        }
    }

    fn raster_line(
        &self,
        text: &str,
        _font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let width = chars.len() * 8 * scale;
        let height = 16 * scale;
        let mut rgba = vec![0u8; width * height * 4];
        let mut set = |x: usize, y: usize| {
            let index = (y * width + x) * 4;
            rgba[index] = color.r;
            rgba[index + 1] = color.g;
            rgba[index + 2] = color.b;
            rgba[index + 3] = color.a;
        };

        // os MESMOS offsets do rasterizador original: glifo ×(2·scale)
        // com folga de (1, 3) lógicos na célula 8×16
        let block = 2 * scale;
        for (index, ch) in chars.iter().enumerate() {
            let Some(rows) = glyph(*ch) else { continue };
            let cell_x = (index * 8 + 1) * scale;
            let cell_y = 3 * scale;
            for row in 0..5usize {
                for col in 0..3usize {
                    let bit = 14 - (row * 3 + col);
                    if rows >> bit & 1 == 1 {
                        for dy in 0..block {
                            for dx in 0..block {
                                set(cell_x + col * block + dx, cell_y + row * block + dy);
                            }
                        }
                    }
                }
            }
        }

        Some(TextRaster { width, height, baseline: 13 * scale, rgba })
    }
}

/// A fonte da casa: 15 bits por glifo (3 colunas × 5 linhas, MSB = canto
/// superior esquerdo).
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

// MARK: - Cache de medição

/// Double-buffer prev/current trocado por passada de layout: hit promove
/// para o current, a troca descarta o que ninguém pediu no frame — LRU de
/// frame exato, sem timer.
#[derive(Default)]
pub struct MeasureCache {
    prev: RefCell<HashMap<(String, FontKey), LineMetrics>>,
    current: RefCell<HashMap<(String, FontKey), LineMetrics>>,
}

impl MeasureCache {
    /// Início de uma passada de layout: o current vira prev.
    pub fn begin_frame(&self) {
        let current = std::mem::take(&mut *self.current.borrow_mut());
        *self.prev.borrow_mut() = current;
    }

    pub fn get_or_measure(
        &self,
        text: &str,
        font: &FontSpec,
        engine: &dyn TextEngine,
    ) -> LineMetrics {
        // (chave aloca por consulta — o cache de glyph-run futuro troca
        // isto por identidade de Rc; anotado, não agora)
        let key = (text.to_string(), font.key());
        if let Some(hit) = self.current.borrow().get(&key) {
            return *hit;
        }
        if let Some(hit) = self.prev.borrow_mut().remove(&key) {
            self.current.borrow_mut().insert(key, hit);
            return hit;
        }
        let measured = engine.measure_line(text, font);
        self.current.borrow_mut().insert(key, measured);
        measured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_font_metrics_match_the_layout_cell() {
        let metrics = PixelFont.measure_line("abc", &FontSpec::DEFAULT);
        assert_eq!(metrics.width, 24.0);
        assert_eq!(metrics.ascent, 13.0);
        assert_eq!(metrics.descent, 3.0);
        assert_eq!(metrics.height(), crate::layout::LINE_H);
    }

    #[test]
    fn empty_text_rasters_to_nothing() {
        assert!(PixelFont.raster_line("", &FontSpec::DEFAULT, Color::BLACK, 1).is_none());
    }

    #[test]
    fn measure_cache_double_buffers_between_frames() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct Counting(Rc<Cell<usize>>);
        impl TextEngine for Counting {
            fn measure_line(&self, text: &str, _font: &FontSpec) -> LineMetrics {
                self.0.set(self.0.get() + 1);
                LineMetrics { width: text.len() as Px, ascent: 1.0, descent: 0.0 }
            }
            fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
                None
            }
        }

        let calls = Rc::new(Cell::new(0));
        let engine = Counting(Rc::clone(&calls));
        let cache = MeasureCache::default();

        cache.begin_frame();
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 1, "hit dentro do frame");

        cache.begin_frame();
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 1, "o frame seguinte promove do prev — zero medições novas");

        cache.begin_frame();
        cache.begin_frame();
        cache.get_or_measure("hello", &FontSpec::DEFAULT, &engine);
        assert_eq!(calls.get(), 2, "dois frames sem uso descartam a entrada");
    }
}
