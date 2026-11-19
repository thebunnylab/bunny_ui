//! A menu opened from outside any event: shown at its point, fired by a pick, closed by an
//! outside press.
use super::Runtime;
use crate::layout::{DrawCommand, Point, Size};
use crate::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
struct Blank;

impl Component for Blank {
    fn body(self) -> impl View {
        crate::views::spacer()
    }
}

fn rows(display: &crate::layout::DisplayList) -> Vec<(String, Point)> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, origin, .. } => Some((content.to_string(), *origin)),
            _ => None,
        })
        .collect()
}

#[test]
fn open_menu_at_shows_the_rows_at_the_point_and_fires_the_one_picked() {
    let picked = Rc::new(RefCell::new(Vec::new()));
    let runtime = Runtime::new();
    let size = Size { width: 300.0, height: 200.0 };
    assert!(rows(&runtime.display_frame(&Blank, size)).is_empty());
    let (first, second) = (Rc::clone(&picked), Rc::clone(&picked));
    runtime.open_menu_at(
        Point { x: 40.0, y: 30.0 },
        vec![
            crate::views::menu_item("Open", move || first.borrow_mut().push("open")),
            crate::views::menu_divider(),
            crate::views::menu_item("Copy", move || second.borrow_mut().push("copy")),
        ],
    );
    assert!(runtime.needs_frame(), "a menu opened outside an event asks for a frame");
    let open = rows(&runtime.display_frame(&Blank, size));
    let copy = open.iter().find(|(label, _)| label == "Copy").expect("the rows paint").1;
    assert!(open.iter().any(|(label, _)| label == "Open"));
    assert!(copy.x > 40.0 && copy.y > 30.0, "the menu begins at the point: {copy:?}");
    runtime.pointer_clicked(copy.x + 4.0, copy.y, 1, false);
    runtime.pointer_released(copy.x + 4.0, copy.y);
    assert_eq!(*picked.borrow(), ["copy"]);
    assert!(rows(&runtime.display_frame(&Blank, size)).is_empty(), "the pick closed it");
}

#[test]
fn open_menu_at_closes_on_a_press_outside_without_firing() {
    let picked = Rc::new(RefCell::new(Vec::new()));
    let runtime = Runtime::new();
    let size = Size { width: 300.0, height: 200.0 };
    let pick = Rc::clone(&picked);
    runtime.open_menu_at(
        Point { x: 10.0, y: 10.0 },
        vec![crate::views::menu_item("Open", move || pick.borrow_mut().push("open"))],
    );
    assert!(!rows(&runtime.display_frame(&Blank, size)).is_empty());
    runtime.pointer_clicked(280.0, 190.0, 1, false);
    runtime.pointer_released(280.0, 190.0);
    assert!(rows(&runtime.display_frame(&Blank, size)).is_empty());
    assert!(picked.borrow().is_empty());
}
