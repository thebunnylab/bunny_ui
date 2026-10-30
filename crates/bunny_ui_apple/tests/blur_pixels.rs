//! Effect pixels reach ImageIO-backed native painting without being decoded again:
//! they ride a feed slot of raw bytes.
#![cfg(any(target_os = "macos", target_os = "ios"))]

use bunny_ui::image_engine::{ImageSource, raster_source};
use bunny_ui::layout::{Color, DrawCommand, Size};
use bunny_ui::prelude::*;
use bunny_ui_apple::CoreGraphicsImageEngine;

#[test]
fn native_decoder_displays_the_complete_blurred_subtree() {
    let view = vstack!(text("Blur sample").font_size(28.0))
        .frame(180.0, 100.0)
        .background_color(Color::rgb(28, 90, 180))
        .foreground_color(Color::rgb(255, 255, 255))
        .blur(5.0)
        .padding_length(20.0);
    let runtime = Runtime::new();
    let display = runtime.display_frame(
        &view,
        Size {
            width: 220.0,
            height: 140.0,
        },
    );
    let source = display
        .iter()
        .find_map(|command| match command {
            DrawCommand::Image { source, .. } => Some(source),
            _ => None,
        })
        .expect("the blur composites the subtree");
    let engine = CoreGraphicsImageEngine::new();
    let native = raster_source(&engine, source, 220, 140)
        .expect("native painting can read the effect pixels");
    assert!(
        native
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[2] > 100 && p[3] > 240)
    );
    // raw bytes, never a platform decoder — on the feed road, so a blur
    // that animates replaces one texture instead of minting a picture
    assert!(
        matches!(source, ImageSource::Feed { .. }),
        "computed pixels ride a feed slot of raw bytes"
    );
}
