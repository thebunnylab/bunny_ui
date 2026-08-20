//! The linux text engine: fontconfig finds the face, FreeType rasters
//! it, HarfBuzz shapes the line. All three are the platform's own
//! libraries, bound by hand — no bindings crate.
//!
//! The border discipline: HarfBuzz and fontconfig are driven through
//! API calls only (their objects stay opaque); FreeType's contract IS
//! its structs, so the handful of fields the engine reads go through
//! offsets extracted from the installed headers with `offsetof` — the
//! vtable-verification habit translated (the extraction record lives
//! in the private notes).
//!
//! Rasters come out STRAIGHT alpha in physical pixels, the trait's
//! law. The gray road is straight by construction (coverage × color);
//! only color-emoji bitmaps arrive premultiplied and get undone.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_void};

use bunny_ui::layout::Color;
use bunny_ui::text_engine::{FontDesign, FontSpec, LineMetrics, Slant, TextEngine, TextRaster, Weight};

// MARK: - fontconfig ABI (API-only; patterns are opaque)

const FC_RESULT_MATCH: c_int = 0;
const FC_MATCH_PATTERN: c_int = 0;

#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcInitLoadConfigAndFonts() -> *mut c_void;
    fn FcPatternCreate() -> *mut c_void;
    fn FcPatternAddString(p: *mut c_void, object: *const c_char, s: *const u8) -> c_int;
    fn FcPatternAddInteger(p: *mut c_void, object: *const c_char, i: c_int) -> c_int;
    fn FcPatternAddCharSet(p: *mut c_void, object: *const c_char, cs: *mut c_void) -> c_int;
    fn FcConfigSubstitute(cfg: *mut c_void, p: *mut c_void, kind: c_int) -> c_int;
    fn FcDefaultSubstitute(p: *mut c_void);
    fn FcFontMatch(cfg: *mut c_void, p: *mut c_void, result: *mut c_int) -> *mut c_void;
    fn FcPatternGetString(
        p: *mut c_void,
        object: *const c_char,
        n: c_int,
        out: *mut *mut u8,
    ) -> c_int;
    fn FcPatternGetInteger(p: *mut c_void, object: *const c_char, n: c_int, out: *mut c_int)
    -> c_int;
    fn FcPatternDestroy(p: *mut c_void);
    fn FcCharSetCreate() -> *mut c_void;
    fn FcCharSetAddChar(cs: *mut c_void, ucs4: u32) -> c_int;
    fn FcCharSetDestroy(cs: *mut c_void);
    /// The roster road: an object set names WHICH properties the
    /// listing carries, and the font set that comes back is the
    /// caller's to destroy.
    fn FcObjectSetCreate() -> *mut c_void;
    fn FcObjectSetAdd(os: *mut c_void, object: *const c_char) -> c_int;
    fn FcObjectSetDestroy(os: *mut c_void);
    fn FcFontList(cfg: *mut c_void, p: *mut c_void, os: *mut c_void) -> *mut FcFontSet;
    fn FcFontSetDestroy(set: *mut FcFontSet);
}

/// fontconfig's own listing shape — the two counts and the array.
#[repr(C)]
struct FcFontSet {
    nfont: c_int,
    sfont: c_int,
    fonts: *mut *mut c_void,
}

// MARK: - FreeType ABI (API calls + offset reads verified by extraction)

const FT_LOAD_RENDER: i32 = 0x4;
const FT_LOAD_COLOR: i32 = 1 << 20;
const PIXEL_MODE_GRAY: u8 = 2;
const PIXEL_MODE_BGRA: u8 = 7;

// FT_FaceRec offsets (x86_64, headers 2026-08-18)
const FACE_GLYPH: usize = 152;
const FACE_SIZE: usize = 160;
// FT_SizeRec.metrics at 24; FT_Size_Metrics fields (26.6 FT_Pos)
const SIZE_METRICS: usize = 24;
const METRICS_ASCENDER: usize = 24;
const METRICS_DESCENDER: usize = 32;
const METRICS_HEIGHT: usize = 40;
// FT_GlyphSlotRec offsets (advances come from HarfBuzz, not the slot)
const SLOT_BITMAP: usize = 152;
const SLOT_BITMAP_LEFT: usize = 192;
const SLOT_BITMAP_TOP: usize = 196;
// FT_Bitmap offsets (relative to SLOT_BITMAP)
const BITMAP_ROWS: usize = 0;
const BITMAP_WIDTH: usize = 4;
const BITMAP_PITCH: usize = 8;
const BITMAP_BUFFER: usize = 16;
const BITMAP_PIXEL_MODE: usize = 26;

