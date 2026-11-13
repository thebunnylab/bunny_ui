//! The calendar and the date picker: a month of days that chooses one,
//! and a button that shows the day and opens that month.
//!
//! ```ignore
//! calendar(self.due.binding())
//! date_picker(self.due.binding())
//! date_picker(self.due.binding()).range(today..)
//! ```
//!
//! The binding holds a [`Date`]. A calendar shows the month of that day,
//! with the day marked, and two buttons that show the month before and
//! the month after. A press on a day writes it. A date picker is a
//! button that shows the day in the reader's language, and a press opens
//! the same calendar under it.
//!
//! [`Calendar::range`] gives the days that can be chosen. The days out
//! of it are shown and cannot be pressed, and a month with no day in it
//! cannot be reached. A day that the binding holds out of the range is
//! shown as it is; the binding is never written to repair it.
//!
//! The names of the months and of the days, the order of a written date
//! and the day a week starts on come from the locale
//! ([`DateNames`]).
//!
//! ## Keys
//!
//! The two buttons are Tab stops, and the days are one more. On the days
//! an arrow moves by one day or one week and the day it reaches is
//! chosen at once, as in a set of radio buttons. At the edge of the
//! month the arrow goes on into the next one. Page Up and Page Down move
//! by one month and keep the day. Home and End go to the first and the
//! last day of the month.
//!
//! The list of a date picker opens with the keyboard on the chosen day.
//! Enter or Space on the chosen day closes the list, and so does a press
//! on it. Escape closes it too. The value is written while the reader
//! moves, so a list that closes has nothing to take back.
//!
//! ## Assistive technology
//!
//! Each day is a `RadioButton` named by its date in full, checked on the
//! chosen day. The button of a date picker is a
//! [`Role::PopUpButton`](crate::accessibility::Role) whose `value` is
//! the date it shows.
//!
//! ## How it is made
//!
//! Buttons in a grid of seven columns, in one group of the keyboard. The
//! days of one month stand in the order of the calendar, so a step of
//! seven in the group is a step of one week, and only a key that leaves
//! the month needs an action of its own
//! ([`CALENDAR_CONTEXT`](crate::action::CALENDAR_CONTEXT)).
//!
//! Not here yet: a mark on today (the framework has no clock), hours,
//! and a range of two days.

use std::ops::{Bound, RangeBounds};
use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context, LayoutDirection, Locale, State};
use motor::views::Edge;

use crate::action::{
    ActionId, CALENDAR_CONTEXT, CALENDAR_DOWN, CALENDAR_LEFT, CALENDAR_NEXT_MONTH, CALENDAR_PREVIOUS_MONTH,
    CALENDAR_RIGHT, CALENDAR_UP,
};
use crate::choice::Grouped;
use crate::date::{Date, DateNames};
use crate::erased::erased;
use crate::ext::ViewExt;
use crate::icon::house as symbol;
use crate::layout::{Color, ControlKind, GroupSpec, Side};
use crate::provenance::{NodeKind, Quiet, Site};
use crate::state_ext::StateExt;
use crate::view::{Component, NodeList, Single, View};
use crate::views::{Alignment, ControlSpec, button, control, empty, grid, hstack, icon, spacer, text, vstack};
use crate::words::{Word, Words};

/// A day, from edge to edge.
const CELL: f64 = 32.0;
const CELL_RADIUS: f64 = 6.0;
/// The row of the names of the days.
const HEADING: f64 = 20.0;
/// A month takes at most six weeks; the calendar keeps that height, so
/// it does not change size from one month to the next.
const WEEKS: f64 = 6.0;
/// Between the list's edge and the calendar in it.
const LIST_INSET: f64 = 8.0;
const LIST_RADIUS: f64 = 8.0;
const NOTHING: Color = Color { r: 0, g: 0, b: 0, a: 0 };

/// The first and the last day of `range`.
fn ends(range: impl RangeBounds<Date>) -> (Date, Date) {
    let low = match range.start_bound() {
        Bound::Included(day) => *day,
        Bound::Excluded(day) => day.adding_days(1),
        Bound::Unbounded => Date::MIN,
    };
    let high = match range.end_bound() {
        Bound::Included(day) => *day,
        Bound::Excluded(day) => day.adding_days(-1),
        Bound::Unbounded => Date::MAX,
    };
    (low, high)
}

/// A month of days that chooses one ([`calendar`]).
#[derive(Clone)]
pub struct Calendar {
    selection: Binding<Date>,
    range: (Date, Date),
    disabled: bool,
    site: Site,
    /// What a press on the chosen day does: the list of a date picker
    /// closes.
    confirm: Option<Rc<dyn Fn()>>,
    /// The days take the keyboard when the layer they are in opens.
    leads: bool,
}

