//! `android.graphics` through the house JNI — the Android text engine.
//!
//! Implements the bunny-ui [`TextEngine`] border: measuring by the FONT's
//! metrics (stable — the line's metrics jump when a glyph fallback kicks
//! in, and line height must not jump per string) and rastering one line
//! with a `Paint` on a `Canvas` over a `Bitmap` of our own size, whose
//! pixels are read back through `libjnigraphics` — no array copies
//! through Java.
//!
//! A bitmap only draws premultiplied; the bunny-ui compositor blends
//! STRAIGHT alpha (a single path for all engines), so the rectangle is
//! unpremultiplied in place before leaving — one pass over a small text
//! rectangle.
//!
//! Fonts: the `Default` design is the system's `sans-serif`, `Mono` its
//! `monospace`; a family the app named is asked for by name, and a name
//! the phone does not carry answers the default face — the platform
//! says nothing about it, so a bundled face goes through
//! [`AndroidTextEngine::register_font`], which reads the family's name
//! out of the file itself. Each `Paint` is created once per `FontKey`
//! and retained in the engine, as a global reference.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;

use bunny_ui::layout::Color;
use bunny_ui::text_engine::{
    FontDesign, FontKey, FontSpec, LineMetrics, Slant, TextEngine, TextRaster, Weight,
};

use crate::face::{family_name, fnv64};
use crate::jni::{boolean, float, int, object, Env, Frame, JObject};

/// `AndroidBitmapInfo`: the shape of a bitmap's pixels.
#[repr(C)]
struct AndroidBitmapInfo {
    width: u32,
    height: u32,
    /// In BYTES.
    stride: u32,
    format: i32,
    flags: u32,
}

const ANDROID_BITMAP_FORMAT_RGBA_8888: i32 = 1;

#[link(name = "jnigraphics")]
unsafe extern "C" {
    fn AndroidBitmap_getInfo(env: *mut c_void, bitmap: JObject, info: *mut AndroidBitmapInfo) -> i32;
    fn AndroidBitmap_lockPixels(env: *mut c_void, bitmap: JObject, pixels: *mut *mut c_void) -> i32;
    fn AndroidBitmap_unlockPixels(env: *mut c_void, bitmap: JObject) -> i32;
}

/// `Paint.ANTI_ALIAS_FLAG | LINEAR_TEXT_FLAG | SUBPIXEL_TEXT_FLAG`.
const PAINT_FLAGS: i32 = 0x01 | 0x40 | 0x80;

/// One face the engine holds: its paint (a global reference) and the
/// font's own metrics, in logical points.
struct Face {
    paint: JObject,
    ascent: f64,
    descent: f64,
}

/// The Android text engine. Single-thread, like the rest of the shell:
/// it holds the UI thread's env, and is asked on that thread only.
pub struct AndroidTextEngine {
    faces: RefCell<HashMap<FontKey, Face>>,
    /// The faces the app ships, by the family name their file declares
    /// (lowercased) — global references.
    registered: RefCell<HashMap<String, JObject>>,
}

impl AndroidTextEngine {
    pub fn new() -> Self {
        AndroidTextEngine { faces: RefCell::new(HashMap::new()), registered: RefCell::new(HashMap::new()) }
    }

    /// Add a face this app SHIPS to the ones it can shape.
    ///
    /// The platform reads a face from a file, so the bytes are written
    /// under the app's private files once (named by their hash) and read
    /// back; the family name comes from the file's own `name` table, and
    /// [`FontSpec::family`] naming it lands on this face from then on.
    ///
    /// Process-scoped: nothing outside this app sees the face. Returns
    /// whether the face was added; the answer is worth reading only at
    /// boot.
    pub fn register_font(&self, bytes: &'static [u8]) -> bool {
        let Some(family) = family_name(bytes) else {
            aerr!("register_font: no family name in the face");
            return false;
        };
        let Some(root) = crate::ffi::internal_data_path() else { return false };
        let dir = std::path::Path::new(&root).join("fonts");
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let path = dir.join(format!("{:016x}.ttf", fnv64(bytes)));
        if !path.exists() && std::fs::write(&path, bytes).is_err() {
            return false;
        }
        let Some(env) = Env::current() else { return false };
        let Some(_frame) = Frame::new(env, 8) else { return false };
        let typeface = (|| {
            let typeface_class = env.class(c"android/graphics/Typeface")?;
            let from_file = env.static_method(
                typeface_class,
                c"createFromFile",
                c"(Ljava/lang/String;)Landroid/graphics/Typeface;",
            )?;
            let path = env.string(&path.to_string_lossy())?;
            let typeface = env.call_static_object(typeface_class, from_file, &[object(path)])?;
            env.global(typeface)
        })();
        let Some(typeface) = typeface else { return false };
        // a spec that missed before this call cached the fallback it got
        self.drop_faces();
        if let Some(old) = self.registered.borrow_mut().insert(family.to_lowercase(), typeface) {
            env.delete_global(old);
        }
        true
    }

