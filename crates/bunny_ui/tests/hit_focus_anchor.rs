//! A decoration can let the pointer through, a control's focus ring is the
//! keyboard's, and a scroll target can be held at an anchor.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::action::{Key, KeyPattern};
use bunny_ui::layout::{DrawCommand, LayoutResult, Proposal, Size};
use bunny_ui::prelude::*;

const BOX: Size = Size { width: 200.0, height: 100.0 };

fn laid(runtime: &Runtime, view: &impl View) -> LayoutResult {
    runtime.layout(view, Proposal::exact(BOX))
}

#[derive(Clone, Copy)]
struct Floor {
    presses: State<u32>,
    passive: bool,
}

impl Component for Floor {
    fn body(self) -> impl View {
        let presses = self.presses;
        let plants = canvas(|ctx, p| {
            p.fill(bunny_ui::layout::Rect { origin: Point::ZERO, size: ctx.size() }, Color::rgba(0, 120, 0, 40));
        });
        zstack!(
            button(text("Meeting room"), move || presses.update(|n| *n += 1)).frame(200.0, 100.0),
            plants.allows_hit_testing(!self.passive)
        )
    }
}

#[test]
fn a_canvas_over_a_control_is_a_wall_until_it_declines_hit_testing() {
    for (passive, expected) in [(false, 0), (true, 1)] {
        let runtime = Runtime::new();
        let floor = Floor { presses: State::new(0), passive };
        runtime.render_stable(&floor);
        laid(&runtime, &floor);
        runtime.pointer_pressed(100.0, 50.0);
        runtime.pointer_released(100.0, 50.0);
        runtime.render_stable(&floor);
        assert_eq!(floor.presses.get(), expected, "passive {passive}");
    }
}

#[test]
fn a_passive_subtree_records_no_pointer_region() {
    let runtime = Runtime::new();
    let view = zstack!(
        button(text("Under"), || {}).id("under"),
        button(text("Over"), || {}).id("over").tooltip("explains").allows_hit_testing(false)
    );
    let layout = laid(&runtime, &view);
    assert!(layout.hits.iter().any(|(path, _)| path.contains("[under]")));
    assert!(!layout.hits.iter().any(|(path, _)| path.contains("[over]")), "{:?}", layout.hits);
}

#[test]
fn a_served_passive_box_says_pointer_events_none() {
    let page = bunny_ui::ssr::render(
        &vstack!(text("glow")).padding_length(4.0).background_color(Color::hex(0xeeeeee)).allows_hit_testing(false),
        BOX,
    );
    assert!(page.css.contains("pointer-events:none"), "{}", page.css);
}

/// Is a control's focus ring painted this frame?
fn ring(layout: &LayoutResult) -> bool {
    let focus = theme::current().focus;
    layout.display.iter().any(|command| {
        matches!(command, DrawCommand::StrokeRect { color, width, .. } if *color == focus && *width == 2.0)
    })
}

#[derive(Clone, Copy)]
struct Pair;

impl Component for Pair {
    fn body(self) -> impl View {
        hstack!(
            button(text("Ports"), || {}).id("ports"),
            button(text("Events"), || {}).id("events")
        )
    }
}

#[test]
fn a_pressed_control_shows_no_ring_and_a_keyboard_focus_does() {
    let runtime = Runtime::new();
    runtime.render_stable(&Pair);
    let layout = laid(&runtime, &Pair);
    let (_, ports) = layout.hits.iter().find(|(path, _)| path.contains("[ports]")).expect("ports").clone();
    let (x, y) = (ports.origin.x + 4.0, ports.origin.y + 4.0);
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
    runtime.render_stable(&Pair);
    assert!(!ring(&laid(&runtime, &Pair)), "a press leaves no ring");

    runtime.key_stroke(KeyPattern::key(Key::Tab));
    runtime.render_stable(&Pair);
    assert!(ring(&laid(&runtime, &Pair)), "the keyboard's focus wears it");
}

#[derive(Clone, Copy)]
struct List {
    anchor: Option<UnitPoint>,
}

impl Component for List {
    fn body(self) -> impl View {
        // forty 20 pt rows; a list records each row's frame under its key
        let rows = list(
            (0..40).collect::<Vec<usize>>(),
            |row| format!("row-{row}"),
            |row| text(format!("row {row}")).frame_height(20.0),
        );
        match self.anchor {
            Some(anchor) => erased(rows.scroll_target_anchored("row-20", anchor)),
            None => erased(rows.scroll_target("row-20")),
        }
    }
}

fn offset_after_reveal(list: List) -> f64 {
    let runtime = Runtime::new();
    runtime.render_stable(&list);
    let layout = laid(&runtime, &list);
    let region = layout.scrolls.first().expect("the list");
    runtime.scroll_offset(&region.path).y
}

#[test]
fn a_scroll_target_reveals_by_the_shortest_distance_or_at_its_anchor() {
    // row 20 spans 400–420 in a 100 pt window
    assert_eq!(offset_after_reveal(List { anchor: None }), 320.0, "the near edge: its bottom");
    assert_eq!(offset_after_reveal(List { anchor: Some(UnitPoint::CENTER) }), 360.0, "centred");
    assert_eq!(offset_after_reveal(List { anchor: Some(UnitPoint::TOP) }), 400.0, "at the top");
}
