//! `calendar(binding)` and `date_picker(binding)`: a month of days that
//! chooses one, and a button that opens it.

use bunny_ui::accessibility::Role;
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::prelude::*;
use bunny_ui::provenance::NodeKind;
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size {
    width: 360.0,
    height: 420.0,
};

/// A calendar with a field after it.
#[derive(Clone, Copy)]
struct Inline {
    due: State<Date>,
    range: State<(Date, Date)>,
    locked: State<bool>,
    note: State<String>,
}

impl Inline {
    fn on(year: i32, month: u8, day: u8) -> Self {
        Self {
            due: State::new(Date::new(year, month, day)),
            range: State::new((Date::MIN, Date::MAX)),
            locked: State::new(false),
            note: State::new(String::new()),
        }
    }

    fn between(self, low: Date, high: Date) -> Self {
        self.range.set((low, high));
        self
    }

    /// The app writes another day, and a frame shows it: the next key
    /// finds the keyboard where that frame put it.
    fn write(&self, runtime: &Runtime, year: i32, month: u8, day: u8) {
        self.due.set(Date::new(year, month, day));
        runtime.display_frame(self, SIZE);
    }
}

impl Component for Inline {
    fn body(self) -> impl View {
        let (low, high) = self.range.get();
        vstack!(
            calendar(self.due.binding()).range(low..=high).disabled(self.locked.get()).id("due"),
            text_field("Note", self.note.binding()).id("note"),
        )
    }
}

/// A date picker with a field after it.
#[derive(Clone, Copy)]
struct Field {
    due: State<Date>,
    locked: State<bool>,
    note: State<String>,
}

impl Field {
    fn on(year: i32, month: u8, day: u8) -> Self {
        Self {
            due: State::new(Date::new(year, month, day)),
            locked: State::new(false),
            note: State::new(String::new()),
        }
    }
}

impl Component for Field {
    fn body(self) -> impl View {
        vstack!(
            date_picker(self.due.binding()).disabled(self.locked.get()).id("due"),
            text_field("Note", self.note.binding()).id("note"),
        )
    }
}

/// One element as assistive technology hears it.
#[derive(Clone, Debug, PartialEq)]
struct Heard {
    role: Role,
    label: String,
    value: Option<String>,
    bounds: Rect,
    focused: bool,
    enabled: bool,
    checked: Option<bool>,
    expanded: Option<bool>,
}

fn runtime() -> Runtime {
    let runtime = Runtime::new();
    runtime.set_accessibility_enabled(true);
    runtime
}

/// A frame, and what it holds.
fn heard(runtime: &Runtime, view: &impl View) -> Vec<Heard> {
    runtime.display_frame(view, SIZE);
    runtime
        .accessibility_tree()
        .nodes()
        .iter()
        .map(|node| Heard {
            role: node.role,
            label: node.label.to_string(),
            value: node.value.as_deref().map(str::to_string),
            bounds: node.bounds,
            focused: node.focused,
            enabled: node.enabled,
            checked: node.checked,
            expanded: node.expanded,
        })
        .collect()
}

fn days(runtime: &Runtime, view: &impl View) -> Vec<Heard> {
    heard(runtime, view).into_iter().filter(|node| node.role == Role::RadioButton).collect()
}

fn named(runtime: &Runtime, view: &impl View, label: &str) -> Heard {
    heard(runtime, view)
        .into_iter()
        .find(|node| node.label == label)
        .unwrap_or_else(|| panic!("nothing is named {label:?}"))
}

/// The name of what holds the keyboard.
fn keyboard(runtime: &Runtime, view: &impl View) -> Option<String> {
    heard(runtime, view).into_iter().find(|node| node.focused).map(|node| node.label)
}

/// The days that are checked, by name.
fn chosen(runtime: &Runtime, view: &impl View) -> Vec<String> {
    days(runtime, view).into_iter().filter(|day| day.checked == Some(true)).map(|day| day.label).collect()
}

fn lines(runtime: &Runtime, view: &impl View) -> Vec<String> {
    runtime
        .display_frame(view, SIZE)
        .as_slice()
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, .. } => Some(content[range.0..range.1].to_string()),
            _ => None,
        })
        .collect()
}

