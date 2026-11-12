//! `Date`: a day of the Gregorian calendar, and its names.

use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

/// The calendar by the rule a person knows, with no arithmetic: the day
/// after a day.
fn day_after(year: i32, month: u8, day: u8) -> (i32, u8, u8) {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day < last {
        (year, month, day + 1)
    } else if month < 12 {
        (year, month + 1, 1)
    } else {
        (year + 1, 1, 1)
    }
}

#[test]
fn every_day_of_three_thousand_years_is_one_day_after_the_day_before() {
    let (mut year, mut month, mut day) = (1, 1, 1);
    let mut number = Date::MIN.unix_days();
    // 0001-01-01 is a Monday by the rules of today's calendar
    let mut weekday = Weekday::Monday;
    while year <= 3000 {
        let date = Date::checked(year, month, day).expect("a day the walk reached is a real day");
        assert_eq!((date.year(), date.month(), date.day()), (year, month, day));
        assert_eq!(date.unix_days(), number, "{date}");
        assert_eq!(Date::from_unix_days(number), date, "{date}");
        assert_eq!(date.weekday(), weekday, "{date}");
        (year, month, day) = day_after(year, month, day);
        number += 1;
        weekday = weekday.adding(1);
    }
}

#[test]
fn the_days_a_person_can_check() {
    assert_eq!(Date::new(1970, 1, 1).unix_days(), 0);
    assert_eq!(Date::new(1970, 1, 1).weekday(), Weekday::Thursday);
    assert_eq!(Date::new(2000, 1, 1).weekday(), Weekday::Saturday);
    assert_eq!(Date::new(2026, 11, 12).weekday(), Weekday::Thursday);
    assert_eq!(Date::new(1969, 7, 20).weekday(), Weekday::Sunday);
    assert_eq!(Date::new(2026, 11, 12).unix_days(), 20_769);
    assert_eq!(Date::MAX.adding_days(-(Date::MAX.days_since(Date::MIN))), Date::MIN);

    assert!(Date::is_leap_year(2024) && Date::is_leap_year(2000));
    assert!(!Date::is_leap_year(2026) && !Date::is_leap_year(1900) && !Date::is_leap_year(2100));
    assert_eq!(Date::new(2024, 2, 1).days_in_month(), 29);
    assert_eq!(Date::new(2026, 2, 1).days_in_month(), 28);
    assert_eq!(Date::new(2026, 4, 1).days_in_month(), 30);
    assert_eq!(Date::new(2026, 12, 1).days_in_month(), 31);
}

#[test]
fn a_part_out_of_range_moves_to_the_nearest_real_day_and_checked_refuses_it() {
    assert_eq!(Date::new(2026, 2, 30), Date::new(2026, 2, 28));
    assert_eq!(Date::new(2024, 2, 30), Date::new(2024, 2, 29));
    assert_eq!(Date::new(2026, 4, 31), Date::new(2026, 4, 30));
    assert_eq!(Date::new(2026, 0, 0), Date::new(2026, 1, 1));
    assert_eq!(Date::new(2026, 13, 40), Date::new(2026, 12, 31));
    assert_eq!(Date::new(0, 1, 1), Date::MIN);
    assert_eq!(Date::new(-44, 3, 15), Date::new(1, 3, 15));
    assert_eq!(Date::new(12_000, 12, 31), Date::MAX);

    assert_eq!(Date::checked(2026, 2, 28), Some(Date::new(2026, 2, 28)));
    for (year, month, day) in [(2026, 2, 29), (2026, 0, 1), (2026, 13, 1), (2026, 1, 0), (2026, 1, 32), (0, 1, 1), (10_000, 1, 1)] {
        assert_eq!(Date::checked(year, month, day), None, "{year}-{month}-{day}");
    }
}

