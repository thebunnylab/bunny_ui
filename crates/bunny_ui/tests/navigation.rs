//! `navigation_stack(path, root).destination(…)`: a stack of pages.

use std::cell::Cell;

use bunny_ui::accessibility::Role;
use bunny_ui::action::ActionId;
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 480.0,
};
const PING: ActionId = ActionId("test.ping");

thread_local! {
    static APP_RUNS: Cell<u32> = const { Cell::new(0) };
    static HOME_RUNS: Cell<u32> = const { Cell::new(0) };
    static ITEM_RUNS: Cell<u32> = const { Cell::new(0) };
    static PINGS: Cell<u32> = const { Cell::new(0) };
}

fn runs(counter: &'static std::thread::LocalKey<Cell<u32>>) -> u32 {
    counter.with(Cell::get)
}

fn count(counter: &'static std::thread::LocalKey<Cell<u32>>) {
    counter.with(|runs| runs.set(runs.get() + 1));
}

#[derive(Clone, Debug, PartialEq)]
enum Route {
    Item(u32),
    About,
}

/// The root page: a counter of its own, a field, and two links.
#[derive(Clone, Copy)]
struct Home {
    note: State<String>,
}

impl Component for Home {
    fn body(self) -> impl View {
        count(&HOME_RUNS);
        let taps = State::new(0u32);
        vstack!(
            text!("Home taps {}", taps),
            button(text("Tap home"), move || taps.add(1)),
            text_field("Note", self.note.binding()).id("note"),
            nav_link_value(text("Open item 1"), Route::Item(1)),
            nav_link_value(text("Open about"), Route::About),
        )
        .on_action(PING, || count(&PINGS))
        .navigation_title("Home")
    }
}

/// The page of an item: a counter of its own and a link to the next item.
#[derive(Clone, Copy)]
struct Item {
    number: u32,
    more: State<bool>,
}

impl Component for Item {
    fn body(self) -> impl View {
        count(&ITEM_RUNS);
        let taps = State::new(0u32);
        vstack!(
            text!("Item {} taps {}", self.number, taps),
            button(text("Tap item"), move || taps.add(1)),
            nav_link_value(text("Open next"), Route::Item(self.number + 1)),
            button(text("More"), move || self.more.set(true)).popover(self.more.binding(), Side::Bottom, |_| {
                erased(text("More about it").padding())
            }),
        )
        // the title reads the page's own state: it changes with it
        .navigation_title(format!("Item {}, {} taps", self.number, taps.get()))
    }
}

#[derive(Clone, Copy)]
struct App {
    path: State<Vec<Route>>,
    note: State<String>,
    more: State<bool>,
}

impl App {
    fn new() -> Self {
        Self { path: State::new(Vec::new()), note: State::new(String::new()), more: State::new(false) }
    }
}

impl Component for App {
    fn body(self) -> impl View {
        count(&APP_RUNS);
        let more = self.more;
        navigation_stack(self.path, Home { note: self.note }).destination(move |route| match route {
            Route::Item(number) => Either::First(Item { number, more }),
            Route::About => Either::Second(text("About this app")),
        })
    }
}

fn lines(runtime: &Runtime, app: &App) -> Vec<String> {
    runtime
        .display_frame(app, SIZE)
        .as_slice()
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, .. } => Some(content[range.0..range.1].to_string()),
            _ => None,
        })
        .collect()
}

fn shows(runtime: &Runtime, app: &App, line: &str) -> bool {
    lines(runtime, app).iter().any(|shown| shown == line)
}

/// The bounds of the element named `label`, as assistive technology has it.
fn named(runtime: &Runtime, app: &App, label: &str) -> Rect {
    runtime.display_frame(app, SIZE);
    let tree = runtime.accessibility_tree();
    tree.nodes()
        .iter()
        .find(|node| &*node.label == label)
        .unwrap_or_else(|| panic!("nothing is named {label:?}"))
        .bounds
}

fn heard(runtime: &Runtime, app: &App) -> Vec<(Role, String)> {
    runtime.display_frame(app, SIZE);
    runtime.accessibility_tree().nodes().iter().map(|node| (node.role, node.label.to_string())).collect()
}

