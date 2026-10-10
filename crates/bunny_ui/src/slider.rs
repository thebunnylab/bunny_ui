//! A bounded scalar control shares the framework's capture, focus and paint paths.
//!
//! ## Wiring
//! Construct a [`SliderRange`], then pass it and a live binding to [`slider`].
//! The private element uses the existing native input/runtime machinery. A
//! range is valid before the view exists; painting never repairs application
//! state. This control requires `canvas`; element-mode web uses a canvas island.
//!
//! ## Production gotchas
//! Native assistive-technology range semantics are not exposed yet. Provide a
//! visible label/value; this primitive alone is not accessibility qualification.
//! Nonfinite or out-of-range binding values are represented at a bounded visual
//! position but stay untouched until an interaction writes a valid value.

use std::cell::Cell;
use std::ops::RangeInclusive;
use std::rc::Rc;

use motor::state::{Binding, LayoutDirection};

use crate::action::Key;
use crate::custom::{CustomElement, ElementEvent, EventCtx, Metrics, PaintCtx, Painter, Response};
use crate::layout::{Axis, Point, Proposal, Rect, Size};
use crate::view::{Component, View};

// =============================================================================
// Validated numeric domain
// =============================================================================

/// Why a slider's numeric domain cannot be represented safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderRangeError {
    /// Either endpoint is NaN or infinite.
    NonFinite,
    /// The upper endpoint is not strictly above the lower endpoint.
    NotIncreasing,
    /// Subtracting the endpoints overflows or leaves no usable keyboard step.
    UnrepresentableSpan,
    /// A step must be positive and finite.
    InvalidStep,
    /// The step is too fine for the endpoints or the range's step index.
    IneffectiveStep,
}
impl std::fmt::Display for SliderRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NonFinite => "slider endpoints must be finite",
            Self::NotIncreasing => "slider endpoints must be increasing",
            Self::UnrepresentableSpan => "slider span cannot be represented",
            Self::InvalidStep => "slider step must be positive and finite",
            Self::IneffectiveStep => "slider step exceeds the range's floating-point precision",
        })
    }
}
impl std::error::Error for SliderRangeError {}

/// A finite increasing domain, optionally snapped to steps from its lower end.
/// Both endpoints are reachable even when the step does not divide the span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderRange {
    lower: f64,
    upper: f64,
    step: Option<f64>,
}
impl SliderRange {
    /// Validates a continuous range. Keyboard arrows move by 1% of its span.
    ///
    /// # Errors
    /// Rejects nonfinite, reversed, empty or numerically unusable ranges.
    pub fn new(bounds: RangeInclusive<f64>) -> Result<Self, SliderRangeError> {
        let (lower, upper) = bounds.into_inner();
        if !lower.is_finite() || !upper.is_finite() {
            return Err(SliderRangeError::NonFinite);
        }
        if lower >= upper {
            return Err(SliderRangeError::NotIncreasing);
        }
        let span = upper - lower;
        if !span.is_finite() || span / 100.0 == 0.0 {
            return Err(SliderRangeError::UnrepresentableSpan);
        }
        Ok(Self {
            lower,
            upper,
            step: None,
        })
    }

    /// Snaps pointer input to the nearest step or endpoint; arrows move one step.
    ///
    /// # Errors
    /// Rejects nonpositive/nonfinite steps and steps finer than the domain's
    /// floating-point precision can represent at either endpoint or across the
    /// step index (at most 2^52 steps).
    pub fn with_step(mut self, step: f64) -> Result<Self, SliderRangeError> {
        if !step.is_finite() || step <= 0.0 {
            return Err(SliderRangeError::InvalidStep);
        }
        if self.span() / step > 4_503_599_627_370_496.0
            || self.lower + step <= self.lower
            || self.upper - step >= self.upper
        {
            return Err(SliderRangeError::IneffectiveStep);
        }
        self.step = Some(step);
        Ok(self)
    }

    /// Inclusive minimum.
    #[must_use]
    pub const fn lower(self) -> f64 {
        self.lower
    }
    /// Inclusive maximum.
    #[must_use]
    pub const fn upper(self) -> f64 {
        self.upper
    }
    /// Quantization interval, or `None` for continuous pointer input.
    #[must_use]
    pub const fn step(self) -> Option<f64> {
        self.step
    }

