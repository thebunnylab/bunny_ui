//! The ready-made update control: a pill that says where the update is,
//! fills with the download, and does the next step on a click.
//!
//! ```ignore
//! use bunny_ui::updater::ui::{UpdateBannerExt, update_pill};
//!
//! // in a toolbar or a title bar
//! hstack!(title(), Spacer::new(), update_pill())
//! // or over the whole window, in its top trailing corner
//! home().update_banner()
//! ```
//!
//! The pill takes no room while there is nothing to say: no updater in
//! the build, or a build that is up to date. Its words follow the locale
//! of the view, in the sixteen languages of the framework's own words.
//! It is made of ordinary views and theme tokens, so it draws on every
//! renderer and follows a theme or a skin.

pub mod model;
pub mod words;

use bunny_ui_core::layout::{Color, Truncation, UnitPoint};
use bunny_ui_core::prelude::{Context, Edge, Locale, ViewExt};
use bunny_ui_core::view::{NodeList, Single, View};
use bunny_ui_core::views::{Alignment, empty, grid, text};

use model::Tone;

/// The pill's height, in points.
pub const HEIGHT: f64 = 22.0;
/// The pill's corner radius.
pub const RADIUS: f64 = 6.0;
/// The size of its words.
pub const FONT_SIZE: f64 = 11.0;
/// How many equal columns divide the pill's width: the fill's length is
/// a thousandth of the pill at its finest, as the progress bar's is.
const RESOLUTION: usize = 1000;

/// The update control: [`update_pill`].
#[derive(Clone, Copy, Debug, Default)]
pub struct UpdatePill;

/// The update control. Nothing (no size, no node of its own) while the
/// updater has nothing to say or no updater runs.
#[must_use]
pub fn update_pill() -> UpdatePill {
    UpdatePill
}

impl View for UpdatePill {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        // reading the state here makes this view follow it
        let state = crate::state().map(|state| state.wrappedValue()).unwrap_or_default();
        let words = words::WORDS.pick(&ctx.environment::<Locale>());
        let Some(reading) = model::reading(&state, &words) else {
            empty().render_into(ctx, out);
            return;
        };
        let theme = bunny_ui_core::theme::current();
        let ink = match reading.tone {
            Tone::Action => theme.accent,
            Tone::Quiet => theme.fg_secondary,
        };
        // one line: a squeezed pill ends in an ellipsis, never over its
        // neighbors
        let label = text(reading.label)
            .font_size(FONT_SIZE)
            .foreground_color(ink)
            .truncation_mode(Truncation::End)
            .padding_edge(Edge::Leading, 8.0)
            .padding_edge(Edge::Trailing, 8.0)
            .frame_height(HEIGHT)
            .background(UnitPoint::CENTER, Ground { filled: reading.filled, well: theme.control, wash: wash(theme.accent) });
        if reading.clickable {
            label.on_click(crate::click).id("update-pill").render_into(ctx, out);
        } else {
            label.id("update-pill").render_into(ctx, out);
        }
    }
}

/// The accent at a wash that the words stay legible on.
fn wash(accent: Color) -> Color {
    Color { a: 70, ..accent }
}

/// The pill's ground: the well, and over it, while an install runs, the
/// share of the width the install has filled. The fill is the pill
/// itself, not a bar beside the words: the pill keeps its size before,
/// during and after an install, and no bar crosses the words.
///
/// Like the determinate `ProgressView`, it is ordinary views: a grid of
/// equal columns divides the width, and the fill spans the share. It
/// draws on every renderer and mirrors right to left.
#[derive(Clone, Copy)]
struct Ground {
    filled: Option<f32>,
    well: Color,
    wash: Color,
}

impl View for Ground {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let units = self.filled.map_or(0, |share| (f64::from(share.clamp(0.0, 1.0)) * RESOLUTION as f64).round() as usize);
        let fill = (units > 0).then(|| {
            empty()
                .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Leading)
                .frame_height(HEIGHT)
                .background_color(self.wash)
                .corner_radius(RADIUS)
                .span(units, 1)
        });
        grid(fill)
            .columns(RESOLUTION)
            .spacing(0.0)
            .alignment(Alignment::Leading)
            .frame_height(HEIGHT)
            .background_color(self.well)
            .corner_radius(RADIUS)
            .allows_hit_testing(false)
            .render_into(ctx, out);
    }
}

/// `.update_banner()` on any view.
pub trait UpdateBannerExt: View<Arity = Single> + Sized {
    /// The view with the update control over its top trailing corner,
    /// for a window with no title bar or toolbar to hold it.
    fn update_banner(self) -> impl View<Arity = Single> {
        self.overlay(UnitPoint::TOP_TRAILING, update_pill().padding_edge(Edge::Top, 8.0).padding_edge(Edge::Trailing, 8.0))
    }
}

impl<V: View<Arity = Single>> UpdateBannerExt for V {}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
