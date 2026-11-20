//! `ProgressView` — the determinate bar, and the placeholder it was.
//!
//! ```ignore
//! progress_view().value(0.42)
//! ```
//!
//! With a value, the view is a horizontal track with a fill on its
//! leading side, as long as the share of the track the value names. It
//! takes the width it is offered and stands at the theme's
//! [`ProgressBar`](crate::theme::ProgressBar) height. A right-to-left
//! scene fills from the right.
//!
//! Without a value it is still the 20 × 20 box it always was: an
//! indeterminate spinner does not exist yet, and a scene that already
//! holds a `progress_view()` lays out exactly as before.
//!
//! ## How it is made
//!
//! Like the toggle, the bar is composed from the views an app uses and
//! paints through [`VisualProps`](crate::layout::VisualProps) only, so it
//! draws on every renderer and lowers on the web without a canvas. The
//! track is an equal-column [`grid`] of [`RESOLUTION`] tracks, and the
//! fill spans as many of them as the value covers: a grid already
//! divides its width evenly, mirrors under right to left and becomes a
//! CSS grid on the DOM road, so the fill needs no layout node of its own.
//!
//! ## Assistive technology
//!
//! The bar adds no semantics of its own yet: the accessibility tree has
//! no read-only range role (its [`RangeValue`](crate::accessibility::RangeValue)
//! belongs to the slider). Give the bar an `accessibility_label`, or put
//! the percentage in a text beside it.

use motor::state::Context;
use motor::view::RenderNode;

use crate::ext::ViewExt;
use crate::layout::{LayoutNode, Size as LayoutSize};
use crate::view::{NodeList, Single, View};
use crate::views::{Alignment, empty, grid};

/// How many equal tracks the bar is divided into: the fill's length is
/// quantized to a thousandth of the track.
pub const RESOLUTION: usize = 1000;

/// `ProgressView()` and `ProgressView(value:)` ([`progress_view`]).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ProgressView {
    value: Option<f32>,
}

impl ProgressView {
    /// The share of the work done, `0..=1`: a value below zero is
    /// zero, one above one is one, and NaN is nothing done.
    #[must_use]
    pub fn value(mut self, value: f32) -> Self {
        self.value = Some(clamped(value));
        self
    }

    /// The value the bar shows, already clamped; `None` without one.
    pub fn fraction(&self) -> Option<f32> {
        self.value
    }
}

/// The value as the bar shows it.
fn clamped(value: f32) -> f32 {
    if value.is_nan() { 0.0 } else { value.clamp(0.0, 1.0) }
}

/// The tracks the fill covers for a clamped value.
pub(crate) fn covered(value: f32) -> usize {
    (f64::from(value) * RESOLUTION as f64).round() as usize
}

impl View for ProgressView {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let Some(value) = self.value else {
            out.push(RenderNode::leaf(crate::view::print_label("ProgressView")));
            out.push_layout(LayoutNode::Leaf {
                size: LayoutSize { width: 20.0, height: 20.0 },
            });
            return;
        };
        let theme = crate::theme::current();
        let bar = theme.progress;
        let units = covered(value);
        // the composition is the framework's, not the app's: it prints
        // as one line and records no source site of its own
        let mut inner = NodeList::new();
        crate::provenance::quiet(|| {
            let fill = (units > 0).then(|| {
                // a box of nothing that takes the cells it spans, and
                // the bar's height
                empty()
                    .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Leading)
                    .frame_height(bar.height)
                    .background_color(theme.accent)
                    .corner_radius(bar.radius)
                    .span(units, 1)
            });
            grid(fill)
                .columns(RESOLUTION)
                .spacing(0.0)
                .alignment(Alignment::Leading)
                .frame_height(bar.height)
                .background_color(theme.divider)
                .corner_radius(bar.radius)
                .render_into(ctx, &mut inner);
        });
        let (_, layout) = inner.into_parts();
        out.push(RenderNode::leaf(crate::view::print_line(|| {
            format!("ProgressView(value: {value})")
        })));
        for node in layout {
            out.push_layout(node);
        }
    }
}

/// `ProgressView()` — chain [`ProgressView::value`] for the determinate
/// bar.
pub fn progress_view() -> ProgressView {
    ProgressView::default()
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
