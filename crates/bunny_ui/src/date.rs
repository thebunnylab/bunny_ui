//! A day of the calendar, and its names in the reader's language.
//!
//! ```ignore
//! let due = Date::new(2026, 11, 12);
//! due.weekday();                  // Weekday::Thursday
//! due.adding_days(30);            // 2026-12-12
//! due.adding_months(3);           // 2027-02-12
//!
//! let names = DateNames::for_locale(&environment::<Locale>());
//! names.date(due);                // "November 12, 2026", "12 de novembro de 2026"
//! names.month_title(due);         // "November 2026", "2026年11月"
//! ```
//!
//! A [`Date`] is a year, a month and a day of the Gregorian calendar. It
//! has no hour and no time zone: it is the day a person writes on a form,
//! not an instant. Every `Date` is a real day. [`Date::new`] moves a
//! month or a day that does not exist to the nearest one that does, and
//! [`Date::checked`] answers `None` for it instead.
//!
//! The calendar is the proleptic Gregorian one: its rules apply to every
//! year from 1 to 9999, also to the years before the calendar was in
//! use.
//!
//! ## Today
//!
//! The framework has no clock of the wall. The day in UTC comes from the
//! system's time:
//!
//! ```ignore
//! let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
//! let today = Date::from_unix_days((seconds / 86_400) as i64);
//! ```
//!
//! The day where the reader is needs the time zone of the device, and
//! that is not here yet.
//!
//! ## Names
//!
//! [`DateNames`] has the twelve months, the seven days of the week and
//! the two ways a date is written, in the sixteen languages of the
//! framework's own words ([`crate::words`]). The languages disagree on
//! the order and on the words between the parts, so a date is written
//! from a pattern of its language: "March 5, 2026", "5 de março de
//! 2026", "2026年3月5日".
//!
//! Russian and Polish change the name of a month inside a date ("март",
//! but "5 марта"). The tables hold one name for each month, the one that
//! stands alone, and those two languages write a date with numbers:
//! "05.03.2026".
//!
//! [`DateNames::first_weekday`] is the day a week starts on where the
//! reader is: Sunday in the United States and Brazil, Monday in most of
//! Europe, Saturday in Egypt.

use std::fmt::{self, Write};
use std::str::FromStr;

use motor::state::Locale;

use crate::catalog::{Catalog, Key, Strings, Table};

/// 1970-01-01 counted from 0000-03-01, the day the arithmetic starts on.
const EPOCH_SHIFT: i64 = 719_468;
/// The days in 400 years, after which the calendar repeats.
const ERA: i64 = 146_097;

/// A day of the Gregorian calendar ([the module](self) has the rules).
///
/// Dates compare in the order of time: an earlier day is less.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

impl Date {
    /// The first day there is: 0001-01-01.
    pub const MIN: Date = Date { year: 1, month: 1, day: 1 };
    /// The last day there is: 9999-12-31.
    pub const MAX: Date = Date { year: 9999, month: 12, day: 31 };

    /// The day `year`-`month`-`day`, with January as month 1.
    ///
    /// A part out of its range moves to the nearest end of the range: a
    /// year to 1 or 9999, a month to 1 or 12, a day to 1 or to the last
    /// day of the month. `Date::new(2026, 2, 30)` is 28 February 2026.
    /// [`Date::checked`] refuses such a day instead.
    pub const fn new(year: i32, month: u8, day: u8) -> Date {
        let year = if year < 1 {
            1
        } else if year > 9999 {
            9999
        } else {
            year
        };
        let month = if month < 1 {
            1
        } else if month > 12 {
            12
        } else {
            month
        };
        let last = days_in(year, month);
        let day = if day < 1 {
            1
        } else if day > last {
            last
        } else {
            day
        };
        Date { year, month, day }
    }

