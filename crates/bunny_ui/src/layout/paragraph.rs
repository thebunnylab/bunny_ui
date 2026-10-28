//! A text's lines: where its hard breaks and its soft wraps fall, how tall
//! each line stands, and how a paragraph of styled runs is measured,
//! broken and drawn as ONE paragraph.
//!
//! A design writes prose the way a browser reads it. A mail body or an
//! agent's answer keeps its own line breaks (`white-space: pre-wrap`), a
//! clause shows the words a change struck out beside the ones it put in,
//! a sentence sets one word bold and a path in the code face. Before
//! this, a `\n` was a glyph on one long line, and every styled word was a
//! view of its own in a wrapping row — each shaped alone, broken only
//! between views, and selected as separate texts.
//!
//! ## What this ships
//!
//! - [`TextMarks`], what a text paints beyond its inherited face: the
//!   [`TextRun`]s of a rich paragraph, the [`InkSpan`]s of a highlight
//!   (one colour per span — a syntax colouring is one record), and the
//!   [`Decoration`] the whole text wears.
//! - The measure and placement of every `text(…)`: a hard break always
//!   ends a line, the widest line is the natural width, and a paragraph
//!   of runs breaks through the same greedy walk a plain one does
//!   ([`crate::text_engine::break_lines_by`]), each run measured in its
//!   own face.
//!
//! ## Production gotchas
//!
//! - **A hard break belongs to no line.** The `\n` is neither painted nor
//!   measured: the line before it ends at the byte before it, the next
//!   one starts after it.
//! - **Runs shape apart.** A run boundary is a shaping boundary, as it is
//!   between inline elements of different fonts in a browser: kerning
//!   across it is lost. Runs that only change the ink, the ground or the
//!   rules keep the paragraph's single shaping — their pieces sit at the
//!   measured prefix of the line, like a highlight's.
//! - **A line of runs is as tall as its tallest run.** With no
//!   `.line_height(…)` a line's box is the largest ascent plus the
//!   largest descent of the faces on it, and every run stands on the one
//!   baseline. A `.line_height(…)` steps every line by itself, as a
//!   length-valued CSS `line-height` does.
//! - **Truncation keeps the runs only at the end.** `.truncation_mode(.end)`
//!   cuts a rich line where it no longer fits and closes it with an
//!   ellipsis in the face that was cut. A start or middle cut composes the
//!   words in the inherited face, like a highlight that does not survive
//!   the ellipsis.
//! - **Rules sit where Chrome's automatic ones do.** An underline stands
//!   one rule's half-thickness (at least a point) below the baseline, a
//!   strikethrough a third of the face's ascent above it; the thickness
//!   is the face's own when the engine reads it
//!   ([`crate::text_engine::TextEngine::decoration_thickness`]), else a
//!   tenth of the size. A line's trailing spaces carry no rule and no
//!   ground.

use super::*;
use crate::text_engine::{FontPatch, LineMeasure, LineMetrics, break_lines_by};

// =============================================================================
// The marks
// =============================================================================

/// What a text paints beyond its inherited face, in BYTE ranges of its
/// content: the runs of a rich paragraph, the inks of a highlight, and the
/// rules the whole text wears.
///
/// The three are kept apart because they answer to different modifiers
/// and touch different things. A run may change the face, so the
/// paragraph measures by its runs; an ink never does, so a highlight over
/// a rich paragraph changes colours and nothing else; a decoration is the
/// text's own, whatever its words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextMarks {
    /// The runs of a rich paragraph, in byte order and apart. Bytes no run
    /// covers wear the inherited face.
    pub runs: Vec<TextRun>,
    /// Inks laid over the words, in byte order — a finder's match, a
    /// syntax colouring. The measure stays intact by construction.
    pub inks: Vec<InkSpan>,
    /// The rules under or through the WHOLE text.
    pub decoration: Decoration,
}

impl TextMarks {
    /// Marks that are inks alone, ordered by where they start.
    pub fn of_inks(inks: impl IntoIterator<Item = InkSpan>) -> TextMarks {
        let mut inks: Vec<InkSpan> = inks.into_iter().filter(|ink| ink.start < ink.end).collect();
        inks.sort_by_key(|ink| ink.start);
        TextMarks { inks, ..TextMarks::default() }
    }

