//! Positioned children retain ordinary view identity and layout wrappers.
use bunny_ui::layout::{DisplayList, DrawCommand, Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

fn origin_of(display: &DisplayList, word: &str) -> Point {
    display
        .iter()
        .find_map(|command| match command {
            DrawCommand::TextLine {
                origin, content, ..
            } if &**content == word => Some(*origin),
            _ => None,
        })
        .expect("text was painted")
}

#[test]
fn a_placed_child_in_a_layered_stack_sits_at_its_point_and_grows_the_extent() {
    let view = zstack!(
        text!("a").position(10.0, 20.0),
        text!("b").position(50.0, 5.0)
    );
    let result = Runtime::new().layout(&view, Proposal::unspecified());
    assert_eq!(
        result.size,
        Size {
            width: 58.0,
            height: 36.0
        }
    );
    assert_eq!(origin_of(&result.display, "a"), Point { x: 10.0, y: 20.0 });
    assert_eq!(origin_of(&result.display, "b"), Point { x: 50.0, y: 5.0 });
    let result = Runtime::new().layout(&view.frame(240.0, 120.0), Proposal::unspecified());
    assert_eq!(
        result.size,
        Size {
            width: 240.0,
            height: 120.0
        }
    );
}

#[test]
fn a_placed_child_in_a_column_takes_no_slot_and_no_spacing() {
    let view = vstack!(text!("a"), text!("placed").position(90.0, 70.0), text!("b")).spacing(4.0);
    let result = Runtime::new().layout(&view, Proposal::unspecified());
    assert_eq!(
        result.size,
        Size {
            width: 8.0,
            height: 36.0
        }
    );
    assert_eq!(origin_of(&result.display, "b"), Point { x: 0.0, y: 20.0 });
    assert_eq!(
        origin_of(&result.display, "placed"),
        Point { x: 90.0, y: 70.0 }
    );
}

#[test]
fn a_position_outside_a_stack_is_kept_and_ignored() {
    let result = Runtime::new().layout(&text!("a").position(30.0, 50.0), Proposal::unspecified());
    assert_eq!(origin_of(&result.display, "a"), Point::ZERO);
    assert_eq!(
        result.size,
        Size {
            width: 8.0,
            height: 16.0
        }
    );
}

#[test]
fn a_position_under_a_frame_places_the_outer_box() {
    let a = zstack!(text!("a").position(10.0, 20.0).frame(40.0, 30.0));
    let b = zstack!(text!("a").frame(40.0, 30.0).position(10.0, 20.0));
    let a = Runtime::new().layout(&a, Proposal::unspecified());
    let b = Runtime::new().layout(&b, Proposal::unspecified());
    assert_eq!(
        a.size,
        Size {
            width: 50.0,
            height: 50.0
        }
    );
    assert_eq!(a.size, b.size);
    assert_eq!(origin_of(&a.display, "a"), Point { x: 26.0, y: 27.0 });
    assert_eq!(origin_of(&a.display, "a"), origin_of(&b.display, "a"));
}

#[test]
fn a_placed_child_is_proposed_the_room_to_the_far_edges() {
    let view = vstack!(rectangle().position(30.0, 20.0).on_measure(|size| {
        assert_eq!(
            size,
            Size {
                width: 210.0,
                height: 100.0
            }
        );
    }));
    let runtime = Runtime::new();
    let result = runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 240.0,
            height: 120.0,
        }),
    );
    // a column with nothing left in its flow is a pile of placed boxes,
    // as big as what it places: 30 + 210 by 20 + 100
    assert_eq!(
        result.size,
        Size {
            width: 240.0,
            height: 120.0
        },
        "a column of placed boxes is as big as what it places"
    );
    let fill = result
        .display
        .iter()
        .find_map(|c| match c {
            DrawCommand::FillRect { rect, .. } => Some(rect),
            _ => None,
        })
        .expect("rectangle painted");
    assert_eq!(fill.origin, Point { x: 30.0, y: 20.0 });
    assert_eq!(
        fill.size,
        Size {
            width: 210.0,
            height: 100.0
        }
    );
}