#[test]
fn days_and_months_are_added_and_stop_at_the_ends() {
    let day = Date::new(2026, 11, 12);
    assert_eq!(day.adding_days(30), Date::new(2026, 12, 12));
    assert_eq!(day.adding_days(-12), Date::new(2026, 10, 31));
    assert_eq!(day.adding_days(365), Date::new(2027, 11, 12));
    assert_eq!(day.adding_days(0), day);
    assert_eq!(Date::new(2027, 1, 1).days_since(day), 50);
    assert_eq!(day.days_since(Date::new(2027, 1, 1)), -50);

    assert_eq!(day.adding_months(3), Date::new(2027, 2, 12));
    assert_eq!(day.adding_months(-11), Date::new(2025, 12, 12));
    assert_eq!(day.adding_months(-12), Date::new(2025, 11, 12));
    assert_eq!(Date::new(2026, 1, 31).adding_months(1), Date::new(2026, 2, 28), "the month's last day");
    assert_eq!(Date::new(2024, 1, 31).adding_months(1), Date::new(2024, 2, 29));
    assert_eq!(Date::new(2026, 3, 31).adding_months(-1), Date::new(2026, 2, 28));
    assert_eq!(Date::new(2024, 2, 29).adding_months(12), Date::new(2025, 2, 28));
    assert_eq!(day.first_of_month(), Date::new(2026, 11, 1));

    assert_eq!(Date::MAX.adding_days(1), Date::MAX);
    assert_eq!(Date::MIN.adding_days(-1), Date::MIN);
    assert_eq!(day.adding_days(i64::MAX), Date::MAX);
    assert_eq!(day.adding_days(i64::MIN), Date::MIN);
    assert_eq!(Date::MAX.adding_months(1), Date::MAX);
    assert_eq!(Date::MIN.adding_months(-1), Date::MIN);
    assert_eq!(day.adding_months(i32::MAX), Date::MAX);
    assert_eq!(day.adding_months(i32::MIN), Date::MIN);
}

#[test]
fn dates_compare_in_the_order_of_time() {
    let mut days = [Date::new(2026, 11, 12), Date::new(2025, 12, 31), Date::new(2026, 2, 28), Date::new(2026, 11, 2)];
    days.sort();
    assert_eq!(days.map(|day| day.to_string()), ["2025-12-31", "2026-02-28", "2026-11-02", "2026-11-12"]);
    assert!(Date::MIN < Date::MAX);
}

#[test]
fn a_date_prints_and_reads_as_year_month_day() {
    assert_eq!(Date::new(2026, 3, 5).to_string(), "2026-03-05");
    assert_eq!(Date::new(800, 12, 25).to_string(), "0800-12-25");
    assert_eq!("2026-03-05".parse(), Ok(Date::new(2026, 3, 5)));
    assert_eq!("0800-12-25".parse(), Ok(Date::new(800, 12, 25)));
    for text in ["", "2026-3-5", "2026/03/05", "2026-02-30", "2026-13-01", "0000-01-01", "20260305", "2026-03-05 ", "２０２６-03-05", "+026-03-05"] {
        assert!(text.parse::<Date>().is_err(), "{text:?}");
    }
    assert!(!"x".parse::<Date>().unwrap_err().to_string().is_empty());
}

#[test]
fn a_week_has_columns_from_its_first_day() {
    assert_eq!(Weekday::Sunday.index(), 0);
    assert_eq!(Weekday::Saturday.index(), 6);
    assert_eq!(Weekday::Saturday.adding(1), Weekday::Sunday);
    assert_eq!(Weekday::Monday.adding(13), Weekday::Sunday);
    assert_eq!(Weekday::Sunday.column(Weekday::Sunday), 0);
    assert_eq!(Weekday::Sunday.column(Weekday::Monday), 6);
    assert_eq!(Weekday::Wednesday.column(Weekday::Monday), 2);
    assert_eq!(Weekday::Friday.column(Weekday::Saturday), 6);
    for first in Weekday::ALL {
        let mut columns: Vec<_> = Weekday::ALL.iter().map(|day| day.column(first)).collect();
        columns.sort_unstable();
        assert_eq!(columns, [0, 1, 2, 3, 4, 5, 6]);
    }
}

