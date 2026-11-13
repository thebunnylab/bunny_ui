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

    // Tab goes round the page on top and never into the root
    let mut stops = Vec::new();
    for _ in 0..4 {
        assert!(runtime.offer_key(KeyPattern::key(Key::Tab)));
        stops.push(keyboard(&runtime, &app).expect("a stop of the page on top"));
    }
    assert_eq!(stops, ["Tap item", "Open next", "More", "Tap item"]);
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
    assert_eq!(keyboard(&runtime, &app).as_deref(), Some("Tap item"));
    assert!(runtime.navigate_back());
    assert_eq!(keyboard(&runtime, &app).as_deref(), Some("Open item 1"));
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
    assert!(sites.iter().any(|site| site.kind == NodeKind::NavLink));
    for site in &sites {
        if let Site::Caller(location) = site.site {
            // a path reads with either separator, by the platform that built it
            let file = location.file().replace('\\', "/");
            assert!(file.ends_with("tests/navigation.rs"), "{:?} at {location}", site.kind);
        }
    }
}
