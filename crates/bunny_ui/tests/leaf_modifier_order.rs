//! A leaf modifier reaches its leaf through every wrapper of the same view.
//!
//! A field, a text and a scroll region each answer modifiers that change
//! the leaf itself: `.select_on_beat`, `.truncation_mode`, `.highlight`,
//! `.scroll_target`. A wrapper such as `.hug_height()` or `.tooltip(..)`
//! is still the same view. The order between the two must not matter.

use bunny_ui::layout::{DrawCommand, LayoutMode};
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

/// One test for each wrapper: the beat is written AFTER the wrapper, and
/// the field must still take the keyboard when the beat rings.
macro_rules! a_field_ask_crosses {
    ($($name:ident: { $($wrapper:tt)+ })+) => {$(
        #[test]
        fn $name() {
            #[derive(Clone, Copy)]
            struct Scene {
                query: State<String>,
                body: State<String>,
                beat: State<u64>,
            }

            impl Component for Scene {
                fn body(self) -> impl View {
                    vstack!(
                        text_field("Find", self.query.binding())
                            .id("query")
                            $($wrapper)+
                            .select_on_beat(self.beat.get()),
                        text_field("Body", self.body.binding()).id("editor"),
                    )
                }
            }

            let scene = Scene {
                query: State::new(String::from("needle")),
                body: State::new(String::new()),
                beat: State::new(0),
            };
            let runtime = Runtime::new();
            runtime.display_frame(&scene, SIZE);
            assert!(runtime.focus_named("editor"));
            assert!(holds(&runtime, "editor"));

            scene.beat.set(1);
            runtime.display_frame(&scene, SIZE);
            assert!(
                holds(&runtime, "query"),
                "the beat written after the wrapper must reach the field; the keys are on {:?}",
                runtime.focused()
            );
            assert!(runtime.key(EditCommand::Insert("x".into())).applied);
            assert_eq!(scene.query.get(), "x", "the summoned query was selected whole");
        }
    )+};
}

a_field_ask_crosses! {
    a_field_beat_written_after_hug_height_takes_the_keyboard: { .hug_height() }
    a_field_beat_written_after_padding_takes_the_keyboard: { .padding() }
    a_field_beat_written_after_a_frame_takes_the_keyboard: { .frame_width(200.0) }
    a_field_beat_written_after_ignores_safe_area_takes_the_keyboard: { .ignores_safe_area() }
    a_field_beat_written_after_a_tooltip_takes_the_keyboard: { .tooltip("Find in file") }
    a_field_beat_written_after_on_measure_takes_the_keyboard: { .on_measure(|_| {}) }
    a_field_beat_written_after_a_hover_group_takes_the_keyboard: { .hover_group() }
    a_field_beat_written_after_a_drag_region_takes_the_keyboard: { .window_drag_region() }
    a_field_beat_written_after_exact_layout_takes_the_keyboard: { .layout(LayoutMode::Exact) }
    a_field_beat_written_after_a_context_menu_takes_the_keyboard: {
        .context_menu(vec![menu_item("Clear", || {})])
    }
    a_field_beat_written_after_a_drop_target_takes_the_keyboard: { .on_drop::<String>(|_| {}) }
}

fn painted(view: &impl View, recording: bool) -> Vec<DrawCommand> {
    let runtime = Runtime::new();
    runtime.record_sites(recording);
    let display = runtime.display_frame(view, SIZE);
    runtime.record_sites(false);
    display.as_slice().to_vec()
}

fn text_lines(commands: &[DrawCommand]) -> Vec<&str> {
    commands
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, .. } => Some(&content[range.0..range.1]),
            _ => None,
        })
        .collect()
}

const LONG: &str = "a label that is far too long for the box it was given";

#[test]
fn a_truncation_written_after_a_frame_truncates() {
    let inside = painted(&text(LONG).truncation_mode(Truncation::End).frame_width(90.0), false);
    let outside = painted(&text(LONG).frame_width(90.0).truncation_mode(Truncation::End), false);
    assert_eq!(text_lines(&inside).len(), 1, "a truncated label is one line");
    assert_eq!(
        text_lines(&outside),
        text_lines(&inside),
        "the frame is the same view; the truncation must reach the text"
    );
}

#[test]
fn a_truncation_written_after_padding_truncates() {
    let inside = painted(
        &text(LONG).truncation_mode(Truncation::End).padding().frame_width(120.0),
        false,
    );
    let outside = painted(
        &text(LONG).padding().truncation_mode(Truncation::End).frame_width(120.0),
        false,
    );
    assert_eq!(text_lines(&outside), text_lines(&inside));
}

