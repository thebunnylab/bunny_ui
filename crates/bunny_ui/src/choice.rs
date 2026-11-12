//! One value out of a few: the choices, and the segmented control.
//!
//! ```ignore
//! segmented(self.mode.binding(), [
//!     (Mode::List, "List"),
//!     (Mode::Grid, "Grid"),
//!     (Mode::Columns, "Columns"),
//! ])
//! ```
//!
//! The binding holds the value. Each choice is a value and the words that
//! show it. A press on a segment writes its value to the binding. A value
//! that is not one of the choices selects nothing, and the control never
//! writes the binding to repair it.
//!
//! ## Keys
//!
//! The control is ONE Tab stop: Tab lands on the selected segment and
//! leaves the control whole. The arrows move to the next segment and
//! select it, and go round at the ends. Home and End jump to the first
//! and the last. Space and Enter select the segment that has the
//! keyboard. In a right-to-left scene the horizontal arrows swap.
//!
//! ## Assistive technology
//!
//! Each segment is a [`Role::RadioButton`](crate::accessibility::Role)
//! named by its words, with `checked` true on the selected one. The
//! activate action selects it.
//!
//! ## How it is made
//!
//! Segments of equal width in a grid, and a thumb behind them in a grid
//! of the same tracks. The thumb is the one view that moves, and the
//! spring a view asks for with `.animated` slides it. No layout node and
//! no draw command are added, so the control draws on every renderer and
//! lowers on the web as a row of buttons.

use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context};

use crate::anim::Spring;
use crate::ext::ViewExt;
use crate::layout::{
    Address, ControlKind, ElementHints, GroupSpec, LayoutNode, UnitPoint, VisualProps,
};
use crate::provenance::{NodeKind, Site};
use crate::view::{NodeList, Single, View};
use crate::views::{Alignment, ControlSpec, control, empty, grid, text};

/// One of the values a control chooses between, and the words that show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice<T> {
    value: T,
    label: Arc<str>,
    disabled: bool,
}

impl<T> Choice<T> {
    /// A choice of `value`, shown as `label`. A pair `(value, "label")`
    /// is a choice too, and is the short form.
    pub fn new(value: T, label: impl Into<Arc<str>>) -> Self {
        Self { value, label: label.into(), disabled: false }
    }

    /// A disabled choice is shown and cannot be selected.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The value this choice writes.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// The words that show it.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether the choice is shown and cannot be selected.
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }
}

impl<T, S: Into<Arc<str>>> From<(T, S)> for Choice<T> {
    fn from((value, label): (T, S)) -> Self {
        Self::new(value, label)
    }
}

/// The choices a control is given: an array or a `Vec` of pairs
/// `(value, "label")`, or of [`Choice`] values.
pub trait IntoChoices<T> {
    /// The choices, in the order they are shown.
    fn into_choices(self) -> Rc<[Choice<T>]>;
}

impl<T, C: Into<Choice<T>>, const N: usize> IntoChoices<T> for [C; N] {
    fn into_choices(self) -> Rc<[Choice<T>]> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T, C: Into<Choice<T>>> IntoChoices<T> for Vec<C> {
    fn into_choices(self) -> Rc<[Choice<T>]> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T> IntoChoices<T> for Rc<[Choice<T>]> {
    fn into_choices(self) -> Rc<[Choice<T>]> {
        self
    }
}

/// The controls inside `content` travel as one group
/// ([`GroupSpec`]): one Tab stop, and the arrows move inside it.
#[derive(Clone)]
pub(crate) struct Grouped<C> {
    pub content: C,
    pub spec: GroupSpec,
}

impl<C: View<Arity = Single>> View for Grouped<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        self.content.render_into(ctx, out);
        let group = Some(self.spec);
        match out.base_from(mark) {
            // the group rides on the style the content already wears
            LayoutNode::Styled { hints, action: None, .. } if hints.is_empty() => {
                hints.address = Some(Address::shared(Address { group, ..Address::default() }));
            }
            _ => out.wrap_layout_from(mark, |node| LayoutNode::Styled {
                props: VisualProps::default().shared(),
                child: Box::new(node),
                hints: ElementHints {
                    address: Some(Address::shared(Address { group, ..Address::default() })),
                    ..ElementHints::default()
                },
                action: None,
            }),
        }
    }
}

/// A segment, from edge to edge.
const SEGMENT_HEIGHT: f64 = 26.0;
/// Between the control's edge and its segments.
const INSET: f64 = 2.0;
const RADIUS: f64 = 8.0;

