//! Compact live surfaces preserve the full raster and their placement ledger.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::custom::canvas;
use bunny_ui::layout::{Color, Point, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::{LiveBlit, Runtime};

#[derive(Clone, Copy)]
struct Mark {
    x: f64,
}

impl Component for Mark {
    fn body(self) -> impl View {
        canvas(move |_, p| {
            p.fill(
                Rect {
                    origin: Point { x: self.x, y: 3.5 },
                    size: Size {
                        width: 12.0,
                        height: 7.0,
                    },
                },
                Color::hex(0xe69933),
            );
        })
        .looping(Loop::secs(1.0).fps(4.0))
        .frame(80.0, 50.0)
    }
}

fn composed(blit: &LiveBlit, size: Size, scale: usize) -> Vec<u8> {
    let width = size.width as usize * scale;
    let height = size.height as usize * scale;
    let mut rgba = vec![0; width * height * 4];
    let x = (blit.frame.origin.x * scale as f64).round() as usize;
    let y = (blit.frame.origin.y * scale as f64).round() as usize;
    assert_eq!(blit.frame.size.width * scale as f64, blit.width as f64);
    assert_eq!(blit.frame.size.height * scale as f64, blit.height as f64);
    for row in 0..blit.height {
        let from = row * blit.width * 4;
        let to = ((y + row) * width + x) * 4;
        rgba[to..to + blit.width * 4].copy_from_slice(&blit.rgba[from..from + blit.width * 4]);
    }
    rgba
}

#[test]
fn an_opaque_live_fill_keeps_only_its_ink_and_matches_the_full_raster() {
    for scale in [1, 2] {
        for x in [-8.5, -0.5, 4.25, 72.5] {
            let runtime = Runtime::new();
            let view = Mark { x };
            let size = Size {
                width: 80.0,
                height: 50.0,
            };
            let display = runtime.display_frame(&view, size);
            let mut blits = runtime.live_islands_all(scale);
            assert_eq!(blits.len(), 1);
            let blit = blits.remove(0);
            assert!(
                blit.width <= 12 * scale && blit.height <= 8 * scale,
                "empty canvas must not own pixels"
            );
            let expected = bunny_ui::raster::rasterize_scaled(
                &display,
                80 * scale,
                50 * scale,
                scale,
                Color::rgba(0, 0, 0, 0),
            );
            assert_eq!(
                composed(&blit, size, scale),
                expected.to_rgba_bytes(),
                "x={x}, scale={scale}"
            );
            assert_eq!(
                runtime.live_frames()[0].1,
                blit.frame,
                "ordinary placement must not expand the cropped layer"
            );
            assert!(
                runtime.live_islands_all(scale).is_empty(),
                "an unchanged picture stays quiet"
            );
        }
    }
}

#[derive(Clone)]
struct Shift {
    offset: State<f64>,
}

impl Component for Shift {
    fn body(self) -> impl View {
        Mark { x: 3.0 }
            .padding_edge(Edge::Leading, self.offset.get())
            .padding_edge(Edge::Top, self.offset.get())
    }
}

#[test]
fn a_compact_live_surface_moves_with_layout_without_repainting() {
    let runtime = Runtime::new();
    let size = Size {
        width: 200.0,
        height: 200.0,
    };
    let offset = State::new(7.0);
    let view = Shift {
        offset: offset.clone(),
    };
    let _ = runtime.display_frame(&view, size);
    let first = runtime.live_islands_all(1).remove(0);
    assert_eq!(first.width, 12);
    let before = runtime.live_frames().remove(0).1;
    offset.set(57.0);
    let _ = runtime.display_frame(&view, size);
    assert!(runtime.live_islands_all(1).is_empty());
    let after = runtime.live_frames().remove(0).1;
    assert_eq!(after.size, before.size);
    assert_eq!(after.origin.x - before.origin.x, 50.0);
    assert_eq!(after.origin.y - before.origin.y, 50.0);
}

#[test]
fn fractional_placement_restores_the_full_texture_without_stale_cropping() {
    let runtime = Runtime::new();
    let size = Size {
        width: 200.0,
        height: 200.0,
    };
    let offset = State::new(7.0);
    let view = Shift {
        offset: offset.clone(),
    };
    let _ = runtime.display_frame(&view, size);
    assert_eq!(runtime.live_islands_all(1).remove(0).width, 12);
    offset.set(7.5);
    let _ = runtime.display_frame(&view, size);
    let full = runtime.live_islands_all(1).remove(0);
    assert_eq!(
        full.width, 80,
        "fractional edges need the old filtering neighborhood"
    );
    assert_eq!(runtime.live_frames()[0].1, full.frame);
    assert!(runtime.live_islands_all(1).is_empty());
    offset.set(8.0);
    let _ = runtime.display_frame(&view, size);
    assert_eq!(runtime.live_islands_all(1).remove(0).width, 12);
}
