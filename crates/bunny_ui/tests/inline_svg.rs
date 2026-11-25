use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 200.0, height: 100.0 };

const SKY: &str = r##"<svg viewBox="0 0 4 2" preserveAspectRatio="xMidYMin slice">
    <defs><linearGradient id="sky"><stop offset="0" stop-color="#0E121C"/><stop offset="1" stop-color="#2A364C"/></linearGradient></defs>
    <mask id="veil"><rect width="4" height="2" fill="white"/></mask>
    <rect width="4" height="2" fill="url(#sky)" mask="url(#veil)"/>
</svg>"##;

fn sky() -> ImageSource {
    ImageSource::from_bytes(SKY.as_bytes().to_vec())
}

/// The ids an `<svg data-n=…>` of the page declares, and the references
/// it makes, by element.
fn drawings(html: &str) -> Vec<(String, Vec<String>, Vec<String>)> {
    html.match_indices("<svg data-n=\"")
        .map(|(at, open)| {
            let rest = &html[at + open.len()..];
            let n = rest.split('"').next().expect("a number").to_string();
            let body = &rest[..rest.find("</svg>").expect("closed")];
            let ids = body
                .match_indices(" id=\"")
                .map(|(at, open)| body[at + open.len()..].split('"').next().unwrap_or_default().to_string())
                .collect();
            let refs = body
                .match_indices("url(#")
                .map(|(at, open)| body[at + open.len()..].split(')').next().unwrap_or_default().to_string())
                .collect();
            (n, ids, refs)
        })
        .collect()
}

/// Two copies of one drawing on one page each own their gradient and
/// their mask: the ids are the element's, and every reference names an
/// id of its own drawing.
#[test]
fn two_copies_of_a_drawing_never_share_a_gradient_or_a_mask() {
    let view = hstack!(
        image(sky()).inline_svg().resizable().frame(80.0, 40.0),
        image(sky()).inline_svg().resizable().frame(80.0, 40.0),
    );
    let page = bunny_ui::ssr::render(&view, SIZE);
    let served = drawings(&page.html);
    assert_eq!(served.len(), 2, "{}", page.html);
    let (first, second) = (&served[0], &served[1]);
    assert_ne!(first.0, second.0);
    for (n, ids, refs) in &served {
        assert_eq!(ids, &[format!("bunny-{n}-0"), format!("bunny-{n}-1")], "{}", page.html);
        assert_eq!(refs, ids, "each reference names its own drawing's id");
    }
    assert!(first.1.iter().all(|id| !second.1.contains(id)));
}

/// The modifier is the element tiers' alone: the pixel pipelines lay out
/// and draw the same scene with it or without it.
#[test]
fn the_pixel_tiers_draw_the_picture_either_way() {
    let drawn = |inline: bool| {
        let picture = image(sky());
        let picture = if inline { picture.inline_svg() } else { picture };
        let view = vstack!(picture.resizable().frame(80.0, 40.0).test_id("sky"), rectangle().frame(10.0, 10.0));
        let runtime = Runtime::new();
        let display: Vec<DrawCommand> = runtime.display_frame(&view, SIZE).iter().cloned().collect();
        (display, runtime.test_frames())
    };
    let (display, frames) = drawn(false);
    assert!(frames.iter().any(|(id, _)| &**id == "sky"));
    let (inline_display, inline_frames) = drawn(true);
    assert!(display == inline_display);
    assert_eq!(frames, inline_frames);
}

/// Without the modifier a picture is the `<img>` it always was, waiting
/// for its bytes by identity.
#[test]
fn a_picture_without_the_modifier_stays_an_img() {
    let page = bunny_ui::ssr::render(&image(sky()).resizable().frame(80.0, 40.0), SIZE);
    assert!(page.html.contains("<img "), "{}", page.html);
    assert!(!page.html.contains("<svg"), "{}", page.html);
}
