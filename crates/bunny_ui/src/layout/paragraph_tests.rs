use super::*;
use crate::text_engine::{TextRaster, Weight};

// =============================================================================
// Harness
// =============================================================================

/// A face whose numbers follow its spec: half the size per character,
/// a quarter more for a bold weight, plus the tracking; an ascent of
/// three quarters of the size and a descent of a quarter. The house font
/// ignores size and weight on purpose, and these tests are about faces
/// that differ.
struct Proportional;

impl TextEngine for Proportional {
    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let bold = if font.weight.number() >= 600 { 1.25 } else { 1.0 };
        LineMetrics {
            width: text.chars().count() as Px * (font.size * 0.5 * bold + font.tracking),
            ascent: font.size * 0.75,
            descent: font.size * 0.25,
        }
    }

    fn decoration_thickness(&self, font: &FontSpec) -> Option<Px> {
        Some(font.size / 5.0)
    }

    fn raster_line(&self, text: &str, font: &FontSpec, color: Color, scale: usize) -> Option<TextRaster> {
        PixelFont.raster_line(text, font, color, scale)
    }
}

/// A paragraph at 10 pt in the proportional face.
fn ten() -> FontSpec {
    FontSpec { size: 10.0, ..FontSpec::DEFAULT }
}

fn place_in(node: &LayoutNode, proposal: Proposal, engine: &dyn TextEngine, font: FontSpec) -> LayoutResult {
    let images = RawImages::default();
    let cache = MeasureCache::default();
    let offsets = HashMap::default();
    let interaction = Interaction::default();
    let carets = HashMap::default();
    layout_with(
        node,
        proposal,
        LayoutEnv {
            text: engine,
            images: &images,
            cache: &cache,
            scroll_offsets: &offsets,
            font,
            line_height: None,
            text_align: None,
            direction: LayoutDirection::LeftToRight,
            flips: false,
            stamp: FrameStamp::idle(&interaction, &carets),
            animator: None,
            anim: None,
            live: None,
            overlay_bounds: None,
            dialog_frames: None,
            scale: 1.0,
            touch: false,
            accessibility: false,
        },
    )
}

fn text(content: &str, marks: Option<TextMarks>, truncation: Option<Truncation>) -> LayoutNode {
    LayoutNode::Text {
        content: crate::bind::TextSource::from(content),
        highlights: marks.map(Rc::new),
        truncation,
        hints: ElementHints::default(),
        action: None,
    }
}

/// The painted words: each line's text, its origin and its ink.
fn words(result: &LayoutResult) -> Vec<(String, Point, Color)> {
    result
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, origin, color, .. } => {
                Some((content[range.0..range.1].to_string(), *origin, *color))
            }
            _ => None,
        })
        .collect()
}

fn fills(result: &LayoutResult) -> Vec<(Rect, Color)> {
    result
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } => Some((*rect, *color)),
            _ => None,
        })
        .collect()
}

fn at(x: Px, y: Px) -> Point {
    Point { x, y }
}

fn bold() -> RunStyle {
    RunStyle { font: FontPatch { weight: Some(Weight::Bold), ..FontPatch::default() }, ..RunStyle::default() }
}

const RED: Color = Color { r: 255, g: 0, b: 0, a: 255 };
const BLUE: Color = Color { r: 0, g: 0, b: 255, a: 255 };
const SAGE: Color = Color { r: 120, g: 160, b: 120, a: 255 };

// =============================================================================
// Hard breaks
// =============================================================================

#[test]
fn a_hard_break_ends_its_line_where_the_text_would_fit() {
    // 8 pt a character in the house font, 16 a line
    let result = layout(&text("ab\ncdef", None, None), Proposal::unspecified());
    assert_eq!(result.size, Size { width: 32.0, height: 32.0 }, "the widest line, two lines tall");
    assert_eq!(
        words(&result),
        [("ab".to_string(), at(0.0, 0.0), Color::BLACK), ("cdef".to_string(), at(0.0, 16.0), Color::BLACK)],
        "the break itself is never painted"
    );
}

#[test]
fn a_blank_paragraph_keeps_its_line() {
    let result = layout(&text("a\n\nb", None, None), Proposal::unspecified());
    assert_eq!(result.size.height, 48.0);
    let lines: Vec<(String, Px)> = words(&result).into_iter().map(|(words, origin, _)| (words, origin.y)).collect();
    assert_eq!(lines, [("a".to_string(), 0.0), ("b".to_string(), 32.0)]);
}

