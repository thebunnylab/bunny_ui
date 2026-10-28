//! The same source shapes fill native layouts and retain vector geometry.
#![cfg(feature = "canvas")]

use bunny_ui_core::image_engine::raster_source;
use bunny_ui_core::layout::DrawCommand;
use bunny_ui_core::prelude::*;

#[test]
fn rectangle_uses_explicit_ink_and_preserves_its_unstyled_fill() {
    let size = Size {
        width: 200.0,
        height: 80.0,
    };
    let plain = Runtime::new().display_frame(&rectangle(), size);
    assert!(
        matches!(plain.as_slice(), [DrawCommand::FillRect { color, rect, .. }]
        if *color == Color::FILL && rect.size == size)
    );
    let ink = Color::hex(0x226644);
    let tinted = Runtime::new().display_frame(&rectangle().foreground_color(ink), size);
    assert!(
        matches!(tinted.as_slice(), [DrawCommand::FillRect { color, rect, .. }]
        if *color == ink && rect.size == size)
    );
}

#[test]
fn ellipse_fills_both_axes_and_is_an_ellipse_instead_of_a_pill() {
    for (width, height) in [(200_u32, 80_u32), (80, 200)] {
        let size = Size {
            width: f64::from(width),
            height: f64::from(height),
        };
        let runtime = Runtime::new();
        runtime.record_sites(true);
        let display = runtime.display_frame(
            &ellipse()
                .foreground_color(Color::hex(0x226644))
                .frame(size.width, size.height),
            size,
        );
        assert!(
            runtime
                .sites()
                .iter()
                .any(|site| site.kind.name() == "ellipse" && site.frame.size == size)
        );
        let [DrawCommand::Image { rect, source, .. }] = display.as_slice() else {
            panic!("ellipse must use the existing vector image pipeline: {display:?}");
        };
        assert_eq!(rect.size, size);
        assert!(matches!(source, ImageSource::Path { .. }));
        let raster = raster_source(
            &RawImages::default(),
            source,
            width as usize,
            height as usize,
        )
        .expect("vector pixels");
        let pixel = |x, y| &raster.rgba[(y * width as usize + x) * 4..][..4];
        assert_eq!(
            pixel(width as usize / 2, height as usize / 2),
            &[0x22, 0x66, 0x44, 255]
        );
        assert_eq!(pixel(0, 0)[3], 0);
        let (x, y) = if width > height { (20, 5) } else { (5, 20) };
        assert_eq!(
            pixel(x, y)[3],
            0,
            "this point is inside a pill but outside the ellipse"
        );
        runtime.record_sites(false);
    }
}

#[test]
fn ellipse_empty_sizes_emit_no_nonfinite_vector_and_ink_changes_identity() {
    for size in [
        Size {
            width: 0.0,
            height: 80.0,
        },
        Size {
            width: 80.0,
            height: 0.0,
        },
    ] {
        let display = Runtime::new().display_frame(&ellipse(), size);
        assert!(display.is_empty());
    }
    let image = |ink| {
        let display = Runtime::new().display_frame(
            &ellipse().foreground_color(ink),
            Size {
                width: 200.0,
                height: 80.0,
            },
        );
        match &display.as_slice()[0] {
            DrawCommand::Image { source, .. } => source.key(),
            other => panic!("{other:?}"),
        }
    };
    assert_ne!(image(Color::BLACK), image(Color::WHITE));
}

#[test]
fn served_shapes_keep_responsive_axes_and_inherit_hover_ink() {
    let page = bunny_ui_core::ssr::render(
        &hstack!(rectangle().frame(200.0, 80.0), ellipse().frame(80.0, 200.0))
            .foreground_color(Color::hex(0x226644))
            .foreground_hovered(Color::WHITE),
        Size {
            width: 400.0,
            height: 240.0,
        },
    );
    assert!(page.css.contains("background:currentColor"), "{}", page.css);
    assert!(page.css.contains("border-radius:50%"), "{}", page.css);
    assert!(page.css.contains(":hover"), "{}", page.css);
    assert!(page.html.contains("width:200px") && page.html.contains("height:80px"));
    assert!(!page.html.contains("<canvas"));
    assert!(!page.html.contains("<img"));
    let plain = bunny_ui_core::ssr::render(
        &ellipse(),
        Size {
            width: 200.0,
            height: 80.0,
        },
    );
    assert!(plain.css.contains("background:currentColor"));
    assert!(plain.css.contains("color:"));
}