    fn drop_faces(&self) {
        if let Some(env) = Env::current() {
            for face in self.faces.borrow_mut().drain().map(|(_, face)| face) {
                env.delete_global(face.paint);
            }
        }
    }

    /// The typeface for a spec — a local reference, or `None` when Java
    /// refused (the caller keeps whatever it had).
    fn typeface(&self, env: Env, spec: &FontSpec) -> Option<JObject> {
        let typeface_class = env.class(c"android/graphics/Typeface")?;
        let bold = !matches!(spec.weight, Weight::Regular | Weight::Medium);
        let italic = spec.slant == Slant::Italic;
        // Typeface.NORMAL 0, BOLD 1, ITALIC 2, BOLD_ITALIC 3
        let style = (bold as i32) | ((italic as i32) << 1);
        let named = spec.family.name().and_then(|name| {
            self.registered.borrow().get(&name.to_lowercase()).copied()
        });
        let base = match named {
            Some(registered) => registered,
            None => {
                // a family the app NAMED is the most specific thing anyone
                // said about this text, so it comes before the design; a
                // name the phone does not carry answers the default face
                let family = match spec.family.name() {
                    Some(name) => name.to_string(),
                    None => match spec.design {
                        FontDesign::Default => "sans-serif".to_string(),
                        FontDesign::Mono => "monospace".to_string(),
                    },
                };
                let create = env.static_method(
                    typeface_class,
                    c"create",
                    c"(Ljava/lang/String;I)Landroid/graphics/Typeface;",
                )?;
                let family = env.string(&family)?;
                env.call_static_object(typeface_class, create, &[object(family), int(style)])?
            }
        };
        // the weight, finely: the platform picks the nearest face the
        // family has (API 28)
        let weight = match spec.weight {
            Weight::Regular => 400,
            Weight::Medium => 500,
            Weight::Semibold => 600,
            Weight::Bold => 700,
            Weight::ExtraBold => 800,
            Weight::Black => 900,
        };
        let refine = env.static_method(
            typeface_class,
            c"create",
            c"(Landroid/graphics/Typeface;IZ)Landroid/graphics/Typeface;",
        )?;
        env.call_static_object(typeface_class, refine, &[object(base), int(weight), boolean(italic)])
    }

    /// The paint for a spec, made once per key.
    fn face<T>(&self, spec: &FontSpec, read: impl FnOnce(&Face) -> T) -> Option<T> {
        let key = spec.key();
        if let Some(face) = self.faces.borrow().get(&key) {
            return Some(read(face));
        }
        let env = Env::current()?;
        let face = {
            let _frame = Frame::new(env, 16)?;
            let paint_class = env.class(c"android/graphics/Paint")?;
            let paint = env.new_object(paint_class, env.method(paint_class, c"<init>", c"(I)V")?, &[int(PAINT_FLAGS)])?;
            if let Some(typeface) = self.typeface(env, spec) {
                let set_typeface = env.method(
                    paint_class,
                    c"setTypeface",
                    c"(Landroid/graphics/Typeface;)Landroid/graphics/Typeface;",
                )?;
                env.call_object(paint, set_typeface, &[object(typeface)]);
            }
            let set_size = env.method(paint_class, c"setTextSize", c"(F)V")?;
            env.call_void(paint, set_size, &[float(spec.size as f32)]);
            let metrics_method =
                env.method(paint_class, c"getFontMetrics", c"()Landroid/graphics/Paint$FontMetrics;")?;
            let metrics = env.call_object(paint, metrics_method, &[])?;
            let metrics_class = env.class(c"android/graphics/Paint$FontMetrics")?;
            let field = |name: &std::ffi::CStr| env.float_field(metrics, env.field(metrics_class, name, c"F")?);
            // the ascent is negative (above the baseline); the leading
            // folds into the descent — the LineMetrics contract
            let ascent = -(field(c"ascent")? as f64);
            let descent = field(c"descent")? as f64 + field(c"leading")? as f64;
            Face { paint: env.global(paint)?, ascent, descent }
        };
        let answer = read(&face);
        self.faces.borrow_mut().insert(key, face);
        Some(answer)
    }
}

impl Default for AndroidTextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AndroidTextEngine {
    fn drop(&mut self) {
        self.drop_faces();
        if let Some(env) = Env::current() {
            for (_, typeface) in self.registered.borrow_mut().drain() {
                env.delete_global(typeface);
            }
        }
    }
}

