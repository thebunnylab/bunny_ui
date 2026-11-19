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