#[link(name = "freetype")]
unsafe extern "C" {
    fn FT_Init_FreeType(library: *mut *mut c_void) -> c_int;
    fn FT_New_Face(
        library: *mut c_void,
        path: *const c_char,
        index: c_long,
        face: *mut *mut c_void,
    ) -> c_int;
    fn FT_Set_Char_Size(
        face: *mut c_void,
        width: i64,
        height: i64,
        h_res: u32,
        v_res: u32,
    ) -> c_int;
    fn FT_Select_Size(face: *mut c_void, strike: c_int) -> c_int;
    fn FT_Load_Glyph(face: *mut c_void, glyph: u32, flags: i32) -> c_int;
    fn FT_Get_Char_Index(face: *mut c_void, code: u64) -> u32;
}

unsafe fn read<T: Copy>(base: *mut c_void, offset: usize) -> T {
    unsafe { *(base.cast::<u8>().add(offset).cast::<T>()) }
}

// MARK: - HarfBuzz ABI (API-only; hb-ft lives inside libharfbuzz)

#[repr(C)]
#[derive(Clone, Copy)]
struct HbGlyphInfo {
    codepoint: u32,
    mask: u32,
    cluster: u32,
    var1: u32,
    var2: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HbGlyphPos {
    x_advance: i32,
    y_advance: i32,
    x_offset: i32,
    y_offset: i32,
    var: u32,
}

#[link(name = "harfbuzz")]
unsafe extern "C" {
    fn hb_ft_font_create_referenced(face: *mut c_void) -> *mut c_void;
    fn hb_buffer_create() -> *mut c_void;
    fn hb_buffer_add_utf8(
        buffer: *mut c_void,
        text: *const c_char,
        length: c_int,
        item_offset: c_uint,
        item_length: c_int,
    );
    fn hb_buffer_guess_segment_properties(buffer: *mut c_void);
    fn hb_shape(font: *mut c_void, buffer: *mut c_void, features: *const c_void, count: c_uint);
    fn hb_buffer_get_glyph_infos(buffer: *mut c_void, length: *mut c_uint) -> *mut HbGlyphInfo;
    fn hb_buffer_get_glyph_positions(buffer: *mut c_void, length: *mut c_uint) -> *mut HbGlyphPos;
    fn hb_buffer_destroy(buffer: *mut c_void);
}

// MARK: - the engine

/// One resolved, sized face: the FreeType handle and its HarfBuzz twin
/// (created AFTER the size is set — hb-ft reads the face's current
/// size).
#[derive(Clone, Copy)]
struct Face {
    ft: *mut c_void,
    hb: *mut c_void,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FaceKey {
    mono: bool,
    weight: u8,
    italic: bool,
    /// The family the app named. A number, so the key stays cheap —
    /// the name itself is only needed once, when the face is opened.
    family: bunny_ui::text_engine::Family,
    /// f64 bits of the LOGICAL size — fractional sizes are contract.
    size_bits: u64,
    scale: usize,
}

/// The text engine backed by the platform stack. Face handles live for
/// the engine's life (the process, in practice) — fonts do not churn.
pub struct FreeTypeEngine {
    library: *mut c_void,
    config: *mut c_void,
    faces: RefCell<HashMap<FaceKey, Option<Face>>>,
    fallbacks: RefCell<HashMap<(FaceKey, u32), Option<Face>>>,
}

impl FreeTypeEngine {
    pub fn new() -> FreeTypeEngine {
        let mut library = std::ptr::null_mut();
        let ok = unsafe { FT_Init_FreeType(&mut library) } == 0;
        let config = unsafe { FcInitLoadConfigAndFonts() };
        if !ok || config.is_null() {
            eprintln!("bunny_ui_linux: the text stack failed to open");
        }
        FreeTypeEngine {
            library,
            config,
            faces: RefCell::new(HashMap::new()),
            fallbacks: RefCell::new(HashMap::new()),
        }
    }

