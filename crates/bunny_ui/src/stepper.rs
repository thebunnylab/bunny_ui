//! The stepper: a number moved one step at a time.
//!
//! ```ignore
//! stepper(self.guests.binding(), 1..=8)
//! ```
//!
//! The binding holds the number, of any integer or float type. The two
//! halves take one step down and one step up, and the number stays inside
//! the bounds. The stepper shows no number: put it beside the text that
//! does.
//!
//! ```ignore
//! hstack!(text!("{} guests", self.guests), stepper(self.guests.binding(), 1..=8))
//! ```
//!
//! A step is 1 unless [`Stepper::step`] names another.
//!
//! ## Keys
//!
//! The stepper is one Tab stop. Up and Right take a step up, Down and
//! Left a step down; the horizontal arrows swap in a right-to-left
//! scene. PageUp and PageDown take ten steps. Home and End go to the
//! bounds.
//!
//! ## Assistive technology
//!
//! The role is [`Role::Stepper`](crate::accessibility::Role), with the
//! number, its bounds and its step as a range. The increment, decrement
//! and set actions move it. Name it with `.accessibility_label(…)`.
//!
//! ## How it is made
//!
//! A stepper is the slider's twin: one box the framework paints, with the
//! slider's own numeric domain, keys and range semantics. It needs the
//! `canvas` feature, and on the web it is a canvas island as the slider
//! is. Holding a half down takes one step; it does not repeat.

use std::cell::Cell;
use std::ops::RangeInclusive;
use std::rc::Rc;

use motor::state::{Binding, LayoutDirection};

use crate::action::Key;
use crate::custom::{CustomElement, ElementEvent, EventCtx, Metrics, PaintCtx, Painter, Response};
use crate::icon::house as symbol;
use crate::layout::{Axis, Point, Proposal, Rect, Size};
use crate::slider::SliderRange;
use crate::view::{Component, View};

/// The box, from edge to edge.
const WIDTH: f64 = 72.0;
const HEIGHT: f64 = 26.0;
const RADIUS: f64 = 6.0;
/// The glyph of a half.
const GLYPH: f64 = 12.0;

mod sealed {
    pub trait Sealed {}
}

/// A number a [`stepper`] can move: the integer and the float types.
///
/// The trait is sealed. A stepper works on one number type inside, and
/// this is the door each type of the binding comes through.
pub trait StepValue: sealed::Sealed + Copy + 'static {
    /// The number as the stepper counts it.
    fn to_f64(self) -> f64;
    /// The number the stepper writes back, rounded for an integer type
    /// and held at the type's ends.
    fn from_f64(value: f64) -> Self;
}

macro_rules! step_integers {
    ($($ty:ty),+ $(,)?) => {$(
        impl sealed::Sealed for $ty {}
        impl StepValue for $ty {
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_lossless,
                reason = "a stepper counts small numbers; a 64-bit value past 2^53 is no step count"
            )]
            fn to_f64(self) -> f64 {
                self as f64
            }
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a float cast to an integer saturates at the type's ends, which is the bound wanted"
            )]
            fn from_f64(value: f64) -> Self {
                value.round() as $ty
            }
        }
    )+};
}

step_integers!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl sealed::Sealed for f64 {}
impl StepValue for f64 {
    fn to_f64(self) -> f64 {
        self
    }
    fn from_f64(value: f64) -> Self {
        value
    }
}

impl sealed::Sealed for f32 {}
impl StepValue for f32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    #[allow(clippy::cast_possible_truncation, reason = "the nearest f32 is the value the binding can hold")]
    fn from_f64(value: f64) -> Self {
        value as f32
    }
}

/// A number moved one step at a time ([`stepper`]).
#[derive(Clone)]
pub struct Stepper {
    value: Binding<f64>,
    /// `None` when the bounds or the step cannot be counted: the stepper
    /// is then shown disabled.
    range: Option<SliderRange>,
    disabled: bool,
}

/// A number moved one step at a time: `stepper(self.guests.binding(), 1..=8)`.
///
/// The bounds are inclusive and must be increasing. A stepper given
/// bounds that are not is shown disabled, and a debug build says why.
/// [The module](self) has the keys, the role and how it is made.
#[must_use]
pub fn stepper<T: StepValue>(value: Binding<T>, bounds: RangeInclusive<T>) -> Stepper {
    let (lower, upper) = bounds.into_inner();
    let range = SliderRange::new(lower.to_f64()..=upper.to_f64()).and_then(|range| range.with_step(1.0));
    debug_assert!(range.is_ok(), "stepper bounds must be finite and increasing: {range:?}");
    let (read, write) = (value.clone(), value);
    Stepper {
        value: Binding::new(move || read.wrappedValue().to_f64(), move |number| write.set(T::from_f64(number))),
        range: range.ok(),
        disabled: false,
    }
}

impl Stepper {
    /// The size of a step, 1 unless named. It must be positive, and
    /// coarse enough to count between the bounds.
    #[must_use]
    pub fn step(mut self, step: f64) -> Self {
        let stepped = self
            .range
            .map(|range| SliderRange::new(range.lower()..=range.upper()).and_then(|range| range.with_step(step)));
        debug_assert!(!matches!(stepped, Some(Err(_))), "stepper step must be positive and countable: {stepped:?}");
        self.range = stepped.and_then(Result::ok);
        self
    }

    /// A disabled stepper is shown muted, takes no focus and never writes.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Component for Stepper {
    fn body(self) -> impl View {
        // the half the pointer holds down: `Some(true)` is the step up
        let pressed = crate::model::view_model(|| Rc::new(Cell::new(None::<bool>)));
        crate::custom::custom(StepperElement { stepper: self, pressed })
    }
}

