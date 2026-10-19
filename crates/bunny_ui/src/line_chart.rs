//! Reusable line plots keep sample geometry and inspection out of applications.
//!
//! ## Wiring
//! Validate [`LineSample`] values and a [`LineSeries`] snapshot when data changes,
//! then give [`line_chart`] a live reader. The application owns its history and
//! clock. The control uses the existing canvas, input and retained-paint paths.
//!
//! ## Production gotchas
//! Requires `canvas` (a canvas island in element-mode web). Provide a visible
//! label and a text/table data alternative: canvas curves and tooltip text are
//! not an assistive-technology chart tree. Native screen-reader qualification
//! requires separate evidence. Inspection reports stable IDs, not array offsets;
//! resolve them against the latest snapshot with [`LineSeries::sample`].

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::sync::Arc;

use motor::state::Binding;

use crate::action::Key;
use crate::custom::{CustomElement, ElementEvent, EventCtx, Metrics, PaintCtx, Painter, Response};
use crate::icon::{Paint, Verb};
use crate::layout::{Axis, Color, Point, Proposal, Rect, Size};
use crate::view::{Component, View};

// =============================================================================
// Validated data snapshots
// =============================================================================

/// Why a plot's data or explicit domain cannot be represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartError {
    /// A sample coordinate or domain endpoint is NaN or infinite.
    NonFinite,
    /// An identity occurs more than once in the same snapshot.
    DuplicateId(u64),
    /// Samples are not in strictly increasing x order.
    NotIncreasingX,
    /// Subtracting the data or domain endpoints overflows.
    UnrepresentableSpan,
    /// An explicit domain has equal or reversed endpoints.
    NotIncreasingDomain,
    /// An explicit y domain excludes at least one sample.
    OutsideDomain,
}
impl std::fmt::Display for ChartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite => f.write_str("chart coordinates must be finite"),
            Self::DuplicateId(id) => write!(f, "duplicate chart sample ID {id}"),
            Self::NotIncreasingX => f.write_str("chart x coordinates must strictly increase"),
            Self::UnrepresentableSpan => f.write_str("chart span cannot be represented"),
            Self::NotIncreasingDomain => f.write_str("chart domain must strictly increase"),
            Self::OutsideDomain => f.write_str("chart sample is outside the explicit y domain"),
        }
    }
}
impl std::error::Error for ChartError {}

/// A finite sample with an application-owned identity stable across snapshots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineSample {
    id: u64,
    x: f64,
    value: f64,
}
impl LineSample {
    /// Constructs a sample. An ID is independent of its position or x value.
    ///
    /// # Errors
    /// Rejects nonfinite x or value coordinates.
    pub const fn new(id: u64, x: f64, value: f64) -> Result<Self, ChartError> {
        if !x.is_finite() || !value.is_finite() {
            return Err(ChartError::NonFinite);
        }
        Ok(Self { id, x, value })
    }
    /// Stable application identity; IDs need not be ordered numerically.
    #[must_use]
    pub const fn id(self) -> u64 {
        self.id
    }
    /// Horizontal coordinate (for example elapsed seconds).
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }
    /// Vertical coordinate in the application's units.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }
}

/// A finite, strictly increasing, representable explicit numeric domain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartDomain {
    lower: f64,
    upper: f64,
}
impl ChartDomain {
    /// Constructs inclusive bounds without silently clipping data.
    ///
    /// # Errors
    /// Rejects nonfinite, equal, reversed or overflowing bounds.
    pub fn new(bounds: RangeInclusive<f64>) -> Result<Self, ChartError> {
        let (lower, upper) = bounds.into_inner();
        if !lower.is_finite() || !upper.is_finite() {
            return Err(ChartError::NonFinite);
        }
        if lower >= upper {
            return Err(ChartError::NotIncreasingDomain);
        }
        if !(upper - lower).is_finite() {
            return Err(ChartError::UnrepresentableSpan);
        }
        Ok(Self { lower, upper })
    }
    /// Inclusive lower endpoint.
    #[must_use]
    pub const fn lower(self) -> f64 {
        self.lower
    }
    /// Inclusive upper endpoint.
    #[must_use]
    pub const fn upper(self) -> f64 {
        self.upper
    }
}

#[derive(Debug, PartialEq)]
struct Samples {
    points: Vec<LineSample>,
    indices: BTreeMap<u64, usize>,
    y: (f64, f64),
}

