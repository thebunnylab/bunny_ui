//! A component that leaves gives back what it held, whoever re-ran.
//!
//! A find bar opens over an editor and borrows the keyboard. When the bar
//! closes, the bar's field must leave the retention, and the keyboard
//! must go back to the editor. This must hold when the body that closes
//! the bar runs alone, and when a list above it runs in the same pass and
//! keeps the row the body is in.

use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 240.0,
};

fn holds(runtime: &Runtime, name: &str) -> bool {
    runtime
        .focused()
        .is_some_and(|path| path.split('/').any(|part| part == format!("[{name}]")))
}

/// The bar: a component of its own that holds the query field.
#[derive(Clone, Copy)]
struct Bar {
    row: usize,
    query: State<String>,
}

impl Component for Bar {
    fn body(self) -> impl View {
        text_field("Find", self.query.binding())
            .id(format!("query-{}", self.row))
            .auto_focus_beat(1)
    }
}

/// The body that opens and closes the bar. It is the only reader of `open`.
#[derive(Clone, Copy)]
struct Pane {
    row: usize,
    open: State<bool>,
    query: State<String>,
    body: State<String>,
}

impl Pane {
    fn new(row: usize) -> Self {
        Self {
            row,
            open: State::new(false),
            query: State::new(String::new()),
            body: State::new(String::new()),
        }
    }
}

impl Component for Pane {
    fn body(self) -> impl View {
        let bar = self.open.get().then_some(Bar {
            row: self.row,
            query: self.query,
        });
        vstack!(
            bar,
            text_field("Body", self.body.binding()).id(format!("editor-{}", self.row)),
        )
    }
}

/// A clean row around a pane: it reads nothing, so a list keeps it.
#[derive(Clone, Copy)]
struct Row {
    pane: Pane,
}

impl Component for Row {
    fn body(self) -> impl View {
        self.pane
    }
}

#[derive(Clone, Copy)]
struct Page {
    order: State<Rc<Vec<usize>>>,
    panes: [Pane; 2],
}

impl Component for Page {
    fn body(self) -> impl View {
        let panes = self.panes;
        for_each(self.order, |row| row.to_string(), move |row: &usize| Row { pane: panes[*row] })
    }
}

fn page() -> Page {
    Page {
        order: State::new(Rc::new(vec![0, 1])),
        panes: [Pane::new(0), Pane::new(1)],
    }
}

/// Opens the bar of row 0 over its editor, which holds the keyboard.
fn open_the_bar(runtime: &Runtime, page: &Page) -> usize {
    runtime.display_frame(page, SIZE);
    assert!(runtime.focus_named("editor-0"));
    page.panes[0].open.set(true);
    runtime.display_frame(page, SIZE);
    assert!(holds(runtime, "query-0"), "the bar borrowed the keyboard: {:?}", runtime.focused());
    runtime.retained_counts().live_editors
}

fn the_keys_went_back(runtime: &Runtime, page: &Page, editors_open: usize) {
    assert!(
        holds(runtime, "editor-0"),
        "the bar left, and the keyboard went back to the editor; it is on {:?}",
        runtime.focused()
    );
    assert_eq!(
        runtime.retained_counts().live_editors,
        editors_open - 1,
        "the query's editor left with the bar"
    );
    assert!(runtime.key(EditCommand::Insert("x".into())).applied);
    assert_eq!(page.panes[0].body.get(), "x", "and the next key lands in the editor");
    assert_eq!(page.panes[0].query.get(), "", "not in a field that is gone");
}

#[test]
fn a_bar_closed_while_its_parent_re_runs_alone_gives_the_keys_back() {
    let runtime = Runtime::new();
    let page = page();
    let editors_open = open_the_bar(&runtime, &page);

    // only the pane reads `open`: its body runs alone under a clean row
    page.panes[0].open.set(false);
    runtime.display_frame(&page, SIZE);
    the_keys_went_back(&runtime, &page, editors_open);
}

#[test]
fn a_bar_closed_inside_a_kept_row_while_its_list_runs_gives_the_keys_back() {
    let runtime = Runtime::new();
    let page = page();
    let editors_open = open_the_bar(&runtime, &page);

    // one pass: the list reorders and keeps both clean rows, and the
    // pane inside row 0 closes its bar
    page.order.set(Rc::new(vec![1, 0]));
    page.panes[0].open.set(false);
    runtime.display_frame(&page, SIZE);
    the_keys_went_back(&runtime, &page, editors_open);
}
