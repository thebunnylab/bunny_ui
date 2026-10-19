//! Foreground blur must composite actual pixels and preserve source geometry.
#![cfg(feature = "canvas")]

use bunny_ui_core::layout::{Color, Size};
use bunny_ui_core::prelude::*;
use bunny_ui_core::raster::rasterize;

fn scene(radius: f64) -> impl UnaryView {
    rectangle()
        .frame(20.0, 20.0)
        .foreground_color(Color::rgb(255, 0, 0))
        .blur(radius)
        .padding_length(20.0)
}

#[test]
fn blur_spreads_paint_without_changing_layout_or_source_sites() {
    let size = Size {
        width: 60.0,
        height: 60.0,
    };
    let ordinary = Runtime::scene("blur");
    let sharp = ordinary.display_frame(&scene(0.0), size);
    let soft = ordinary.display_frame(&scene(3.0), size);
    let clear = Color::rgba(0, 0, 0, 0);
    let sharp = rasterize(&sharp, 60, 60, clear).to_rgba_bytes();
    let soft = rasterize(&soft, 60, 60, clear).to_rgba_bytes();
    assert!(sharp != soft, "a supported blur must change pixels");
    let edge = (30 * 60 + 18) * 4;
    assert!(
        soft[edge + 3] > 0,
        "blur must spread past the original edge"
    );
    let center = (30 * 60 + 30) * 4;
    assert!(soft[center + 3] > 240, "the interior stays opaque");
    ordinary.record_sites(true);
    let recorded = ordinary.display_frame(&scene(3.0), size);
    assert_eq!(soft, rasterize(&recorded, 60, 60, clear).to_rgba_bytes());
    assert!(
        ordinary
            .sites()
            .iter()
            .any(|site| site.modifier == Some("blur"))
    );
    ordinary.record_sites(false);
}

#[test]
fn zero_negative_and_nonfinite_blurs_are_inert() {
    let runtime = Runtime::new();
    let size = Size {
        width: 60.0,
        height: 60.0,
    };
    let clear = Color::rgba(0, 0, 0, 0);
    let pixels = |radius| {
        rasterize(&runtime.display_frame(&scene(radius), size), 60, 60, clear).to_rgba_bytes()
    };
    let sharp = pixels(0.0);
    for radius in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(sharp, pixels(radius));
    }
}

fn paint(view: &impl View) -> Vec<u8> {
    let display = Runtime::new().display_frame(
        view,
        Size {
            width: 60.0,
            height: 60.0,
        },
    );
    rasterize(&display, 60, 60, Color::rgba(0, 0, 0, 0)).to_rgba_bytes()
}

#[test]
fn blur_preserves_transparent_color_and_modifier_order() {
    let red = || {
        rectangle()
            .frame(20.0, 20.0)
            .foreground_color(Color::rgba(255, 0, 0, 128))
    };
    let soft = paint(&red().blur(3.0).padding_length(20.0));
    for pixel in soft.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] > 0) {
        assert_eq!(pixel[1], 0);
        assert_eq!(pixel[2], 0);
        assert!(
            pixel[0].abs_diff(pixel[3]) <= 1,
            "red must not acquire a dark halo"
        );
    }
    let clipped = paint(&red().blur(3.0).clipped().padding_length(20.0));
    assert_eq!(clipped[(30 * 60 + 18) * 4 + 3], 0);
    let before = paint(&red().clipped().blur(3.0).padding_length(20.0));
    assert!(before[(30 * 60 + 18) * 4 + 3] > 0);
    let background = paint(
        &red()
            .blur(3.0)
            .background_color(Color::rgb(0, 255, 0))
            .padding_length(20.0),
    );
    assert_eq!(
        background[(20 * 60 + 20) * 4 + 3],
        255,
        "outer background stays sharp"
    );
}

