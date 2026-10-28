//! A picture drawn with `Painter::image_subpixel` lands at its exact
//! origin, between device pixels when it falls there — the way a browser
//! moves an image under a transform. A drift of a point a second then
//! glides by fractions of a pixel instead of stepping a whole one, and a
//! soft edge between pixels keeps its colour: the texels are shared in
//! premultiplied colour, so a transparent texel adds nothing, whatever RGB
//! its decoder left in it.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::custom::canvas;
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::{Color, DisplayList, DrawCommand, ImagePlacement, Point, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::raster::{ListDamage, list_damage, rasterize_with};
use bunny_ui::text_engine::MeasureCache;
use bunny_ui::runtime::Runtime;

const GROUND: Color = Color { r: 0, g: 0, b: 0, a: 255 };

/// A 4×1 white bar: opaque in the middle, half at each end.
fn bar() -> ImageSource {
    let rgba = [255, 255, 255, 128, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 128];
    ImageSource::rgba(0xba5, (4, 1), rgba.to_vec())
}

/// `picture` drawn at `x` in a 16×1 window over black, at 1×, by the
/// placement asked; the red row back.
fn row(x: f64, picture: ImageSource, subpixel: bool) -> Vec<u8> {
    let runtime = Runtime::new();
    let view = canvas(move |_, p| {
        let rect = Rect { origin: Point { x, y: 0.0 }, size: Size { width: 4.0, height: 1.0 } };
        if subpixel {
            p.image_subpixel(rect, picture.clone());
        } else {
            p.image(rect, picture.clone());
        }
    })
    .frame(16.0, 1.0);
    let display = runtime.display_frame(&view, Size { width: 16.0, height: 1.0 });
    rasterize_with(&display, 16, 1, 1, GROUND, &*runtime.text(), &*runtime.images())
        .to_rgba_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| pixel[0])
        .collect()
}

/// Where the row's light sits: its centre of mass along x, in pixels.
fn centre(row: &[u8]) -> f64 {
    let total: f64 = row.iter().map(|&v| f64::from(v)).sum();
    row.iter().enumerate().map(|(x, &v)| (x as f64 + 0.5) * f64::from(v)).sum::<f64>() / total
}

#[test]
fn a_subpixel_picture_moves_by_fractions_of_a_pixel() {
    let steps = [4.0, 4.25, 4.5, 4.75, 5.0];
    let glided: Vec<f64> = steps.iter().map(|&x| centre(&row(x, bar(), true))).collect();
    for (pair, x) in glided.windows(2).zip(&steps) {
        let moved = pair[1] - pair[0];
        assert!((moved - 0.25).abs() < 0.02, "from {x} the light moved {moved}, not a quarter pixel");
    }
    // the snapped picture stands still, then jumps
    let snapped: Vec<f64> = steps.iter().map(|&x| centre(&row(x, bar(), false))).collect();
    assert_eq!(snapped[0], snapped[1], "a quarter of a pixel rounds away");
    assert!((snapped[2] - snapped[0] - 1.0).abs() < 1e-9, "and a half jumps a whole one");
}

#[test]
fn on_the_grid_both_placements_paint_the_same_bytes() {
    for x in [0.0, 3.0, 11.0] {
        assert_eq!(row(x, bar(), true), row(x, bar(), false), "at {x}");
    }
}

#[test]
fn between_pixels_the_light_is_shared_not_lost() {
    let whole: u32 = row(4.0, bar(), true).iter().map(|&v| u32::from(v)).sum();
    for x in [4.1, 4.5, 4.9] {
        let shared: u32 = row(x, bar(), true).iter().map(|&v| u32::from(v)).sum();
        assert!(shared.abs_diff(whole) <= 4, "at {x} the bar holds {shared} of {whole}");
    }
    // the half pixel at each end is a pixel of its own: 4 texels, 5 pixels
    let lit = row(4.5, bar(), true).iter().filter(|&&v| v > 0).count();
    assert_eq!(lit, 5);
}

#[test]
fn a_soft_edge_between_pixels_never_darkens() {
    // a white cloud on a white sky: wherever it is soft, the sky shows
    // through; its transparent texels hold BLACK, as a decoder leaves them
    let rgba: Vec<u8> = [0u8, 64, 128, 192, 255, 255, 192, 128, 64, 0]
        .iter()
        .flat_map(|&alpha| if alpha == 0 { [0, 0, 0, 0] } else { [255, 255, 255, alpha] })
        .collect();
    let cloud = ImageSource::rgba(0xc10d, (10, 1), rgba);
    let display = DisplayList::from(vec![
        DrawCommand::FillRect {
            rect: Rect { origin: Point::ZERO, size: Size { width: 16.0, height: 1.0 } },
            color: Color::WHITE,
            corner_radius: 0.0.into(),
        },
        DrawCommand::Image {
            rect: Rect { origin: Point { x: 3.37, y: 0.0 }, size: Size { width: 10.0, height: 1.0 } },
            source: cloud,
            placement: ImagePlacement::Subpixel,
        },
    ]);
    let runtime = Runtime::new();
    let painted = rasterize_with(&display, 16, 1, 1, GROUND, &*runtime.text(), &*runtime.images());
    let darkest = painted.to_rgba_bytes().as_chunks::<4>().0.iter().map(|pixel| pixel[0].min(pixel[1]).min(pixel[2])).min();
    assert_eq!(darkest, Some(255), "black texels at alpha 0 bled into the edge");
}

#[test]
fn a_drift_between_pixels_damages_the_pixel_it_spills_into() {
    let at = |x: f64| {
        DisplayList::from(vec![DrawCommand::Image {
            rect: Rect { origin: Point { x, y: 0.0 }, size: Size { width: 4.0, height: 1.0 } },
            source: bar(),
            placement: ImagePlacement::Subpixel,
        }])
    };
    let runtime = Runtime::new();
    let cache = MeasureCache::default();
    let damage = list_damage(at(4.0).as_slice(), at(4.25).as_slice(), 1, (16, 1), 8, &cache, &*runtime.text());
    let ListDamage::Rect((x0, _, x1, _)) = damage else { panic!("one box of damage") };
    assert!(x0 <= 4 && x1 >= 9, "the moved bar covers pixels 4 to 8, not {x0}..{x1}");
}
