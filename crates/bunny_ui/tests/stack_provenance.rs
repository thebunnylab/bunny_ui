//! Convenience constructors preserve application sites and actual layout.
use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::provenance::{NodeKind, Site};
use bunny_ui::runtime::Runtime;
use bunny_ui_core as bunny_ui;

fn commands(runtime: &Runtime, view: &impl View) -> Vec<String> {
    runtime
        .layout(
            view,
            Proposal::exact(Size {
                width: 320.0,
                height: 200.0,
            }),
        )
        .display
        .as_slice()
        .iter()
        .map(|command| format!("{command:?}"))
        .collect()
}

fn assert_stack_caller(view: &impl View, direct: &impl View, kind: NodeKind, line: u32) {
    let runtime = Runtime::new();
    runtime.record_sites(true);
    let actual = commands(&runtime, view);
    let sites = runtime.sites();
    let stack = sites
        .iter()
        .find(|site| site.kind == kind)
        .expect("the stack is recorded");
    let Site::Caller(caller) = stack.site else {
        panic!("a constructor retains its caller")
    };
    assert_eq!(caller.file(), file!());
    assert_eq!(caller.line(), line);
    assert_eq!(caller.column(), 16);
    assert_eq!(actual, commands(&Runtime::new(), direct));
}

#[test]
fn vertical_convenience_constructor_keeps_the_application_call_site() {
    let line = line!() + 1;
    let view = Stack::vertical((text("A"), text("B")));
    assert_stack_caller(
        &view,
        &vstack((text("A"), text("B"))),
        NodeKind::VStack,
        line,
    );
}

#[test]
fn horizontal_convenience_constructor_keeps_the_application_call_site() {
    let line = line!() + 1;
    let view = Stack::horizontal((text("A"), text("B")));
    assert_stack_caller(
        &view,
        &hstack((text("A"), text("B"))),
        NodeKind::HStack,
        line,
    );
}

#[test]
fn layered_convenience_constructor_keeps_the_application_call_site() {
    let line = line!() + 1;
    let view = Stack::layered((text("A"), text("B")));
    assert_stack_caller(
        &view,
        &zstack((text("A"), text("B"))),
        NodeKind::ZStack,
        line,
    );
}

#[test]
fn repeated_nested_convenience_stacks_keep_one_source_and_distinct_occurrences() {
    let runtime = Runtime::new();
    runtime.record_sites(true);
    let row = || Stack::horizontal((text("A"), text("B")));
    let view = Stack::vertical((row().id("first"), row().id("second")));
    let _ = commands(&runtime, &view);
    let sites = runtime.sites();
    let rows: Vec<_> = sites
        .iter()
        .filter(|site| site.kind == NodeKind::HStack)
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].site, rows[1].site);
    assert_ne!(rows[0].index, rows[1].index);
    assert_ne!(rows[0].frame, rows[1].frame);
    for site in rows {
        let Site::Caller(caller) = site.site else {
            panic!("caller")
        };
        assert_eq!(caller.file(), file!());
    }
}
