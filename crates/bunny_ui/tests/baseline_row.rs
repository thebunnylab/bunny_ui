use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

/// A row on a shared first baseline drops each child by its distance
/// to the deepest baseline — and holds what that drop reaches. A bar's
/// baseline is its bottom edge; a column's is its first line's, near
/// its top. Dropped to the bar's, the column hangs below the bar, and
/// the row ends where the column does.
#[test]
fn a_baseline_row_holds_the_children_it_dropped() {
    let runtime = Runtime::new();
    let view = hstack!(
        rectangle().frame(10.0, 40.0).test_id("bar"),
        vstack!(text("first"), rectangle().frame(10.0, 30.0)).test_id("column"),
    )
    .alignment(VerticalAlignment::Baseline)
    .test_id("row");
    runtime.display_frame(&view, Size { width: 300.0, height: 200.0 });

    let frames = runtime.test_frames();
    let frame = |name: &str| frames.iter().find(|(id, _)| &**id == name).map(|(_, rect)| *rect).expect(name);
    let bottom = |rect: Rect| rect.origin.y + rect.size.height;
    let (row, bar, column) = (frame("row"), frame("bar"), frame("column"));
    assert!(column.origin.y > row.origin.y, "the column drops to the bar's baseline: {column:?} in {row:?}");
    assert!(bottom(column) > bottom(bar), "and hangs below the bar: {column:?}, {bar:?}");
    assert!((bottom(row) - bottom(column)).abs() < 1e-9, "the row ends with it: {row:?}, {column:?}");
}
