//! ImageIO reads the same pictures as the codecs of the house: the
//! fixtures Pillow wrote (`crates/bunny_ui/tests/fixtures/codec/`),
//! decoded by the platform through `CoreGraphicsImageEngine` and by
//! `bunny_ui::codec`. A PNG agrees byte for byte. A JPEG does not: the
//! house decodes bit for bit like libjpeg-turbo (the fixture tests in
//! the core pin that), and ImageIO is another decoder — its own IDCT,
//! its own chroma filter — that lands within two steps on average and
//! as far as sixty at a hard edge of a 4:2:0 picture, measured, where
//! the two filters disagree on how a chroma sample spreads. The gate
//! here is the average, which a broken decoder blows past by a hundred.

#![cfg(any(target_os = "macos", target_os = "ios"))]

use bunny_ui::codec;
use bunny_ui::image_engine::{ImageEngine, ImageSource};
use bunny_ui_apple::CoreGraphicsImageEngine;

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("../../bunny_ui/tests/fixtures/codec/", $name))
    };
}

fn agree(name: &str, bytes: &'static [u8], mean_allowed: f64) {
    let ours = codec::decode(bytes).unwrap_or_else(|| panic!("{name}: the house decodes"));
    let engine = CoreGraphicsImageEngine::new();
    let source = ImageSource::from_bytes(bytes);
    let (width, height) = engine.intrinsic(&source).expect("ImageIO reads the size");
    assert_eq!((width, height), (ours.width, ours.height), "{name}: the sizes agree");
    let theirs = engine
        .raster(&source, width as usize, height as usize)
        .expect("ImageIO decodes");
    assert_eq!(theirs.rgba.len(), ours.rgba.len(), "{name}: the byte counts agree");
    let deltas = ours.rgba.iter().zip(theirs.rgba.iter()).map(|(a, b)| a.abs_diff(*b) as f64);
    let mean = deltas.sum::<f64>() / ours.rgba.len() as f64;
    assert!(
        mean <= mean_allowed,
        "{name}: mean channel delta {mean:.2} against ImageIO (allowed {mean_allowed})"
    );
}

#[test]
fn imageio_agrees_with_the_house_jpeg() {
    // measured: 1.6 on the full-chroma pictures, 3.5 where ImageIO's
    // chroma filter differs from libjpeg's on the hard checker edges
    agree("base_444", fixture!("base_444.jpg"), 3.0);
    agree("base_422", fixture!("base_422.jpg"), 3.0);
    agree("base_420", fixture!("base_420.jpg"), 5.0);
    agree("base_gray", fixture!("base_gray.jpg"), 3.0);
    agree("prog_420", fixture!("prog_420.jpg"), 5.0);
    agree("restart_420", fixture!("restart_420.jpg"), 5.0);
}

#[test]
fn imageio_agrees_with_the_house_png() {
    agree("plain", fixture!("plain.png"), 0.0);
    agree("adam7", fixture!("adam7.png"), 0.0);
}
