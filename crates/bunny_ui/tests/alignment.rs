//! Two-axis alignment: the top and bottom rows of `Alignment` hold a
//! child against an edge in frames, layered stacks and grid cells, and
//! the middle row centres vertically as it always did.
use bunny_ui::layout::{DrawCommand, Proposal};
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const MARK: Color = Color::hex(0xFF6600);

/// Where the 10 × 10 mark painted, in the layout's coordinates.
fn mark_at(display: &bunny_ui::layout::DisplayList) -> Point {
    display
        .iter()
        .find_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } if *color == MARK => Some(rect.origin),
            _ => None,
        })
        .expect("the mark paints")
}

fn mark() -> impl View<Arity = Single> {
    spacer().frame(10.0, 10.0).background_color(MARK)
}

fn exact(width: f64, height: f64) -> Proposal {
    Proposal::exact(Size { width, height })
}

#[test]
fn a_frame_holds_its_child_against_the_edges_it_names() {
    let at = |alignment| {
        let view = mark().frame_aligned(100.0, 60.0, alignment);
        mark_at(&Runtime::new().layout(&view, exact(100.0, 60.0)).display)
    };
    for (alignment, x, y) in [
        (Alignment::TopLeading, 0.0, 0.0),
        (Alignment::Top, 45.0, 0.0),
        (Alignment::TopTrailing, 90.0, 0.0),
        // the middle row centres vertically, as it did before it had a name
        (Alignment::Leading, 0.0, 25.0),
        (Alignment::Center, 45.0, 25.0),
        (Alignment::Trailing, 90.0, 25.0),
        (Alignment::BottomLeading, 0.0, 50.0),
        (Alignment::Bottom, 45.0, 50.0),
        (Alignment::BottomTrailing, 90.0, 50.0),
    ] {
        assert_eq!(at(alignment), Point { x, y }, "{alignment}");
    }
}

#[test]
fn a_filling_frame_holds_its_child_at_the_top_it_names() {
    // the page idiom: content that reads from the top of a pane taller
    // than itself, with no spacer underneath to push it there
    let at = |alignment| {
        let view = mark().frame_max(f64::INFINITY, f64::INFINITY, alignment);
        mark_at(&Runtime::new().layout(&view, exact(200.0, 300.0)).display)
    };
    assert_eq!(at(Alignment::TopLeading), Point { x: 0.0, y: 0.0 });
    assert_eq!(at(Alignment::Leading), Point { x: 0.0, y: 145.0 });
    assert_eq!(at(Alignment::BottomTrailing), Point { x: 190.0, y: 290.0 });
}

#[test]
fn a_frame_width_keeps_the_edge_on_its_own_axis() {
    // a frame that pins only the width is as tall as its child: there is
    // no room above or below to place it in, whatever the row says
    let view = mark().frame_width_aligned(100.0, Alignment::BottomTrailing);
    let result = Runtime::new().layout(&view, Proposal { width: Some(100.0), height: None });
    assert_eq!(result.size, Size { width: 100.0, height: 10.0 });
    assert_eq!(mark_at(&result.display), Point { x: 90.0, y: 0.0 });
}

#[test]
fn a_layered_stack_takes_the_corner_it_was_given() {
    let at = |alignment| {
        let pile = zstack!(spacer(), mark()).alignment(alignment);
        mark_at(&Runtime::new().layout(&pile, exact(120.0, 60.0)).display)
    };
    assert_eq!(at(Alignment::TopLeading), Point { x: 0.0, y: 0.0 });
    assert_eq!(at(Alignment::TopTrailing), Point { x: 110.0, y: 0.0 });
    assert_eq!(at(Alignment::Leading), Point { x: 0.0, y: 25.0 });
    assert_eq!(at(Alignment::Bottom), Point { x: 55.0, y: 50.0 });
}

#[test]
fn a_layered_corner_follows_the_reading_direction() {
    // a leading corner is the right one when the scene reads right to left
    let pile = zstack!(spacer(), mark()).alignment(Alignment::TopLeading);
    let runtime = Runtime::new();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let display = runtime.layout(&pile, exact(120.0, 60.0)).display;
    assert_eq!(mark_at(&display), Point { x: 110.0, y: 0.0 });
}

#[test]
fn a_grid_cell_holds_a_short_child_against_the_edge_it_names() {
    let at = |alignment| {
        let view = grid!(spacer().frame(10.0, 40.0), mark()).columns(2).alignment(alignment);
        mark_at(&Runtime::new().layout(&view, Proposal { width: Some(100.0), height: None }).display)
    };
    assert_eq!(at(Alignment::TopLeading), Point { x: 50.0, y: 0.0 });
    assert_eq!(at(Alignment::Leading), Point { x: 50.0, y: 15.0 });
    assert_eq!(at(Alignment::BottomLeading), Point { x: 50.0, y: 30.0 });
}

#[test]
fn served_alignment_names_its_vertical_edge_only_when_it_has_one() {
    let css = |view: &dyn Fn() -> bunny_ui::ssr::SsrPage| view().css;
    let page = |alignment| {
        move || {
            bunny_ui::ssr::render(
                &mark().frame_aligned(100.0, 60.0, alignment),
                Size { width: 100.0, height: 60.0 },
            )
        }
    };
    // the middle row lowers as it always did: the flow's own placement
    let middle = css(&page(Alignment::Leading));
    assert!(!middle.contains("justify-content"), "{middle}");
    let bottom = css(&page(Alignment::BottomTrailing));
    assert!(bottom.contains("justify-content:flex-end"), "{bottom}");
    let top = css(&page(Alignment::Top));
    assert!(top.contains("justify-content:flex-start"), "{top}");

    let layers = bunny_ui::ssr::render(
        &zstack!(spacer(), mark()).alignment(Alignment::BottomLeading),
        Size { width: 120.0, height: 60.0 },
    );
    assert!(layers.css.contains("align-items:flex-end"), "{}", layers.css);
    assert!(layers.css.contains("justify-items:flex-start"), "{}", layers.css);
}

#[test]
fn an_alignment_prints_its_swiftui_name() {
    assert_eq!(Alignment::TopLeading.to_string(), ".topLeading");
    assert_eq!(Alignment::Bottom.to_string(), ".bottom");
    assert_eq!(Alignment::Leading.to_string(), ".leading");
}