#[test]
fn a_placed_child_keeps_x_from_the_left_in_rtl() {
    let runtime = Runtime::new();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let result = runtime.layout(
        &vstack!(
            rectangle().frame(240.0, 120.0),
            text!("a").position(30.0, 20.0)
        ),
        Proposal::exact(Size {
            width: 240.0,
            height: 120.0,
        }),
    );
    assert_eq!(origin_of(&result.display, "a"), Point { x: 30.0, y: 20.0 });
}

#[test]
fn a_positioned_child_past_its_stack_is_cut_only_under_clipped() {
    let view = vstack!(text!("a").position(90.0, 70.0)).frame(40.0, 30.0);
    let open = Runtime::new().layout(&view, Proposal::unspecified());
    assert!(origin_of(&open.display, "a").x >= 90.0);
    assert!(
        !open
            .display
            .iter()
            .any(|c| matches!(c, DrawCommand::PushClip { .. }))
    );
    let clipped = Runtime::new().layout(&view.clipped(), Proposal::unspecified());
    assert!(clipped.display.iter().any(|c| matches!(c, DrawCommand::PushClip { rect, .. } if rect.size == Size { width: 40.0, height: 30.0 })));
}

#[derive(Clone)]
struct Positioned;
impl Component for Positioned {
    fn body(self) -> impl View {
        vstack!(text!("a").position(10.0, 20.0), text!("b"))
    }
}

#[test]
fn overrides_move_a_position_and_append_one_through_site_wrappers() {
    use bunny_ui::modifier::Modifier;
    use bunny_ui::provenance::{self, Override, SiteKey};
    provenance::clear_overrides();
    let runtime = Runtime::new();
    runtime.record_sites(true);
    let size = Size {
        width: 240.0,
        height: 120.0,
    };
    runtime.display_frame(&Positioned, size);
    let sites = runtime.sites();
    let position = sites
        .iter()
        .find(|s| s.modifier == Some("position"))
        .unwrap();
    let key = SiteKey::of(position.site).unwrap();
    provenance::set_overrides([(
        key,
        Override::Replace(Modifier::Position(Point { x: 40.0, y: 50.0 })),
    )]);
    assert_eq!(
        origin_of(&runtime.display_frame(&Positioned, size), "a"),
        Point { x: 40.0, y: 50.0 }
    );
    let b = sites
        .iter()
        .find(|s| {
            s.kind == provenance::NodeKind::Text
                && s.modifier.is_none()
                && s.frame.origin == Point::ZERO
        })
        .unwrap();
    provenance::set_overrides([(
        SiteKey::of(b.site).unwrap(),
        Override::Append(Modifier::Position(Point { x: 80.0, y: 60.0 })),
    )]);
    let display = runtime.display_frame(&Positioned, size);
    assert_eq!(origin_of(&display, "b"), Point { x: 80.0, y: 60.0 });
    provenance::clear_overrides();
    let display = runtime.display_frame(&Positioned, size);
    assert_eq!(origin_of(&display, "a"), Point { x: 10.0, y: 20.0 });
}

#[test]
fn served_html_preserves_flow_and_layer_placement() {
    let size = Size {
        width: 240.0,
        height: 120.0,
    };
    let flow = bunny_ui::ssr::render(&vstack!(text!("a").position(10.0, 20.0)), size);
    assert!(
        flow.html.contains("left:10px") && flow.html.contains("top:20px"),
        "{}",
        flow.html
    );
    assert!(flow.html.contains("position:absolute"));
    assert!(flow.css.contains("position:relative"));
    let layers = bunny_ui::ssr::render(&zstack!(text!("a").position(10.0, 20.0)), size);
    assert!(
        layers.html.contains("margin-left:10px") && layers.html.contains("margin-top:20px"),
        "{}",
        layers.html
    );
    assert!(layers.html.contains("justify-self:left"));
}