/// A row of segments that chooses one value ([`segmented`]).
#[derive(Clone)]
pub struct Segmented<T: 'static> {
    selection: Binding<T>,
    choices: Rc<[Choice<T>]>,
    disabled: bool,
    site: Site,
}

impl<T: 'static> Segmented<T> {
    /// A disabled control shows its selection and cannot change: no
    /// segment takes focus, a press or an action.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<T: Clone + PartialEq + 'static> View for Segmented<T> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        crate::provenance::quiet(|| {
            Bar {
                selection: &self.selection,
                choices: &self.choices,
                disabled: self.disabled,
                tabs: false,
                site: self.site,
            }
            .render(ctx, out);
        });
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::Segmented, None);
    }
}

/// A tab, from edge to edge.
const TAB_HEIGHT: f64 = 32.0;
/// The line under the selected tab.
const TAB_RULE: f64 = 2.0;

/// The row a segmented control and a tab bar are both made of: members
/// of equal width in a grid, one of them selected, and a mark that slides
/// to the selected track.
pub(crate) struct Bar<'a, T: 'static> {
    pub selection: &'a Binding<T>,
    pub choices: &'a [Choice<T>],
    pub disabled: bool,
    /// Tabs wear a line under the selected member; segments a thumb behind it.
    pub tabs: bool,
    pub site: Site,
}

impl<T: Clone + PartialEq + 'static> Bar<'_, T> {
    pub(crate) fn render(&self, ctx: &Context, out: &mut NodeList) {
        let current = self.selection.wrappedValue();
        let selected = self.choices.iter().position(|choice| choice.value == current);
        let count = self.choices.len().max(1);
        let theme = crate::theme::current();
        let height = if self.tabs { TAB_HEIGHT } else { SEGMENT_HEIGHT };
        let members: Vec<_> = self
            .choices
            .iter()
            .enumerate()
            .map(|(index, choice)| {
                let on = selected == Some(index);
                let selection = self.selection.clone();
                let value = choice.value.clone();
                let label = text(&*choice.label)
                    .foreground_color(if on { theme.fg } else { theme.fg_secondary })
                    .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center)
                    .frame_height(height);
                control(
                    label,
                    move || selection.set(value.clone()),
                    ControlSpec {
                        kind: if self.tabs { ControlKind::Tab } else { ControlKind::Radio },
                        checked: Some(on),
                        disabled: self.disabled || choice.disabled,
                        plain: true,
                        site: self.site,
                        shows: None,
                    },
                )
            })
            .collect();
        // the mark stands in the selected track of a grid with the same
        // tracks; it keeps its place in the tuple, so it is the same view
        // from one selection to the next and the spring carries it there
        let lead = selected.filter(|index| *index > 0).map(|index| empty().span(index, 1));
        let mark = |height: f64| empty().frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center).frame_height(height);
        let opacity = if self.disabled { 0.5 } else { 1.0 };
        let spec = GroupSpec { columns: u8::try_from(count).unwrap_or(u8::MAX), selects: true };
        if self.tabs {
            let line = selected.map(|_| mark(TAB_RULE).background_color(theme.accent).animated(Spring::snappy()));
            let bar = grid(members)
                .columns(count)
                .background(UnitPoint::BOTTOM, mark(1.0).background_color(theme.divider))
                .overlay(UnitPoint::BOTTOM, grid((lead, line)).columns(count))
                .opacity(opacity);
            Grouped { content: bar, spec }.render_into(ctx, out);
        } else {
            let thumb = selected.map(|_| {
                mark(SEGMENT_HEIGHT)
                    .background_color(theme.panel)
                    .corner_radius(RADIUS - INSET)
                    .animated(Spring::snappy())
            });
            let bar = grid(members)
                .columns(count)
                .background(UnitPoint::CENTER, grid((lead, thumb)).columns(count))
                .padding_length(INSET)
                .background_color(theme.control)
                .corner_radius(RADIUS)
                .opacity(opacity);
            Grouped { content: bar, spec }.render_into(ctx, out);
        }
    }
}

/// A row of segments that chooses one value:
/// `segmented(self.mode.binding(), [(Mode::List, "List"), (Mode::Grid, "Grid")])`.
///
/// [The module](self) has the keys, the roles and how it is made.
#[track_caller]
pub fn segmented<T, O>(selection: Binding<T>, options: O) -> Segmented<T>
where
    T: Clone + PartialEq + 'static,
    O: IntoChoices<T>,
{
    Segmented { selection, choices: options.into_choices(), disabled: false, site: crate::provenance::here() }
}
