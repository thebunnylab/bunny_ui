# Controls

A control is a view bound to a value. The app owns the value; the control
shows it and writes it. Every control here takes a `Binding`, is reached by
the keyboard, and is announced to assistive technology by its role. All but
the two fields have a `.disabled(bool)`: a disabled control shows its value
and takes no key and no press.

```bash
cargo run -p bunny-ui --example controls
```

The example shows every control on one page, each with the value its binding
holds.

## The controls

| Control | Call | The binding holds |
| --- | --- | --- |
| Button | `button(text("Save"), move \|\| model.save())` | nothing: it runs an action |
| Checkbox | `checkbox(text("Remember me"), self.remember.binding())` | `bool` |
| Toggle | `toggle(text("Wi-Fi"), self.wifi.binding())` | `bool` |
| Segmented | `segmented(self.mode.binding(), [(Mode::List, "List"), (Mode::Grid, "Grid")])` | one of a few values |
| Tabs | `tab_view(self.tab.binding(), (tab(Tab::A, "General", a()), tab(Tab::B, "Advanced", b())))` | the page on show |
| Picker | `picker(self.look.binding(), [(Look::System, "System"), (Look::Dark, "Dark")])` | one of many values |
| Stepper | `stepper(self.guests.binding(), 1..=8)` | a number |
| Slider | `slider(self.level.binding(), range)` | `f64` |
| Text field | `text_field("Name", self.name.binding())` | `String` |
| Search field | `search_field(self.query.binding())` | `String` |
| Calendar | `calendar(self.due.binding())` | `Date` |
| Date picker | `date_picker(self.due.binding())` | `Date` |

`form!(…)` puts rows of controls on one card with a line between two rows, and
`labeled("Name", control)` is the row most settings want: the words on the
leading side and the control on the trailing side.

```rust
form!(
    labeled("Name", text_field("Jane Appleseed", self.name.binding())),
    toggle(text("Public profile"), self.public.binding()),
    labeled("Theme", picker(self.look.binding(), looks())),
    labeled("Due", date_picker(self.due.binding())),
)
```

## Which one

| The reader chooses | Use |
| --- | --- |
| yes or no, and it takes effect at once | `toggle` |
| yes or no, as a part of a form that is sent later | `checkbox` |
| one of two to five values that deserve to be seen | `segmented` |
| one of many values | `picker` |
| which page of the same place to look at | `tab_view` |
| a small number, a step at a time | `stepper` |
| a number in a range, where the place matters more than the digits | `slider` |
| a day | `date_picker`, or `calendar` when the page has the room |

## Choices

`segmented`, `tabs` and `picker` take their choices as pairs of a value and its
words. The value is any type that is `Clone + PartialEq`: an enum of the app, a
number, a string.

```rust
segmented(self.mode.binding(), [(Mode::List, "List"), (Mode::Grid, "Grid")])
```

A choice that cannot be chosen is written in full:

```rust
picker(self.plan.binding(), [
    Choice::new(Plan::Free, "Free"),
    Choice::new(Plan::Team, "Team").disabled(true),
])
```

A value that is none of the choices selects nothing. The control never writes
the binding to repair it.

## Dates

`Date` is a year, a month and a day of the Gregorian calendar, with no hour
and no time zone.

```rust
let due = Date::new(2026, 11, 12);
due.weekday();          // Weekday::Thursday
due.adding_days(30);    // 2026-12-12
due.adding_months(3);   // 2027-02-12
due.to_string();        // "2026-11-12", and "2026-11-12".parse() reads it
```

Every `Date` is a real day: `Date::new(2026, 2, 30)` is 28 February, and
`Date::checked` answers `None` for it. `DateNames::for_locale(&locale)` writes
a date in the reader's language and gives the day a week starts on.

`.range(first..=last)` on a calendar or a date picker gives the days that can
be chosen. `first..` and `..=last` leave one end open.

The framework has no clock yet. The day in UTC comes from the system's time:

```rust
let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
let today = Date::from_unix_days((seconds / 86_400) as i64);
```

## Keys

Tab goes from one control to the next. A control made of several targets is
one Tab stop, and the arrows move inside it: the segments, the tabs, the rows
of an open picker, the days of a calendar. [Keyboard
controls](keyboard-controls.md) has the table for each.

| Control | Keys |
| --- | --- |
| Button | Space, Enter |
| Checkbox, toggle | Space |
| Segmented, tabs | the arrows move and select; Home, End |
| Picker | Space, Enter or a vertical arrow opens; the arrows move; Space or Enter chooses; Escape closes |
| Stepper, slider | the arrows change the value; Page Up, Page Down; Home, End |
| Search field | the keys of a text field; Enter runs `.on_submit` |
| Calendar | the arrows move by a day or a week and choose; Page Up, Page Down by a month; Home, End |
| Date picker | Space or Enter opens; then the keys of the calendar; Enter or Escape closes |

## Assistive technology

Each control is announced by a role, with its name, its value and its state.
The macOS, Windows and Linux shells project them to the platform
([accessibility](accessibility.md)).

| Control | Role | Says |
| --- | --- | --- |
| Checkbox | `Checkbox` | checked or not |
| Toggle | `Switch` | on or off |
| Segment, row of a picker, day | `RadioButton` | selected or not |
| Tab | `Tab` | selected or not |
| Picker, date picker | `PopUpButton` | the value it shows; open or closed |
| Stepper | `Stepper` | the value and its range |
| Slider | `Slider` | the value and its range |
| Text field, search field | `TextField` | the text; named by its prompt or its row |

In a `labeled` row the words name the control, and are heard once.

## Words of the framework

A few controls write words of their own: the prompt of a search field, the
name of its clear button, the names of the two buttons of a calendar, the
months and the days. They come in sixteen languages, picked by the locale
([i18n](i18n.md)). `words::install` lets the app answer for the first four;
the names of the months and the days have no such hook yet.

## How they are made

All but two are ordinary views: a button, a stack, a grid, a popover, put
together as an app could. The stepper and the slider are custom elements,
because they paint themselves and take a drag. On the web the first kind is
real elements, and the second kind is a canvas island.

No control adds a colour to `Theme`. They wear the tokens that are there:
`accent` for what is selected, `control` for what can be pressed, `field` for
what takes text, `panel` for a card or a list.

## Not yet

- A picker does not jump to a row when the reader types its first letters.
- Tabs share the width of the bar equally; they do not take the width of
  their words.
- A stepper does not repeat while its button is held.
- The words of the rows of a form are not one column of one width.
- A calendar has no mark on today, and the framework has no hours.
- Escape does not empty a search field, and the two fields cannot be disabled.
- On the web the controls have no ARIA roles yet.
- iOS, Android and the web canvas have no screen reader projection yet.
