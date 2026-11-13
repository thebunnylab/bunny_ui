//! A view tells the app WHERE the layout placed it: `.on_frame` hands
//! back the origin and the size, in the window's coordinates, through
//! nested stacks and the travel of a scroll region, and only on change.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::layout::{Proposal, Rect, Size};
use bunny_ui::prelude::*;

const WINDOW: Size = Size { width: 400.0, height: 300.0 };

fn room() -> Proposal {
    Proposal::exact(WINDOW)
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect { origin: Point { x, y }, size: Size { width, height } }
}

/// A tab strip over a toolbar: inset, buttons, then the field.
#[derive(Clone, Copy)]
struct Toolbar {
    buttons: State<f64>,
    field: State<Rect>,
    frames: State<u32>,
    sizes: State<u32>,
}

impl Component for Toolbar {
    fn body(self) -> impl View {
        let (field, frames, sizes) = (self.field, self.frames, self.sizes);
        vstack!(
            text("tabs").frame(400.0, 30.0),
            hstack!(
                spacer().frame(self.buttons.get(), 20.0),
                text("address")
                    .frame(200.0, 24.0)
                    .on_frame(move |frame| {
                        field.set(frame);
                        frames.update(|n| *n += 1);
                    })
                    .on_measure(move |_| sizes.update(|n| *n += 1)),
            )
            .spacing(8.0)
            .alignment(VerticalAlignment::Center)
            .padding_edge(Edge::Leading, 10.0)
            .frame_max(f64::INFINITY, 40.0, Alignment::Leading)
            .frame_height(40.0),
            spacer(),
        )
        .spacing(0.0)
    }
}

fn toolbar(buttons: f64) -> Toolbar {
    Toolbar {
        buttons: State::new(buttons),
        field: State::new(Rect::default()),
        frames: State::new(0),
        sizes: State::new(0),
    }
}

#[test]
fn a_nested_view_reports_its_origin_and_size_in_the_window() {
    let runtime = Runtime::new();
    let view = toolbar(60.0);
    let _ = runtime.settled_layout(&view, room());
    // under the 30 pt strip, centred in the 40 pt bar; after the 10 pt
    // inset, the 60 pt of buttons and the 8 pt of spacing
    assert_eq!(view.field.get(), rect(78.0, 38.0, 200.0, 24.0));
    assert_eq!(view.frames.get(), 1);
}

#[test]
fn a_frame_at_rest_is_not_news_and_a_move_is() {
    let runtime = Runtime::new();
    let view = toolbar(60.0);
    let _ = runtime.settled_layout(&view, room());
    let _ = runtime.settled_layout(&view, room());
    assert_eq!((view.frames.get(), view.sizes.get()), (1, 1), "a view at rest says nothing");

    // a wider row of buttons moves the field and keeps its size: the
    // frame probe hears it, the size probe on the same view does not
    view.buttons.set(90.0);
    let _ = runtime.settled_layout(&view, room());
    assert_eq!(view.field.get(), rect(108.0, 38.0, 200.0, 24.0));
    assert_eq!((view.frames.get(), view.sizes.get()), (2, 1), "moved, same size");
}

/// Forty rows of 20 pt in a region; row 10 says where it is.
#[derive(Clone, Copy)]
struct Rows {
    row: State<Rect>,
}

impl Component for Rows {
    fn body(self) -> impl View {
        let row = self.row;
        vstack!(
            text("header").frame(400.0, 50.0),
            scroll(vstack!(for_each(
                (0..40).collect::<Vec<usize>>(),
                |index| index.to_string(),
                move |index| {
                    let line = text(format!("row {index}")).frame(400.0, 20.0);
                    if *index == 10 {
                        Either::First(line.on_frame(move |frame| row.set(frame)))
                    } else {
                        Either::Second(line)
                    }
                },
            )).spacing(0.0))
            .id("rows"),
        )
        .spacing(0.0)
    }
}

