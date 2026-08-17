//! The finder, in a browser tab — the same scene the desktop runs:
//! ten thousand rows behind the virtual window, a live filter, spring
//! animations, all rasterized by the engine and blitted to a canvas.
#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use bunny_ui::prelude::*;

fn matches(dir: &str, name: &str, needle: &str) -> bool {
    let mut haystack = dir.chars().chain(name.chars()).map(|c| c.to_ascii_lowercase());
    needle.chars().map(|c| c.to_ascii_lowercase()).all(|wanted| haystack.any(|c| c == wanted))
}

#[derive(Clone)]
struct Finder {
    query: State<String>,
    selected: State<usize>,
    visible: State<Rc<Vec<usize>>>,
    files: Rc<Vec<(Rc<str>, Rc<str>)>>,
    /// Built ONCE (hash + registration happen per identity, never per
    /// body) — the browser decodes it and reports back.
    logo: ImageSource,
}

impl Component for Finder {
    fn body(self, _ctx: &Context) -> impl View {
        let files = Rc::clone(&self.files);
        let visible = self.visible.get();
        let count = visible.len();
        let selected = self.selected;
        let selected_index = selected.get().min(count.saturating_sub(1));
        let id_files = Rc::clone(&files);
        let id_visible = Rc::clone(&visible);
        // a floating panel over the theme's canvas — rounded, bordered,
        // with a soft shadow: the SAME chrome on every rendering
        vstack!(vstack!(
            hstack!(
                image(self.logo.clone()).resizable().frame(18.0, 18.0),
                text("›").foreground_color(theme::accent()),
                text_field("Search ten thousand files…", self.query.binding()).monospaced(),
                count_meter(count),
            )
            .spacing(10.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(10.0),
            virtual_list(
                count,
                move |row| {
                    let (name, dir) = &id_files[id_visible[row]];
                    format!("{dir}{name}")
                },
                move |row| {
                    let (name, dir) = &files[visible[row]];
                    let on = row == selected_index;
                    hstack!(
                        text(name.clone()).foreground_color(theme::fg()),
                        text(dir.clone())
                            .monospaced()
                            .foreground_color(theme::fg_secondary()),
                        spacer(),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .padding_edge(Edge::Leading, 12.0)
                    .padding_edge(Edge::Trailing, 12.0)
                    .padding_edge(Edge::Top, 6.0)
                    .padding_edge(Edge::Bottom, 6.0)
                    .background_color(if on { theme::row_pressed() } else { CLEAR })
                    .background_hovered(theme::row_hover())
                    .animated(Spring::snappy())
                    .on_click(move || selected.set(row))
                },
            )
            .reveal(selected_index),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Bottom, 10.0)
        .background_color(theme::panel())
        .corner_radius(12.0)
        .border(theme::border(), 1.0)
        .shadow(28.0))
        .padding_length(28.0)
        .on_change(
            {
                let query = self.query;
                move || query.get()
            },
            false,
            {
                let files = Rc::clone(&self.files);
                let cache = self.visible;
                move |_, query: &String| {
                    cache.set(Rc::new(
                        (0..files.len())
                            .filter(|index| {
                                let (name, dir) = &files[*index];
                                query.is_empty() || matches(dir, name, query)
                            })
                            .collect(),
                    ));
                }
            },
        )
    }
}

const CLEAR: Color = Color { r: 0, g: 0, b: 0, a: 0 };

// MARK: - The logo, a png written by hand

/// Bitwise CRC-32 (the PNG polynomial) — tiny, demo-only.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut body = kind.to_vec();
    body.extend_from_slice(payload);
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// The 12×12 bunny mark, one bit per pixel — ears, head, eyes.
const BUNNY: [u16; 12] = [
    0b0110_0000_0110,
    0b0110_0000_0110,
    0b0110_0000_0110,
    0b0111_1111_1110,
    0b1111_1111_1111,
    0b1111_1111_1111,
    0b1101_1111_1011,
    0b1111_1111_1111,
    0b1111_1111_1111,
    0b0111_1111_1110,
    0b0011_1111_1100,
    0b0000_0000_0000,
];

/// A REAL png (zlib with one stored deflate block needs no
/// compressor) — the browser decodes it like any other asset.
fn logo() -> ImageSource {
    let accent = theme::accent();
    let mut raw = Vec::new();
    for row in BUNNY {
        raw.push(0); // filter: none
        for column in 0..12u16 {
            let on = row >> (11 - column) & 1 == 1;
            let alpha = if on { 255 } else { 0 };
            raw.extend_from_slice(&[accent.r, accent.g, accent.b, alpha]);
        }
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&12u32.to_be_bytes());
    ihdr.extend_from_slice(&12u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    let mut idat = vec![0x78, 0x01, 0x01];
    idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    idat.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    idat.extend_from_slice(&raw);
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &idat));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    ImageSource::from_bytes(png)
}

/// The visible count, drawn as five digit bars — custom pixels. On the
/// Dom page this subtree claims a CANVAS ISLAND (`.rendering(Gpu)`):
/// our layout positions the element, our rasterizer fills it, and the
/// bars redraw live as the filter types. On the canvas page the
/// modifier dissolves — everything is pixels there already.
fn count_meter(count: usize) -> impl View {
    let digit = |place: u32| ((count / 10usize.pow(place)) % 10) as f64;
    let bar = |place: u32| {
        spacer()
            .frame(6.0, 6.0 + digit(place) * 2.0)
            .background_color(theme::accent())
            .corner_radius(2.0)
    };
    hstack!(bar(4), bar(3), bar(2), bar(1), bar(0))
        .spacing(2.0)
        .alignment(VerticalAlignment::Bottom)
        .rendering(Rendering::Gpu)
}

fn finder() -> Finder {
    let files: Rc<Vec<(Rc<str>, Rc<str>)>> = Rc::new(
        (0..10_000)
            .map(|index| {
                (
                    Rc::from(format!("file_{index:04}.rs")),
                    Rc::from(format!("src/mod_{:02}/", index % 100)),
                )
            })
            .collect(),
    );
    Finder {
        query: State::new(String::new()),
        selected: State::new(0),
        visible: State::new(Rc::new((0..10_000).collect())),
        files,
        logo: logo(),
    }
}

/// The glue calls this once, with the canvas geometry.
#[unsafe(no_mangle)]
pub extern "C" fn start(width: f64, height: f64, scale: f64) {
    bunny_ui_web::start(width, height, scale, finder());
}

/// The Dom page calls this one — the SAME scene, rendered at home.
/// `scale` rasters the canvas islands at the device's density.
#[unsafe(no_mangle)]
pub extern "C" fn start_dom(width: f64, height: f64, scale: f64) {
    bunny_ui_web::start_dom(width, height, scale, finder());
}