    /// Does a run change the face? Then the paragraph measures by its runs:
    /// a bold word is wider than the regular one it replaces.
    pub fn reshapes(&self) -> bool {
        self.runs.iter().any(|run| run.style.font != FontPatch::default())
    }

    /// Is there more to paint than inks over one shaping — a run, or a
    /// rule?
    pub(crate) fn styles(&self) -> bool {
        !self.runs.is_empty() || !self.decoration.is_none()
    }
}

/// One run of a rich paragraph: its bytes, and how they differ from the
/// paragraph around them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextRun {
    pub start: usize,
    pub end: usize,
    pub style: RunStyle,
}

/// How a run differs from the paragraph it sits in. Every slot left empty
/// is the paragraph's own: the face patch applies over the inherited font
/// field by field, the way `.bold()` applies over a `.font(…)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RunStyle {
    /// Over the inherited face — weight, slant, design, size, family.
    pub font: FontPatch,
    /// The ink, or the paragraph's.
    pub color: Option<Color>,
    /// A ground behind the run's line box — a `<mark>`, an inserted
    /// clause, a code span.
    pub background: Option<Color>,
    /// Rules of the run's own, over the paragraph's.
    pub decoration: Decoration,
}

/// One coloured stretch of a highlight: BYTE range + ink.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkSpan {
    pub start: usize,
    pub end: usize,
    pub color: Color,
}

/// The rules a text draws along its words — under them, through them, or
/// both. `None` on a slot is no rule.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Decoration {
    pub underline: Option<RuleInk>,
    pub strikethrough: Option<RuleInk>,
}

impl Decoration {
    /// No rule at all.
    pub const NONE: Decoration = Decoration { underline: None, strikethrough: None };

    pub fn is_none(&self) -> bool {
        self.underline.is_none() && self.strikethrough.is_none()
    }

    /// Rule by rule, the closer one wins — a run's strikethrough over a
    /// paragraph's underline keeps both.
    pub fn or(self, outer: Decoration) -> Decoration {
        Decoration {
            underline: self.underline.or(outer.underline),
            strikethrough: self.strikethrough.or(outer.strikethrough),
        }
    }
}

/// The ink of a rule: the words' own, or a colour of its own
/// (`text-decoration-color`) — the struck clause of a diff draws its
/// rule in a colour the words do not wear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RuleInk {
    Text,
    Color(Color),
}

impl From<Option<Color>> for RuleInk {
    fn from(color: Option<Color>) -> RuleInk {
        color.map_or(RuleInk::Text, RuleInk::Color)
    }
}

// =============================================================================
// Measuring by runs
// =============================================================================

/// A paragraph of runs, measured run by run — what the breaker asks when a
/// run changes the face.
struct ByRuns<'a, 'e> {
    content: &'a str,
    marks: &'a TextMarks,
    env: &'a LayoutEnv<'e>,
}

impl LineMeasure for ByRuns<'_, '_> {
    fn width(&self, start: usize, end: usize) -> Px {
        run_segments(self.content, self.marks, (start, end))
            .map(|segment| self.metrics(segment).width)
            .sum()
    }
}

impl ByRuns<'_, '_> {
    fn metrics(&self, segment: Segment<'_>) -> LineMetrics {
        let font = face(segment.style, self.env);
        self.env.cache.get_or_measure(&self.content[segment.start..segment.end], &font, self.env.text)
    }

    /// The tallest ascent and descent of the faces on a line. An empty
    /// line — a blank paragraph — stands in the face its first byte would
    /// wear.
    fn line_box(&self, line: (usize, usize)) -> (Px, Px) {
        let mut tallest: Option<(Px, Px)> = None;
        for segment in run_segments(self.content, self.marks, line) {
            let metrics = self.metrics(segment);
            let (ascent, descent) = tallest.unwrap_or((0.0, 0.0));
            tallest = Some((ascent.max(metrics.ascent), descent.max(metrics.descent)));
        }
        tallest.unwrap_or_else(|| {
            let style = self.marks.runs.iter().find(|run| run.start <= line.0 && line.0 < run.end);
            let font = face(style.map(|run| &run.style), self.env);
            let metrics = self.env.cache.get_or_measure("0", &font, self.env.text);
            (metrics.ascent, metrics.descent)
        })
    }
}