    /// The day `year`-`month`-`day`, or `None` when the calendar has no
    /// such day.
    pub const fn checked(year: i32, month: u8, day: u8) -> Option<Date> {
        if year < 1 || year > 9999 || month < 1 || month > 12 || day < 1 || day > days_in(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    /// The year, 1 to 9999.
    pub const fn year(self) -> i32 {
        self.year
    }

    /// The month, 1 (January) to 12.
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The day of the month, 1 to 31.
    pub const fn day(self) -> u8 {
        self.day
    }

    /// The day of the week.
    pub const fn weekday(self) -> Weekday {
        // 1970-01-01 was a Thursday
        Weekday::from_index((self.unix_days() + 4).rem_euclid(7) as u8)
    }

    /// How many days the month of this date has.
    pub const fn days_in_month(self) -> u8 {
        days_in(self.year, self.month)
    }

    /// Is `year` a leap year?
    pub const fn is_leap_year(year: i32) -> bool {
        year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    }

    /// The first day of this date's month.
    pub const fn first_of_month(self) -> Date {
        Date { year: self.year, month: self.month, day: 1 }
    }

    /// The date `days` later, or earlier for a negative count. It stops
    /// at [`Date::MIN`] and at [`Date::MAX`].
    pub const fn adding_days(self, days: i64) -> Date {
        Date::from_unix_days(self.unix_days().saturating_add(days))
    }

    /// The same day `months` later, or earlier for a negative count. A
    /// day the new month does not have becomes its last day: 31 January
    /// and one month is 28 February. It stops at [`Date::MIN`] and at
    /// [`Date::MAX`].
    pub const fn adding_months(self, months: i32) -> Date {
        let count = self.year as i64 * 12 + self.month as i64 - 1 + months as i64;
        let year = count.div_euclid(12);
        if year < 1 {
            return Date::MIN;
        }
        if year > 9999 {
            return Date::MAX;
        }
        Date::new(year as i32, count.rem_euclid(12) as u8 + 1, self.day)
    }

    /// How many days this date is after `earlier`; negative when it is
    /// before.
    pub const fn days_since(self, earlier: Date) -> i64 {
        self.unix_days() - earlier.unix_days()
    }

    /// The days from 1970-01-01 to this date; negative before it.
    pub const fn unix_days(self) -> i64 {
        // the year starts in March, so the leap day is its last day
        let year = self.year as i64 - (self.month <= 2) as i64;
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let month_from_march = (self.month as i64 + 9) % 12;
        let day_of_year = (153 * month_from_march + 2) / 5 + self.day as i64 - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * ERA + day_of_era - EPOCH_SHIFT
    }

    /// The date `days` after 1970-01-01. A count out of the calendar
    /// stops at [`Date::MIN`] or at [`Date::MAX`].
    pub const fn from_unix_days(days: i64) -> Date {
        if days <= Date::MIN.unix_days() {
            return Date::MIN;
        }
        if days >= Date::MAX.unix_days() {
            return Date::MAX;
        }
        let days = days + EPOCH_SHIFT;
        let era = days.div_euclid(ERA);
        let day_of_era = days.rem_euclid(ERA);
        let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / (ERA - 1)) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_from_march = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
        let month = if month_from_march < 10 { month_from_march + 3 } else { month_from_march - 9 };
        let year = era * 400 + year_of_era + (month <= 2) as i64;
        Date { year: year as i32, month: month as u8, day: day as u8 }
    }
}

const fn days_in(year: i32, month: u8) -> u8 {
    match month {
        2 if Date::is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `2026-11-12`: the year in four digits, the month and the day in two.
impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// The text is not a date written as `2026-11-12`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseDateError;

impl fmt::Display for ParseDateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a date is written as 2026-11-12, and it must be a day of the calendar")
    }
}

impl std::error::Error for ParseDateError {}

/// Reads what [`Date`] prints: `2026-11-12`, and nothing else.
impl FromStr for Date {
    type Err = ParseDateError;

    fn from_str(text: &str) -> Result<Date, ParseDateError> {
        let bytes = text.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return Err(ParseDateError);
        }
        let number = |from: usize, to: usize| -> Option<u32> {
            bytes[from..to].iter().try_fold(0u32, |sum, byte| byte.is_ascii_digit().then(|| sum * 10 + u32::from(byte - b'0')))
        };
        let (Some(year), Some(month), Some(day)) = (number(0, 4), number(5, 7), number(8, 10)) else {
            return Err(ParseDateError);
        };
        Date::checked(year as i32, month as u8, day as u8).ok_or(ParseDateError)
    }
}

/// A day of the week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weekday {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

