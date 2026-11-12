//! `segmented(binding, choices)`: a row of segments that chooses one value.

use bunny_ui::accessibility::{Action, Role};
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 200.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    List,
    Grid,
    Columns,
    /// A value the control is not given.
    Gallery,
}

#[derive(Clone, Copy)]
struct Bar {
    mode: State<Mode>,
    before: State<String>,
    after: State<String>,
    locked: State<bool>,
}

impl Bar {
    fn new() -> Self {
        Self {
            mode: State::new(Mode::List),
            before: State::new(String::new()),
            after: State::new(String::new()),
            locked: State::new(false),
        }
    }
}

impl Component for Bar {
    fn body(self) -> impl View {
        vstack!(
            text_field("Before", self.before.binding()).id("before"),
            segmented(
                self.mode.binding(),
                [(Mode::List, "List"), (Mode::Grid, "Grid"), (Mode::Columns, "Columns")],
            )
            .disabled(self.locked.get())
            .id("mode"),
            text_field("After", self.after.binding()).id("after"),
        )
    }
}

/// The three segments' targets, left to right.
fn segments(runtime: &Runtime) -> Vec<(String, Rect)> {
    let mut found: Vec<_> = runtime
        .hits_snapshot()
        .into_iter()
        .filter(|(path, _)| path.contains("[mode]"))
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

/// Which segment holds the keyboard, by its place from the left.
fn focused_segment(runtime: &Runtime) -> Option<usize> {
    let held = runtime.focused()?;
    segments(runtime).iter().position(|(path, _)| *path == held)
}

fn settled(runtime: &Runtime, bar: &Bar) -> Vec<DrawCommand> {
    runtime.display_frame(bar, SIZE);
    for _ in 0..240 {
        runtime.tick(1.0 / 60.0);
    }
    runtime.display_frame(bar, SIZE).as_slice().to_vec()
}

/// The thumb is the fill in the panel colour on the segments' own line
/// (a field's well is the same colour, on another line).
fn thumb(runtime: &Runtime, commands: &[DrawCommand]) -> Option<Rect> {
    let panel = theme::current().panel;
    let line = segments(runtime).first()?.1.origin.y;
    commands.iter().find_map(|command| match command {
        DrawCommand::FillRect { rect, color, .. } if *color == panel && rect.origin.y == line => Some(*rect),
        _ => None,
    })
}

#[test]
fn the_segments_share_the_width_and_a_press_selects() {
    let runtime = Runtime::new();
    let bar = Bar::new();
    runtime.display_frame(&bar, SIZE);
    let found = segments(&runtime);
    assert_eq!(found.len(), 3, "one target for each segment");
    let width = found[0].1.size.width;
    for pair in found.windows(2) {
        assert_eq!(pair[1].1.size.width, width, "equal widths");
        assert_eq!(pair[1].1.origin.x, pair[0].1.origin.x + width, "side by side, no gap");
        assert_eq!(pair[1].1.origin.y, pair[0].1.origin.y);
    }
    assert_eq!(found[0].1.size.height, 26.0);

    press(&runtime, found[2].1);
    assert_eq!(bar.mode.get(), Mode::Columns);
    press(&runtime, found[1].1);
    assert_eq!(bar.mode.get(), Mode::Grid);
}

#[test]
fn the_thumb_stands_behind_the_selected_segment() {
    let runtime = Runtime::new();
    let bar = Bar::new();
    let frame = settled(&runtime, &bar);
    let found = segments(&runtime);
    assert_eq!(thumb(&runtime, &frame).expect("a thumb").origin.x, found[0].1.origin.x);

    bar.mode.set(Mode::Columns);
    // the first frame after the change: the thumb is on its way
    let first = runtime.display_frame(&bar, SIZE).as_slice().to_vec();
    let first = thumb(&runtime, &first).expect("a thumb").origin.x;
    assert!(first < found[2].1.origin.x, "it does not jump: {first}");
    let frame = settled(&runtime, &bar);
    let rest = thumb(&runtime, &frame).expect("a thumb");
    assert_eq!(rest.origin.x, found[2].1.origin.x);
    assert_eq!(rest.size, found[2].1.size, "the thumb is as large as a segment");
}

#[test]
fn the_control_is_one_tab_stop_on_its_selected_segment() {
    let runtime = Runtime::new();
    let bar = Bar::new();
    bar.mode.set(Mode::Grid);
    runtime.display_frame(&bar, SIZE);
    assert!(runtime.focus_named("before"));

    assert!(key(&runtime, Key::Tab));
    assert_eq!(focused_segment(&runtime), Some(1), "Tab lands on the selected segment");
    assert!(key(&runtime, Key::Tab));
    assert!(runtime.focused().is_some_and(|path| path.contains("[after]")), "and leaves the control whole");

    let mut back = KeyPattern::key(Key::Tab);
    back.shift = true;
    assert!(runtime.key_stroke(back).handled);
    assert_eq!(focused_segment(&runtime), Some(1), "Shift+Tab comes back to the selected segment");
    assert!(runtime.key_stroke(back).handled);
    assert!(runtime.focused().is_some_and(|path| path.contains("[before]")));
    assert_eq!(bar.mode.get(), Mode::Grid, "Tab selects nothing");
}

#[test]
fn the_arrows_move_and_select_and_go_round() {
    let runtime = Runtime::new();
    let bar = Bar::new();
    runtime.display_frame(&bar, SIZE);
    assert!(runtime.focus_named("before"));
    assert!(key(&runtime, Key::Tab));
    let step = |pressed: Key, mode: Mode, segment: usize| {
        assert!(key(&runtime, pressed), "{pressed:?} is the group's");
        assert_eq!(bar.mode.get(), mode, "{pressed:?} selects");
        runtime.display_frame(&bar, SIZE);
        assert_eq!(focused_segment(&runtime), Some(segment), "{pressed:?} moves the keyboard");
    };
    step(Key::Right, Mode::Grid, 1);
    step(Key::Right, Mode::Columns, 2);
    step(Key::Right, Mode::List, 0);
    step(Key::Left, Mode::Columns, 2);
    step(Key::Home, Mode::List, 0);
    step(Key::End, Mode::Columns, 2);
    step(Key::Up, Mode::Grid, 1);
    step(Key::Down, Mode::Columns, 2);
    // on the last segment End has nowhere to go, and is left to others
    assert!(!key(&runtime, Key::End));
}

#[test]
fn the_keyboard_goes_with_a_selection_the_app_writes_only_while_it_is_in_the_group() {
    let runtime = Runtime::new();
    let bar = Bar::new();
    runtime.display_frame(&bar, SIZE);
    assert!(runtime.focus_named("before"));
    // the keyboard is in a field: a write moves the selection and nothing else
    bar.mode.set(Mode::Columns);
    runtime.display_frame(&bar, SIZE);
    assert!(runtime.focused().is_some_and(|path| path.contains("[before]")), "a selection takes no keyboard");

    assert!(key(&runtime, Key::Tab));
    assert_eq!(focused_segment(&runtime), Some(2));
    // the keyboard is on a segment: it goes where the selection goes
    bar.mode.set(Mode::List);
    runtime.display_frame(&bar, SIZE);
    assert_eq!(focused_segment(&runtime), Some(0), "the keyboard and the selection are one");
    // a value that is no choice selects nothing, and the keyboard stays where it is
    bar.mode.set(Mode::Gallery);
    runtime.display_frame(&bar, SIZE);
    assert_eq!(focused_segment(&runtime), Some(0));
}

#[test]
fn right_to_left_swaps_the_horizontal_arrows() {
    let runtime = Runtime::new();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let bar = Bar::new();
    runtime.display_frame(&bar, SIZE);
    assert!(runtime.focus_named("before"));
    assert!(key(&runtime, Key::Tab));
    assert!(key(&runtime, Key::Left));
    assert_eq!(bar.mode.get(), Mode::Grid, "left is forward where the row reads right to left");
    assert!(key(&runtime, Key::Right));
    assert_eq!(bar.mode.get(), Mode::List);

    // and the first choice stands on the right
    runtime.display_frame(&bar, SIZE);
    let found = segments(&runtime);
    press(&runtime, found[2].1);
    assert_eq!(bar.mode.get(), Mode::List, "the rightmost segment is the first choice");
}

#[test]
fn space_and_enter_select_the_segment_with_the_keyboard() {
    let runtime = Runtime::new();
    let bar = Bar::new();
    bar.mode.set(Mode::Gallery);
    runtime.display_frame(&bar, SIZE);
    assert!(runtime.focus_named("before"));
    assert!(key(&runtime, Key::Tab));
    assert_eq!(focused_segment(&runtime), Some(0), "with nothing selected Tab lands on the first");
    assert_eq!(bar.mode.get(), Mode::Gallery, "and selects nothing");
    assert!(key(&runtime, Key::Enter));
    assert_eq!(bar.mode.get(), Mode::List);
    bar.mode.set(Mode::Gallery);
    runtime.display_frame(&bar, SIZE);
    assert!(key(&runtime, Key::Char(' ')));
    assert_eq!(bar.mode.get(), Mode::List);
}

#[test]
fn a_value_that_is_no_choice_selects_nothing_and_is_not_repaired() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let bar = Bar::new();
    bar.mode.set(Mode::Gallery);
    let frame = settled(&runtime, &bar);
    assert_eq!(thumb(&runtime, &frame), None, "no thumb");
    let tree = runtime.accessibility_tree();
    assert!(tree.nodes().iter().filter(|node| node.role == Role::RadioButton).all(|node| node.checked == Some(false)));
    assert_eq!(bar.mode.get(), Mode::Gallery, "the binding is the app's");
}