/// The face a run wears: its patch over the inherited font.
fn face(style: Option<&RunStyle>, env: &LayoutEnv<'_>) -> FontSpec {
    style.map_or(env.font, |style| style.font.apply_over(env.font))
}

/// A stretch of a line in ONE run (or in none): the unit that is shaped.
#[derive(Clone, Copy)]
struct Segment<'m> {
    start: usize,
    end: usize,
    style: Option<&'m RunStyle>,
}

/// The line cut at its run boundaries, in byte order. Bytes outside every
/// run come out as segments with no style.
fn run_segments<'m>(
    content: &str,
    marks: &'m TextMarks,
    line: (usize, usize),
) -> impl Iterator<Item = Segment<'m>> {
    let clamp = move |index: usize| crate::text_input::clamp_index(content, index).clamp(line.0, line.1);
    let mut segments = Vec::new();
    let mut cursor = line.0;
    for run in &marks.runs {
        let (start, end) = (clamp(run.start).max(cursor), clamp(run.end));
        if end <= start {
            continue;
        }
        if start > cursor {
            segments.push(Segment { start: cursor, end: start, style: None });
        }
        segments.push(Segment { start, end, style: Some(&run.style) });
        cursor = end;
    }
    if cursor < line.1 {
        segments.push(Segment { start: cursor, end: line.1, style: None });
    }
    segments.into_iter()
}

// =============================================================================
// The shape of a text
// =============================================================================

/// How one text is measured: in one face, or run by run. Built once per
/// measure or placement — it decides which of the two roads the lines take
/// and holds what both sides need.
struct Shape<'a, 'e> {
    content: &'a str,
    env: &'a LayoutEnv<'e>,
    /// Present when a run changes the face.
    runs: Option<ByRuns<'a, 'e>>,
    /// The hard lines, `\n` excluded — one, for a text with no break.
    hard: Rc<Vec<(usize, usize)>>,
    /// The widest hard line: what the text asks for when nothing narrows it.
    natural: Px,
    /// The face's box, for a text in one face.
    face: LineMetrics,
}

impl<'a, 'e> Shape<'a, 'e> {
    fn of(content: &'a str, marks: Option<&'a TextMarks>, env: &'a LayoutEnv<'e>) -> Self {
        let runs = marks.filter(|marks| marks.reshapes()).map(|marks| ByRuns { content, marks, env });
        let breaks = content.contains('\n');
        let hard: Rc<Vec<(usize, usize)>> = Rc::new(if breaks {
            let mut start = 0;
            content
                .split('\n')
                .map(|paragraph| {
                    let line = (start, start + paragraph.len());
                    start = line.1 + 1;
                    line
                })
                .collect()
        } else {
            vec![(0, content.len())]
        });
        // the face's own box — measured on the first line, never across a
        // break the engine would draw as a glyph
        let face = env.cache.get_or_measure(&content[..hard[0].1], &env.font, env.text);
        let natural = match &runs {
            Some(runs) => hard.iter().map(|&(start, end)| runs.width(start, end)).fold(0.0, Px::max),
            None if breaks => hard
                .iter()
                .map(|&(start, end)| env.cache.get_or_measure(&content[start..end], &env.font, env.text).width)
                .fold(0.0, Px::max),
            None => face.width,
        };
        Shape { content, env, runs, hard, natural, face }
    }

    /// The width of one line, in the faces it is drawn in.
    fn width(&self, line: (usize, usize)) -> Px {
        match &self.runs {
            Some(runs) => runs.width(line.0, line.1),
            None => self.env.cache.get_or_measure(&self.content[line.0..line.1], &self.env.font, self.env.text).width,
        }
    }

    /// The lines at `width`: soft wraps inside every hard line.
    fn wrapped(&self, width: Px) -> Rc<Vec<(usize, usize)>> {
        match &self.runs {
            Some(runs) => Rc::new(break_lines_by(self.content, width, runs)),
            None => self.env.cache.get_or_break(self.content, &self.env.font, width, self.env.text),
        }
    }

