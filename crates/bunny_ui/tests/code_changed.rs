//! A hot reload loads a new build of the app next to the old one and
//! says so with `code_changed`: every body runs again with the new code,
//! and what the app holds stays — also where an edit moved the line of
//! the call that made it.
//!
//! The two arms of an `if` stand for one call before and after an edit
//! that inserted a line above it: the same file, the same column, a line
//! further down.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn new_code_runs_every_body_again_and_keeps_the_state() {
    #[derive(Clone)]
    struct Counter {
        count: State<i32>,
        runs: Rc<Cell<usize>>,
    }
    impl Component for Counter {
        fn body(self) -> impl View {
            self.runs.set(self.runs.get() + 1);
            text!("Count: {}", self.count)
        }
    }
    let runtime = Runtime::scene("code_changed_bodies");
    let view = Counter { count: State::new(3), runs: Rc::new(Cell::new(0)) };
    runtime.render_stable(&view);
    runtime.render_stable(&view);
    assert_eq!(view.runs.get(), 1, "nothing changed: the body ran once");

    bunny_ui::code_changed();
    assert!(runtime.frame_need().code, "new code is a reason for a frame");
    let text = runtime.render_stable(&view);
    assert_eq!(view.runs.get(), 2, "new code: the body runs again");
    assert!(text.contains("Count: 3"), "and the state is where it was: {text}");
    assert!(!runtime.frame_need().code, "once");
}

#[derive(Clone, Copy)]
struct Model {
    count: State<i32>,
}

#[derive(Clone)]
struct Moved {
    edited: State<bool>,
    model: Rc<Cell<Option<Model>>>,
}

impl Component for Moved {
    fn body(self) -> impl View {
        let model = if !self.edited.get() {
            view_model(|| Model { count: State::new(0) })
        } else {
            // the line the edit inserted
            view_model(|| Model { count: State::new(0) })
        };
        self.model.set(Some(model));
        text!("Count: {}", model.count)
    }
}

#[test]
fn a_view_model_stays_when_an_edit_moves_its_line() {
    let runtime = Runtime::scene("code_changed_model");
    let view = Moved { edited: State::new(false), model: Rc::new(Cell::new(None)) };
    runtime.render_stable(&view);
    view.model.get().unwrap().count.set(7);

    view.edited.set(true);
    bunny_ui::code_changed();
    let text = runtime.render_stable(&view);
    assert!(text.contains("Count: 7"), "{text}");
    assert_eq!(view.model.get().unwrap().count.get(), 7);
}

#[derive(Clone)]
struct Worker {
    edited: State<bool>,
    starts: Rc<Cell<usize>>,
}

impl Component for Worker {
    fn body(self) -> impl View {
        let starts = Rc::clone(&self.starts);
        let work = move || {
            let starts = Rc::clone(&starts);
            async move {
                starts.set(starts.get() + 1);
                std::future::pending::<()>().await
            }
        };
        if !self.edited.get() {
            text("working").task(work)
        } else {
            // the line the edit inserted
            text("working").task(work)
        }
    }
}

#[test]
fn a_task_keeps_running_when_an_edit_moves_its_line() {
    let runtime = Runtime::scene("code_changed_task");
    let view = Worker { edited: State::new(false), starts: Rc::new(Cell::new(0)) };
    runtime.render_stable(&view);
    assert_eq!(view.starts.get(), 1);

    view.edited.set(true);
    bunny_ui::code_changed();
    runtime.render_stable(&view);
    assert_eq!(view.starts.get(), 1, "the moved callsite took the running task");
    assert_eq!(motor::task::pending(), 1, "one task, not one per build");
}

#[test]
fn without_new_code_a_new_line_is_a_new_task() {
    let runtime = Runtime::scene("code_changed_control");
    let view = Worker { edited: State::new(false), starts: Rc::new(Cell::new(0)) };
    runtime.render_stable(&view);
    view.edited.set(true);
    runtime.render_stable(&view);
    assert_eq!(view.starts.get(), 2, "another callsite: its own task");
    assert_eq!(motor::task::pending(), 1, "and the old one was cancelled");
}