#[test]
fn blur_composites_overlapping_children_and_nested_effects() {
    let stack = || {
        zstack!(
            rectangle().foreground_color(Color::rgb(255, 0, 0)),
            rectangle().foreground_color(Color::rgba(0, 0, 255, 128)),
        )
        .frame(20.0, 20.0)
    };
    let pixels = paint(&stack().blur(2.0).padding_length(20.0));
    let center = (30 * 60 + 30) * 4;
    assert_eq!(&pixels[center..center + 4], &[127, 0, 128, 255]);
    let twice = paint(&stack().blur(2.0).blur(2.0).padding_length(20.0));
    assert!(pixels != twice, "two effects must not fold into one");
}

#[test]
fn blur_keeps_click_geometry_and_caps_positive_radius() {
    let runtime = Runtime::scene("click_blur");
    let size = Size {
        width: 60.0,
        height: 60.0,
    };
    let view = |radius| {
        rectangle()
            .frame(20.0, 20.0)
            .on_click(|| {})
            .blur(radius)
            .padding_length(20.0)
    };
    runtime.display_frame(&view(0.0), size);
    let sharp = runtime.hits_snapshot();
    runtime.display_frame(&view(3.0), size);
    assert_eq!(sharp, runtime.hits_snapshot());
    assert!(!sharp.is_empty());
    assert!(paint(&scene(128.0)) == paint(&scene(10000.0)));
}

#[test]
fn dom_and_ssr_keep_native_elements_and_filter_changes() {
    let size = Size {
        width: 60.0,
        height: 60.0,
    };
    let view = |radius| text("still selectable").blur(radius);
    let page = bunny_ui_core::ssr::render(&view(3.0), size);
    assert!(page.css.contains("filter:blur(3px)"));
    assert!(page.html.contains("still selectable"));
    let runtime = Runtime::new();
    let first = runtime.dom_frame(&view(3.0), size);
    let second = runtime.dom_frame(&view(5.0), size);
    assert!(!first.is_empty());
    assert!(
        !second.is_empty(),
        "filter changes must invalidate the style rule"
    );
    let bytes = bunny_ui_core::dom::encode(&second);
    assert!(!bytes.is_empty());
}

#[test]
fn looping_custom_paint_inside_blur_requests_a_scene_frame() {
    use bunny_ui_core::anim::Loop;
    let runtime = Runtime::new();
    #[derive(Clone, Copy)]
    struct Animated;
    impl Component for Animated {
        fn body(self) -> impl View {
            canvas(|ctx, painter| {
                painter.fill(
                    bunny_ui_core::layout::Rect {
                        origin: Default::default(),
                        size: ctx.size(),
                    },
                    Color::rgb((ctx.phase * 255.0) as u8, 0, 0),
                );
            })
            .frame(20.0, 20.0)
            .looping(Loop::secs(1.0).fps(4.0))
            .blur(2.0)
        }
    }
    let view = Animated;
    let before = runtime.display_frame(
        &view,
        Size {
            width: 20.0,
            height: 20.0,
        },
    );
    assert!(
        runtime.live_paths().is_empty(),
        "an independent layer would bypass the blur"
    );
    assert!(
        runtime.tick(0.3).scene,
        "the composite must advance with its child"
    );
    let after = runtime.animation_frame(
        &view,
        Size {
            width: 20.0,
            height: 20.0,
        },
    );
    assert!(
        before.as_slice() != after.as_slice(),
        "the filtered pixels must advance too"
    );
}

#[test]
fn blur_respects_device_scale_and_keeps_scrollbars_in_the_composite() {
    let size = Size {
        width: 60.0,
        height: 60.0,
    };
    let runtime = Runtime::new();
    let sample = |scale: usize| {
        runtime.set_device_scale(scale as f64);
        let display = runtime.display_frame(&scene(3.0), size);
        let bitmap = bunny_ui_core::raster::rasterize_scaled(
            &display,
            60 * scale,
            60 * scale,
            scale,
            Color::rgba(0, 0, 0, 0),
        );
        bitmap.to_rgba_bytes()[(30 * scale * 60 * scale + 18 * scale) * 4 + 3]
    };
    assert!(sample(1).abs_diff(sample(2)) < 20);
    runtime.drop_unseen();
    runtime.set_thumb_layers(true);
    let view = scroll(text("Tall").frame_height(300.0))
        .blur(3.0)
        .frame(60.0, 60.0);
    runtime.display_frame(&view, size);
    assert!(
        runtime.thumbs().is_empty(),
        "a separate sharp thumb would bypass the effect"
    );
}

