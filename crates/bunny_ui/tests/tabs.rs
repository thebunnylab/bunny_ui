//! `tab_view(binding, tabs)` and `tabs(binding, choices)`.

use bunny_ui::accessibility::{Action, Role};
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 240.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pane {
    General,
    Advanced,
    About,
}

/// A page with state of its own: the count lives in the page.
#[derive(Clone, Copy)]
struct Counter;

impl Component for Counter {
    fn body(self) -> impl View {
        let count = State::new(0u32);
        button(text!("Count {}", count), move || count.add(1)).id("count")
    }
}

#[derive(Clone, Copy)]
struct Settings {
    pane: State<Pane>,
    name: State<String>,
}

impl Settings {
    fn new() -> Self {
        Self {
            pane: State::new(Pane::General),
            name: State::new(String::new()),
        }
    }
}

impl Component for Settings {
    fn body(self) -> impl View {
        tab_view(
            self.pane.binding(),
            (
                tab(Pane::General, "General", text_field("Name", self.name.binding()).id("name")),
                tab(Pane::Advanced, "Advanced", Counter),
                tab(Pane::About, "About", text("Version 1")),
            ),
        )
        .id("settings")
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

fn shown(runtime: &Runtime, settings: &Settings) -> Vec<String> {
    lines(runtime.display_frame(settings, SIZE).as_slice())
}

/// The three tabs' targets, left to right: the targets on the top line.
fn tab_targets(runtime: &Runtime) -> Vec<(String, Rect)> {
    let mut found: Vec<_> = runtime
        .hits_snapshot()
        .into_iter()
        .filter(|(path, rect)| path.contains("[settings]") && rect.origin.y == 0.0 && rect.size.height == 32.0)
        .collect();
    found.sort_by(|a, b| a.1.origin.x.total_cmp(&b.1.origin.x));
    found
}

fn press(runtime: &Runtime, rect: Rect) {
    let (x, y) = (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
}

fn key(runtime: &Runtime, key: Key) -> bool {
    runtime.key_stroke(KeyPattern::key(key)).handled
}

#[test]
fn only_the_page_of_the_selected_tab_is_made() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    let general = shown(&runtime, &settings);
    assert!(general.iter().any(|line| line == "General") && general.iter().any(|line| line == "About"), "{general:?}");
    assert!(!general.iter().any(|line| line.starts_with("Count") || line == "Version 1"), "{general:?}");
    assert!(runtime.focus_named("name"), "the field of the first page is on screen");

    settings.pane.set(Pane::About);
    let about = shown(&runtime, &settings);
    assert!(about.iter().any(|line| line == "Version 1"), "{about:?}");
    assert!(!runtime.focus_named("name"), "and the field left with its page");
}

#[test]
fn a_press_on_a_tab_shows_its_page() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    runtime.display_frame(&settings, SIZE);
    let tabs = tab_targets(&runtime);
    assert_eq!(tabs.len(), 3);
    assert_eq!(tabs[0].1.size.width, SIZE.width / 3.0, "the tabs share the width");
    press(&runtime, tabs[1].1);
    assert_eq!(settings.pane.get(), Pane::Advanced);
    assert!(shown(&runtime, &settings).iter().any(|line| line == "Count 0"));
}

#[test]
fn the_state_of_a_page_leaves_with_the_page() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    settings.pane.set(Pane::Advanced);
    runtime.display_frame(&settings, SIZE);
    assert!(runtime.focus_named("count"));
    assert!(key(&runtime, Key::Enter));
    assert!(key(&runtime, Key::Enter));
    assert!(shown(&runtime, &settings).iter().any(|line| line == "Count 2"));

    settings.pane.set(Pane::General);
    runtime.display_frame(&settings, SIZE);
    settings.pane.set(Pane::Advanced);
    assert!(
        shown(&runtime, &settings).iter().any(|line| line == "Count 0"),
        "the page was unmounted: its state went with it"
    );
}

