//! A scroll viewport must reach flexible descendants on its non-scrolling axis.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::layout::{LayoutResult, Proposal, Size};
use bunny_ui::prelude::*;

#[derive(Clone, Copy)]
struct Sheet {
    count: State<usize>,
}

impl Component for Sheet {
    fn body(self) -> impl View {
        vstack!(
            text("Toolbar")
                .frame_height(36.0)
                .on_click(|| {})
                .id("toolbar"),
            table(
                vec![column("Name", 420.0), column("Value", 140.0)],
                self.count.get(),
                |row| row.to_string(),
                |row, column| text(format!("r{row}c{column}"))
            )
            .row_height(28.0)
            .row(|row, band| band.on_click(|| {}).id(format!("row-{row}"))),
            text("Footer")
                .frame_height(24.0)
                .on_click(|| {})
                .id("footer"),
        )
        .spacing(0.0)
    }
}

fn laid(runtime: &Runtime, sheet: &impl View, size: Size) -> LayoutResult {
    runtime.layout(sheet, Proposal::exact(size))
}

fn assert_windowed(layout: &LayoutResult, size: Size) {
    let down = layout
        .scrolls
        .iter()
        .find(|region| region.row_extent.is_some())
        .unwrap();
    assert!(
        down.frame.size.height <= size.height - 60.0,
        "unbounded viewport: {:?}",
        down.frame
    );
    let footer = layout
        .hits
        .iter()
        .find(|(path, _)| path.ends_with("[footer]"))
        .unwrap()
        .1;
    assert!(
        footer.origin.y + footer.size.height <= size.height + 0.01,
        "footer below viewport: {footer:?}"
    );
    let mounted = layout
        .hits
        .iter()
        .filter(|(path, _)| path.contains("[row-"))
        .count();
    assert!(
        mounted <= 256,
        "materialized {mounted} rows instead of a viewport"
    );
}

#[test]
fn table_between_bars_keeps_a_viewport_through_scroll_filter_and_resize() {
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let sheet = Sheet {
        count: State::new(100_000),
    };
    let size = Size {
        width: 640.0,
        height: 480.0,
    };
    let first = laid(&runtime, &sheet, size);
    assert_windowed(&first, size);
    let down = first
        .scrolls
        .iter()
        .find(|region| region.row_extent.is_some())
        .unwrap();
    assert_eq!(down.content.height, 2_800_000.0);
    assert!(runtime.wheel(
        down.frame.origin.x + 40.0,
        down.frame.origin.y + 40.0,
        0.0,
        -3_000_000.0
    ));
    let last = laid(&runtime, &sheet, size);
    assert_windowed(&last, size);
    assert!(
        last.hits
            .iter()
            .any(|(path, _)| path.ends_with("[row-99999]"))
    );
    sheet.count.set(0);
    let empty = laid(&runtime, &sheet, size);
    assert_windowed(&empty, size);
    assert!(!empty.hits.iter().any(|(path, _)| path.contains("[row-")));
    sheet.count.set(100_000);
    let resized = Size {
        width: 400.0,
        height: 240.0,
    };
    let restored = laid(&runtime, &sheet, resized);
    assert_windowed(&restored, resized);
    assert!(
        restored
            .scrolls
            .iter()
            .any(|region| region.content.width > region.frame.size.width)
    );
}

#[test]
fn vertical_scroll_passes_horizontal_flexibility_to_its_child() {
    let runtime = Runtime::new();
    runtime.drop_unseen();
    let page = hstack!(
        text("Side").frame_width(80.0),
        scroll(scroll(text("wide").frame(4000.0, 40.0)).horizontal()).id("outer"),
        text("End").frame_width(60.0),
    )
    .spacing(0.0);
    let layout = laid(
        &runtime,
        &page,
        Size {
            width: 640.0,
            height: 200.0,
        },
    );
    assert_eq!(layout.scrolls.len(), 2);
    assert!(
        layout
            .scrolls
            .iter()
            .all(|region| region.frame.size.width <= 500.0),
        "{:?}",
        layout.scrolls
    );
    assert!(
        layout
            .scrolls
            .iter()
            .any(|region| region.content.width == 4000.0)
    );
}

#[test]
fn horizontal_scroll_of_fixed_height_content_keeps_its_natural_height() {
    let runtime = Runtime::new();
    let page = vstack!(
        scroll(text("line").frame(1200.0, 22.0)).horizontal(),
        spacer(),
        text("End").frame_height(20.0),
    )
    .spacing(0.0);
    let layout = laid(
        &runtime,
        &page,
        Size {
            width: 400.0,
            height: 300.0,
        },
    );
    assert_eq!(layout.scrolls[0].frame.size.height, 22.0);
}