#[test]
fn soft_wraps_run_inside_each_hard_line() {
    let result = layout(&text("aa bb\ncc", None, None), Proposal { width: Some(24.0), height: None });
    assert_eq!(result.size, Size { width: 24.0, height: 48.0 });
    let lines: Vec<String> = words(&result).into_iter().map(|(words, ..)| words).collect();
    assert_eq!(lines, ["aa ", "bb", "cc"]);
}

#[test]
fn truncation_cuts_each_hard_line_on_its_own() {
    let node = text("abcdefgh\nab", None, Some(Truncation::End));
    let result = layout(&node, Proposal { width: Some(40.0), height: None });
    assert_eq!(result.size, Size { width: 40.0, height: 32.0 }, "one line per hard line, never a wrap");
    let lines: Vec<String> = words(&result).into_iter().map(|(words, ..)| words).collect();
    assert_eq!(lines, ["abcd…", "ab"]);
}

#[test]
fn a_cut_paragraph_closes_at_its_own_break() {
    // two rows of room for three lines: the second says more follows,
    // and never borrows the words past its break
    let node = text("abcdef\ncd\nef", None, None);
    let result = layout(&node, Proposal::exact(Size { width: 100.0, height: 32.0 }));
    let lines: Vec<String> = words(&result).into_iter().map(|(words, ..)| words).collect();
    assert_eq!(lines, ["abcdef", "cd…"]);
}

// =============================================================================
// Inks
// =============================================================================

#[test]
fn each_ink_paints_in_its_own_colour() {
    let marks = TextMarks::of_inks([
        InkSpan { start: 4, end: 6, color: BLUE },
        InkSpan { start: 0, end: 2, color: RED },
    ]);
    assert_eq!(marks.inks[0].start, 0, "inks are kept in the order they start");
    let result = layout(&text("abcdef", Some(marks), None), Proposal::unspecified());
    assert_eq!(
        words(&result),
        [
            ("ab".to_string(), at(0.0, 0.0), RED),
            ("cd".to_string(), at(16.0, 0.0), Color::BLACK),
            ("ef".to_string(), at(32.0, 0.0), BLUE),
        ]
    );
    assert_eq!(result.size.width, 48.0, "an ink never touches the measure");
}

// =============================================================================
// Runs
// =============================================================================

#[test]
fn a_run_in_another_face_is_measured_in_it() {
    // "ab" at 5 a character, "cd" bold at 6.25
    let marks = TextMarks { runs: vec![TextRun { start: 2, end: 4, style: bold() }], ..TextMarks::default() };
    let result = place_in(&text("abcd", Some(marks), None), Proposal::unspecified(), &Proportional, ten());
    assert_eq!(result.size, Size { width: 22.5, height: 10.0 });
    let fonts: Vec<(String, Px, Weight)> = result
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, origin, font, .. } => {
                Some((content[range.0..range.1].to_string(), origin.x, font.weight))
            }
            _ => None,
        })
        .collect();
    assert_eq!(fonts, [("ab".to_string(), 0.0, Weight::Regular), ("cd".to_string(), 10.0, Weight::Bold)]);
}

#[test]
fn a_paragraph_of_runs_breaks_where_its_runs_say() {
    // "aaaa " is 25 and the bold "bbbb" 25: 50 in all, where one face
    // would be 45 — a box of 48 holds the plain line and not the rich one
    let marks = TextMarks { runs: vec![TextRun { start: 5, end: 9, style: bold() }], ..TextMarks::default() };
    let rich = place_in(&text("aaaa bbbb", Some(marks), None), Proposal { width: Some(48.0), height: None }, &Proportional, ten());
    let plain = place_in(&text("aaaa bbbb", None, None), Proposal { width: Some(48.0), height: None }, &Proportional, ten());
    assert_eq!(plain.size.height, 10.0, "one face fits on one line");
    assert_eq!(rich.size.height, 20.0, "the bold run takes the paragraph to two");
    let lines: Vec<(String, Point)> = words(&rich).into_iter().map(|(words, origin, _)| (words, origin)).collect();
    assert_eq!(lines, [("aaaa ".to_string(), at(0.0, 0.0)), ("bbbb".to_string(), at(0.0, 10.0))]);
}

