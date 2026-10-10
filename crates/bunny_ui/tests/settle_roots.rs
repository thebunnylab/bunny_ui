//! One runtime, two roots. A settle is quiet when nothing could have moved
//! a body of THE ROOT BEING SETTLED — the previous root's stillness says
//! nothing about a root the runtime has never run. A probe that lays out
//! one surface and then another used to have the second one's settle
//! skipped (its tasks never ran) and its layout answered with the first
//! surface's frame.
extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const SIZE: Size = Size { width: 300.0, height: 100.0 };

#[derive(Clone)]
struct First;

impl Component for First {
    fn body(self) -> impl View {
        text!("first").on_click(|| {}).id("first-root")
    }
}

#[derive(Clone)]
struct Second {
    landed: State<bool>,
}

impl Component for Second {
    fn body(self) -> impl View {
        let landed = self.landed;
        text!("second").on_click(|| {}).id("second-root").task(move || async move { landed.set(true) })
    }
}

/// A root of plain views, rebuilt by the caller with the key its task is
/// declared under — what a probe does when it lays out a view it builds
/// from the shell's state outside any pass. Such a root is rebuilt whole
/// by every pass, and so is what it declares.
fn plain(key: &str, runs: State<u32>) -> impl View {
    vstack!(text!("plain").task_id(key.to_owned(), move || async move { runs.update(|n| *n += 1) }))
}

#[test]
fn a_plain_root_rebuilt_with_another_task_starts_it() {
    let runtime = Runtime::new();
    let runs = State::new(0u32);
    let _ = runtime.settled_layout(&plain("first", runs), Proposal::exact(SIZE));
    assert_eq!(runs.get(), 1, "the first task ran in the first settle");
    // nothing was written through state: the caller simply built another
    // root, with another task under another key — a quiet settle would
    // never have seen it
    let _ = runtime.settled_layout(&plain("second", runs), Proposal::exact(SIZE));
    assert_eq!(runs.get(), 2, "a plain root rebuilt with another task starts that task");
}

fn hit(runtime: &Runtime, view: &impl View, name: &str) -> bool {
    runtime.settled_layout(view, Proposal::exact(SIZE)).hits.iter().any(|(path, _)| path.contains(name))
}

#[test]
fn a_second_root_on_the_same_runtime_settles_and_lays_out_as_itself() {
    let runtime = Runtime::new();
    let _ = runtime.settled_layout(&First, Proposal::exact(SIZE));
    let _ = runtime.settled_layout(&First, Proposal::exact(SIZE));
    let landed = State::new(false);
    let second = Second { landed };
    let laid = runtime.settled_layout(&second, Proposal::exact(SIZE));
    assert!(landed.get(), "the second root's task ran in its own settle");
    assert!(laid.hits.iter().any(|(path, _)| path.contains("[second-root]")), "the frame is the second root's: {:?}", laid.hits);
    assert!(!laid.hits.iter().any(|(path, _)| path.contains("[first-root]")), "the first root's frame was served for the second");
    // and back: the first root is itself again
    assert!(hit(&runtime, &First, "[first-root]"), "the first root lays out as itself after the second");
}