#[test]
fn a_clipped_multichild_blur_matches_full_and_culled_placement() {
    let view = zstack!(
        rectangle().foreground_color(Color::rgb(255, 0, 0)),
        text("Inside"),
    )
    .frame(30.0, 30.0)
    .blur(3.0)
    .clipped()
    .padding_length(15.0);
    let size = Size {
        width: 60.0,
        height: 60.0,
    };
    let full = Runtime::new();
    let cut = Runtime::new();
    cut.drop_unseen();
    let full = full.display_frame(&view, size);
    let cut = cut.display_frame(&view, size);
    assert_eq!(
        rasterize(&full, 60, 60, Color::WHITE).to_rgba_bytes(),
        rasterize(&cut, 60, 60, Color::WHITE).to_rgba_bytes()
    );
}

#[test]
fn native_hosts_keep_their_paint_order_and_geometry_inside_blur() {
    #[derive(Clone, Copy)]
    struct Hosted;
    impl Component for Hosted {
        fn body(self) -> impl View {
            zstack!(
                rectangle().foreground_color(Color::rgb(255, 0, 0)),
                bunny_ui_core::host::webview("https://example.test/"),
                text("Above the native surface"),
            )
            .frame(40.0, 40.0)
            .clipped()
            .blur(2.0)
            .padding_length(10.0)
        }
    }
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let display = runtime.display_frame(
        &Hosted,
        Size {
            width: 60.0,
            height: 60.0,
        },
    );
    let hosts = runtime.hosts();
    assert_eq!(hosts.len(), 1);
    assert_eq!(
        hosts[0].frame.size,
        Size {
            width: 40.0,
            height: 40.0
        }
    );
    assert_eq!(
        hosts[0].mark, 1,
        "the background stays beneath the native host"
    );
    assert_eq!(
        display.len(),
        2,
        "the two framework paint layers stay separate"
    );
    assert_eq!(runtime.host_segments(display.len())[0].1, (1, 2));
}

// Decoded effect pixels must not depend on a platform decoder understanding
// the headless RawImages envelope. Native ImageIO only accepts real codecs.
#[test]
fn computed_blur_pixels_need_no_encoded_image_decoder() {
    use bunny_ui_core::image_engine::{ImageEngine, ImageRaster, ImageSource};
    use bunny_ui_core::raster::rasterize_with;
    use bunny_ui_core::text_engine::PixelFont;
    use std::rc::Rc;

    struct NoEncodedImages;
    impl ImageEngine for NoEncodedImages {
        fn intrinsic(&self, _: &ImageSource) -> Option<(u32, u32)> {
            None
        }
        fn raster(&self, _: &ImageSource, _: usize, _: usize) -> Option<Rc<ImageRaster>> {
            None
        }
    }
    let runtime = Runtime::new().image_engine(Rc::new(NoEncodedImages));
    let display = runtime.display_frame(
        &scene(3.0),
        Size {
            width: 60.0,
            height: 60.0,
        },
    );
    let clear = Color::rgba(0, 0, 0, 0);
    let expected = rasterize(&display, 60, 60, clear).to_rgba_bytes();
    assert!(expected.as_chunks::<4>().0.iter().any(|p| p[3] > 240));
    let actual =
        rasterize_with(&display, 60, 60, 1, clear, &PixelFont, &NoEncodedImages).to_rgba_bytes();
    assert!(
        actual == expected,
        "computed pixels are independent of platform codecs"
    );
}