impl Weekday {
    /// The seven days, Sunday first.
    pub const ALL: [Weekday; 7] = [
        Weekday::Sunday,
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
    ];

    /// The day's number, with Sunday as 0.
    pub const fn index(self) -> u8 {
        self as u8
    }

    const fn from_index(index: u8) -> Weekday {
        Weekday::ALL[(index % 7) as usize]
    }

    /// The day `days` after this one.
    pub const fn adding(self, days: u8) -> Weekday {
        Weekday::from_index(self.index() + days % 7)
    }

    /// The column of this day in a week that starts on `first`: 0 for
    /// `first` itself, 6 for the day before it.
    pub const fn column(self, first: Weekday) -> u8 {
        (self.index() + 7 - first.index()) % 7
    }

    /// The day a week starts on for `locale`: by its region, or by the
    /// region where most readers of its language are when it names none.
    pub fn first_for(locale: &Locale) -> Weekday {
        match locale.region() {
            Some(region) => first_in_region(region),
            None => first_in_language(locale.language()),
        }
    }
}

/// The regions whose week starts on a day that is not Monday, as the
/// Unicode week data lists them. Every other region starts on Monday.
const SUNDAY: &str = "AG AS BD BR BS BT BW BZ CA CN CO DM DO ET GT GU HK HN ID IL IN JM JO JP KE KH KR LA MH MM MO MT \
                      MX MZ NI NP PA PE PH PK PR PT PY SA SG SV TH TT TW UM US VE VI WS YE ZA ZW";
const SATURDAY: &str = "AF BH DJ EG IQ IR KW LY OM QA SD SY";
const FRIDAY: &str = "MV";

fn first_in_region(region: &str) -> Weekday {
    let listed = |regions: &str| regions.split(' ').any(|known| known == region);
    if listed(SUNDAY) {
        Weekday::Sunday
    } else if listed(SATURDAY) {
        Weekday::Saturday
    } else if listed(FRIDAY) {
        Weekday::Friday
    } else {
        Weekday::Monday
    }
}

/// A language with no region answers for the region most of its readers
/// are in: English for the United States, Portuguese for Brazil, Arabic
/// for Egypt.
fn first_in_language(language: &str) -> Weekday {
    match language {
        "en" | "pt" | "ja" | "ko" | "zh" | "he" | "hi" | "id" | "th" | "fil" | "bn" | "ur" | "ne" | "km" | "lo"
        | "my" | "am" | "mt" => Weekday::Sunday,
        "ar" | "fa" | "ps" => Weekday::Saturday,
        "dv" => Weekday::Friday,
        _ => Weekday::Monday,
    }
}

/// How many names a language has here: twelve months, seven days and two
/// patterns.
pub const DATE_NAMES: usize = 21;

const MONTHS: u8 = 0;
const WEEKDAYS: u8 = 12;
const MONTH_TITLE: u8 = 19;
const FULL_DATE: u8 = 20;

/// A row of a table of names.
#[derive(Clone, Copy)]
struct Name(u8);

impl Key for Name {
    const COUNT: usize = DATE_NAMES;

    fn index(self) -> usize {
        self.0 as usize
    }
}

/// One language's names.
pub type DateTable = Table<DATE_NAMES>;

// Every table lists the names in this order, on these four lines:
//   the months, January first
//   the days of the week, Sunday first, as short as a calendar's heading
//   the month and the year, as the title of a calendar
//   the date in full
// A pattern holds `{y}` for the year, `{month}` for the month's name,
// `{m}` and `{mm}` for its number without and with a leading zero, and
// `{d}` and `{dd}` for the day.
#[rustfmt::skip]
const EN: DateTable = Table { tag: "en", entries: [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
    "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat",
    "{month} {y}",
    "{month} {d}, {y}",
]};

#[rustfmt::skip]
const PT: DateTable = Table { tag: "pt", entries: [
    "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro",
    "dom", "seg", "ter", "qua", "qui", "sex", "sáb",
    "{month} de {y}",
    "{d} de {month} de {y}",
]};

#[rustfmt::skip]
const PT_PT: DateTable = Table { tag: "pt-PT", entries: [
    "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro",
    "dom", "seg", "ter", "qua", "qui", "sex", "sáb",
    "{month} de {y}",
    "{d} de {month} de {y}",
]};

