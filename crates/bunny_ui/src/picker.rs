//! The pop-up choice: a button that shows one value and opens a list of
//! the others.
//!
//! ```ignore
//! picker(self.look.binding(), [
//!     (Look::System, "System"),
//!     (Look::Light, "Light"),
//!     (Look::Dark, "Dark"),
//! ])
//! ```
//!
//! The binding holds the value. The button shows the words of the choice
//! that has it. A press opens the list under the button, and a press on a
//! row writes its value and closes the list. A value that is none of the
//! choices shows no words and no check mark, and the binding is never
//! written to repair it.
//!
//! Use it for one value out of more than a few. For two to five that
//! deserve to be seen all the time, a segmented control shows them.
//!
//! ## Keys
//!
//! The button is one Tab stop. Space, Enter and the vertical arrows open
//! the list. The list opens with the keyboard on the row of the current
//! choice. The vertical arrows move between rows and go round, Home and
//! End jump, and Space or Enter chooses the row and closes the list.
//! Escape closes it and leaves the value. Tab stays inside the list
//! while it is open. When the list closes, the keyboard is on the button
//! again.
//!
//! ## Assistive technology
//!
//! The button is a [`Role::PopUpButton`](crate::accessibility::Role)
//! whose `value` is the words it shows and whose `expanded` says whether
//! the list is open. Each row is a `RadioButton` on the list's own
//! surface, checked on the current choice.
//!
//! ## How it is made
//!
//! A button, a popover and a column of rows: views an app could write.
//! The list is not the context menu. A context menu is opened by the
//! pointer at a point and has no keys of its own; a list of choices is
//! reached by the keyboard first.

use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context, State};
use motor::views::Edge;

use crate::choice::{Choice, Grouped, IntoChoices};
use crate::erased::erased;
use crate::ext::ViewExt;
use crate::icon::house as symbol;
use crate::layout::{Axis, ControlKind, GroupSpec, LayoutNode, Side};
use crate::provenance::{NodeKind, Quiet, Site};
use crate::state_ext::StateExt;
use crate::view::{Component, NodeList, Single, View};
use crate::views::{Alignment, ControlSpec, control, hstack, icon, text, vstack};

/// A row of the list, from edge to edge.
const ROW_HEIGHT: f64 = 26.0;
const ROW_RADIUS: f64 = 5.0;
/// Between a row's edge and its mark, and after its words.
const ROW_INSET: f64 = 8.0;
/// Between the list's edge and its rows.
const LIST_INSET: f64 = 4.0;
const LIST_RADIUS: f64 = 8.0;

/// A button that shows one value and opens a list of the others
/// ([`picker`]).
#[derive(Clone)]
pub struct Picker<T: 'static> {
    selection: Binding<T>,
    choices: Rc<[Choice<T>]>,
    disabled: bool,
    site: Site,
}

impl<T: 'static> Picker<T> {
    /// A disabled picker shows its choice and cannot open.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<T: Clone + PartialEq + 'static> View for Picker<T> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        Open {
            selection: self.selection.clone(),
            choices: Rc::clone(&self.choices),
            disabled: self.disabled,
            site: self.site,
        }
        .render_into(ctx, out);
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::Picker, None);
    }
}

/// The picker's own body: it holds whether the list is open, so opening
/// the list runs this body again and nothing of the app's.
#[derive(Clone)]
struct Open<T: 'static> {
    selection: Binding<T>,
    choices: Rc<[Choice<T>]>,
    disabled: bool,
    site: Site,
}

impl<T: Clone + PartialEq + 'static> Component for Open<T> {
    fn body(self) -> impl View {
        let open = State::new(false);
        let current = self.selection.wrappedValue();
        let selected = self.choices.iter().position(|choice| *choice.value() == current);
        let shown: Arc<str> = selected.map_or_else(|| Arc::from(""), |index| Arc::from(self.choices[index].label()));
        let site = self.site;
        let label = hstack((text(&*shown), icon(symbol::CHEVRON_DOWN))).spacing(6.0);
        let button = control(
            label,
            move || open.set(!open.get()),
            ControlSpec {
                kind: ControlKind::PopUp,
                checked: None,
                disabled: self.disabled,
                plain: false,
                site,
                shows: Some((shown, open.get())),
            },
        );
        let choices = self.choices;
        let selection = self.selection;
        Quiet(button.popover(open.binding(), Side::Bottom, move |_| {
            let theme = crate::theme::current();
            let rows: Vec<_> = choices
                .iter()
                .enumerate()
                .map(|(index, choice)| {
                    let on = selected == Some(index);
                    let selection = selection.clone();
                    let value = choice.value().clone();
                    // the mark keeps its column on every row, so the words line up
                    let mark = icon(symbol::CHECK).opacity(if on { 1.0 } else { 0.0 });
                    let row = hstack((mark, text(choice.label())))
                        .spacing(6.0)
                        .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Leading)
                        .frame_height(ROW_HEIGHT)
                        .padding_edge(Edge::Leading, ROW_INSET)
                        .padding_edge(Edge::Trailing, ROW_INSET)
                        .background_hovered(theme.row_hover)
                        .background_pressed(theme.row_pressed)
                        .corner_radius(ROW_RADIUS);
                    control(
                        row,
                        move || {
                            selection.set(value.clone());
                            open.set(false);
                        },
                        ControlSpec {
                            kind: ControlKind::Radio,
                            checked: Some(on),
                            disabled: choice.is_disabled(),
                            plain: true,
                            site,
                            shows: None,
                        },
                    )
                })
                .collect();
            let list = vstack(rows)
                .spacing(0.0)
                .padding_length(LIST_INSET)
                .background_color(theme.panel)
                .border(theme.border, 1.0)
                .corner_radius(LIST_RADIUS);
            let rows = Grouped { content: list, spec: GroupSpec { columns: 1, selects: false, leads: false, round: true } };
            erased(Quiet(AsWideAsItsRows(rows)))
        }))
    }
}

/// The list is as wide as its widest row, and every row is as wide as
/// the list.
///
/// A row fills the width it is offered, so that its hover covers the
/// whole row. Offered a window, a column of such rows is as wide as the
/// window. Measured with the width left open, each row answers what its
/// words need, the column answers the widest, and the rows then fill
/// THAT.
#[derive(Clone)]
struct AsWideAsItsRows<C>(C);

impl<C: View<Arity = Single>> View for AsWideAsItsRows<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        self.0.render_into(ctx, out);
        // a hug gives back the width it does not need and takes the
        // HEIGHT it is offered — it was made for a region that scrolls.
        // A column offers a child that is not flexible down its own
        // height, so the hug stands in a column of one
        out.wrap_layout_from(mark, |node| LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: crate::layout::CrossAlign::Start,
            children: vec![LayoutNode::Hug { axis: Axis::Horizontal, child: Box::new(node) }],
            hints: Default::default(),
            action: None,
        });
    }
}

/// A button that shows one value and opens a list of the others:
/// `picker(self.look.binding(), [(Look::System, "System"), (Look::Dark, "Dark")])`.
///
/// [The module](self) has the keys, the roles and how it is made.
#[track_caller]
pub fn picker<T, O>(selection: Binding<T>, options: O) -> Picker<T>
where
    T: Clone + PartialEq + 'static,
    O: IntoChoices<T>,
{
    Picker { selection, choices: options.into_choices(), disabled: false, site: crate::provenance::here() }
}