/// An immutable validated snapshot; cloning shares its points and ID index.
///
/// Construction is O(n log n), painting is O(n), and nearest/ID inspection is
/// O(log n). Construct once per data update, outside paint. No samples are
/// dropped or decimated; the application chooses its visible history window.
#[derive(Clone, Debug, PartialEq)]
pub struct LineSeries {
    samples: Arc<Samples>,
    y_domain: Option<ChartDomain>,
}
impl LineSeries {
    /// Collects points in strictly increasing x order. Empty data is valid.
    ///
    /// # Errors
    /// Rejects duplicate IDs, equal/decreasing x and overflowing x/y spans.
    pub fn new(points: impl IntoIterator<Item = LineSample>) -> Result<Self, ChartError> {
        let points: Vec<_> = points.into_iter().collect();
        let mut indices = BTreeMap::new();
        let mut previous = None;
        let mut y = points.first().map_or((0.0, 0.0), |p| (p.value, p.value));
        for (index, point) in points.iter().enumerate() {
            if previous.is_some_and(|x| x >= point.x) {
                return Err(ChartError::NotIncreasingX);
            }
            if indices.insert(point.id, index).is_some() {
                return Err(ChartError::DuplicateId(point.id));
            }
            previous = Some(point.x);
            y = (y.0.min(point.value), y.1.max(point.value));
        }
        if !(y.1 - y.0).is_finite()
            || points
                .first()
                .zip(points.last())
                .is_some_and(|(a, b)| !(b.x - a.x).is_finite())
        {
            return Err(ChartError::UnrepresentableSpan);
        }
        Ok(Self {
            samples: Arc::new(Samples { points, indices, y }),
            y_domain: None,
        })
    }
    /// Uses a fixed vertical domain instead of fitting the snapshot's data.
    ///
    /// # Errors
    /// Rejects any sample outside the domain; no silent clipping or saturation.
    pub fn with_y_domain(mut self, domain: ChartDomain) -> Result<Self, ChartError> {
        if !self.samples.points.is_empty()
            && (self.samples.y.0 < domain.lower || self.samples.y.1 > domain.upper)
        {
            return Err(ChartError::OutsideDomain);
        }
        self.y_domain = Some(domain);
        Ok(self)
    }
    /// Points in increasing x order, without allocation.
    #[must_use]
    pub fn samples(&self) -> &[LineSample] {
        &self.samples.points
    }
    /// Resolves an ID in the current snapshot, including after rolling updates.
    #[must_use]
    pub fn sample(&self, id: u64) -> Option<LineSample> {
        self.samples
            .indices
            .get(&id)
            .map(|&i| self.samples.points[i])
    }
    fn x_fraction(&self, sample: LineSample) -> f64 {
        self.samples
            .points
            .first()
            .zip(self.samples.points.last())
            .map_or(0.5, |(a, b)| fraction(sample.x, a.x, b.x))
    }
    fn y_fraction(&self, sample: LineSample) -> f64 {
        let (lower, upper) = self.y_domain.map_or(self.samples.y, |d| (d.lower, d.upper));
        fraction(sample.value, lower, upper)
    }
    fn nearest(&self, fraction: f64) -> Option<LineSample> {
        let points = &self.samples.points;
        let after = points.partition_point(|p| self.x_fraction(*p) < fraction);
        match (
            after.checked_sub(1).and_then(|i| points.get(i)),
            points.get(after),
        ) {
            (Some(before), Some(after)) => Some(
                if fraction - self.x_fraction(*before) <= self.x_fraction(*after) - fraction {
                    *before
                } else {
                    *after
                },
            ),
            (Some(point), None) | (None, Some(point)) => Some(*point),
            (None, None) => None,
        }
    }
}
fn fraction(value: f64, lower: f64, upper: f64) -> f64 {
    if lower >= upper {
        0.5
    } else {
        ((value - lower) / (upper - lower)).clamp(0.0, 1.0)
    }
}

// =============================================================================
// Public control
// =============================================================================

#[derive(Clone, Copy, Default)]
struct InspectionInput {
    pointer: Option<Point>,
    muted: bool,
}

#[derive(Clone)]
struct Inspection {
    selected: Binding<Option<u64>>,
    tooltip: Rc<dyn Fn(LineSample) -> String>,
}
impl Inspection {
    fn select(&self, id: Option<u64>) {
        if self.selected.wrappedValue() != id {
            self.selected.set(id);
        }
    }
}

