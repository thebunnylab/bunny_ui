//! Equal-column grids expose the same cells to native and browser layout.
use bunny_ui::layout::{DrawCommand, Proposal};
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

fn origin(display: &bunny_ui::layout::DisplayList, word: &str) -> Point {
    display
        .iter()
        .find_map(|c| match c {
            DrawCommand::TextLine {
                origin, content, ..
            } if &**content == word => Some(*origin),
            _ => None,
        })
        .expect("painted cell")
}

#[test]
fn a_grid_of_five_in_two_columns_makes_three_rows() {
    let view = grid!(text!("a"), text!("b"), text!("c"), text!("d"), text!("e"))
        .columns(2)
        .spacing(4.0);
    let result = Runtime::new().layout(
        &view,
        Proposal {
            width: Some(100.0),
            height: None,
        },
    );
    assert_eq!(
        result.size,
        Size {
            width: 100.0,
            height: 56.0
        }
    );
    for (word, x, y) in [
        ("a", 20.0, 0.0),
        ("b", 72.0, 0.0),
        ("c", 20.0, 20.0),
        ("d", 72.0, 20.0),
        ("e", 20.0, 40.0),
    ] {
        assert_eq!(origin(&result.display, word), Point { x, y });
    }
}

#[test]
fn a_span_of_two_takes_two_columns() {
    let view = grid!(text!("a").span(2, 1), text!("b"))
        .columns(2)
        .spacing(4.0);
    let result = Runtime::new().layout(
        &view,
        Proposal {
            width: Some(100.0),
            height: None,
        },
    );
    assert_eq!(origin(&result.display, "a"), Point { x: 46.0, y: 0.0 });
    assert_eq!(origin(&result.display, "b"), Point { x: 20.0, y: 20.0 });
}

#[test]
fn a_row_span_holds_its_column_open_for_the_next_row() {
    let view = grid!(text!("a").span(1, 2), text!("b"), text!("c"), text!("d"))
        .columns(2)
        .spacing(4.0);
    let result = Runtime::new().layout(
        &view,
        Proposal {
            width: Some(100.0),
            height: None,
        },
    );
    assert_eq!(origin(&result.display, "b"), Point { x: 72.0, y: 0.0 });
    assert_eq!(origin(&result.display, "c"), Point { x: 72.0, y: 20.0 });
    assert_eq!(origin(&result.display, "d"), Point { x: 20.0, y: 40.0 });
}

#[test]
fn a_grid_offered_nothing_is_as_wide_as_its_widest_cell() {
    let result = Runtime::new().layout(
        &grid!(text!("abc"), text!("d")).spacing(4.0),
        Proposal::unspecified(),
    );
    assert_eq!(
        result.size,
        Size {
            width: 52.0,
            height: 16.0
        }
    );
}

#[test]
fn a_span_outside_a_grid_is_kept_and_ignored() {
    let result = Runtime::new().layout(&text!("a").span(2, 3), Proposal::unspecified());
    assert_eq!(
        result.size,
        Size {
            width: 8.0,
            height: 16.0
        }
    );
}

#[test]
fn a_grid_reads_from_the_right_in_rtl() {
    let runtime = Runtime::new();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let view = grid!(text!("a"), text!("b")).columns(2).spacing(4.0);
    let result = runtime.layout(
        &view,
        Proposal {
            width: Some(100.0),
            height: None,
        },
    );
    assert_eq!(origin(&result.display, "a"), Point { x: 72.0, y: 0.0 });
    assert_eq!(origin(&result.display, "b"), Point { x: 20.0, y: 0.0 });
}

#[test]
fn a_span_wider_than_the_grid_is_clamped_and_wrappers_keep_the_outer_span() {
    let view = grid!(text!("a").span(9, 1).frame(40.0, 30.0), text!("b"))
        .columns(2)
        .spacing(4.0);
    let result = Runtime::new().layout(
        &view,
        Proposal {
            width: Some(100.0),
            height: None,
        },
    );
    assert_eq!(origin(&result.display, "a"), Point { x: 46.0, y: 7.0 });
    assert_eq!(origin(&result.display, "b"), Point { x: 20.0, y: 34.0 });
}

#[test]
fn explicit_row_spacing_and_zero_counts_have_defined_results() {
    let view = grid!(text!("a").span(0, 0), text!("b"))
        .columns(0)
        .spacing(4.0)
        .row_spacing(8.0);
    let result = Runtime::new().layout(
        &view,
        Proposal {
            width: Some(100.0),
            height: None,
        },
    );
    assert_eq!(
        result.size,
        Size {
            width: 100.0,
            height: 40.0
        }
    );
    assert_eq!(origin(&result.display, "b"), Point { x: 46.0, y: 24.0 });
}

#[test]
fn served_grid_has_tracks_gaps_and_clamped_spans() {
    let view = grid!(text!("a").span(9, 2), text!("b"))
        .columns(2)
        .spacing(4.0)
        .row_spacing(8.0)
        .alignment(Alignment::Leading);
    let page = bunny_ui::ssr::render(
        &view,
        Size {
            width: 100.0,
            height: 100.0,
        },
    );
    assert!(
        page.css
            .contains("grid-template-columns:repeat(2,minmax(0,1fr))"),
        "{}",
        page.css
    );
    assert!(page.css.contains("column-gap:4px") && page.css.contains("row-gap:8px"));
    assert!(
        page.css.contains("justify-items:flex-start") && page.css.contains("align-items:center")
    );
    assert!(
        page.html.contains("grid-column:span 2") && page.html.contains("grid-row:span 2"),
        "{}",
        page.html
    );
}

#[test]
fn a_retained_grid_keeps_the_same_cell_geometry() {
    #[derive(Clone)]
    struct Page;
    impl Component for Page {
        fn body(self) -> impl View {
            grid!(text!("a").span(2, 1), text!("b"))
                .columns(2)
                .spacing(4.0)
        }
    }
    let runtime = Runtime::new();
    let size = Size {
        width: 100.0,
        height: 100.0,
    };
    let first = runtime.display_frame(&Page, size);
    let next = runtime.display_frame(&Page, size);
    for word in ["a", "b"] {
        assert_eq!(origin(&first, word), origin(&next, word));
    }
}

#[test]
fn measured_cell_hit_areas_include_empty_space_and_mirror_in_rtl() {
    use bunny_ui_core::layout::{grid_cells, Point, Rect, Size};
    let bounds = Rect { origin: Point { x: 12.0, y: 20.0 }, size: Size { width: 210.0, height: 130.0 } };
    let cells = [
        (Size { width: 20.0, height: 10.0 }, (1, 1)),
        (Size { width: 90.0, height: 100.0 }, (1, 1)),
        (Size { width: 80.0, height: 20.0 }, (2, 1)),
    ];
    let hit = grid_cells(bounds, 2, 10.0, 10.0, false, &cells);
    assert_eq!(
        hit[0],
        Rect {
            origin: bounds.origin,
            size: Size {
                width: 100.0,
                height: 100.0
            }
        }
    );
    assert_eq!(hit[1].origin, Point { x: 122.0, y: 20.0 });
    assert_eq!(
        hit[2],
        Rect {
            origin: Point { x: 12.0, y: 130.0 },
            size: Size {
                width: 210.0,
                height: 20.0
            }
        }
    );
    let rtl = grid_cells(bounds, 2, 10.0, 10.0, true, &cells);
    assert_eq!(rtl[0], hit[1]);
    assert_eq!(rtl[1], hit[0]);
    assert_eq!(rtl[2], hit[2]);
}