struct StepperElement {
    stepper: Stepper,
    pressed: Rc<Cell<Option<bool>>>,
}

impl StepperElement {
    /// The range, while the stepper can move.
    fn live(&self) -> Option<SliderRange> {
        self.stepper.range.filter(|_| !self.stepper.disabled)
    }

    fn write(&self, value: f64) {
        if self.stepper.value.wrappedValue().to_bits() != value.to_bits() {
            self.stepper.value.set(value);
        }
    }

    fn take(&self, range: SliderRange, increasing: bool, count: f64) {
        self.write(range.advance(self.stepper.value.wrappedValue(), increasing, count));
    }

    /// The half under `x`: `true` is the step up. The step down is on the
    /// leading side.
    fn half(x: f64, width: f64, rtl: bool) -> bool {
        (x >= width / 2.0) != rtl
    }
}

impl CustomElement for StepperElement {
    fn name(&self) -> &'static str {
        "stepper"
    }
    fn stable_measure(&self) -> bool {
        true
    }
    // a box takes the leftover room of its stack unless it says no, and a
    // stepper is a small control of one size
    fn flexible(&self, _axis: Axis) -> bool {
        false
    }
    fn measure(&self, _proposal: Proposal, _metrics: &Metrics<'_>) -> Size {
        Size { width: WIDTH, height: HEIGHT }
    }
    fn accepts_keys(&self) -> bool {
        self.live().is_some()
    }
    fn accessibility_role(&self) -> crate::accessibility::Role {
        crate::accessibility::Role::Stepper
    }
    fn accessibility_range(&self) -> Option<crate::accessibility::RangeValue> {
        let range = self.stepper.range?;
        Some(crate::accessibility::RangeValue::new(range, self.stepper.value.wrappedValue()))
    }
    fn accessibility_action(
        &self,
        action: &crate::accessibility::Action,
    ) -> Result<(), crate::accessibility::ActionError> {
        use crate::accessibility::{Action, ActionError};
        let Some(range) = self.live() else {
            return Err(ActionError::Unsupported);
        };
        match *action {
            Action::Increment => self.take(range, true, 1.0),
            Action::Decrement => self.take(range, false, 1.0),
            Action::SetValue(requested) => {
                if !requested.is_finite() || !(range.lower()..=range.upper()).contains(&requested) {
                    return Err(ActionError::InvalidValue);
                }
                self.write(range.snap(requested));
            }
            _ => return Err(ActionError::Unsupported),
        }
        Ok(())
    }
    fn takes_text(&self) -> bool {
        false
    }
    fn paint(&self, ctx: &PaintCtx<'_>, painter: &mut Painter<'_>) {
        let size = ctx.size();
        let theme = crate::theme::current();
        let rtl = ctx.direction == LayoutDirection::RightToLeft;
        let live = self.live();
        let whole = Rect { origin: Point { x: 0.0, y: 0.0 }, size };
        painter.fill_rounded(whole, theme.control, RADIUS);
        let half = Size { width: size.width / 2.0, height: size.height };
        let value = self.stepper.value.wrappedValue();
        for increasing in [false, true] {
            // the step down stands on the leading side
            let on_the_right = increasing != rtl;
            let rect = Rect { origin: Point { x: if on_the_right { half.width } else { 0.0 }, y: 0.0 }, size: half };
            if live.is_some() && self.pressed.get() == Some(increasing) {
                painter.fill_rounded(rect, theme.control_pressed, RADIUS);
            }
            // a half with no step left to take is drawn faint
            let spent = self.stepper.range.is_some_and(|range| {
                if increasing { value >= range.upper() } else { value <= range.lower() }
            });
            let ink = if live.is_none() || spent { theme.fg_faint } else { theme.fg };
            let glyph = Rect {
                origin: Point {
                    x: rect.origin.x + (half.width - GLYPH) / 2.0,
                    y: (size.height - GLYPH) / 2.0,
                },
                size: Size { width: GLYPH, height: GLYPH },
            };
            painter.icon(glyph, if increasing { symbol::PLUS } else { symbol::MINUS }, ink);
        }
        let rule = Rect {
            origin: Point { x: half.width - 0.5, y: 5.0 },
            size: Size { width: 1.0, height: (size.height - 10.0).max(0.0) },
        };
        painter.fill(rule, theme.divider);
        if ctx.focused && live.is_some() {
            painter.stroke(whole, theme.focus, 2.0, RADIUS);
        }
    }
    fn event(&self, event: &ElementEvent, ctx: &EventCtx<'_>) -> Response {
        let Some(range) = self.live() else {
            self.pressed.set(None);
            return Response::ignored();
        };
        let rtl = ctx.direction == LayoutDirection::RightToLeft;
        match event {
            ElementEvent::PointerDown { at, .. } => {
                let increasing = Self::half(at.x, ctx.size().width, rtl);
                self.pressed.set(Some(increasing));
                self.take(range, increasing, 1.0);
            }
            ElementEvent::PointerUp { .. }
            | ElementEvent::PointerCancelled { .. }
            | ElementEvent::Focused(false) => self.pressed.set(None),
            ElementEvent::Key(stroke) if stroke.pattern.is_plain() && !stroke.pattern.shift => {
                match stroke.pattern.key {
                    Key::Home => self.write(range.lower()),
                    Key::End => self.write(range.upper()),
                    Key::Up => self.take(range, true, 1.0),
                    Key::Down => self.take(range, false, 1.0),
                    Key::Right => self.take(range, !rtl, 1.0),
                    Key::Left => self.take(range, rtl, 1.0),
                    Key::PageUp => self.take(range, true, 10.0),
                    Key::PageDown => self.take(range, false, 10.0),
                    _ => return Response::ignored(),
                }
            }
            _ => return Response::ignored(),
        }
        Response::handled()
    }
}