    const fn span(self) -> f64 {
        self.upper - self.lower
    }
    const fn bound(self, value: f64) -> f64 {
        if value.is_nan() {
            self.lower
        } else {
            value.clamp(self.lower, self.upper)
        }
    }
    const fn fraction(self, value: f64) -> f64 {
        (self.bound(value) - self.lower) / self.span()
    }
    fn snap(self, value: f64) -> f64 {
        let value = self.bound(value);
        let Some(step) = self.step else {
            return value;
        };
        let grid = self.bound(
            ((value - self.lower) / step)
                .round()
                .mul_add(step, self.lower),
        );
        if (self.upper - value).abs() < (grid - value).abs() {
            self.upper
        } else {
            grid
        }
    }
    fn advance(self, value: f64, increasing: bool, count: f64) -> f64 {
        let value = self.bound(value);
        let next = self.step.map_or_else(
            || {
                let moved =
                    (self.span() / 100.0).mul_add(if increasing { count } else { -count }, value);
                if increasing {
                    moved.max(value.next_up())
                } else {
                    moved.min(value.next_down())
                }
            },
            |step| {
                let index = (value - self.lower) / step;
                // A represented grid point may divide to 2.9999999999999996.
                // Treat that rounding error as the same point, not another step.
                let epsilon = (f64::EPSILON * index.abs().max(1.0) * 4.0).min(0.25);
                let next = if increasing {
                    (index + epsilon).floor() + count
                } else {
                    (index - epsilon).ceil() - count
                };
                next.mul_add(step, self.lower)
            },
        );
        self.bound(next)
    }
}

// =============================================================================
// Public control
// =============================================================================

/// A horizontal slider with captured pointer input and keyboard focus.
///
/// Arrows change one step (1% for continuous ranges); PageUp/PageDown change
/// ten; Home/End reach the endpoints. RTL mirrors the track and horizontal
/// arrows. Escape ends a drag at its last committed value. The binding is read
/// live for every event and paint; repeated endpoint input does not write again.
#[derive(Clone)]
pub struct Slider {
    value: Binding<f64>,
    range: SliderRange,
    disabled: bool,
}