#[test]
fn clearing_dom_placement_removes_its_inline_geometry() {
    use bunny_ui::dom::DomPatch;
    let size = Size {
        width: 240.0,
        height: 120.0,
    };
    let runtime = Runtime::new();
    let patches = runtime.dom_frame(&vstack!(rectangle().position(10.0, 20.0)), size);
    let id = patches
        .iter()
        .find_map(|p| match p {
            DomPatch::SetBox {
                id, at: Some(at), ..
            } => {
                assert!(at.fill_x && at.fill_y);
                Some(*id)
            }
            _ => None,
        })
        .expect("positioned rectangle");
    let mut page = bunny_ui::ssr::Replay::new(size);
    page.apply(&patches);
    let html = page.html();
    assert!(
        html.contains("right:0") && html.contains("bottom:0"),
        "{html}"
    );
    page.apply(&[DomPatch::SetBox {
        id,
        width: None,
        height: None,
        max_width: None,
        max_height: None,
        slot_y: None,
        at: None,
        span: None,
        sticky: None,
    }]);
    let html = page.html();
    assert!(
        !html.contains("left:10px") && !html.contains("top:20px") && !html.contains("bottom:0"),
        "{html}"
    );
}

#[test]
fn placed_rows_of_a_list_sit_at_their_points_in_the_layers_they_are_written_in() {
    // a chart's marks, one row of data each: the list is the zstack's
    // child, and every mark is placed in the zstack it was written in —
    // not in a column of the list's own that the zstack then centred
    let points = vec![("a", 10.0, 20.0), ("b", 150.0, 60.0)];
    let view = zstack!(
        spacer(),
        for_each(points, |(word, _, _)| (*word).to_owned(), |(word, x, y)| {
            text(*word).position(*x, *y)
        })
    );
    let result = Runtime::new().layout(
        &view,
        Proposal::exact(Size {
            width: 200.0,
            height: 100.0,
        }),
    );
    assert_eq!(origin_of(&result.display, "a"), Point { x: 10.0, y: 20.0 });
    assert_eq!(origin_of(&result.display, "b"), Point { x: 150.0, y: 60.0 });
}

#[test]
fn a_list_of_placed_rows_grows_the_layers_extent_like_the_rows_would() {
    let points = vec![("a", 10.0, 20.0), ("b", 50.0, 5.0)];
    let view = zstack!(for_each(points, |(word, _, _)| (*word).to_owned(), |(word, x, y)| {
        text(*word).position(*x, *y)
    }));
    let result = Runtime::new().layout(&view, Proposal::unspecified());
    assert_eq!(
        result.size,
        Size {
            width: 58.0,
            height: 36.0
        },
        "the same extent the two marks give written straight into the zstack"
    );
}

#[test]
fn a_list_with_rows_in_the_flow_keeps_its_own_column() {
    // only a list whose EVERY row is placed hands its rows to the layers:
    // one row in the flow makes it a column again, and a placed row in it
    // counts from that column, as in any stack
    let view = zstack!(
        spacer(),
        for_each(vec![0, 1], |row| row.to_string(), |row| match row {
            0 => erased(text("flow")),
            _ => erased(text("placed").position(5.0, 30.0)),
        })
    )
    .alignment(Alignment::TopLeading);
    let result = Runtime::new().layout(
        &view,
        Proposal::exact(Size {
            width: 200.0,
            height: 100.0,
        }),
    );
    assert_eq!(origin_of(&result.display, "flow"), Point { x: 0.0, y: 0.0 });
    assert_eq!(origin_of(&result.display, "placed"), Point { x: 5.0, y: 30.0 });
}

#[test]
fn placed_rows_of_a_bound_list_sit_at_their_points_too() {
    // a list that reads its items is a boundary of its own, whose rows
    // stack in the boundary's implicit column: the same pile
    #[derive(Clone, Copy)]
    struct Chart {
        marks: State<Vec<(&'static str, f64, f64)>>,
    }
    impl Component for Chart {
        fn body(self) -> impl View {
            zstack!(
                spacer(),
                for_each(self.marks, |(word, _, _)| (*word).to_owned(), |(word, x, y)| {
                    text(*word).position(*x, *y)
                })
            )
        }
    }
    let view = Chart { marks: State::new(vec![("a", 10.0, 20.0), ("b", 150.0, 60.0)]) };
    let result = Runtime::new().layout(
        &view,
        Proposal::exact(Size {
            width: 200.0,
            height: 100.0,
        }),
    );
    assert_eq!(origin_of(&result.display, "a"), Point { x: 10.0, y: 20.0 });
    assert_eq!(origin_of(&result.display, "b"), Point { x: 150.0, y: 60.0 });
}