/// A live line plot using the framework's canvas and interaction machinery.
///
/// The reader must return a validated snapshot, without mutating application
/// state. Default plots include horizontal grid lines; [`Self::sparkline`]
/// chooses compact presentation. X increases toward the trailing edge in RTL.
#[derive(Clone)]
pub struct LineChart {
    series: Rc<dyn Fn() -> LineSeries>,
    compact: bool,
    color: Option<Color>,
    grid_color: Option<Color>,
    inspection: Option<Inspection>,
    input: Rc<Cell<InspectionInput>>,
}
/// Plots a live validated snapshot. Pair it with a visible label/data alternative.
///
/// ```
/// use bunny_ui_core::prelude::*;
/// let samples = LineSeries::new([
///     LineSample::new(7, 0.0, 42.0)?,
///     LineSample::new(8, 0.1, 43.0)?,
/// ])?;
/// let history = State::new(samples);
/// let selected = State::new(None);
/// let chart = vstack!(
///     text("Metric history"),
///     line_chart(move || history.get())
///         .inspection(selected.binding(), |s| format!("{}: {}", s.id(), s.value())),
///     text_with(move || selected.get().and_then(|id| history.get().sample(id))
///         .map_or_else(|| "No selected sample".to_owned(), |s| s.value().to_string())),
/// );
/// # Ok::<(), ChartError>(())
/// ```
#[must_use]
pub fn line_chart(series: impl Fn() -> LineSeries + 'static) -> LineChart {
    LineChart {
        series: Rc::new(series),
        compact: false,
        color: None,
        grid_color: None,
        inspection: None,
        input: Rc::new(Cell::new(InspectionInput::default())),
    }
}
impl LineChart {
    /// Compact insets, no grid, default height 48 points. Inspection stays opt-in.
    #[must_use]
    pub const fn sparkline(mut self) -> Self {
        self.compact = true;
        self
    }
    /// Explicit series ink; otherwise the current theme's accent is used.
    #[must_use]
    pub const fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
    /// Explicit grid ink; otherwise the current theme's divider is used.
    #[must_use]
    pub const fn grid_color(mut self, color: Color) -> Self {
        self.grid_color = Some(color);
        self
    }
    /// Enables pointer and keyboard sample inspection with an optional stable ID.
    ///
    /// Hover/press chooses nearest x, ties toward earlier x. Left/Right step in
    /// visual order; Home/End choose first/last x. Exit, loss of focus, pointer
    /// cancellation and Escape clear selection. Escape ends capture; a new hover
    /// may inspect again. Tooltip text is clipped inside the chart, never an
    /// external overlay. Its formatter controls units/localization.
    ///
    /// Retained data updates resolve the ID against the new snapshot. A retired
    /// ID paints no selection and remains untouched in the binding until input;
    /// `series.sample(id)` lets a text/table alternative follow the same rule.
    #[must_use]
    pub fn inspection(
        mut self,
        selected: Binding<Option<u64>>,
        tooltip: impl Fn(LineSample) -> String + 'static,
    ) -> Self {
        self.inspection = Some(Inspection {
            selected,
            tooltip: Rc::new(tooltip),
        });
        self
    }
}
impl Component for LineChart {
    fn body(mut self) -> impl View {
        self.input = crate::model::view_model(|| Rc::new(Cell::new(InspectionInput::default())));
        crate::custom::custom(self)
    }
}

// =============================================================================
// Geometry, paint and runtime input
// =============================================================================

