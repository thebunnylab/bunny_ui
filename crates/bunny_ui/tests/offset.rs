use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 200.0, height: 200.0 };

fn frame(runtime: &Runtime, name: &str) -> Rect {
    runtime.test_frames().into_iter().find(|(id, _)| &**id == name).map(|(_, rect)| rect).expect(name)
}

/// `.offset(x, y)` draws the view away from its place — what the view
/// draws and where it is found move together — and the layout around it
/// never hears of it: the next box stands where it stood.
#[test]
fn an_offset_view_is_drawn_away_and_its_neighbours_stay() {
    let runtime = Runtime::new();
    let view = vstack!(
        rectangle().frame(40.0, 20.0).test_id("moved").offset(6.0, -4.0),
        rectangle().frame(40.0, 20.0).test_id("next"),
    );
    runtime.display_frame(&view, SIZE);
    assert_eq!(frame(&runtime, "moved").origin, Point { x: 6.0, y: -4.0 });
    assert_eq!(frame(&runtime, "next").origin, Point { x: 0.0, y: 20.0 });
}

/// `.shadow_offset(x, y)` moves the halo, never the box: a light from
/// above casts the shadow down.
#[test]
fn a_shadow_offset_moves_the_halo_alone() {
    let runtime = Runtime::new();
    let view = rectangle().frame(40.0, 20.0).shadow_color(8.0, Color::rgba(0, 0, 0, 64)).shadow_offset(0.0, 10.0).test_id("card");
    let display = runtime.display_frame(&view, SIZE);
    let halo = display
        .iter()
        .find_map(|command| match command {
            DrawCommand::Shadow { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("a halo");
    let card = frame(&runtime, "card");
    assert_eq!(halo.origin, Point { x: card.origin.x, y: card.origin.y + 10.0 });
    assert_eq!(halo.size, card.size);
}

/// The served page says both: the halo's layers carry the offset, the
/// box rides CSS `translate`, which its neighbours never feel.
#[test]
fn the_page_spells_both_offsets() {
    let page = bunny_ui::ssr::render(
        &vstack!(
            text("lifted").shadow_color(10.0, Color::rgba(0, 0, 0, 128)).shadow_offset(2.0, 6.0),
            text("moved").offset(-3.0, 4.5),
        ),
        SIZE,
    );
    assert!(page.css.contains("box-shadow:2px 6px 5.124px"), "{}", page.css);
    assert!(page.css.contains("translate:-3px 4.5px"), "{}", page.css);
}

/// A negative padding is SwiftUI's way to overlap: the content moves out
/// past the box's edge and the box gives the flow that much back — an
/// avatar pulled up over the cover above it, the row below following.
#[test]
fn a_negative_padding_overlaps_the_box_above() {
    let runtime = Runtime::new();
    let view = vstack!(
        rectangle().frame(100.0, 60.0).test_id("cover"),
        rectangle().frame(40.0, 40.0).test_id("avatar").padding_edge(Edge::Top, -20.0),
        rectangle().frame(100.0, 10.0).test_id("below"),
    )
    .alignment(HorizontalAlignment::Leading);
    runtime.display_frame(&view, SIZE);
    assert_eq!(frame(&runtime, "avatar").origin.y, 40.0, "pulled up over the cover");
    // the avatar takes 40 - 20 in the flow: the next box starts at 80
    assert_eq!(frame(&runtime, "below").origin.y, 80.0, "and the flow gave back the overlap");
}

/// CSS has no negative padding: the page writes a negative side as the
/// negative margin that means the same.
#[test]
fn the_page_writes_a_negative_padding_as_a_margin() {
    let page = bunny_ui::ssr::render(&vstack!(text("pulled").padding_edge(Edge::Top, -20.0), text("below")), SIZE);
    assert!(page.css.contains("margin-block-start:-20px"), "{}", page.css);
    assert!(page.css.contains("isolation:isolate"), "painted whole over the box above: {}", page.css);
    assert!(!page.css.contains("padding-block:-20px"), "{}", page.css);
}

/// Two offsets add up, as SwiftUI's do.
#[test]
fn offsets_add_up() {
    let runtime = Runtime::new();
    runtime.display_frame(&rectangle().frame(10.0, 10.0).test_id("moved").offset(1.0, 2.0).offset(3.0, 4.0), SIZE);
    assert_eq!(frame(&runtime, "moved").origin, Point { x: 4.0, y: 6.0 });
}

/// A negative spacing overlaps the items — an avatar stack, each face
/// over the one before — and CSS has no negative gap: the page pulls
/// each item after the first back by a margin along the line.
#[test]
fn the_page_writes_a_negative_spacing_as_margins() {
    let runtime = Runtime::new();
    let faces = hstack!(
        rectangle().frame(30.0, 30.0).test_id("first"),
        rectangle().frame(30.0, 30.0).test_id("second"),
        rectangle().frame(30.0, 30.0).test_id("third"),
    )
    .spacing(-9.0);
    runtime.display_frame(&faces, SIZE);
    assert_eq!(frame(&runtime, "third").origin.x, 42.0);
    let page = bunny_ui::ssr::render(&faces, SIZE);
    assert!(page.css.contains(">*+*{margin-inline-start:-9px;isolation:isolate}"), "{}", page.css);
    assert!(!page.css.contains("gap:-9px"), "{}", page.css);
    let column = bunny_ui::ssr::render(&vstack!(text("a"), text("b")).spacing(-4.0), SIZE);
    assert!(column.css.contains(">*+*{margin-block-start:-4px;isolation:isolate}"), "{}", column.css);
}

/// A layer larger than its base keeps its size and overflows the box,
/// as `SwiftUI`'s does — a banner bled past the page's edges, a glow
/// behind a button — and the box keeps its own.
#[test]
fn a_layer_larger_than_its_base_keeps_its_size() {
    let runtime = Runtime::new();
    let view = vstack!(
        rectangle()
            .frame(100.0, 40.0)
            .overlay(UnitPoint::TOP_LEADING, rectangle().frame(140.0, 10.0).test_id("band"))
            .test_id("box"),
        rectangle().frame(100.0, 40.0).overlay(UnitPoint::CENTER, rectangle().frame(140.0, 60.0).test_id("glow")),
    )
    .alignment(HorizontalAlignment::Leading);
    runtime.display_frame(&view, SIZE);
    assert_eq!(frame(&runtime, "box").size, Size { width: 100.0, height: 40.0 });
    assert_eq!(frame(&runtime, "band"), Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: 140.0, height: 10.0 } });
    assert_eq!(frame(&runtime, "glow"), Rect { origin: Point { x: -20.0, y: 30.0 }, size: Size { width: 140.0, height: 60.0 } });
}