#[test]
fn a_line_is_as_tall_as_its_tallest_run_and_every_run_shares_its_baseline() {
    // a 20 pt run in a 10 pt paragraph: the line is 15 + 5 tall, and both
    // runs stand on its baseline at 15
    let big = RunStyle { font: FontPatch { size: Some(20.0), ..FontPatch::default() }, ..RunStyle::default() };
    let marks = TextMarks { runs: vec![TextRun { start: 1, end: 2, style: big }], ..TextMarks::default() };
    let result = place_in(&text("aB\nc", Some(marks), None), Proposal::unspecified(), &Proportional, ten());
    assert_eq!(result.size, Size { width: 15.0, height: 30.0 }, "20 for the tall line, 10 for the plain one");
    let tops: Vec<(String, Point)> = words(&result).into_iter().map(|(words, origin, _)| (words, origin)).collect();
    assert_eq!(
        tops,
        [("a".to_string(), at(0.0, 7.5)), ("B".to_string(), at(5.0, 0.0)), ("c".to_string(), at(0.0, 20.0))],
        "a 10 pt glyph box hangs 7.5 below the line's top, a 20 pt one none"
    );
}

#[test]
fn runs_that_only_colour_keep_the_one_shaping() {
    let ink = RunStyle { color: Some(RED), ..RunStyle::default() };
    let marks = TextMarks { runs: vec![TextRun { start: 1, end: 3, style: ink }], ..TextMarks::default() };
    assert!(!marks.reshapes());
    let result = layout(&text("abcd", Some(marks), None), Proposal::unspecified());
    assert_eq!(result.size.width, 32.0);
    assert_eq!(
        words(&result),
        [
            ("a".to_string(), at(0.0, 0.0), Color::BLACK),
            ("bc".to_string(), at(8.0, 0.0), RED),
            ("d".to_string(), at(24.0, 0.0), Color::BLACK),
        ],
        "each piece at its measured prefix"
    );
}

#[test]
fn an_ink_over_a_run_wins_the_colour_and_keeps_the_face() {
    let marks = TextMarks {
        runs: vec![TextRun { start: 0, end: 4, style: RunStyle { color: Some(RED), ..bold() } }],
        inks: vec![InkSpan { start: 1, end: 2, color: BLUE }],
        ..TextMarks::default()
    };
    let result = place_in(&text("abcd", Some(marks), None), Proposal::unspecified(), &Proportional, ten());
    assert_eq!(
        words(&result),
        [
            ("a".to_string(), at(0.0, 0.0), RED),
            ("b".to_string(), at(6.25, 0.0), BLUE),
            ("cd".to_string(), at(12.5, 0.0), RED),
        ]
    );
}

#[test]
fn an_end_cut_keeps_the_runs_and_closes_in_the_face_it_cut() {
    // "ab" 10 + bold "cdef" 25 = 35 in a box of 24: "ab" + bold "c" (6.25)
    // + a bold ellipsis (6.25) is 22.5, and one more character is not
    let marks = TextMarks { runs: vec![TextRun { start: 2, end: 6, style: bold() }], ..TextMarks::default() };
    let node = text("abcdef", Some(marks), Some(Truncation::End));
    let result = place_in(&node, Proposal { width: Some(24.0), height: None }, &Proportional, ten());
    let pieces: Vec<(String, Px, Weight)> = result
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, origin, font, .. } => {
                Some((content[range.0..range.1].to_string(), origin.x, font.weight))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        pieces,
        [
            ("ab".to_string(), 0.0, Weight::Regular),
            ("c".to_string(), 10.0, Weight::Bold),
            ("…".to_string(), 16.25, Weight::Bold),
        ]
    );
}

// =============================================================================
// Grounds and rules
// =============================================================================

#[test]
fn an_underline_stands_half_its_thickness_below_the_baseline() {
    // house font: baseline at 13, a rule of a tenth of 13 pt (1.3), one
    // point below it
    let marks = TextMarks {
        decoration: Decoration { underline: Some(RuleInk::Text), ..Decoration::NONE },
        ..TextMarks::default()
    };
    let result = layout(&text("abc", Some(marks), None), Proposal::unspecified());
    assert_eq!(
        fills(&result),
        [(Rect { origin: at(0.0, 14.0), size: Size { width: 24.0, height: 1.3 } }, Color::BLACK)]
    );
}