#[test]
fn the_bar_is_one_tab_stop_and_the_arrows_change_page() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    runtime.display_frame(&settings, SIZE);
    assert!(key(&runtime, Key::Tab));
    let tabs = tab_targets(&runtime);
    assert_eq!(runtime.focused().as_deref(), Some(tabs[0].0.as_str()), "Tab lands on the tab that is shown");

    assert!(key(&runtime, Key::Tab));
    assert!(runtime.focused().is_some_and(|path| path.contains("[name]")), "then goes on into the page");

    let mut back = KeyPattern::key(Key::Tab);
    back.shift = true;
    assert!(runtime.key_stroke(back).handled);
    assert!(key(&runtime, Key::Right));
    assert_eq!(settings.pane.get(), Pane::Advanced, "an arrow shows the next page");
    runtime.display_frame(&settings, SIZE);
    assert!(key(&runtime, Key::End));
    assert_eq!(settings.pane.get(), Pane::About);
    runtime.display_frame(&settings, SIZE);
    assert!(key(&runtime, Key::Right));
    assert_eq!(settings.pane.get(), Pane::General, "and goes round");
}

#[test]
fn assistive_technology_hears_tabs_and_which_one_is_shown() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let settings = Settings::new();
    runtime.display_frame(&settings, SIZE);
    let tree = runtime.accessibility_tree();
    let tabs: Vec<_> = tree.nodes().iter().filter(|node| node.role == Role::Tab).collect();
    let heard: Vec<_> = tabs.iter().map(|node| (&*node.label, node.checked)).collect();
    assert_eq!(heard, [("General", Some(true)), ("Advanced", Some(false)), ("About", Some(false))]);
    assert!(tree.nodes().iter().any(|node| node.role == Role::TextField), "the page is in the tree too");

    assert_eq!(runtime.accessibility_action(tabs[2].id, Action::Activate), Ok(()));
    assert_eq!(settings.pane.get(), Pane::About);
}

#[derive(Clone, Copy)]
struct BarOnly {
    pane: State<Pane>,
}

impl Component for BarOnly {
    fn body(self) -> impl View {
        tabs(self.pane.binding(), [(Pane::General, "General"), (Pane::Advanced, "Advanced")]).id("bar")
    }
}

#[test]
fn the_line_stands_under_the_tab_that_is_shown() {
    let runtime = Runtime::new();
    let bar = BarOnly { pane: State::new(Pane::Advanced) };
    runtime.display_frame(&bar, SIZE);
    for _ in 0..240 {
        runtime.tick(1.0 / 60.0);
    }
    let frame = runtime.display_frame(&bar, SIZE).as_slice().to_vec();
    let accent = theme::current().accent;
    let line = frame
        .iter()
        .find_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } if *color == accent => Some(*rect),
            _ => None,
        })
        .expect("the line");
    assert_eq!(line.size, Size { width: SIZE.width / 2.0, height: 2.0 });
    assert_eq!(line.origin.x, SIZE.width / 2.0, "under the second tab");
    assert_eq!(line.origin.y, 30.0, "at the bottom of the bar");
}

#[test]
fn a_tool_sees_the_tab_view_and_the_page_the_app_wrote() {
    let page = bunny_ui::ssr::render(&Settings::new(), SIZE);
    assert!(page.html.contains("Advanced") && !page.html.contains("<canvas"));

    let runtime = Runtime::scene("tab_sites");
    runtime.record_sites(true);
    runtime.display_frame(&Settings::new(), SIZE);
    let sites = runtime.sites();
    runtime.record_sites(false);
    assert!(sites.iter().any(|site| site.kind == NodeKind::TabView));
    assert!(sites.iter().any(|site| site.kind == NodeKind::Field), "the page's own field is sited");
    for site in &sites {
        if let Site::Caller(location) = site.site {
            // a path reads with either separator, by the platform that built it
            let file = location.file().replace('\\', "/");
            assert!(file.ends_with("tests/tabs.rs"), "{:?} at {location}", site.kind);
        }
    }
}