impl TextEngine for AndroidTextEngine {
    fn families(&self) -> Vec<std::sync::Arc<str>> {
        // the platform names no families; these three it always has
        let mut names: Vec<std::sync::Arc<str>> =
            ["monospace", "sans-serif", "serif"].into_iter().map(std::sync::Arc::from).collect();
        names.extend(self.registered.borrow().keys().map(|name| std::sync::Arc::from(name.as_str())));
        names.sort();
        names.dedup();
        names
    }

    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let Some((paint, ascent, descent)) = self.face(font, |face| (face.paint, face.ascent, face.descent))
        else {
            return LineMetrics { width: 0.0, ascent: font.size, descent: 0.0 };
        };
        if text.is_empty() {
            // line height preserved without asking Java
            return LineMetrics { width: 0.0, ascent, descent };
        }
        let width = (|| {
            let env = Env::current()?;
            let _frame = Frame::new(env, 8)?;
            let paint_class = env.class(c"android/graphics/Paint")?;
            let measure = env.method(paint_class, c"measureText", c"(Ljava/lang/String;)F")?;
            let string = env.string(text)?;
            env.call_float(paint, measure, &[object(string)])
        })()
        .unwrap_or(0.0) as f64;
        LineMetrics { width, ascent, descent }
    }

    fn raster_line(&self, text: &str, font: &FontSpec, color: Color, scale: usize) -> Option<TextRaster> {
        if text.is_empty() {
            return None;
        }
        let metrics = self.measure_line(text, font);
        let width = (metrics.width * scale as f64).ceil() as usize;
        let height = (metrics.height() * scale as f64).ceil() as usize;
        if width == 0 || height == 0 {
            return None;
        }
        let paint = self.face(font, |face| face.paint)?;
        let env = Env::current()?;
        let _frame = Frame::new(env, 16)?;
        // the bitmap, transparent; the canvas over it, in logical points
        let bitmap_class = env.class(c"android/graphics/Bitmap")?;
        let config_class = env.class(c"android/graphics/Bitmap$Config")?;
        let argb = env.static_object_field(
            config_class,
            env.static_field(config_class, c"ARGB_8888", c"Landroid/graphics/Bitmap$Config;")?,
        )?;
        let create = env.static_method(
            bitmap_class,
            c"createBitmap",
            c"(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
        )?;
        let bitmap =
            env.call_static_object(bitmap_class, create, &[int(width as i32), int(height as i32), object(argb)])?;
        let canvas_class = env.class(c"android/graphics/Canvas")?;
        let canvas = env.new_object(
            canvas_class,
            env.method(canvas_class, c"<init>", c"(Landroid/graphics/Bitmap;)V")?,
            &[object(bitmap)],
        )?;
        env.call_void(canvas, env.method(canvas_class, c"scale", c"(FF)V")?, &[float(scale as f32), float(scale as f32)]);
        let paint_class = env.class(c"android/graphics/Paint")?;
        let argb_color = ((color.a as i32) << 24) | ((color.r as i32) << 16) | ((color.g as i32) << 8) | (color.b as i32);
        env.call_void(paint, env.method(paint_class, c"setColor", c"(I)V")?, &[int(argb_color)]);
        // the baseline sits `ascent` below the top of the line box (the
        // ceil slack stays at the bottom, sub-pixel)
        let draw = env.method(canvas_class, c"drawText", c"(Ljava/lang/String;FFLandroid/graphics/Paint;)V")?;
        let string = env.string(text)?;
        env.call_void(canvas, draw, &[object(string), float(0.0), float(metrics.ascent as f32), object(paint)]);
        // the pixels, straight out of the bitmap's memory
        let mut rgba = vec![0u8; width * height * 4];
        let copied = unsafe {
            let mut info = AndroidBitmapInfo { width: 0, height: 0, stride: 0, format: 0, flags: 0 };
            if AndroidBitmap_getInfo(env.raw(), bitmap, &mut info) != 0
                || info.format != ANDROID_BITMAP_FORMAT_RGBA_8888
            {
                false
            } else {
                let mut pixels: *mut c_void = null_mut();
                if AndroidBitmap_lockPixels(env.raw(), bitmap, &mut pixels) != 0 || pixels.is_null() {
                    false
                } else {
                    let rows = height.min(info.height as usize);
                    let columns = width.min(info.width as usize);
                    for row in 0..rows {
                        std::ptr::copy_nonoverlapping(
                            pixels.cast::<u8>().add(row * info.stride as usize),
                            rgba.as_mut_ptr().add(row * width * 4),
                            columns * 4,
                        );
                    }
                    AndroidBitmap_unlockPixels(env.raw(), bitmap);
                    true
                }
            }
        };
        if let Some(recycle) = env.method(bitmap_class, c"recycle", c"()V") {
            env.call_void(bitmap, recycle, &[]);
        }
        if !copied {
            return None;
        }
        crate::image::unpremultiply(&mut rgba);
        Some(TextRaster { width, height, baseline: (metrics.ascent * scale as f64).round() as usize, rgba })
    }
}

