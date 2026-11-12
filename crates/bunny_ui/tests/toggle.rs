//! `toggle(label, binding)`: a labelled switch.

use bunny_ui::accessibility::{Action, ActionError, Role};
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 180.0,
};

#[derive(Clone, Copy)]
struct Settings {
    wifi: State<bool>,
    locked: State<bool>,
}

impl Settings {
    fn new() -> Self {
        Self {
            wifi: State::new(false),
            locked: State::new(false),
        }
    }
}

impl Component for Settings {
    fn body(self) -> impl View {
        toggle(text("Wi-Fi"), self.wifi.binding())
            .disabled(self.locked.get())
            .id("wifi")
    }
}

/// Lets the springs come to rest, then answers the frame.
fn settled(runtime: &Runtime, view: &Settings) -> Vec<DrawCommand> {
    runtime.display_frame(view, SIZE);
    for _ in 0..240 {
        runtime.tick(1.0 / 60.0);
    }
    runtime.display_frame(view, SIZE).as_slice().to_vec()
}

fn fills(commands: &[DrawCommand]) -> Vec<(Rect, Color)> {
    commands
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } => Some((*rect, *color)),
            _ => None,
        })
        .collect()
}

/// The track is the 36 × 20 fill, and the knob the 16 × 16 one.
fn track(commands: &[DrawCommand]) -> (Rect, Color) {
    fills(commands)
        .into_iter()
        .find(|(rect, _)| rect.size.width == 36.0 && rect.size.height == 20.0)
        .expect("the track")
}

fn knob(commands: &[DrawCommand]) -> Rect {
    fills(commands)
        .into_iter()
        .find(|(rect, color)| rect.size.width == 16.0 && rect.size.height == 16.0 && *color == Color::WHITE)
        .expect("the knob")
        .0
}

fn row(runtime: &Runtime) -> Rect {
    runtime
        .hits_snapshot()
        .into_iter()
        .find(|(path, _)| path.contains("[wifi]"))
        .expect("the row is a target")
        .1
}

#[test]
fn the_switch_stands_on_the_trailing_side_and_the_whole_row_is_the_target() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    let frame = settled(&runtime, &settings);

    let row = row(&runtime);
    assert_eq!(row.size.width, SIZE.width, "the row takes the width it is given");
    let (rail, _) = track(&frame);
    assert_eq!(rail.origin.x + rail.size.width, SIZE.width, "the switch ends at the trailing edge");
    assert_eq!(knob(&frame).origin.x, rail.origin.x + 2.0, "off: the knob is on the leading side");

    // a press on the label, far from the switch, flips it
    runtime.pointer_pressed(10.0, row.origin.y + row.size.height / 2.0);
    runtime.pointer_released(10.0, row.origin.y + row.size.height / 2.0);
    assert!(settings.wifi.get());
    let frame = settled(&runtime, &settings);
    let (rail, _) = track(&frame);
    assert_eq!(
        knob(&frame).origin.x,
        rail.origin.x + rail.size.width - 2.0 - 16.0,
        "on: the knob is on the trailing side"
    );
}

#[test]
fn the_knob_slides_to_its_side_and_does_not_jump() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    let off = knob(&settled(&runtime, &settings)).origin.x;

    settings.wifi.set(true);
    // the first frame after the flip: the knob has not arrived
    let first = knob(runtime.display_frame(&settings, SIZE).as_slice()).origin.x;
    assert!(runtime.tick(1.0 / 60.0).any(), "a spring is in flight");
    let moving = knob(runtime.display_frame(&settings, SIZE).as_slice()).origin.x;
    let on = knob(&settled(&runtime, &settings)).origin.x;
    assert_eq!(on, off + 16.0, "it ends on the other side");
    assert!(first < on, "the flip did not move it in one frame: {first} then {on}");
    assert!(off < moving && moving < on, "it is seen on the way: {off} < {moving} < {on}");

    // a reader who asked for less motion sees it arrive at once
    runtime.set_reduce_motion(true);
    settings.wifi.set(false);
    assert_eq!(knob(runtime.display_frame(&settings, SIZE).as_slice()).origin.x, off);
}

#[test]
fn the_track_wears_the_accent_when_on_and_the_control_color_when_off() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    let theme = theme::current();
    assert_eq!(track(&settled(&runtime, &settings)).1, theme.control);
    settings.wifi.set(true);
    assert_eq!(track(&settled(&runtime, &settings)).1, theme.accent);
}

