//! The built-in controls on one page. Each control is bound to a value,
//! and the line under each group shows what the bindings hold.
//!
//! cargo run -p bunny-ui --example controls
//!
//! Tab goes from control to control, and the arrows move inside one:
//! `docs/controls.md` and `docs/keyboard-controls.md` have the keys.

use bunny_ui::prelude::*;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Choices,
    Numbers,
    Words,
    Dates,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scale {
    Small,
    Medium,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fruit {
    Apple,
    Cherry,
    Pear,
    Plum,
    Quince,
}

const TREES: [&str; 12] = [
    "Alder", "Ash", "Beech", "Birch", "Cedar", "Elm", "Hazel", "Larch", "Maple", "Oak", "Rowan", "Willow",
];

/// One value out of two, three, or many.
#[derive(Clone, Copy)]
struct Choices {
    on: State<bool>,
    checked: State<bool>,
    scale: State<Scale>,
    fruit: State<Fruit>,
}

impl Component for Choices {
    fn body(self) -> impl View {
        let held = format!(
            "{} · {} · {:?} · {:?}",
            if self.on.get() { "on" } else { "off" },
            if self.checked.get() { "checked" } else { "not checked" },
            self.scale.get(),
            self.fruit.get(),
        );
        vstack!(
            form!(
                toggle(text("Toggle"), self.on.binding()),
                labeled("Checkbox", checkbox(text("Checked"), self.checked.binding())),
                labeled(
                    "Segmented",
                    segmented(
                        self.scale.binding(),
                        [(Scale::Small, "Small"), (Scale::Medium, "Medium"), (Scale::Large, "Large")],
                    ),
                ),
                labeled(
                    "Picker",
                    picker(
                        self.fruit.binding(),
                        [
                            (Fruit::Apple, "Apple"),
                            (Fruit::Cherry, "Cherry"),
                            (Fruit::Pear, "Pear"),
                            (Fruit::Plum, "Plum"),
                            (Fruit::Quince, "Quince"),
                        ],
                    ),
                ),
            ),
            text(held).foreground_color(theme::current().fg_secondary),
            spacer(),
        )
        .spacing(12.0)
    }
}

/// A number moved one step at a time, and one moved along a line.
#[derive(Clone, Copy)]
struct Numbers {
    guests: State<i32>,
    level: State<f64>,
}

impl Component for Numbers {
    fn body(self) -> impl View {
        // a range that does not go up is a mistake of the page, not of the reader
        let percent = SliderRange::new(0.0..=100.0)
            .and_then(|range| range.with_step(5.0))
            .expect("0 to 100 in steps of 5 is a range");
        let held = format!("{} guests · {:.0} %", self.guests.get(), self.level.get());
        vstack!(
            form!(
                labeled("Stepper", stepper(self.guests.binding(), 1..=8)),
                labeled("Slider", slider(self.level.binding(), percent).frame_width(180.0)),
            ),
            text(held).foreground_color(theme::current().fg_secondary),
            spacer(),
        )
        .spacing(12.0)
    }
}

/// A line of text, and a query that filters a list.
#[derive(Clone, Copy)]
struct Words {
    name: State<String>,
    query: State<String>,
}

impl Component for Words {
    fn body(self) -> impl View {
        // the search field filters nothing: the page reads the same binding
        let query = self.query.get().to_lowercase();
        let found: Vec<_> = TREES.iter().filter(|tree| tree.to_lowercase().contains(&query)).map(|tree| text(*tree)).collect();
        let count = format!("{} of {}", found.len(), TREES.len());
        vstack!(
            form!(labeled("Text field", text_field("Jane Appleseed", self.name.binding()))),
            search_field(self.query.binding()).prompt("Find a tree"),
            text(count).foreground_color(theme::current().fg_secondary),
            vstack(found).spacing(4.0).alignment(HorizontalAlignment::Leading),
            spacer(),
        )
        .spacing(12.0)
        .alignment(HorizontalAlignment::Leading)
    }
}

/// A day, chosen in a list or on the page.
#[derive(Clone, Copy)]
struct Dates {
    day: State<Date>,
}

impl Component for Dates {
    fn body(self) -> impl View {
        let day = self.day.get();
        let held = format!("{day} · {:?}", day.weekday());
        vstack!(
            form!(labeled("Date picker", date_picker(self.day.binding()))),
            // the two show one binding: a day chosen in one is the day of the other
            calendar(self.day.binding()),
            text(held).foreground_color(theme::current().fg_secondary),
            spacer(),
        )
        .spacing(12.0)
    }
}

#[derive(Clone, Copy)]
struct Gallery {
    page: State<Page>,
    choices: Choices,
    numbers: Numbers,
    words: Words,
    dates: Dates,
}

impl Component for Gallery {
    fn body(self) -> impl View {
        // a page is on the scene only while its tab is the one shown; the
        // values live here, above the tabs, so a page finds them again
        tab_view(
            self.page.binding(),
            (
                tab(Page::Choices, "Choices", self.choices.padding_length(16.0)),
                tab(Page::Numbers, "Numbers", self.numbers.padding_length(16.0)),
                tab(Page::Words, "Words", self.words.padding_length(16.0)),
                tab(Page::Dates, "Dates", self.dates.padding_length(16.0)),
            ),
        )
    }
}

fn home() -> impl View {
    Gallery {
        page: State::new(Page::Choices),
        choices: Choices {
            on: State::new(true),
            checked: State::new(false),
            scale: State::new(Scale::Medium),
            fruit: State::new(Fruit::Pear),
        },
        numbers: Numbers { guests: State::new(2), level: State::new(40.0) },
        words: Words { name: State::new(String::new()), query: State::new(String::new()) },
        dates: Dates { day: State::new(Date::new(2026, 11, 12)) },
    }
}

bunny_ui::app!(home, bunny_ui::AppConfig::new().title("Controls").size(460.0, 560.0));

fn main() {
    run()
}