    /// The lines a box this wide shows: the hard ones when they fit or a
    /// truncation keeps each to one line, else the wrapped ones.
    fn lines(&self, width: Option<Px>, truncation: Option<Truncation>) -> (Rc<Vec<(usize, usize)>>, bool) {
        match width {
            Some(width) if width > 0.0 && width < self.natural => match truncation {
                Some(_) => (self.hard.clone(), true),
                None => (self.wrapped(width), true),
            },
            _ => (self.hard.clone(), false),
        }
    }

    /// One line's box: its ascent, its descent, and the step to the next.
    fn line_box(&self, line: (usize, usize)) -> LineBox {
        let (ascent, descent) = match &self.runs {
            Some(runs) => runs.line_box(line),
            None => (self.face.ascent, self.face.descent),
        };
        LineBox { ascent, descent, advance: self.env.line_height.unwrap_or(ascent + descent) }
    }

    /// How many of `lines` a box `room` tall shows — at least one, the way
    /// a paragraph told how tall it may be answers that and no more.
    fn rows_that_fit(&self, lines: &[(usize, usize)], room: Option<Px>) -> usize {
        if self.runs.is_none() {
            return rows_that_fit(lines.len(), self.line_box((0, 0)).advance, room);
        }
        let Some(room) = room.filter(|room| *room > 0.0) else {
            return lines.len();
        };
        let mut height = 0.0;
        let mut shown = 0;
        for &line in lines {
            height += self.line_box(line).advance;
            if shown > 0 && height > room + SETTLED {
                break;
            }
            shown += 1;
        }
        shown
    }

    /// The height of the first `shown` lines.
    fn height(&self, lines: &[(usize, usize)], shown: usize) -> Px {
        match &self.runs {
            Some(_) => lines.iter().take(shown).map(|&line| self.line_box(line).advance).sum(),
            None => shown as Px * self.line_box((0, 0)).advance,
        }
    }
}

/// A line's box: the faces' tallest ascent and descent, and the step the
/// paragraph takes past it.
#[derive(Clone, Copy)]
struct LineBox {
    ascent: Px,
    descent: Px,
    advance: Px,
}

impl LineBox {
    /// The half-leading that centres the glyph box inside a taller line —
    /// the baseline stays ascent-based. Zero with no `.line_height(…)`.
    fn leading(&self) -> Px {
        (self.advance - (self.ascent + self.descent)) / 2.0
    }
}

/// What a text answers a proposal: its widest line when nothing narrows
/// it, else the proposed width, and the height of the lines that fit.
///
/// A paragraph told how tall it may be answers that, not what it wishes it
/// were. Answering the wish is what put a card's text over the card: the
/// parent centres what it was told, so an answer bigger than the room
/// leaks HALF of itself upward, into the sibling above. Proposed nothing
/// on this axis, it still answers everything — which is what a scrolling
/// region asks for, and how a column of text keeps growing.
pub(super) fn measure_text(
    content: &str,
    marks: Option<&TextMarks>,
    truncation: Option<Truncation>,
    proposal: Proposal,
    env: &LayoutEnv<'_>,
) -> Size {
    let shape = Shape::of(content, marks, env);
    let (lines, narrowed) = shape.lines(proposal.width, truncation);
    let shown = shape.rows_that_fit(&lines, proposal.height);
    Size {
        width: match (narrowed, proposal.width) {
            (true, Some(width)) => width,
            _ => shape.natural,
        },
        height: shape.height(&lines, shown),
    }
}

/// The first baseline of a text, from its top: its first line's ascent.
pub(super) fn first_baseline(content: &str, marks: Option<&TextMarks>, env: &LayoutEnv<'_>) -> Px {
    let shape = Shape::of(content, marks, env);
    shape.line_box(shape.hard[0]).ascent
}

// =============================================================================
// Placing a text
// =============================================================================

