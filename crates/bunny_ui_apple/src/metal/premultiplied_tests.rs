//! Premultiplied pictures land as a compositor lays its layers:
//! `s + (d·(255 − a) + 127) / 255` in every channel, on the GPU and in the
//! raster alike, for every colour, alpha and ground there is.

use super::{Color, DisplayList, MTLCreateSystemDefaultDevice, OffscreenGpu, msg_void, sel};
use bunny_ui::image_engine::{ImageSource, RawImages};
use bunny_ui::layout::{Corners, DrawCommand, ImagePlacement, Point, Rect, Size};
use bunny_ui::raster::{rasterize_translucent, rasterize_with};
use bunny_ui::text_engine::PixelFont;

fn device_present() -> bool {
    unsafe {
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            eprintln!("no metal device — skipping");
            return false;
        }
        msg_void(device, sel("release"));
        true
    }
}

/// The compositor's lowp source-over of one premultiplied channel.
fn laid(s: u8, a: u8, d: u8) -> u8 {
    let (s, a, d) = (u32::from(s), u32::from(a), u32::from(d));
    u8::try_from(s + (d * (255 - a) + 127) / 255).expect("a premultiplied channel stays a byte")
}

fn picture(at: (f64, f64), size: (usize, usize), source: ImageSource) -> DrawCommand {
    DrawCommand::Image {
        rect: Rect {
            origin: Point { x: at.0, y: at.1 },
            size: Size {
                width: size.0 as f64,
                height: size.1 as f64,
            },
        },
        source,
        placement: ImagePlacement::Pixel,
    }
}

fn rgba(pixels: &[[u8; 4]]) -> Vec<u8> {
    pixels.iter().flatten().copied().collect()
}

const WIDTH: usize = 2048;
const CLEAR: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

/// Every alpha, every colour no brighter than it and every ground, three
/// to a pixel: the ground (opaque, straight) and the picture over it.
fn every_combination() -> (Vec<[u8; 4]>, Vec<[u8; 4]>) {
    let (mut ground, mut over) = (Vec::new(), Vec::new());
    for a in 0..=255_u8 {
        let cells: Vec<(u8, u8)> = (0..=a)
            .flat_map(|s| (0..=255_u8).map(move |d| (s, d)))
            .collect();
        for chunk in cells.chunks(3) {
            let (mut below, mut above) = ([0, 0, 0, 255], [0, 0, 0, a]);
            for (channel, &(s, d)) in chunk.iter().enumerate() {
                below[channel] = d;
                above[channel] = s;
            }
            ground.push(below);
            over.push(above);
        }
    }
    while ground.len() % WIDTH != 0 {
        ground.push([0, 0, 0, 255]);
        over.push([0, 0, 0, 0]);
    }
    (ground, over)
}

fn assert_laid(shown: &[u8], ground: &[[u8; 4]], over: &[[u8; 4]], what: &str) {
    let mut wrong = 0_usize;
    let mut first = None;
    for (at, (pixel, (below, above))) in shown
        .as_chunks::<4>()
        .0
        .iter()
        .zip(ground.iter().zip(over))
        .enumerate()
    {
        let expected: [u8; 4] =
            std::array::from_fn(|channel| laid(above[channel], above[3], below[channel]));
        if *pixel != expected {
            wrong += 1;
            first.get_or_insert((at, *below, *above, *pixel, expected));
        }
    }
    assert_eq!(
        wrong, 0,
        "{what}: {wrong} pixels off the compositor's rounding, the first {first:?}"
    );
}

/// Every (s, a, d) with s ≤ a, all 8.4 million of them, through the GPU's
/// `ONE, ONE_MINUS_SRC_ALPHA` into the 8-bit target: float to unorm rounds
/// to nearest, and `d·(255 − a) / 255` is never a tie, so each lands on
/// the compositor's integer answer — as the raster's does.
#[test]
fn every_premultiplied_colour_over_every_ground_lands_as_the_compositor_lays_it() {
    if !device_present() {
        return;
    }
    let (ground, over) = every_combination();
    let height = ground.len() / WIDTH;
    let size = (WIDTH as u32, height as u32);
    let display = DisplayList::from(vec![
        picture(
            (0.0, 0.0),
            (WIDTH, height),
            ImageSource::rgba(1, size, rgba(&ground)),
        ),
        picture(
            (0.0, 0.0),
            (WIDTH, height),
            ImageSource::rgba_premultiplied(2, size, rgba(&over)),
        ),
    ]);
    let images = RawImages::default();
    let mut gpu = OffscreenGpu::new(WIDTH, height).expect("offscreen gpu");
    gpu.present_wait(&display, 1, Color::BLACK, &PixelFont, &images);
    assert_laid(&gpu.read_rgba(), &ground, &over, "metal");
    let cpu = rasterize_with(
        &display,
        WIDTH,
        height,
        1,
        Color::BLACK,
        &PixelFont,
        &images,
    );
    assert_laid(&cpu.to_rgba_bytes(), &ground, &over, "raster");
}