impl Calendar {
    /// The days that can be chosen: `first..=last`, `first..` or
    /// `..=last`.
    #[must_use]
    pub fn range(mut self, range: impl RangeBounds<Date>) -> Self {
        self.range = ends(range);
        self
    }

    /// A disabled calendar shows its day and cannot be changed.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl View for Calendar {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        let locale = ctx.environment::<Locale>();
        let words = Words::for_locale(&locale);
        Month {
            selection: self.selection.clone(),
            range: self.range,
            disabled: self.disabled,
            site: self.site,
            confirm: self.confirm.clone(),
            leads: self.leads,
            names: DateNames::for_locale(&locale),
            previous: Arc::from(&*words.get(Word::PreviousMonth)),
            next: Arc::from(&*words.get(Word::NextMonth)),
            rtl: ctx.environment::<LayoutDirection>().is_rtl(),
        }
        .render_into(ctx, out);
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::Calendar, None);
    }
}

/// The calendar's own body: it holds how far the reader paged from the
/// month of the chosen day, so a press on a button of the month runs this
/// body and nothing of the app's.
#[derive(Clone)]
struct Month {
    selection: Binding<Date>,
    range: (Date, Date),
    disabled: bool,
    site: Site,
    confirm: Option<Rc<dyn Fn()>>,
    leads: bool,
    names: DateNames,
    previous: Arc<str>,
    next: Arc<str>,
    rtl: bool,
}

impl Month {
    /// A key that leaves the month: the date goes where `to` says, when
    /// that day can be chosen, and the calendar shows its month.
    fn key(&self, paged: State<i32>, to: impl Fn(Date) -> Date + 'static) -> impl Fn() + 'static {
        let selection = self.selection.clone();
        let (low, high) = self.range;
        move || {
            let from = selection.wrappedValue();
            let target = to(from);
            if target != from && low <= target && target <= high {
                selection.set(target);
                paged.set_if_changed(0);
            }
        }
    }
}

impl Component for Month {
    fn body(self) -> impl View {
        // The month on show is counted FROM the month of the chosen day.
        // A day written from outside moves the calendar with it, and no
        // second copy of the date can disagree with the binding.
        let paged = State::new(0i32);
        let theme = crate::theme::current();
        let names = self.names;
        let chosen = self.selection.wrappedValue();
        let shown = chosen.first_of_month().adding_months(paged.get()).first_of_month();
        let (low, high) = self.range;
        let enabled = !self.disabled;

        let after = shown.adding_months(1).first_of_month();
        let pager = |by: i32, forward: bool, word: &Arc<str>, open: bool| {
            // the arrow points where the month goes on the page
            let glyph = if forward != self.rtl { symbol::CHEVRON_RIGHT } else { symbol::CHEVRON_LEFT };
            button(icon(glyph).foreground_color(theme.fg_secondary), move || paged.set(paged.get() + by))
                .plain()
                .disabled(!open)
                .accessibility_label(&**word)
        };
        let header = hstack((
            text(names.month_title(shown)).bold(),
            spacer(),
            pager(-1, false, &self.previous, enabled && low < shown),
            pager(1, true, &self.next, enabled && shown < after && after <= high),
        ))
        .spacing(4.0)
        .padding_edge(Edge::Leading, 8.0)
        .padding_edge(Edge::Trailing, 4.0);

        let first = names.first_weekday();
        let heading: Vec<_> = (0..7)
            .map(|column| {
                text(names.weekday(first.adding(column)))
                    .font_size(11.0)
                    .foreground_color(theme.fg_secondary)
                    .frame(CELL, HEADING)
            })
            .collect();
        // a day is named by its date in full; the columns say nothing more
        let heading = grid(heading).columns(7).accessibility_hidden();

        // the first day stands under its own day of the week
        let lead = usize::from(shown.weekday().column(first));
        let blank = (lead > 0).then(|| empty().span(lead, 1));
        let cells: Vec<_> = (1..=shown.days_in_month())
            .map(|number| {
                let day = Date::new(shown.year(), shown.month(), number);
                let on = day == chosen;
                let open = enabled && low <= day && day <= high;
                let ink = match (on, open) {
                    (true, _) => Color::WHITE,
                    (false, true) => theme.fg,
                    (false, false) => theme.fg_faint,
                };
                let (rest, hovered, pressed) =
                    if on { (theme.accent, theme.accent, theme.accent) } else { (NOTHING, theme.row_hover, theme.row_pressed) };
                let face = text(number.to_string())
                    .foreground_color(ink)
                    .frame(CELL, CELL)
                    .background_color(rest)
                    .background_hovered(hovered)
                    .background_pressed(pressed)
                    .corner_radius(CELL_RADIUS);
                let selection = self.selection.clone();
                let confirm = self.confirm.clone();
                control(
                    face,
                    move || {
                        if selection.wrappedValue() != day {
                            selection.set(day);
                        } else if let Some(confirm) = &confirm {
                            confirm();
                        }
                        paged.set_if_changed(0);
                    },
                    ControlSpec {
                        kind: ControlKind::Radio,
                        checked: Some(on),
                        disabled: !open,
                        plain: true,
                        site: self.site,
                        shows: None,
                    },
                )
                .accessibility_label(names.date(day))
            })
            .collect();
        // the days of a month are a window on a longer line: the arrows
        // do not go round at its ends, they leave by the actions below
        let spec = GroupSpec { columns: 7, selects: true, leads: self.leads, round: false };
        // in a page that reads right to left the days run that way too
        let (back, on): (ActionId, ActionId) =
            if self.rtl { (CALENDAR_RIGHT, CALENDAR_LEFT) } else { (CALENDAR_LEFT, CALENDAR_RIGHT) };
        let days = Grouped { content: grid((blank, cells)).columns(7), spec }
            .key_context_focused(CALENDAR_CONTEXT)
            .on_action(CALENDAR_UP, self.key(paged, |day| day.adding_days(-7)))
            .on_action(CALENDAR_DOWN, self.key(paged, |day| day.adding_days(7)))
            .on_action(back, self.key(paged, |day| day.adding_days(-1)))
            .on_action(on, self.key(paged, |day| day.adding_days(1)))
            // a month before or after, on the same day; the last day that
            // can be chosen where that one cannot
            .on_action(CALENDAR_PREVIOUS_MONTH, self.key(paged, move |day| day.adding_months(-1).max(low)))
            .on_action(CALENDAR_NEXT_MONTH, self.key(paged, move |day| day.adding_months(1).min(high)))
            .frame_aligned(7.0 * CELL, WEEKS * CELL, Alignment::TopLeading);

        Quiet(vstack((header, heading, days)).spacing(4.0).frame_width(7.0 * CELL).opacity(if enabled { 1.0 } else { 0.5 }))
    }
}

/// A month of days that chooses one: `calendar(self.due.binding())`.
///
/// [The module](self) has the keys, the roles and how it is made.
#[track_caller]
pub fn calendar(selection: Binding<Date>) -> Calendar {
    Calendar {
        selection,
        range: (Date::MIN, Date::MAX),
        disabled: false,
        site: crate::provenance::here(),
        confirm: None,
        leads: false,
    }
}

/// A button that shows a day and opens a calendar ([`date_picker`]).
#[derive(Clone)]
pub struct DatePicker {
    selection: Binding<Date>,
    range: (Date, Date),
    disabled: bool,
    site: Site,
}

impl DatePicker {
    /// The days that can be chosen: `first..=last`, `first..` or
    /// `..=last`.
    #[must_use]
    pub fn range(mut self, range: impl RangeBounds<Date>) -> Self {
        self.range = ends(range);
        self
    }