    fn key(font: &FontSpec, scale: usize) -> FaceKey {
        FaceKey {
            mono: font.design == FontDesign::Mono,
            weight: match font.weight {
                Weight::Regular => 0,
                Weight::Medium => 1,
                Weight::Semibold => 2,
                Weight::Bold => 3,
                Weight::ExtraBold => 4,
                Weight::Black => 5,
            },
            italic: font.slant == Slant::Italic,
            family: font.family,
            size_bits: font.size.to_bits(),
            scale,
        }
    }

    /// fontconfig match → FreeType face at `size × scale` pixels →
    /// HarfBuzz font. `charset` narrows the match to faces covering a
    /// codepoint (the fallback road).
    fn open_face(&self, key: &FaceKey, charset: Option<u32>) -> Option<Face> {
        if self.config.is_null() || self.library.is_null() {
            return None;
        }
        let (path, index) = unsafe {
            let pattern = FcPatternCreate();
            // a family the app NAMED is the most specific thing anyone
            // said about this text; fontconfig substitutes its own way
            // out of a name this machine has not got, so an unknown one
            // degrades instead of failing
            let named = key.family.name().and_then(|name| CString::new(&*name).ok());
            let family: &CStr = match &named {
                Some(name) => name,
                None if key.mono => c"monospace",
                None => c"sans-serif",
            };
            FcPatternAddString(pattern, c"family".as_ptr(), family.as_ptr().cast());
            // the fc scale: regular 80, medium 100, demibold 180, bold 200
            let weight = [80, 100, 180, 200][key.weight as usize];
            FcPatternAddInteger(pattern, c"weight".as_ptr(), weight);
            FcPatternAddInteger(pattern, c"slant".as_ptr(), if key.italic { 100 } else { 0 });
            let charset_handle = charset.map(|code| {
                let set = FcCharSetCreate();
                FcCharSetAddChar(set, code);
                FcPatternAddCharSet(pattern, c"charset".as_ptr(), set);
                set
            });
            FcConfigSubstitute(self.config, pattern, FC_MATCH_PATTERN);
            FcDefaultSubstitute(pattern);
            let mut result = 0;
            let matched = FcFontMatch(self.config, pattern, &mut result);
            let found = (!matched.is_null() && result == FC_RESULT_MATCH)
                .then(|| {
                    let mut file: *mut u8 = std::ptr::null_mut();
                    let mut index: c_int = 0;
                    let has_file =
                        FcPatternGetString(matched, c"file".as_ptr(), 0, &mut file)
                            == FC_RESULT_MATCH
                            && !file.is_null();
                    FcPatternGetInteger(matched, c"index".as_ptr(), 0, &mut index);
                    has_file.then(|| {
                        (CStr::from_ptr(file.cast()).to_string_lossy().into_owned(), index)
                    })
                })
                .flatten();
            if !matched.is_null() {
                FcPatternDestroy(matched);
            }
            FcPatternDestroy(pattern);
            if let Some(set) = charset_handle {
                FcCharSetDestroy(set);
            }
            found?
        };
        let path_c = CString::new(path).ok()?;
        unsafe {
            let mut face = std::ptr::null_mut();
            if FT_New_Face(self.library, path_c.as_ptr(), index as c_long, &mut face) != 0 {
                return None;
            }
            // points at 72dpi are pixels: the char size carries the
            // fractional logical px and the resolution carries scale
            let size = f64::from_bits(key.size_bits);
            let size_26_6 = (size * 64.0).round() as i64;
            if FT_Set_Char_Size(face, 0, size_26_6, 72 * key.scale as u32, 72 * key.scale as u32)
                != 0
            {
                // bitmap-only faces (color emoji) carry fixed strikes
                if FT_Select_Size(face, 0) != 0 {
                    return None;
                }
            }
            let hb = hb_ft_font_create_referenced(face);
            if hb.is_null() {
                return None;
            }
            Some(Face { ft: face, hb })
        }
    }

    fn face(&self, key: &FaceKey) -> Option<Face> {
        if let Some(cached) = self.faces.borrow().get(key) {
            return *cached;
        }
        let opened = self.open_face(key, None);
        self.faces.borrow_mut().insert(key.clone(), opened);
        opened
    }

