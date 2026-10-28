//! `.sticky(top)` — CSS `position: sticky; top: N`. A view keeps its slot
//! until its scroll region carries it past the line `top` below the
//! viewport's top edge; from there it holds the line, and its containing
//! block (the stack it sits in) carries it away at its own bottom edge.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::anim::Spring;
use bunny_ui::layout::{DrawCommand, LayoutResult, Proposal, Rect, Size};
use bunny_ui::prelude::*;

const BOX: Size = Size { width: 300.0, height: 200.0 };

fn panel_ink() -> Color {
    Color::hex(0x95b2e8)
}

/// A module catalogue: a 100 pt heading, then a row of a 600 pt card
/// column and the detail beside it (held to the row's top), then a 200 pt
/// footer. The content is 900 pt tall, so the region travels 700 pt; the
/// row spans 100–700 of it.
fn catalogue(detail: impl View<Arity = Single>) -> impl View<Arity = Single> {
    scroll(
        vstack!(
            text("Build your workspace").frame_height(100.0),
            hstack!(text("Cards").frame(150.0, 600.0), detail)
                .spacing(0.0)
                .alignment(VerticalAlignment::Top),
            text("Extensions").frame_height(200.0)
        )
        .spacing(0.0),
    )
}

/// The detail: an 80 pt panel that sticks 10 pt below the region's top.
fn detail() -> impl View<Arity = Single> {
    text("Detail").frame(100.0, 80.0).background_color(panel_ink()).sticky(10.0)
}

/// The catalogue as a page: a component, so its region has a path and
/// keeps the offset it is given.
#[derive(Clone, Copy)]
struct Modules;

impl Component for Modules {
    fn body(self) -> impl View {
        catalogue(detail())
    }
}

/// Lays `view` out once, scrolls its region to `offset`, and lays it out
/// again.
fn scrolled_to(runtime: &Runtime, view: &impl View, offset: f64) -> LayoutResult {
    runtime.render_stable(view);
    let layout = runtime.layout(view, Proposal::exact(BOX));
    let path = layout.scrolls.first().expect("the catalogue scrolls").path.clone();
    runtime.set_scroll_offset(&path, Point { x: 0.0, y: offset });
    runtime.render_stable(view);
    runtime.layout(view, Proposal::exact(BOX))
}

/// Where the panel's background was painted.
fn panel(layout: &LayoutResult) -> Rect {
    layout
        .display
        .iter()
        .find_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } if *color == panel_ink() => Some(*rect),
            _ => None,
        })
        .expect("the panel paints")
}

#[test]
fn a_sticky_view_keeps_its_slot_until_the_region_carries_it_past_the_line() {
    for (offset, top) in [(0.0, 100.0), (50.0, 50.0), (90.0, 10.0)] {
        let layout = scrolled_to(&Runtime::new(), &Modules, offset);
        assert_eq!(panel(&layout).origin.y, top, "at offset {offset} the panel is still in its slot");
    }
}

#[test]
fn a_sticky_view_holds_the_line_once_its_slot_has_scrolled_above_it() {
    for offset in [91.0, 250.0, 500.0] {
        let layout = scrolled_to(&Runtime::new(), &Modules, offset);
        let painted = panel(&layout);
        assert_eq!(painted.origin.y, 10.0, "at offset {offset} the panel holds the line");
        assert_eq!(painted.size.height, 80.0, "it keeps its own size");
    }
}

#[test]
fn the_containing_row_carries_the_view_away_at_its_bottom_edge() {
    // at 650 the row ends 50 pt into the window: the panel's bottom rides
    // that edge, so its top is at -30 and the footer below stays clear
    let layout = scrolled_to(&Runtime::new(), &Modules, 650.0);
    let painted = panel(&layout);
    assert_eq!(painted.origin.y, -30.0);
    assert_eq!(painted.origin.y + painted.size.height, 50.0, "released at the row's bottom");
}

#[test]
fn outside_any_scroll_region_a_sticky_view_stays_in_its_slot() {
    let runtime = Runtime::new();
    let view = vstack!(text("Heading").frame_height(100.0), detail()).spacing(0.0);
    let layout = runtime.layout(&view, Proposal::exact(BOX));
    assert_eq!(panel(&layout).origin.y, 100.0);
}