/// Paints a placed text: its lines, hard and wrapped, each one whole,
/// truncated with an ellipsis, or the paragraph's last visible line cut
/// short — always through the SAME caches as measurement.
pub(super) fn place_text(
    content: &Arc<str>,
    marks: Option<&TextMarks>,
    truncation: Option<Truncation>,
    frame: Rect,
    base_color: Color,
    env: &LayoutEnv<'_>,
    out: &mut Placement,
) {
    if content.is_empty() {
        return;
    }
    // the rows of a long list that are off the glass neither measure nor
    // break their lines: the band is the one every line below is drawn in,
    // with the reach a line of text is given everywhere (three sizes down,
    // and one up for a line height tighter than the face)
    if out.hides_band(
        frame.origin.y - env.font.size,
        frame.origin.y + frame.size.height + env.font.size * 3.0,
    ) {
        return;
    }
    let shape = Shape::of(content, marks, env);
    let (lines, _) = shape.lines(Some(frame.size.width), truncation);
    // the same rule the measure used, so the two never disagree about
    // which line is the last one
    let shown = shape.rows_that_fit(&lines, Some(frame.size.height));
    let styled = marks.filter(|marks| marks.styles());
    let line = LinePen { content, marks, styled, frame, base_color, env };
    let mut top = frame.origin.y;
    for (index, &(start, end)) in lines.iter().enumerate().take(shown) {
        let line_box = shape.line_box((start, end));
        let baseline = top + line_box.leading() + line_box.ascent;
        if index + 1 == shown && shown < lines.len() {
            // the last line of a paragraph that was CUT says so, the way a
            // single line that does not fit already does — with the rest
            // of ITS paragraph, never the words past a hard break
            let stop = content[end..].find('\n').map_or(content.len(), |at| end + at);
            line.truncated(&shape, (start, stop), Truncation::End, true, baseline, out);
            break;
        }
        match truncation {
            Some(mode) if shape.width((start, end)) > frame.size.width => {
                line.truncated(&shape, (start, end), mode, false, baseline, out);
            }
            _ => line.whole(&shape, (start, end), baseline, out),
        }
        top += line_box.advance;
    }
}

/// What every line of one placed text shares.
struct LinePen<'a, 'e> {
    content: &'a Arc<str>,
    marks: Option<&'a TextMarks>,
    /// The marks, when they paint more than inks over one shaping.
    styled: Option<&'a TextMarks>,
    frame: Rect,
    base_color: Color,
    env: &'a LayoutEnv<'e>,
}

impl LinePen<'_, '_> {
    /// Where a line `width` wide starts: leading is offset zero, which is
    /// where every line has always been placed — and the whole room in a
    /// right-to-left scene, where the leading edge is the right one (a line
    /// wider than its box keeps its END on that edge and its start spills
    /// left, trailing). The origin stays the drawn run's LEFT edge either
    /// way: that is what every reader of a `TextLine` counts from.
    fn slide(&self, width: Px) -> Px {
        let room = self.frame.size.width - width;
        let left = match (self.env.text_align, self.env.rtl()) {
            (None | Some(motor::views::TextAlignment::Leading), false) => 0.0,
            (None | Some(motor::views::TextAlignment::Leading), true) => room,
            (Some(motor::views::TextAlignment::Center), _) => room / 2.0,
            (Some(motor::views::TextAlignment::Trailing), false) => room,
            (Some(motor::views::TextAlignment::Trailing), true) => 0.0,
        };
        self.frame.origin.x + left
    }

    /// Does a line start at the frame's left edge whatever its width? Then
    /// it never asks for its width — the common case measures nothing extra.
    fn starts_at_left(&self) -> bool {
        matches!(
            (self.env.text_align, self.env.rtl()),
            (None | Some(motor::views::TextAlignment::Leading), false)
                | (Some(motor::views::TextAlignment::Trailing), true)
        )
    }

    /// One line drawn whole. A blank one paints nothing.
    fn whole(&self, shape: &Shape<'_, '_>, line: (usize, usize), baseline: Px, out: &mut Placement) {
        if line.0 >= line.1 {
            return;
        }
        let x = if self.starts_at_left() {
            self.frame.origin.x
        } else {
            // the space a break swallowed is INVISIBLE, and counting it
            // would slide the line half a space off centre — the browser
            // trims it too, so the two lowerings agree
            let trimmed = (line.0, line.0 + self.content[line.0..line.1].trim_end().len());
            self.slide(shape.width(trimmed))
        };
        match self.styled {
            Some(marks) => emit_styled_line(self, shape, marks, line, None, x, baseline, out),
            None => emit_text_runs(
                self.content,
                line,
                self.marks,
                Point { x, y: baseline - shape.face.ascent },
                self.base_color,
                self.env,
                out,
            ),
        }
    }