#[test]
fn a_scrolled_view_reports_where_the_travel_left_it() {
    let runtime = Runtime::new();
    let view = Rows { row: State::new(Rect::default()) };
    let layout = runtime.settled_layout(&view, room());
    // the region starts under the header; row 10 is 200 pt into it
    assert_eq!(view.row.get(), rect(0.0, 250.0, 400.0, 20.0));

    let region = layout.scrolls.iter().find(|region| region.path.ends_with("[rows]")).expect("the region");
    runtime.set_scroll_offset(&region.path, Point { x: 0.0, y: 120.0 });
    let _ = runtime.settled_layout(&view, room());
    assert_eq!(view.row.get(), rect(0.0, 130.0, 400.0, 20.0), "120 pt of travel up");

    // the wheel is travel too: a turn toward the reader shows what is below
    assert!(runtime.wheel(200.0, 200.0, 0.0, -30.0));
    let _ = runtime.settled_layout(&view, room());
    assert_eq!(view.row.get(), rect(0.0, 100.0, 400.0, 20.0), "30 pt more");

    // a row scrolled out of sight is still where the layout put it
    runtime.set_scroll_offset(&region.path, Point { x: 0.0, y: 400.0 });
    let _ = runtime.settled_layout(&view, room());
    assert_eq!(view.row.get(), rect(0.0, -150.0, 400.0, 20.0));
}

/// The use the probe is for: a list hangs under a field it reads the
/// frame of, from a layer on the window's root.
#[derive(Clone, Copy)]
struct Dropdown {
    field: State<Rect>,
}

impl Component for Dropdown {
    fn body(self) -> impl View {
        let field = self.field;
        let at = self.field.get();
        vstack!(
            text("tabs").frame(400.0, 30.0),
            hstack!(
                text("back").frame(44.0, 24.0),
                text("address").frame(240.0, 24.0).on_frame(move |frame| field.set(frame)),
            )
            .spacing(6.0)
            .padding_edge(Edge::Leading, 12.0)
            .frame_max(f64::INFINITY, 36.0, Alignment::Leading)
            .frame_height(36.0),
            spacer(),
        )
        .spacing(0.0)
        .overlay(
            UnitPoint::TOP_LEADING,
            text("suggestions")
                .frame(at.size.width, 60.0)
                .on_click(|| {})
                .id("suggestions")
                .padding_edge(Edge::Leading, at.origin.x)
                .padding_edge(Edge::Top, at.origin.y + at.size.height + 2.0),
        )
    }
}

#[test]
fn a_layer_hangs_under_the_frame_it_heard_in_the_same_frame() {
    let runtime = Runtime::new();
    let view = Dropdown { field: State::new(Rect::default()) };
    let layout = runtime.settled_layout(&view, room());
    assert_eq!(view.field.get(), rect(62.0, 36.0, 240.0, 24.0));
    // the layout that heard the frame already placed the list under it:
    // the first frame painted never shows the list at the origin
    let (_, list) = layout.hits.iter().find(|(path, _)| path.contains("[suggestions]")).expect("the list");
    assert_eq!(*list, rect(62.0, 62.0, 240.0, 60.0));
}

/// Two probes on one view are two addresses, as two `.on_measure` are.
#[test]
fn a_frame_probe_and_a_size_probe_on_one_view_both_hear() {
    #[derive(Clone, Copy)]
    struct Both {
        frame: State<Rect>,
        size: State<Size>,
    }
    impl Component for Both {
        fn body(self) -> impl View {
            let (frame, size) = (self.frame, self.size);
            vstack!(
                text("top").frame(100.0, 40.0),
                text("probed")
                    .frame(120.0, 30.0)
                    .on_measure(move |measured| size.set(measured))
                    .on_frame(move |placed| frame.set(placed)),
                spacer(),
            )
            .spacing(0.0)
            .alignment(HorizontalAlignment::Leading)
        }
    }
    let runtime = Runtime::new();
    let view = Both { frame: State::new(Rect::default()), size: State::new(Size::default()) };
    let _ = runtime.settled_layout(&view, room());
    assert_eq!(view.size.get(), Size { width: 120.0, height: 30.0 });
    assert_eq!(view.frame.get(), rect(0.0, 40.0, 120.0, 30.0));
}
