//! The switch: a labelled two-state control.
//!
//! ```ignore
//! toggle(text("Wi-Fi"), self.wifi.binding())
//! ```
//!
//! The label is on the leading side and the switch on the trailing side,
//! with the room between them, so a column of toggles lines its switches
//! up. The whole row is the target: a press anywhere on it flips the
//! value.
//!
//! ## Keys
//!
//! The toggle is one Tab stop. Space flips it. Enter does not: a
//! two-state control leaves Enter to the form around it.
//!
//! ## Assistive technology
//!
//! The role is [`Role::Switch`](crate::accessibility::Role::Switch), the
//! name is the label, and the value is published as `checked`. The
//! activate action flips it. A disabled toggle stays in the tree and
//! refuses actions.
//!
//! ## How it is made
//!
//! A toggle is composed from the views an app uses: a stack, a spacer and
//! two painted boxes over the same button a checkbox is made of. It adds
//! no layout node and no draw command, so it draws on every renderer and
//! lowers on the web as a button does. The knob slides and the track
//! changes colour through the spring a view asks for with `.animated`.

use motor::state::{Binding, Context};

use crate::anim::Spring;
use crate::ext::ViewExt;
use crate::layout::{Color, ControlKind};
use crate::provenance::{NodeKind, Site, Written};
use crate::view::{NodeList, Single, View};
use crate::views::{ControlSpec, control, empty, hstack, spacer};

/// The track, from edge to edge.
const TRACK_WIDTH: f64 = 36.0;
const TRACK_HEIGHT: f64 = 20.0;
/// Between the track's edge and the knob.
const KNOB_INSET: f64 = 2.0;
const KNOB: f64 = TRACK_HEIGHT - 2.0 * KNOB_INSET;
/// Between the label and the switch when the row has no room to spare.
const GAP: f64 = 12.0;

/// A labelled switch backed by the application's boolean binding
/// ([`toggle`]).
#[derive(Clone)]
pub struct Toggle<L> {
    label: L,
    value: Binding<bool>,
    disabled: bool,
    site: Site,
}

impl<L> Toggle<L> {
    /// A disabled toggle shows its label and its value, and cannot take
    /// focus or change through the pointer, the keyboard or assistive
    /// technology.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<L: View + Clone> View for Toggle<L> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let on = self.value.wrappedValue();
        let value = self.value.clone();
        let theme = crate::theme::current();
        let mark = out.layout_mark();
        crate::provenance::quiet(|| {
            // the knob keeps its place in the tuple whichever side it
            // stands on, so its identity holds and the spring slides it
            let knob = empty()
                .frame(KNOB, KNOB)
                .background_color(Color::WHITE)
                .corner_radius(KNOB / 2.0)
                .animated(Spring::snappy());
            let track = hstack((on.then(spacer), knob, (!on).then(spacer)))
                .spacing(0.0)
                .padding_length(KNOB_INSET)
                .frame(TRACK_WIDTH, TRACK_HEIGHT);
            let track = if on {
                track.background_color(theme.accent).background_hovered(theme.accent).background_pressed(theme.accent)
            } else {
                track
                    .background_color(theme.control)
                    .background_hovered(theme.control_hovered)
                    .background_pressed(theme.control_pressed)
            };
            let switch = track.corner_radius(TRACK_HEIGHT / 2.0).animated(Spring::snappy()).accessibility_hidden();
            let row = hstack((Written(self.label.clone()), spacer(), switch)).spacing(GAP);
            control(
                row,
                move || value.set(!value.wrappedValue()),
                ControlSpec {
                    kind: ControlKind::Switch,
                    checked: Some(on),
                    disabled: self.disabled,
                    plain: true,
                    site: self.site,
                    shows: None,
                },
            )
            .render_unsited(ctx, out);
        });
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::Toggle, None);
    }
}

/// A labelled switch: `toggle(text("Wi-Fi"), self.wifi.binding())`.
///
/// A press on the row or a Space with the row focused flips the value.
/// [The module](self) has the keys, the role and how it is made.
#[track_caller]
pub fn toggle<L: View + Clone>(label: L, value: Binding<bool>) -> Toggle<L> {
    Toggle { label, value, disabled: false, site: crate::provenance::here() }
}