#[test]
fn right_to_left_mirrors_the_row_and_the_knob() {
    let runtime = Runtime::new();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let settings = Settings::new();
    let frame = settled(&runtime, &settings);
    let (rail, _) = track(&frame);
    assert_eq!(rail.origin.x, 0.0, "the switch is on the trailing side, which is the left");
    assert_eq!(knob(&frame).origin.x, rail.size.width - 2.0 - 16.0, "off: the knob is on the leading side");
    settings.wifi.set(true);
    let frame = settled(&runtime, &settings);
    assert_eq!(knob(&frame).origin.x, 2.0, "on: the knob is on the trailing side");
}

#[test]
fn space_flips_it_and_enter_does_not() {
    let runtime = Runtime::new();
    let settings = Settings::new();
    runtime.display_frame(&settings, SIZE);
    assert!(runtime.key_stroke(KeyPattern::key(Key::Tab)).handled, "the toggle is a Tab stop");
    assert!(runtime.focused().is_some_and(|path| path.contains("[wifi]")));

    assert!(runtime.key_stroke(KeyPattern::key(Key::Char(' '))).handled);
    assert!(settings.wifi.get());
    runtime.display_frame(&settings, SIZE);
    assert!(
        !runtime.key_stroke(KeyPattern::key(Key::Enter)).handled,
        "Enter belongs to the form around a two-state control"
    );
    assert!(settings.wifi.get());
}

#[test]
fn a_disabled_toggle_shows_its_value_and_cannot_change() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let settings = Settings::new();
    settings.wifi.set(true);
    settings.locked.set(true);
    runtime.display_frame(&settings, SIZE);

    assert!(!runtime.key_stroke(KeyPattern::key(Key::Tab)).handled, "no Tab stop");
    runtime.pointer_pressed(10.0, 10.0);
    runtime.pointer_released(10.0, 10.0);
    assert!(settings.wifi.get(), "a press changes nothing");

    let tree = runtime.accessibility_tree();
    let node = tree.nodes().iter().find(|node| node.role == Role::Switch).expect("still in the tree");
    assert!(!node.enabled);
    assert_eq!(node.checked, Some(true));
    assert_eq!(runtime.accessibility_action(node.id, Action::Activate), Err(ActionError::Unsupported));
    assert!(settings.wifi.get());
}

#[test]
fn assistive_technology_hears_a_switch_with_the_labels_name() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let settings = Settings::new();
    runtime.display_frame(&settings, SIZE);

    let tree = runtime.accessibility_tree();
    assert_eq!(tree.nodes().len(), 1, "one element: the label is the switch's name, not a text beside it");
    let node = &tree.nodes()[0];
    assert_eq!((node.role, &*node.label, node.checked), (Role::Switch, "Wi-Fi", Some(false)));
    assert!(node.supports(&Action::Activate) && node.supports(&Action::Focus));

    assert_eq!(runtime.accessibility_action(node.id, Action::Activate), Ok(()));
    assert!(settings.wifi.get(), "the activate action flips the value");
    runtime.display_frame(&settings, SIZE);
    let tree = runtime.accessibility_tree();
    assert_eq!(tree.nodes()[0].checked, Some(true));
    assert_eq!(tree.nodes()[0].id, node.id, "the same element, with a new value");
}

#[test]
fn it_lowers_to_elements_with_no_canvas() {
    let page = bunny_ui::ssr::render(&Settings::new(), SIZE);
    assert!(page.html.contains("data-path"), "the row is a target the page can press");
    assert!(page.html.contains("Wi-Fi"));
    assert!(!page.html.contains("<canvas"), "a toggle is elements, never pixels");
}

#[test]
fn a_tool_sees_the_apps_lines_and_none_of_the_frameworks() {
    let runtime = Runtime::scene("toggle_sites");
    runtime.record_sites(true);
    let settings = Settings::new();
    runtime.display_frame(&settings, SIZE);
    let sites = runtime.sites();
    runtime.record_sites(false);

    assert!(sites.iter().any(|site| site.kind == NodeKind::Toggle), "the toggle itself is sited");
    assert!(sites.iter().any(|site| site.kind == NodeKind::Text), "and so is the label the app wrote");
    for site in &sites {
        let Site::Caller(location) = site.site else {
            continue;
        };
        // a path reads with either separator, by the platform that built it
        let file = location.file().replace('\\', "/");
        assert!(
            file.ends_with("tests/toggle.rs"),
            "{:?} at {location} is a line of the framework, shown as if the app wrote it",
            site.kind
        );
    }
}