#[derive(Clone, Copy)]
struct Catalogue {
    presses: State<u32>,
}

impl Component for Catalogue {
    fn body(self) -> impl View {
        let presses = self.presses;
        catalogue(
            button(text("Install").frame(100.0, 80.0), move || presses.update(|n| *n += 1))
                .id("detail")
                .sticky(10.0),
        )
    }
}

#[test]
fn presses_reach_the_view_where_it_is_stuck_and_not_its_slot() {
    let runtime = Runtime::new();
    let view = Catalogue { presses: State::new(0) };
    // the button is 92 pt tall: at 120 its slot spans -20–72 of the
    // window, and the stuck button 10–102
    let layout = scrolled_to(&runtime, &view, 120.0);
    let (_, hit) = layout.hits.iter().find(|(path, _)| path.contains("[detail]")).expect("the detail is a target");
    assert_eq!(hit.origin.y, 10.0, "the hit follows the paint");
    let x = hit.origin.x + hit.size.width / 2.0;

    // above the stuck panel, where its slot still is: nothing answers
    runtime.pointer_pressed(x, 5.0);
    runtime.pointer_released(x, 5.0);
    runtime.render_stable(&view);
    assert_eq!(view.presses.get(), 0, "the slot is empty glass");

    // below the slot, on the stuck panel
    runtime.layout(&view, Proposal::exact(BOX));
    runtime.pointer_pressed(x, 85.0);
    runtime.pointer_released(x, 85.0);
    runtime.render_stable(&view);
    assert_eq!(view.presses.get(), 1, "the stuck panel takes the press");
}

#[derive(Clone, Copy)]
struct Flying;

impl Component for Flying {
    fn body(self) -> impl View {
        catalogue(
            vstack!(for_each(vec!["badge"], |id| id.to_string(), |id| text(*id).animated(Spring::smooth())))
                .frame(100.0, 80.0)
                .background_color(panel_ink())
                .sticky(10.0),
        )
    }
}

#[test]
fn a_stuck_views_animated_content_moves_with_it_and_never_flies() {
    let runtime = Runtime::new();
    let layout = scrolled_to(&runtime, &Flying, 250.0);
    let badge = layout
        .display
        .iter()
        .find_map(|command| match command {
            DrawCommand::TextLine { origin, content, .. } if content.as_ref() == "badge" => Some(origin.y),
            _ => None,
        })
        .expect("the badge paints");
    let painted = panel(&layout);
    assert!(
        badge >= painted.origin.y && badge < painted.origin.y + painted.size.height,
        "the badge rides inside the stuck panel: {badge} in {painted:?}"
    );
    assert!(!runtime.wants_frame(), "the stick is no animation: nothing is in flight");
}

#[test]
fn a_served_page_hands_the_stick_to_the_browser() {
    let page = bunny_ui::ssr::render(&Modules, BOX);
    assert!(page.html.contains("position:sticky"), "{}", page.html);
    assert!(page.html.contains("top:10px"), "{}", page.html);
}

/// An exact interior positions every element by its frame, and the browser
/// scrolls it without a new layout. The sticky view leaves that regime: a
/// wrapper as tall as the rest of its containing row is its block, and the
/// view sits in it in the flow, where `position: sticky` works.
#[cfg(feature = "canvas")]
#[test]
fn an_exact_interior_gives_the_sticky_view_its_containing_block() {
    let page = bunny_ui::ssr::render(
        &catalogue(detail()).frame(300.0, 200.0).layout(bunny_ui::layout::LayoutMode::Exact),
        BOX,
    );
    // the wrapper: from the slot's top to the row's bottom (100–700), no
    // pointer of its own
    assert!(page.html.contains("height:600px"), "{}", page.html);
    assert!(page.css.contains("pointer-events:none"), "{}", page.css);
    // the view: in the flow at the top of its block, stuck by the browser
    assert!(
        page.html.contains("height:80px;pointer-events:auto;position:sticky;top:10px;width:100px"),
        "{}",
        page.html
    );
}