    /// One line cut to the frame with an ellipsis. `forced`: more words
    /// follow it, so the ellipsis stands even where the line itself fits.
    fn truncated(
        &self,
        shape: &Shape<'_, '_>,
        line: (usize, usize),
        mode: Truncation,
        forced: bool,
        baseline: Px,
        out: &mut Placement,
    ) {
        let width = self.frame.size.width;
        let rest = &self.content[line.0..line.1];
        let trimmed = (line.0, line.0 + rest.trim_end().len());
        if let (Some(marks), Truncation::End) = (self.styled, mode) {
            // the runs survive an end cut: the longest prefix that fits
            // beside an ellipsis in the face that was cut
            let ellipsis = |at: usize| {
                let style = marks.runs.iter().find(|run| run.start <= at && at < run.end);
                let font = face(style.map(|run| &run.style), self.env);
                self.env.cache.get_or_measure(ELLIPSIS, &font, self.env.text).width
            };
            let whole = shape.width(trimmed);
            if whole <= width && !forced {
                emit_styled_line(self, shape, marks, trimmed, None, self.slide(whole), baseline, out);
                return;
            }
            let mut cut = line.0;
            if whole + ellipsis(trimmed.1) <= width {
                cut = trimmed.1;
            } else {
                for (offset, _) in rest.char_indices().skip(1) {
                    let at = line.0 + offset;
                    if shape.width((line.0, at)) + ellipsis(at) > width {
                        break;
                    }
                    cut = at;
                }
            }
            let drawn = shape.width((line.0, cut)) + ellipsis(cut);
            emit_styled_line(self, shape, marks, (line.0, cut), Some(cut), self.slide(drawn), baseline, out);
            return;
        }
        // one face — or a start or middle cut, which composes the words in
        // the inherited face: the original's ranges do not map onto the
        // composed text, so neither inks nor runs survive it
        let fits = |candidate: &str| {
            self.env.cache.get_or_measure(candidate, &self.env.font, self.env.text).width <= width
        };
        let rest = &self.content[trimmed.0..trimmed.1];
        let closed = format!("{rest}{ELLIPSIS}");
        let composed: Arc<str> = match (forced, fits(rest)) {
            (false, true) => Arc::from(rest),
            (true, _) if fits(&closed) => Arc::from(closed.as_str()),
            _ => Arc::from(truncate_to_width(rest, mode, width, self.env)),
        };
        let length = composed.len();
        let cut = self.env.cache.get_or_measure(&composed, &self.env.font, self.env.text).width;
        let x = self.slide(cut);
        out.draw(DrawCommand::TextLine {
            origin: Point { x, y: baseline - shape.face.ascent },
            content: composed,
            range: (0, length),
            color: self.base_color,
            font: self.env.font,
        });
        if let Some(marks) = self.styled {
            let rules = Rules { decoration: marks.decoration, color: self.base_color, font: self.env.font };
            rules.draw_under(x, cut, baseline, self.env, out);
            rules.draw_through(x, cut, baseline, shape.face.ascent, self.env, out);
        }
    }
}

/// The rules one stretch of a line draws, in its ink and its face.
struct Rules {
    decoration: Decoration,
    color: Color,
    font: FontSpec,
}

impl Rules {
    fn thickness(&self, env: &LayoutEnv<'_>) -> Px {
        env.text.decoration_thickness(&self.font).unwrap_or(self.font.size / 10.0).max(1.0)
    }

    fn ink(&self, rule: RuleInk) -> Color {
        match rule {
            RuleInk::Text => self.color,
            RuleInk::Color(color) => color,
        }
    }

    /// The underline: one half-thickness (at least a point) below the
    /// baseline, where Chrome's automatic underline stands.
    fn draw_under(&self, x: Px, width: Px, baseline: Px, env: &LayoutEnv<'_>, out: &mut Placement) {
        let Some(rule) = self.decoration.underline.filter(|_| width > 0.0) else { return };
        let thickness = self.thickness(env);
        let gap = (thickness / 2.0).ceil().max(1.0);
        out.draw(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x, y: baseline + gap },
                size: Size { width, height: thickness },
            },
            color: self.ink(rule),
            corner_radius: Corners::ZERO,
        });
    }

    /// The strikethrough: centred a third of the face's ascent above the
    /// baseline, where Chrome strikes a line through.
    fn draw_through(&self, x: Px, width: Px, baseline: Px, ascent: Px, env: &LayoutEnv<'_>, out: &mut Placement) {
        let Some(rule) = self.decoration.strikethrough.filter(|_| width > 0.0) else { return };
        let thickness = self.thickness(env);
        out.draw(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x, y: baseline - ascent / 3.0 - thickness / 2.0 },
                size: Size { width, height: thickness },
            },
            color: self.ink(rule),
            corner_radius: Corners::ZERO,
        });
    }
}