#[test]
fn a_strikethrough_runs_a_third_of_the_ascent_above_the_baseline_in_its_own_colour() {
    let marks = TextMarks {
        decoration: Decoration { strikethrough: Some(RuleInk::Color(RED)), ..Decoration::NONE },
        ..TextMarks::default()
    };
    let result = place_in(&text("abcd", Some(marks), None), Proposal::unspecified(), &Proportional, ten());
    // baseline 7.5, ascent 7.5: the rule centred at 5, two points thick
    // (the engine's own: a fifth of the size)
    assert_eq!(fills(&result), [(Rect { origin: at(0.0, 4.0), size: Size { width: 20.0, height: 2.0 } }, RED)]);
}

#[test]
fn a_wrapped_lines_trailing_spaces_carry_no_rule() {
    let marks = TextMarks {
        decoration: Decoration { underline: Some(RuleInk::Text), ..Decoration::NONE },
        ..TextMarks::default()
    };
    let result = layout(&text("aa bb", Some(marks), None), Proposal { width: Some(24.0), height: None });
    let widths: Vec<Px> = fills(&result).into_iter().map(|(rect, _)| rect.size.width).collect();
    assert_eq!(widths, [16.0, 16.0], "\"aa \" rules its two letters, not its space");
}

#[test]
fn a_run_paints_its_ground_then_its_words_then_the_rule_through_them() {
    let struck = RunStyle {
        background: Some(SAGE),
        decoration: Decoration { strikethrough: Some(RuleInk::Text), ..Decoration::NONE },
        ..RunStyle::default()
    };
    let marks = TextMarks { runs: vec![TextRun { start: 1, end: 3, style: struck }], ..TextMarks::default() };
    let result = layout(&text("abcd", Some(marks), None), Proposal::unspecified());
    let order: Vec<&str> = result
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { color, .. } if *color == SAGE => Some("ground"),
            DrawCommand::FillRect { .. } => Some("rule"),
            DrawCommand::TextLine { .. } => Some("words"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["ground", "words", "words", "words", "rule"]);
    let ground = fills(&result)[0].0;
    assert_eq!(ground, Rect { origin: at(8.0, 0.0), size: Size { width: 16.0, height: 16.0 } }, "the run's own line box");
}

#[test]
fn a_run_rule_joins_the_paragraphs() {
    let under = RunStyle {
        decoration: Decoration { strikethrough: Some(RuleInk::Text), ..Decoration::NONE },
        ..RunStyle::default()
    };
    let marks = TextMarks {
        runs: vec![TextRun { start: 0, end: 2, style: under }],
        decoration: Decoration { underline: Some(RuleInk::Text), ..Decoration::NONE },
        ..TextMarks::default()
    };
    let result = layout(&text("abcd", Some(marks), None), Proposal::unspecified());
    let rules: Vec<(Px, Px)> = fills(&result).into_iter().map(|(rect, _)| (rect.origin.x, rect.size.width)).collect();
    // the two underlines of the two pieces, then the run's strikethrough
    assert_eq!(rules, [(0.0, 16.0), (16.0, 16.0), (0.0, 16.0)]);
}

// =============================================================================
// Raster
// =============================================================================

/// The ink of a box of the raster, black on white.
fn ink(bitmap: &crate::raster::Bitmap, x: std::ops::Range<usize>, y: std::ops::Range<usize>) -> u32 {
    y.flat_map(|row| x.clone().map(move |column| (column, row)))
        .filter(|&(column, row)| bitmap.pixel(column, row).is_some_and(|pixel| ((pixel >> 24) & 0xff) < 128))
        .count() as u32
}

#[test]
fn the_raster_draws_the_second_line_under_the_first() {
    let result = layout(&text("ab\ncd", None, None), Proposal::unspecified());
    let bitmap = crate::raster::rasterize(&result.display, 48, 40, Color::WHITE);
    assert!(ink(&bitmap, 0..16, 16..32) > 0, "\"cd\" stands on the second line, at the left");
    assert_eq!(ink(&bitmap, 16..48, 0..16), 0, "nothing past \"ab\" on the first: the break is no glyph");
}

#[test]
fn the_raster_draws_an_underline_as_a_whole_row() {
    let marks = TextMarks {
        decoration: Decoration { underline: Some(RuleInk::Text), ..Decoration::NONE },
        ..TextMarks::default()
    };
    let result = layout(&text("abc", Some(marks), None), Proposal::unspecified());
    let bitmap = crate::raster::rasterize(&result.display, 30, 20, Color::WHITE);
    assert_eq!(ink(&bitmap, 0..24, 14..15), 24, "the rule covers the words' width");
}