    fn fallback(&self, key: &FaceKey, code: u32) -> Option<Face> {
        let cache_key = (key.clone(), code);
        if let Some(cached) = self.fallbacks.borrow().get(&cache_key) {
            return *cached;
        }
        let opened = self.open_face(key, Some(code));
        self.fallbacks.borrow_mut().insert(cache_key, opened);
        opened
    }

    /// Shapes the whole line. The text splits into maximal runs by
    /// "does the primary face cover this char" (charmap probe); the
    /// uncovered runs shape with a fontconfig-charset fallback face —
    /// basic per-run fallback, the documented v1 scope.
    fn shape(&self, text: &str, key: &FaceKey) -> Option<(Vec<Shaped>, i64)> {
        let primary = self.face(key)?;
        let mut runs: Vec<(Face, std::ops::Range<usize>)> = Vec::new();
        let mut run_start = 0;
        let mut run_covered = true;
        for (at, ch) in text.char_indices() {
            let covered = ch == ' '
                || unsafe { FT_Get_Char_Index(primary.ft, ch as u64) } != 0;
            if at == 0 {
                run_covered = covered;
            } else if covered != run_covered {
                runs.push((
                    if run_covered {
                        primary
                    } else {
                        self.fallback_for(text, run_start, key, primary)
                    },
                    run_start..at,
                ));
                run_start = at;
                run_covered = covered;
            }
        }
        if !text.is_empty() {
            runs.push((
                if run_covered {
                    primary
                } else {
                    self.fallback_for(text, run_start, key, primary)
                },
                run_start..text.len(),
            ));
        }
        let mut shaped = Vec::new();
        let mut pen: i64 = 0;
        for (face, range) in runs {
            unsafe {
                let buffer = hb_buffer_create();
                hb_buffer_add_utf8(
                    buffer,
                    text.as_ptr().cast(),
                    text.len() as c_int,
                    range.start as c_uint,
                    (range.end - range.start) as c_int,
                );
                hb_buffer_guess_segment_properties(buffer);
                hb_shape(face.hb, buffer, std::ptr::null(), 0);
                let mut count = 0;
                let infos = hb_buffer_get_glyph_infos(buffer, &mut count);
                let mut pos_count = 0;
                let positions = hb_buffer_get_glyph_positions(buffer, &mut pos_count);
                for i in 0..count.min(pos_count) as usize {
                    let info = *infos.add(i);
                    let position = *positions.add(i);
                    shaped.push(Shaped {
                        face,
                        glyph: info.codepoint,
                        x: pen + position.x_offset as i64,
                        y: position.y_offset as i64,
                    });
                    pen += position.x_advance as i64;
                }
                hb_buffer_destroy(buffer);
            }
        }
        Some((shaped, pen))
    }

    fn fallback_for(&self, text: &str, at: usize, key: &FaceKey, primary: Face) -> Face {
        text[at..]
            .chars()
            .next()
            .and_then(|ch| self.fallback(key, ch as u32))
            .unwrap_or(primary)
    }

    /// ascent/descent from the sized face, the leading folded into the
    /// descent (the house line-box contract, all three platforms).
    fn line_box(&self, face: Face) -> (f64, f64) {
        unsafe {
            let size: *mut c_void = read(face.ft, FACE_SIZE);
            if size.is_null() {
                return (0.0, 0.0);
            }
            let metrics = size.cast::<u8>().add(SIZE_METRICS).cast::<c_void>();
            let ascent = read::<i64>(metrics, METRICS_ASCENDER) as f64 / 64.0;
            let descent = -(read::<i64>(metrics, METRICS_DESCENDER) as f64) / 64.0;
            let height = read::<i64>(metrics, METRICS_HEIGHT) as f64 / 64.0;
            let gap = (height - (ascent + descent)).max(0.0);
            (ascent, descent + gap)
        }
    }
}

#[derive(Clone, Copy)]
struct Shaped {
    face: Face,
    glyph: u32,
    /// 26.6 pen position (x includes the run base and the offset).
    x: i64,
    y: i64,
}

impl TextEngine for FreeTypeEngine {
    fn families(&self) -> Vec<std::sync::Arc<str>> {
        if self.config.is_null() {
            return Vec::new();
        }
        unsafe {
            let pattern = FcPatternCreate();
            let objects = FcObjectSetCreate();
            FcObjectSetAdd(objects, c"family".as_ptr());
            let listed = FcFontList(self.config, pattern, objects);
            let mut names: Vec<std::sync::Arc<str>> = Vec::new();
            if !listed.is_null() {
                for index in 0..(*listed).nfont.max(0) {
                    let face = *(*listed).fonts.offset(index as isize);
                    let mut value: *mut u8 = std::ptr::null_mut();
                    if FcPatternGetString(face, c"family".as_ptr(), 0, &mut value)
                        == FC_RESULT_MATCH
                        && !value.is_null()
                    {
                        let name = CStr::from_ptr(value.cast()).to_string_lossy();
                        names.push(std::sync::Arc::from(&*name));
                    }
                }
                FcFontSetDestroy(listed);
            }
            FcObjectSetDestroy(objects);
            FcPatternDestroy(pattern);
            // one face per FILE comes back, so a family with four
            // weights is listed four times
            names.sort();
            names.dedup();
            names
        }
    }

    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let key = Self::key(font, 1);
        let Some(primary) = self.face(&key) else {
            return LineMetrics { width: 0.0, ascent: 0.0, descent: 0.0 };
        };
        let (ascent, descent) = self.line_box(primary);
        // width includes trailing whitespace — the caret_from_x law
        let width = self
            .shape(text, &key)
            .map(|(_, advance)| advance as f64 / 64.0)
            .unwrap_or(0.0);
        LineMetrics { width, ascent, descent }
    }