fn shows(runtime: &Runtime, view: &impl View, title: &str) -> bool {
    lines(runtime, view).iter().any(|line| line == title)
}

fn press(runtime: &Runtime, rect: Rect) {
    let (x, y) = (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
}

/// A key the way a shell sends it: to what holds the keyboard, then to
/// the keymap.
fn key(runtime: &Runtime, key: Key) -> bool {
    runtime.offer_key(KeyPattern::key(key))
}

/// Tab until the keyboard is on a day.
fn onto_the_days(runtime: &Runtime, view: &impl View) {
    runtime.display_frame(view, SIZE);
    for _ in 0..4 {
        assert!(key(runtime, Key::Tab));
        if heard(runtime, view).iter().any(|node| node.focused && node.role == Role::RadioButton) {
            return;
        }
    }
    panic!("Tab did not reach the days");
}

/// The key, then the day the binding holds, and that the keyboard and
/// the mark are both on it.
fn after(runtime: &Runtime, view: &Inline, pressed: Key) -> String {
    key(runtime, pressed);
    let day = view.due.get();
    let name = DateNames::for_locale(&Locale::default()).date(day);
    assert_eq!(chosen(runtime, view), std::slice::from_ref(&name), "the mark is on the day the binding holds");
    assert_eq!(keyboard(runtime, view), Some(name), "and so is the keyboard");
    day.to_string()
}

#[test]
fn it_shows_the_month_of_the_day_with_each_day_under_its_day_of_the_week() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    let shown = lines(&runtime, &view);
    assert!(shown.iter().any(|line| line == "November 2026"), "{shown:?}");
    let heading: Vec<_> = shown.iter().skip_while(|line| *line != "Sun").take(7).collect();
    assert_eq!(heading, ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]);

    let days = days(&runtime, &view);
    assert_eq!(days.len(), 30);
    assert_eq!(days[0].label, "November 1, 2026");
    assert_eq!(days[29].label, "November 30, 2026");
    assert_eq!(chosen(&runtime, &view), ["November 12, 2026"]);
    // 1 November 2026 is a Sunday: the first column, with 8 under it and 7 at the end of its row
    let cell = days[0].bounds.size;
    assert_eq!((cell.width, cell.height), (32.0, 32.0));
    assert_eq!(days[7].bounds.origin.x, days[0].bounds.origin.x);
    assert_eq!(days[7].bounds.origin.y, days[0].bounds.origin.y + 32.0);
    assert_eq!(days[6].bounds.origin.x, days[0].bounds.origin.x + 6.0 * 32.0);
    assert_eq!(days[6].bounds.origin.y, days[0].bounds.origin.y);

    // 1 October 2026 is a Thursday: four empty places before it
    view.due.set(Date::new(2026, 10, 9));
    let october = self::days(&runtime, &view);
    assert_eq!(october.len(), 31);
    assert_eq!(october[0].bounds.origin.x, days[4].bounds.origin.x);
    assert_eq!(october[3].bounds.origin.x, days[0].bounds.origin.x, "the 4th is a Sunday");
    assert_eq!(october[3].bounds.origin.y, october[0].bounds.origin.y + 32.0);
}

#[test]
fn the_calendar_keeps_its_height_from_a_month_of_four_weeks_to_one_of_six() {
    let runtime = runtime();
    // February 2026 starts on a Sunday and has four weeks; August 2026 starts on a Saturday and has six
    let view = Inline::on(2026, 2, 14);
    let short = named(&runtime, &view, "Note").bounds;
    view.due.set(Date::new(2026, 8, 14));
    let long = named(&runtime, &view, "Note").bounds;
    assert_eq!(short, long, "what stands under the calendar does not move");
    let august = days(&runtime, &view);
    assert_eq!(august[30].bounds.origin.y, august[0].bounds.origin.y + 5.0 * 32.0, "the 31st is in the sixth week");
    assert!(august[30].bounds.origin.y + 32.0 <= long.origin.y);
}

#[test]
fn a_press_on_a_day_writes_it() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    press(&runtime, named(&runtime, &view, "November 20, 2026").bounds);
    assert_eq!(view.due.get(), Date::new(2026, 11, 20));
    assert_eq!(chosen(&runtime, &view), ["November 20, 2026"]);
}

