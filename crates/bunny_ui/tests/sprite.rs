//! A picture under an affine map (`DrawCommand::Sprite`): the compositor
//! turns, scales and fades pixels it already holds. The CPU raster is the
//! oracle every GPU tier is held to, and the walk hands the tiers one
//! turned instance per picture, on the map the raster uses.

#![cfg(feature = "gpu")]

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::gpu::walk::{FrameBatches, RecordingGround, RunAtlas, RunKind, build_frame};
use bunny_ui::image_engine::{ImageSource, RawImages};
use bunny_ui::layout::{
    Affine, Color, DisplayList, DrawCommand, ImagePlacement, Point, Rect, Size,
};
use bunny_ui::raster::{rasterize_with, sprite_texels};
use bunny_ui::text_engine::PixelFont;

const GROUND: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};

fn square(side: u32) -> ImageSource {
    let mut rgba = Vec::new();
    for _ in 0..side * side {
        rgba.extend_from_slice(&[255, 64, 32, 255]);
    }
    ImageSource::rgba(0x5a1, (side, side), rgba)
}

fn rect(x: f64, y: f64, side: f64) -> Rect {
    Rect {
        origin: Point { x, y },
        size: Size {
            width: side,
            height: side,
        },
    }
}

fn pixels(display: DisplayList, side: usize) -> Vec<[u8; 4]> {
    rasterize_with(
        &display,
        side,
        side,
        1,
        GROUND,
        &PixelFont,
        &RawImages::default(),
    )
    .to_rgba_bytes()
    .as_chunks::<4>()
    .0
    .to_vec()
}

fn sprite(transform: Affine, opacity: f64) -> DisplayList {
    DisplayList::from(vec![DrawCommand::Sprite {
        rect: rect(0.0, 0.0, 8.0),
        source: square(8),
        transform,
        opacity,
    }])
}

#[test]
fn an_upright_sprite_lays_the_pixels_an_image_does() {
    let moved = Affine::translation(4.0, 6.0);
    let image = DisplayList::from(vec![DrawCommand::Image {
        rect: rect(4.0, 6.0, 8.0),
        source: square(8),
        placement: ImagePlacement::Pixel,
    }]);
    assert_eq!(pixels(sprite(moved, 1.0), 20), pixels(image, 20));
}

#[test]
fn a_turned_sprite_covers_its_turned_box_with_soft_edges() {
    // an eighth of a turn about the square's centre, moved to the middle
    let (cos, sin) = (
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    );
    let about = Affine::translation(10.0, 10.0)
        .then(&Affine {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        })
        .then(&Affine::translation(-4.0, -4.0));
    let shown = pixels(sprite(about, 1.0), 20);
    let at = |x: usize, y: usize| shown[y * 20 + x];
    assert_eq!(
        at(10, 10),
        [255, 64, 32, 255],
        "the centre is the picture's own colour"
    );
    assert_eq!(
        at(10, 6),
        [255, 64, 32, 255],
        "a turned square reaches up its diagonal"
    );
    assert_eq!(at(4, 4)[0], 0, "and leaves its old corner bare");
    let edge = (0..20)
        .map(|x| at(x, 10)[0])
        .filter(|red| *red > 0 && *red < 255)
        .count();
    assert!(edge >= 2, "the turned edges fade over a texel, not a step");
}

#[test]
fn a_faded_sprite_lands_by_its_opacity() {
    let shown = pixels(sprite(Affine::IDENTITY, 0.5), 10);
    let red = shown[3 * 10 + 3][0];
    assert!(
        (126..=129).contains(&red),
        "half of 255 over black, got {red}"
    );
}

#[test]
fn the_walk_hands_the_tiers_one_turned_instance_on_the_rasters_map() {
    let turn = Affine {
        a: 0.0,
        b: 1.0,
        c: -1.0,
        d: 0.0,
        e: 30.0,
        f: 10.0,
    };
    let display = sprite(turn, 0.75);
    let mut ground = RecordingGround::default();
    let mut atlas = RunAtlas::new();
    let mut batches = FrameBatches::default();
    build_frame(
        &mut ground,
        &display,
        2,
        (200, 200),
        &PixelFont,
        &RawImages::default(),
        &mut atlas,
        &mut batches,
        Some(GROUND),
    )
    .expect("the picture fits the atlas");
    assert_eq!(batches.turned.len(), 1);
    assert_eq!(batches.runs.len(), 1);
    assert_eq!(batches.runs[0].kind, RunKind::Turned);
    let instance = batches.turned[0];
    let texels = sprite_texels(rect(0.0, 0.0, 8.0), &turn, 2, (16, 16));
    assert_eq!(
        instance.axes,
        [
            texels.a as f32,
            texels.b as f32,
            texels.c as f32,
            texels.d as f32
        ]
    );
    assert_eq!(instance.origin, [60.0, 20.0, 0.75, 0.0]);
    assert_eq!(instance.tex[2] - instance.tex[0], 16.0);
}