/// Over a translucent target the alpha channel follows the same rule:
/// every alpha over every ground alpha, the ground read back as the GPU
/// left it.
#[test]
fn a_premultiplied_picture_over_a_translucent_ground_lays_its_alpha_alike() {
    if !device_present() {
        return;
    }
    let ground: Vec<[u8; 4]> = (0..=255_u8)
        .flat_map(|below| (0..=255_u8).map(move |_| [255, 128, 37, below]))
        .collect();
    let over: Vec<[u8; 4]> = (0..=255_u8)
        .flat_map(|_| (0..=255_u8).map(|a| [a, a / 2, 0, a]))
        .collect();
    let size = (256, 256);
    let below = picture(
        (0.0, 0.0),
        (256, 256),
        ImageSource::rgba(3, size, rgba(&ground)),
    );
    let above = picture(
        (0.0, 0.0),
        (256, 256),
        ImageSource::rgba_premultiplied(4, size, rgba(&over)),
    );
    let images = RawImages::default();
    let mut gpu = OffscreenGpu::new(256, 256)
        .expect("offscreen gpu")
        .translucent();
    gpu.present_wait(
        &DisplayList::from(vec![below.clone()]),
        1,
        CLEAR,
        &PixelFont,
        &images,
    );
    let laid_ground = gpu.read_rgba().as_chunks::<4>().0.to_vec();
    let display = DisplayList::from(vec![below, above]);
    gpu.present_wait(&display, 1, CLEAR, &PixelFont, &images);
    assert_laid(&gpu.read_rgba(), &laid_ground, &over, "metal");
    let cpu = rasterize_translucent(&display, 256, 256, 1, CLEAR, &PixelFont, &images);
    let cpu_ground = rasterize_translucent(
        &DisplayList::from(vec![picture(
            (0.0, 0.0),
            (256, 256),
            ImageSource::rgba(3, size, rgba(&ground)),
        )]),
        256,
        256,
        1,
        CLEAR,
        &PixelFont,
        &images,
    );
    let cpu_ground = cpu_ground.to_rgba_bytes().as_chunks::<4>().0.to_vec();
    assert_laid(&cpu.to_rgba_bytes(), &cpu_ground, &over, "raster");
}

/// A veil of premultiplied texels, some clear, some opaque, most between.
fn veil(width: usize, height: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    let mut bytes = Vec::with_capacity(width * height * 4);
    for _ in 0..width * height {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let a = match state >> 29 {
            0 => 0,
            1 => 255,
            _ => (state >> 8) as u8,
        };
        let channel = |shift: u32| ((state >> shift) as u8).min(a);
        bytes.extend_from_slice(&[channel(3), channel(11), channel(19), a]);
    }
    bytes
}

fn stripes(width: usize, height: usize) -> Vec<u8> {
    (0..width * height)
        .flat_map(|at| {
            let row = (at / width) as u8;
            [
                row.wrapping_mul(7),
                200_u8.wrapping_sub(row),
                (at % width) as u8,
                255,
            ]
        })
        .collect()
}

/// On every road a picture takes to the GPU — the shared atlas, a texture
/// of its own, the live slot a big picture on the move rides — and cut by
/// a clip, the GPU lays the bytes the raster lays.
#[test]
fn premultiplied_pictures_match_the_raster_on_every_road() {
    if !device_present() {
        return;
    }
    let (width, height) = (700, 600);
    let images = RawImages::default();
    let frame = |big: u64| {
        let ground = ImageSource::rgba(10, (700, 600), stripes(700, 600));
        let small = ImageSource::rgba_premultiplied(11, (90, 60), veil(90, 60, 1));
        let tall = ImageSource::rgba_premultiplied(big, (300, 520), veil(300, 520, big as u32));
        let clip = Rect {
            origin: Point { x: 400.0, y: 30.0 },
            size: Size {
                width: 250.0,
                height: 200.0,
            },
        };
        let clipped = ImageSource::rgba_premultiplied(13, (200, 200), veil(200, 200, 3));
        DisplayList::from(vec![
            picture((0.0, 0.0), (700, 600), ground),
            picture((20.0, 10.0), (90, 60), small.clone()),
            picture((140.0, 40.0), (300, 520), tall),
            DrawCommand::PushClip {
                rect: clip,
                corner_radius: Corners::ZERO,
            },
            picture((350.0, 100.0), (200, 200), clipped),
            DrawCommand::PopClip,
            picture((30.5, 300.25), (90, 60), small),
        ])
    };
    let mut gpu = OffscreenGpu::new(width, height).expect("offscreen gpu");
    for big in [20, 21, 22] {
        let display = frame(big);
        gpu.present_wait(&display, 1, Color::WHITE, &PixelFont, &images);
        let cpu = rasterize_with(
            &display,
            width,
            height,
            1,
            Color::WHITE,
            &PixelFont,
            &images,
        );
        let (shown, oracle) = (gpu.read_rgba(), cpu.to_rgba_bytes());
        let differ = shown
            .as_chunks::<4>()
            .0
            .iter()
            .zip(oracle.as_chunks::<4>().0)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            differ, 0,
            "frame of picture {big}: {differ} pixels apart from the raster"
        );
    }
}