/// Binds a reusable horizontal scalar control to an already validated domain.
///
/// ```
/// use bunny_ui_core::prelude::*;
/// let value = State::new(10.0);
/// let range = SliderRange::new(1.0..=60.0)?.with_step(1.0)?;
/// let rate = slider(value.binding(), range);
/// # Ok::<(), SliderRangeError>(())
/// ```
#[must_use]
pub const fn slider(value: Binding<f64>, range: SliderRange) -> Slider {
    Slider {
        value,
        range,
        disabled: false,
    }
}
impl Slider {
    /// A disabled slider paints muted, declines focus and never writes.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

// =============================================================================
// Retained interaction and painting
// =============================================================================

#[derive(Clone, Copy, Debug, Default)]
enum Gesture {
    #[default]
    Idle,
    Dragging {
        offset: f64,
    },
    Cancelled,
}

impl Component for Slider {
    fn body(self) -> impl View {
        let gesture = crate::model::view_model(|| Rc::new(Cell::new(Gesture::Idle)));
        if self.disabled {
            gesture.set(Gesture::Cancelled);
        }
        crate::custom::custom(SliderElement {
            slider: self,
            gesture,
        })
    }
}

struct SliderElement {
    slider: Slider,
    gesture: Rc<Cell<Gesture>>,
}
impl SliderElement {
    // The thumb has a 16-point diameter inside a minimum 28-point hit target.
    fn track(width: f64) -> (f64, f64) {
        let inset = 10.0_f64.min(width.max(0.0) / 2.0);
        (inset, (-2.0_f64).mul_add(inset, width).max(0.0))
    }
    fn position(&self, width: f64, rtl: bool) -> f64 {
        let (start, length) = Self::track(width);
        let fraction = self.slider.range.fraction(self.slider.value.wrappedValue());
        length.mul_add(if rtl { 1.0 - fraction } else { fraction }, start)
    }
    fn write(&self, value: f64) {
        if self.slider.value.wrappedValue().to_bits() != value.to_bits() {
            self.slider.value.set(value);
        }
    }
    fn move_to(&self, x: f64, ctx: &EventCtx<'_>) {
        let (start, length) = Self::track(ctx.size().width);
        if !x.is_finite() || !length.is_finite() || length <= 0.0 {
            return;
        }
        let fraction = ((x - start) / length).clamp(0.0, 1.0);
        let fraction = if ctx.direction == LayoutDirection::RightToLeft {
            1.0 - fraction
        } else {
            fraction
        };
        self.write(
            self.slider.range.snap(
                self.slider
                    .range
                    .span()
                    .mul_add(fraction, self.slider.range.lower),
            ),
        );
    }
}
impl CustomElement for SliderElement {
    fn name(&self) -> &'static str {
        "slider"
    }
    fn stable_measure(&self) -> bool {
        true
    }
    fn flexible(&self, axis: Axis) -> bool {
        axis == Axis::Horizontal
    }
    fn measure(&self, proposal: Proposal, _metrics: &Metrics<'_>) -> Size {
        Size {
            width: proposal.width.unwrap_or(160.0).max(0.0),
            height: proposal.height.unwrap_or(28.0).max(0.0),
        }
    }
    fn accepts_keys(&self) -> bool {
        !self.slider.disabled
    }
    fn takes_text(&self) -> bool {
        false
    }
    fn takes_drag(&self) -> bool {
        true
    }
    fn paint(&self, ctx: &PaintCtx<'_>, painter: &mut Painter<'_>) {
        let size = ctx.size();
        let (start, length) = Self::track(size.width);
        let rtl = ctx.direction == LayoutDirection::RightToLeft;
        let center = self.position(size.width, rtl);
        let y = size.height / 2.0;
        let accent = if self.slider.disabled {
            crate::theme::fg_faint()
        } else {
            crate::theme::accent()
        };
        painter.fill_rounded(
            Rect {
                origin: Point {
                    x: start,
                    y: y - 2.0,
                },
                size: Size {
                    width: length,
                    height: 4.0,
                },
            },
            crate::theme::divider(),
            2.0,
        );
        let (origin, width) = if rtl {
            (center, start + length - center)
        } else {
            (start, center - start)
        };
        painter.fill_rounded(
            Rect {
                origin: Point {
                    x: origin,
                    y: y - 2.0,
                },
                size: Size { width, height: 4.0 },
            },
            accent,
            2.0,
        );
        let radius = 8.0_f64
            .min(size.width / 2.0)
            .min(size.height / 2.0)
            .max(0.0);
        let thumb = Rect {
            origin: Point {
                x: center - radius,
                y: y - radius,
            },
            size: Size {
                width: 2.0 * radius,
                height: 2.0 * radius,
            },
        };
        painter.fill_rounded(thumb, accent, radius);
        if ctx.focused && !self.slider.disabled {
            painter.stroke(
                Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size,
                },
                accent,
                1.0,
                6.0,
            );
        }
    }
    fn event(&self, event: &ElementEvent, ctx: &EventCtx<'_>) -> Response {
        if self.slider.disabled {
            self.gesture.set(Gesture::Cancelled);
            return Response::ignored();
        }
        match event {
            ElementEvent::PointerDown { at, .. } => {
                let center = self.position(
                    ctx.size().width,
                    ctx.direction == LayoutDirection::RightToLeft,
                );
                let on_thumb = (at.x - center).abs() <= 8.0;
                let offset = if on_thumb { at.x - center } else { 0.0 };
                self.gesture.set(Gesture::Dragging { offset });
                if !on_thumb {
                    self.move_to(at.x, ctx);
                }
            }
            ElementEvent::PointerMoved {
                at, pressed: true, ..
            } => {
                if let Gesture::Dragging { offset } = self.gesture.get() {
                    self.move_to(at.x - offset, ctx);
                }
            }
            ElementEvent::PointerUp { .. }
            | ElementEvent::PointerCancelled { .. }
            | ElementEvent::Focused(false) => self.gesture.set(Gesture::Idle),
            ElementEvent::Key(stroke) if stroke.pattern.is_plain() && !stroke.pattern.shift => {
                let current = self.slider.value.wrappedValue();
                let rtl = ctx.direction == LayoutDirection::RightToLeft;
                let next = match stroke.pattern.key {
                    Key::Escape if matches!(self.gesture.get(), Gesture::Dragging { .. }) => {
                        self.gesture.set(Gesture::Cancelled);
                        return Response::handled();
                    }
                    Key::Home => self.slider.range.lower,
                    Key::End => self.slider.range.upper,
                    Key::Up => self.slider.range.advance(current, true, 1.0),
                    Key::Down => self.slider.range.advance(current, false, 1.0),
                    Key::Right => self.slider.range.advance(current, !rtl, 1.0),
                    Key::Left => self.slider.range.advance(current, rtl, 1.0),
                    Key::PageUp => self.slider.range.advance(current, true, 10.0),
                    Key::PageDown => self.slider.range.advance(current, false, 10.0),
                    _ => return Response::ignored(),
                };
                self.write(next);
            }
            _ => return Response::ignored(),
        }
        Response::handled()
    }
}
