//! `search_field(binding)`: a text field for a query.

use bunny_ui::accessibility::Role;
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 160.0,
};

#[derive(Clone, Copy)]
struct Finder {
    query: State<String>,
    other: State<String>,
    submitted: State<u32>,
    named: State<bool>,
}

impl Finder {
    fn new() -> Self {
        Self {
            query: State::new(String::new()),
            other: State::new(String::new()),
            submitted: State::new(0),
            named: State::new(false),
        }
    }
}

impl Component for Finder {
    fn body(self) -> impl View {
        let field = search_field(self.query.binding()).on_submit(move || self.submitted.add(1));
        let field = if self.named.get() { field.prompt("Find a country") } else { field };
        vstack!(field.id("search"), text_field("Other", self.other.binding()).id("other"))
    }
}

fn lines(commands: &[DrawCommand]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, .. } => Some(content[range.0..range.1].to_string()),
            _ => None,
        })
        .collect()
}

fn shown(runtime: &Runtime, finder: &Finder) -> Vec<String> {
    lines(runtime.display_frame(finder, SIZE).as_slice())
}

/// The border the box wears: the stroke of the widest rounded box.
fn border(runtime: &Runtime, finder: &Finder) -> Color {
    let frame = runtime.display_frame(finder, SIZE);
    frame
        .as_slice()
        .iter()
        .filter_map(|command| match command {
            DrawCommand::StrokeRect { rect, color, .. } if rect.origin.y == 0.0 => Some((rect.size.width, *color)),
            _ => None,
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("the box has a border")
        .1
}

/// The button that empties the field, while there is one. The runtime
/// must have accessibility on: the tree is where a button differs from
/// the field it sits in.
fn clear_button(runtime: &Runtime) -> Option<Rect> {
    let tree = runtime.accessibility_tree();
    tree.nodes().iter().find(|node| node.role == Role::Button).map(|node| node.bounds)
}

fn key(runtime: &Runtime, key: Key) -> bool {
    runtime.key_stroke(KeyPattern::key(key)).handled
}

#[test]
fn it_shows_the_prompt_while_it_is_empty() {
    let runtime = Runtime::new();
    let finder = Finder::new();
    assert!(shown(&runtime, &finder).iter().any(|line| line == "Search"));

    finder.named.set(true);
    let named = shown(&runtime, &finder);
    assert!(named.iter().any(|line| line == "Find a country") && !named.iter().any(|line| line == "Search"));

    finder.query.set("ice".into());
    let filled = shown(&runtime, &finder);
    assert!(filled.iter().any(|line| line == "ice") && !filled.iter().any(|line| line == "Find a country"));
}

#[test]
fn the_prompt_is_in_the_readers_language() {
    let runtime = Runtime::new();
    runtime.set_locale(Some(Locale::new("pt-BR")));
    let finder = Finder::new();
    assert!(shown(&runtime, &finder).iter().any(|line| line == "Buscar"));
    runtime.set_locale(Some(Locale::new("ja")));
    assert!(shown(&runtime, &finder).iter().any(|line| line == "検索"));
}

#[test]
fn the_button_exists_while_there_is_text_and_empties_the_field() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let finder = Finder::new();
    runtime.display_frame(&finder, SIZE);
    assert_eq!(clear_button(&runtime), None, "nothing to empty");

    assert!(key(&runtime, Key::Tab), "the field is the first Tab stop");
    assert!(runtime.key(EditCommand::Insert("ice".into())).applied);
    assert_eq!(finder.query.get(), "ice");
    runtime.display_frame(&finder, SIZE);
    let field = runtime.focused().expect("the field has the keyboard");
    let button = clear_button(&runtime).expect("the button is there now");
    assert!(button.origin.x > SIZE.width / 2.0, "at the trailing edge");

    let (x, y) = (button.origin.x + button.size.width / 2.0, button.origin.y + button.size.height / 2.0);
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
    assert_eq!(finder.query.get(), "", "a press empties the field");
    runtime.display_frame(&finder, SIZE);
    assert_eq!(runtime.focused(), Some(field), "and the keyboard stays in the field");
    assert_eq!(clear_button(&runtime), None);
}

#[test]
fn enter_submits_and_tab_reaches_the_field_then_the_button() {
    let runtime = Runtime::new();
    let finder = Finder::new();
    finder.query.set("ice".into());
    runtime.display_frame(&finder, SIZE);
    assert!(key(&runtime, Key::Tab));
    assert!(runtime.focus_takes_text(), "the field first");
    assert!(runtime.key(EditCommand::Newline).applied);
    assert_eq!(finder.submitted.get(), 1, "Enter runs the submit");

    assert!(key(&runtime, Key::Tab));
    assert!(!runtime.focus_takes_text(), "then the button");
    assert!(runtime.focused().is_some_and(|path| path.contains("[search]")));
    assert!(key(&runtime, Key::Tab));
    assert!(runtime.focused().is_some_and(|path| path.contains("[other]")));
}

#[test]
fn the_box_wears_the_focus_color_while_the_field_has_the_keyboard() {
    let runtime = Runtime::new();
    let finder = Finder::new();
    let theme = theme::current();
    assert_eq!(border(&runtime, &finder), theme.field_border);
    assert!(key(&runtime, Key::Tab));
    assert_eq!(border(&runtime, &finder), theme.focus);
    assert!(key(&runtime, Key::Tab));
    assert_eq!(border(&runtime, &finder), theme.field_border, "the keyboard moved on");
}

#[test]
fn assistive_technology_hears_a_field_named_by_its_prompt_and_a_clear_button() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let finder = Finder::new();
    runtime.display_frame(&finder, SIZE);
    let tree = runtime.accessibility_tree();
    let first: Vec<_> = tree.nodes().iter().map(|node| (node.role, &*node.label)).collect();
    assert_eq!(first, [(Role::TextField, "Search"), (Role::TextField, "Other")]);

    finder.query.set("ice".into());
    runtime.display_frame(&finder, SIZE);
    let tree = runtime.accessibility_tree();
    let filled: Vec<_> = tree.nodes().iter().map(|node| (node.role, &*node.label, node.value.as_deref())).collect();
    assert_eq!(
        filled,
        [
            (Role::TextField, "Search", Some("ice")),
            (Role::Button, "Clear", None),
            (Role::TextField, "Other", Some("")),
        ]
    );
}

#[test]
fn it_lowers_to_a_real_input_and_a_tool_sees_only_the_apps_line() {
    let page = bunny_ui::ssr::render(&Finder::new(), SIZE);
    assert!(page.html.contains("<input"), "a text field is a real input on the web");
    assert!(!page.html.contains("<canvas"));

    let runtime = Runtime::scene("search_sites");
    runtime.record_sites(true);
    let finder = Finder::new();
    finder.query.set("ice".into());
    runtime.display_frame(&finder, SIZE);
    let sites = runtime.sites();
    runtime.record_sites(false);
    assert!(sites.iter().any(|site| site.kind == NodeKind::SearchField));
    for site in &sites {
        if let Site::Caller(location) = site.site {
            // a path reads with either separator, by the platform that built it
            let file = location.file().replace('\\', "/");
            assert!(file.ends_with("tests/search_field.rs"), "{:?} at {location}", site.kind);
        }
    }
}
