//! Images on the Mac: the platform decodes a PNG this example writes
//! by hand (zlib with stored blocks needs no compressor), and the same
//! pixels ride the GPU atlas — small ones as shared tiles, the big
//! hero on a dedicated texture.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example image_window
//! ```

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

/// Bitwise CRC-32 (the PNG polynomial) — tiny, example-only.
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

/// A REAL png: one stored (uncompressed) deflate block is valid zlib.
fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA

    let mut raw = Vec::new();
    for y in 0..height {
        raw.push(0); // filter: none
        for x in 0..width {
            raw.extend_from_slice(&pixel(x, y));
        }
    }
    let mut idat = vec![0x78, 0x01, 0x01];
    idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    idat.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    idat.extend_from_slice(&raw);
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    out.extend_from_slice(&chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&chunk(b"IDAT", &idat));
    out.extend_from_slice(&chunk(b"IEND", &[]));
    out
}

#[cfg(target_os = "macos")]
fn main() {
    // a 64×40 "sunset": sky gradient over a dark ground band
    let hero = ImageSource::from_bytes(png(64, 40, |x, y| {
        if y >= 30 {
            [30, 30, 46, 255]
        } else {
            [200 + (x % 32) as u8, (140 - y * 3) as u8, (60 + y * 4) as u8, 255]
        }
    }));
    // a 16×16 "app icon": rounded feel from the alpha corners
    let icon = ImageSource::from_bytes(png(16, 16, |x, y| {
        let corner = |a: u32, b: u32| a < 2 && b < 2;
        let alpha = if corner(x, y)
            || corner(15 - x, y)
            || corner(x, 15 - y)
            || corner(15 - x, 15 - y)
        {
            0
        } else {
            255
        };
        [59, 130, 246, alpha]
    }));

    let row = |label: &str, view: Erased| {
        hstack!(text(label).monospaced().foreground_color(theme::fg_secondary()), spacer(), view)
            .spacing(12.0)
            .alignment(VerticalAlignment::Center)
    };

    bunny_ui_macos::run_window(
        "bunny_ui — images",
        Size { width: 520.0, height: 460.0 },
        vstack!(
            text("one png, every fit").bold(),
            row("intrinsic 64×40", erased(image(hero.clone()))),
            row(
                "fit in 240×120",
                erased(
                    image(hero.clone())
                        .resizable()
                        .aspect_ratio(ContentMode::Fit)
                        .frame(240.0, 120.0)
                        .background_color(theme::panel())
                ),
            ),
            row(
                "fill 240×60 (clips)",
                erased(
                    image(hero.clone())
                        .resizable()
                        .aspect_ratio(ContentMode::Fill)
                        .frame(240.0, 60.0)
                ),
            ),
            row(
                "dedicated 360×360",
                erased(image(hero).resizable().frame(360.0, 220.0)),
            ),
            row(
                "icons 16 and 32",
                erased(hstack!(
                    image(icon.clone()).resizable().frame(16.0, 16.0),
                    image(icon).resizable().frame(32.0, 32.0),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)),
            ),
        )
        .spacing(14.0)
        .alignment(HorizontalAlignment::Leading)
        .padding_length(24.0),
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
