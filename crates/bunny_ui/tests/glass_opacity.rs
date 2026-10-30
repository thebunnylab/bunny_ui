//! A pane of glass under an opacity lands that much of itself over the
//! scene it reads, down to none.
#![cfg(feature = "canvas")]

use bunny_ui_core::layout::{Color, Glass, Size};
use bunny_ui_core::prelude::*;
use bunny_ui_core::raster::rasterize;

const WIDE: usize = 120;
const TALL: usize = 80;

/// Black and white stripes, with a frosted pane over the middle of them
/// at `opacity` (none: no pane at all).
fn frame(opacity: Option<f64>) -> Vec<u8> {
    let stripes = hstack(
        (0..WIDE / 8)
            .map(|i| {
                rectangle()
                    .foreground_color(if i % 2 == 0 { Color::BLACK } else { Color::WHITE })
                    .frame(8.0, TALL as f64)
            })
            .collect::<Vec<_>>(),
    )
    .spacing(0.0);
    let pane = opacity.map(|opacity| empty().frame(60.0, 40.0).glass(Glass::frosted()).opacity(opacity));
    let view = zstack!(stripes, pane).frame(WIDE as f64, TALL as f64);
    let size = Size {
        width: WIDE as f64,
        height: TALL as f64,
    };
    rasterize(&Runtime::new().display_frame(&view, size), WIDE, TALL, Color::WHITE).to_rgba_bytes()
}

/// The middle row under the pane, every channel.
fn under_the_pane(pixels: &[u8]) -> &[u8] {
    let row = TALL / 2;
    &pixels[(row * WIDE + 36) * 4..(row * WIDE + 84) * 4]
}

#[test]
fn a_pane_fades_from_the_scene_to_itself_and_holds_no_blur_at_none() {
    // a closing panel faded its glass by its blur alone, which stops at
    // the pyramid's floor (5.2 device px): the scene stayed frosted, then
    // snapped clear when the pane went (T2-BUNNY-394)
    let scene = frame(None);
    let whole = frame(Some(1.0));
    assert_ne!(under_the_pane(&scene), under_the_pane(&whole), "a whole pane frosts the stripes");
    assert_eq!(under_the_pane(&frame(Some(0.0))), under_the_pane(&scene), "a pane at none is the scene");

    let half = frame(Some(0.5));
    for ((&got, &clear), &frosted) in under_the_pane(&half)
        .iter()
        .zip(under_the_pane(&scene))
        .zip(under_the_pane(&whole))
    {
        let between = (f64::from(clear) + f64::from(frosted)) / 2.0;
        assert!(
            (f64::from(got) - between).abs() <= 1.5,
            "half a pane is half way between the scene ({clear}) and the pane ({frosted}): {got}"
        );
    }
}

#[test]
fn a_fading_pane_clears_a_little_every_step() {
    // the same pane at opacities a tenth apart: each step moves the pixels
    // under it toward the scene, and the last one lands on it
    let scene = frame(None);
    let distance = |pixels: &[u8]| -> u64 {
        under_the_pane(pixels)
            .iter()
            .zip(under_the_pane(&scene))
            .map(|(&a, &b)| u64::from(a.abs_diff(b)))
            .sum()
    };
    let steps: Vec<u64> = (0..=10).rev().map(|tenths| distance(&frame(Some(f64::from(tenths) / 10.0)))).collect();
    for pair in steps.windows(2) {
        assert!(pair[1] < pair[0], "each step clears more of the pane: {steps:?}");
    }
    assert_eq!(steps[10], 0, "and none of it is left: {steps:?}");
}

#[test]
fn a_material_fades_by_its_own_opacity_and_leaves_what_the_box_draws() {
    // a card's box forms behind words already there: the material fades
    // by its own opacity and the square drawn over it keeps its colour
    let red = Color::rgb(220, 0, 0);
    let card = |opacity: f64| {
        let stripes = hstack(
            (0..WIDE / 8)
                .map(|i| {
                    rectangle()
                        .foreground_color(if i % 2 == 0 { Color::BLACK } else { Color::WHITE })
                        .frame(8.0, TALL as f64)
                })
                .collect::<Vec<_>>(),
        )
        .spacing(0.0);
        let card = zstack!(rectangle().foreground_color(red).frame(8.0, 8.0))
            .frame(60.0, 40.0)
            .glass(Glass::frosted().opacity(opacity));
        let view = zstack!(stripes, card).frame(WIDE as f64, TALL as f64);
        let size = Size {
            width: WIDE as f64,
            height: TALL as f64,
        };
        rasterize(&Runtime::new().display_frame(&view, size), WIDE, TALL, Color::WHITE).to_rgba_bytes()
    };
    let at = |pixels: &[u8], x: usize, y: usize| pixels[(y * WIDE + x) * 4..(y * WIDE + x) * 4 + 3].to_vec();
    let (none, whole, scene) = (card(0.0), card(1.0), frame(None));
    assert_eq!(at(&none, 60, 40), vec![220, 0, 0], "the square keeps its colour at none");
    assert_eq!(at(&whole, 60, 40), vec![220, 0, 0], "and over the whole pane");
    let beside = |pixels: &[u8]| (36..52).map(|x| at(pixels, x, 40)).collect::<Vec<_>>();
    assert_eq!(beside(&none), beside(&scene), "beside it, the material at none is the scene");
    assert_ne!(beside(&whole), beside(&scene), "and the whole material frosts it");
}
