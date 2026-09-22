//! The codecs against the fixtures Pillow (libjpeg-turbo) wrote and
//! decoded — `tests/fixtures/codec/make.py` says how. A JPEG agrees
//! within two steps per channel and half a step on average; a PNG
//! agrees byte for byte, interlaced or not.

#![cfg(feature = "codec")]

use bunny_ui::codec::{self, jpeg, png};

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("fixtures/codec/", $name))
    };
}

const WIDTH: u32 = 33;
const HEIGHT: u32 = 21;

/// The largest per-channel delta and the mean, decoded against Pillow.
fn compare(name: &str, bytes: &[u8], expected: &[u8]) -> (u8, f64) {
    let image = codec::decode(bytes).unwrap_or_else(|| panic!("{name} decodes"));
    assert_eq!((image.width, image.height), (WIDTH, HEIGHT), "{name}: the size");
    assert_eq!(image.rgba.len(), expected.len(), "{name}: the byte count");
    let mut worst = 0u8;
    let mut total = 0u64;
    for (ours, theirs) in image.rgba.iter().zip(expected) {
        let delta = ours.abs_diff(*theirs);
        worst = worst.max(delta);
        total += delta as u64;
    }
    (worst, total as f64 / expected.len() as f64)
}

fn assert_close(name: &str, bytes: &[u8], expected: &[u8]) {
    let (worst, mean) = compare(name, bytes, expected);
    assert!(worst <= 2, "{name}: worst channel delta {worst} (allowed 2), mean {mean:.3}");
    assert!(mean <= 0.5, "{name}: mean delta {mean:.3} (allowed 0.5), worst {worst}");
}

#[test]
fn baseline_at_every_sampling_agrees_with_libjpeg() {
    assert_close("base_444", fixture!("base_444.jpg"), fixture!("base_444.rgba"));
    assert_close("base_422", fixture!("base_422.jpg"), fixture!("base_422.rgba"));
    assert_close("base_420", fixture!("base_420.jpg"), fixture!("base_420.rgba"));
}

#[test]
fn a_gray_jpeg_agrees() {
    assert_close("base_gray", fixture!("base_gray.jpg"), fixture!("base_gray.rgba"));
}

#[test]
fn a_progressive_jpeg_agrees() {
    assert_close("prog_420", fixture!("prog_420.jpg"), fixture!("prog_420.rgba"));
}

#[test]
fn restart_intervals_resynchronize() {
    assert_close("restart_420", fixture!("restart_420.jpg"), fixture!("restart_420.rgba"));
}

#[test]
fn an_adobe_rgb_jpeg_stays_rgb() {
    assert_close("adobe_rgb", fixture!("adobe_rgb.jpg"), fixture!("adobe_rgb.rgba"));
}

#[test]
fn the_header_answers_the_size_of_every_jpeg() {
    for bytes in [
        fixture!("base_444.jpg").as_slice(),
        fixture!("prog_420.jpg"),
        fixture!("adobe_rgb.jpg"),
        fixture!("base_gray.jpg"),
    ] {
        assert_eq!(jpeg::header(bytes), Some((WIDTH, HEIGHT)));
        assert_eq!(codec::header(bytes), Some((WIDTH, HEIGHT)));
    }
}

#[test]
fn a_plain_png_decodes_byte_for_byte() {
    let image = png::decode(fixture!("plain.png")).expect("decodes");
    assert_eq!((image.width, image.height), (WIDTH, HEIGHT));
    assert_eq!(&image.rgba[..], &fixture!("plain.rgba")[..]);
}

#[test]
fn an_interlaced_png_equals_its_plain_twin() {
    let interlaced = png::decode(fixture!("adam7.png")).expect("decodes");
    assert_eq!(&interlaced.rgba[..], &fixture!("adam7.rgba")[..]);
    assert_eq!(png::header(fixture!("adam7.png")), Some((WIDTH, HEIGHT)));
    let plain = png::decode(fixture!("plain.png")).expect("decodes");
    assert_eq!(interlaced.rgba, plain.rgba, "Adam7 and the plain file are one picture");
}
