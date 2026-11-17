#![cfg(feature = "canvas")]
//! A page painted in one box, heard through the nodes it answers for.
extern crate bunny_ui_core as bunny_ui;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bunny_ui::accessibility::{AccessibleNode, Action, ActionError, Node, Role};
use bunny_ui::custom::{CustomElement, Painter, PaintCtx};
use bunny_ui::prelude::*;

const SIZE: Size = Size { width: 300.0, height: 200.0 };

fn area(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect { origin: Point { x, y }, size: Size { width, height } }
}

#[derive(Default)]
struct Page {
    headline: RefCell<String>,
    focused_link: RefCell<Option<u64>>,
    followed: RefCell<Vec<(u64, Action)>>,
    asked: Cell<u32>,
}

#[derive(Clone)]
struct Surface {
    page: Rc<Page>,
    keys: bool,
}

impl CustomElement for Surface {
    fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}

    fn accepts_keys(&self) -> bool {
        self.keys
    }

    fn accessibility_nodes(&self, size: Size) -> Vec<AccessibleNode> {
        self.page.asked.set(self.page.asked.get() + 1);
        let focused = *self.page.focused_link.borrow();
        vec![AccessibleNode::group(1, area(0.0, 0.0, size.width, size.height)).children([
            AccessibleNode::new(2, Role::Heading, area(8.0, 8.0, 200.0, 24.0))
                .label(self.page.headline.borrow().as_str())
                .level(1),
            AccessibleNode::new(3, Role::Link, area(8.0, 40.0, 80.0, 16.0))
                .label("Read more")
                .focused(focused == Some(3)),
            AccessibleNode::new(4, Role::Text, area(8.0, 64.0, 200.0, 16.0)).label("Posted today"),
            AccessibleNode::new(5, Role::Link, area(8.0, 400.0, 80.0, 16.0)).label("Far below"),
        ])]
    }

    fn accessibility_node_action(&self, key: u64, action: &Action) -> Result<(), ActionError> {
        if key == 4 {
            return Err(ActionError::Unsupported);
        }
        if *action == Action::Focus {
            *self.page.focused_link.borrow_mut() = Some(key);
        }
        self.page.followed.borrow_mut().push((key, action.clone()));
        Ok(())
    }
}

#[derive(Clone)]
struct Screen {
    surface: Surface,
}

impl Component for Screen {
    fn body(self) -> impl View {
        vstack!(text("Toolbar"), custom(self.surface).frame(300.0, 160.0).id("page"))
    }
}

fn setup(keys: bool) -> (Runtime, Screen, Rc<Page>) {
    let page = Rc::new(Page::default());
    *page.headline.borrow_mut() = "Hop ships".into();
    let screen = Screen { surface: Surface { page: Rc::clone(&page), keys } };
    let runtime = Runtime::new();
    runtime.drop_unseen();
    runtime.set_accessibility_enabled(true);
    runtime.display_frame(&screen, SIZE);
    (runtime, screen, page)
}

fn inside(runtime: &Runtime) -> Vec<Node> {
    runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .filter(|node| node.label.as_ref() != "Toolbar")
        .cloned()
        .collect()
}

#[test]
fn the_box_is_heard_through_its_nodes_in_reading_order_and_in_the_window() {
    let (runtime, _, _) = setup(false);
    let tree = runtime.accessibility_tree();
    let labels: Vec<_> = tree.nodes().iter().map(|node| (node.role, node.label.as_ref())).collect();
    assert_eq!(
        labels,
        [
            (Role::Text, "Toolbar"),
            (Role::Heading, "Hop ships"),
            (Role::Link, "Read more"),
            (Role::Text, "Posted today"),
        ],
        "the group is heard through its children and a link past the window's edge not at all"
    );
    let nodes = inside(&runtime);
    let toolbar = tree.nodes()[0].bounds;
    assert!(nodes[0].bounds.origin.y >= toolbar.origin.y + toolbar.size.height, "moved under the toolbar, into the scene");
    assert_eq!(nodes[0].bounds.size, Size { width: 200.0, height: 24.0 });
    assert_eq!(nodes[0].level, Some(1));
    assert_eq!(nodes[1].level, None);
}

#[test]
fn a_node_keeps_its_identity_while_its_words_change() {
    let (runtime, screen, page) = setup(false);
    let before = inside(&runtime)[0].clone();
    *page.headline.borrow_mut() = "Hop ships audio".into();
    runtime.request_frame();
    runtime.display_frame(&screen, SIZE);
    let after = inside(&runtime)[0].clone();
    assert_eq!(after.id, before.id);
    assert_eq!(after.label.as_ref(), "Hop ships audio");
}

#[test]
fn actions_reach_the_element_with_the_key_of_their_node() {
    let (runtime, _, page) = setup(false);
    let nodes = inside(&runtime);
    let (heading, link, text) = (nodes[0].id, nodes[1].id, nodes[2].id);
    runtime.accessibility_action(link, Action::Activate).unwrap();
    assert_eq!(*page.followed.borrow(), [(3, Action::Activate)]);
    assert_eq!(runtime.accessibility_action(heading, Action::Activate), Err(ActionError::Unsupported));
    assert_eq!(runtime.accessibility_action(text, Action::Focus), Err(ActionError::Unsupported));
    assert_eq!(page.followed.borrow().len(), 1, "a refused request never reaches the element");
}

#[test]
fn a_focused_link_is_heard_focused_while_its_box_holds_the_keyboard() {
    let (runtime, screen, page) = setup(true);
    let link = inside(&runtime)[1].id;
    runtime.accessibility_action(link, Action::Focus).unwrap();
    assert_eq!(*page.focused_link.borrow(), Some(3));
    runtime.display_frame(&screen, SIZE);
    let focused: Vec<_> = runtime.accessibility_tree().nodes().iter().filter(|node| node.focused).map(|node| node.label.to_string()).collect();
    assert_eq!(focused, ["Read more"], "the box took the keyboard, and the element says which node holds it");
}

#[test]
fn without_accessibility_the_element_is_never_asked() {
    let page = Rc::new(Page::default());
    let screen = Screen { surface: Surface { page: Rc::clone(&page), keys: false } };
    let runtime = Runtime::new();
    runtime.display_frame(&screen, SIZE);
    assert!(runtime.accessibility_tree().nodes().is_empty());
    assert_eq!(page.asked.get(), 0);
}