struct PlotRect(Rect);
impl PlotRect {
    fn new(size: Size, compact: bool) -> Option<Self> {
        if !size.width.is_finite()
            || !size.height.is_finite()
            || size.width <= 0.0
            || size.height <= 0.0
            || size.width > f64::from(f32::MAX) / 4.0
            || size.height > f64::from(f32::MAX) / 4.0
        {
            return None;
        }
        let inset: f64 = if compact { 2.0 } else { 18.0 };
        let x = inset.min(size.width / 2.0);
        let y = inset.min(size.height / 2.0);
        Some(Self(Rect {
            origin: Point { x, y },
            size: Size {
                width: (-2.0_f64).mul_add(x, size.width),
                height: (-2.0_f64).mul_add(y, size.height),
            },
        }))
    }
    fn point(&self, series: &LineSeries, sample: LineSample, rtl: bool) -> Point {
        let x = series.x_fraction(sample);
        Point {
            x: self
                .0
                .size
                .width
                .mul_add(if rtl { 1.0 - x } else { x }, self.0.origin.x),
            y: self
                .0
                .size
                .height
                .mul_add(1.0 - series.y_fraction(sample), self.0.origin.y),
        }
    }
    fn fraction(&self, x: f64, rtl: bool) -> f64 {
        let f = if self.0.size.width > 0.0 {
            ((x - self.0.origin.x) / self.0.size.width).clamp(0.0, 1.0)
        } else {
            0.5
        };
        if rtl { 1.0 - f } else { f }
    }
}
impl LineChart {
    fn inspect_at(&self, inspection: &Inspection, at: Point, ctx: &EventCtx<'_>) {
        if !at.x.is_finite() || !at.y.is_finite() {
            return;
        }
        let selected = PlotRect::new(ctx.size(), self.compact)
            .and_then(|plot| (self.series)().nearest(plot.fraction(at.x, ctx.direction.is_rtl())))
            .map(LineSample::id);
        inspection.select(selected);
    }
    fn inspect_key(&self, inspection: &Inspection, key: Key, rtl: bool) -> Response {
        if matches!(
            key,
            Key::Escape | Key::Home | Key::End | Key::Left | Key::Right
        ) {
            let mut input = self.input.get();
            input.muted = true;
            self.input.set(input);
        }
        if key == Key::Escape {
            inspection.select(None);
            return Response::handled();
        }
        let series = (self.series)();
        let points = series.samples();
        let index = inspection
            .selected
            .wrappedValue()
            .and_then(|id| series.samples.indices.get(&id).copied());
        let next = match key {
            Key::Home => points.first(),
            Key::End => points.last(),
            Key::Left | Key::Right => {
                let increasing = (key == Key::Right) != rtl;
                let index = index.map_or_else(
                    || {
                        if increasing {
                            0
                        } else {
                            points.len().saturating_sub(1)
                        }
                    },
                    |i| {
                        if increasing {
                            i.saturating_add(1).min(points.len().saturating_sub(1))
                        } else {
                            i.saturating_sub(1)
                        }
                    },
                );
                points.get(index)
            }
            _ => return Response::ignored(),
        };
        inspection.select(next.map(|p| p.id));
        Response::handled()
    }
    fn paint_inspection(
        &self,
        series: &LineSeries,
        plot: &PlotRect,
        ctx: &PaintCtx<'_>,
        painter: &mut Painter<'_>,
        ink: Color,
    ) {
        let Some((inspection, sample)) = self.inspection.as_ref().and_then(|i| {
            i.selected
                .wrappedValue()
                .and_then(|id| series.sample(id))
                .map(|s| (i, s))
        }) else {
            return;
        };
        let at = plot.point(series, sample, ctx.rtl());
        painter.fill(
            Rect {
                origin: Point {
                    x: at.x,
                    y: plot.0.origin.y,
                },
                size: Size {
                    width: 1.0,
                    height: plot.0.size.height,
                },
            },
            ink,
        );
        painter.fill_rounded(
            Rect {
                origin: Point {
                    x: at.x - 4.0,
                    y: at.y - 4.0,
                },
                size: Size {
                    width: 8.0,
                    height: 8.0,
                },
            },
            ink,
            4.0,
        );
        let text = (inspection.tooltip)(sample);
        let size = ctx.size();
        let margin = 4.0_f64.min(size.width / 2.0).min(size.height / 2.0);
        let width = (ctx.metrics.width(&text) + 16.0).min((-2.0_f64).mul_add(margin, size.width));
        let height = (ctx.metrics.line_height() + 8.0).min((-2.0_f64).mul_add(margin, size.height));
        let preferred = if ctx.rtl() {
            at.x - width - 12.0
        } else {
            at.x + 12.0
        };
        let box_rect = Rect {
            origin: Point {
                x: preferred.clamp(margin, (size.width - width - margin).max(margin)),
                y: margin,
            },
            size: Size { width, height },
        };
        painter.fill_rounded(box_rect, crate::theme::panel(), 5.0);
        painter.clipped(box_rect, |p| {
            p.text(
                Point {
                    x: box_rect.origin.x + 8.0,
                    y: box_rect.origin.y + 4.0,
                },
                text,
                crate::theme::fg(),
            );
        });
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Validated finite viewport coordinates cross the existing f32 canvas path boundary; sample values stay f64"
    )]
    fn paint_series(
        series: &LineSeries,
        plot: &PlotRect,
        ctx: &PaintCtx<'_>,
        painter: &mut Painter<'_>,
        ink: Color,
    ) {
        if let [sample] = series.samples() {
            let at = plot.point(series, *sample, ctx.rtl());
            painter.fill_rounded(
                Rect {
                    origin: Point {
                        x: at.x - 2.0,
                        y: at.y - 2.0,
                    },
                    size: Size {
                        width: 4.0,
                        height: 4.0,
                    },
                },
                ink,
                2.0,
            );
        } else {
            let path: Vec<_> = series
                .samples()
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let p = plot.point(series, *sample, ctx.rtl());
                    if index == 0 {
                        Verb::Move(p.x as f32, p.y as f32)
                    } else {
                        Verb::Line(p.x as f32, p.y as f32)
                    }
                })
                .collect();
            painter.path(&path, Paint::Stroke { width: 2.0 }, ink);
        }
    }
}
impl CustomElement for LineChart {
    fn name(&self) -> &'static str {
        "line-chart"
    }
    fn stable_measure(&self) -> bool {
        true
    }
    fn flexible(&self, axis: Axis) -> bool {
        axis == Axis::Horizontal || !self.compact
    }
    fn measure(&self, proposal: Proposal, _: &Metrics<'_>) -> Size {
        Size {
            width: proposal.width.unwrap_or(260.0).max(0.0),
            height: proposal
                .height
                .unwrap_or(if self.compact { 48.0 } else { 180.0 })
                .max(0.0),
        }
    }
    fn accepts_keys(&self) -> bool {
        self.inspection.is_some()
    }
    fn takes_text(&self) -> bool {
        false
    }
    fn takes_drag(&self) -> bool {
        self.inspection.is_some()
    }
    fn paint(&self, ctx: &PaintCtx<'_>, painter: &mut Painter<'_>) {
        let Some(plot) = PlotRect::new(ctx.size(), self.compact) else {
            return;
        };
        let series = (self.series)();
        let ink = self.color.unwrap_or_else(crate::theme::accent);
        painter.clipped(ctx.bounds(), |p| {
            if !self.compact {
                for i in 0..=4 {
                    p.fill(
                        Rect {
                            origin: Point {
                                x: plot.0.origin.x,
                                y: plot
                                    .0
                                    .size
                                    .height
                                    .mul_add(f64::from(i) / 4.0, plot.0.origin.y),
                            },
                            size: Size {
                                width: plot.0.size.width,
                                height: 1.0,
                            },
                        },
                        self.grid_color.unwrap_or_else(crate::theme::divider),
                    );
                }
            }
            Self::paint_series(&series, &plot, ctx, p, ink);
            self.paint_inspection(&series, &plot, ctx, p, ink);
            if ctx.focused && self.inspection.is_some() {
                p.stroke(ctx.bounds(), crate::theme::focus(), 1.0, 4.0);
            }
        });
    }
    fn event(&self, event: &ElementEvent, ctx: &EventCtx<'_>) -> Response {
        let Some(inspection) = &self.inspection else {
            return Response::ignored();
        };
        match event {
            ElementEvent::PointerDown { at, .. } => {
                self.input.set(InspectionInput {
                    pointer: Some(*at),
                    muted: false,
                });
                self.inspect_at(inspection, *at, ctx);
            }
            ElementEvent::PointerMoved { at, .. } => {
                let previous = self.input.get();
                if previous.muted && previous.pointer == Some(*at) {
                    return Response::handled();
                }
                self.input.set(InspectionInput {
                    pointer: Some(*at),
                    muted: false,
                });
                self.inspect_at(inspection, *at, ctx);
            }
            ElementEvent::PointerExited => {
                self.input.set(InspectionInput::default());
                inspection.select(None);
            }
            ElementEvent::PointerCancelled { .. } | ElementEvent::Focused(false) => {
                let mut input = self.input.get();
                input.muted = true;
                self.input.set(input);
                inspection.select(None);
            }
            ElementEvent::PointerUp { .. } => {}
            ElementEvent::Key(stroke) if stroke.pattern.is_plain() && !stroke.pattern.shift => {
                return self.inspect_key(inspection, stroke.pattern.key, ctx.direction.is_rtl());
            }
            _ => return Response::ignored(),
        }
        Response::handled()
    }
}
