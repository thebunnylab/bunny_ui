//! `form!(rows…)` and `labeled(words, control)`.

use bunny_ui::accessibility::Role;
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 300.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Look {
    Light,
    Dark,
}

#[derive(Clone, Copy)]
struct Profile {
    name: State<String>,
    public: State<bool>,
    look: State<Look>,
}

impl Profile {
    fn new() -> Self {
        Self {
            name: State::new(String::new()),
            public: State::new(false),
            look: State::new(Look::Light),
        }
    }
}

impl Component for Profile {
    fn body(self) -> impl View {
        form!(
            labeled("Name", text_field("Jane Appleseed", self.name.binding()).id("name")),
            toggle(text("Public profile"), self.public.binding()).id("public"),
            labeled(
                "Theme",
                segmented(self.look.binding(), [(Look::Light, "Light"), (Look::Dark, "Dark")]).id("look"),
            ),
        )
    }
}

fn fills(commands: &[DrawCommand], color: Color) -> Vec<Rect> {
    commands
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color: fill, .. } if *fill == color => Some(*rect),
            _ => None,
        })
        .collect()
}

fn line_origin(commands: &[DrawCommand], words: &str) -> Option<Point> {
    commands.iter().find_map(|command| match command {
        DrawCommand::TextLine { origin, content, range, .. } if &content[range.0..range.1] == words => Some(*origin),
        _ => None,
    })
}

#[test]
fn the_rows_stand_on_one_card_with_a_hairline_between_two() {
    let runtime = Runtime::new();
    let profile = Profile::new();
    let frame = runtime.display_frame(&profile, SIZE).as_slice().to_vec();
    let theme = theme::current();

    let card = fills(&frame, theme.panel)
        .into_iter()
        .find(|rect| rect.size.width == SIZE.width)
        .expect("one card as wide as its place");
    let rules = fills(&frame, theme.divider);
    let rules: Vec<_> = rules.iter().filter(|rect| rect.size.height == 1.0).collect();
    assert_eq!(rules.len(), 2, "three rows, two hairlines: {rules:?}");
    for rule in &rules {
        assert_eq!(rule.origin.x, 12.0, "a hairline starts under the words");
        assert_eq!(rule.origin.x + rule.size.width, SIZE.width, "and runs to the trailing edge");
        assert!(rule.origin.y > card.origin.y && rule.origin.y < card.origin.y + card.size.height);
    }

    let name = line_origin(&frame, "Name").expect("the first row's words");
    let public = line_origin(&frame, "Public profile").expect("the second row's words");
    let theme_words = line_origin(&frame, "Theme").expect("the third row's words");
    assert_eq!((name.x, public.x, theme_words.x), (12.0, 12.0, 12.0), "the words share a leading edge");
    assert!(name.y < rules[0].origin.y && rules[0].origin.y < public.y, "the first hairline is between rows one and two");
    assert!(public.y < rules[1].origin.y && rules[1].origin.y < theme_words.y);
}

#[test]
fn a_labeled_row_puts_its_control_on_the_trailing_side() {
    let runtime = Runtime::new();
    let profile = Profile::new();
    runtime.display_frame(&profile, SIZE);
    let hits = runtime.hits_snapshot();
    let segments: Vec<_> = hits.iter().filter(|(path, _)| path.contains("[look]")).collect();
    let trailing = segments.iter().map(|(_, rect)| rect.origin.x + rect.size.width).fold(0.0, f64::max);
    assert!(
        (SIZE.width - 12.0 - 4.0..=SIZE.width - 12.0).contains(&trailing),
        "the control ends at the row's trailing inset: {trailing}"
    );
    let leading = segments.iter().map(|(_, rect)| rect.origin.x).fold(f64::MAX, f64::min);
    assert!(leading > SIZE.width / 3.0, "the room is between the words and the control: {leading}");
}

#[test]
fn tab_goes_through_the_rows_in_reading_order() {
    let runtime = Runtime::new();
    let profile = Profile::new();
    runtime.display_frame(&profile, SIZE);
    let mut seen = Vec::new();
    for _ in 0..3 {
        assert!(runtime.key_stroke(KeyPattern::key(Key::Tab)).handled);
        let held = runtime.focused().expect("a control");
        seen.push(["name", "public", "look"].into_iter().find(|name| held.contains(&format!("[{name}]"))));
        runtime.display_frame(&profile, SIZE);
    }
    assert_eq!(seen, [Some("name"), Some("public"), Some("look")]);
}

#[test]
fn the_words_of_a_labeled_row_name_its_control_and_are_heard_once() {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    let profile = Profile::new();
    runtime.display_frame(&profile, SIZE);
    let tree = runtime.accessibility_tree();
    let heard: Vec<_> = tree.nodes().iter().map(|node| (node.role, &*node.label)).collect();
    assert_eq!(
        heard,
        [
            // the field is named by its row, not by its placeholder
            (Role::TextField, "Name"),
            (Role::Switch, "Public profile"),
            // a control of several elements keeps the words before them
            (Role::Text, "Theme"),
            (Role::RadioButton, "Light"),
            (Role::RadioButton, "Dark"),
        ]
    );
}

#[test]
fn right_to_left_puts_the_words_on_the_right() {
    let runtime = Runtime::new();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let profile = Profile::new();
    let frame = runtime.display_frame(&profile, SIZE).as_slice().to_vec();
    let name = line_origin(&frame, "Name").expect("the words");
    assert!(name.x > SIZE.width / 2.0, "the leading side is the right: {}", name.x);
}

#[test]
fn a_tool_sees_the_form_its_rows_and_only_the_apps_lines() {
    let page = bunny_ui::ssr::render(&Profile::new(), SIZE);
    assert!(page.html.contains("Public profile") && !page.html.contains("<canvas"));

    let runtime = Runtime::scene("form_sites");
    runtime.record_sites(true);
    runtime.display_frame(&Profile::new(), SIZE);
    let sites = runtime.sites();
    runtime.record_sites(false);
    for kind in [NodeKind::Form, NodeKind::Labeled, NodeKind::Field, NodeKind::Toggle, NodeKind::Segmented] {
        assert!(sites.iter().any(|site| site.kind == kind), "{kind:?} is sited");
    }
    for site in &sites {
        if let Site::Caller(location) = site.site {
            // a path reads with either separator, by the platform that built it
            let file = location.file().replace('\\', "/");
            assert!(file.ends_with("tests/form.rs"), "{:?} at {location}", site.kind);
        }
    }
}
