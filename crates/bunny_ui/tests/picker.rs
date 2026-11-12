//! `picker(binding, choices)`: a button that shows one value and opens a
//! list of the others.

use bunny_ui::accessibility::{Action, Node, Role};
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 240.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Look {
    System,
    Light,
    Dark,
    /// A value the picker is not given.
    Sepia,
}

#[derive(Clone, Copy)]
struct Prefs {
    look: State<Look>,
    name: State<String>,
    locked: State<bool>,
}

impl Prefs {
    fn new() -> Self {
        Self {
            look: State::new(Look::System),
            name: State::new(String::new()),
            locked: State::new(false),
        }
    }
}

impl Component for Prefs {
    fn body(self) -> impl View {
        vstack!(
            text_field("Name", self.name.binding()).id("name"),
            picker(
                self.look.binding(),
                [(Look::System, "System"), (Look::Light, "Light"), (Look::Dark, "Dark")],
            )
            .disabled(self.locked.get())
            .id("look"),
        )
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

fn shown(runtime: &Runtime, prefs: &Prefs) -> Vec<String> {
    lines(runtime.display_frame(prefs, SIZE).as_slice())
}

/// The picker's targets, top to bottom: the button, then the rows of an
/// open list.
fn targets(runtime: &Runtime) -> Vec<(String, Rect)> {
    let mut found: Vec<_> = runtime
        .hits_snapshot()
        .into_iter()
        .filter(|(path, _)| path.contains("[look]"))
        .collect();
    found.sort_by(|a, b| a.1.origin.y.total_cmp(&b.1.origin.y));
    found
}

fn press(runtime: &Runtime, rect: Rect) {
    let (x, y) = (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
}

/// A key the way a shell sends it: the bindings first — Escape on an open
/// list is one — and the control under the keyboard after.
fn key(runtime: &Runtime, key: Key) -> bool {
    use bunny_ui::action::KeyMatch;
    let pattern = KeyPattern::key(key);
    match runtime.chord(pattern) {
        KeyMatch::Action(action) => runtime.dispatch_action(action),
        KeyMatch::Pending | KeyMatch::Disabled => true,
        KeyMatch::None => runtime.key_stroke(pattern).handled,
    }
}

fn is_open(runtime: &Runtime) -> bool {
    targets(runtime).len() > 1
}

/// Which row of the open list holds the keyboard, from the top.
fn focused_row(runtime: &Runtime) -> Option<usize> {
    let held = runtime.focused()?;
    targets(runtime).iter().skip(1).position(|(path, _)| *path == held)
}

fn on_the_button(runtime: &Runtime) -> bool {
    let held = runtime.focused();
    targets(runtime).first().is_some_and(|(path, _)| Some(path) == held.as_ref())
}

#[test]
fn the_button_shows_the_words_of_the_current_choice() {
    let runtime = Runtime::new();
    let prefs = Prefs::new();
    let first = shown(&runtime, &prefs);
    assert!(first.iter().any(|line| line == "System"), "{first:?}");
    assert!(!first.iter().any(|line| line == "Dark"), "the list is closed: {first:?}");

    prefs.look.set(Look::Dark);
    let second = shown(&runtime, &prefs);
    assert!(second.iter().any(|line| line == "Dark") && !second.iter().any(|line| line == "System"), "{second:?}");
}

#[test]
fn a_press_opens_the_list_under_the_button_and_a_row_chooses_and_closes() {
    let runtime = Runtime::new();
    let prefs = Prefs::new();
    runtime.display_frame(&prefs, SIZE);
    assert!(!is_open(&runtime));
    let button = targets(&runtime)[0].1;

    press(&runtime, button);
    runtime.display_frame(&prefs, SIZE);
    let found = targets(&runtime);
    assert_eq!(found.len(), 4, "the button and three rows: {found:?}");
    for (_, row) in &found[1..] {
        assert!(row.origin.y >= button.origin.y + button.size.height, "a row is under the button");
        assert_eq!(row.size.height, 26.0);
        assert_eq!(row.size.width, found[1].1.size.width, "the rows share one width");
    }
    assert!(
        found[1].1.size.width < SIZE.width / 2.0,
        "the list is as wide as its words need, not as wide as the window: {}",
        found[1].1.size.width
    );

    press(&runtime, found[3].1);
    assert_eq!(prefs.look.get(), Look::Dark);
    runtime.display_frame(&prefs, SIZE);
    assert!(!is_open(&runtime), "choosing closes the list");
}

#[test]
fn the_keyboard_opens_moves_chooses_and_comes_back_to_the_button() {
    let runtime = Runtime::new();
    let prefs = Prefs::new();
    prefs.look.set(Look::Light);
    runtime.display_frame(&prefs, SIZE);
    assert!(runtime.focus_named("name"));
    assert!(key(&runtime, Key::Tab));
    assert!(on_the_button(&runtime), "the button is one Tab stop");

    assert!(key(&runtime, Key::Down), "a vertical arrow opens the list");
    runtime.display_frame(&prefs, SIZE);
    assert!(is_open(&runtime));
    assert_eq!(focused_row(&runtime), Some(1), "the list opens on the row of the current choice");

    assert!(key(&runtime, Key::Down));
    assert_eq!(focused_row(&runtime), Some(2));
    assert_eq!(prefs.look.get(), Look::Light, "moving does not choose");
    assert!(key(&runtime, Key::Down));
    assert_eq!(focused_row(&runtime), Some(0), "the rows go round");
    assert!(key(&runtime, Key::End));
    assert_eq!(focused_row(&runtime), Some(2));
    assert!(key(&runtime, Key::Tab));
    assert!(focused_row(&runtime).is_some(), "Tab stays inside the open list");

    assert!(key(&runtime, Key::End));
    assert!(key(&runtime, Key::Enter));
    assert_eq!(prefs.look.get(), Look::Dark, "Enter chooses the row");
    runtime.display_frame(&prefs, SIZE);
    assert!(!is_open(&runtime));
    assert!(on_the_button(&runtime), "and the keyboard is on the button again");
}

#[test]
fn escape_closes_the_list_and_leaves_the_value() {
    let runtime = Runtime::new();
    let prefs = Prefs::new();
    runtime.display_frame(&prefs, SIZE);
    assert!(runtime.focus_named("name"));
    assert!(key(&runtime, Key::Tab));
    assert!(key(&runtime, Key::Char(' ')), "Space opens it too");
    runtime.display_frame(&prefs, SIZE);
    assert!(is_open(&runtime));
    assert!(key(&runtime, Key::Down));

    assert!(key(&runtime, Key::Escape));
    runtime.display_frame(&prefs, SIZE);
    assert!(!is_open(&runtime));
    assert_eq!(prefs.look.get(), Look::System);
    assert!(on_the_button(&runtime));
}

fn pop_up(nodes: &[Node]) -> &Node {
    nodes.iter().find(|node| node.role == Role::PopUpButton).expect("the button")
}

#[test]
fn assistive_technology_hears_the_choice_and_whether_the_list_is_open() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let prefs = Prefs::new();
    runtime.display_frame(&prefs, SIZE);
    let tree = runtime.accessibility_tree();
    let button = pop_up(tree.nodes());
    assert_eq!((button.value.as_deref(), button.expanded), (Some("System"), Some(false)));
    assert!(!tree.nodes().iter().any(|node| node.role == Role::RadioButton), "no rows while closed");

    assert_eq!(runtime.accessibility_action(button.id, Action::Activate), Ok(()));
    runtime.display_frame(&prefs, SIZE);
    let tree = runtime.accessibility_tree();
    assert_eq!(pop_up(tree.nodes()).expanded, Some(true));
    let rows: Vec<_> = tree.nodes().iter().filter(|node| node.role == Role::RadioButton).collect();
    let heard: Vec<_> = rows.iter().map(|node| (&*node.label, node.checked)).collect();
    assert_eq!(heard, [("System", Some(true)), ("Light", Some(false)), ("Dark", Some(false))]);
    assert!(rows.iter().all(|node| node.surface.is_some()), "the rows are on the list's own surface");

    assert_eq!(runtime.accessibility_action(rows[1].id, Action::Activate), Ok(()));
    assert_eq!(prefs.look.get(), Look::Light);
    runtime.display_frame(&prefs, SIZE);
    let tree = runtime.accessibility_tree();
    let button = pop_up(tree.nodes());
    assert_eq!((button.value.as_deref(), button.expanded), (Some("Light"), Some(false)));
}

#[test]
fn a_value_that_is_no_choice_shows_no_words_and_no_mark() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let prefs = Prefs::new();
    prefs.look.set(Look::Sepia);
    let closed = shown(&runtime, &prefs);
    assert!(!closed.iter().any(|line| ["System", "Light", "Dark"].contains(&line.as_str())), "{closed:?}");

    press(&runtime, targets(&runtime)[0].1);
    runtime.display_frame(&prefs, SIZE);
    let tree = runtime.accessibility_tree();
    assert!(tree.nodes().iter().filter(|node| node.role == Role::RadioButton).all(|node| node.checked == Some(false)));
    assert_eq!(prefs.look.get(), Look::Sepia, "the binding is the app's");
}

#[test]
fn a_disabled_picker_shows_its_choice_and_cannot_open() {
    let runtime = Runtime::new();
    let prefs = Prefs::new();
    prefs.locked.set(true);
    assert!(shown(&runtime, &prefs).iter().any(|line| line == "System"));
    assert!(targets(&runtime).is_empty(), "the button is no target");
    assert!(runtime.focus_named("name"));
    assert!(!key(&runtime, Key::Tab) || !on_the_button(&runtime), "and no Tab stop");
    assert!(!is_open(&runtime));
}

#[test]
fn a_tool_sees_only_the_apps_line_also_after_the_list_opened() {
    let page = bunny_ui::ssr::render(&Prefs::new(), SIZE);
    assert!(page.html.contains("System") && !page.html.contains("<canvas"));

    let runtime = Runtime::scene("picker_sites");
    runtime.record_sites(true);
    let prefs = Prefs::new();
    runtime.display_frame(&prefs, SIZE);
    // the picker's own body runs again, alone, when the list opens
    press(&runtime, targets(&runtime)[0].1);
    runtime.display_frame(&prefs, SIZE);
    assert!(is_open(&runtime));
    let sites = runtime.sites();
    runtime.record_sites(false);
    assert!(sites.iter().any(|site| site.kind == NodeKind::Picker));
    for site in &sites {
        if let Site::Caller(location) = site.site {
            // a path reads with either separator, by the platform that built it
            let file = location.file().replace('\\', "/");
            assert!(file.ends_with("tests/picker.rs"), "{:?} at {location}", site.kind);
        }
    }
}