fn press(runtime: &Runtime, app: &App, label: &str) {
    let rect = named(runtime, app, label);
    let (x, y) = (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
    runtime.display_frame(app, SIZE);
}

fn keyboard(runtime: &Runtime, app: &App) -> Option<String> {
    runtime.display_frame(app, SIZE);
    runtime.accessibility_tree().nodes().iter().find(|node| node.focused).map(|node| node.label.to_string())
}

fn runtime() -> Runtime {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime
}

#[test]
fn the_stack_shows_the_root_and_then_the_page_of_the_last_route() {
    let runtime = runtime();
    let app = App::new();
    assert!(shows(&runtime, &app, "Home taps 0"));
    assert!(!shows(&runtime, &app, "Item 1 taps 0"));

    app.path.push(Route::Item(1));
    let shown = lines(&runtime, &app);
    assert!(shown.iter().any(|line| line == "Item 1 taps 0"), "{shown:?}");
    assert!(!shown.iter().any(|line| line == "Home taps 0"), "the root is under it: {shown:?}");

    app.path.push(Route::About);
    assert!(shows(&runtime, &app, "About this app"));
    assert!(!shows(&runtime, &app, "Item 1 taps 0"));

    assert!(app.path.pop());
    assert!(shows(&runtime, &app, "Item 1 taps 0"));
    assert!(app.path.pop_to_root());
    assert!(shows(&runtime, &app, "Home taps 0"));
    assert!(!app.path.pop(), "the root stays");
    assert!(!app.path.pop_to_root());

    // a deep link is one write
    app.path.set(vec![Route::Item(7), Route::Item(8)]);
    assert!(shows(&runtime, &app, "Item 8 taps 0"));
}

#[test]
fn a_link_pushes_its_route_on_the_stack_it_stands_in() {
    let runtime = runtime();
    let app = App::new();
    press(&runtime, &app, "Open item 1");
    assert_eq!(app.path.get(), [Route::Item(1)]);
    press(&runtime, &app, "Open next");
    assert_eq!(app.path.get(), [Route::Item(1), Route::Item(2)]);
    assert!(shows(&runtime, &app, "Item 2 taps 0"));

    // the pages leave with the keyboard in them: it goes back to the
    // link that opened the first one
    app.path.pop_to_root();
    assert_eq!(keyboard(&runtime, &app).as_deref(), Some("Open item 1"));
    // and the keyboard presses a link too
    runtime.offer_key(KeyPattern::key(Key::Tab));
    assert_eq!(keyboard(&runtime, &app).as_deref(), Some("Open about"));
    assert!(runtime.offer_key(KeyPattern::key(Key::Enter)));
    assert_eq!(app.path.get(), [Route::About]);
}

#[test]
fn a_covered_page_keeps_its_state_and_a_page_that_leaves_takes_its_own() {
    let runtime = runtime();
    let app = App::new();
    press(&runtime, &app, "Tap home");
    press(&runtime, &app, "Tap home");
    assert!(shows(&runtime, &app, "Home taps 2"));

    app.path.push(Route::Item(1));
    press(&runtime, &app, "Tap item");
    assert!(shows(&runtime, &app, "Item 1 taps 1"));
    app.path.push(Route::About);
    assert!(shows(&runtime, &app, "About this app"));
    app.path.pop();
    assert!(shows(&runtime, &app, "Item 1 taps 1"), "the page that was covered kept its count");

    app.path.pop();
    assert!(shows(&runtime, &app, "Home taps 2"), "and so did the root");
    app.path.push(Route::Item(1));
    assert!(shows(&runtime, &app, "Item 1 taps 0"), "the same route again is a new page");
}

#[test]
fn another_route_at_the_same_place_is_another_page() {
    let runtime = runtime();
    let app = App::new();
    app.path.set(vec![Route::Item(1), Route::Item(2)]);
    press(&runtime, &app, "Tap item");
    assert!(shows(&runtime, &app, "Item 2 taps 1"));
    app.path.pop();
    press(&runtime, &app, "Tap item");
    press(&runtime, &app, "Tap item");
    assert!(shows(&runtime, &app, "Item 1 taps 2"));

    // [1, 3]: the page of 1 stays what it was, the page of 3 is new
    app.path.set(vec![Route::Item(1), Route::Item(3)]);
    assert!(shows(&runtime, &app, "Item 3 taps 0"));
    app.path.pop();
    assert!(shows(&runtime, &app, "Item 1 taps 2"));
}

#[test]
fn a_push_and_a_pop_run_the_stack_and_no_page_that_did_not_change() {
    let runtime = runtime();
    let app = App::new();
    runtime.display_frame(&app, SIZE);
    let (app_runs, home_runs) = (runs(&APP_RUNS), runs(&HOME_RUNS));

    app.path.push(Route::Item(1));
    runtime.display_frame(&app, SIZE);
    let item_runs = runs(&ITEM_RUNS);
    app.path.push(Route::Item(2));
    runtime.display_frame(&app, SIZE);
    app.path.pop();
    runtime.display_frame(&app, SIZE);
    app.path.pop();
    runtime.display_frame(&app, SIZE);

    assert_eq!(runs(&APP_RUNS), app_runs, "the app's body did not run: the stack reads the path");
    assert_eq!(runs(&HOME_RUNS), home_runs, "the root page did not run while pages went over it and left");
    assert_eq!(runs(&ITEM_RUNS), item_runs + 1, "only the page of item 2 was made; the page of item 1 was kept");
}

#[test]
fn a_covered_page_is_not_on_the_screen_for_the_keyboard_or_for_assistive_technology() {
    let runtime = runtime();
    let app = App::new();
    app.path.push(Route::Item(1));
    let elements = heard(&runtime, &app);
    assert!(elements.iter().any(|(_, label)| label == "Tap item"));
    for gone in ["Tap home", "Note", "Open item 1", "Open about"] {
        assert!(!elements.iter().any(|(_, label)| label == gone), "{gone} is under the page: {elements:?}");
    }

    // Tab goes round the bar and the page on top, and never into the root
    let mut stops = Vec::new();
    for _ in 0..5 {
        assert!(runtime.offer_key(KeyPattern::key(Key::Tab)));
        stops.push(keyboard(&runtime, &app).expect("a stop of the page on top"));
    }
    assert_eq!(stops, ["Back", "Tap item", "Open next", "More", "Back"]);
    assert!(!runtime.focus_named("note"), "a field under the page cannot take the keyboard");
}

#[test]
fn the_keyboard_leaves_a_field_that_a_page_covers() {
    let runtime = runtime();
    let app = App::new();
    runtime.display_frame(&app, SIZE);
    assert!(runtime.focus_named("note"));
    assert!(runtime.focus_takes_text());
    app.path.push(Route::About);
    runtime.display_frame(&app, SIZE);
    assert_eq!(runtime.focused(), None, "typing must not reach a field nobody sees");
    assert!(!runtime.key(EditCommand::Insert("x".into())).applied);
    assert_eq!(app.note.get(), "");
}

#[test]
fn what_a_covered_page_mounted_answers_no_action() {
    let runtime = runtime();
    let app = App::new();
    runtime.display_frame(&app, SIZE);
    assert!(runtime.dispatch_action(PING), "the root is on top: its handler answers");
    assert_eq!(runs(&PINGS), 1);

    app.path.push(Route::About);
    runtime.display_frame(&app, SIZE);
    assert!(!runtime.dispatch_action(PING), "the root is covered: nobody answers");
    assert_eq!(runs(&PINGS), 1);

    app.path.pop();
    runtime.display_frame(&app, SIZE);
    assert!(runtime.dispatch_action(PING), "and it answers again when it is back");
    assert_eq!(runs(&PINGS), 2);
}

#[test]
fn back_takes_the_page_on_top_away_after_the_overlays() {
    let runtime = runtime();
    let app = App::new();
    runtime.display_frame(&app, SIZE);
    assert!(!runtime.navigate_back(), "the root has nothing under it");

    app.path.set(vec![Route::Item(1), Route::Item(2)]);
    press(&runtime, &app, "More");
    assert!(app.more.get());
    assert!(runtime.navigate_back());
    runtime.display_frame(&app, SIZE);
    assert!(!app.more.get(), "the popover first");
    assert_eq!(app.path.get().len(), 2);

    assert!(runtime.navigate_back());
    runtime.display_frame(&app, SIZE);
    assert_eq!(app.path.get(), [Route::Item(1)]);
    assert!(runtime.navigate_back());
    runtime.display_frame(&app, SIZE);
    assert!(app.path.get().is_empty());
    assert!(!runtime.navigate_back(), "and at the root the platform takes it");
}

#[test]
fn the_keys_of_back_leave_a_page_and_escape_does_not() {
    let runtime = runtime();
    let app = App::new();
    runtime.display_frame(&app, SIZE);
    let bracket = KeyPattern::command(Key::Char('['));
    let mut alt_left = KeyPattern::key(Key::Left);
    alt_left.option = true;
    assert!(!runtime.offer_key(bracket), "at the root the key is not the stack's");
    assert!(!runtime.offer_key(alt_left));

    app.path.set(vec![Route::Item(1), Route::Item(2)]);
    runtime.display_frame(&app, SIZE);
    assert!(runtime.offer_key(bracket));
    runtime.display_frame(&app, SIZE);
    assert_eq!(app.path.get(), [Route::Item(1)]);
    assert!(!runtime.offer_key(KeyPattern::key(Key::Escape)), "Escape leaves no page");
    assert_eq!(app.path.get(), [Route::Item(1)]);
    assert!(runtime.offer_key(alt_left));
    runtime.display_frame(&app, SIZE);
    assert!(app.path.get().is_empty());
}

#[test]
fn a_field_keeps_its_own_alt_left() {
    #[derive(Clone, Copy)]
    struct Editor {
        path: State<Vec<Route>>,
        note: State<String>,
    }
    impl Component for Editor {
        fn body(self) -> impl View {
            let note = self.note;
            navigation_stack(self.path, text("Root"))
                .destination(move |_| text_field("Note", note.binding()).id("note"))
        }
    }
    let runtime = runtime();
    let editor = Editor { path: State::new(vec![Route::About]), note: State::new("two words".into()) };
    runtime.display_frame(&editor, SIZE);
    assert!(runtime.focus_named("note"));
    let mut alt_left = KeyPattern::key(Key::Left);
    alt_left.option = true;
    assert!(runtime.offer_key(alt_left), "the field takes it: one word to the left");
    runtime.display_frame(&editor, SIZE);
    assert_eq!(editor.path.get(), [Route::About], "and the page stays");
}

#[test]
fn the_keyboard_goes_back_to_the_link_when_its_page_leaves_holding_it() {
    let runtime = runtime();
    let app = App::new();
    runtime.display_frame(&app, SIZE);
    for _ in 0..3 {
        runtime.offer_key(KeyPattern::key(Key::Tab));
    }
    assert_eq!(keyboard(&runtime, &app).as_deref(), Some("Open item 1"));
    assert!(runtime.offer_key(KeyPattern::key(Key::Enter)));
    runtime.display_frame(&app, SIZE);
    // the reader walks into the page, then goes back
    runtime.offer_key(KeyPattern::key(Key::Tab));
    runtime.offer_key(KeyPattern::key(Key::Tab));
    assert_eq!(keyboard(&runtime, &app).as_deref(), Some("Tap item"));
    assert!(runtime.navigate_back());
    assert_eq!(keyboard(&runtime, &app).as_deref(), Some("Open item 1"));
}

#[test]
fn the_bar_shows_the_title_of_the_page_on_top_and_a_button_that_goes_back() {
    let runtime = runtime();
    let app = App::new();
    let root = heard(&runtime, &app);
    assert!(root.contains(&(Role::Text, "Home".into())), "{root:?}");
    assert!(!root.iter().any(|(_, label)| label == "Back"), "the root has no page to go back to");

    app.path.push(Route::Item(1));
    let pushed = heard(&runtime, &app);
    assert!(pushed.contains(&(Role::Button, "Back".into())), "{pushed:?}");
    assert!(pushed.contains(&(Role::Text, "Item 1, 0 taps".into())));
    assert!(!pushed.contains(&(Role::Text, "Home".into())), "one title: the page's on top");
    // the bar is over the page, from edge to edge
    let (back, title, first) = (named(&runtime, &app, "Back"), named(&runtime, &app, "Item 1, 0 taps"), named(&runtime, &app, "Tap item"));
    assert!(back.origin.x < title.origin.x, "the button, then the title");
    assert!(back.origin.y + back.size.height <= 44.0 && title.origin.y + title.size.height <= 44.0);
    assert!(first.origin.y >= 45.0, "the page starts under the bar and its line");

    // the title is the page's: it follows the page's state
    press(&runtime, &app, "Tap item");
    assert!(heard(&runtime, &app).contains(&(Role::Text, "Item 1, 1 taps".into())));

    press(&runtime, &app, "Back");
    assert!(app.path.get().is_empty());
    assert!(heard(&runtime, &app).contains(&(Role::Text, "Home".into())));
}

#[test]
fn the_back_button_is_named_in_the_readers_language_and_changes_sides_right_to_left() {
    let runtime = runtime();
    runtime.set_locale(Some(Locale::new("pt-BR")));
    let app = App::new();
    app.path.push(Route::About);
    assert!(heard(&runtime, &app).contains(&(Role::Button, "Voltar".into())));
    let left_to_right = named(&runtime, &app, "Voltar");
    assert!(left_to_right.origin.x < SIZE.width / 2.0);

    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let right_to_left = named(&runtime, &app, "Voltar");
    assert!(right_to_left.origin.x > SIZE.width / 2.0, "the leading edge is on the right");
}

thread_local! {
    static ROWS_RUNS: Cell<u32> = const { Cell::new(0) };
}

/// A page with items in its bar: one that is always there, at the
/// leading side, and one that reads the page's state and can leave.
#[derive(Clone, Copy)]
struct Rows {
    added: State<u32>,
    edited: State<u32>,
    locked: State<bool>,
    titled: bool,
}

impl Component for Rows {
    fn body(self) -> impl View {
        count(&ROWS_RUNS);
        let rows = self.added.get();
        let page = vstack!(
            text!("Rows {}", self.added),
            button(text("Lock"), move || self.locked.set(!self.locked.get())),
        )
        .toolbar((
            toolbar_item(button(text("Edit"), move || self.edited.add(1))).leading(),
            toolbar_item((!self.locked.get()).then(|| button(text(format!("Add {rows}")), move || self.added.add(1)))),
        ));
        if self.titled { Either::First(page.navigation_title("All rows")) } else { Either::Second(page) }
    }
}

#[derive(Clone, Copy)]
struct Table {
    path: State<Vec<Route>>,
    rows: Rows,
}

impl Table {
    fn new(titled: bool) -> Self {
        Self {
            path: State::new(Vec::new()),
            rows: Rows { added: State::new(0), edited: State::new(0), locked: State::new(false), titled },
        }
    }
}

impl Component for Table {
    fn body(self) -> impl View {
        let rows = self.rows;
        navigation_stack(self.path, rows).destination(move |_| rows)
    }
}

/// The elements of the frame, in the order assistive technology has them.
fn elements(runtime: &Runtime, view: &impl View) -> Vec<(Role, String, Rect)> {
    runtime.display_frame(view, SIZE);
    runtime.accessibility_tree().nodes().iter().map(|node| (node.role, node.label.to_string(), node.bounds)).collect()
}

fn element(runtime: &Runtime, view: &impl View, label: &str) -> Rect {
    elements(runtime, view)
        .into_iter()
        .find(|(_, name, _)| name == label)
        .unwrap_or_else(|| panic!("nothing is named {label:?}"))
        .2
}

fn press_on(runtime: &Runtime, view: &impl View, label: &str) {
    let rect = element(runtime, view, label);
    let (x, y) = (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
    runtime.display_frame(view, SIZE);
}

#[test]
fn a_page_puts_its_items_in_the_bar_on_the_side_it_names() {
    let runtime = runtime();
    let table = Table::new(true);
    let (edit, title, add) = (element(&runtime, &table, "Edit"), element(&runtime, &table, "All rows"), element(&runtime, &table, "Add 0"));
    assert!(edit.origin.x < title.origin.x, "a leading item stands before the title");
    assert!(title.origin.x < add.origin.x, "and the other at the trailing edge");
    assert!(add.origin.x + add.size.width > SIZE.width - 20.0);
    for item in [edit, add] {
        assert!(item.origin.y + item.size.height <= 44.0, "inside the bar");
    }
    assert!(element(&runtime, &table, "Lock").origin.y >= 45.0, "the page is under the bar");

    // over a page that was pushed: the button that goes back, then the leading item
    table.path.push(Route::About);
    let (back, edit) = (element(&runtime, &table, "Back"), element(&runtime, &table, "Edit"));
    assert!(back.origin.x < edit.origin.x);
    // Tab: the bar from its leading edge, then the page
    let mut stops = Vec::new();
    for _ in 0..4 {
        assert!(runtime.offer_key(KeyPattern::key(Key::Tab)));
        runtime.display_frame(&table, SIZE);
        let tree = runtime.accessibility_tree();
        stops.push(tree.nodes().iter().find(|node| node.focused).map(|node| node.label.to_string()).unwrap_or_default());
    }
    assert_eq!(stops, ["Back", "Edit", "Add 0", "Lock"]);
}

#[test]
fn the_items_are_the_pages_own_and_follow_its_state() {
    let runtime = runtime();
    let table = Table::new(true);
    runtime.display_frame(&table, SIZE);
    let made = runs(&ROWS_RUNS);

    press_on(&runtime, &table, "Edit");
    assert_eq!(table.rows.edited.get(), 1, "an item runs its action");
    press_on(&runtime, &table, "Add 0");
    assert_eq!(table.rows.added.get(), 1);
    // the page ran because its state moved, and said its items again:
    // the bar shows the new ones
    let labels: Vec<_> = elements(&runtime, &table).into_iter().map(|(_, label, _)| label).collect();
    assert!(labels.iter().any(|label| label == "Add 1") && !labels.iter().any(|label| label == "Add 0"), "{labels:?}");
    assert_eq!(runs(&ROWS_RUNS), made + 1, "one run of the page for one change of its state");

    // an item that the page stops making leaves the bar
    press_on(&runtime, &table, "Lock");
    let labels: Vec<_> = elements(&runtime, &table).into_iter().map(|(_, label, _)| label).collect();
    assert!(labels.iter().any(|label| label == "Edit") && !labels.iter().any(|label| label.starts_with("Add")), "{labels:?}");
}

#[test]
fn a_root_with_items_and_no_title_has_a_bar() {
    let runtime = runtime();
    let table = Table::new(false);
    let add = element(&runtime, &table, "Add 0");
    assert!(add.origin.y + add.size.height <= 44.0);
    assert!(element(&runtime, &table, "Lock").origin.y >= 45.0);
}

/// A page that no route names: a link makes it.
#[derive(Clone, Copy)]
struct Notes;

impl Component for Notes {
    fn body(self) -> impl View {
        let taps = State::new(0u32);
        vstack!(
            text!("Notes taps {}", taps),
            button(text("Tap notes"), move || taps.add(1)),
            nav_link_value("Open item 9", Route::Item(9)),
        )
        .navigation_title("Notes")
    }
}

#[derive(Clone, Copy)]
struct Shop {
    path: State<Vec<Route>>,
}

impl Component for Shop {
    fn body(self) -> impl View {
        let root = vstack!(nav_link("About the shop", || Notes), nav_link_value("Open item 1", Route::Item(1)))
            .navigation_title("Shop");
        navigation_stack(self.path, root).destination(|route| match route {
            Route::Item(number) => Either::First(
                vstack!(text!("Item {}", number), nav_link(text("Notes on it"), || Notes)).navigation_title("An item"),
            ),
            Route::About => Either::Second(text("About this app")),
        })
    }
}

fn titles(runtime: &Runtime, view: &impl View) -> Vec<String> {
    elements(runtime, view).into_iter().filter(|(role, _, _)| *role == Role::Text).map(|(_, label, _)| label).collect()
}

#[test]
fn a_link_opens_a_page_that_has_no_route() {
    let runtime = runtime();
    let shop = Shop { path: State::new(Vec::new()) };
    // the label is words: a button named by them
    assert!(elements(&runtime, &shop).iter().any(|(role, label, _)| *role == Role::Button && label == "About the shop"));
    press_on(&runtime, &shop, "About the shop");
    assert!(titles(&runtime, &shop).contains(&"Notes taps 0".to_string()));
    assert!(shop.path.get().is_empty(), "the page is not in the path");

    // the button of the bar leaves it, and so do "back" and its keys
    press_on(&runtime, &shop, "Back");
    assert!(titles(&runtime, &shop).contains(&"Shop".to_string()));
    press_on(&runtime, &shop, "About the shop");
    assert!(runtime.navigate_back());
    assert!(titles(&runtime, &shop).contains(&"Shop".to_string()));
    press_on(&runtime, &shop, "About the shop");
    assert!(runtime.offer_key(KeyPattern::command(Key::Char('['))));
    assert!(titles(&runtime, &shop).contains(&"Shop".to_string()));
    assert!(!runtime.navigate_back(), "and then the root has nothing under it");
    // the same link again makes a new page
    press_on(&runtime, &shop, "About the shop");
    press_on(&runtime, &shop, "Tap notes");
    press_on(&runtime, &shop, "Back");
    press_on(&runtime, &shop, "About the shop");
    assert!(titles(&runtime, &shop).contains(&"Notes taps 0".to_string()));
}

#[test]
fn a_route_goes_over_a_page_with_no_route_and_back_returns_to_it() {
    let runtime = runtime();
    let shop = Shop { path: State::new(vec![Route::Item(1)]) };
    press_on(&runtime, &shop, "Notes on it");
    press_on(&runtime, &shop, "Tap notes");
    press_on(&runtime, &shop, "Tap notes");
    assert!(titles(&runtime, &shop).contains(&"Notes taps 2".to_string()));

    // a route pushed from it stands over it
    press_on(&runtime, &shop, "Open item 9");
    assert_eq!(shop.path.get(), [Route::Item(1), Route::Item(9)]);
    assert!(titles(&runtime, &shop).contains(&"Item 9".to_string()));
    // back: the page with no route again, as it was left
    assert!(runtime.navigate_back());
    assert_eq!(shop.path.get(), [Route::Item(1)]);
    assert!(titles(&runtime, &shop).contains(&"Notes taps 2".to_string()));
    assert!(runtime.navigate_back());
    assert!(titles(&runtime, &shop).contains(&"Item 1".to_string()));
    assert_eq!(shop.path.get(), [Route::Item(1)], "leaving it wrote nothing to the path");
}

#[test]
fn a_write_that_changes_a_route_under_it_takes_a_page_with_no_route_away() {
    let runtime = runtime();
    let shop = Shop { path: State::new(vec![Route::Item(1)]) };
    press_on(&runtime, &shop, "Notes on it");
    assert!(titles(&runtime, &shop).contains(&"Notes taps 0".to_string()));
    // the app's own pop does not see the page: it takes the route under it
    assert!(shop.path.pop());
    let shown = titles(&runtime, &shop);
    assert!(shown.contains(&"Shop".to_string()) && !shown.iter().any(|title| title.starts_with("Notes taps")), "{shown:?}");

    // over the root there is no route under it: a route that comes and goes leaves it there
    press_on(&runtime, &shop, "About the shop");
    shop.path.push(Route::Item(4));
    assert!(titles(&runtime, &shop).contains(&"Item 4".to_string()));
    shop.path.pop_to_root();
    assert!(titles(&runtime, &shop).contains(&"Notes taps 0".to_string()));
}

/// A root that can be searched, and a page over it that cannot.
#[derive(Clone, Copy)]
struct Catalog {
    path: State<Vec<Route>>,
    query: State<String>,
}

impl Component for Catalog {
    fn body(self) -> impl View {
        let query = self.query.get().to_lowercase();
        let found: Vec<_> =
            ["Alder", "Ash", "Birch"].iter().filter(|name| name.to_lowercase().contains(&query)).map(|name| text(*name)).collect();
        let root = vstack!(vstack(found), nav_link_value("Open item 1", Route::Item(1)))
            .navigation_title("Trees")
            .searchable(self.query.binding());
        navigation_stack(self.path, root).destination(|_| button(text("Plain"), || {}).navigation_title("Other"))
    }
}

fn find_key() -> KeyPattern {
    KeyPattern::command(Key::Char('f'))
}

#[test]
fn a_page_that_can_be_searched_has_a_field_in_the_bar() {
    let runtime = runtime();
    let catalog = Catalog { path: State::new(Vec::new()), query: State::new(String::new()) };
    let all = elements(&runtime, &catalog);
    let field = all.iter().find(|(role, _, _)| *role == Role::TextField).expect("the bar has a field");
    assert_eq!(field.1, "Search", "named by its prompt");
    let (title, first) = (element(&runtime, &catalog, "Trees"), element(&runtime, &catalog, "Alder"));
    assert!(field.2.origin.y >= title.origin.y + title.size.height, "under the title");
    assert!(field.2.origin.y + field.2.size.height < first.origin.y, "and over the page");
    assert!(field.2.size.width > SIZE.width / 2.0, "as wide as the bar lets it be");
}

#[test]
fn find_takes_the_keyboard_to_the_field_and_the_page_filters_by_the_same_binding() {
    let runtime = runtime();
    let catalog = Catalog { path: State::new(Vec::new()), query: State::new(String::new()) };
    runtime.display_frame(&catalog, SIZE);
    assert!(!runtime.focus_takes_text());
    assert!(runtime.offer_key(find_key()), "the stack takes the key of find");
    runtime.display_frame(&catalog, SIZE);
    assert!(runtime.focus_takes_text(), "and the keyboard is in the field");
    assert!(runtime.key(EditCommand::Insert("bi".into())).applied);
    assert_eq!(catalog.query.get(), "bi");
    let shown = titles(&runtime, &catalog);
    assert!(shown.contains(&"Birch".to_string()) && !shown.contains(&"Alder".to_string()), "{shown:?}");

    // find again selects the query: the next keys replace it
    assert!(runtime.offer_key(find_key()));
    runtime.display_frame(&catalog, SIZE);
    assert!(runtime.key(EditCommand::Insert("a".into())).applied);
    assert_eq!(catalog.query.get(), "a");
    let shown = titles(&runtime, &catalog);
    assert!(shown.contains(&"Alder".to_string()) && shown.contains(&"Ash".to_string()) && !shown.contains(&"Birch".to_string()));
}

#[test]
fn the_field_is_the_page_on_tops_and_a_page_with_none_has_none() {
    let runtime = runtime();
    let catalog = Catalog { path: State::new(Vec::new()), query: State::new("as".into()) };
    runtime.display_frame(&catalog, SIZE);
    catalog.path.push(Route::Item(1));
    let over = elements(&runtime, &catalog);
    assert!(!over.iter().any(|(role, _, _)| *role == Role::TextField), "the page on top said no search: {over:?}");
    assert!(!runtime.offer_key(find_key()), "and the key of find is not the stack's");

    catalog.path.pop();
    let back = elements(&runtime, &catalog);
    assert!(back.iter().any(|(role, label, _)| *role == Role::TextField && label == "Search"));
    assert_eq!(catalog.query.get(), "as", "the query was the app's all the time");
}

#[test]
fn a_root_with_no_title_has_no_bar() {
    #[derive(Clone, Copy)]
    struct Bare {
        path: State<Vec<Route>>,
    }
    impl Component for Bare {
        fn body(self) -> impl View {
            navigation_stack(self.path, text("Only this").frame_max(f64::INFINITY, f64::INFINITY, Alignment::TopLeading))
                .destination(|_| text("A page"))
        }
    }
    let runtime = runtime();
    let bare = Bare { path: State::new(Vec::new()) };
    runtime.display_frame(&bare, SIZE);
    let tree = runtime.accessibility_tree();
    let only = tree.nodes().iter().find(|node| &*node.label == "Only this").expect("the root");
    assert_eq!(only.bounds.origin.y, 0.0, "nothing was put over the root");
    drop(tree);

    // a page over it has the button, with or without a title
    bare.path.push(Route::About);
    runtime.display_frame(&bare, SIZE);
    let tree = runtime.accessibility_tree();
    assert!(tree.nodes().iter().any(|node| node.role == Role::Button && &*node.label == "Back"));
}

#[test]
fn a_link_outside_every_stack_is_disabled() {
    let runtime = runtime();
    let lonely = nav_link_value(text("Nowhere"), Route::About).id("lonely");
    runtime.display_frame(&lonely, SIZE);
    let tree = runtime.accessibility_tree();
    let link = tree.nodes().iter().find(|node| &*node.label == "Nowhere").expect("the link is there");
    assert_eq!(link.role, Role::Button);
    assert!(!link.enabled);
}

#[test]
fn it_lowers_the_page_on_top_to_the_web_and_a_tool_sees_only_the_apps_lines() {
    let app = App::new();
    app.path.push(Route::About);
    let page = bunny_ui::ssr::render(&app, SIZE);
    assert!(page.html.contains("About this app"));
    assert!(!page.html.contains("Tap home"), "a covered page is not in the document");

    let runtime = Runtime::scene("navigation_sites");
    runtime.record_sites(true);
    let app = App::new();
    runtime.display_frame(&app, SIZE);
    let sites = runtime.sites();
    runtime.record_sites(false);
    assert!(sites.iter().any(|site| site.kind == NodeKind::NavigationStack));
    assert!(sites.iter().any(|site| site.kind == NodeKind::NavLinkValue));
    for site in &sites {
        if let Site::Caller(location) = site.site {
            // a path reads with either separator, by the platform that built it
            let file = location.file().replace('\\', "/");
            assert!(file.ends_with("tests/navigation.rs"), "{:?} at {location}", site.kind);
        }
    }
}