#[test]
fn tab_reaches_the_two_buttons_then_the_days_as_one_stop() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    runtime.display_frame(&view, SIZE);
    let mut stops = Vec::new();
    for _ in 0..4 {
        assert!(key(&runtime, Key::Tab));
        stops.push(keyboard(&runtime, &view).expect("something holds the keyboard"));
    }
    assert_eq!(stops, ["Previous month", "Next month", "November 12, 2026", "Note"]);
}

#[test]
fn an_arrow_moves_by_a_day_or_a_week_and_chooses_the_day_it_reaches() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    onto_the_days(&runtime, &view);
    assert_eq!(after(&runtime, &view, Key::Right), "2026-11-13");
    assert_eq!(after(&runtime, &view, Key::Down), "2026-11-20");
    assert_eq!(after(&runtime, &view, Key::Left), "2026-11-19");
    assert_eq!(after(&runtime, &view, Key::Up), "2026-11-12");
    // a row ends and the next day is the first of the row under it
    view.write(&runtime, 2026, 11, 14);
    assert_eq!(after(&runtime, &view, Key::Right), "2026-11-15");
    assert_eq!(after(&runtime, &view, Key::Home), "2026-11-01");
    assert_eq!(after(&runtime, &view, Key::End), "2026-11-30");
}

#[test]
fn an_arrow_at_the_edge_of_the_month_goes_on_into_the_next_one() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 3);
    onto_the_days(&runtime, &view);
    assert_eq!(after(&runtime, &view, Key::Up), "2026-10-27");
    assert!(shows(&runtime, &view, "October 2026"));
    assert_eq!(after(&runtime, &view, Key::Down), "2026-11-03");
    assert!(shows(&runtime, &view, "November 2026"));

    view.write(&runtime, 2026, 11, 28);
    assert_eq!(after(&runtime, &view, Key::Down), "2026-12-05");
    view.write(&runtime, 2026, 11, 1);
    assert_eq!(after(&runtime, &view, Key::Left), "2026-10-31");
    assert_eq!(after(&runtime, &view, Key::Right), "2026-11-01");
    view.write(&runtime, 2026, 12, 31);
    assert_eq!(after(&runtime, &view, Key::Right), "2027-01-01");
    assert!(shows(&runtime, &view, "January 2027"));
}

#[test]
fn page_up_and_page_down_move_by_a_month_and_keep_the_day() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    onto_the_days(&runtime, &view);
    assert_eq!(after(&runtime, &view, Key::PageDown), "2026-12-12");
    assert_eq!(after(&runtime, &view, Key::PageUp), "2026-11-12");
    // the 31st, in a month that has none: the keyboard was on a day the next month does not have
    view.write(&runtime, 2026, 1, 31);
    assert_eq!(after(&runtime, &view, Key::PageDown), "2026-02-28");
    assert_eq!(after(&runtime, &view, Key::PageDown), "2026-03-28");
    view.write(&runtime, 2024, 3, 31);
    assert_eq!(after(&runtime, &view, Key::PageUp), "2024-02-29");
}

#[test]
fn the_keys_of_the_calendar_are_its_own_only_while_the_keyboard_is_on_a_day() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    runtime.display_frame(&view, SIZE);
    assert!(key(&runtime, Key::Tab), "the first button");
    assert_eq!(keyboard(&runtime, &view).as_deref(), Some("Previous month"));
    for pressed in [Key::Left, Key::Up, Key::PageDown] {
        key(&runtime, pressed);
        runtime.display_frame(&view, SIZE);
        assert_eq!(view.due.get(), Date::new(2026, 11, 12), "{pressed:?} on a button moves no date");
    }
}