/// One run of a line, placed: its stretch, its face, where it starts and
/// how wide it is drawn.
struct Placed<'m> {
    segment: Segment<'m>,
    font: FontSpec,
    metrics: LineMetrics,
    x: Px,
    width: Px,
}

/// One line of a styled text, run by run: the grounds first, then the
/// underlines, the words (split again by the inks), and the strikethroughs
/// over them — the order a browser paints an inline's decorations in.
/// `ellipsis` closes a cut line at that byte, in the face that was cut.
#[allow(clippy::too_many_arguments)]
fn emit_styled_line(
    pen: &LinePen<'_, '_>,
    shape: &Shape<'_, '_>,
    marks: &TextMarks,
    line: (usize, usize),
    ellipsis: Option<usize>,
    x: Px,
    baseline: Px,
    out: &mut Placement,
) {
    let (content, env) = (pen.content, pen.env);
    let reshapes = shape.runs.is_some();
    let mut placed: Vec<Placed<'_>> = Vec::new();
    let mut cursor = x;
    for segment in run_segments(content, marks, line) {
        let font = face(segment.style, env);
        let metrics = env.cache.get_or_measure(&content[segment.start..segment.end], &font, env.text);
        // one shaping: a piece sits at the measured prefix of the line, the
        // way a highlight's does; runs in other faces add up
        let at = if reshapes || segment.start == line.0 {
            cursor
        } else {
            x + env.cache.get_or_measure(&content[line.0..segment.start], &font, env.text).width
        };
        placed.push(Placed { segment, font, metrics, x: at, width: metrics.width });
        cursor = at + metrics.width;
    }
    // a right-to-left line is drawn in visual order: its logical start is
    // its right end. Pure runs exact; a mixed line follows its first word,
    // like every reader of it here
    if crate::text_input::reads_right_to_left(&content[line.0..line.1]) {
        let total = cursor - x;
        for piece in &mut placed {
            piece.x = x + total - (piece.x - x) - piece.width;
        }
    }
    // the trailing spaces of a wrapped line hang past its end: no ground
    // and no rule there
    let visible = line.0 + content[line.0..line.1].trim_end_matches(' ').len();
    let inked = |piece: &Placed<'_>| -> Px {
        if piece.segment.end <= visible {
            piece.width
        } else if piece.segment.start >= visible {
            0.0
        } else {
            env.cache.get_or_measure(&content[piece.segment.start..visible], &piece.font, env.text).width
        }
    };
    let ink_of = |piece: &Placed<'_>| piece.segment.style.and_then(|style| style.color).unwrap_or(pen.base_color);
    let rules_of = |piece: &Placed<'_>| Rules {
        decoration: piece.segment.style.map_or(Decoration::NONE, |style| style.decoration).or(marks.decoration),
        color: ink_of(piece),
        font: piece.font,
    };
    for piece in &placed {
        if let Some(ground) = piece.segment.style.and_then(|style| style.background) {
            out.draw(DrawCommand::FillRect {
                rect: Rect {
                    origin: Point { x: piece.x, y: baseline - piece.metrics.ascent },
                    size: Size { width: inked(piece), height: piece.metrics.height() },
                },
                color: ground,
                corner_radius: Corners::ZERO,
            });
        }
    }
    for piece in &placed {
        rules_of(piece).draw_under(piece.x, inked(piece), baseline, env, out);
    }
    for piece in &placed {
        let top = baseline - piece.metrics.ascent;
        let color = ink_of(piece);
        let (start, end) = (piece.segment.start, piece.segment.end);
        // the inks split a run again; each piece at its measured prefix
        // inside the run, so the run keeps its one shaping
        let mut cursor = start;
        let mut paint = |from: usize, to: usize, color: Color| {
            if to <= from {
                return;
            }
            let offset = if from == start {
                0.0
            } else {
                env.cache.get_or_measure(&content[start..from], &piece.font, env.text).width
            };
            out.draw(DrawCommand::TextLine {
                origin: Point { x: piece.x + offset, y: top },
                content: content.clone(),
                range: (from, to),
                color,
                font: piece.font,
            });
        };
        for ink in &marks.inks {
            let from = crate::text_input::clamp_index(content, ink.start).clamp(cursor, end);
            let to = crate::text_input::clamp_index(content, ink.end).clamp(from, end);
            if to <= from {
                continue;
            }
            paint(cursor, from, color);
            paint(from, to, ink.color);
            cursor = to;
        }
        paint(cursor, end, color);
    }
    for piece in &placed {
        rules_of(piece).draw_through(piece.x, inked(piece), baseline, piece.metrics.ascent, env, out);
    }
    if let Some(at) = ellipsis {
        let style = marks.runs.iter().find(|run| run.start <= at && at < run.end).map(|run| &run.style);
        let font = face(style, env);
        let ascent = env.cache.get_or_measure(ELLIPSIS, &font, env.text).ascent;
        out.draw(DrawCommand::TextLine {
            origin: Point { x: cursor, y: baseline - ascent },
            content: Arc::from(ELLIPSIS),
            range: (0, ELLIPSIS.len()),
            color: style.and_then(|style| style.color).unwrap_or(pen.base_color),
            font,
        });
    }
}

