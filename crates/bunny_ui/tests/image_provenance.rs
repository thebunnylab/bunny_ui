//! Source recording must not change image modifiers or their painted geometry.
#![cfg(feature = "canvas")]

use bunny_ui_core::layout::{DrawCommand, Size};
use bunny_ui_core::prelude::*;
use bunny_ui_core::provenance::NodeKind;

fn pixels() -> impl UnaryView {
    image(ImageSource::rgba(
        20181019,
        (3, 1),
        vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
    ))
}

#[test]
fn recorded_image_modifiers_keep_resize_fit_fill_and_source_sites() {
    for mode in [ContentMode::Fit, ContentMode::Fill] {
        let view = pixels()
            .resizable()
            .frame(180.0, 120.0)
            .background_color(Color::rgb(30, 30, 30))
            .clipped()
            .aspect_ratio(mode);
        let size = Size {
            width: 180.0,
            height: 120.0,
        };
        let ordinary = Runtime::new().display_frame(&view, size);
        let watched = Runtime::scene("image_provenance");
        watched.record_sites(true);
        let recorded = watched.display_frame(&view, size);
        // Styled nodes can fold with recording off, moving a background
        // across an equivalent clip. Compare the image's actual geometry.
        let image_commands = |display: &bunny_ui_core::layout::DisplayList| {
            display
                .as_slice()
                .iter()
                .filter(|command| matches!(command, DrawCommand::Image { .. }))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            image_commands(&ordinary),
            image_commands(&recorded),
            "{mode:?}"
        );
        let image_rect = recorded
            .as_slice()
            .iter()
            .find_map(|command| match command {
                DrawCommand::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("painted image");
        let expected = match mode {
            ContentMode::Fit => Size {
                width: 180.0,
                height: 60.0,
            },
            ContentMode::Fill => Size {
                width: 360.0,
                height: 120.0,
            },
        };
        assert_eq!(image_rect.size, expected);
        let sites = watched.sites();
        assert!(sites.iter().any(|site| site.kind == NodeKind::Image));
        for modifier in [
            "resizable",
            "frame",
            "background_color",
            "clipped",
            "aspect_ratio",
        ] {
            assert!(
                sites.iter().any(|site| site.modifier == Some(modifier)),
                "{modifier}"
            );
        }
        assert!(sites.iter().all(|site| site.mounted.is_some()));
        watched.record_sites(false);
    }
}

#[test]
fn recorded_resizable_icons_keep_their_frame_and_source() {
    let view = icon(bunny_ui_core::icon::house::CHEVRON_RIGHT)
        .padding_length(2.0)
        .resizable()
        .frame(80.0, 60.0);
    let size = Size {
        width: 80.0,
        height: 60.0,
    };
    let plain = Runtime::new().display_frame(&view, size);
    let watched = Runtime::scene("icon_provenance");
    watched.record_sites(true);
    let recorded = watched.display_frame(&view, size);
    assert_eq!(plain.as_slice(), recorded.as_slice());
    let site = watched
        .sites()
        .into_iter()
        .find(|site| site.kind == NodeKind::Icon)
        .expect("icon site");
    assert_eq!(
        site.frame.size,
        Size {
            width: 76.0,
            height: 56.0
        }
    );
    assert!(site.mounted.is_some());
    watched.record_sites(false);
}

#[test]
fn recording_preserves_pixel_rewrite_action_hint_and_overlay_boundaries() {
    let size = Size {
        width: 180.0,
        height: 120.0,
    };
    fn assert_parity(view: &impl UnaryView) -> Vec<Size> {
        let size = Size {
            width: 180.0,
            height: 120.0,
        };
        let image_sizes = |recording| {
            let runtime = Runtime::new();
            runtime.record_sites(recording);
            let display = runtime.display_frame(view, size);
            let images: Vec<_> = display
                .as_slice()
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::Image { rect, .. } => Some(rect.size),
                    _ => None,
                })
                .collect();
            runtime.record_sites(false);
            images
        };
        let ordinary = image_sizes(false);
        assert_eq!(ordinary, image_sizes(true));
        ordinary
    }
    assert_parity(&pixels().on_click(|| {}).resizable().frame(180.0, 120.0));
    assert_parity(&pixels().element("img").resizable().frame(180.0, 120.0));
    let overlay = assert_parity(
        &pixels()
            .overlay(UnitPoint::CENTER, pixels())
            .resizable()
            .frame(180.0, 120.0),
    );
    assert_eq!(
        overlay,
        [
            size,
            Size {
                width: 3.0,
                height: 1.0
            }
        ]
    );
}
