//! A picture that takes its offer, inside the box its clip or its
//! corners make, on the page.

use bunny_ui_core::prelude::*;

const SIZE: Size = Size { width: 300.0, height: 200.0 };

/// A box around a resizable picture is laid out as the picture: it takes
/// the frame's offer and stretches across it, or the box keeps the
/// picture's natural width and the picture covers that, cropped to a
/// corner of itself.
#[test]
fn a_clipped_picture_takes_its_frame_across() {
    let picture = image(ImageSource::rgba(20_181_019, (8, 3), vec![255; 8 * 3 * 4]));
    let page = bunny_ui_core::ssr::render(
        &picture.resizable().aspect_ratio(ContentMode::Fill).clipped().frame_aligned(160.0, 50.0, Alignment::TopLeading),
        SIZE,
    );
    let clip = page.css.lines().find(|rule| rule.contains("overflow:hidden")).expect("the clip's rule");
    assert!(clip.contains("align-self:stretch") && clip.contains("flex:1 1 auto"), "{clip}");
}