    /// A disabled date picker shows its day and cannot open.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl View for DatePicker {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        Opened {
            selection: self.selection.clone(),
            range: self.range,
            disabled: self.disabled,
            site: self.site,
            names: DateNames::for_locale(&ctx.environment::<Locale>()),
        }
        .render_into(ctx, out);
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::DatePicker, None);
    }
}

/// The date picker's own body: it holds whether the list is open.
#[derive(Clone)]
struct Opened {
    selection: Binding<Date>,
    range: (Date, Date),
    disabled: bool,
    site: Site,
    names: DateNames,
}

impl Component for Opened {
    fn body(self) -> impl View {
        let open = State::new(false);
        let shown: Arc<str> = Arc::from(self.names.date(self.selection.wrappedValue()));
        let label = hstack((text(&*shown), icon(symbol::CHEVRON_DOWN))).spacing(6.0);
        let button = control(
            label,
            move || open.set(!open.get()),
            ControlSpec {
                kind: ControlKind::PopUp,
                checked: None,
                disabled: self.disabled,
                plain: false,
                site: self.site,
                shows: Some((shown, open.get())),
            },
        );
        let month = Calendar {
            selection: self.selection,
            range: self.range,
            disabled: false,
            site: self.site,
            confirm: Some(Rc::new(move || open.set(false))),
            leads: true,
        };
        Quiet(button.popover(open.binding(), Side::Bottom, move |_| {
            let theme = crate::theme::current();
            let list = month
                .clone()
                .padding_length(LIST_INSET)
                .background_color(theme.panel)
                .border(theme.border, 1.0)
                .corner_radius(LIST_RADIUS);
            erased(Quiet(list))
        }))
    }
}

/// A button that shows a day and opens a calendar:
/// `date_picker(self.due.binding())`.
///
/// [The module](self) has the keys, the roles and how it is made.
#[track_caller]
pub fn date_picker(selection: Binding<Date>) -> DatePicker {
    DatePicker { selection, range: (Date::MIN, Date::MAX), disabled: false, site: crate::provenance::here() }
}
