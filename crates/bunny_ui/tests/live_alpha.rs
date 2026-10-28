//! A live box rasters onto a transparent ground, so its pixels come out
//! PREMULTIPLIED, and that is what the blit says it carries. Composited
//! over the scene's hole the way a layer composites (`out = src + dst x
//! (1 - a)`), a cloud with soft edges lands on the very pixels the whole
//! frame paints. Taken for straight alpha and multiplied again, each soft
//! edge would darken into a fringe.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::custom::canvas;
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::{Color, Point, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::raster::rasterize_with;
use bunny_ui::runtime::Runtime;

const SIZE: Size = Size { width: 48.0, height: 24.0 };

/// A soft white puff: opaque at its middle, fading to nothing at its ends.
fn puff() -> ImageSource {
    let (width, height) = (32u32, 8u32);
    let rgba: Vec<u8> = (0..height)
        .flat_map(|_| {
            (0..width).flat_map(|x| {
                let from_middle = (f64::from(x) - 15.5).abs() / 15.5;
                let alpha = ((1.0 - from_middle) * 255.0).round() as u8;
                [255, 255, 255, alpha]
            })
        })
        .collect();
    ImageSource::rgba(0x000c_100d, (width, height), rgba)
}

#[derive(Clone, Copy)]
struct Dawn;

impl Component for Dawn {
    fn body(self) -> impl View {
        zstack!(
            // a navy sky, then the puff drifting over it on a loop
            rectangle().foreground_color(Color::rgba(20, 30, 70, 255)),
            canvas(|ctx, p| {
                let drift = ctx.phase * 8.0;
                p.image(
                    Rect { origin: Point { x: 4.0 + drift, y: 8.0 }, size: Size { width: 32.0, height: 8.0 } },
                    puff(),
                );
            })
            .looping(Loop::secs(1.0).fps(4.0))
        )
        .frame(SIZE.width, SIZE.height)
    }
}

#[test]
fn a_soft_edged_live_picture_composites_onto_the_scenes_own_pixels() {
    for scale in [1usize, 2] {
        let runtime = Runtime::new();
        let full = runtime.display_frame(&Dawn, SIZE);
        let slices = runtime.live_slices();
        assert_eq!(slices.len(), 1, "the puff presents on a surface of its own");
        let blit = runtime.live_islands_all(scale).remove(0);

        let (width, height) = (SIZE.width as usize * scale, SIZE.height as usize * scale);
        let paint = |display: &bunny_ui::layout::DisplayList| {
            rasterize_with(display, width, height, scale, Color::WHITE, &*runtime.text(), &*runtime.images())
                .to_rgba_bytes()
        };
        let scene = paint(&full);
        let mut layered = paint(&full.without_slices(&slices));

        // the layer, over the hole the scene kept for it
        let x0 = (blit.frame.origin.x * scale as f64).round() as usize;
        let y0 = (blit.frame.origin.y * scale as f64).round() as usize;
        let mut soft = 0;
        for row in 0..blit.height {
            for column in 0..blit.width {
                let from = (row * blit.width + column) * 4;
                let to = ((y0 + row) * width + x0 + column) * 4;
                let source = &blit.premultiplied[from..from + 4];
                let alpha = u32::from(source[3]);
                soft += usize::from(alpha > 0 && alpha < 255);
                for channel in 0..3 {
                    let under = u32::from(layered[to + channel]);
                    layered[to + channel] = (u32::from(source[channel]) + (under * (255 - alpha) + 127) / 255).min(255) as u8;
                }
            }
        }
        assert!(soft > 0, "the puff has soft edges to get wrong");
        let worst = scene.iter().zip(&layered).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(worst <= 1, "at {scale}x the layer is off the scene by {worst}");
    }
}