#[rustfmt::skip]
const ES: DateTable = Table { tag: "es", entries: [
    "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre",
    "dom", "lun", "mar", "mié", "jue", "vie", "sáb",
    "{month} de {y}",
    "{d} de {month} de {y}",
]};

#[rustfmt::skip]
const FR: DateTable = Table { tag: "fr", entries: [
    "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre",
    "dim", "lun", "mar", "mer", "jeu", "ven", "sam",
    "{month} {y}",
    "{d} {month} {y}",
]};

#[rustfmt::skip]
const DE: DateTable = Table { tag: "de", entries: [
    "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember",
    "So", "Mo", "Di", "Mi", "Do", "Fr", "Sa",
    "{month} {y}",
    "{d}. {month} {y}",
]};

#[rustfmt::skip]
const IT: DateTable = Table { tag: "it", entries: [
    "gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre",
    "dom", "lun", "mar", "mer", "gio", "ven", "sab",
    "{month} {y}",
    "{d} {month} {y}",
]};

#[rustfmt::skip]
const JA: DateTable = Table { tag: "ja", entries: [
    "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
    "日", "月", "火", "水", "木", "金", "土",
    "{y}年{m}月",
    "{y}年{m}月{d}日",
]};

#[rustfmt::skip]
const ZH_HANS: DateTable = Table { tag: "zh-Hans", entries: [
    "一月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "十一月", "十二月",
    "日", "一", "二", "三", "四", "五", "六",
    "{y}年{m}月",
    "{y}年{m}月{d}日",
]};

#[rustfmt::skip]
const ZH_HANT: DateTable = Table { tag: "zh-Hant", entries: [
    "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
    "日", "一", "二", "三", "四", "五", "六",
    "{y}年{m}月",
    "{y}年{m}月{d}日",
]};

#[rustfmt::skip]
const KO: DateTable = Table { tag: "ko", entries: [
    "1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월",
    "일", "월", "화", "수", "목", "금", "토",
    "{y}년 {m}월",
    "{y}년 {m}월 {d}일",
]};

#[rustfmt::skip]
const NL: DateTable = Table { tag: "nl", entries: [
    "januari", "februari", "maart", "april", "mei", "juni", "juli", "augustus", "september", "oktober", "november", "december",
    "zo", "ma", "di", "wo", "do", "vr", "za",
    "{month} {y}",
    "{d} {month} {y}",
]};

/// Russian writes a date with numbers here: the name of a month changes
/// inside a date, and the table holds the name that stands alone.
#[rustfmt::skip]
const RU: DateTable = Table { tag: "ru", entries: [
    "январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь",
    "Вс", "Пн", "Вт", "Ср", "Чт", "Пт", "Сб",
    "{month} {y} г.",
    "{dd}.{mm}.{y}",
]};

#[rustfmt::skip]
const TR: DateTable = Table { tag: "tr", entries: [
    "Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık",
    "Paz", "Pzt", "Sal", "Çar", "Per", "Cum", "Cmt",
    "{month} {y}",
    "{d} {month} {y}",
]};

/// Polish writes a date with numbers here, for the reason Russian does.
#[rustfmt::skip]
const PL: DateTable = Table { tag: "pl", entries: [
    "styczeń", "luty", "marzec", "kwiecień", "maj", "czerwiec", "lipiec", "sierpień", "wrzesień", "październik", "listopad", "grudzień",
    "niedz", "pon", "wt", "śr", "czw", "pt", "sob",
    "{month} {y}",
    "{d}.{mm}.{y}",
]};

#[rustfmt::skip]
const SV: DateTable = Table { tag: "sv", entries: [
    "januari", "februari", "mars", "april", "maj", "juni", "juli", "augusti", "september", "oktober", "november", "december",
    "sön", "mån", "tis", "ons", "tors", "fre", "lör",
    "{month} {y}",
    "{d} {month} {y}",
]};

/// The sixteen tables, English first. They are the languages of
/// [`crate::words`], in its order.
pub static DATE_TABLES: [DateTable; 16] = [
    EN, PT, PT_PT, ES, FR, DE, IT, JA, ZH_HANS, ZH_HANT, KO, NL, RU, TR, PL, SV,
];