    fn raster_line(
        &self,
        text: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        let scale = scale.max(1);
        let key = Self::key(font, scale);
        let primary = self.face(&key)?;
        let (ascent, descent) = self.line_box(primary);
        let (shaped, advance) = self.shape(text, &key)?;
        let width = (advance as f64 / 64.0).ceil() as usize;
        let baseline = ascent.ceil() as usize;
        let height = baseline + descent.ceil() as usize;
        if width == 0 || height == 0 {
            return None;
        }
        let mut rgba = vec![0u8; width * height * 4];
        for entry in &shaped {
            unsafe {
                if FT_Load_Glyph(entry.face.ft, entry.glyph, FT_LOAD_RENDER | FT_LOAD_COLOR) != 0 {
                    continue;
                }
                let slot: *mut c_void = read(entry.face.ft, FACE_GLYPH);
                if slot.is_null() {
                    continue;
                }
                let bitmap = slot.cast::<u8>().add(SLOT_BITMAP).cast::<c_void>();
                let rows = read::<u32>(bitmap, BITMAP_ROWS) as usize;
                let cols = read::<u32>(bitmap, BITMAP_WIDTH) as usize;
                let pitch = read::<i32>(bitmap, BITMAP_PITCH);
                let buffer: *const u8 = read(bitmap, BITMAP_BUFFER);
                let mode = read::<u8>(bitmap, BITMAP_PIXEL_MODE);
                if buffer.is_null() || rows == 0 || cols == 0 {
                    continue;
                }
                let left = read::<i32>(slot, SLOT_BITMAP_LEFT) as i64;
                let top = read::<i32>(slot, SLOT_BITMAP_TOP) as i64;
                let pen_x = (entry.x as f64 / 64.0).round() as i64 + left;
                let pen_y = baseline as i64 - top - (entry.y as f64 / 64.0).round() as i64;
                for row in 0..rows {
                    let target_y = pen_y + row as i64;
                    if target_y < 0 || target_y >= height as i64 {
                        continue;
                    }
                    let source_row = if pitch >= 0 {
                        buffer.add(row * pitch as usize)
                    } else {
                        buffer.add((rows - 1 - row) * (-pitch) as usize)
                    };
                    for col in 0..cols {
                        let target_x = pen_x + col as i64;
                        if target_x < 0 || target_x >= width as i64 {
                            continue;
                        }
                        let at = (target_y as usize * width + target_x as usize) * 4;
                        let px = &mut rgba[at..at + 4];
                        match mode {
                            PIXEL_MODE_GRAY => {
                                let coverage = *source_row.add(col) as u32;
                                let alpha = ((coverage * color.a as u32 + 127) / 255) as u8;
                                if alpha > px[3] {
                                    px[0] = color.r;
                                    px[1] = color.g;
                                    px[2] = color.b;
                                    px[3] = alpha;
                                }
                            }
                            PIXEL_MODE_BGRA => {
                                // color emoji keep their own colors;
                                // the bytes arrive premultiplied
                                let b = *source_row.add(col * 4) as u32;
                                let g = *source_row.add(col * 4 + 1) as u32;
                                let r = *source_row.add(col * 4 + 2) as u32;
                                let a = *source_row.add(col * 4 + 3) as u32;
                                if a as u8 > px[3] {
                                    let un = |c: u32| {
                                        ((c * 255 + a / 2) / a.max(1)).min(255) as u8
                                    };
                                    px[0] = un(r);
                                    px[1] = un(g);
                                    px[2] = un(b);
                                    px[3] = a as u8;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Some(TextRaster { width, height, baseline, rgba })
    }
}

// MARK: - tests

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> FreeTypeEngine {
        FreeTypeEngine::new()
    }

    fn spec() -> FontSpec {
        FontSpec::DEFAULT
    }

    fn black() -> Color {
        Color { r: 10, g: 20, b: 30, a: 255 }
    }

    fn ink(raster: &TextRaster) -> usize {
        raster.rgba.chunks_exact(4).filter(|px| px[3] > 0).count()
    }

    #[test]
    fn the_raster_is_deterministic() {
        let engine = engine();
        let a = engine.raster_line("Determinism 123", &spec(), black(), 1).unwrap();
        let b = engine.raster_line("Determinism 123", &spec(), black(), 1).unwrap();
        assert_eq!(a.width, b.width);
        assert_eq!(a.rgba, b.rgba, "same input, byte-identical raster");
    }

    #[test]
    fn the_alpha_stays_straight() {
        let engine = engine();
        let raster = engine.raster_line("l", &spec(), black(), 1).unwrap();
        let edge = raster
            .rgba
            .chunks_exact(4)
            .find(|px| px[3] > 0 && px[3] < 255)
            .expect("an anti-aliased edge exists");
        assert_eq!(
            (edge[0], edge[1], edge[2]),
            (10, 20, 30),
            "straight alpha: the RGB stays the text color at every coverage"
        );
    }

    #[test]
    fn metrics_make_sense_and_empty_paints_nothing() {
        let engine = engine();
        let hello = engine.measure_line("hello", &spec());
        let h = engine.measure_line("h", &spec());
        assert!(hello.ascent > 0.0 && hello.descent > 0.0);
        assert!(hello.width > h.width && h.width > 0.0);
        let spaces = engine.measure_line("h ", &spec());
        assert!(spaces.width > h.width, "trailing whitespace counts — the caret law");
        let empty = engine.measure_line("", &spec());
        assert_eq!(empty.width, 0.0);
        assert!(empty.ascent > 0.0, "an empty line still has a line box");
        assert!(engine.raster_line("", &spec(), black(), 1).is_none());
    }

    #[test]
    fn scale_two_doubles_the_raster() {
        let engine = engine();
        let one = engine.raster_line("Scale", &spec(), black(), 1).unwrap();
        let two = engine.raster_line("Scale", &spec(), black(), 2).unwrap();
        let near = |a: usize, b: usize| (a as i64 - b as i64).abs() <= 2;
        assert!(near(two.width, one.width * 2), "{} vs {}", two.width, one.width);
        assert!(near(two.height, one.height * 2), "{} vs {}", two.height, one.height);
    }

    #[test]
    fn a_surrogate_emoji_does_not_panic() {
        let engine = engine();
        let metrics = engine.measure_line("a🐰b", &spec());
        assert!(metrics.width > 0.0);
        let _ = engine.raster_line("a🐰b", &spec(), black(), 1);
    }

    #[test]
    fn weights_tell_apart() {
        let engine = engine();
        let regular = engine.raster_line("Weight", &spec(), black(), 1).unwrap();
        let bold_spec = FontSpec { weight: Weight::Bold, ..spec() };
        let bold = engine.raster_line("Weight", &bold_spec, black(), 1).unwrap();
        assert!(
            regular.width != bold.width || regular.rgba != bold.rgba,
            "bold and regular are different faces"
        );
    }

    #[test]
    fn the_fallback_finds_a_face_with_ink() {
        // DejaVu has no emoji glyphs; the charset fallback walks to a
        // face that does (Noto Color Emoji on the QA box)
        let engine = engine();
        let raster = engine.raster_line("🙂", &spec(), black(), 1).expect("emoji rasters");
        assert!(ink(&raster) > 0, "the fallback face left ink");
    }
}