#[test]
fn a_highlight_written_after_a_frame_paints_the_same_commands() {
    let ink = Color::rgb(200, 40, 40);
    let inside = painted(&text("needle in hay").highlight(vec![(0, 6)], ink).frame_width(200.0), false);
    let outside = painted(&text("needle in hay").frame_width(200.0).highlight(vec![(0, 6)], ink), false);
    assert_eq!(outside, inside);
    let plain = painted(&text("needle in hay").frame_width(200.0), false);
    assert_ne!(inside, plain, "the highlight paints something");
}

#[test]
fn recording_sites_keeps_a_text_modifier() {
    let view = text(LONG).truncation_mode(Truncation::End).frame_width(90.0);
    let ordinary = painted(&view, false);
    let recorded = painted(&view, true);
    assert_eq!(text_lines(&ordinary).len(), 1);
    assert_eq!(
        text_lines(&recorded),
        text_lines(&ordinary),
        "a source site is the same view; recording must not drop the truncation"
    );
}

#[test]
fn recording_sites_keeps_a_field_modifier() {
    #[derive(Clone, Copy)]
    struct Scene {
        query: State<String>,
        body: State<String>,
        beat: State<u64>,
    }

    impl Component for Scene {
        fn body(self) -> impl View {
            vstack!(
                text_field("Find", self.query.binding())
                    .id("query")
                    .select_on_beat(self.beat.get()),
                text_field("Body", self.body.binding()).id("editor"),
            )
        }
    }

    let scene = Scene {
        query: State::new(String::new()),
        body: State::new(String::new()),
        beat: State::new(0),
    };
    let runtime = Runtime::scene("leaf_modifier_order");
    runtime.record_sites(true);
    runtime.display_frame(&scene, SIZE);
    assert!(runtime.focus_named("editor"));
    scene.beat.set(1);
    runtime.display_frame(&scene, SIZE);
    let query = holds(&runtime, "query");
    runtime.record_sites(false);
    assert!(query, "a recorded field must still answer its beat");
}

/// A region finds its target by the frame of a keyed component.
#[derive(Clone, Copy)]
struct Row(usize);

impl Component for Row {
    fn body(self) -> impl View {
        text(format!("row {}", self.0)).frame_height(20.0)
    }
}

fn rows() -> impl View {
    vstack!(for_each(
        (0..40).collect::<Vec<usize>>(),
        |row| row.to_string(),
        |row| Row(row.to_owned()).id(format!("row-{row}")),
    ))
    .spacing(0.0)
}

/// A field ask on a view with no field is lost. A debug build says so
/// once for the line that wrote it; a field that is reached says nothing.
#[test]
#[cfg(debug_assertions)]
fn a_field_ask_that_reaches_no_field_says_so() {
    let _ = bunny_ui::diagnostics::take_unreached_fields();
    let value = State::new(String::new());
    let reached = text_field("Name", value.binding()).id("name").hug_height().auto_focus();
    let runtime = Runtime::new();
    runtime.display_frame(&reached, SIZE);
    assert!(bunny_ui::diagnostics::take_unreached_fields().is_empty());

    let lost = text("no field here").auto_focus();
    for _ in 0..3 {
        Runtime::new().display_frame(&lost, SIZE);
    }
    let hints = bunny_ui::diagnostics::take_unreached_fields();
    assert_eq!(hints.len(), 1, "once for the line, however many passes run");
    assert_eq!(hints[0].modifier, "auto_focus");
    assert!(hints[0].site.file().ends_with("leaf_modifier_order.rs"));
}

fn offset_after_reveal(view: &impl View) -> f64 {
    let runtime = Runtime::new();
    runtime.render_stable(view);
    let layout = runtime.layout(view, Proposal::exact(SIZE));
    let region = layout.scrolls.first().expect("the region");
    runtime.scroll_offset(&region.path).y
}

#[test]
fn a_hugging_scroll_follows_its_target() {
    // forty 20 pt rows in a 100 pt window: row 30 spans 600–620, and the
    // near edge that reveals it is its bottom
    let plain = scroll(rows()).scroll_target("row-30").frame(200.0, 100.0).id("list");
    assert_eq!(offset_after_reveal(&plain), 520.0);
    let hugging = scroll(rows()).hugging().scroll_target("row-30").frame(200.0, 100.0).id("list");
    assert_eq!(
        offset_after_reveal(&hugging),
        520.0,
        "a region that hugs its content is the same region; it must follow its target"
    );
}