static CATALOG: Catalog<Name, DATE_NAMES> = Catalog::new(&DATE_TABLES);

/// The names of a date for one locale: a table picked one time
/// ([the module](self) has what the tables hold).
#[derive(Clone, Copy)]
pub struct DateNames {
    strings: Strings<Name, DATE_NAMES>,
    first: Weekday,
}

impl DateNames {
    /// The names for `locale`. A language with no table reads in
    /// English; the first day of the week is the locale's own in every
    /// language.
    pub fn for_locale(locale: &Locale) -> DateNames {
        DateNames { strings: CATALOG.pick(locale), first: Weekday::first_for(locale) }
    }

    /// The tag of the table that answered.
    pub fn tag(&self) -> &'static str {
        self.strings.tag()
    }

    /// The name of `month`, 1 (January) to 12, as it stands alone. A
    /// number out of that range reads as the nearest month.
    pub fn month(&self, month: u8) -> &'static str {
        self.strings.get(Name(MONTHS + month.clamp(1, 12) - 1))
    }

    /// The short name of `day`, as the heading of a calendar's column.
    pub fn weekday(&self, day: Weekday) -> &'static str {
        self.strings.get(Name(WEEKDAYS + day.index()))
    }

    /// The day a week starts on for the locale ([`Weekday::first_for`]).
    pub fn first_weekday(&self) -> Weekday {
        self.first
    }

    /// The month and the year of `date`, as the title of a calendar:
    /// "November 2026".
    pub fn month_title(&self, date: Date) -> String {
        self.write(self.strings.get(Name(MONTH_TITLE)), date)
    }

    /// `date` in full: "November 12, 2026".
    pub fn date(&self, date: Date) -> String {
        self.write(self.strings.get(Name(FULL_DATE)), date)
    }

    /// One pass over the pattern; each part is written in its place.
    fn write(&self, pattern: &str, date: Date) -> String {
        let mut out = String::with_capacity(pattern.len() + 16);
        let mut rest = pattern;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else {
                rest = &rest[open..];
                break;
            };
            // a write to a String does not fail
            let _ = match &after[..close] {
                "y" => write!(out, "{}", date.year),
                "month" => out.write_str(self.month(date.month)),
                "m" => write!(out, "{}", date.month),
                "mm" => write!(out, "{:02}", date.month),
                "d" => write!(out, "{}", date.day),
                "dd" => write!(out, "{:02}", date.day),
                other => write!(out, "{{{other}}}"),
            };
            rest = &after[close + 1..];
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One locale picks the same language for a date and for a word: the
    /// two catalogs hold the same tables in the same order.
    #[test]
    fn the_tables_are_the_languages_of_the_words() {
        let dates: Vec<_> = CATALOG.tags().collect();
        let words: Vec<_> = crate::words::CATALOG.tags().collect();
        assert_eq!(dates, words);
    }

    #[test]
    fn every_table_holds_every_name_and_two_patterns_with_a_year() {
        for table in &DATE_TABLES {
            for (index, entry) in table.entries.iter().enumerate() {
                assert!(!entry.is_empty(), "{}: name {index} is written", table.tag);
            }
            for pattern in [MONTH_TITLE, FULL_DATE] {
                let pattern = table.entries[pattern as usize];
                assert!(pattern.contains("{y}"), "{}: {pattern}", table.tag);
                assert!(pattern.contains("{month}") || pattern.contains("{m}") || pattern.contains("{mm}"), "{}", table.tag);
            }
            let full = table.entries[FULL_DATE as usize];
            assert!(full.contains("{d}") || full.contains("{dd}"), "{}: {full}", table.tag);
            // a calendar's heading has seven columns of one width
            for day in &table.entries[WEEKDAYS as usize..MONTH_TITLE as usize] {
                assert!(day.chars().count() <= 5, "{}: {day}", table.tag);
            }
        }
    }

    #[test]
    fn a_pattern_keeps_what_it_does_not_know() {
        let names = DateNames::for_locale(&Locale::default());
        let date = Date::new(2026, 3, 5);
        assert_eq!(names.write("{dd}/{mm}/{y} {era}", date), "05/03/2026 {era}");
        assert_eq!(names.write("{d} {month", date), "5 {month");
    }
}
