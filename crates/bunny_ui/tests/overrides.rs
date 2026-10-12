//! Overrides against a living runtime: a `Replace` on a modifier's site
//! moves the frame in the next pass, an `Append` on a builder's site
//! paints what it added, a `Hide` keeps the place and paints nothing, and
//! a clear puts the code back in charge — the designer's gesture on the
//! picture before the real edit is compiled.
use bunny_ui_core as bunny_ui;

use bunny_ui::layout::{DisplayList, DrawCommand};
use bunny_ui::modifier::Modifier;
use bunny_ui::prelude::*;
use bunny_ui::provenance::{self, NodeKind, Override, SiteKey};

const SIZE: Size = Size { width: 240.0, height: 120.0 };

#[derive(Clone)]
struct Page;

impl Component for Page {
    fn body(self) -> impl View {
        vstack!(text!("hello").padding_length(4.0)).background_color(theme::canvas())
    }
}

fn text_origin(display: &DisplayList) -> Point {
    display
        .iter()
        .find_map(|command| match command {
            DrawCommand::TextLine { origin, content, .. } if &**content == "hello" => Some(*origin),
            _ => None,
        })
        .expect("hello is painted")
}

fn has_text(display: &DisplayList) -> bool {
    display.iter().any(|command| matches!(command, DrawCommand::TextLine { content, .. } if &**content == "hello"))
}

fn fills(display: &DisplayList) -> Vec<Color> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { color, .. } => Some(*color),
            _ => None,
        })
        .collect()
}

#[test]
fn a_replace_moves_the_frame_an_append_paints_a_hide_hides_and_a_clear_restores() {
    provenance::clear_overrides();
    let runtime = Runtime::new();
    runtime.record_sites(true);
    let before = runtime.display_frame(&Page, SIZE);
    let origin = text_origin(&before);
    let sites = runtime.sites();
    // `.padding_length(..)` is named `padding` on its site, like `.padding()`: one word for the one thing
    let padding = sites.iter().find(|site| site.modifier == Some("padding")).expect("the padding has a site");
    let padding_key = SiteKey::of(padding.site).expect("a site the compiler recorded");
    assert!(padding_key.file.ends_with("overrides.rs"));
    let text_key = SiteKey::of(sites.iter().find(|site| site.kind == NodeKind::Text).expect("the text has a site").site).unwrap();

    // Replace: the 4 px padding stands aside for 24 — the text moves by 20
    provenance::set_overrides([(padding_key.clone(), Override::Replace(Modifier::PaddingLength(24.0)))]);
    assert!(runtime.frame_need().overrides, "a moved table asks for a frame");
    let replaced = runtime.display_frame(&Page, SIZE);
    assert_eq!(text_origin(&replaced), Point { x: origin.x + 20.0, y: origin.y + 20.0 });

    // Append on the builder's site: a background the code never wrote
    let ink = Color { r: 1, g: 2, b: 3, a: 255 };
    provenance::set_overrides([(text_key.clone(), Override::Append(Modifier::BackgroundColor(ink)))]);
    let appended = runtime.display_frame(&Page, SIZE);
    assert!(fills(&appended).contains(&ink), "{:?}", fills(&appended));
    assert_eq!(text_origin(&appended), origin, "the padding is the code's again");

    // Hide: the place kept, nothing painted
    provenance::set_overrides([(text_key, Override::Hide)]);
    let hidden = runtime.display_frame(&Page, SIZE);
    assert!(!has_text(&hidden));

    // clear: the code is back in charge, and the frame after is quiet
    provenance::clear_overrides();
    let restored = runtime.display_frame(&Page, SIZE);
    assert!(has_text(&restored));
    assert_eq!(text_origin(&restored), origin);
    let _ = bunny_ui::stats::take();
    let _ = runtime.display_frame(&Page, SIZE);
    assert_eq!(bunny_ui::stats::take().body_passes, 0, "nothing moved: no body runs");
}
