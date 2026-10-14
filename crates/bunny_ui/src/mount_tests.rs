//! Mount actions belong to a view identity, not to a body evaluation.
use crate::prelude::*;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
struct Row {
    name: String,
    label: State<String>,
    mounted: Rc<RefCell<Vec<String>>>,
}
impl Component for Row {
    fn body(self) -> impl View {
        let mounted = self.mounted.clone();
        let name = self.name.clone();
        text(self.label.get()).on_mount(move || mounted.borrow_mut().push(name.clone()))
    }
}
#[derive(Clone)]
struct Board {
    items: State<Vec<String>>,
    label: State<String>,
    mounted: Rc<RefCell<Vec<String>>>,
}
impl Component for Board {
    fn body(self) -> impl View {
        vstack!(for_each(
            self.items.get(),
            |name| name.clone(),
            move |name| Row {
                name: name.clone(),
                label: self.label,
                mounted: self.mounted.clone(),
            }
        ))
    }
}
#[test]
fn mount_runs_once_through_rebuild_reorder_and_again_after_removal() {
    let board = Board {
        items: State::new(vec!["A".into(), "B".into()]),
        label: State::new("Ready".into()),
        mounted: Rc::default(),
    };
    let runtime = Runtime::scene("mount-board");
    runtime.render_stable(&board);
    assert_eq!(*board.mounted.borrow(), ["A", "B"]);
    board.label.set("Edited".into());
    runtime.render_stable(&board);
    board.items.set(vec!["B".into(), "A".into()]);
    runtime.render_stable(&board);
    assert_eq!(*board.mounted.borrow(), ["A", "B"]);
    board.items.set(vec!["B".into()]);
    runtime.render_stable(&board);
    board.items.set(vec!["B".into(), "A".into()]);
    runtime.render_stable(&board);
    assert_eq!(*board.mounted.borrow(), ["A", "B", "A"]);
}
#[test]
fn mount_slots_are_independent_between_scene_instances() {
    let row = Row {
        name: "same source".into(),
        label: State::new("Ready".into()),
        mounted: Rc::default(),
    };
    let first = Runtime::scene("mount-first");
    let second = Runtime::scene("mount-second");
    first.render_stable(&row);
    second.render_stable(&row);
    row.label.set("Edited".into());
    first.render_stable(&row);
    second.render_stable(&row);
    assert_eq!(row.mounted.borrow().len(), 2);
}

#[test]
fn keyed_mount_helpers_keep_distinct_slots_and_can_write_state() {
    fn helper(key: &'static str, count: State<u32>) -> impl View {
        text(key).on_mount_keyed(key, move || count.set(count.get() + 1))
    }
    #[derive(Clone, Copy)]
    struct Pair {
        count: State<u32>,
    }
    impl Component for Pair {
        fn body(self) -> impl View {
            vstack!(
                text(self.count.get().to_string()),
                helper("first", self.count),
                helper("second", self.count)
            )
        }
    }
    let pair = Pair {
        count: State::new(0),
    };
    let runtime = Runtime::scene("keyed-mount");
    runtime.render_stable(&pair);
    assert_eq!(pair.count.get(), 2);
    pair.count.set(8);
    runtime.render_stable(&pair);
    assert_eq!(pair.count.get(), 8);
}
