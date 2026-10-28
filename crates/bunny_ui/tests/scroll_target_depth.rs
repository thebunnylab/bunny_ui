//! A scroll target names a keyed component anywhere in its region's
//! content, not only a list's direct row.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;

const BOX: Size = Size { width: 200.0, height: 100.0 };

/// A 40 pt section of a document, keyed by its caller.
#[derive(Clone, Copy)]
struct Section(usize);

impl Component for Section {
    fn body(self) -> impl View {
        text(format!("section {}", self.0)).frame_height(40.0)
    }
}

/// A 100 pt heading over ten keyed sections, two stacks deep in the
/// scrolled content: section `i` spans `100 + 40 i` to `140 + 40 i`.
fn document() -> impl View<Arity = Single> {
    vstack!(
        text("Agreement").frame_height(100.0),
        vstack!(for_each((0..10).collect::<Vec<usize>>(), |i| i.to_string(), |&i| Section(i).id(format!("s-{i}"))))
            .spacing(0.0)
    )
    .spacing(0.0)
}

#[derive(Clone, Copy)]
struct Desk {
    anchor: Option<UnitPoint>,
}

impl Component for Desk {
    fn body(self) -> impl View {
        let desk = scroll(document());
        match self.anchor {
            Some(anchor) => erased(desk.scroll_target_anchored("s-6", anchor)),
            None => erased(desk.scroll_target("s-6")),
        }
    }
}

fn offset_after_reveal(view: &impl Component) -> f64 {
    let runtime = Runtime::new();
    runtime.render_stable(view);
    let layout = runtime.layout(view, Proposal::exact(BOX));
    let region = layout.scrolls.first().expect("the desk");
    runtime.scroll_offset(&region.path).y
}

#[test]
fn a_keyed_component_deep_in_the_content_is_revealed() {
    // section 6 spans 340–380 in a 100 pt window
    assert_eq!(offset_after_reveal(&Desk { anchor: Some(UnitPoint::CENTER) }), 310.0, "centred");
    assert_eq!(offset_after_reveal(&Desk { anchor: Some(UnitPoint::TOP) }), 340.0, "at the top");
    assert_eq!(offset_after_reveal(&Desk { anchor: None }), 280.0, "the near edge: its bottom");
}

#[derive(Clone, Copy)]
struct Nested;

impl Component for Nested {
    fn body(self) -> impl View {
        // the keyed section lives in an inner region: it is that region's
        // to reveal, and the outer one stays where it is
        scroll(vstack!(text("Cover").frame_height(150.0), scroll(document()).frame_height(60.0)).spacing(0.0))
            .scroll_target("s-6")
    }
}

#[test]
fn a_keyed_component_inside_a_nearer_region_does_not_move_the_outer_one() {
    let runtime = Runtime::new();
    runtime.render_stable(&Nested);
    let layout = runtime.layout(&Nested, Proposal::exact(BOX));
    let outer = layout.scrolls.iter().min_by_key(|region| region.path.len()).expect("the outer region");
    assert_eq!(runtime.scroll_offset(&outer.path).y, 0.0);
}

#[derive(Clone, Copy)]
struct Moving {
    target: State<usize>,
}

impl Component for Moving {
    fn body(self) -> impl View {
        scroll(document()).scroll_target_anchored(format!("s-{}", self.target.get()), UnitPoint::CENTER)
    }
}

#[test]
fn choosing_another_section_reveals_it_once_and_the_wheel_keeps_its_say() {
    let runtime = Runtime::new();
    let view = Moving { target: State::new(1) };
    runtime.render_stable(&view);
    let layout = runtime.layout(&view, Proposal::exact(BOX));
    let path = layout.scrolls.first().expect("the desk").path.clone();
    // section 1 spans 140–180: centred at 110
    assert_eq!(runtime.scroll_offset(&path).y, 110.0);

    // the reader scrolls on; the unchanged target does not pull back
    runtime.set_scroll_offset(&path, Point { x: 0.0, y: 200.0 });
    runtime.render_stable(&view);
    runtime.layout(&view, Proposal::exact(BOX));
    assert_eq!(runtime.scroll_offset(&path).y, 200.0);

    // the last section, 460–500, centres past the travel: clamped to 400
    view.target.set(9);
    runtime.render_stable(&view);
    runtime.layout(&view, Proposal::exact(BOX));
    assert_eq!(runtime.scroll_offset(&path).y, 400.0);
}