#[test]
fn the_buttons_show_another_month_and_leave_the_date() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    press(&runtime, named(&runtime, &view, "Next month").bounds);
    assert!(shows(&runtime, &view, "December 2026"));
    assert_eq!(view.due.get(), Date::new(2026, 11, 12), "a look at another month is not a choice");
    assert_eq!(chosen(&runtime, &view), [""; 0], "the chosen day is not in this month");
    assert_eq!(days(&runtime, &view).len(), 31);

    press(&runtime, named(&runtime, &view, "Previous month").bounds);
    press(&runtime, named(&runtime, &view, "Previous month").bounds);
    assert!(shows(&runtime, &view, "October 2026"));
    press(&runtime, named(&runtime, &view, "October 5, 2026").bounds);
    assert_eq!(view.due.get(), Date::new(2026, 10, 5));
    assert!(shows(&runtime, &view, "October 2026"), "the month of the day that was pressed stays");

    // a day written from outside brings its month
    view.due.set(Date::new(2027, 3, 9));
    assert!(shows(&runtime, &view, "March 2027"));
    assert_eq!(chosen(&runtime, &view), ["March 9, 2027"]);
}

#[test]
fn a_day_out_of_the_range_cannot_be_chosen_and_a_month_out_of_it_cannot_be_reached() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12).between(Date::new(2026, 11, 5), Date::new(2026, 12, 20));
    let before = named(&runtime, &view, "November 4, 2026");
    assert!(!before.enabled);
    assert!(named(&runtime, &view, "November 5, 2026").enabled);
    press(&runtime, before.bounds);
    assert_eq!(view.due.get(), Date::new(2026, 11, 12));
    assert!(!named(&runtime, &view, "Previous month").enabled, "no day of October can be chosen");
    assert!(named(&runtime, &view, "Next month").enabled);

    onto_the_days(&runtime, &view);
    assert_eq!(after(&runtime, &view, Key::Up), "2026-11-05");
    assert_eq!(after(&runtime, &view, Key::Up), "2026-11-05", "a week before is out of the range");
    assert_eq!(after(&runtime, &view, Key::Left), "2026-11-05");
    assert_eq!(after(&runtime, &view, Key::Home), "2026-11-05", "the first day that can be chosen");
    assert_eq!(after(&runtime, &view, Key::PageUp), "2026-11-05");
    view.write(&runtime, 2026, 11, 25);
    assert_eq!(after(&runtime, &view, Key::PageDown), "2026-12-20", "the last day that can be chosen");
    assert!(!named(&runtime, &view, "Next month").enabled);
    assert!(!named(&runtime, &view, "December 21, 2026").enabled);
}

#[test]
fn a_range_of_a_few_days_over_two_months_is_one_line_to_the_arrows() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 29).between(Date::new(2026, 11, 28), Date::new(2026, 12, 4));
    onto_the_days(&runtime, &view);
    assert_eq!(after(&runtime, &view, Key::Right), "2026-11-30");
    assert_eq!(after(&runtime, &view, Key::Right), "2026-12-01", "three days are not a ring");
    assert_eq!(after(&runtime, &view, Key::Down), "2026-12-01", "a week on is out of the range");
    assert_eq!(after(&runtime, &view, Key::Left), "2026-11-30");
    assert_eq!(after(&runtime, &view, Key::Up), "2026-11-30");
}

#[test]
fn a_disabled_calendar_shows_its_day_and_takes_no_key_and_no_press() {
    let runtime = runtime();
    let view = Inline::on(2026, 11, 12);
    view.locked.set(true);
    let nodes = heard(&runtime, &view);
    assert!(nodes.iter().filter(|node| node.role != Role::TextField && node.role != Role::Text).all(|node| !node.enabled));
    assert_eq!(chosen(&runtime, &view), ["November 12, 2026"]);
    press(&runtime, named(&runtime, &view, "November 20, 2026").bounds);
    press(&runtime, named(&runtime, &view, "Next month").bounds);
    assert_eq!(view.due.get(), Date::new(2026, 11, 12));
    assert!(shows(&runtime, &view, "November 2026"));
    assert!(key(&runtime, Key::Tab));
    assert_eq!(keyboard(&runtime, &view).as_deref(), Some("Note"), "the only stop left");
}