/// Emits the `TextLine`s of ONE line in one face: whole in the base color,
/// or sliced into segments by the inks — each at its measured prefix
/// position (kerning between segments is approximate, the runs above shape
/// apart for the same reason).
pub(super) fn emit_text_runs(
    content: &Arc<str>,
    line: (usize, usize),
    marks: Option<&TextMarks>,
    origin: Point,
    base_color: Color,
    env: &LayoutEnv<'_>,
    out: &mut Placement,
) {
    let (line_start, line_end) = line;
    // a blank line — a paragraph between two breaks — paints nothing
    if line_start >= line_end {
        return;
    }
    let whole = || DrawCommand::TextLine {
        origin,
        content: content.clone(),
        range: (line_start, line_end),
        color: base_color,
        font: env.font,
    };
    let Some(marks) = marks.filter(|marks| !marks.inks.is_empty()) else {
        out.draw(whole());
        return;
    };

    // segments covering the whole line, alternating base and inked
    let clamp = |index: usize| crate::text_input::clamp_index(content, index);
    let mut segments: Vec<(usize, usize, Color)> = Vec::new();
    let mut cursor = line_start;
    for ink in &marks.inks {
        let start = clamp(ink.start).clamp(cursor, line_end);
        let end = clamp(ink.end).min(line_end);
        if end <= start {
            continue;
        }
        if start > cursor {
            segments.push((cursor, start, base_color));
        }
        segments.push((start, end, ink.color));
        cursor = end;
    }
    if cursor < line_end {
        segments.push((cursor, line_end, base_color));
    }
    if let [(_, _, color)] = segments.as_slice()
        && *color == base_color
    {
        out.draw(whole());
        return;
    }

    // a right-to-left line is drawn in visual order: its logical start
    // is its right end, so a segment sits as far from the right as the
    // text through its END measures. Pure runs exact; a mixed line
    // follows its first word, like every reader of it here
    let rtl = crate::text_input::reads_right_to_left(&content[line_start..line_end]);
    let line_width = if rtl {
        env.cache.get_or_measure(&content[line_start..line_end], &env.font, env.text).width
    } else {
        0.0
    };
    for (start, end, color) in segments {
        let x = if rtl {
            let through = env.cache.get_or_measure(&content[line_start..end], &env.font, env.text).width;
            origin.x + line_width - through
        } else if start == line_start {
            origin.x
        } else {
            origin.x + env.cache.get_or_measure(&content[line_start..start], &env.font, env.text).width
        };
        out.draw(DrawCommand::TextLine {
            origin: Point { x, y: origin.y },
            content: content.clone(),
            range: (start, end),
            color,
            font: env.font,
        });
    }
}

#[cfg(test)]
#[path = "paragraph_tests.rs"]
mod tests;