#[test]
fn assistive_technology_hears_three_radio_buttons() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let bar = Bar::new();
    runtime.display_frame(&bar, SIZE);
    let tree = runtime.accessibility_tree();
    let radios: Vec<_> = tree.nodes().iter().filter(|node| node.role == Role::RadioButton).collect();
    let heard: Vec<_> = radios.iter().map(|node| (&*node.label, node.checked)).collect();
    assert_eq!(heard, [("List", Some(true)), ("Grid", Some(false)), ("Columns", Some(false))]);

    assert_eq!(runtime.accessibility_action(radios[1].id, Action::Activate), Ok(()));
    assert_eq!(bar.mode.get(), Mode::Grid);
    runtime.display_frame(&bar, SIZE);
    let tree = runtime.accessibility_tree();
    let checked: Vec<_> = tree.nodes().iter().filter(|node| node.role == Role::RadioButton).map(|node| node.checked).collect();
    assert_eq!(checked, [Some(false), Some(true), Some(false)]);
}

#[test]
fn a_disabled_control_keeps_its_selection_and_takes_nothing() {
    let runtime = Runtime::new();
    let bar = Bar::new();
    bar.locked.set(true);
    runtime.display_frame(&bar, SIZE);
    assert!(segments(&runtime).is_empty(), "no segment is a target");
    assert!(runtime.focus_named("before"));
    assert!(key(&runtime, Key::Tab));
    assert!(runtime.focused().is_some_and(|path| path.contains("[after]")), "Tab passes it by");
    assert_eq!(bar.mode.get(), Mode::List);
}

#[test]
fn it_lowers_to_elements_and_a_tool_sees_only_the_apps_line() {
    let page = bunny_ui::ssr::render(&Bar::new(), SIZE);
    assert!(page.html.contains("Columns") && !page.html.contains("<canvas"));

    let runtime = Runtime::scene("segmented_sites");
    runtime.record_sites(true);
    runtime.display_frame(&Bar::new(), SIZE);
    let sites = runtime.sites();
    runtime.record_sites(false);
    assert!(sites.iter().any(|site| site.kind == NodeKind::Segmented));
    for site in &sites {
        if let Site::Caller(location) = site.site {
            // a path reads with either separator, by the platform that built it
            let file = location.file().replace('\\', "/");
            assert!(file.ends_with("tests/segmented.rs"), "{:?} at {location}", site.kind);
        }
    }
}