#[test]
fn the_names_and_the_first_day_of_the_week_are_the_readers() {
    let runtime = runtime();
    runtime.set_locale(Some(Locale::new("de-DE")));
    let view = Inline::on(2026, 11, 12);
    let shown = lines(&runtime, &view);
    assert!(shown.iter().any(|line| line == "November 2026"));
    let heading: Vec<_> = shown.iter().skip_while(|line| *line != "Mo").take(7).collect();
    assert_eq!(heading, ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"]);
    let days = days(&runtime, &view);
    assert_eq!(days[11].label, "12. November 2026");
    // the 1st is a Sunday: the LAST column where a week starts on Monday
    assert_eq!(days[0].bounds.origin.x, days[1].bounds.origin.x + 6.0 * 32.0);
    assert_eq!(days[1].bounds.origin.y, days[0].bounds.origin.y + 32.0);
    assert_eq!(named(&runtime, &view, "Vorheriger Monat").role, Role::Button);
    assert_eq!(named(&runtime, &view, "Nächster Monat").role, Role::Button);

    runtime.set_locale(Some(Locale::new("pt-BR")));
    assert!(shows(&runtime, &view, "novembro de 2026"));
    assert_eq!(chosen(&runtime, &view), ["12 de novembro de 2026"]);
    runtime.set_locale(Some(Locale::new("ja-JP")));
    assert!(shows(&runtime, &view, "2026年11月"));
}

#[test]
fn right_to_left_the_days_run_from_the_right_and_the_arrows_follow() {
    let runtime = runtime();
    runtime.set_layout_direction(Some(LayoutDirection::RightToLeft));
    let view = Inline::on(2026, 11, 12);
    let days = days(&runtime, &view);
    assert_eq!(days[1].bounds.origin.x, days[0].bounds.origin.x - 32.0, "the 2nd is at the left of the 1st");
    let (previous, next) = (named(&runtime, &view, "Previous month").bounds, named(&runtime, &view, "Next month").bounds);
    assert!(next.origin.x < previous.origin.x, "the buttons change sides");

    onto_the_days(&runtime, &view);
    assert_eq!(after(&runtime, &view, Key::Left), "2026-11-13", "left is forward");
    assert_eq!(after(&runtime, &view, Key::Right), "2026-11-12");
    view.write(&runtime, 2026, 11, 30);
    assert_eq!(after(&runtime, &view, Key::Left), "2026-12-01");
    assert_eq!(after(&runtime, &view, Key::Right), "2026-11-30");
}

fn the_button(runtime: &Runtime, view: &Field) -> Heard {
    heard(runtime, view).into_iter().find(|node| node.role == Role::PopUpButton).expect("the date picker's button")
}

fn is_open(runtime: &Runtime, view: &Field) -> bool {
    the_button(runtime, view).expanded == Some(true)
}

#[test]
fn the_button_of_a_date_picker_shows_the_day_in_words() {
    let runtime = runtime();
    let view = Field::on(2026, 11, 12);
    assert!(lines(&runtime, &view).iter().any(|line| line == "November 12, 2026"));
    let button = the_button(&runtime, &view);
    assert_eq!(button.value.as_deref(), Some("November 12, 2026"));
    assert_eq!(button.expanded, Some(false));
    assert!(days(&runtime, &view).is_empty(), "the calendar is not there until the button opens it");

    view.due.set(Date::new(2027, 1, 2));
    assert_eq!(the_button(&runtime, &view).value.as_deref(), Some("January 2, 2027"));
    runtime.set_locale(Some(Locale::new("ru")));
    assert_eq!(the_button(&runtime, &view).value.as_deref(), Some("02.01.2027"));
}

#[test]
fn the_list_opens_with_the_keyboard_on_the_chosen_day_and_the_arrows_write_as_they_move() {
    let runtime = runtime();
    let view = Field::on(2026, 11, 12);
    runtime.display_frame(&view, SIZE);
    assert!(key(&runtime, Key::Tab));
    assert!(key(&runtime, Key::Char(' ')));
    assert!(is_open(&runtime, &view));
    assert_eq!(days(&runtime, &view).len(), 30);
    assert_eq!(keyboard(&runtime, &view).as_deref(), Some("November 12, 2026"), "not on the buttons before the days");

    key(&runtime, Key::Right);
    assert_eq!(view.due.get(), Date::new(2026, 11, 13));
    assert_eq!(the_button(&runtime, &view).value.as_deref(), Some("November 13, 2026"), "the button shows it at once");
    assert!(is_open(&runtime, &view));
    key(&runtime, Key::PageDown);
    assert_eq!(view.due.get(), Date::new(2026, 12, 13));
    assert_eq!(keyboard(&runtime, &view).as_deref(), Some("December 13, 2026"));

    // Enter on the chosen day closes the list, and the keyboard is on the button again
    assert!(key(&runtime, Key::Enter));
    assert!(!is_open(&runtime, &view));
    assert_eq!(view.due.get(), Date::new(2026, 12, 13));
    assert!(the_button(&runtime, &view).focused);
}

#[test]
fn escape_closes_the_list_and_tab_stays_in_it() {
    let runtime = runtime();
    let view = Field::on(2026, 11, 12);
    runtime.display_frame(&view, SIZE);
    assert!(key(&runtime, Key::Tab));
    assert!(key(&runtime, Key::Char(' ')));
    assert!(is_open(&runtime, &view));
    let mut stops = Vec::new();
    for _ in 0..3 {
        assert!(key(&runtime, Key::Tab));
        stops.push(keyboard(&runtime, &view).expect("the keyboard is in the list"));
    }
    assert_eq!(stops, ["Previous month", "Next month", "November 12, 2026"]);

    key(&runtime, Key::Left);
    assert!(key(&runtime, Key::Escape));
    assert!(!is_open(&runtime, &view));
    assert_eq!(view.due.get(), Date::new(2026, 11, 11), "the day the arrows wrote stays written");
    assert!(the_button(&runtime, &view).focused);
}

#[test]
fn a_press_on_a_day_chooses_it_and_a_press_on_the_chosen_day_closes_the_list() {
    let runtime = runtime();
    let view = Field::on(2026, 11, 12);
    press(&runtime, the_button(&runtime, &view).bounds);
    assert!(is_open(&runtime, &view));
    press(&runtime, named(&runtime, &view, "November 20, 2026").bounds);
    assert_eq!(view.due.get(), Date::new(2026, 11, 20));
    assert!(is_open(&runtime, &view), "the reader can still change the day");
    press(&runtime, named(&runtime, &view, "November 20, 2026").bounds);
    assert!(!is_open(&runtime, &view));
    assert_eq!(view.due.get(), Date::new(2026, 11, 20));
}

#[test]
fn a_disabled_date_picker_shows_its_day_and_does_not_open() {
    let runtime = runtime();
    let view = Field::on(2026, 11, 12);
    view.locked.set(true);
    let button = the_button(&runtime, &view);
    assert!(!button.enabled);
    assert_eq!(button.value.as_deref(), Some("November 12, 2026"));
    press(&runtime, button.bounds);
    assert!(!is_open(&runtime, &view));
    assert!(key(&runtime, Key::Tab));
    assert_eq!(keyboard(&runtime, &view).as_deref(), Some("Note"));
}

#[test]
fn they_lower_to_the_web_and_a_tool_sees_only_the_apps_lines() {
    let page = bunny_ui::ssr::render(&Inline::on(2026, 11, 12), SIZE);
    assert!(page.html.contains("November 2026") && !page.html.contains("<canvas"));
    let page = bunny_ui::ssr::render(&Field::on(2026, 11, 12), SIZE);
    assert!(page.html.contains("November 12, 2026") && !page.html.contains("<canvas"));

    for (scene, kind) in [("calendar_sites", NodeKind::Calendar), ("date_picker_sites", NodeKind::DatePicker)] {
        let runtime = Runtime::scene(scene);
        runtime.record_sites(true);
        if kind == NodeKind::Calendar {
            runtime.display_frame(&Inline::on(2026, 11, 12), SIZE);
        } else {
            let view = Field::on(2026, 11, 12);
            runtime.display_frame(&view, SIZE);
            // the open list is the framework's too
            runtime.offer_key(KeyPattern::key(Key::Tab));
            runtime.offer_key(KeyPattern::key(Key::Char(' ')));
            runtime.display_frame(&view, SIZE);
        }
        let sites = runtime.sites();
        runtime.record_sites(false);
        assert!(sites.iter().any(|site| site.kind == kind), "{kind:?}");
        for site in &sites {
            if let Site::Caller(location) = site.site {
                // a path reads with either separator, by the platform that built it
                let file = location.file().replace('\\', "/");
                assert!(file.ends_with("tests/calendar.rs"), "{:?} at {location}", site.kind);
            }
        }
    }
}
