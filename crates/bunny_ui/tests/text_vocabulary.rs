//! The text vocabulary through the public door: hard breaks, the light
//! weights, a paragraph of styled runs, its rules, and a highlight in
//! more than one colour — each as a view writes it, read back from the
//! frame it paints.

use bunny_ui_core as bunny_ui;

use bunny_ui::layout::{DisplayList, DrawCommand};
use bunny_ui::prelude::*;

const RED: Color = Color { r: 255, g: 0, b: 0, a: 255 };
const BLUE: Color = Color { r: 0, g: 0, b: 255, a: 255 };
const SAGE: Color = Color { r: 120, g: 160, b: 120, a: 255 };

fn frame(view: &impl View) -> DisplayList {
    Runtime::new().display_frame(view, Size { width: 400.0, height: 300.0 })
}

/// Each painted piece of words: its text, where it starts, its ink and
/// its weight.
fn pieces(display: &DisplayList) -> Vec<(String, Point, Color, Weight)> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, origin, color, font } => {
                Some((content[range.0..range.1].to_string(), *origin, *color, font.weight))
            }
            _ => None,
        })
        .collect()
}

fn rules(display: &DisplayList, color: Color) -> Vec<Rect> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color: ink, .. } if *ink == color => Some(*rect),
            _ => None,
        })
        .collect()
}

#[test]
fn a_hard_break_in_a_text_starts_a_new_line() {
    let display = frame(&vstack!(text("first\nsecond")).padding_length(0.0));
    let lines: Vec<(String, f64)> = pieces(&display).into_iter().map(|(words, origin, ..)| (words, origin.y)).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0].0, "first");
    assert_eq!(lines[1].0, "second");
    assert!(lines[1].1 > lines[0].1, "the second line stands under the first");
}

#[test]
fn a_light_weight_reaches_the_line_it_paints() {
    let display = frame(&text("step 1").font_weight(Weight::Light));
    assert!(pieces(&display).iter().all(|(.., weight)| *weight == Weight::Light));
    assert_eq!(Weight::Light.number(), 300);
    assert!(Weight::Thin.number() < Weight::ExtraLight.number());
}

#[test]
fn a_rich_paragraph_paints_each_run_in_its_own_style() {
    let paragraph = rich_text([
        run("Fees within "),
        run("thirty").strikethrough(Some(RED)),
        run(" "),
        run("forty-five").bold().background_color(SAGE),
        run(" days."),
    ])
    .foreground_color(Color::BLACK);
    let display = frame(&paragraph);
    let words: Vec<(String, Weight)> = pieces(&display).into_iter().map(|(words, _, _, weight)| (words, weight)).collect();
    assert_eq!(
        words,
        [
            ("Fees within ".to_string(), Weight::Regular),
            ("thirty".to_string(), Weight::Regular),
            (" ".to_string(), Weight::Regular),
            ("forty-five".to_string(), Weight::Bold),
            (" days.".to_string(), Weight::Regular),
        ]
    );
    // one line: every run shares its top
    let tops: Vec<f64> = pieces(&display).into_iter().map(|(_, origin, ..)| origin.y).collect();
    assert!(tops.windows(2).all(|pair| pair[0] == pair[1]), "{tops:?}");
    assert_eq!(rules(&display, RED).len(), 1, "the struck run draws its rule in its own colour");
    assert_eq!(rules(&display, SAGE).len(), 1, "the inserted run draws its ground");
}

#[test]
fn an_underline_on_a_text_draws_one_rule_in_the_words_ink() {
    let display = frame(&text("link").foreground_color(BLUE).underline(None));
    let under = rules(&display, BLUE);
    assert_eq!(under.len(), 1);
    let (_, origin, ..) = &pieces(&display)[0];
    assert!(under[0].origin.y > origin.y, "the rule stands under the words");
}

#[test]
fn a_highlight_paints_each_stretch_in_its_own_colour_over_the_runs() {
    let display = frame(
        &rich_text([run("fn "), run("main").bold()])
            .highlight_colored(vec![(0, 2, RED), (3, 7, BLUE)]),
    );
    let inks: Vec<(String, Color, Weight)> =
        pieces(&display).into_iter().map(|(words, _, color, weight)| (words, color, weight)).collect();
    assert_eq!(inks[0], ("fn".to_string(), RED, Weight::Regular));
    assert_eq!(inks.last().cloned(), Some(("main".to_string(), BLUE, Weight::Bold)), "the run keeps its face");
}

#[test]
fn the_print_names_the_rules_a_text_wears() {
    let printed = Runtime::new().render_stable(&text("old").strikethrough(Some(RED)).underline(None));
    assert!(printed.contains(".strikethrough("), "{printed}");
    assert!(printed.contains(".underline()"), "{printed}");
}
