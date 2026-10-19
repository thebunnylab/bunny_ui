//! Stack macros keep flat native/browser children beyond Rust's tuple arities.
use bunny_ui::dom::{CreateKind, DomPatch};
use bunny_ui::layout::{DrawCommand, Proposal};
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

macro_rules! stacks {
    ($($child:expr),* $(,)?) => {
        [erased(vstack!($($child),*)), erased(hstack!($($child),*)),
         erased(zstack!($($child),*)), erased(grid!($($child),*))]
    };
}

fn words(display: &bunny_ui::layout::DisplayList) -> Vec<String> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, .. } => Some(content.to_string()),
            _ => None,
        })
        .collect()
}

fn verify(views: [Erased; 4], count: usize) {
    let expected: Vec<_> = (0..count).map(|i| i.to_string()).collect();
    for (view, kind) in views.into_iter().zip([
        CreateKind::FlexColumn,
        CreateKind::FlexRow,
        CreateKind::Layers,
        CreateKind::Grid,
    ]) {
        let runtime = Runtime::new();
        runtime.record_sites(true);
        let layout = runtime.layout(&view, Proposal::unspecified());
        assert_eq!(words(&layout.display), expected);
        assert_eq!(
            runtime
                .sites()
                .iter()
                .filter(|site| site.kind == bunny_ui::provenance::NodeKind::Text)
                .count(),
            count
        );
        let patches = runtime.dom_frame(
            &view,
            Size {
                width: 4096.0,
                height: 4096.0,
            },
        );
        let content: Vec<_> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::SetContent { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(content, expected);
        assert_eq!(patches.iter().filter(|patch| matches!(patch, DomPatch::Create { kind: actual, .. } if *actual == kind)).count(), 1, "chunks introduce no extra containers");
    }
}

#[test]
fn all_four_macros_keep_empty_and_single_child_forms() {
    verify(stacks!(), 0);
    verify(stacks!(text("0")), 1);
}

#[test]
fn all_four_macros_keep_12_flat_children() {
    verify(
        stacks!(
            text("0"),
            text("1"),
            text("2"),
            text("3"),
            text("4"),
            text("5"),
            text("6"),
            text("7"),
            text("8"),
            text("9"),
            text("10"),
            text("11")
        ),
        12,
    );
}

#[test]
fn all_four_macros_keep_13_flat_children() {
    verify(
        stacks!(
            text("0"),
            text("1"),
            text("2"),
            text("3"),
            text("4"),
            text("5"),
            text("6"),
            text("7"),
            text("8"),
            text("9"),
            text("10"),
            text("11"),
            text("12")
        ),
        13,
    );
}

#[test]
fn all_four_macros_keep_64_flat_children() {
    verify(
        stacks!(
            text("0"),
            text("1"),
            text("2"),
            text("3"),
            text("4"),
            text("5"),
            text("6"),
            text("7"),
            text("8"),
            text("9"),
            text("10"),
            text("11"),
            text("12"),
            text("13"),
            text("14"),
            text("15"),
            text("16"),
            text("17"),
            text("18"),
            text("19"),
            text("20"),
            text("21"),
            text("22"),
            text("23"),
            text("24"),
            text("25"),
            text("26"),
            text("27"),
            text("28"),
            text("29"),
            text("30"),
            text("31"),
            text("32"),
            text("33"),
            text("34"),
            text("35"),
            text("36"),
            text("37"),
            text("38"),
            text("39"),
            text("40"),
            text("41"),
            text("42"),
            text("43"),
            text("44"),
            text("45"),
            text("46"),
            text("47"),
            text("48"),
            text("49"),
            text("50"),
            text("51"),
            text("52"),
            text("53"),
            text("54"),
            text("55"),
            text("56"),
            text("57"),
            text("58"),
            text("59"),
            text("60"),
            text("61"),
            text("62"),
            text("63")
        ),
        64,
    );
}

#[derive(Clone, Copy)]
struct Counter(usize);
impl Component for Counter {
    fn body(self) -> impl View {
        let value = State::new(0usize);
        button(text(format!("{}:{}", self.0, value.get())), move || {
            value.set(value.get() + 1)
        })
    }
}

fn short() -> Erased {
    erased(
        vstack!(
            Counter(0),
            Counter(1),
            Counter(2),
            Counter(3),
            Counter(4),
            Counter(5),
            Counter(6),
            Counter(7),
            Counter(8),
            Counter(9),
            Counter(10),
            Counter(11)
        )
        .id("list"),
    )
}
fn long(hole: bool) -> Erased {
    erased(
        vstack!(
            Counter(0),
            Counter(1),
            Counter(2),
            Counter(3),
            Counter(4),
            Counter(5),
            Counter(6),
            Counter(7),
            Counter(8),
            Counter(9),
            Counter(10),
            (!hole).then_some(Counter(11)),
            Counter(12)
        )
        .id("list"),
    )
}

#[test]
fn adding_the_thirteenth_child_keeps_the_first_twelve_addresses_and_state() {
    let runtime = Runtime::new();
    let before = runtime.settled_layout(&short(), Proposal::unspecified());
    assert_eq!(before.hits.len(), 12);
    let hit = before.hits[11].1;
    let x = hit.origin.x + hit.size.width / 2.0;
    let y = hit.origin.y + hit.size.height / 2.0;
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
    let after = runtime.settled_layout(&long(false), Proposal::unspecified());
    assert_eq!(after.hits.len(), 13);
    for (before, after) in before.hits.iter().zip(&after.hits) {
        assert_eq!(before.0, after.0);
    }
    let content = words(&after.display);
    assert_eq!(content[11], "11:1");
    assert_eq!(content[12], "12:0");
    assert!(content[..11].iter().all(|text| text.ends_with(":0")));
    let without = runtime.settled_layout(&long(true), Proposal::unspecified());
    assert_eq!(
        without.hits.last().unwrap().0,
        after.hits[12].0,
        "an empty Option retains its index across a chunk boundary"
    );
}