#[test]
fn a_week_starts_on_the_day_the_readers_region_starts_it() {
    let first = |tag: &str| Weekday::first_for(&Locale::new(tag));
    assert_eq!(first("en-US"), Weekday::Sunday);
    assert_eq!(first("en-GB"), Weekday::Monday);
    assert_eq!(first("en-AU"), Weekday::Monday);
    assert_eq!(first("pt-BR"), Weekday::Sunday);
    assert_eq!(first("es-ES"), Weekday::Monday);
    assert_eq!(first("es-MX"), Weekday::Sunday);
    assert_eq!(first("fr-CA"), Weekday::Sunday);
    assert_eq!(first("de-DE"), Weekday::Monday);
    assert_eq!(first("ja-JP"), Weekday::Sunday);
    assert_eq!(first("zh-Hant-TW"), Weekday::Sunday);
    assert_eq!(first("ar-EG"), Weekday::Saturday);
    assert_eq!(first("ar-AE"), Weekday::Monday);
    assert_eq!(first("dv-MV"), Weekday::Friday);
    // a language alone answers for where most of its readers are
    assert_eq!(first("en"), Weekday::Sunday);
    assert_eq!(first("pt"), Weekday::Sunday);
    assert_eq!(first("fr"), Weekday::Monday);
    assert_eq!(first("ru"), Weekday::Monday);
    assert_eq!(first("ko"), Weekday::Sunday);
    assert_eq!(first("ar"), Weekday::Saturday);
    assert_eq!(first("eo"), Weekday::Monday);
    assert_eq!(Weekday::first_for(&Locale::default()), Weekday::Sunday);
    assert_eq!(DateNames::for_locale(&Locale::new("nl-NL")).first_weekday(), Weekday::Monday);
}

/// What each language writes for 5 March 2026: the title of its month,
/// and the date in full. The words are the ones the macOS calendar data
/// gives for the same day; Russian and Polish are written with numbers.
const WRITTEN: [(&str, &str, &str); 16] = [
    ("en", "March 2026", "March 5, 2026"),
    ("pt-BR", "março de 2026", "5 de março de 2026"),
    ("pt-PT", "março de 2026", "5 de março de 2026"),
    ("es", "marzo de 2026", "5 de marzo de 2026"),
    ("fr", "mars 2026", "5 mars 2026"),
    ("de", "März 2026", "5. März 2026"),
    ("it", "marzo 2026", "5 marzo 2026"),
    ("ja", "2026年3月", "2026年3月5日"),
    ("zh-CN", "2026年3月", "2026年3月5日"),
    ("zh-TW", "2026年3月", "2026年3月5日"),
    ("ko", "2026년 3월", "2026년 3월 5일"),
    ("nl", "maart 2026", "5 maart 2026"),
    ("ru", "март 2026 г.", "05.03.2026"),
    ("tr", "Mart 2026", "5 Mart 2026"),
    ("pl", "marzec 2026", "5.03.2026"),
    ("sv", "mars 2026", "5 mars 2026"),
];

#[test]
fn each_language_writes_a_date_its_own_way() {
    let day = Date::new(2026, 3, 5);
    let mut tags = Vec::new();
    for (tag, title, full) in WRITTEN {
        let names = DateNames::for_locale(&Locale::new(tag));
        assert_eq!(names.month_title(day), title, "{tag}");
        assert_eq!(names.date(day), full, "{tag}");
        tags.push(names.tag());
    }
    tags.dedup();
    assert_eq!(tags.len(), 16, "each locale found a table of its own");

    let english = DateNames::for_locale(&Locale::new("eo"));
    assert_eq!(english.tag(), "en", "a language with no table reads in English");
    assert_eq!(english.date(Date::new(2026, 11, 12)), "November 12, 2026");
    assert_eq!(english.date(Date::new(800, 12, 25)), "December 25, 800");
}

#[test]
fn months_and_days_have_their_names() {
    let english = DateNames::for_locale(&Locale::new("en-US"));
    assert_eq!(english.month(1), "January");
    assert_eq!(english.month(12), "December");
    assert_eq!(english.month(0), "January", "a number out of range reads as the nearest month");
    assert_eq!(english.month(13), "December");
    assert_eq!(Weekday::ALL.map(|day| english.weekday(day)), ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]);

    let german = DateNames::for_locale(&Locale::new("de-DE"));
    assert_eq!(german.month(3), "März");
    assert_eq!(Weekday::ALL.map(|day| german.weekday(day)), ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"]);
    assert_eq!(DateNames::for_locale(&Locale::new("ja")).weekday(Weekday::Wednesday), "水");
    assert_eq!(DateNames::for_locale(&Locale::new("pt-BR")).weekday(Weekday::Saturday), "sáb");
}
